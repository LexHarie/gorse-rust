#!/usr/bin/env bash
# G9 for `map` (D-352, P4-9): the Lean oracle `oracle_map` against the Rust `map`.
#
#   1. The Rust example writes seeded cases, 100,000 by default. A case is a target size t and the
#      relevance flags of a ranked list of up to 64 entries. Some targets are empty, some lists are
#      empty, and a relevant id may repeat, as a ranked list can hold it.
#   2. The compiled Lean executable `oracle_map` computes each case over the rationals, with the
#      rounding of each step of the Rust sum (the twin Gorse.Spec.MapRat.mapQ, D-352), and writes
#      the binary32 result as `m e`, or `none` where `map` is undefined.
#   3. The Rust `map` runs on the same cases, and every result is compared bit for bit with the
#      oracle's. Any mismatch fails the check.
#
# Usage: scripts/check-oracle-map.sh [work-directory] [case-count]
#
# The oracle reads its cases in chunks of CHUNK lines, one process per chunk, as
# scripts/check-oracle-dot.sh does.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
cases="$work/cases.jsonl"
results="$work/results.txt"
count="${2:-}"

CHUNK=1000

example=(cargo run --release -q -p gorse-model --example g9_map --)

if [[ -n "$count" ]]; then
  "${example[@]}" gen "$cases" "$count"
else
  "${example[@]}" gen "$cases"
fi

# The oracle is built once, then run on each chunk of the cases, in order.
(cd proofs && lake build oracle_map)
oracle="$PWD/proofs/.lake/build/bin/oracle_map"
: >"$results"
split -l "$CHUNK" --filter="\"$oracle\" >>\"$results\"" "$cases"

"${example[@]}" check "$cases" "$results"
