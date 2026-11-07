#!/usr/bin/env bash
# G9 for the ndcg log table (D-354, P4-10), the Rust side of the check.
#
#   1. The Rust example g9_log2 prints LOG2_TABLE, the table of binary32 values of log2 k for
#      k = 2..65 that `ndcg` reads, one line `k 0xBITS` for each k.
#   2. The compiled Lean executable `oracle_log2` (proofs/Oracle/Log2.lean) prints the bit pattern of
#      the exact log2 k, rounded once to binary32, on the same lines. It uses Lean's core library only.
#   3. The two outputs must be equal byte for byte. Any difference fails the check. A difference is
#      never accepted as a tolerance; it needs an exact override with its derivation in D-354.
#
# Usage: scripts/check-oracle-log2.sh [work-directory]
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
rust_out="$work/log2-rust.txt"
oracle_out="$work/log2-oracle.txt"

cargo run --release -q -p gorse-model --example g9_log2 >"$rust_out"

(cd proofs && lake build oracle_log2)
proofs/.lake/build/bin/oracle_log2 >"$oracle_out"

if ! cmp -s "$rust_out" "$oracle_out"; then
  echo "G9 (log2 table) failed: LOG2_TABLE differs from the Lean oracle" >&2
  diff "$rust_out" "$oracle_out" >&2 || true
  exit 1
fi
echo "G9 (log2 table): $(wc -l <"$rust_out") entries for k = 2..65; every entry equals the oracle's"
