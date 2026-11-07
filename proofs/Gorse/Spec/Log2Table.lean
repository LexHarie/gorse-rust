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
theorem exp_lower {y : ℝ} (h0 : 0 ≤ y) (h1 : y ≤ 1) :
    (∑ i ∈ Finset.range 12, y ^ i / (Nat.factorial i : ℝ))
      - y ^ 12 * (13 / ((Nat.factorial 12 : ℝ) * 12)) ≤ Real.exp y := by
  have h := Real.exp_bound (x := y) (abs_le.mpr ⟨by linarith, h1⟩) (n := 12) (by norm_num)
  rw [abs_of_nonneg h0] at h
  have h2 := (abs_le.mp h).1
  norm_num [Finset.sum_range_succ, Nat.factorial] at h2 ⊢
  linarith

/-- The degree-11 Taylor sum of `exp y`, plus its error bound, is above `exp y` on `[0, 1]`. -/
theorem exp_upper {y : ℝ} (h0 : 0 ≤ y) (h1 : y ≤ 1) :
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

theorem decodeBits_3 : decodeBits 0x3FCAE00D = (13295629 : ℝ) * (2:ℝ)^(-23 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_5 : decodeBits 0x40149A78 = (9738872 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_6 : decodeBits 0x40257007 = (10842119 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_7 : decodeBits 0x4033ABB4 = (11774900 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_8 : decodeBits 0x40400000 = (12582912 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_9 : decodeBits 0x404AE00D = (13295629 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_10 : decodeBits 0x40549A78 = (13933176 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_11 : decodeBits 0x405D6754 = (14509908 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_12 : decodeBits 0x40657007 = (15036423 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_13 : decodeBits 0x406CD401 = (15520769 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_14 : decodeBits 0x4073ABB4 = (15969204 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_15 : decodeBits 0x407A0A7F = (16386687 : ℝ) * (2:ℝ)^(-22 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_17 : decodeBits 0x4082CC7F = (8572031 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_18 : decodeBits 0x40857007 = (8744967 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_19 : decodeBits 0x4087EF06 = (8908550 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_20 : decodeBits 0x408A4D3C = (9063740 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_21 : decodeBits 0x408C8DDD = (9211357 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_22 : decodeBits 0x408EB3AA = (9352106 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_23 : decodeBits 0x4090C105 = (9486597 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_24 : decodeBits 0x4092B803 = (9615363 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_25 : decodeBits 0x40949A78 = (9738872 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_26 : decodeBits 0x40966A01 = (9857537 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_27 : decodeBits 0x4098280A = (9971722 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_28 : decodeBits 0x4099D5DA = (10081754 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_29 : decodeBits 0x409B7495 = (10187925 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_30 : decodeBits 0x409D053F = (10290495 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_31 : decodeBits 0x409E88C7 = (10389703 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_32 : decodeBits 0x40A00000 = (10485760 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_33 : decodeBits 0x40A16BAD = (10578861 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_34 : decodeBits 0x40A2CC7F = (10669183 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_35 : decodeBits 0x40A42316 = (10756886 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_36 : decodeBits 0x40A57007 = (10842119 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_37 : decodeBits 0x40A6B3D8 = (10925016 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_38 : decodeBits 0x40A7EF06 = (11005702 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_39 : decodeBits 0x40A92204 = (11084292 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_40 : decodeBits 0x40AA4D3C = (11160892 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_41 : decodeBits 0x40AB7111 = (11235601 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_42 : decodeBits 0x40AC8DDD = (11308509 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_43 : decodeBits 0x40ADA3F6 = (11379702 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_44 : decodeBits 0x40AEB3AA = (11449258 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_45 : decodeBits 0x40AFBD43 = (11517251 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_46 : decodeBits 0x40B0C105 = (11583749 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_47 : decodeBits 0x40B1BF31 = (11648817 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_48 : decodeBits 0x40B2B803 = (11712515 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_49 : decodeBits 0x40B3ABB4 = (11774900 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_50 : decodeBits 0x40B49A78 = (11836024 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_51 : decodeBits 0x40B58482 = (11895938 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_52 : decodeBits 0x40B66A01 = (11954689 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_53 : decodeBits 0x40B74B20 = (12012320 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_54 : decodeBits 0x40B8280A = (12068874 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_55 : decodeBits 0x40B900E6 = (12124390 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_56 : decodeBits 0x40B9D5DA = (12178906 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_57 : decodeBits 0x40BAA709 = (12232457 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_58 : decodeBits 0x40BB7495 = (12285077 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_59 : decodeBits 0x40BC3E9D = (12336797 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_60 : decodeBits 0x40BD053F = (12387647 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_61 : decodeBits 0x40BDC89A = (12437658 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_62 : decodeBits 0x40BE88C7 = (12486855 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_63 : decodeBits 0x40BF45E1 = (12535265 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_64 : decodeBits 0x40C00000 = (12582912 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem decodeBits_65 : decodeBits 0x40C0B73D = (12629821 : ℝ) * (2:ℝ)^(-21 : ℤ) := by
  norm_num [decodeBits]


theorem entry_3 : rn (Real.logb 2 3) = decodeBits 0x3FCAE00D := by
  rw [decodeBits_3]
  have hb := logb_mem_of_bounds (k := 3) (c := (13295629 / 8388608 : ℝ)) (h := (1 / 16777216 : ℝ))
    (y1 := (150992127561 / 1099511627776 : ℝ)) (y2 := (75496058157 / 549755813888 : ℝ)) (L := (1261362702527 / 1099511627776 : ℝ)) (U := (630681344813 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (150992127561 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (75496058157 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 13295629) (e := -23) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_5 : rn (Real.logb 2 5) = decodeBits 0x40149A78 := by
  rw [decodeBits_5]
  have hb := logb_mem_of_bounds (k := 5) (c := (1217359 / 524288 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (221199466911 / 1099511627776 : ℝ)) (y2 := (110599722179 / 549755813888 : ℝ)) (L := (84033237609 / 68719476736 : ℝ)) (U := (672265887083 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (221199466911 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (110599722179 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9738872) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_6 : rn (Real.logb 2 6) = decodeBits 0x40257007 := by
  rw [decodeBits_6]
  have hb := logb_mem_of_bounds (k := 6) (c := (10842119 / 4194304 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (123128783829 / 549755813888 : ℝ)) (y2 := (61564386281 / 274877906944 : ℝ)) (L := (1375525802067 / 1099511627776 : ℝ)) (U := (687762886939 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (123128783829 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (61564386281 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10842119) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_7 : rn (Real.logb 2 7) = decodeBits 0x4033ABB4 := by
  rw [decodeBits_7]
  have hb := logb_mem_of_bounds (k := 7) (c := (2943725 / 1048576 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (267443866169 / 1099511627776 : ℝ)) (y2 := (133721921825 / 549755813888 : ℝ)) (L := (701143787853 / 549755813888 : ℝ)) (U := (1402287546987 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (267443866169 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (133721921825 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11774900) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_8 : rn (Real.logb 2 8) = decodeBits 0x40400000 := by
  rw [decodeBits_8]
  have hb := logb_mem_of_bounds (k := 8) (c := (3 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (8931133767 / 34359738368 : ℝ)) (y2 := (142898129019 / 549755813888 : ℝ)) (L := (1425890184287 / 1099511627776 : ℝ)) (U := (1425890155101 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (8931133767 / 34359738368 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (142898129019 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12582912) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_9 : rn (Real.logb 2 9) = decodeBits 0x404AE00D := by
  rw [decodeBits_9]
  have hb := logb_mem_of_bounds (k := 9) (c := (13295629 / 4194304 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (301984255123 / 1099511627776 : ℝ)) (y2 := (75496058157 / 274877906944 : ℝ)) (L := (723519345833 / 549755813888 : ℝ)) (U := (723519331031 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (301984255123 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (75496058157 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 13295629) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_10 : rn (Real.logb 2 10) = decodeBits 0x40549A78 := by
  rw [decodeBits_10]
  have hb := logb_mem_of_bounds (k := 10) (c := (1741647 / 524288 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (316464889973 / 1099511627776 : ℝ)) (y2 := (158232433745 / 549755813888 : ℝ)) (L := (183277790821 / 137438953472 : ℝ)) (U := (1466222296587 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (316464889973 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (158232433745 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 13933176) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_11 : rn (Real.logb 2 11) = decodeBits 0x405D6754 := by
  rw [decodeBits_11]
  have hb := logb_mem_of_bounds (k := 11) (c := (3627477 / 1048576 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (329564230883 / 1099511627776 : ℝ)) (y2 := (41195526051 / 137438953472 : ℝ)) (L := (1483795049425 / 1099511627776 : ℝ)) (U := (185474377387 / 137438953472 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (329564230883 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (41195526051 / 137438953472 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 14509908) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_12 : rn (Real.logb 2 12) = decodeBits 0x40657007 := by
  rw [decodeBits_12]
  have hb := logb_mem_of_bounds (k := 12) (c := (15036423 / 4194304 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (341522990721 / 1099511627776 : ℝ)) (y2 := (341522968255 / 1099511627776 : ℝ)) (L := (1500021523587 / 1099511627776 : ℝ)) (U := (1500021492939 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (341522990721 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (341522968255 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 15036423) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_13 : rn (Real.logb 2 13) = decodeBits 0x406CD401 := by
  rw [decodeBits_13]
  have hb := logb_mem_of_bounds (k := 13) (c := (15520769 / 4194304 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (44065495511 / 137438953472 : ℝ)) (y2 := (176261970815 / 549755813888 : ℝ)) (L := (378776265365 / 274877906944 : ℝ)) (U := (757552515257 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (44065495511 / 137438953472 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (176261970815 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 15520769) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_14 : rn (Real.logb 2 14) = decodeBits 0x4073ABB4 := by
  rw [decodeBits_14]
  have hb := logb_mem_of_bounds (k := 14) (c := (3992301 / 1048576 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (22669330577 / 68719476736 : ℝ)) (y2 := (362709266781 / 1099511627776 : ℝ)) (L := (1529205444679 / 1099511627776 : ℝ)) (U := (1529205413455 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (22669330577 / 68719476736 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (362709266781 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 15969204) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_15 : rn (Real.logb 2 15) = decodeBits 0x407A0A7F := by
  rw [decodeBits_15]
  have hb := logb_mem_of_bounds (k := 15) (c := (16386687 / 4194304 : ℝ)) (h := (1 / 8388608 : ℝ))
    (y1 := (372191600151 / 1099511627776 : ℝ)) (y2 := (372191577707 / 1099511627776 : ℝ)) (L := (1542450514391 / 1099511627776 : ℝ)) (U := (771225241453 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (372191600151 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (372191577707 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 16386687) (e := -22) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_17 : rn (Real.logb 2 17) = decodeBits 0x4082CC7F := by
  rw [decodeBits_17]
  have hb := logb_mem_of_bounds (k := 17) (c := (8572031 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (194696952665 / 549755813888 : ℝ)) (y2 := (194696930093 / 549755813888 : ℝ)) (L := (391693136481 / 274877906944 : ℝ)) (U := (1566772481597 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (194696952665 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (194696930093 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 8572031) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_18 : rn (Real.logb 2 18) = decodeBits 0x40857007 := by
  rw [decodeBits_18]
  have hb := logb_mem_of_bounds (k := 18) (c := (8744967 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (397249712255 / 1099511627776 : ℝ)) (y2 := (99312416779 / 274877906944 : ℝ)) (L := (1578006931573 / 1099511627776 : ℝ)) (U := (1578006866791 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (397249712255 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (99312416779 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 8744967) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_19 : rn (Real.logb 2 19) = decodeBits 0x4087EF06 := by
  rw [decodeBits_19]
  have hb := logb_mem_of_bounds (k := 19) (c := (4454275 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (404680648925 / 1099511627776 : ℝ)) (y2 := (25292537737 / 68719476736 : ℝ)) (L := (99294240595 / 68719476736 : ℝ)) (U := (1588707784307 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (404680648925 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (25292537737 / 68719476736 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 8908550) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_20 : rn (Real.logb 2 20) = decodeBits 0x408A4D3C := by
  rw [decodeBits_20]
  have hb := logb_mem_of_bounds (k := 20) (c := (2265935 / 524288 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (51466290549 / 137438953472 : ℝ)) (y2 := (12866571227 / 34359738368 : ℝ)) (L := (99932925087 / 68719476736 : ℝ)) (U := (1598926735767 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (51466290549 / 137438953472 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (12866571227 / 34359738368 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9063740) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_21 : rn (Real.logb 2 21) = decodeBits 0x408C8DDD := by
  rw [decodeBits_21]
  have hb := logb_mem_of_bounds (k := 21) (c := (9211357 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (418435988053 / 1099511627776 : ℝ)) (y2 := (418435942929 / 1099511627776 : ℝ)) (L := (1608708077657 / 1099511627776 : ℝ)) (U := (1608708011637 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (418435988053 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (418435942929 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9211357) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_22 : rn (Real.logb 2 22) = decodeBits 0x408EB3AA := by
  rw [decodeBits_22]
  have hb := logb_mem_of_bounds (k := 22) (c := (4676053 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (212414832651 / 549755813888 : ℝ)) (y2 := (424829620183 / 1099511627776 : ℝ)) (L := (404522497937 / 274877906944 : ℝ)) (U := (809044962675 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (212414832651 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (424829620183 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9352106) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_23 : rn (Real.logb 2 23) = decodeBits 0x4090C105 := by
  rw [decodeBits_23]
  have hb := logb_mem_of_bounds (k := 23) (c := (9486597 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (430939066057 / 1099511627776 : ℝ)) (y2 := (430939020943 / 1099511627776 : ℝ)) (L := (1627105881443 / 1099511627776 : ℝ)) (U := (813552907341 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (430939066057 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (430939020943 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9486597) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_24 : rn (Real.logb 2 24) = decodeBits 0x4092B803 := by
  rw [decodeBits_24]
  have hb := logb_mem_of_bounds (k := 24) (c := (9615363 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (436788402427 / 1099511627776 : ℝ)) (y2 := (436788357317 / 1099511627776 : ℝ)) (L := (1635785053687 / 1099511627776 : ℝ)) (U := (1635784986577 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (436788402427 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (436788357317 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9615363) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_25 : rn (Real.logb 2 25) = decodeBits 0x40949A78 := by
  rw [decodeBits_25]
  have hb := logb_mem_of_bounds (k := 25) (c := (1217359 / 262144 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (221199466911 / 549755813888 : ℝ)) (y2 := (110599722179 / 274877906944 : ℝ)) (L := (822076693067 / 549755813888 : ℝ)) (U := (822076659343 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (221199466911 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (110599722179 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9738872) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_26 : rn (Real.logb 2 26) = decodeBits 0x40966A01 := by
  rw [decodeBits_26]
  have hb := logb_mem_of_bounds (k := 26) (c := (9857537 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (111947355305 / 274877906944 : ℝ)) (y2 := (223894688059 / 549755813888 : ℝ)) (L := (103264614779 / 68719476736 : ℝ)) (U := (1652233768691 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (111947355305 / 274877906944 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (223894688059 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9857537) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_27 : rn (Real.logb 2 27) = decodeBits 0x4098280A := by
  rw [decodeBits_27]
  have hb := logb_mem_of_bounds (k := 27) (c := (4985861 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (452976399719 / 1099511627776 : ℝ)) (y2 := (452976354621 / 1099511627776 : ℝ)) (L := (830023356257 / 549755813888 : ℝ)) (U := (830023322213 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (452976399719 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (452976354621 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 9971722) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_28 : rn (Real.logb 2 28) = decodeBits 0x4099D5DA := by
  rw [decodeBits_28]
  have hb := logb_mem_of_bounds (k := 28) (c := (5040877 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (457974723651 / 1099511627776 : ℝ)) (y2 := (114493669639 / 274877906944 : ℝ)) (L := (833805189713 / 549755813888 : ℝ)) (U := (1667610311033 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (457974723651 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (114493669639 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10081754) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_29 : rn (Real.logb 2 29) = decodeBits 0x409B7495 := by
  rw [decodeBits_29]
  have hb := logb_mem_of_bounds (k := 29) (c := (10187925 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (462797657435 / 1099511627776 : ℝ)) (y2 := (57849701543 / 137438953472 : ℝ)) (L := (52341915835 / 34359738368 : ℝ)) (U := (104683827377 / 68719476736 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (462797657435 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (57849701543 / 137438953472 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10187925) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_30 : rn (Real.logb 2 30) = decodeBits 0x409D053F := by
  rw [decodeBits_30]
  have hb := logb_mem_of_bounds (k := 30) (c := (10290495 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (467457011857 / 1099511627776 : ℝ)) (y2 := (467456966769 / 1099511627776 : ℝ)) (L := (1682054195769 / 1099511627776 : ℝ)) (U := (1682054126793 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (467457011857 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (467456966769 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10290495) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_31 : rn (Real.logb 2 31) = decodeBits 0x409E88C7 := by
  rw [decodeBits_31]
  have hb := logb_mem_of_bounds (k := 31) (c := (10389703 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (471963643751 / 1099511627776 : ℝ)) (y2 := (471963598667 / 1099511627776 : ℝ)) (L := (1688962676999 / 1099511627776 : ℝ)) (U := (844481303873 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (471963643751 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (471963598667 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10389703) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_32 : rn (Real.logb 2 32) = decodeBits 0x40A00000 := by
  rw [decodeBits_32]
  have hb := logb_mem_of_bounds (k := 32) (c := (5 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (476327138025 / 1099511627776 : ℝ)) (y2 := (29770443309 / 68719476736 : ℝ)) (L := (1695678769769 / 1099511627776 : ℝ)) (U := (847839350123 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (476327138025 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (29770443309 / 68719476736 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10485760) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_33 : rn (Real.logb 2 33) = decodeBits 0x40A16BAD := by
  rw [decodeBits_33]
  have hb := logb_mem_of_bounds (k := 33) (c := (10578861 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (240278176383 / 549755813888 : ℝ)) (y2 := (60069538461 / 137438953472 : ℝ)) (L := (851106835341 / 549755813888 : ℝ)) (U := (13298543757 / 8589934592 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (240278176383 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (60069538461 / 137438953472 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10578861) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_34 : rn (Real.logb 2 34) = decodeBits 0x40A2CC7F := by
  rw [decodeBits_34]
  have hb := logb_mem_of_bounds (k := 34) (c := (10669183 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (484659328393 / 1099511627776 : ℝ)) (y2 := (484659283317 / 1099511627776 : ℝ)) (L := (1708577576603 / 1099511627776 : ℝ)) (U := (1708577506559 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (484659328393 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (484659283317 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10669183) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_35 : rn (Real.logb 2 35) = decodeBits 0x40A42316 := by
  rw [decodeBits_35]
  have hb := logb_mem_of_bounds (k := 35) (c := (5378443 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (488643333081 / 1099511627776 : ℝ)) (y2 := (61080411001 / 137438953472 : ℝ)) (L := (1714779719559 / 1099511627776 : ℝ)) (U := (1714779649265 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (488643333081 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (61080411001 / 137438953472 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10756886) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_36 : rn (Real.logb 2 36) = decodeBits 0x40A57007 := by
  rw [decodeBits_36]
  have hb := logb_mem_of_bounds (k := 36) (c := (10842119 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (492515135317 / 1099511627776 : ℝ)) (y2 := (492515090247 / 1099511627776 : ℝ)) (L := (860414380511 / 549755813888 : ℝ)) (U := (1720828690485 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (492515135317 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (492515090247 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10842119) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_37 : rn (Real.logb 2 37) = decodeBits 0x40A6B3D8 := by
  rw [decodeBits_37]
  have hb := logb_mem_of_bounds (k := 37) (c := (1365627 / 262144 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (496280822199 / 1099511627776 : ℝ)) (y2 := (124070194283 / 274877906944 : ℝ)) (L := (1726732483701 / 1099511627776 : ℝ)) (U := (1726732412927 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (496280822199 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (124070194283 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 10925016) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_38 : rn (Real.logb 2 38) = decodeBits 0x40A7EF06 := by
  rw [decodeBits_38]
  have hb := logb_mem_of_bounds (k := 38) (c := (5502851 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (124986517997 / 274877906944 : ℝ)) (y2 := (499946026923 / 1099511627776 : ℝ)) (L := (216562274349 / 137438953472 : ℝ)) (U := (216562265473 / 137438953472 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (124986517997 / 274877906944 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (499946026923 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11005702) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_39 : rn (Real.logb 2 39) = decodeBits 0x40A92204 := by
  rw [decodeBits_39]
  have hb := logb_mem_of_bounds (k := 39) (c := (2771073 / 524288 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (503516108685 / 1099511627776 : ℝ)) (y2 := (503516063623 / 1099511627776 : ℝ)) (L := (1738132636589 / 1099511627776 : ℝ)) (U := (1738132565355 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (503516108685 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (503516063623 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11084292) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_40 : rn (Real.logb 2 40) = decodeBits 0x40AA4D3C := by
  rw [decodeBits_40]
  have hb := logb_mem_of_bounds (k := 40) (c := (2790223 / 524288 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (506995747455 / 1099511627776 : ℝ)) (y2 := (126748925599 / 274877906944 : ℝ)) (L := (1743642040827 / 1099511627776 : ℝ)) (U := (1743641969373 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (506995747455 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (126748925599 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11160892) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_41 : rn (Real.logb 2 41) = decodeBits 0x40AB7111 := by
  rw [decodeBits_41]
  have hb := logb_mem_of_bounds (k := 41) (c := (11235601 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (255194742741 / 549755813888 : ℝ)) (y2 := (510389440425 / 1099511627776 : ℝ)) (L := (874516128831 / 549755813888 : ℝ)) (U := (874516092995 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (255194742741 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (510389440425 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11235601) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_42 : rn (Real.logb 2 42) = decodeBits 0x40AC8DDD := by
  rw [decodeBits_42]
  have hb := logb_mem_of_bounds (k := 42) (c := (11308509 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (513701411115 / 1099511627776 : ℝ)) (y2 := (513701366061 / 1099511627776 : ℝ)) (L := (438577149557 / 274877906944 : ℝ)) (U := (219288565793 / 137438953472 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (513701411115 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (513701366061 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11308509) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_43 : rn (Real.logb 2 43) = decodeBits 0x40ADA3F6 := by
  rw [decodeBits_43]
  have hb := logb_mem_of_bounds (k := 43) (c := (5689851 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (516935430997 / 1099511627776 : ℝ)) (y2 := (516935385945 / 1099511627776 : ℝ)) (L := (1759476184167 / 1099511627776 : ℝ)) (U := (1759476112075 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (516935430997 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (516935385945 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11379702) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_44 : rn (Real.logb 2 44) = decodeBits 0x40AEB3AA := by
  rw [decodeBits_44]
  have hb := logb_mem_of_bounds (k := 44) (c := (5724629 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (130023772091 / 274877906944 : ℝ)) (y2 := (260047521657 / 549755813888 : ℝ)) (L := (1764539648091 / 1099511627776 : ℝ)) (U := (1764539575795 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (130023772091 / 274877906944 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (260047521657 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11449258) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_45 : rn (Real.logb 2 45) = decodeBits 0x40AFBD43 := by
  rw [decodeBits_45]
  have hb := logb_mem_of_bounds (k := 45) (c := (11517251 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (523183744747 / 1099511627776 : ℝ)) (y2 := (523183699699 / 1099511627776 : ℝ)) (L := (1769503414371 / 1099511627776 : ℝ)) (U := (884751670937 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (523183744747 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (523183699699 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11517251) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_46 : rn (Real.logb 2 46) = decodeBits 0x40B0C105 := by
  rw [decodeBits_46]
  have hb := logb_mem_of_bounds (k := 46) (c := (11583749 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (16443890285 / 34359738368 : ℝ)) (y2 := (263102222037 / 549755813888 : ℝ)) (L := (1774371545521 / 1099511627776 : ℝ)) (U := (443592868207 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (16443890285 / 34359738368 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (263102222037 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11583749) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_47 : rn (Real.logb 2 47) = decodeBits 0x40B1BF31 := by
  rw [decodeBits_47]
  have hb := logb_mem_of_bounds (k := 47) (c := (11648817 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (529160274179 / 1099511627776 : ℝ)) (y2 := (529160229135 / 1099511627776 : ℝ)) (L := (1779147954089 / 1099511627776 : ℝ)) (U := (444786970301 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (529160274179 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (529160229135 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11648817) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_48 : rn (Real.logb 2 48) = decodeBits 0x40B2B803 := by
  rw [decodeBits_48]
  have hb := logb_mem_of_bounds (k := 48) (c := (11712515 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (532053825489 / 1099511627776 : ℝ)) (y2 := (16626680639 / 34359738368 : ℝ)) (L := (1783836249965 / 1099511627776 : ℝ)) (U := (1783836176893 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (532053825489 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (16626680639 / 34359738368 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11712515) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_49 : rn (Real.logb 2 49) = decodeBits 0x40B3ABB4 := by
  rw [decodeBits_49]
  have hb := logb_mem_of_bounds (k := 49) (c := (2943725 / 524288 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (534887732339 / 1099511627776 : ℝ)) (y2 := (133721921825 / 274877906944 : ℝ)) (L := (894219940611 / 549755813888 : ℝ)) (U := (447109951991 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (534887732339 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (133721921825 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11774900) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_50 : rn (Real.logb 2 50) = decodeBits 0x40B49A78 := by
  rw [decodeBits_50]
  have hb := logb_mem_of_bounds (k := 50) (c := (1479503 / 262144 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (537664356885 / 1099511627776 : ℝ)) (y2 := (537664311847 / 1099511627776 : ℝ)) (L := (1792961981209 / 1099511627776 : ℝ)) (U := (224120238471 / 137438953472 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (537664356885 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (537664311847 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11836024) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_51 : rn (Real.logb 2 51) = decodeBits 0x40B58482 := by
  rw [decodeBits_51]
  have hb := logb_mem_of_bounds (k := 51) (c := (5947969 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (540386015857 / 1099511627776 : ℝ)) (y2 := (270192985411 / 549755813888 : ℝ)) (L := (1797405659231 / 1099511627776 : ℝ)) (U := (1797405585613 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (540386015857 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (270192985411 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11895938) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_52 : rn (Real.logb 2 52) = decodeBits 0x40B66A01 := by
  rw [decodeBits_52]
  have hb := logb_mem_of_bounds (k := 52) (c := (11954689 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (543054844283 / 1099511627776 : ℝ)) (y2 := (543054799249 / 1099511627776 : ℝ)) (L := (1801773774777 / 1099511627776 : ℝ)) (U := (900886850491 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (543054844283 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (543054799249 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 11954689) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_53 : rn (Real.logb 2 53) = decodeBits 0x40B74B20 := by
  rw [decodeBits_53]
  have hb := logb_mem_of_bounds (k := 53) (c := (375385 / 65536 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (545672795483 / 1099511627776 : ℝ)) (y2 := (136418187613 / 274877906944 : ℝ)) (L := (1806068932483 / 1099511627776 : ℝ)) (U := (1806068858517 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (545672795483 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (136418187613 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12012320) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_54 : rn (Real.logb 2 54) = decodeBits 0x40B8280A := by
  rw [decodeBits_54]
  have hb := logb_mem_of_bounds (k := 54) (c := (6034437 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (548241822781 / 1099511627776 : ℝ)) (y2 := (68530222219 / 137438953472 : ℝ)) (L := (905146888261 / 549755813888 : ℝ)) (U := (905146851193 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (548241822781 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (68530222219 / 137438953472 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12068874) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_55 : rn (Real.logb 2 55) = decodeBits 0x40B900E6 := by
  rw [decodeBits_55]
  have hb := logb_mem_of_bounds (k := 55) (c := (6062195 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (275381848897 / 549755813888 : ℝ)) (y2 := (275381826383 / 549755813888 : ℝ)) (L := (907225344791 / 549755813888 : ℝ)) (U := (1814450615277 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (275381848897 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (275381826383 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12124390) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_56 : rn (Real.logb 2 56) = decodeBits 0x40B9D5DA := by
  rw [decodeBits_56]
  have hb := logb_mem_of_bounds (k := 56) (c := (6089453 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (553240146713 / 1099511627776 : ℝ)) (y2 := (553240101687 / 1099511627776 : ℝ)) (L := (113658875861 / 68719476736 : ℝ)) (U := (454635484827 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (553240146713 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (553240101687 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12178906) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_57 : rn (Real.logb 2 57) = decodeBits 0x40BAA709 := by
  rw [decodeBits_57]
  have hb := logb_mem_of_bounds (k := 57) (c := (12232457 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (138918189863 / 274877906944 : ℝ)) (y2 := (138918178607 / 274877906944 : ℝ)) (L := (1822569898241 / 1099511627776 : ℝ)) (U := (1822569823611 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (138918189863 / 274877906944 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (138918178607 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12232457) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_58 : rn (Real.logb 2 58) = decodeBits 0x40BB7495 := by
  rw [decodeBits_58]
  have hb := logb_mem_of_bounds (k := 58) (c := (12285077 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (279031540249 / 549755813888 : ℝ)) (y2 := (558063035475 / 1099511627776 : ℝ)) (L := (1826536446679 / 1099511627776 : ℝ)) (U := (114158523243 / 68719476736 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (279031540249 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (558063035475 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12285077) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_59 : rn (Real.logb 2 59) = decodeBits 0x40BC3E9D := by
  rw [decodeBits_59]
  have hb := logb_mem_of_bounds (k := 59) (c := (12336797 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (560412518059 / 1099511627776 : ℝ)) (y2 := (280206236519 / 549755813888 : ℝ)) (L := (915221782245 / 549755813888 : ℝ)) (U := (1830443489543 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (560412518059 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (280206236519 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12336797) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_60 : rn (Real.logb 2 60) = decodeBits 0x40BD053F := by
  rw [decodeBits_60]
  have hb := logb_mem_of_bounds (k := 60) (c := (12387647 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (562722434919 / 1099511627776 : ℝ)) (y2 := (140680597475 / 274877906944 : ℝ)) (L := (917146553593 / 549755813888 : ℝ)) (U := (458573258021 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (562722434919 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (140680597475 / 274877906944 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12387647) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_61 : rn (Real.logb 2 61) = decodeBits 0x40BDC89A := by
  rw [decodeBits_61]
  have hb := logb_mem_of_bounds (k := 61) (c := (6218829 / 1048576 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (564994239287 / 1099511627776 : ℝ)) (y2 := (564994194269 / 1099511627776 : ℝ)) (L := (919043515403 / 549755813888 : ℝ)) (U := (1838086955551 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (564994239287 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (564994194269 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12437658) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_62 : rn (Real.logb 2 62) = decodeBits 0x40BE88C7 := by
  rw [decodeBits_62]
  have hb := logb_mem_of_bounds (k := 62) (c := (12486855 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (283614533407 / 549755813888 : ℝ)) (y2 := (283614510899 / 549755813888 : ℝ)) (L := (920913429695 / 549755813888 : ℝ)) (U := (1841826783985 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (283614533407 / 549755813888 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (283614510899 / 549755813888 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12486855) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_63 : rn (Real.logb 2 63) = decodeBits 0x40BF45E1 := by
  rw [decodeBits_63]
  have hb := logb_mem_of_bounds (k := 63) (c := (12535265 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (569428144005 / 1099511627776 : ℝ)) (y2 := (569428098991 / 1099511627776 : ℝ)) (L := (1845514289749 / 1099511627776 : ℝ)) (U := (461378553549 / 274877906944 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (569428144005 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (569428098991 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12535265) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_64 : rn (Real.logb 2 64) = decodeBits 0x40C00000 := by
  rw [decodeBits_64]
  have hb := logb_mem_of_bounds (k := 64) (c := (6 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (8931133767 / 17179869184 : ℝ)) (y2 := (571592516075 / 1099511627776 : ℝ)) (L := (1849150810489 / 1099511627776 : ℝ)) (U := (1849150734789 / 1099511627776 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (8931133767 / 17179869184 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (571592516075 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12582912) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


theorem entry_65 : rn (Real.logb 2 65) = decodeBits 0x40C0B73D := by
  rw [decodeBits_65]
  have hb := logb_mem_of_bounds (k := 65) (c := (12629821 / 2097152 : ℝ)) (h := (1 / 4194304 : ℝ))
    (y1 := (573723453713 / 1099511627776 : ℝ)) (y2 := (573723408701 / 1099511627776 : ℝ)) (L := (926369002717 / 549755813888 : ℝ)) (U := (926368964795 / 549755813888 : ℝ))
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower (573723453713 / 1099511627776 : ℝ) (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper (573723408701 / 1099511627776 : ℝ) (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := 12629821) (e := -21) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num


end Gorse.Spec.Log2Table
