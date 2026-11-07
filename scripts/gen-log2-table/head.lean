import Mathlib
import Gorse.Spec.Float32

/-! # The ndcg log table is the binary32 rounding of log2 k (P4-10, D-354)

`ndcg` reads `log2 (i + 2)` as a binary32 value. For the integers `k = 2..65`, which cover rank
lists of up to 64 entries, the value is read from the table `LOG2_TABLE` of
crates/gorse-model/src/cf/evaluator.rs. Each entry is a bit pattern. This file proves that each
entry is `rn (Real.logb 2 k)`, the rounding of the exact logarithm to binary32 (`rn` of
`Gorse.Spec.Float32`), for each of the 64 values of `k`, with the decoding `decodeBits`.

The method, for each entry: the exact `logb 2 k` lies inside the rounding interval of the entry.
That is shown from bounds on `exp` (`Real.exp_bound`, with the exponent written as `8 · y`), from
the bounds `0.6931471803 < log 2 < 0.6931471808` (`Real.log_two_gt_d9`, `Real.log_two_lt_d9`),
and from rational values checked by `norm_num`. The entry lemmas are generated from exact
arithmetic, and the G9 oracle `oracle_log2` checks the same table independently.
-/

namespace Gorse.Spec.Log2Table

open Gorse.Spec.Float32

/-- The binary32 value with bit pattern `b`, for a normal number: the significand `b % 2^23` with
its hidden bit, times `2^(exponent - 150)`, where the exponent field is `b / 2^23 % 256`. -/
noncomputable def decodeBits (b : ℕ) : ℝ :=
  ((2 ^ 23 + b % 2 ^ 23 : ℕ) : ℝ) * (2 : ℝ) ^ (((b / 2 ^ 23 % 256 : ℕ) : ℤ) - 150)

/-- A value in `[2^n, 2^(n+1))` has unit `2^(n - 23)`, when `n ≥ -126`. -/
theorem ulpExp_of_bounds {x : ℝ} {n : ℤ} (hx : 0 < x) (hn : -126 ≤ n)
    (h1 : (2 : ℝ) ^ n ≤ x) (h2 : x < (2 : ℝ) ^ (n + 1)) :
    ulpExp x = n - 23 := by
  have hpos : 0 < |x| := abs_pos.mpr hx.ne'
  have hxa : |x| = x := abs_of_pos hx
  have h1' : ((2 : ℕ) : ℝ) ^ n ≤ |x| := by
    rw [hxa]
    exact_mod_cast h1
  have h2' : |x| < ((2 : ℕ) : ℝ) ^ (n + 1) := by
    rw [hxa]
    exact_mod_cast h2
  have a := (Int.zpow_le_iff_le_log (by norm_num) hpos).mp h1'
  have b := (Int.lt_zpow_iff_log_lt (by norm_num) hpos).mp h2'
  have hl : Int.log 2 |x| = n := by omega
  unfold ulpExp
  rw [hl, max_eq_left hn]

/-- A value within `1/2` of an integer `m` rounds to `m`. -/
theorem roundEven_of_near (r : ℝ) (m : ℤ) (h : |r - (m : ℝ)| < 1 / 2) :
    roundEven r = m := by
  rcases abs_lt.mp h with ⟨hl, hu⟩
  rcases le_or_gt 0 (r - m) with h0 | h0
  · have hf : ⌊r⌋ = m := by
      rw [Int.floor_eq_iff]
      constructor <;> push_cast <;> linarith
    unfold roundEven
    rw [hf]
    have c1 : r - (m : ℝ) < 1 / 2 := by linarith
    rw [if_pos c1]
  · have hf : ⌊r⌋ = m - 1 := by
      rw [Int.floor_eq_iff]
      constructor <;> push_cast <;> linarith
    unfold roundEven
    rw [hf]
    have c2 : ¬ (r - ((m - 1 : ℤ) : ℝ) < 1 / 2) := not_lt.mpr (by push_cast; linarith)
    have c3 : 1 / 2 < r - ((m - 1 : ℤ) : ℝ) := by push_cast; linarith
    rw [if_neg c2, if_pos c3]
    ring

/-- A value within half a unit of a binary32 grid point with significand `m` and unit `2^e`
rounds to that grid point. The binade of the grid point is `e + 23`. -/
theorem rn_eq_of_near {x : ℝ} {m e : ℤ} (hm1 : 2 ^ 23 < m) (hm2 : m < 2 ^ 24)
    (hx : |x - (m : ℝ) * (2 : ℝ) ^ e| < (2 : ℝ) ^ e / 2) (hn : -126 ≤ e + 23) :
    rn x = (m : ℝ) * (2 : ℝ) ^ e := by
  have hm1i : (2 : ℤ) ^ 23 + 1 ≤ m := by omega
  have hm2i : m + 1 ≤ (2 : ℤ) ^ 24 := by omega
  have hm1r : (2 : ℝ) ^ 23 + 1 ≤ (m : ℝ) := by exact_mod_cast hm1i
  have hm2r : (m : ℝ) + 1 ≤ (2 : ℝ) ^ 24 := by exact_mod_cast hm2i
  have hP : (0 : ℝ) < (2 : ℝ) ^ e := zpow_pos (by norm_num) e
  have hT : (2 : ℝ) ^ e ≠ 0 := hP.ne'
  have hE23 : (2 : ℝ) ^ (e + 23) = 8388608 * (2 : ℝ) ^ e := by
    rw [zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0), mul_comm]
    norm_num
  have hE24 : (2 : ℝ) ^ (e + 23 + 1) = 16777216 * (2 : ℝ) ^ e := by
    rw [zpow_add_one₀ (by norm_num : (2 : ℝ) ≠ 0), hE23]
    ring
  have h3 := mul_le_mul_of_nonneg_right hm1r hP.le
  have h4 := mul_le_mul_of_nonneg_right hm2r hP.le
  have hlo : (2 : ℝ) ^ (e + 23) < x := by
    have := (abs_sub_lt_iff.mp hx).2
    rw [hE23]
    linarith
  have hhi : x < (2 : ℝ) ^ (e + 23 + 1) := by
    have := (abs_sub_lt_iff.mp hx).1
    rw [hE24]
    linarith
  have hxpos : 0 < x := lt_trans (by positivity) hlo
  have hux : ulpExp x = e := by
    rw [ulpExp_of_bounds hxpos hn hlo.le hhi]
    ring
  have hq : |x / (2 : ℝ) ^ e - (m : ℝ)| < 1 / 2 := by
    have hd : x / (2 : ℝ) ^ e - (m : ℝ) = (x - (m : ℝ) * (2 : ℝ) ^ e) / (2 : ℝ) ^ e := by
      rw [sub_div, mul_div_assoc, div_self hT, mul_one]
    rw [hd, abs_div, abs_of_pos hP, div_lt_iff hP]
    linarith
  unfold rn
  rw [if_neg hxpos.ne', hux, roundEven_of_near _ m hq]

/-- An entry of the table: if the bounds of the rounding interval hold, the rounding is the
entry. -/
theorem rn_entry {x c h : ℝ} {m e : ℤ} (hm1 : 2 ^ 23 < m) (hm2 : m < 2 ^ 24) (hn : -126 ≤ e + 23)
    (hc : c = (m : ℝ) * (2 : ℝ) ^ e) (hh : h = (2 : ℝ) ^ e / 2)
    (hlo : c - h < x) (hhi : x < c + h) :
    rn x = (m : ℝ) * (2 : ℝ) ^ e := by
  refine rn_eq_of_near hm1 hm2 ?_ hn
  rw [← hc, ← hh, abs_sub_lt_iff]
  constructor <;> linarith

/-- If `exp (log 2 · t)` is above `k`, then `logb 2 k < t`. -/
theorem logb_lt_of_lt_exp {k t : ℝ} (hk : 0 < k) (h : k < Real.exp (Real.log 2 * t)) :
    Real.logb 2 k < t := by
  have h2 : 0 < Real.log 2 := Real.log_pos (by norm_num)
  have h2' : Real.log 2 ≠ 0 := ne_of_gt h2
  have hL : Real.logb 2 k * Real.log 2 = Real.log k := by
    unfold Real.logb
    field_simp
  have h3 := Real.log_lt_log hk h
  rw [Real.log_exp] at h3
  nlinarith

/-- If `exp (log 2 · t)` is below `k`, then `t < logb 2 k`. -/
theorem lt_logb_of_exp_lt {k t : ℝ} (hk : 0 < k) (h : Real.exp (Real.log 2 * t) < k) :
    t < Real.logb 2 k := by
  have h2 : 0 < Real.log 2 := Real.log_pos (by norm_num)
  have h2' : Real.log 2 ≠ 0 := ne_of_gt h2
  have hL : Real.logb 2 k * Real.log 2 = Real.log k := by
    unfold Real.logb
    field_simp
  have h3 := Real.log_lt_log (Real.exp_pos _) h
  rw [Real.log_exp] at h3
  nlinarith

/-- The degree-11 Taylor sum of `exp y`, less its error bound, is below `exp y` on `[0, 1]`. -/
theorem exp_lower (y : ℝ) (h0 : 0 ≤ y) (h1 : y ≤ 1) :
    (∑ i ∈ Finset.range 12, y ^ i / (Nat.factorial i : ℝ))
      - y ^ 12 * (13 / ((Nat.factorial 12 : ℝ) * 12)) ≤ Real.exp y := by
  have h := Real.exp_bound (x := y) (abs_le.mpr ⟨by linarith, h1⟩) (n := 12) (by norm_num)
  rw [abs_of_nonneg h0] at h
  have h2 := (abs_le.mp h).1
  norm_num [Finset.sum_range_succ, Nat.factorial] at h2 ⊢
  linarith

/-- The degree-11 Taylor sum of `exp y`, plus its error bound, is above `exp y` on `[0, 1]`. -/
theorem exp_upper (y : ℝ) (h0 : 0 ≤ y) (h1 : y ≤ 1) :
    Real.exp y ≤
      (∑ i ∈ Finset.range 12, y ^ i / (Nat.factorial i : ℝ))
        + y ^ 12 * (13 / ((Nat.factorial 12 : ℝ) * 12)) := by
  have h := Real.exp_bound (x := y) (abs_le.mpr ⟨by linarith, h1⟩) (n := 12) (by norm_num)
  rw [abs_of_nonneg h0] at h
  have h2 := (abs_le.mp h).2
  norm_num [Finset.sum_range_succ, Nat.factorial] at h2 ⊢
  linarith

/-- The logarithm `logb 2 k` lies in `(c - h, c + h)`, from bounds on `exp` at `8 y1` and `8 y2`. -/
theorem logb_mem_of_bounds {k c h y1 y2 L U : ℝ} (hk : 0 < k)
    (hy1 : 8 * y1 ≤ Real.log 2 * (c + h)) (hL0 : 0 ≤ L) (hLe : L ≤ Real.exp y1)
    (hLk : k < L ^ 8) (hy2 : Real.log 2 * (c - h) ≤ 8 * y2) (hU0 : 0 ≤ U)
    (hUe : Real.exp y2 ≤ U) (hUk : U ^ 8 < k) :
    c - h < Real.logb 2 k ∧ Real.logb 2 k < c + h := by
  constructor
  · apply lt_logb_of_exp_lt hk
    calc Real.exp (Real.log 2 * (c - h)) ≤ Real.exp (8 * y2) := Real.exp_le_exp.mpr hy2
      _ = Real.exp y2 ^ 8 := by rw [← Real.exp_nat_mul]; norm_num
      _ ≤ U ^ 8 := pow_le_pow_left (Real.exp_pos y2).le hUe 8
      _ < k := hUk
  · apply logb_lt_of_lt_exp hk
    calc k < L ^ 8 := hLk
      _ ≤ Real.exp y1 ^ 8 := pow_le_pow_left hL0 hLe 8
      _ = Real.exp (8 * y1) := by rw [← Real.exp_nat_mul]; norm_num
      _ ≤ Real.exp (Real.log 2 * (c + h)) := Real.exp_le_exp.mpr hy1

/-- `logb 2 2 = 1`, and the value 1 lies on the binary32 grid. -/
theorem entry_2 : rn (Real.logb 2 2) = decodeBits 0x3F800000 := by
  have hd : decodeBits 0x3F800000 = 1 := by norm_num [decodeBits]
  rw [hd, Real.logb_self_eq_one (b := 2) (by norm_num)]
  have hu : ulpExp 1 = -23 := by
    have := ulpExp_of_bounds (x := 1) (n := 0) one_pos (by norm_num) (by norm_num) (by norm_num)
    rw [this]
    norm_num
  exact rn_of_grid (x := 1) (2 ^ 23) (by rw [hu]; norm_num)

/-- `logb 2 4 = 2`, and the value 2 lies on the binary32 grid. -/
theorem entry_4 : rn (Real.logb 2 4) = decodeBits 0x40000000 := by
  have hd : decodeBits 0x40000000 = 2 := by norm_num [decodeBits]
  have h2 : Real.log 2 ≠ 0 := (Real.log_pos (by norm_num)).ne'
  have hl : Real.logb 2 4 = 2 := by
    unfold Real.logb
    rw [show (4 : ℝ) = 2 ^ 2 by norm_num, Real.log_pow, mul_div_assoc, div_self h2]
    norm_num
  rw [hd, hl]
  have hu : ulpExp 2 = -22 := by
    have := ulpExp_of_bounds (x := 2) (n := 1) two_pos (by norm_num) (by norm_num) (by norm_num)
    rw [this]
    norm_num
  exact rn_of_grid (x := 2) (2 ^ 23) (by rw [hu]; norm_num)

/-- `logb 2 16 = 4`, and the value 4 lies on the binary32 grid. -/
theorem entry_16 : rn (Real.logb 2 16) = decodeBits 0x40800000 := by
  have hd : decodeBits 0x40800000 = 4 := by norm_num [decodeBits]
  have h2 : Real.log 2 ≠ 0 := (Real.log_pos (by norm_num)).ne'
  have hl : Real.logb 2 16 = 4 := by
    unfold Real.logb
    rw [show (16 : ℝ) = 2 ^ 4 by norm_num, Real.log_pow, mul_div_assoc, div_self h2]
    norm_num
  rw [hd, hl]
  have hu : ulpExp 4 = -21 := by
    have := ulpExp_of_bounds (x := 4) (n := 2) (by norm_num) (by norm_num) (by norm_num)
      (by norm_num)
    rw [this]
    norm_num
  exact rn_of_grid (x := 4) (2 ^ 23) (by rw [hu]; norm_num)

