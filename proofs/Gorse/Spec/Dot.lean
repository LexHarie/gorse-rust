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

/-- For every real `x`, `rn` moves `x` by at most `u|x| + η`. Above the normal range this is
`rn_err_normal`. Below it the unit is `2^-149`, so the error is at most `η` (`rn_sub_le`). -/
theorem rn_err_all (x : ℝ) : |rn x - x| ≤ unitRoundoff * |x| + subnormalError := by
  have hη : (0 : ℝ) ≤ subnormalError := by unfold subnormalError; positivity
  have hu0 : (0 : ℝ) ≤ unitRoundoff * |x| := by unfold unitRoundoff; positivity
  by_cases hs : (2 : ℝ) ^ (-126 : ℤ) ≤ |x|
  · linarith [rn_err_normal hs]
  · by_cases hx : x = 0
    · subst hx
      simpa [rn_zero] using hη
    · have hlo : (2 : ℝ) ^ Int.log 2 |x| ≤ |x| := by
        exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) (abs_pos.mpr hx)
      have hL : Int.log 2 |x| < -126 := by
        by_contra hc
        push_neg at hc
        have h1 : (2 : ℝ) ^ (-126 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 |x| :=
          zpow_le_zpow_right₀ (by norm_num) hc
        push_neg at hs
        linarith
      have hu : ulpExp x = -149 := by
        unfold ulpExp
        rw [max_eq_right hL.le]
        norm_num
      have hr := rn_sub_le hx
      rw [hu] at hr
      have e : (2 : ℝ) ^ (-149 : ℤ) / 2 = subnormalError := by
        unfold subnormalError
        rw [show (-150 : ℤ) = -149 - 1 by norm_num, zpow_sub_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
        ring
      rw [e] at hr
      linarith

/-- A grid value `y = R·2^e` whose unit is at most `2^e` is a multiple of its own unit. -/
theorem grid_of_le {y : ℝ} {R e : ℤ} (hy : y = (R : ℝ) * 2 ^ e) (hf : ulpExp y ≤ e) :
    ∃ K : ℤ, y = (K : ℝ) * 2 ^ ulpExp y := by
  obtain ⟨n, hn⟩ : ∃ n : ℕ, (n : ℤ) = e - ulpExp y :=
    ⟨(e - ulpExp y).toNat, Int.toNat_of_nonneg (by linarith)⟩
  have he : (2 : ℝ) ^ e = (2 : ℝ) ^ n * (2 : ℝ) ^ ulpExp y := by
    rw [← zpow_natCast, ← zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0)]
    congr 1
    omega
  refine ⟨R * 2 ^ n, ?_⟩
  conv_lhs => rw [hy]
  rw [he]
  push_cast
  ring

/-- If rounding `x` raises its unit, the result is a multiple of its own unit. Writing `L` for
`⌊log₂|x|⌋`, the unit rises only when `rn x` is the power of two `2^(L+1)`, and that power is a
multiple of its unit. Below the normal range the unit cannot rise at all. -/
theorem grid_of_gt {x y : ℝ} {R : ℤ} (hx : x ≠ 0) (hy : y = (R : ℝ) * 2 ^ ulpExp x)
    (hy0 : y ≠ 0) (hsub : |y - x| ≤ (2 : ℝ) ^ ulpExp x / 2) (hgt : ulpExp x < ulpExp y) :
    ∃ K : ℤ, y = (K : ℝ) * 2 ^ ulpExp y := by
  have hxpos : (0 : ℝ) < |x| := abs_pos.mpr hx
  have hypos : (0 : ℝ) < |y| := abs_pos.mpr hy0
  have hlo : (2 : ℝ) ^ Int.log 2 |x| ≤ |x| := by
    exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hxpos
  have hhi : |x| < (2 : ℝ) ^ (Int.log 2 |x| + 1) := by
    exact_mod_cast Int.lt_zpow_succ_log_self (by norm_num : 1 < 2) |x|
  have hylo : (2 : ℝ) ^ Int.log 2 |y| ≤ |y| := by
    exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hypos
  have hyx : |y| ≤ |x| + |y - x| := by
    have := abs_sub_abs_le_abs_sub y x
    linarith
  have hLt : Int.log 2 |x| < Int.log 2 |y| := by
    by_contra hc
    push_neg at hc
    have hm : max (Int.log 2 |y|) (-126) ≤ max (Int.log 2 |x|) (-126) :=
      max_le_max hc le_rfl
    unfold ulpExp at hgt
    linarith
  rcases le_or_lt (-126) (Int.log 2 |x|) with hL | hL
  · -- Normal x: the unit of x is `2^(L-23)`.
    have hu : ulpExp x = Int.log 2 |x| - 23 := by
      unfold ulpExp
      rw [max_eq_left hL]
    have hu' : ulpExp y = Int.log 2 |y| - 23 := by
      unfold ulpExp
      rw [max_eq_left (show (-126 : ℤ) ≤ Int.log 2 |y| by omega)]
    rw [hu] at hy hsub
    have hL1 : Int.log 2 |x| + 1 ≤ Int.log 2 |y| := by omega
    have h2 : (2 : ℝ) ^ (Int.log 2 |x| + 1) ≤ |y| :=
      le_trans (zpow_le_zpow_right₀ (by norm_num) hL1) hylo
    have hpow : (2 : ℝ) ^ (Int.log 2 |x| + 1)
        = (2 : ℝ) ^ (24 : ℤ) * (2 : ℝ) ^ (Int.log 2 |x| - 23) := by
      rw [← zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0)]
      congr 1
      ring
    have h24 : (2 : ℝ) ^ (24 : ℤ) = 16777216 := by norm_num
    have ht : (0 : ℝ) < (2 : ℝ) ^ (Int.log 2 |x| - 23) := by positivity
    have hpow' : (2 : ℝ) ^ (Int.log 2 |x| + 1)
        = 16777216 * (2 : ℝ) ^ (Int.log 2 |x| - 23) := by
      rw [hpow, h24]
    have hyabs : |y| = |(R : ℝ)| * (2 : ℝ) ^ (Int.log 2 |x| - 23) := by
      rw [hy, abs_mul, abs_of_pos ht]
    have hNR : |(R : ℝ)| = ((|R| : ℤ) : ℝ) := by simp
    have hyN : |y| = ((|R| : ℤ) : ℝ) * (2 : ℝ) ^ (Int.log 2 |x| - 23) := by
      rw [hyabs, hNR]
    have hN2 : (16777216 : ℝ) ≤ ((|R| : ℤ) : ℝ) := by
      have h := h2
      rw [hpow', hyN] at h
      exact le_of_mul_le_mul_right h ht
    have hB : |y| < 16777216 * (2 : ℝ) ^ (Int.log 2 |x| - 23)
        + (2 : ℝ) ^ (Int.log 2 |x| - 23) / 2 := by
      rw [← hpow']
      linarith
    have hN3 : ((|R| : ℤ) : ℝ) < 16777216 + 1 / 2 := by
      have h := hB
      rw [hyN] at h
      have h' : ((|R| : ℤ) : ℝ) * (2 : ℝ) ^ (Int.log 2 |x| - 23)
          < (16777216 + 1 / 2) * (2 : ℝ) ^ (Int.log 2 |x| - 23) := by
        linarith
      exact lt_of_mul_lt_mul_right h' ht.le
    have hNa : (16777216 : ℤ) ≤ |R| := by exact_mod_cast hN2
    have hNb : |R| < 16777217 := by
      have : ((|R| : ℤ) : ℝ) < 16777217 := by linarith
      exact_mod_cast this
    have hNeq : |R| = 16777216 := by omega
    have hNcast : ((|R| : ℤ) : ℝ) = 16777216 := by
      rw [hNeq]
      norm_num
    have hRv : R = 16777216 ∨ R = -16777216 := by
      rcases abs_cases R with ⟨h, _⟩ | ⟨h, _⟩
      · left
        rw [h] at hNeq
        exact hNeq
      · right
        rw [h] at hNeq
        omega
    have hLy2 : Int.log 2 |y| ≤ Int.log 2 |x| + 1 := by
      by_contra hc
      push_neg at hc
      have hYeq : |y| = (2 : ℝ) ^ (Int.log 2 |x| + 1) := by
        rw [hyN, hNcast, hpow']
      have h3 : (2 : ℝ) ^ (Int.log 2 |x| + 2) ≤ (2 : ℝ) ^ Int.log 2 |y| :=
        zpow_le_zpow_right₀ (by norm_num) (by omega)
      have h4 : (2 : ℝ) ^ (Int.log 2 |x| + 2) = 2 * (2 : ℝ) ^ (Int.log 2 |x| + 1) := by
        rw [show Int.log 2 |x| + 2 = (Int.log 2 |x| + 1) + 1 by ring,
          zpow_add_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
        ring
      have hpos2 : (0 : ℝ) < (2 : ℝ) ^ (Int.log 2 |x| + 1) := by positivity
      linarith
    have hLyeq : Int.log 2 |y| = Int.log 2 |x| + 1 := by omega
    refine ⟨R / 2, ?_⟩
    have hR2 : (R : ℝ) = 2 * ((R / 2 : ℤ) : ℝ) := by
      have hR : R = 2 * (R / 2) := by
        rcases hRv with h | h <;> omega
      exact_mod_cast hR
    rw [hu', hLyeq, hy, hR2,
      show Int.log 2 |x| + 1 - 23 = (Int.log 2 |x| - 23) + 1 by ring,
      zpow_add_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
    ring
  · -- Subnormal x: the unit of x is `2^-149`, and the unit cannot rise.
    exfalso
    have hu : ulpExp x = -149 := by
      unfold ulpExp
      rw [max_eq_right (by omega : Int.log 2 |x| ≤ -126)]
      norm_num
    rw [hu] at hsub hgt
    have hLy : -125 ≤ Int.log 2 |y| := by
      by_contra hc
      push_neg at hc
      have hu2 : ulpExp y = -149 := by
        unfold ulpExp
        rw [max_eq_right (by omega : Int.log 2 |y| ≤ -126)]
        norm_num
      omega
    have h1 : (2 : ℝ) ^ (-125 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 |y| :=
      zpow_le_zpow_right₀ (by norm_num) hLy
    have h2 : (2 : ℝ) ^ (Int.log 2 |x| + 1) ≤ (2 : ℝ) ^ (-126 : ℤ) :=
      zpow_le_zpow_right₀ (by norm_num) (by omega)
    have h4 : (2 : ℝ) ^ (-125 : ℤ) = 2 * (2 : ℝ) ^ (-126 : ℤ) := by
      rw [show (-125 : ℤ) = -126 + 1 by norm_num, zpow_add_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
      ring
    have h5 : (2 : ℝ) ^ (-150 : ℤ) ≤ (2 : ℝ) ^ (-126 : ℤ) :=
      zpow_le_zpow_right₀ (by norm_num) (by norm_num)
    have e : (2 : ℝ) ^ (-149 : ℤ) / 2 = (2 : ℝ) ^ (-150 : ℤ) := by
      rw [show (-150 : ℤ) = -149 - 1 by norm_num, zpow_sub_one₀ (by norm_num : (2 : ℝ) ≠ 0)]
      ring
    rw [e] at hsub
    linarith

/-- Rounding is idempotent: a value of `rn` is fixed by `rn`. The base case of the error
induction needs this, since `rn (0 + rn p)` is then `rn p` with no further error. -/
theorem rn_idem (x : ℝ) : rn (rn x) = rn x := by
  by_cases hx : x = 0
  · subst hx
    simp [rn_zero]
  · obtain ⟨R, hR⟩ : ∃ R : ℤ, roundEven (x / 2 ^ ulpExp x) = R := ⟨_, rfl⟩
    have hdef : rn x = (R : ℝ) * 2 ^ ulpExp x := by
      unfold rn
      rw [if_neg hx, hR]
    have hsub : |rn x - x| ≤ (2 : ℝ) ^ ulpExp x / 2 := rn_sub_le hx
    by_cases hy0 : rn x = 0
    · rw [hy0, rn_zero]
    · obtain ⟨K, hK⟩ : ∃ K : ℤ, rn x = (K : ℝ) * 2 ^ ulpExp (rn x) := by
        rcases le_or_lt (ulpExp (rn x)) (ulpExp x) with hf | hf
        · exact grid_of_le hdef hf
        · exact grid_of_gt hx hdef hy0 hsub hf
      exact rn_of_grid K hK

end Gorse.Spec.Dot
