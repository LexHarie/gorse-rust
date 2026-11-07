#!/usr/bin/env bash
# G10 on the verified top-k code of crates/gorse-common/src/heap.rs (P3-5, D-336).
#
# The mutants are the methods of TopKFilter, and its helpers worse and is_nan. Each mutant
# must be caught by the heap lib tests, or by the tests in tests/g10_heap.rs, whose names
# also carry `heap`. cargo-mutants exits non-zero when a mutant is missed or times out. An
# unviable mutant does not compile, so it is not run and does not count as missed.
#
# One mutant is excluded, and it is equivalent (D-336). Its replacement of `>` by `>=` in
# `worse` changes the comparison only when two elements have equal weight and equal value.
# Such elements are identical, so the heap invariant (a child is never worse than its
# parent) is kept, and pop_all returns the same (value, weight) sequence either way. The
# exclusion is anchored to its line and column, so any other mutant still runs.
#
# Usage: scripts/check-mutants-topk.sh [output-directory]
set -euo pipefail

cd "$(dirname "$0")/.."

out="${1:-$(mktemp -d)}"
mkdir -p "$out"

cargo mutants \
  --package gorse-common \
  --file crates/gorse-common/src/heap.rs \
  --re 'TopKFilter|worse|is_nan' \
  --exclude-re 'heap\.rs:391:61: replace > with >= in worse' \
  --output "$out" \
  -- --lib --test g10_heap heap
