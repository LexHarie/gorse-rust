import Gorse.Spec.Float32

/-! # The computable twin of the binary32 rounding model (P4-3, T3 of the verification plan)

`rnQ` computes `rn` on the rationals. Its definitions use `Int.log`, `Int.floor` and `zpow`
on `ℚ`, so the G9 oracle can run `rnQ` on exact rational inputs once that oracle is built
(P4-7, D-340).
`rn_ratCast` proves that `rnQ` agrees with `rn` on every rational, read as a real
(plan §2.2 item 3). The mirror that the oracle compiles without Mathlib is not part of this
file (D-340). -/

namespace Gorse.Spec.Float32

/-- `roundEven` on the rationals: the same rule as `roundEven` on the reals. -/
def roundEvenQ (r : ℚ) : ℤ :=
  if r - (⌊r⌋ : ℚ) < 1 / 2 then ⌊r⌋
  else if 1 / 2 < r - (⌊r⌋ : ℚ) then ⌊r⌋ + 1
  else if Even ⌊r⌋ then ⌊r⌋ else ⌊r⌋ + 1

/-- `ulpExp` on the rationals, with the same formula as `ulpExp` on the reals. -/
def ulpExpQ (q : ℚ) : ℤ := max (Int.log 2 |q|) (-126) - 23

/-- `rn` on the rationals: the computable twin of `rn`. -/
def rnQ (q : ℚ) : ℚ :=
  if q = 0 then 0 else (roundEvenQ (q / 2 ^ ulpExpQ q) : ℚ) * 2 ^ ulpExpQ q

/-- Casting a rational power of a rational to `ℝ` gives the real power of the cast base. The
exponent is an integer, so the proof splits it into a natural number or its negative. -/
theorem rat_cast_zpow (x : ℚ) (n : ℤ) : ((x ^ n : ℚ) : ℝ) = (x : ℝ) ^ n := by
  rcases Int.eq_nat_or_neg n with ⟨m, rfl | rfl⟩
  · simp [zpow_natCast]
  · simp [zpow_neg]

/-- The logarithm of a nonzero rational is the logarithm of the same rational read as a real.
The two `Int.log` values are equal, because each is determined by the same two bounds. -/
theorem int_log_ratCast {q : ℚ} (hq : q ≠ 0) :
    Int.log 2 |(q : ℝ)| = Int.log 2 |q| := by
  have hA : ((2 : ℕ) : ℚ) ^ Int.log 2 |q| ≤ |q| :=
    Int.zpow_log_le_self (by norm_num) (abs_pos.mpr hq)
  have hB : |q| < ((2 : ℕ) : ℚ) ^ (Int.log 2 |q| + 1) :=
    Int.lt_zpow_succ_log_self (by norm_num) |q|
  have hpos : (0 : ℝ) < |(q : ℝ)| := abs_pos.mpr (by exact_mod_cast hq)
  have hA' : ((2 : ℕ) : ℝ) ^ Int.log 2 |q| ≤ |(q : ℝ)| := by
    have h : ((((2 : ℕ) : ℚ) ^ Int.log 2 |q| : ℚ) : ℝ) ≤ ((|q| : ℚ) : ℝ) := Rat.cast_le.mpr hA
    rw [rat_cast_zpow] at h
    exact_mod_cast h
  have hB' : |(q : ℝ)| < ((2 : ℕ) : ℝ) ^ (Int.log 2 |q| + 1) := by
    have h : ((|q| : ℚ) : ℝ) < ((((2 : ℕ) : ℚ) ^ (Int.log 2 |q| + 1) : ℚ) : ℝ) :=
      Rat.cast_lt.mpr hB
    rw [rat_cast_zpow] at h
    exact_mod_cast h
  have h1 : Int.log 2 |q| ≤ Int.log 2 |(q : ℝ)| :=
    (Int.zpow_le_iff_le_log (by norm_num) hpos).mp hA'
  have h2 : Int.log 2 |(q : ℝ)| < Int.log 2 |q| + 1 :=
    (Int.lt_zpow_iff_log_lt (by norm_num) hpos).mp hB'
  omega

/-- `roundEvenQ` agrees with `roundEven` on the rationals, read as reals. The floor of a
rational is the floor of the same rational as a real, and each comparison of the rounding
rule holds over `ℚ` exactly when it holds over `ℝ`. -/
theorem roundEven_ratCast (r : ℚ) : roundEven (r : ℝ) = roundEvenQ r := by
  have hf : ⌊(r : ℝ)⌋ = ⌊r⌋ := Rat.floor_cast r
  have e : (r : ℝ) - ((⌊r⌋ : ℤ) : ℝ) = ((r - (⌊r⌋ : ℚ) : ℚ) : ℝ) := by norm_num
  have h12 : (1 / 2 : ℝ) = ((1 / 2 : ℚ) : ℝ) := by norm_num
  have c1 : (r : ℝ) - ((⌊r⌋ : ℤ) : ℝ) < 1 / 2 ↔ r - (⌊r⌋ : ℚ) < 1 / 2 := by
    rw [e, h12, Rat.cast_lt]
  have c2 : 1 / 2 < (r : ℝ) - ((⌊r⌋ : ℤ) : ℝ) ↔ 1 / 2 < r - (⌊r⌋ : ℚ) := by
    rw [e, h12, Rat.cast_lt]
  unfold roundEven roundEvenQ
  rw [hf]
  simp only [c1, c2]

/-- `rnQ` computes `rn` on the rationals: `rn (q : ℝ) = (rnQ q : ℝ)`. -/
theorem rn_ratCast (q : ℚ) : rn (q : ℝ) = ((rnQ q : ℚ) : ℝ) := by
  by_cases hq : q = 0
  · subst hq
    simp [rn, rnQ]
  · have hq' : (q : ℝ) ≠ 0 := by exact_mod_cast hq
    have hu : ulpExp (q : ℝ) = ulpExpQ q := by
      unfold ulpExp ulpExpQ
      rw [int_log_ratCast hq]
    have hdiv : (q : ℝ) / 2 ^ ulpExpQ q = ((q / 2 ^ ulpExpQ q : ℚ) : ℝ) := by
      rw [Rat.cast_div, rat_cast_zpow]
      norm_num
    unfold rn rnQ
    rw [if_neg hq', if_neg hq, hu, hdiv, roundEven_ratCast, Rat.cast_mul, rat_cast_zpow]
    try push_cast
    try ring

end Gorse.Spec.Float32
