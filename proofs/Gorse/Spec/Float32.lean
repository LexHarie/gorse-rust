import Mathlib

/-! # The binary32 rounding model (P4-1, T3 of the verification plan)

`rn` rounds a real number to the nearest binary32 value, ties to even, with subnormals.
It is defined here, not assumed (plan §2.2, item 1): no `axiom`, and no hypothesis about
the grid. The unit in the last place at `x` is `2^(max(⌊log₂|x|⌋, -126) - 23)`. In the
normal range that gives 24 significand bits. Below `2^-126` the unit stays at the subnormal
unit `2^-149` (IEEE 754-2019 §3.3 and §3.4).

This file proves the exactness and error lemmas that P4-2 uses. P4-2 proves `rn_model`,
the error form `x(1+δ)+ε`, and it also states the overflow threshold. Decision D-337. -/

namespace Gorse.Spec.Float32

/-- Round a real number to the nearest integer, ties to even (IEEE 754-2019
roundTiesToEven, on ℝ). -/
noncomputable def roundEven (r : ℝ) : ℤ :=
  if r - (⌊r⌋ : ℝ) < 1 / 2 then ⌊r⌋
  else if 1 / 2 < r - (⌊r⌋ : ℝ) then ⌊r⌋ + 1
  else if Even ⌊r⌋ then ⌊r⌋ else ⌊r⌋ + 1

/-- The unit in the last place at `x`: `2^(max(⌊log₂|x|⌋, -126) - 23)`. Below `2^-126` the
unit is `2^-149`, the subnormal unit (IEEE 754-2019 §3.4). -/
noncomputable def ulpExp (x : ℝ) : ℤ := max (Int.log 2 |x|) (-126) - 23

/-- Round `x` to the binary32 grid: to nearest, ties to even, with subnormals. `rn 0 = 0`. -/
noncomputable def rn (x : ℝ) : ℝ :=
  if x = 0 then 0 else (roundEven (x / 2 ^ ulpExp x) : ℝ) * 2 ^ ulpExp x

/-- Rounding a real to an integer moves it by at most one half. -/
theorem roundEven_sub_le (r : ℝ) : |(roundEven r : ℝ) - r| ≤ 1 / 2 := by
  have h1 : (⌊r⌋ : ℝ) ≤ r := Int.floor_le r
  have h2 : r < (⌊r⌋ : ℝ) + 1 := Int.lt_floor_add_one r
  unfold roundEven
  split_ifs <;> (try push_cast) <;> rw [abs_le] <;> constructor <;> linarith

/-- An integer is its own rounding. -/
theorem roundEven_intCast (k : ℤ) : roundEven (k : ℝ) = k := by
  unfold roundEven
  rw [Int.floor_intCast, sub_self, if_pos (by norm_num : (0 : ℝ) < 1 / 2)]

/-- The rounding of zero is zero. -/
theorem rn_zero : rn 0 = 0 := by
  simp [rn]

/-- The unit never goes below the subnormal unit `2^-149`. -/
theorem ulpExp_ge (x : ℝ) : (-149 : ℤ) ≤ ulpExp x := by
  unfold ulpExp
  have h := le_max_right (Int.log 2 |x|) (-126)
  linarith

/-- A value that already lies on the grid is fixed by `rn`. The grid point is written as an
integer multiple of its unit `2^ulpExp x`. -/
theorem rn_of_grid {x : ℝ} (k : ℤ) (h : x = (k : ℝ) * 2 ^ ulpExp x) : rn x = x := by
  by_cases hx : x = 0
  · subst hx
    exact rn_zero
  · have hne : (2 : ℝ) ^ ulpExp x ≠ 0 := by positivity
    have hdiv : x / 2 ^ ulpExp x = (k : ℝ) := by
      rw [div_eq_iff hne]
      exact h
    unfold rn
    rw [if_neg hx, hdiv, roundEven_intCast]
    exact h.symm

/-- The rounding error is at most half a unit in the last place. -/
theorem rn_sub_le {x : ℝ} (hx : x ≠ 0) : |rn x - x| ≤ (2 : ℝ) ^ ulpExp x / 2 := by
  have hpos : (0 : ℝ) < 2 ^ ulpExp x := by positivity
  have key : rn x - x
      = ((roundEven (x / 2 ^ ulpExp x) : ℝ) - x / 2 ^ ulpExp x) * 2 ^ ulpExp x := by
    unfold rn
    rw [if_neg hx]
    field_simp
  have h := roundEven_sub_le (x / 2 ^ ulpExp x)
  have h2 := mul_le_mul_of_nonneg_right h hpos.le
  rw [key, abs_mul, abs_of_pos hpos]
  linarith

/-- Exactness of subnormal addition (Higham 2002 §2.2; plan §2.2 item 1). A value on the
binary32 grid is an integer multiple of the subnormal unit `2^-149`. So when `a` and `b` are
on the grid and their sum lies in the subnormal range, the sum is on the grid too, and `rn`
leaves it unchanged. -/
theorem rn_add_subnormal {a b : ℝ} (ha : rn a = a) (hb : rn b = b)
    (hs : |a + b| < (2 : ℝ) ^ (-126 : ℤ)) : rn (a + b) = a + b := by
  -- Every grid point is an integer multiple of the subnormal unit.
  have hmul : ∀ x : ℝ, rn x = x → ∃ k : ℤ, x = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ) := by
    intro x hx
    by_cases hx0 : x = 0
    · exact ⟨0, by simp [hx0]⟩
    · have hdef : rn x = (roundEven (x / 2 ^ ulpExp x) : ℝ) * 2 ^ ulpExp x := by
        unfold rn
        rw [if_neg hx0]
      obtain ⟨R, hR⟩ : ∃ R : ℤ, roundEven (x / 2 ^ ulpExp x) = R := ⟨_, rfl⟩
      rw [hR] at hdef
      obtain ⟨n, hn⟩ : ∃ n : ℕ, (n : ℤ) = ulpExp x + 149 :=
        ⟨(ulpExp x + 149).toNat, Int.toNat_of_nonneg (by linarith [ulpExp_ge x])⟩
      have he : (2 : ℝ) ^ ulpExp x = (2 : ℝ) ^ n * (2 : ℝ) ^ (-149 : ℤ) := by
        rw [← zpow_natCast, ← zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0)]
        congr 1
        omega
      have hx2 : (R : ℝ) * 2 ^ ulpExp x = x := by
        rw [← hdef]
        exact hx
      refine ⟨R * 2 ^ n, ?_⟩
      push_cast
      rw [← hx2, he]
      ring
  rcases eq_or_ne (a + b) 0 with hz | hz
  · rw [hz, rn_zero]
  · obtain ⟨ka, hka⟩ := hmul a ha
    obtain ⟨kb, hkb⟩ := hmul b hb
    have hsum : a + b = ((ka + kb : ℤ) : ℝ) * (2 : ℝ) ^ (-149 : ℤ) := by
      rw [hka, hkb]
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
    exact rn_of_grid (ka + kb) (by rw [hu]; exact hsum)

end Gorse.Spec.Float32
