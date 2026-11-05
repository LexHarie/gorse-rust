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
#                                     storage/docker-compose.yml
#
# Environment:
#   PARITY_DIR   scratch root (default: target/parity)
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

cargo_in_pinned_tree() {
  (cd "$SRC" && CARGO_TARGET_DIR="$SCRATCH/target" cargo "$@")
}

# The golden test seeds the database that MYSQL_URI names, so it gets the
# database-level DSN from backends.env. The subshell keeps the variable local.
run_golden() {
  (
    export MYSQL_URI="$MYSQL_GOLDEN_URI"
    cargo_in_pinned_tree test -p gorse-conformance --test bench_golden -- \
      --include-ignored --test-threads=1
  )
}

extract_pinned_tree
build_go_nodes

case "$cmd" in
  build)
    ;;
  dry-run)
    cargo_in_pinned_tree test -p gorse-conformance --no-run
    cargo_in_pinned_tree test -p gorse-conformance -- --list
    echo "parity: dry run done; no backend was started and no suite ran"
    ;;
  run)
    # Without --no-fail-fast, cargo stops at the first failing test binary and
    # the suites after it never run. bench_golden is run on its own below,
    # because it needs MYSQL_URI to name a database, and the other suites need
    # the server-level DSN in MYSQL_URI.
    if [ "$#" -eq 0 ]; then
      status=0
      cargo_in_pinned_tree test -p gorse-conformance --no-fail-fast -- \
        --include-ignored --test-threads=1 --skip bench_golden_matches_go || status=$?
      run_golden || status=$?
      exit "$status"
    else
      status=0
      for name in "$@"; do
        if [ "$name" = bench_golden ]; then
          run_golden || status=$?
        else
          cargo_in_pinned_tree test -p gorse-conformance --test "$name" -- \
            --include-ignored --test-threads=1 || status=$?
        fi
      done
      exit "$status"
    fi
    ;;
esac
