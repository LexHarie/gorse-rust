import Gorse.Spec.Float32

/-! # The binary32 error model (P4-2, T3 of the verification plan)

`rn_model` is the standard model of rounding (Higham 2002 §2.2; IEEE 754-2019 §3.3). For
`|x|` below the overflow threshold, the rounded value is `x(1+δ)+ε`, with `|δ| ≤ u`,
`|ε| ≤ η` and `δε = 0`. For binary32, `u = 2^-24` and `η = 2^-150`. In the normal range the
error is relative, and it goes into `δ`. In the subnormal range the error is absolute, and it
goes into `ε`.

The grid of `rn` is unbounded here, so the error form holds for every nonzero `x`. The
proof does not use the overflow threshold. The threshold is the range in which `rn x` is a
finite binary32 value, and that finiteness is not proved in this file. Decision D-338. -/

namespace Gorse.Spec.Float32

/-- The unit roundoff of binary32, `u = 2^-24` (Higham 2002 §2.2). -/
noncomputable def unitRoundoff : ℝ := (2 : ℝ) ^ (-24 : ℤ)

/-- The absolute error bound of the subnormal range, `η = 2^-150`, half the subnormal unit. -/
noncomputable def subnormalError : ℝ := (2 : ℝ) ^ (-150 : ℤ)

/-- The overflow threshold of binary32: the largest finite value plus half its unit,
`(2 - 2^-24) · 2^127`. -/
noncomputable def ovfThreshold : ℝ := (2 - (2 : ℝ) ^ (-24 : ℤ)) * (2 : ℝ) ^ (127 : ℤ)

/-- `rn` obeys the standard model below the overflow threshold: `rn x = x(1+δ)+ε` with
`|δ| ≤ u`, `|ε| ≤ η` and `δε = 0`. -/
theorem rn_model {x : ℝ} (h : |x| < ovfThreshold) :
    ∃ δ ε : ℝ, rn x = x * (1 + δ) + ε ∧ |δ| ≤ unitRoundoff ∧ |ε| ≤ subnormalError ∧
      δ * ε = 0 := by
  by_cases hx : x = 0
  · refine ⟨0, 0, ?_, ?_, ?_, ?_⟩
    · simp [hx, rn_zero]
    · rw [abs_zero]
      unfold unitRoundoff
      positivity
    · rw [abs_zero]
      unfold subnormalError
      positivity
    · simp
  · have hx' : x ≠ 0 := hx
    have hpos : (0 : ℝ) < |x| := abs_pos.mpr hx'
    have hlo : (2 : ℝ) ^ Int.log 2 |x| ≤ |x| := by
      exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hpos
    have hhi : |x| < (2 : ℝ) ^ (Int.log 2 |x| + 1) := by
      exact_mod_cast Int.lt_zpow_succ_log_self (by norm_num : 1 < 2) |x|
    have hrn := rn_sub_le hx'
    by_cases hsub : |x| < (2 : ℝ) ^ (-126 : ℤ)
    · -- Subnormal range: the unit is 2^-149, and the error is absolute.
      have hL : Int.log 2 |x| < -126 := by
        by_contra hc
        push_neg at hc
        have h1 : (2 : ℝ) ^ (-126 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 |x| :=
          zpow_le_zpow_right₀ (by norm_num) hc
        linarith
      have hu : ulpExp x = -149 := by
        unfold ulpExp
        rw [max_eq_right hL.le]
        norm_num
      rw [hu] at hrn
      have e : (2 : ℝ) ^ (-149 : ℤ) / 2 = subnormalError := by
        unfold subnormalError
        rw [show (-150 : ℤ) = -149 - 1 by norm_num,
          zpow_sub_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
        ring
      rw [e] at hrn
      refine ⟨0, rn x - x, ?_, ?_, hrn, ?_⟩
      · ring
      · rw [abs_zero]
        unfold unitRoundoff
        positivity
      · simp
    · -- Normal range: the unit is 2^(⌊log₂|x|⌋ - 23), and the error is relative.
      have hL : -126 ≤ Int.log 2 |x| := by
        by_contra hc
        push_neg at hc
        have h1 : (2 : ℝ) ^ (Int.log 2 |x| + 1) ≤ (2 : ℝ) ^ (-126 : ℤ) :=
          zpow_le_zpow_right₀ (by norm_num) (by omega)
        have h2 : (2 : ℝ) ^ (-126 : ℤ) ≤ |x| := not_lt.mp hsub
        linarith
      have hu : ulpExp x = Int.log 2 |x| - 23 := by
        unfold ulpExp
        rw [max_eq_left hL]
      rw [hu] at hrn
      have e : (2 : ℝ) ^ (Int.log 2 |x| - 23) / 2
          = (2 : ℝ) ^ (-24 : ℤ) * (2 : ℝ) ^ Int.log 2 |x| := by
        rw [sub_eq_add_neg, zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0),
          show (-23 : ℤ) = -24 + 1 by norm_num,
          zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0), zpow_one]
        ring
      rw [e] at hrn
      have h3 : (2 : ℝ) ^ (-24 : ℤ) * (2 : ℝ) ^ Int.log 2 |x|
          ≤ (2 : ℝ) ^ (-24 : ℤ) * |x| :=
        mul_le_mul_of_nonneg_left hlo (by positivity)
      have hd : x * ((rn x - x) / x) = rn x - x := by
        field_simp
      refine ⟨(rn x - x) / x, 0, ?_, ?_, ?_, ?_⟩
      · rw [add_zero, mul_add, mul_one, hd]
        ring
      · rw [abs_div, div_le_iff₀ hpos]
        show |rn x - x| ≤ (2 : ℝ) ^ (-24 : ℤ) * |x|
        linarith
      · rw [abs_zero]
        unfold subnormalError
        positivity
      · simp

end Gorse.Spec.Float32
