#!/usr/bin/env bash
# G9, the Lean oracle for the top-k filter (D-335), the Rust side of the check.
#
#   1. The Rust example writes 10^6 cases, from a fixed seed. Each case is a k up to 18
#      and up to 24 items with distinct ids, with ties, extreme weights and extreme ids.
#   2. The compiled Lean executable `oracle_topk` runs each case on the computable mirror
#      of the model (proofs/Gorse/Oracle/TopKMirror.lean). That mirror equals topKModel
#      (proofs/Gorse/Oracle/TopKLink.lean), so the oracle is the model.
#   3. The Rust example runs TopKFilter on the same cases. The results are discrete, so
#      the two outputs must be equal byte for byte. Any difference fails the check.
#
# Usage: scripts/check-oracle-topk.sh [work-directory]
#
# The oracle reads its cases in chunks of CHUNK lines, one process per chunk, for the same
# reason as scripts/check-oracle.sh: a bounded chunk keeps the recursion depth bounded.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
cases="$work/topk-cases.jsonl"
oracle_out="$work/topk-oracle.txt"
rust_out="$work/topk-rust.txt"

CHUNK=1000

example=(cargo run --release -q -p gorse-common --example g9_topk --)

"${example[@]}" gen "$cases" 1000000

# The oracle is built once, then run on each chunk of the cases, in order.
(cd proofs && lake build oracle_topk)
oracle="$PWD/proofs/.lake/build/bin/oracle_topk"
: >"$oracle_out"
split -l "$CHUNK" --filter="\"$oracle\" >>\"$oracle_out\"" "$cases"

"${example[@]}" run "$cases" >"$rust_out"

if ! cmp -s "$rust_out" "$oracle_out"; then
  echo "G9 (top-k) failed: the TopKFilter results differ from the Lean oracle" >&2
  echo "first differing lines (Rust, then oracle):" >&2
  diff "$rust_out" "$oracle_out" | head -n 20 >&2 || true
  exit 1
fi
echo "G9 (top-k): $(wc -l <"$cases") cases; every TopKFilter result equals the oracle's"
