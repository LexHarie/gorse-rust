import Mathlib
import Gorse.Spec.Float32
import Gorse.Spec.Log2Table

/-!
# The mean over users does not depend on the split across jobs (D-353, P5-5, D-362)

`evaluate` (`crates/gorse-model/src/cf/evaluator.rs`) adds each user's metric value into a
per-job sum, and then adds the job sums in job-id order. Which users a job receives depends on
scheduling (`parallel.rs` hands out user indices from an atomic counter). Float addition is not
associative, so before the fix the binary32 mean can change in its last bits between runs when
`n_jobs > 1`. That breaks the tier I contract that a build is deterministic for a given input
(D-313).

The file has three parts, in this order.

1. **The bug (pre-fix), a counterexample.** `preFix groups` is the sum the code computed before
   the fix: each job folds its users' values from `0` in claim order, then the job partials are
   folded from `0` in job-id order. `preFix_not_schedule_independent` gives two schedules of the
   same three values with two different binary32 results.
2. **The fix.** `postFix values` folds the values in user order, which is how the code now stores
   them, by user index. It takes no schedule, so no split across jobs can change it.
   `postFix_eq_one` shows the fixed sum of the same three values, in user order, is `1` for every
   schedule.
3. **The rounding facts** that the counterexample needs, over the binary32 model `rn` of
   `Gorse.Spec.Float32`. They use the unit of `Gorse.Spec.Log2Table.ulpExp_of_bounds`.
-/

namespace Gorse.Spec.UserSum

open Gorse.Spec.Float32

/-- A left fold of binary32 additions from `acc`: each addition rounds, as `*total += value` does. -/
noncomputable def fsumL : ℝ → List ℝ → ℝ
  | acc, [] => acc
  | acc, x :: xs => fsumL (rn (acc + x)) xs

/-- Before the fix: each job's partial sum is the fold of its users' values from `0`, and the job
partials are folded from `0` in job-id order. `groups` lists the users of each job, in claim
order. -/
noncomputable def preFix (groups : List (List ℝ)) : ℝ :=
  fsumL 0 (groups.map (fsumL 0))

/-- After the fix: the values are stored by user index and folded in user order. The argument is
the user-ordered list alone, so no split across jobs can change the result. -/
noncomputable def postFix (values : List ℝ) : ℝ :=
  fsumL 0 values

/-- The unit `3/4 · 2^-24` of the counterexample. -/
noncomputable def uVal : ℝ := (3 / 4 : ℝ) * (2 : ℝ) ^ (-24 : ℤ)

theorem uVal_pos : 0 < uVal := by
  unfold uVal
  positivity

/-! ## Rounding ties and near-ties, at `1 + x` -/

/-- An integer plus a fraction below one half rounds to the integer. -/
theorem roundEven_add_of_lt_half (m : ℤ) {t : ℝ} (h0 : 0 ≤ t) (h1 : t < 1 / 2) :
    roundEven ((m : ℝ) + t) = m := by
  have hf : ⌊(m : ℝ) + t⌋ = m := Int.floor_eq_iff.mpr ⟨by linarith, by linarith⟩
  unfold roundEven
  rw [hf, show (m : ℝ) + t - (m : ℝ) = t by ring, if_pos h1]

/-- An integer plus a fraction strictly between one half and one rounds up. -/
theorem roundEven_add_of_gt_half (m : ℤ) {t : ℝ} (h1 : 1 / 2 < t) (h2 : t < 1) :
    roundEven ((m : ℝ) + t) = m + 1 := by
  have hf : ⌊(m : ℝ) + t⌋ = m := Int.floor_eq_iff.mpr ⟨by linarith, by linarith⟩
  unfold roundEven
  rw [hf, show (m : ℝ) + t - (m : ℝ) = t by ring, if_neg (not_lt.mpr h1.le), if_pos h1]

/-- `1 + x` for a small `x ≥ 0` below the half-unit `2^-24` rounds down to `1`. -/
theorem rn_one_add_of_lt {x : ℝ} (h0 : 0 ≤ x) (h1 : x < (2 : ℝ) ^ (-24 : ℤ)) :
    rn (1 + x) = 1 := by
  have h24 : (2 : ℝ) ^ (-24 : ℤ) < 1 := by norm_num [zpow_neg]
  have hpos : 0 < 1 + x := by linarith
  have hu : ulpExp (1 + x) = -23 := by
    have := Gorse.Spec.Log2Table.ulpExp_of_bounds hpos (n := 0) (by norm_num)
      (by norm_num; linarith) (by norm_num; linarith)
    rw [this]
    norm_num
  have hscale : (1 + x) / (2 : ℝ) ^ (-23 : ℤ) = ((2 ^ 23 : ℤ) : ℝ) + 2 ^ 23 * x := by
    norm_num [zpow_neg] <;> ring
  have ht : 0 ≤ (2 : ℝ) ^ 23 * x := mul_nonneg (by norm_num) h0
  have ht' : (2 : ℝ) ^ 23 * x < 1 / 2 := by
    have := mul_lt_mul_of_pos_left h1 (show (0 : ℝ) < 2 ^ 23 by norm_num)
    have e : (2 : ℝ) ^ 23 * (2 : ℝ) ^ (-24 : ℤ) = 1 / 2 := by norm_num [zpow_neg]
    exact this.trans_eq e
  have hm := roundEven_add_of_lt_half (2 ^ 23) ht ht'
  unfold rn
  rw [if_neg hpos.ne', hu, hscale, hm]
  norm_num [zpow_neg]

/-- `1 + x` for `x` strictly between `2^-24` and `2^-23` rounds up to `1 + 2^-23`. -/
theorem rn_one_add_of_gt {x : ℝ} (h1 : (2 : ℝ) ^ (-24 : ℤ) < x) (h2 : x < (2 : ℝ) ^ (-23 : ℤ)) :
    rn (1 + x) = 1 + (2 : ℝ) ^ (-23 : ℤ) := by
  have h23 : (2 : ℝ) ^ (-23 : ℤ) < 1 := by norm_num [zpow_neg]
  have h24 : (0 : ℝ) < (2 : ℝ) ^ (-24 : ℤ) := by positivity
  have hpos : 0 < 1 + x := by linarith
  have hu : ulpExp (1 + x) = -23 := by
    have := Gorse.Spec.Log2Table.ulpExp_of_bounds hpos (n := 0) (by norm_num)
      (by norm_num; linarith) (by norm_num; linarith)
    rw [this]
    norm_num
  have hscale : (1 + x) / (2 : ℝ) ^ (-23 : ℤ) = ((2 ^ 23 : ℤ) : ℝ) + 2 ^ 23 * x := by
    norm_num [zpow_neg] <;> ring
  have ht1 : 1 / 2 < (2 : ℝ) ^ 23 * x := by
    have := mul_lt_mul_of_pos_left h1 (show (0 : ℝ) < 2 ^ 23 by norm_num)
    have e : (2 : ℝ) ^ 23 * (2 : ℝ) ^ (-24 : ℤ) = 1 / 2 := by norm_num [zpow_neg]
    exact e.symm.trans_lt this
  have ht2 : (2 : ℝ) ^ 23 * x < 1 := by
    have := mul_lt_mul_of_pos_left h2 (show (0 : ℝ) < 2 ^ 23 by norm_num)
    have e : (2 : ℝ) ^ 23 * (2 : ℝ) ^ (-23 : ℤ) = 1 := by norm_num [zpow_neg]
    linarith
  have hm := roundEven_add_of_gt_half (2 ^ 23) ht1 ht2
  unfold rn
  rw [if_neg hpos.ne', hu, hscale, hm]
  norm_num [zpow_neg]

/-! ## The units of the counterexample lie on the binary32 grid -/

/-- `uVal` is in `[2^-25, 2^-24)`, so its unit is `2^-48`. -/
theorem ulpExp_uVal : ulpExp uVal = -48 := by
  have h1 : (2 : ℝ) ^ (-25 : ℤ) ≤ uVal := by
    unfold uVal
    norm_num [zpow_neg]
  have h2 : uVal < (2 : ℝ) ^ ((-25 : ℤ) + 1) := by
    unfold uVal
    norm_num [zpow_neg]
  have := Gorse.Spec.Log2Table.ulpExp_of_bounds uVal_pos (by norm_num) h1 h2
  rw [this]
  norm_num

/-- `rn uVal = uVal`: `uVal` is `12582912 · 2^-48`, a grid point. -/
theorem rn_uVal : rn uVal = uVal := by
  apply rn_of_grid (12582912 : ℤ)
  rw [ulpExp_uVal]
  unfold uVal
  norm_num [zpow_neg]

/-- `2 · uVal` is in `[2^-24, 2^-23)`, so its unit is `2^-47`. -/
theorem ulpExp_twice_uVal : ulpExp (uVal + uVal) = -47 := by
  have h1 : (2 : ℝ) ^ (-24 : ℤ) ≤ uVal + uVal := by
    unfold uVal
    norm_num [zpow_neg]
  have h2 : uVal + uVal < (2 : ℝ) ^ ((-24 : ℤ) + 1) := by
    unfold uVal
    norm_num [zpow_neg]
  have := Gorse.Spec.Log2Table.ulpExp_of_bounds (by linarith [uVal_pos]) (by norm_num) h1 h2
  rw [this]
  norm_num

/-- `rn (uVal + uVal) = uVal + uVal`: `2 · uVal` is `12582912 · 2^-47`, a grid point. -/
theorem rn_twice_uVal : rn (uVal + uVal) = uVal + uVal := by
  apply rn_of_grid (12582912 : ℤ)
  rw [ulpExp_twice_uVal]
  unfold uVal
  norm_num [zpow_neg]

/-- `rn 1 = 1`. -/
theorem rn_one : rn 1 = 1 := by
  have h := rn_one_add_of_lt (x := 0) le_rfl (by norm_num [zpow_neg])
  simpa using h

/-! ## The bug and the fix -/

/-- The three values of the counterexample sum to `1 + 2^-23` under one split, and to `1` under
another. -/
theorem preFix_A : preFix [[1], [uVal, uVal]] = 1 + (2 : ℝ) ^ (-23 : ℤ) := by
  have hq : fsumL 0 [uVal, uVal] = uVal + uVal := by
    simp only [fsumL, zero_add, rn_uVal, rn_twice_uVal]
  have hs : fsumL 0 [(1 : ℝ)] = 1 := by
    simp only [fsumL, zero_add, rn_one]
  have hp : preFix [[1], [uVal, uVal]] = fsumL 0 [1, uVal + uVal] := by
    simp only [preFix, List.map_cons, List.map_nil, hq, hs]
  rw [hp]
  have hlo : (2 : ℝ) ^ (-24 : ℤ) < uVal + uVal := by
    unfold uVal
    norm_num [zpow_neg]
  have hhi : uVal + uVal < (2 : ℝ) ^ (-23 : ℤ) := by
    unfold uVal
    norm_num [zpow_neg]
  simp only [fsumL, zero_add, rn_one]
  exact rn_one_add_of_gt hlo hhi

theorem preFix_B : preFix [[1, uVal], [uVal]] = 1 := by
  have hpos : (0 : ℝ) ≤ uVal := uVal_pos.le
  have hlt : uVal < (2 : ℝ) ^ (-24 : ℤ) := by
    unfold uVal
    norm_num [zpow_neg]
  have h1 : fsumL 0 [1, uVal] = 1 := by
    simp only [fsumL, zero_add, rn_one]
    exact rn_one_add_of_lt hpos hlt
  have h2 : fsumL 0 [uVal] = uVal := by
    simp only [fsumL, zero_add, rn_uVal]
  simp only [preFix, List.map_cons, List.map_nil, h1, h2]

/-- **The bug (pre-fix).** Two schedules of the same three values give two different binary32
sums, so the mean depended on the split across jobs. -/
theorem preFix_not_schedule_independent :
    preFix [[1], [uVal, uVal]] ≠ preFix [[1, uVal], [uVal]] := by
  rw [preFix_A, preFix_B]
  intro h
  have hp : (0 : ℝ) < (2 : ℝ) ^ (-23 : ℤ) := by positivity
  linarith

/-- **The fix.** The post-fix sum of the same three values, in user order, is `1`. The fixed sum
takes the user-ordered list alone, so every split gives this value. -/
theorem postFix_eq_one : postFix [1, uVal, uVal] = 1 := by
  have h : rn (1 + uVal) = 1 := rn_one_add_of_lt uVal_pos.le (by
    unfold uVal
    norm_num [zpow_neg])
  simp only [postFix, fsumL, zero_add, rn_one, h]

end Gorse.Spec.UserSum
