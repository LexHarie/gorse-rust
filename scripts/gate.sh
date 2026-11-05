#!/usr/bin/env bash
# The verification gate (G0 in porting/verification-plan.md). It reproduces the
# D-312 stage list from the repository alone.
#
# Usage:
#   scripts/gate.sh              run every stage in order
#   scripts/gate.sh TEST FMT     run only the named stages, in the order given
#
# Stages (each runs with the cargo toolchain pinned by rust-toolchain.toml):
#   FMT, CLIPPY, DOC, CONFIGGOLDEN, YAMLGOLDEN, SETUP, TEST, CLOUDCLIPPY,
#   MONGOCLIPPY, VECTOR_ACCEPT, PROXY_STORE, REDIS_TLS_UNSET
#
# Pass criteria (D-312): every stage exits 0, except REDIS_TLS_UNSET, which must
# exit 101 (it is the negative check: the TLS cases fail without REDIS_TLS_URI).
# The TEST stage also prints its totals, summed over every suite.
#
# Backends: the VECTOR_ACCEPT, PROXY_STORE and TEST stages need the backends of
# storage/docker-compose.yml, with the URIs in scripts/backends.env. The workflow starts them;
# this script does not start containers. A backend that is down makes its
# stage fail, which is the intended result.
#
# Logs go to $GATE_LOG_DIR (default: target/gate), one file per stage.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

LOG_DIR="${GATE_LOG_DIR:-target/gate}"
mkdir -p "$LOG_DIR"

# Backend URIs (see scripts/backends.env).
. "$(dirname "$0")/backends.env"

# Expected exit status per stage. Every other stage expects 0.
expected_status() {
  case "$1" in
    REDIS_TLS_UNSET) echo 101 ;;
    *) echo 0 ;;
  esac
}

run_stage() {
  case "$1" in
    FMT)            cargo fmt --all -- --check ;;
    CLIPPY)         cargo clippy --workspace --all-targets -- -D warnings ;;
    DOC)            RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps ;;
    CONFIGGOLDEN)   cargo test -p gorse-config --test config_golden ;;
    YAMLGOLDEN)     cargo test -p gorse-config --test yaml_golden ;;
    SETUP)          bash scripts/setup-datasets.sh ;;
    TEST)           cargo test --workspace --no-fail-fast ;;
    CLOUDCLIPPY)    cargo clippy -p gorse-storage --all-targets --features s3,gcs,azure -- -D warnings ;;
    MONGOCLIPPY)    cargo clippy -p gorse-storage --all-targets --features mongodb -- -D warnings ;;
    VECTOR_ACCEPT)  cargo test -p gorse-storage --features qdrant,milvus,weaviate vectors \
                      -- --include-ignored --skip vectors::weaviate::tests::sparse ;;
    PROXY_STORE)    cargo test -p gorse-storage --features mysql,postgres,sqlite,redis proxy ;;
    REDIS_TLS_UNSET) env -u REDIS_TLS_URI -u REDIS_TLS_UNTRUSTED \
                      cargo test -p gorse-storage --features redis tls -- --include-ignored ;;
    *) echo "unknown stage: $1" >&2; return 2 ;;
  esac
}

# Sums the "test result:" lines of a cargo test log into one set of totals.
test_totals() {
  awk '
    /^test result:/ {
      suites++
      for (i = 1; i <= NF; i++) {
        if ($(i+1) == "passed;")  passed  += $i
        if ($(i+1) == "failed;")  failed  += $i
        if ($(i+1) == "ignored;") ignored += $i
      }
    }
    END { printf "TEST: %d suites, %d passed, %d failed, %d ignored\n", suites, passed, failed, ignored }
  ' "$1"
}

if [ "$#" -eq 0 ]; then
  set -- FMT CLIPPY DOC CONFIGGOLDEN YAMLGOLDEN SETUP TEST CLOUDCLIPPY MONGOCLIPPY \
         VECTOR_ACCEPT PROXY_STORE REDIS_TLS_UNSET
fi

overall=0
for stage in "$@"; do
  log="$LOG_DIR/$stage.log"
  echo "== $stage"
  run_stage "$stage" >"$log" 2>&1
  status=$?
  want=$(expected_status "$stage")
  if [ "$status" -eq "$want" ]; then
    result="ok (exit $status)"
  else
    result="FAILED (exit $status, expected $want)"
    overall=1
  fi
  echo "   $result, log: $log"
  if [ "$stage" = "TEST" ]; then
    test_totals "$log"
  fi
done

if [ "$overall" -eq 0 ]; then
  echo "gate: every stage matched its expected status"
else
  echo "gate: at least one stage did not match its expected status" >&2
fi
exit "$overall"
