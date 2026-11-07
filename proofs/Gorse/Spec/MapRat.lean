import Gorse.Spec.Float32Q

/-!
# The ℚ twin of `map` for one user (D-352, P4-9)

`map` in `crates/gorse-model/src/cf/evaluator.rs` adds, for each relevant entry of the ranked list
in order, the term `hit / (i + 1)`: `hit` counts the relevant entries so far, and `i` is the index of
the entry. Each term is one binary32 division, and each partial sum is one binary32 addition, so each
is rounded. The total is divided once by `|T|`, which is one more rounded division. This file writes
that sequence over the rationals, with `rnQ` (D-340) at each step, so the value is the binary32
result that Rust computes. The executable `oracle_map` (proofs/Oracle/Map.lean) prints it.

`map` depends only on `|T|` and on which entries of the ranked list are relevant, in order, so the
inputs are `t = |T|` and the relevance flags `hits`.
-/

namespace Gorse.Spec.MapRat

open Gorse.Spec.Float32

/-- The rounded sum of the terms of `map`. `hits` are the relevance flags of the ranked list, `i` is
the index of the next entry, `hit` is the number of relevant entries so far, and `s` is the partial
sum. Each term and each partial sum is rounded, as the binary32 operations of Rust are. -/
def mapSumQ : List Bool → ℕ → ℕ → ℚ → ℚ
  | [], _, _, s => s
  | b :: bs, i, hit, s =>
    if b then
      mapSumQ bs (i + 1) (hit + 1)
        (rnQ (s + rnQ (((hit + 1 : ℕ) : ℚ) / ((i + 1 : ℕ) : ℚ))))
    else
      mapSumQ bs (i + 1) hit s

/-- `map` of one user, as a binary32 value over the rationals: the rounded sum divided by `|T|`, and
rounded. It is `none` when the target or the ranked list is empty, as in Rust. -/
def mapQ (t : ℕ) (hits : List Bool) : Option ℚ :=
  if t = 0 ∨ hits = [] then none
  else some (rnQ (mapSumQ hits 0 0 0 / (t : ℚ)))

end Gorse.Spec.MapRat
