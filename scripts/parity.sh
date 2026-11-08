#!/usr/bin/env bash
# The parity tier (G11 in porting/verification-plan.md). It builds the Go oracle
# at the pinned commit 2e9c969 (branch go-oracle-final) in a scratch tree, then
# runs the frozen differential suites (conformance/tests at that commit) there.
#
# The suites resolve their Go inputs from their own tree, so they run in an
# extracted copy of 2e9c969 and not in this tree. That copy is a separate
# workspace, so it adds no test to the root workspace and TEST stays at 1371.
#
# Usage:
#   scripts/parity.sh build           extract the pinned tree and build the Go nodes
#   scripts/parity.sh dry-run         build, compile the suites, list them; starts no backend
#   scripts/parity.sh derive-overrides DIR  head mode: write the derived references (D-368)
#   scripts/parity.sh run [NAME...]   build, then run the named suites (default: all), with
#                                     ignored tests included; needs the backends of
#                                     storage/docker-compose.yml. A NAME may carry one test
#                                     filter: SUITE:only=TEST runs that test alone, and
#                                     SUITE:except=TEST runs the rest of SUITE (D-320)
#
# Environment:
#   PARITY_DIR    scratch root (default: target/parity)
#   PARITY_TREE   the Rust side the suites test. pinned (the default) runs the frozen
#                 suites in the pinned tree. head runs the metrics suites against this
#                 checkout's crates, with the Go oracle still built from 2e9c969 (D-325).
#                 Head mode takes only metrics NAMEs (metrics, metrics:only=TEST,
#                 metrics:except=TEST) and has no default run.
#   PARITY_DERIVED_DIR  head mode: the directory of the derived references (D-368). When it
#                 is set, the overlay's copy of the suite reads its four expected values
#                 from there, at build and at run time. derive-overrides writes it.
#
# Hermeticity: the tree is exactly `git archive 2e9c969`; Go runs with
# GOTOOLCHAIN=go1.27.0, the version go.mod names, through the module proxy, with
# -mod=readonly, so go.sum must match. Every built binary's SHA-256 is written to
# $PARITY_DIR/bin/SHA256SUMS.
set -euo pipefail

PIN=2e9c969
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
SCRATCH="${PARITY_DIR:-$ROOT/target/parity}"
SRC="$SCRATCH/src"
BIN="$SCRATCH/bin"
HEAD="$SCRATCH/head"

# The tree the suites run in, and the cargo target directory for it. Pinned mode
# keeps the target it always used. Head mode has its own, so the two never share
# build artifacts.
PARITY_TREE="${PARITY_TREE:-pinned}"
case "$PARITY_TREE" in
  pinned) TREE="$SRC"; TARGET="$SCRATCH/target" ;;
  head) TREE="$HEAD"; TARGET="$SCRATCH/head-target" ;;
  *) echo "parity: PARITY_TREE must be pinned or head, not $PARITY_TREE" >&2; exit 2 ;;
esac

# shellcheck source=backends.env
. "$ROOT/scripts/backends.env"

cmd="${1:-}"
case "$cmd" in
  build|dry-run|run|derive-overrides) shift ;;
  *) echo "usage: $0 build | dry-run | run [NAME...] | derive-overrides DIR" >&2; exit 2 ;;
esac

if ! git cat-file -e "$PIN^{commit}" 2>/dev/null; then
  echo "parity: commit $PIN is not in this clone; fetch it (git fetch --unshallow, or fetch-depth: 0)" >&2
  exit 2
fi

# Each CI step that runs one suite calls this script again. The pinned tree is
# extracted once per scratch root, so the later steps reuse it and cargo does
# not rebuild the pinned workspace for every suite.
extract_pinned_tree() {
  if [ -f "$SCRATCH/extracted-$PIN" ] && [ -d "$SRC" ]; then
    return 0
  fi
  rm -rf "$SRC" "$BIN"
  mkdir -p "$SRC" "$BIN"
  git archive "$PIN" | tar -x -C "$SRC"
  touch "$SCRATCH/extracted-$PIN"
}

# The head overlay (D-325). It is the pinned tree, which keeps the Go module,
# conformance/ and contracts/, with this checkout's Rust workspace in its place. The
# pinned conformance crate joins the workspace as a member, so the frozen suites link
# to this checkout's crates. The overlay is rebuilt on each call from the working
# tree. cp -a keeps the file times, so cargo does not rebuild unchanged crates.
extract_head_tree() {
  rm -rf "$HEAD"
  mkdir -p "$HEAD"
  cp -a "$SRC/." "$HEAD/"
  rm -rf "$HEAD/crates" "$HEAD/third_party" "$HEAD/Cargo.toml" "$HEAD/Cargo.lock" \
    "$HEAD/rust-toolchain.toml"
  cp -a "$ROOT/crates" "$ROOT/third_party" "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" \
    "$ROOT/rust-toolchain.toml" "$HEAD/"
  if ! grep -qx 'members = \["crates/\*"\]' "$HEAD/Cargo.toml"; then
    echo "parity: the head Cargo.toml has no members = [\"crates/*\"] line to add conformance to" >&2
    exit 2
  fi
  sed -i 's|^members = \["crates/\*"\]$|members = ["crates/*", "conformance"]|' "$HEAD/Cargo.toml"
  # The adapter (D-325; President ruling 2026-11-08, 15:16Z). The head's CF Score returns
  # ndcg, precision and recall as Option<f32> (D-324), and the frozen suite reads f32. The
  # frozen file is not edited. The overlay's copy reads each metric through head_metric: a
  # value passes through, and a None fails with the metric's name. A None case gets an exact
  # expected-value override only with attribution in PORTING.md, never a skip or a wider
  # band. No override is recorded yet.
  local suite="$HEAD/conformance/tests/metrics.rs"
  sed -i -E 's/(^|[^A-Za-z0-9_])score\.(ndcg|precision|recall)\b/\1head_metric(score.\2, "\2")/g' "$suite"
  cat >> "$suite" <<'EOF'

// Head-mode adapter (D-325, scripts/parity.sh). Not part of the frozen suite.
trait HeadMetric {
    fn value(self, name: &str) -> f32;
}

impl HeadMetric for f32 {
    fn value(self, _name: &str) -> f32 {
        self
    }
}

impl HeadMetric for Option<f32> {
    fn value(self, name: &str) -> f32 {
        self.unwrap_or_else(|| {
            panic!("head metric {name} is undefined (D-324); a None case needs a recorded override")
        })
    }
}

fn head_metric<M: HeadMetric>(value: M, name: &str) -> f32 {
    value.value(name)
}
EOF
  # The derived references (D-368). When PARITY_DERIVED_DIR is set, four expected values in
  # the overlay's copy of the suite come from derive-overrides, and no band changes:
  #   ml_1m_als: the literal 0.36 becomes the derived als-ml-1m NDCG, and its Go run is the
  #              derived als-ml-1m metrics (one job).
  #   pinterest_20_bpr: the commented literal 0.53 becomes the derived bpr-pinterest-20 NDCG.
  #   check_one_job_cf (ml_1m_bpr_one_job, pinterest_20_bpr_one_job): the Go run is the
  #              derived metrics of its configuration. A configuration with no derived file
  #              (ml-100k) keeps its Go run. ml_1m_bpr's literal 0.36 is not changed.
  if [ -n "${PARITY_DERIVED_DIR:-}" ]; then
    sed -i -E \
      -e '/^fn ml_1m_als\(\) \{/,/^}/ s/^        0\.36,$/        derived_ndcg("als-ml-1m"),/' \
      -e '/^fn ml_1m_als\(\) \{/,/^}/ s/oracle\("als-ml-1m"\)/derived_or("als-ml-1m", || oracle("als-ml-1m"))/' \
      -e '/^fn pinterest_20_bpr\(\) \{/,/^}/ s/^        0\.53,$/        derived_ndcg("bpr-pinterest-20"),/' \
      -e '/^fn check_one_job_cf\(/,/^}/ s/oracle_with_jobs\(config, 1\)/derived_or(config, || oracle_with_jobs(config, 1))/' \
      "$suite"
    cat >> "$suite" <<'EOF'

// Head-mode derived references (D-368, scripts/parity.sh derive-overrides). Not part of the
// frozen suite. PARITY_DERIVED_DIR names the directory that derive-overrides wrote: one
// <config>.json per configuration, the Go oracle's metrics on the dedup-derived test file.
fn derived_path(config: &str) -> std::path::PathBuf {
    let dir = std::env::var("PARITY_DERIVED_DIR").expect("PARITY_DERIVED_DIR names the derived references");
    Path::new(&dir).join(format!("{config}.json"))
}

fn derived_or<T: DeserializeOwned>(config: &str, run: impl FnOnce() -> T) -> T {
    if std::env::var_os("PARITY_DERIVED_DIR").is_some() && derived_path(config).exists() {
        let text = fs::read_to_string(derived_path(config)).expect("the derived reference is readable");
        serde_json::from_str(&text).expect("the derived reference is valid JSON")
    } else {
        run()
    }
}

fn derived_ndcg(config: &str) -> f32 {
    let go: GoCf = derived_or(config, || panic!("no derived reference for {config} (D-368)"));
    go.ndcg
}
EOF
  fi
  echo "parity: head overlay built at $HEAD (pinned conformance, this checkout's crates)"
}

# Head mode runs the metrics suites only (D-325). The other suites stay on the
# pinned snapshot, and ctr_replay is refused here too: its golden depends on the
# CPU path (D-319), and the head's golden choice is not wired yet.
check_head_name() {
  case "$1" in
    metrics|metrics:only=*|metrics:except=*) return 0 ;;
    *) echo "parity: PARITY_TREE=head runs the metrics suites only; refused: $1" >&2; return 2 ;;
  esac
}

build_go_nodes() {
  export GOTOOLCHAIN=go1.27.0
  export GOPROXY=https://proxy.golang.org
  export GOFLAGS=-mod=readonly
  export GOCACHE="$SCRATCH/gocache"
  export GOPATH="$SCRATCH/gopath"
  export GOBIN="$BIN"
  (cd "$SRC" && go build -o "$BIN/" ./cmd/...)
  (cd "$BIN" && sha256sum -- * > SHA256SUMS)
  echo "parity: Go nodes built into $BIN"
  cat "$BIN/SHA256SUMS"
}

cargo_in_tree() {
  (cd "$TREE" && CARGO_TARGET_DIR="$TARGET" cargo "$@")
}

# The CTR replay golden depends on the CPU (D-319). The pinned Go oracle's float
# kernels (common/floats) use the AVX-512 kernels when the CPU has AVX-512F, AVX and
# FMA, and the AVX kernels otherwise, and the two round differently. The pinned
# golden, afm_replay.json, records the AVX-512 kernels. afm_replay.avx.json records
# the AVX kernels: it is the pinned oracle's output with AVX-512F masked, as the D-319
# probes ran it. The pinned test reads its golden from its own tree and compares it
# byte for byte, so the golden for this host is copied into the extracted tree first.
# GODEBUG cpu options are refused here, because they change the kernel path without
# changing which golden applies.
select_ctr_golden() {
  local golden=crates/gorse-model/testdata/ctr/afm_replay.json
  if [[ "${GODEBUG:-}" == *cpu.* ]]; then
    echo "parity: ctr_replay refuses GODEBUG cpu options ($GODEBUG); they change the kernel path" >&2
    return 2
  fi
  local flags
  flags=" $(sed -n 's/^flags[[:space:]]*:[[:space:]]*//p' /proc/cpuinfo | head -1) "
  if [[ "$flags" == *" avx512f "* && "$flags" == *" avx "* && "$flags" == *" fma "* ]]; then
    git -C "$ROOT" show "$PIN:$golden" > "$SRC/$golden"
    echo "parity: ctr_replay golden for AVX-512 kernels: $golden (pinned)"
  else
    cp "$ROOT/crates/gorse-model/testdata/ctr/afm_replay.avx.json" "$SRC/$golden"
    echo "parity: ctr_replay golden for AVX kernels: afm_replay.avx.json"
  fi
}

# The golden test seeds the database that MYSQL_URI names, so it gets the
# database-level DSN from backends.env. The subshell keeps the variable local.
run_golden() {
  (
    export MYSQL_URI="$MYSQL_GOLDEN_URI"
    cargo_in_tree test -p gorse-conformance --test bench_golden -- \
      --include-ignored --test-threads=1
  )
}

# The derived references (D-368). For the three head configurations, the pinned Go oracle
# fits the same configuration with one job on a test file whose candidate lists have no
# repeats (scripts/derive_dedup_negatives.py). Its metrics are the expected values that the
# head overlay reads through PARITY_DERIVED_DIR. The training files are not changed. The two
# original test.txt files are copied back before the function returns, also when a step
# fails: the EXIT trap does it.
DERIVED_SWAPPED=()
restore_derived_tests() {
  local ds
  for ds in ${DERIVED_SWAPPED[@]+"${DERIVED_SWAPPED[@]}"}; do
    cp -- "$SCRATCH/derive-orig/$ds.test.txt" "$HOME/.gorse/dataset/$ds/test.txt"
    cmp -s -- "$SCRATCH/derive-orig/$ds.test.txt" "$HOME/.gorse/dataset/$ds/test.txt" \
      || { echo "parity: could not restore $ds/test.txt" >&2; return 1; }
  done
  DERIVED_SWAPPED=()
}

derive_overrides() {
  local dir="${1:-}" ds cfg testfile
  if [ "$PARITY_TREE" != head ]; then
    echo "parity: derive-overrides runs in head mode only (PARITY_TREE=head)" >&2
    return 2
  fi
  case "$dir" in
    /*) ;;
    *) echo "parity: derive-overrides needs an absolute DIR, not '$dir'" >&2; return 2 ;;
  esac
  mkdir -p "$dir" "$SCRATCH/derive-orig"
  rm -f -- "$dir"/*.json "$dir"/SHA256SUMS
  trap restore_derived_tests EXIT
  GORSE_DATASETS="ml-1m pinterest-20" scripts/setup-datasets.sh
  # Information only: how many training rows repeat a (user, item) pair. The Go loader and
  # the Rust test loader both keep every training row (add_feedback), so the count does not
  # change the training set of either side.
  for ds in ml-1m pinterest-20; do
    train="$HOME/.gorse/dataset/$ds/train.txt"
    if [ -f "$train" ]; then
      total=$(wc -l < "$train")
      distinct=$(cut -f1,2 "$train" | sort -u | wc -l)
      echo "parity: $ds train.txt: $total rows, $((total - distinct)) rows repeat a (user,item) pair"
    fi
  done
  for ds in ml-1m pinterest-20; do
    testfile="$HOME/.gorse/dataset/$ds/test.txt"
    if [ ! -f "$testfile" ]; then
      echo "parity: $testfile is missing after setup-datasets" >&2
      return 2
    fi
    cp -- "$testfile" "$SCRATCH/derive-orig/$ds.test.txt"
    python3 -I scripts/derive_dedup_negatives.py "$testfile" "$SCRATCH/derive-orig/$ds.derived.txt" \
      --label "$ds" --manifest "$dir/$ds.manifest.json" > /dev/null
    DERIVED_SWAPPED+=("$ds")
    cp -- "$SCRATCH/derive-orig/$ds.derived.txt" "$testfile"
  done
  for cfg in bpr-ml-1m als-ml-1m bpr-pinterest-20; do
    echo "parity: derive $cfg, one job, on the derived test file"
    # The flags and GODEBUG match oracle_with_jobs in the frozen suite.
    (cd "$HEAD" && GODEBUG=randautoseed=0 go run ./conformance/oracle/metrics \
      -config "$cfg" -jobs 1 -out "$dir/$cfg.json")
  done
  restore_derived_tests
  trap - EXIT
  (cd "$dir" && sha256sum -- *.json *.manifest.json > SHA256SUMS)
  cat "$dir/SHA256SUMS"
  for cfg in bpr-ml-1m als-ml-1m bpr-pinterest-20; do
    echo "parity: $cfg: $(cat "$dir/$cfg.json")"
  done
  echo "parity: derived references written to $dir; the original test files are restored"
}

extract_pinned_tree
build_go_nodes
if [ "$PARITY_TREE" = head ]; then
  extract_head_tree
fi

case "$cmd" in
  build)
    ;;
  dry-run)
    if [ "$PARITY_TREE" = head ]; then
      cargo_in_tree test -p gorse-conformance --test metrics --no-run
      cargo_in_tree test -p gorse-conformance --test metrics -- --list
      echo "parity: dry run done for the head tree (metrics only); no backend was started and no suite ran"
    else
      cargo_in_tree test -p gorse-conformance --no-run
      cargo_in_tree test -p gorse-conformance -- --list
      echo "parity: dry run done; no backend was started and no suite ran"
    fi
    ;;
  derive-overrides)
    derive_overrides "${1:-}"
    ;;
  run)
    if [ "$PARITY_TREE" = head ] && [ "$#" -eq 0 ]; then
      echo "parity: PARITY_TREE=head needs a NAME: metrics, metrics:only=TEST or metrics:except=TEST" >&2
      exit 2
    fi
    # Without --no-fail-fast, cargo stops at the first failing test binary and
    # the suites after it never run. bench_golden is run on its own below,
    # because it needs MYSQL_URI to name a database, and the other suites need
    # the server-level DSN in MYSQL_URI.
    if [ "$#" -eq 0 ]; then
      status=0
      cargo_in_tree test -p gorse-conformance --no-fail-fast -- \
        --include-ignored --test-threads=1 --skip bench_golden_matches_go || status=$?
      run_golden || status=$?
      exit "$status"
    else
      status=0
      for name in "$@"; do
        if [ "$PARITY_TREE" = head ] && ! check_head_name "$name"; then
          status=2
          continue
        fi
        if [ "$name" = bench_golden ]; then
          run_golden || status=$?
        else
          # A NAME may carry one test filter after a colon. SUITE:only=TEST runs the test
          # TEST alone; SUITE:except=TEST runs the rest of SUITE. Both match the name
          # exactly, so pinterest_20_bpr_one_job is not caught by pinterest_20_bpr. D-320
          # runs pinterest_20_bpr in a CI step of its own, with its own limit.
          suite="${name%%:*}"
          test_args=()
          case "$name" in
            *:only=*) test_args=(--exact "${name#*:only=}") ;;
            *:except=*) test_args=(--exact --skip "${name#*:except=}") ;;
            *:*) echo "parity: unknown filter in $name; use SUITE:only=TEST or SUITE:except=TEST" >&2; status=2; continue ;;
          esac
          if [ "$suite" = ctr_replay ]; then
            select_ctr_golden
          fi
          # The pinned mixed_server test appends only the database name to MONGO_URI,
          # so the auth source is added for that suite alone. The subshell keeps the
          # change local, as run_golden does for MYSQL_URI.
          (
            if [ "$suite" = mixed_server ]; then
              export MONGO_URI="$MONGO_URI?authSource=admin"
            fi
            cargo_in_tree test -p gorse-conformance --test "$suite" -- \
              --include-ignored --test-threads=1 "${test_args[@]}"
          ) || status=$?
        fi
      done
      exit "$status"
    fi
    ;;
esac
