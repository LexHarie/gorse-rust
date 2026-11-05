#!/usr/bin/env bash
# G10, cargo-mutants on the verified T1 metric code (porting/verification-plan.md, G10;
# D-327).
#
# The mutants are the metric functions in crates/gorse-model/src/cf/evaluator.rs
# (ndcg, precision, recall, hr, map, mrr), and candidate_list, which removes repeated
# candidates (A4, D-323). Each mutant must be caught by the cf::evaluator tests.
# cargo-mutants exits non-zero when any mutant is missed, timed out, or unviable, which
# fails this check. The unmutated baseline must also pass before any mutant is tried.
#
# This checks the T1 module only. The crate-level baseline of gorse-model is not measured
# here (D-327).
#
# Usage: scripts/check-mutants.sh [output-directory]
set -euo pipefail

cd "$(dirname "$0")/.."

out="${1:-$(mktemp -d)}"
mkdir -p "$out"

cargo mutants \
  --package gorse-model \
  --file crates/gorse-model/src/cf/evaluator.rs \
  --re '(ndcg|log2|precision|recall|hr|map|mrr|candidate_list)' \
  --output "$out" \
  -- --lib cf::evaluator
