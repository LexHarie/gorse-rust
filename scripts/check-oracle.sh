#!/usr/bin/env bash
# G9, the Lean oracle for the T1 metrics (D-326): precision, recall, hr and mrr.
#
#   1. The rounding that the Lean oracle mirrors is checked against IEEE division.
#   2. The Rust example writes 10^6 cases per metric, from a fixed seed. The cases
#      include the empty sets, duplicates, full overlaps, prefixes and extreme ids.
#   3. The compiled Lean executable `oracle` computes each case exactly and writes the
#      f32 bit pattern of the result, or `none`.
#   4. The Rust metrics are run on the same cases, and every result is compared bit for
#      bit with the oracle's. Any mismatch fails the check.
#
# map and ndcg are not covered (D-326): they round a sum, or take a logarithm, and the
# checks for them wait for their own models.
#
# Usage: scripts/check-oracle.sh [work-directory]
# The Lean project in proofs/ must be built first, so that `lake exe oracle` runs.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
cases="$work/cases.jsonl"
results="$work/results.txt"

example=(cargo run --release -q -p gorse-model --example g9_oracle --)

"${example[@]}" selftest
"${example[@]}" gen "$cases"
(cd proofs && lake exe oracle <"$cases" >"$results")
"${example[@]}" check "$cases" "$results"
