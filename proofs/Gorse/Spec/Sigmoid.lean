import Mathlib.Analysis.SpecialFunctions.Exp
import Mathlib.Data.Complex.ExponentialBounds
import Mathlib.Tactic.Positivity
import Mathlib.Tactic.Linarith
import Mathlib.Tactic.NormNum
import Gorse.Spec.Float32Model

/-!
# The stable sigmoid of BPR's gradient (A8, P5-2, D-358)

BPR's update (`crates/gorse-model/src/cf/model.rs`, `bpr_update`) multiplies the factors by the
gradient factor `σ(−d)`, where `d` is the margin `⟨w, h_i⟩ − ⟨w, h_j⟩` and `σ(x) = 1 / (1 + e^{−x})`.
The code computed it as `exp(−d) / (1 + exp(−d))`. For `d ≤ −89` the f32 `exp` overflows to +∞, and
`∞ / ∞` is NaN (plan §3.2, A8; F5).

Over the reals the old formula is correct, so the bug is an f32 fact. The file has three parts.

1. **The ℝ statements.** `sigStable` is the stable form: `exp` is applied only at non-positive
   arguments. `sigStable_eq` shows it equals σ for every real `x`, and `sigStable_mem_Icc` shows it
   lies in `[0, 1]`.
2. **A binary32 model of `math32::exp` on x86-64 (`Bin32`, `fexpAmd`).** A finite result is the
   rounding `rn` of a real (Gorse.Spec.Float32). `math32::exp` is Go's `exp_amd64.s`, and its two
   decisions are modelled exactly: with `k = CVTSS2SL(LOG2E · x)`, the result is `+∞` when
   `k ≥ 128` and `0` when `k ≤ −127`, and `cvtss2sl` saturates to `i32::MIN` for large `x`. A finite
   result is the correctly rounded `exp`, which stands in for the assembly's polynomial (a stated gap).
   `fadd1` and `fdiv` round once each, and `∞ / ∞` is NaN. `oldB` is the code before the fix, and
   `newB` is the stable form, both over this model.
3. **The bug and the fix, in that model.** `oldB_nan_of_mem`: for `89 ≤ x ≤ 2^20` the old formula is
   NaN, because `k ≥ 128` there (`amdK_overflow`), which breaks "finite and in [0, 1]"
   (`oldB_not_spec`). `newB_mem`: the stable form is finite and in `[0, 1]` for every real `x`, and
   each `exp` in it has a non-positive argument, where `amdK` is at most `0` (`amdK_le_zero`).

The Rust `exp` is not correctly rounded, and the model follows the amd64 code, not IEEE's overflow
rule. The IEEE rule would overflow from 88.72, but the amd64 code overflows from `k = 128`, about
88.40, and returns `0` from about 1.49e9 on. The bug theorem is stated only on `[89, 2^20]`, where
both decisions are as modelled. The Rust regression test (`bpr_update_stays_finite_at_margin_minus_89`)
shows the same NaN at margin −89. The tier II error bound of the stable form is still open (D-328).
-/

namespace Gorse.Spec.Sigmoid

open Gorse.Spec.Float32

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

/-! ## Rounding on the binary32 grid

`rn` (Gorse.Spec.Float32) rounds to the binary32 grid, with the unit `2^ulpExp w`. The lemmas below
show that `rn` maps `[0, 1]` into `[0, 1]`, and `[1, 2]` into `[1, ∞)`. Both follow from `roundEven`
being an integer, and from the grid points `1` and `0` being integer multiples of the unit. -/

theorem roundEven_nonneg {q : ℝ} (hq : 0 ≤ q) : 0 ≤ roundEven q := by
  have h1 : (0 : ℤ) ≤ ⌊q⌋ := Int.floor_nonneg.mpr hq
  unfold roundEven
  split_ifs <;> omega

theorem roundEven_ge_of_le {q : ℝ} {N : ℤ} (h : (N : ℝ) ≤ q) : N ≤ roundEven q := by
  have hfl : N ≤ ⌊q⌋ := Int.le_floor.mpr h
  unfold roundEven
  split_ifs <;> omega

theorem roundEven_le_of_le {q : ℝ} {N : ℤ} (h : q ≤ (N : ℝ)) : roundEven q ≤ N := by
  have h1 : (⌊q⌋ : ℝ) ≤ q := Int.floor_le q
  have hfl : ⌊q⌋ ≤ N := by
    have h2 : (⌊q⌋ : ℝ) ≤ (N : ℝ) := le_trans h1 h
    exact_mod_cast h2
  unfold roundEven
  split_ifs with hA hB hC
  · exact hfl
  · have h2 : (⌊q⌋ : ℝ) < (N : ℝ) := by linarith
    have h3 : ⌊q⌋ < N := by exact_mod_cast h2
    omega
  · exact hfl
  · have hA' : 1 / 2 ≤ q - (⌊q⌋ : ℝ) := by
      push_neg at hA
      exact hA
    have h2 : (⌊q⌋ : ℝ) < (N : ℝ) := by linarith
    have h3 : ⌊q⌋ < N := by exact_mod_cast h2
    omega

/-- For `0 < w ≤ 2` the exponent of the unit is at most `-22`. -/
theorem ulpExp_le_of_le_two {w : ℝ} (hw : 0 < w) (hw2 : w ≤ 2) : ulpExp w ≤ -22 := by
  have hpos : (0 : ℝ) < |w| := abs_pos.mpr hw.ne'
  have hlo : (2 : ℝ) ^ Int.log 2 |w| ≤ |w| := by
    exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hpos
  have h2 : |w| ≤ 2 := by
    rw [abs_of_pos hw]
    exact hw2
  have hL : Int.log 2 |w| ≤ 1 := by
    by_contra hc
    push_neg at hc
    have h3 : (2 : ℝ) ^ (2 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 |w| :=
      zpow_le_zpow_right₀ (by norm_num) (by omega)
    rw [zpow_two] at h3
    linarith
  unfold ulpExp
  have hmax := max_le hL (by norm_num : (-126 : ℤ) ≤ 1)
  omega

/-- A unit exponent at most `0` is a power of two inverted: some `2^n` times `2^e` is `1`. -/
theorem exists_pow_mul_ulp {w : ℝ} (he : ulpExp w ≤ 0) :
    ∃ n : ℕ, (2 : ℝ) ^ n * (2 : ℝ) ^ ulpExp w = 1 := by
  obtain ⟨n, hn⟩ : ∃ n : ℕ, (n : ℤ) = -ulpExp w :=
    ⟨(-ulpExp w).toNat, Int.toNat_of_nonneg (by omega)⟩
  refine ⟨n, ?_⟩
  rw [← zpow_natCast, ← zpow_add₀ (by norm_num : (2 : ℝ) ≠ 0),
    show (n : ℤ) + ulpExp w = 0 by omega, zpow_zero]

/-- `rn` is nonnegative on the nonnegative reals. -/
theorem rn_nonneg {w : ℝ} (h0 : 0 ≤ w) : 0 ≤ rn w := by
  by_cases hw : w = 0
  · simp [hw, rn_zero]
  · unfold rn
    rw [if_neg hw]
    have hq : 0 ≤ w / (2 : ℝ) ^ ulpExp w := div_nonneg h0 (by positivity)
    have hR : (0 : ℝ) ≤ (roundEven (w / (2 : ℝ) ^ ulpExp w) : ℝ) := by
      exact_mod_cast roundEven_nonneg hq
    exact mul_nonneg hR (by positivity)

/-- `rn` maps `[0, 1]` into `[0, 1]`. -/
theorem rn_le_one_of_le_one {w : ℝ} (h0 : 0 ≤ w) (h1 : w ≤ 1) : rn w ≤ 1 := by
  rcases eq_or_lt_of_le h0 with hw | hw
  · rw [← hw, rn_zero]
    norm_num
  · have he : ulpExp w ≤ 0 := by
      have := ulpExp_le_of_le_two hw (by linarith)
      linarith
    obtain ⟨n, key⟩ := exists_pow_mul_ulp he
    have hpos : (0 : ℝ) < (2 : ℝ) ^ ulpExp w := by positivity
    have hq : w / (2 : ℝ) ^ ulpExp w ≤ ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) := by
      push_cast
      rw [div_le_iff₀ hpos]
      linarith
    have hR : roundEven (w / (2 : ℝ) ^ ulpExp w) ≤ (((2 : ℕ) ^ n : ℕ) : ℤ) :=
      roundEven_le_of_le hq
    have hR' : (roundEven (w / (2 : ℝ) ^ ulpExp w) : ℝ) ≤ ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) :=
      Int.cast_le.mpr hR
    unfold rn
    rw [if_neg hw.ne']
    calc (roundEven (w / (2 : ℝ) ^ ulpExp w) : ℝ) * (2 : ℝ) ^ ulpExp w
        ≤ ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) * (2 : ℝ) ^ ulpExp w :=
          mul_le_mul_of_nonneg_right hR' hpos.le
      _ = 1 := by
          push_cast
          exact key

/-- `rn` maps `[1, 2]` into `[1, ∞)`: a value at least one stays at least one. -/
theorem one_le_rn_of_one_le {w : ℝ} (h1 : 1 ≤ w) (h2 : w ≤ 2) : 1 ≤ rn w := by
  have hw : 0 < w := by linarith
  have he : ulpExp w ≤ 0 := by
    have := ulpExp_le_of_le_two hw h2
    linarith
  obtain ⟨n, key⟩ := exists_pow_mul_ulp he
  have hpos : (0 : ℝ) < (2 : ℝ) ^ ulpExp w := by positivity
  have hq : ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) ≤ w / (2 : ℝ) ^ ulpExp w := by
    push_cast
    rw [le_div_iff₀ hpos]
    linarith
  have hR : (((2 : ℕ) ^ n : ℕ) : ℤ) ≤ roundEven (w / (2 : ℝ) ^ ulpExp w) :=
    roundEven_ge_of_le hq
  have hR' : ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) ≤ (roundEven (w / (2 : ℝ) ^ ulpExp w) : ℝ) :=
    Int.cast_le.mpr hR
  unfold rn
  rw [if_neg hw.ne']
  calc (1 : ℝ) = ((((2 : ℕ) ^ n : ℕ) : ℤ) : ℝ) * (2 : ℝ) ^ ulpExp w := by
        push_cast
        exact key.symm
    _ ≤ (roundEven (w / (2 : ℝ) ^ ulpExp w) : ℝ) * (2 : ℝ) ^ ulpExp w :=
        mul_le_mul_of_nonneg_right hR' hpos.le

/-! ## The binary32 model with overflow -/

/-- A binary32 result: a finite value (the rounding of a real), `+∞`, or NaN. -/
inductive Bin32 where
  | ofReal (r : ℝ)
  | pinf
  | nan

/-- Rounds a nonnegative real to binary32, with overflow. At or above `ovfThreshold` the result is
`+∞` (IEEE 754-2019 §4.3.1: the midpoint above the largest finite value rounds to ∞ with ties to
even). Below it the result is `rn r`. -/
noncomputable def fround (r : ℝ) : Bin32 :=
  if ovfThreshold ≤ r then Bin32.pinf else Bin32.ofReal (rn r)

/-- `exp` in binary32: the rounding of the exact exponential, with overflow. -/
noncomputable def fexp (x : ℝ) : Bin32 := fround (Real.exp x)

/-- `1 + y` in binary32, rounded once. -/
noncomputable def fadd1 : Bin32 → Bin32
  | .ofReal r => fround (1 + r)
  | .pinf => .pinf
  | .nan => .nan

/-- `a / b` in binary32 for nonnegative operands. A finite quotient is rounded once. A zero
denominator gives `+∞` (NaN for `0 / 0`), `x / ∞ = 0`, and `∞ / ∞` is NaN. -/
noncomputable def fdiv : Bin32 → Bin32 → Bin32
  | .ofReal a, .ofReal b => if b = 0 then (if a = 0 then .nan else .pinf) else fround (a / b)
  | .ofReal _, .pinf => .ofReal 0
  | .ofReal _, .nan => .nan
  | .pinf, .ofReal _ => .pinf
  | .pinf, .pinf => .nan
  | .pinf, .nan => .nan
  | .nan, _ => .nan

/-- The f32 constant `LOG2E` of `exp_amd64.s`. The decimal is exactly a binary32 value, so it is the
constant the Rust code holds. -/
def LOG2E32 : ℝ := 1.44269502162933349609375

/-- The integer `k` of `exp_amd64.s`: `CVTSS2SL` of the binary32 product `rn(LOG2E32 · x)`, rounded
to an integer with ties to even, and `i32::MIN` when that integer is outside the `i32` range. -/
noncomputable def amdK (x : ℝ) : ℤ :=
  if -2147483648 ≤ roundEven (rn (LOG2E32 * x)) ∧ roundEven (rn (LOG2E32 * x)) < 2147483648 then
    roundEven (rn (LOG2E32 * x))
  else -2147483648

/-- `math32::exp` on x86-64, in the binary32 model. Its two decisions are exact: `e = k + 127`, so
`e ≥ 255` gives `+∞` (`k ≥ 128`) and `e ≤ 0` gives `0` (`k ≤ −127`). A finite result is the correctly
rounded `exp`, which stands in for the assembly's polynomial (a stated gap). -/
noncomputable def fexpAmd (x : ℝ) : Bin32 :=
  if 128 ≤ amdK x then Bin32.pinf
  else if amdK x ≤ -127 then Bin32.ofReal 0
  else fexp x

/-- The code before the fix: `exp(−diff) / (1 + exp(−diff))`, with `x = −diff`, and `exp` is
`math32::exp`. -/
noncomputable def oldB (x : ℝ) : Bin32 := fdiv (fexpAmd x) (fadd1 (fexpAmd x))

/-- The stable form, as `stable_sigmoid(x)` computes it, over the same model. Every `exp` is
applied at a non-positive argument. -/
noncomputable def newB (x : ℝ) : Bin32 :=
  if 0 ≤ x then fdiv (Bin32.ofReal 1) (fadd1 (fexpAmd (-x)))
  else fdiv (fexpAmd x) (fadd1 (fexpAmd x))

/-! ## Facts about the model -/

theorem four_le_ovfThreshold : (4 : ℝ) ≤ ovfThreshold := by
  unfold ovfThreshold
  norm_num [zpow_neg]

/-- `exp` of a non-positive argument is finite and at most one. -/
theorem fexp_ofReal_of_nonpos {y : ℝ} (hy : y ≤ 0) :
    fexp y = Bin32.ofReal (rn (Real.exp y)) := by
  have h1 : Real.exp y ≤ 1 := by
    have := Real.exp_le_exp.mpr hy
    rwa [Real.exp_zero] at this
  have h2 : Real.exp y < ovfThreshold := by
    linarith [four_le_ovfThreshold]
  unfold fexp fround
  rw [if_neg (not_le.mpr h2)]

/-! ## The amd64 `exp` of `math32::exp`: its two decisions -/

/-- `rn` is nonpositive on the nonpositive reals. -/
theorem rn_nonpos {w : ℝ} (h0 : w ≤ 0) : rn w ≤ 0 := by
  by_cases hw : w = 0
  · subst hw
    simp [rn_zero]
  · have hp : (0 : ℝ) < 2 ^ ulpExp w := by positivity
    have hd : w / 2 ^ ulpExp w ≤ 0 := by
      rw [div_le_iff₀ hp]
      simpa using h0
    have hr : roundEven (w / 2 ^ ulpExp w) ≤ 0 := roundEven_le_of_le (by simpa using hd)
    have hr' : ((roundEven (w / 2 ^ ulpExp w) : ℤ) : ℝ) ≤ 0 := by exact_mod_cast hr
    unfold rn
    rw [if_neg hw]
    simpa using mul_le_mul_of_nonneg_right hr' hp.le

/-- For a nonpositive argument, `k ≤ 0`, so `e = k + 127` never overflows. -/
theorem amdK_le_zero {y : ℝ} (hy : y ≤ 0) : amdK y ≤ 0 := by
  have hL : LOG2E32 * y ≤ 0 := by
    unfold LOG2E32
    linarith
  have hr : roundEven (rn (LOG2E32 * y)) ≤ 0 :=
    roundEven_le_of_le (by simpa using rn_nonpos hL)
  unfold amdK
  split_ifs with h
  · exact hr
  · norm_num

/-- `math32::exp` of a nonpositive argument is finite, in `[0, 1]`: the overflow branch is never
taken, and the underflow branch gives `0`. -/
theorem fexpAmd_ofReal_of_nonpos {y : ℝ} (hy : y ≤ 0) :
    ∃ r ∈ Set.Icc (0 : ℝ) 1, fexpAmd y = Bin32.ofReal r := by
  have hk : amdK y ≤ 0 := amdK_le_zero hy
  have e1 : 0 ≤ Real.exp y := (Real.exp_pos y).le
  have e2 : Real.exp y ≤ 1 := by
    have := Real.exp_le_exp.mpr hy
    rwa [Real.exp_zero] at this
  unfold fexpAmd
  by_cases h1 : 128 ≤ amdK y
  · exact absurd (le_trans h1 hk) (by norm_num)
  · by_cases h2 : amdK y ≤ -127
    · rw [if_neg h1, if_pos h2]
      exact ⟨0, ⟨le_refl (0 : ℝ), zero_le_one⟩, rfl⟩
    · rw [if_neg h1, if_neg h2]
      exact ⟨rn (Real.exp y), ⟨rn_nonneg e1, rn_le_one_of_le_one e1 e2⟩,
        fexp_ofReal_of_nonpos hy⟩

/-- For `89 ≤ x ≤ 2^20` the binary32 product `rn(LOG2E32 · x)` lies in `[128, 2^31 − 1]`. Its
error is at most `1/16`, from `rn_sub_le` and the unit `2^-3` (the value is below `2^21`). -/
theorem roundEven_rn_mem {x : ℝ} (hx : 89 ≤ x) (hx2 : x ≤ 1048576) :
    128 ≤ roundEven (rn (LOG2E32 * x)) ∧ roundEven (rn (LOG2E32 * x)) ≤ 2147483647 := by
  have hL : (1.44 : ℝ) ≤ LOG2E32 := by
    unfold LOG2E32
    norm_num
  have hL2 : LOG2E32 ≤ (1.45 : ℝ) := by
    unfold LOG2E32
    norm_num
  have hw1 : (128.1 : ℝ) ≤ LOG2E32 * x := by nlinarith
  have hw2 : LOG2E32 * x < (2097152 : ℝ) := by nlinarith
  have hpos : (0 : ℝ) < LOG2E32 * x := by linarith
  have hw0 : LOG2E32 * x ≠ 0 := hpos.ne'
  have hlog : Int.log 2 |LOG2E32 * x| ≤ 20 := by
    rw [abs_of_pos hpos]
    by_contra hc
    push_neg at hc
    have h1 : (2 : ℝ) ^ (21 : ℤ) ≤ (2 : ℝ) ^ Int.log 2 (LOG2E32 * x) :=
      zpow_le_zpow_right₀ (by norm_num) (by omega)
    have h2 : (2 : ℝ) ^ Int.log 2 (LOG2E32 * x) ≤ LOG2E32 * x := by
      exact_mod_cast Int.zpow_log_le_self (by norm_num : 1 < 2) hpos
    have h21 : (2 : ℝ) ^ (21 : ℤ) = 2097152 := by norm_num
    linarith
  have hu : ulpExp (LOG2E32 * x) ≤ -3 := by
    unfold ulpExp
    linarith [max_le hlog (by norm_num : (-126 : ℤ) ≤ 20)]
  have hpow : (2 : ℝ) ^ ulpExp (LOG2E32 * x) ≤ (2 : ℝ) ^ (-3 : ℤ) :=
    zpow_le_zpow_right₀ (by norm_num) hu
  have h8 : (2 : ℝ) ^ (-3 : ℤ) = 1 / 8 := by norm_num
  have herr : |rn (LOG2E32 * x) - LOG2E32 * x| ≤ 1 / 16 := by
    have := rn_sub_le hw0
    linarith
  obtain ⟨hlo, hhi⟩ := abs_le.mp herr
  constructor
  · exact roundEven_ge_of_le (by push_cast; linarith)
  · exact roundEven_le_of_le (by push_cast; linarith)

/-- For `89 ≤ x ≤ 2^20`, `math32::exp` takes the overflow branch: `k ≥ 128`, so `e ≥ 255`. -/
theorem amdK_overflow {x : ℝ} (hx : 89 ≤ x) (hx2 : x ≤ 1048576) : 128 ≤ amdK x := by
  obtain ⟨h1, h2⟩ := roundEven_rn_mem hx hx2
  have hc : -2147483648 ≤ roundEven (rn (LOG2E32 * x)) ∧
      roundEven (rn (LOG2E32 * x)) < 2147483648 := ⟨by omega, by omega⟩
  unfold amdK
  rw [if_pos hc]
  exact h1

/-- For `89 ≤ x ≤ 2^20`, `math32::exp` returns `+∞`. -/
theorem fexpAmd_pinf_of_mem {x : ℝ} (hx : 89 ≤ x) (hx2 : x ≤ 1048576) :
    fexpAmd x = Bin32.pinf := by
  unfold fexpAmd
  rw [if_pos (amdK_overflow hx hx2)]

/-- `1 + r` in binary32 for `0 ≤ r ≤ 1` is finite, and it is the rounding of `1 + r`. -/
theorem fadd1_ofReal {r : ℝ} (hr : r ≤ 1) :
    fadd1 (Bin32.ofReal r) = Bin32.ofReal (rn (1 + r)) := by
  have h : 1 + r < ovfThreshold := by
    linarith [four_le_ovfThreshold]
  simp only [fadd1]
  unfold fround
  rw [if_neg (not_le.mpr h)]

/-- A quotient below the overflow threshold, over a nonzero denominator, is the rounded quotient. -/
theorem fdiv_ofReal_of_ne {a b : ℝ} (hb : b ≠ 0) (hq : a / b < ovfThreshold) :
    fdiv (Bin32.ofReal a) (Bin32.ofReal b) = Bin32.ofReal (rn (a / b)) := by
  simp only [fdiv]
  rw [if_neg hb]
  unfold fround
  rw [if_neg (not_le.mpr hq)]

/-- The divide-and-add step of both formulas: a numerator and a denominator in `[0, 1]` give a
result in `[0, 1]`, with no overflow and no NaN. -/
theorem fdiv_fadd1_mem {a r : ℝ} (ha0 : 0 ≤ a) (ha1 : a ≤ 1) (hr0 : 0 ≤ r) (hr1 : r ≤ 1) :
    ∃ q ∈ Set.Icc (0 : ℝ) 1,
      fdiv (Bin32.ofReal a) (fadd1 (Bin32.ofReal r)) = Bin32.ofReal q := by
  have hs1 : 1 ≤ rn (1 + r) := one_le_rn_of_one_le (by linarith) (by linarith)
  have hs0 : rn (1 + r) ≠ 0 := (show (0 : ℝ) < rn (1 + r) by linarith).ne'
  rw [fadd1_ofReal hr1]
  have hq : a / rn (1 + r) ≤ 1 := by
    rw [div_le_one (show (0 : ℝ) < rn (1 + r) by linarith)]
    linarith
  have hq0 : 0 ≤ a / rn (1 + r) := div_nonneg ha0 (by linarith)
  have hlt : a / rn (1 + r) < ovfThreshold := by
    linarith [four_le_ovfThreshold]
  refine ⟨rn (a / rn (1 + r)), ⟨rn_nonneg hq0, rn_le_one_of_le_one hq0 hq⟩, ?_⟩
  exact fdiv_ofReal_of_ne hs0 hlt

/-! ## The bug (A8) and the fix, in the model -/

/-- **Bug.** For `89 ≤ x ≤ 2^20` the code before the fix gives NaN: `math32::exp` returns `+∞`
(`fexpAmd_pinf_of_mem`), and `∞ / (1 + ∞)` is `∞ / ∞`. -/
theorem oldB_nan_of_mem {x : ℝ} (hx : 89 ≤ x) (hx2 : x ≤ 1048576) : oldB x = Bin32.nan := by
  simp [oldB, fexpAmd_pinf_of_mem hx hx2, fadd1, fdiv]

/-- **Bug, at the margin of the regression test.** The margin `d = −89` gives `x = −d = 89`, and
the code before the fix gives NaN there, as the Rust test `bpr_update_stays_finite_at_margin_minus_89`
shows on the old formula. -/
theorem oldB_nan_at_margin_minus_89 : oldB (-(-89 : ℝ)) = Bin32.nan :=
  oldB_nan_of_mem (by norm_num) (by norm_num)

/-- **Bug, as a counterexample to the spec.** The spec needs every value to be finite and in
`[0, 1]`. The code before the fix is not: at `x = 89` it is NaN. -/
theorem oldB_not_spec : ¬ ∀ x : ℝ, ∃ r ∈ Set.Icc (0 : ℝ) 1, oldB x = Bin32.ofReal r := by
  intro h
  obtain ⟨r, _, hr⟩ := h 89
  rw [oldB_nan_of_mem (le_refl (89 : ℝ)) (by norm_num)] at hr
  cases hr

/-- **Fix.** For every real `x`, the stable form is finite and in `[0, 1]`, in the binary32 model. -/
theorem newB_mem (x : ℝ) : ∃ r ∈ Set.Icc (0 : ℝ) 1, newB x = Bin32.ofReal r := by
  by_cases hx : 0 ≤ x
  · have hnp : -x ≤ 0 := by linarith
    obtain ⟨r, hrI, he⟩ := fexpAmd_ofReal_of_nonpos hnp
    have hnb : newB x = fdiv (Bin32.ofReal 1) (fadd1 (Bin32.ofReal r)) := by
      unfold newB
      rw [if_pos hx, he]
    rw [hnb]
    exact fdiv_fadd1_mem (by norm_num) (by norm_num) hrI.1 hrI.2
  · have hxn : x ≤ 0 := le_of_lt (not_le.mp hx)
    obtain ⟨r, hrI, he⟩ := fexpAmd_ofReal_of_nonpos hxn
    have hnb : newB x = fdiv (Bin32.ofReal r) (fadd1 (Bin32.ofReal r)) := by
      unfold newB
      rw [if_neg hx, he]
    rw [hnb]
    exact fdiv_fadd1_mem hrI.1 hrI.2 hrI.1 hrI.2

end Gorse.Spec.Sigmoid
