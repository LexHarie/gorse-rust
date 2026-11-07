import Gorse.Spec.Float32Model

/-! # The sequential binary32 dot product: definitions and grid rounding (P4-4, T3)

`dotSeq` is the model of `native::dot` (crates/gorse-kernels/src/floats.rs:457-463). It starts
from 0, and for each pair it rounds the product, then rounds the sum. The operand order of
`add_first` and `mul_first` does not change a finite result, since `rn (a + b) = rn (b + a)`.

This file has the definitions and four lemmas about `rn` on grid values. A grid value is an
integer multiple of the subnormal unit `2^-149`. Every value of `rn` is a grid value, so
partial sums and rounded products are grid values. The error induction and `dotSeq_err` are
in later PRs (D-342, D-343). Decision D-341. -/

namespace Gorse.Spec.Dot

open Gorse.Spec.Float32

/-- The constant `γₙ = n u / (1 - n u)` of the sequential dot product bound (Higham 2002 §3.1). -/
noncomputable def γ (n : ℕ) : ℝ := n * unitRoundoff / (1 - n * unitRoundoff)

/-- A binary32 value: a real that `rn` fixes, below the overflow threshold. -/
def IsF32 (x : ℝ) : Prop := rn x = x ∧ |x| < ovfThreshold

/-- The sequential dot product of `native::dot` (floats.rs:457-463). Each product is rounded,
then each sum is rounded, starting from 0. -/
noncomputable def dotAux : List (ℝ × ℝ) → ℝ → ℝ
  | [], s => s
  | p :: v, s => dotAux v (rn (s + rn (p.1 * p.2)))

/-- The dot product of a list of pairs, computed sequentially from 0. -/
noncomputable def dotSeq (v : List (ℝ × ℝ)) : ℝ := dotAux v 0

/-- No intermediate product or sum leaves the binary32 range, at each step of `dotAux`. -/
def NoOverflowAux : List (ℝ × ℝ) → ℝ → Prop
  | [], _ => True
  | p :: v, s =>
      |p.1 * p.2| < ovfThreshold ∧ |s + rn (p.1 * p.2)| < ovfThreshold ∧
        NoOverflowAux v (rn (s + rn (p.1 * p.2)))

/-- The hypothesis of the error bound: no overflow in the sequential computation of `dotSeq`. -/
def NoOverflow (v : List (ℝ × ℝ)) : Prop := NoOverflowAux v 0

/-- Every value of `rn` is an integer multiple of the subnormal unit `2^-149`, because the unit
of `rn x` is at least `2^-149` (`ulpExp_ge`). -/
theorem rn_multiple (x : ℝ) : ∃ k : ℤ, rn x = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ) := by
  by_cases hx : x = 0
  · exact ⟨0, by simp [hx, rn_zero]⟩
  · obtain ⟨R, hR⟩ : ∃ R : ℤ, roundEven (x / 2 ^ ulpExp x) = R := ⟨_, rfl⟩
    have hdef : rn x = (R : ℝ) * 2 ^ ulpExp x := by
      unfold rn
      rw [if_neg hx, hR]
    obtain ⟨n, hn⟩ : ∃ n : ℕ, (n : ℤ) = ulpExp x + 149 :=
      ⟨(ulpExp x + 149).toNat, Int.toNat_of_nonneg (by linarith [ulpExp_ge x])⟩
    have he : (2 : ℝ) ^ ulpExp x = (2 : ℝ) ^ n * (2 : ℝ) ^ (-149 : ℤ) := by
      rw [← zpow_natCast, ← zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0)]
      congr 1
      omega
    refine ⟨R * 2 ^ n, ?_⟩
    rw [hdef, he]
    push_cast
    ring

/-- The sum of two grid values, below the normal range, is exact: `rn` leaves it unchanged.
This is `rn_add_subnormal` with the hypothesis `rn a = a` replaced by the weaker fact that `a`
is a grid value. Every value of `rn` is such a value (`rn_multiple`), so no idempotence is
needed. -/
theorem rn_add_grid_subnormal {a b : ℝ} (ha : ∃ k : ℤ, a = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ))
    (hb : ∃ m : ℤ, b = (m : ℝ) * (2 : ℝ) ^ (-149 : ℤ))
    (hs : |a + b| < (2 : ℝ) ^ (-126 : ℤ)) : rn (a + b) = a + b := by
  obtain ⟨k, hk⟩ := ha
  obtain ⟨m, hm⟩ := hb
  rcases eq_or_ne (a + b) 0 with hz | hz
  · rw [hz, rn_zero]
  · have hsum : a + b = ((k + m : ℤ) : ℝ) * (2 : ℝ) ^ (-149 : ℤ) := by
      rw [hk, hm]
      push_cast
      ring
    have hlo : (2 : ℝ) ^ Int.log 2 |a + b| ≤ |a + b| := by
      exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) (abs_pos.mpr hz)
    have hL : Int.log 2 |a + b| < -126 := by
      by_contra hc
      push_neg at hc
      have h1 : (2 : ℝ) ^ (-126 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 |a + b| :=
        zpow_le_zpow_right₀ (by norm_num) hc
      linarith
    have hu : ulpExp (a + b) = -149 := by
      unfold ulpExp
      rw [max_eq_right hL.le]
      norm_num
    exact rn_of_grid (k + m) (by rw [hu]; exact hsum)

/-- Above the normal range, `rn` moves a value by at most `u` times its size. The unit there is
`2^(⌊log₂|x|⌋-23)`, so the error is at most `2^(⌊log₂|x|⌋-24) ≤ u|x|`. -/
theorem rn_err_normal {x : ℝ} (hx : (2 : ℝ) ^ (-126 : ℤ) ≤ |x|) :
    |rn x - x| ≤ unitRoundoff * |x| := by
  unfold unitRoundoff
  have hx0 : x ≠ 0 := by
    intro h
    rw [h, abs_zero] at hx
    have : (0 : ℝ) < (2 : ℝ) ^ (-126 : ℤ) := by positivity
    linarith
  have hpos : (0 : ℝ) < |x| := abs_pos.mpr hx0
  have hlo : (2 : ℝ) ^ Int.log 2 |x| ≤ |x| := by
    exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hpos
  have hL : -126 ≤ Int.log 2 |x| := by
    by_contra hc
    push_neg at hc
    have h1 : (2 : ℝ) ^ (Int.log 2 |x| + 1) ≤ (2 : ℝ) ^ (-126 : ℤ) :=
      zpow_le_zpow_right₀ (by norm_num) (by omega)
    have hhi : |x| < (2 : ℝ) ^ (Int.log 2 |x| + 1) := by
      exact_mod_cast Int.lt_zpow_succ_log_self (by norm_num : 1 < 2) |x|
    linarith
  have hu : ulpExp x = Int.log 2 |x| - 23 := by
    unfold ulpExp
    rw [max_eq_left hL]
  have hr := rn_sub_le hx0
  rw [hu] at hr
  have e : (2 : ℝ) ^ (Int.log 2 |x| - 23) / 2 = (2 : ℝ) ^ (-24 : ℤ) * (2 : ℝ) ^ Int.log 2 |x| := by
    rw [sub_eq_add_neg, zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0),
      show (-23 : ℤ) = -24 + 1 by norm_num,
      zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0), zpow_one]
    ring
  rw [e] at hr
  have h3 : (2 : ℝ) ^ (-24 : ℤ) * (2 : ℝ) ^ Int.log 2 |x| ≤ (2 : ℝ) ^ (-24 : ℤ) * |x| :=
    mul_le_mul_of_nonneg_left hlo (by positivity)
  linarith

/-- The rounding of a sum of two grid values has error at most `u` times the sum. The sum is
exact when it lies below the normal range (`rn_add_grid_subnormal`), and otherwise the error is
relative (`rn_err_normal`). -/
theorem rn_add_grid_err {a b : ℝ} (ha : ∃ k : ℤ, a = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ))
    (hb : ∃ m : ℤ, b = (m : ℝ) * (2 : ℝ) ^ (-149 : ℤ)) :
    |rn (a + b) - (a + b)| ≤ unitRoundoff * |a + b| := by
  by_cases hs : |a + b| < (2 : ℝ) ^ (-126 : ℤ)
  · rw [rn_add_grid_subnormal ha hb hs, sub_self, abs_zero]
    exact mul_nonneg (by unfold unitRoundoff; positivity) (abs_nonneg _)
  · exact rn_err_normal (not_lt.mp hs)

end Gorse.Spec.Dot
