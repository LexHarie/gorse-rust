#!/usr/bin/env bash
# G9 for the dot kernels (D-349, P4-7d): the Lean oracle `oracle_dot` against the Rust kernels.
#
#   1. The binary32 decomposition that the cases use is checked to round-trip.
#   2. The Rust example writes seeded, adversarial vector pairs, three cases for each: the
#      models `seq`, `avx` and `avx512`. The cases include cancellation, subnormal operands,
#      large operands and zeros, and their lengths run from 0 to 64.
#   3. The compiled Lean executable `oracle_dot` computes each case over the rationals, with
#      the rounding of the model (the twins in Gorse.Spec.DotRat, D-348), and writes the
#      binary32 result as `m e`.
#   4. The Rust kernels run on the same cases, at the levels the CPU has, and every result is
#      compared bit for bit with the oracle's. The levels that were run, and the levels that
#      were skipped, are printed. Any mismatch fails the check.
#
# Usage: scripts/check-oracle-dot.sh [work-directory] [vector-pairs]
#
# The oracle reads its cases in chunks of CHUNK lines, one process per chunk, as
# scripts/check-oracle.sh does: its loop recurses once per line, and a bounded chunk keeps the
# stack depth bounded.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
cases="$work/cases.jsonl"
results="$work/results.txt"
pairs="${2:-}"

CHUNK=1000

example=(cargo run --release -q -p gorse-model --example g9_dot --)

"${example[@]}" selftest
if [[ -n "$pairs" ]]; then
  "${example[@]}" gen "$cases" "$pairs"
else
  "${example[@]}" gen "$cases"
fi

# The oracle is built once, then run on each chunk of the cases, in order.
(cd proofs && lake build oracle_dot)
oracle="$PWD/proofs/.lake/build/bin/oracle_dot"
: >"$results"
split -l "$CHUNK" --filter="\"$oracle\" >>\"$results\"" "$cases"

"${example[@]}" check "$cases" "$results"
