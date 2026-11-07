import Gorse.Spec.Dot
import Gorse.Spec.DotAvx512

/-!
# The AVX-512 and scalar dot products agree within the sum of their bounds (P4-8)

The plan's `dot_levels_close`. Each kernel is within its own bound of the exact sum:
`dotSeq_err` (D-343) for the scalar loop and `dotAvx512_err` (D-344) for the AVX-512 kernel.
Both bounds carry the term `2 · n · η`, so the triangle inequality through the exact sum gives
`4 · n · η`, not the `2 · n · η` the plan wrote. That difference is recorded in D-351.

The proof uses no hypothesis beyond the length condition `2 · n · u ≤ 1` that both bounds need.
-/

namespace Gorse.Spec.DotLevels

open Gorse.Spec.Float32 Gorse.Spec.Dot Gorse.Spec.DotAvx512

/-- The AVX-512 dot product and the scalar `dotSeq` differ by at most the sum of their two bounds
(D-351). -/
theorem dot_levels_close (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    |dotAvx512 v - dotSeq v| ≤
      (γ (depthAvx512 v.length) + γ v.length) * (v.map fun p => |p.1 * p.2|).sum
        + 4 * (v.length : ℝ) * subnormalError := by
  have h1 := dotAvx512_err v hn
  have h2 := dotSeq_err v hn
  have h3 := abs_sub_le (dotAvx512 v) (v.map fun p => p.1 * p.2).sum (dotSeq v)
  rw [abs_sub_comm (v.map fun p => p.1 * p.2).sum (dotSeq v)] at h3
  have h4 : (γ (depthAvx512 v.length) + γ v.length) * (v.map fun p => |p.1 * p.2|).sum
      = γ (depthAvx512 v.length) * (v.map fun p => |p.1 * p.2|).sum
        + γ v.length * (v.map fun p => |p.1 * p.2|).sum := add_mul _ _ _
  have h5 : 4 * (v.length : ℝ) * subnormalError
      = 2 * (v.length : ℝ) * subnormalError + 2 * (v.length : ℝ) * subnormalError := by ring
  rw [h4, h5]
  linarith

end Gorse.Spec.DotLevels
