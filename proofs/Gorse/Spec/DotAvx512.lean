import Gorse.Spec.Dot

/-! # The AVX-512 binary32 dot product: definitions (P4-5, T3)

`dotAvx512` models `dot` in common/floats/src/floats_avx512.c:306-366. The vector loop keeps
16 lane accumulators. Lane `j` holds the elements with index `≡ j (mod 16)`. The first epoch
starts from zero, so its first step is the same as `rn (x*y + 0) = rn (x*y)`. Each later
element is one fused multiply-add. The 16 lanes are reduced by the tree 16 → 8 → 4 → 2 → 1. A
block of 8 is then added if the remainder is at least 8. The scalar tail is fused by
`-ffp-contract=on` (crates/gorse-kernels/build.rs). -/

namespace Gorse.Spec.DotAvx512

open Gorse.Spec.Float32
open Gorse.Spec.Dot

/-- One vector step: lane `j` of the 16-lane accumulator takes `rn (x*y + s_j)`. -/
noncomputable def laneStep (acc : List ℝ) (blk : List (ℝ × ℝ)) : List ℝ :=
  (acc.zip blk).map fun q => rn (q.2.1 * q.2.2 + q.1)

/-- Run `k` vector epochs. Each epoch reads the next 16 elements. -/
noncomputable def accum : ℕ → List ℝ → List (ℝ × ℝ) → List ℝ
  | 0, acc, _ => acc
  | k + 1, acc, v => accum k (laneStep acc (v.take 16)) (v.drop 16)

/-- The horizontal reduction of the 16 lanes, in the order of floats_avx512.c:322-336. -/
noncomputable def reduce16 : List ℝ → ℝ
  | [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15] =>
      let t0 := rn (a0 + a8)
      let t1 := rn (a1 + a9)
      let t2 := rn (a2 + a10)
      let t3 := rn (a3 + a11)
      let t4 := rn (a4 + a12)
      let t5 := rn (a5 + a13)
      let t6 := rn (a6 + a14)
      let t7 := rn (a7 + a15)
      let w0 := rn (t0 + t4)
      let w1 := rn (t1 + t5)
      let w2 := rn (t2 + t6)
      let w3 := rn (t3 + t7)
      rn (rn (w0 + w2) + rn (w1 + w3))
  | _ => 0

/-- The 8-lane block of floats_avx512.c:343-357: products, then the tree 8 → 4 → 2 → 1. -/
noncomputable def block8 (p : List (ℝ × ℝ)) : ℝ :=
  match p.map fun q => rn (q.1 * q.2) with
  | [a0, a1, a2, a3, a4, a5, a6, a7] =>
      let y0 := rn (a0 + a4)
      let y1 := rn (a1 + a5)
      let y2 := rn (a2 + a6)
      let y3 := rn (a3 + a7)
      rn (rn (y0 + y2) + rn (y1 + y3))
  | _ => 0

/-- The scalar tail: each element is fused into the running sum, as `sum += a[i]*b[i]`. -/
noncomputable def tail : List (ℝ × ℝ) → ℝ → ℝ
  | [], s => s
  | p :: v, s => tail v (rn (p.1 * p.2 + s))

/-- The full AVX-512 dot product of a list of pairs. -/
noncomputable def dotAvx512 (v : List (ℝ × ℝ)) : ℝ :=
  let E := v.length / 16
  let r := v.length % 16
  let sum0 := reduce16 (accum E (List.replicate 16 0) v)
  let rest := v.drop (16 * E)
  if 8 ≤ r then
    tail (rest.drop 8) (rn (sum0 + block8 (rest.take 8)))
  else
    tail rest sum0

/-! ## Error certificates

A certificate for a computed value `c` with exact value `e`, abs-sum `A`, depth `d` and `N`
products says `|c - e| ≤ γ d * A + N * (1+u)^d * η`. The factor `(1+u)^d` carries each
subnormal error `η` through the rounding steps above it. The grid sums (`rn_add_grid_err`) add no
new `η`, so `η` enters only once per product. -/

/-- A grid value: an integer multiple of the subnormal unit `2^-149`. -/
def Grid (x : ℝ) : Prop := ∃ k : ℤ, x = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ)

/-- The error certificate, see the section comment. -/
def Cert (c e A : ℝ) (d N : ℕ) : Prop :=
  |c - e| ≤ γ d * A + (N : ℝ) * (1 + unitRoundoff) ^ d * subnormalError ∧ |e| ≤ A ∧ 0 ≤ A

theorem u_nonneg : (0 : ℝ) ≤ unitRoundoff := by unfold unitRoundoff; positivity

theorem eta_nonneg : (0 : ℝ) ≤ subnormalError := by unfold subnormalError; positivity

theorem one_le_pow_unit (d : ℕ) : (1 : ℝ) ≤ (1 + unitRoundoff) ^ d := by
  have h : (1 : ℝ) ≤ 1 + unitRoundoff := by linarith [u_nonneg]
  first
    | exact one_le_pow₀ h
    | exact one_le_pow_of_one_le h d

theorem pow_unit_mono {d e : ℕ} (hde : d ≤ e) :
    (1 + unitRoundoff) ^ d ≤ (1 + unitRoundoff) ^ e := by
  have h : (1 : ℝ) ≤ 1 + unitRoundoff := by linarith [u_nonneg]
  first
    | exact pow_le_pow_right₀ h hde
    | exact pow_le_pow_right h hde

theorem gamma_nonneg {d : ℕ} (hd : 2 * ((d : ℝ) + 1) * unitRoundoff ≤ 1) : 0 ≤ γ d := by
  have hdu : (d : ℝ) * unitRoundoff ≤ 1 := by
    nlinarith [u_nonneg, (Nat.cast_nonneg d : (0 : ℝ) ≤ d)]
  unfold γ
  apply div_nonneg
  · exact mul_nonneg (Nat.cast_nonneg d) u_nonneg
  · linarith

theorem gamma_mono {d e : ℕ} (hde : d ≤ e) (he : 2 * ((e : ℝ) + 1) * unitRoundoff ≤ 1) :
    γ d ≤ γ e := by
  have hdu : (d : ℝ) * unitRoundoff ≤ (e : ℝ) * unitRoundoff := by
    apply mul_le_mul_of_nonneg_right _ u_nonneg
    exact_mod_cast hde
  have heu : 2 * ((e : ℝ) * unitRoundoff) ≤ 1 := by nlinarith [u_nonneg]
  have hD1 : (0 : ℝ) < 1 - (d : ℝ) * unitRoundoff := by linarith
  have hE1 : (0 : ℝ) < 1 - (e : ℝ) * unitRoundoff := by linarith
  unfold γ
  first
    | rw [div_le_div_iff hD1 hE1]
    | rw [div_le_div_iff_of_pos hD1 hE1]
  nlinarith [hdu]

theorem cert_zero : Cert 0 0 0 0 0 := by
  simp [Cert, γ]

theorem cert_mul (x y : ℝ) : Cert (rn (x * y)) (x * y) |x * y| 1 1 := by
  unfold Cert
  have h1 := rn_err_all (x * y)
  have h2 := mul_le_mul_of_nonneg_right unit_le_gamma_one (abs_nonneg (x * y))
  have h3 : subnormalError ≤ (1 + unitRoundoff) ^ 1 * subnormalError := by
    rw [pow_one]
    nlinarith [u_nonneg, eta_nonneg]
  refine ⟨?_, le_refl _, abs_nonneg _⟩
  simp only [Nat.cast_one, one_mul]
  linarith

/-- One fused step `rn (x*y + c)`: the product is one new `η`. -/
theorem cert_fma {x y c e A : ℝ} {d N : ℕ} (hd : 2 * ((d : ℝ) + 1) * unitRoundoff ≤ 1)
    (hc : Cert c e A d N) :
    Cert (rn (x * y + c)) (e + x * y) (A + |x * y|) (d + 1) (N + 1) := by
  obtain ⟨hce, hea, hA⟩ := hc
  unfold Cert
  push_cast
  have hu := u_nonneg
  have hη := eta_nonneg
  have hg := gamma_nonneg hd
  have hsucc := gamma_succ_ge hd
  have hu_le : unitRoundoff ≤ γ (d + 1) := by
    nlinarith [mul_nonneg (by linarith : (0 : ℝ) ≤ 1 + unitRoundoff) hg]
  have hc' : |c| ≤ A + (γ d * A + (N : ℝ) * (1 + unitRoundoff) ^ d * subnormalError) := by
    have h := dot_abs_add_le (c - e) e
    rw [sub_add_cancel] at h
    linarith
  have hxy := dot_abs_add_le (x * y) c
  have hxy' : |x * y + c|
      ≤ |x * y| + A + (γ d * A + (N : ℝ) * (1 + unitRoundoff) ^ d * subnormalError) := by
    linarith
  have hsplit : |rn (x * y + c) - (e + x * y)|
      ≤ |rn (x * y + c) - (x * y + c)| + |c - e| := by
    have heq : rn (x * y + c) - (e + x * y) = (rn (x * y + c) - (x * y + c)) + (c - e) := by
      ring
    rw [heq]
    exact dot_abs_add_le _ _
  have h1 := rn_err_all (x * y + c)
  have h2 := mul_le_mul_of_nonneg_left hxy' hu
  have F2 : (1 + unitRoundoff) * (γ d * A + (N : ℝ) * (1 + unitRoundoff) ^ d * subnormalError)
      = (1 + unitRoundoff) * γ d * A + (N : ℝ) * (1 + unitRoundoff) ^ (d + 1) * subnormalError := by
    rw [pow_succ]
    ring
  have F3 : ((1 + unitRoundoff) * γ d + unitRoundoff) * A ≤ γ (d + 1) * A :=
    mul_le_mul_of_nonneg_right hsucc hA
  have F4 : unitRoundoff * |x * y| ≤ γ (d + 1) * |x * y| :=
    mul_le_mul_of_nonneg_right hu_le (abs_nonneg _)
  have F5 : subnormalError ≤ (1 + unitRoundoff) ^ (d + 1) * subnormalError := by
    nlinarith [one_le_pow_unit (d + 1), eta_nonneg]
  linarith

/-- Adding two grid values: the rounding adds `u` times the sum, and no new `η`. The depths are
bounded by a common `d`, and the output depth is `d + 1`. -/
theorem cert_add {c₁ c₂ e₁ e₂ A₁ A₂ : ℝ} {d₁ d₂ d N₁ N₂ : ℕ} (hg₁ : Grid c₁) (hg₂ : Grid c₂)
    (hd : 2 * ((d : ℝ) + 1) * unitRoundoff ≤ 1) (hd₁ : d₁ ≤ d) (hd₂ : d₂ ≤ d)
    (h₁ : Cert c₁ e₁ A₁ d₁ N₁) (h₂ : Cert c₂ e₂ A₂ d₂ N₂) :
    Cert (rn (c₁ + c₂)) (e₁ + e₂) (A₁ + A₂) (d + 1) (N₁ + N₂) := by
  obtain ⟨hce₁, hea₁, hA₁⟩ := h₁
  obtain ⟨hce₂, hea₂, hA₂⟩ := h₂
  unfold Cert
  push_cast
  have hu := u_nonneg
  have hη := eta_nonneg
  have hg := gamma_nonneg hd
  have hsucc := gamma_succ_ge hd
  have hγ₁ : γ d₁ ≤ γ d := gamma_mono hd₁ hd
  have hγ₂ : γ d₂ ≤ γ d := gamma_mono hd₂ hd
  have hp₁ : (1 + unitRoundoff) ^ d₁ ≤ (1 + unitRoundoff) ^ d := pow_unit_mono hd₁
  have hp₂ : (1 + unitRoundoff) ^ d₂ ≤ (1 + unitRoundoff) ^ d := pow_unit_mono hd₂
  have hb₁ : γ d₁ * A₁ + (N₁ : ℝ) * (1 + unitRoundoff) ^ d₁ * subnormalError
      ≤ γ d * A₁ + (N₁ : ℝ) * (1 + unitRoundoff) ^ d * subnormalError := by
    have a1 := mul_le_mul_of_nonneg_right hγ₁ hA₁
    have a2 := mul_le_mul_of_nonneg_left hp₁ (mul_nonneg (Nat.cast_nonneg N₁) hη)
    linarith
  have hb₂ : γ d₂ * A₂ + (N₂ : ℝ) * (1 + unitRoundoff) ^ d₂ * subnormalError
      ≤ γ d * A₂ + (N₂ : ℝ) * (1 + unitRoundoff) ^ d * subnormalError := by
    have a1 := mul_le_mul_of_nonneg_right hγ₂ hA₂
    have a2 := mul_le_mul_of_nonneg_left hp₂ (mul_nonneg (Nat.cast_nonneg N₂) hη)
    linarith
  -- the input errors, summed
  have hEQ : |c₁ - e₁| + |c₂ - e₂|
      ≤ γ d * (A₁ + A₂) + ((N₁ : ℝ) + (N₂ : ℝ)) * (1 + unitRoundoff) ^ d * subnormalError := by
    linarith
  have hsum : |c₁ + c₂ - (e₁ + e₂)| ≤ |c₁ - e₁| + |c₂ - e₂| := by
    have heq : c₁ + c₂ - (e₁ + e₂) = (c₁ - e₁) + (c₂ - e₂) := by ring
    rw [heq]
    exact dot_abs_add_le _ _
  have hcabs : |c₁ + c₂| ≤ (A₁ + A₂) + (|c₁ - e₁| + |c₂ - e₂|) := by
    have h := dot_abs_add_le (c₁ + c₂ - (e₁ + e₂)) (e₁ + e₂)
    rw [sub_add_cancel] at h
    have h' := dot_abs_add_le e₁ e₂
    linarith
  have hG := rn_add_grid_err hg₁ hg₂
  have hsplit : |rn (c₁ + c₂) - (e₁ + e₂)|
      ≤ |rn (c₁ + c₂) - (c₁ + c₂)| + |c₁ + c₂ - (e₁ + e₂)| := by
    have heq : rn (c₁ + c₂) - (e₁ + e₂) = (rn (c₁ + c₂) - (c₁ + c₂)) + (c₁ + c₂ - (e₁ + e₂)) := by
      ring
    rw [heq]
    exact dot_abs_add_le _ _
  have hu_mul := mul_le_mul_of_nonneg_left hcabs hu
  have hmul2 := mul_le_mul_of_nonneg_left hEQ (by linarith : (0 : ℝ) ≤ 1 + unitRoundoff)
  have F2 : (1 + unitRoundoff) * (γ d * (A₁ + A₂) + ((N₁ : ℝ) + (N₂ : ℝ)) * (1 + unitRoundoff) ^ d
        * subnormalError)
      = (1 + unitRoundoff) * γ d * (A₁ + A₂)
        + ((N₁ : ℝ) + (N₂ : ℝ)) * (1 + unitRoundoff) ^ (d + 1) * subnormalError := by
    rw [pow_succ]
    ring
  have F3 : ((1 + unitRoundoff) * γ d + unitRoundoff) * (A₁ + A₂) ≤ γ (d + 1) * (A₁ + A₂) :=
    mul_le_mul_of_nonneg_right hsucc (by linarith)
  linarith

end Gorse.Spec.DotAvx512
