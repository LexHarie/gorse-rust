#!/usr/bin/env bash
# G9 for `ndcg` (D-356, P4-11): the Lean oracle `oracle_ndcg` against the Rust `ndcg`.
#
#   1. The Rust example writes seeded cases, 100,000 by default. A case is a target size t and the
#      relevance flags of a ranked list of up to 64 entries. Some targets are empty, some lists are
#      empty, and a relevant id may repeat, as a ranked list can hold it. `gen` reports how many cases
#      are undefined, how many lists have 64 entries, and how many repeat a relevant id.
#   2. The compiled Lean executable `oracle_ndcg` computes each case over the rationals, with the
#      rounding of each step of the Rust sums and of the division (the twin Gorse.Spec.NdcgRat.ndcgQ,
#      D-356), and writes the binary32 result as `m e`, or `none` where `ndcg` is undefined.
#   3. The Rust `ndcg` runs on the same cases, and every result is compared bit for bit with the
#      oracle's. Any mismatch fails the check.
#
# Usage: scripts/check-oracle-ndcg.sh [work-directory] [case-count]
#
# The oracle reads its cases in chunks of CHUNK lines, one process per chunk, as
# scripts/check-oracle-map.sh does.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
cases="$work/cases.jsonl"
results="$work/results.txt"
count="${2:-}"

CHUNK=1000

example=(cargo run --release -q -p gorse-model --example g9_ndcg --)

if [[ -n "$count" ]]; then
  "${example[@]}" gen "$cases" "$count"
else
  "${example[@]}" gen "$cases"
fi

# The oracle is built once, then run on each chunk of the cases, in order.
(cd proofs && lake build oracle_ndcg)
oracle="$PWD/proofs/.lake/build/bin/oracle_ndcg"
: >"$results"
split -l "$CHUNK" --filter="\"$oracle\" >>\"$results\"" "$cases"

"${example[@]}" check "$cases" "$results"
