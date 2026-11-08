import Mathlib.Analysis.SpecialFunctions.Exp
import Mathlib.Tactic.Positivity

/-!
# The stable sigmoid of BPR's gradient (A8, P5-2, D-358)

BPR's update (`crates/gorse-model/src/cf/model.rs`, `bpr_update`) multiplies the factors by the
gradient factor `σ(−d)`, where `d` is the margin `⟨w, h_i⟩ − ⟨w, h_j⟩` and `σ(x) = 1 / (1 + e^{−x})`.
The code computed it as `exp(−d) / (1 + exp(−d))`. For `d ≤ −89` the f32 `exp` overflows to +∞, and
`∞ / ∞` is NaN (plan §3.2, A8; F5).

The fix evaluates `exp` only at non-positive arguments: for `x ≥ 0` it uses `1 / (1 + exp (−x))`, and
for `x < 0` it uses `exp x / (1 + exp x)`. This file proves that the two branches together give σ for
every real `x`, and that the value lies in [0, 1].
-/

namespace Gorse.Spec.Sigmoid

/-- The logistic function `σ(x) = 1 / (1 + e^{−x})`, as the spec states it. -/
noncomputable def logistic (x : ℝ) : ℝ := 1 / (1 + Real.exp (-x))

/-- The stable form: `exp` is applied only at non-positive arguments in each branch. -/
noncomputable def sigStable (x : ℝ) : ℝ :=
  if 0 ≤ x then 1 / (1 + Real.exp (-x)) else Real.exp x / (1 + Real.exp x)

/-- The stable form equals the logistic function for every real `x`. -/
theorem sigStable_eq (x : ℝ) : sigStable x = logistic x := by
  unfold sigStable logistic
  split_ifs with h
  · rfl
  · have h1 : 0 < Real.exp x := Real.exp_pos x
    have h2 : (1 : ℝ) + Real.exp x ≠ 0 := by positivity
    have h3 : (1 : ℝ) + (Real.exp x)⁻¹ ≠ 0 := by positivity
    rw [Real.exp_neg, div_eq_div_iff h2 h3, mul_add, mul_one, mul_inv_cancel₀ h1.ne']
    ring

/-- The stable form lies in `[0, 1]` for every real `x`. -/
theorem sigStable_mem_Icc (x : ℝ) : sigStable x ∈ Set.Icc (0 : ℝ) 1 := by
  rw [sigStable_eq, Set.mem_Icc]
  unfold logistic
  have h1 : (0 : ℝ) < 1 + Real.exp (-x) := by positivity
  refine ⟨by positivity, ?_⟩
  rw [div_le_one h1]
  linarith [Real.exp_pos (-x)]

end Gorse.Spec.Sigmoid
