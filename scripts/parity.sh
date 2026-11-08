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
  build|dry-run|run) shift ;;
  *) echo "usage: $0 build | dry-run | run [NAME...]" >&2; exit 2 ;;
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
