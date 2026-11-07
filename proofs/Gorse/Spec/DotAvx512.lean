import Gorse.Spec.Dot

/-! # The AVX-512 binary32 dot product: model and error bound (P4-5, T3)

`dotCore` models `dot` in common/floats/src/floats_avx512.c:306-366. Lane `j` of the 16-lane
accumulator takes the elements `16 k + j` for `k < E`, where `E = n / 16`. The first epoch is
`rn (x*y + 0) = rn (x*y)`, so every epoch is one fused step `rn (x*y + s)`. The 16 lanes are
reduced by the tree 16 → 8 → 4 → 2 → 1 (floats_avx512.c:327-338). If `r = n % 16 ≥ 8`, an 8-block
of products is reduced by the tree 8 → 4 → 2 → 1 and added. The rest is a scalar tail of fused
steps (`-ffp-contract=on`, crates/gorse-kernels/build.rs).

The error bound uses certificates `Cert c e A d N`: `|c - e| ≤ γ d * A + N (1+u)^d η`, where
`A` is the sum of absolute products, `d` is the rounding depth, and `N` counts products. A grid
sum (`rn_add_grid_err`) adds no `η`, so `η` enters once per product (D-344). -/

namespace Gorse.Spec.DotAvx512

open Gorse.Spec.Float32
open Gorse.Spec.Dot

/-! ## Definitions -/

/-- Element `i` of the input, with `(0, 0)` past the end. -/
noncomputable def elem (v : List (ℝ × ℝ)) (i : ℕ) : ℝ × ℝ := v.getD i (0, 0)

/-- The exact product of element `i`. -/
noncomputable def fe (v : List (ℝ × ℝ)) (i : ℕ) : ℝ := (elem v i).1 * (elem v i).2

/-- Lane `j` after `k` vector epochs: `s ← rn (x*y + s)` with the element `16 i + j`
(floats_avx512.c:311-326). -/
noncomputable def lane (v : List (ℝ × ℝ)) (j : ℕ) : ℕ → ℝ
  | 0 => 0
  | k + 1 => rn (fe v (16 * k + j) + lane v j k)

/-- Lanes `j` and `j + 8` are added (floats_avx512.c:327-329). -/
noncomputable def red8 (s : ℕ → ℝ) (j : ℕ) : ℝ := rn (s j + s (j + 8))

/-- Then `j` and `j + 4` (floats_avx512.c:330-332). -/
noncomputable def red4 (s : ℕ → ℝ) (j : ℕ) : ℝ := rn (red8 s j + red8 s (j + 4))

/-- The horizontal reduction of 16 lanes: `(0,2)` and `(1,3)`, then their sum
(floats_avx512.c:333-338). -/
noncomputable def reduce16 (s : ℕ → ℝ) : ℝ :=
  rn (rn (red4 s 0 + red4 s 2) + rn (red4 s 1 + red4 s 3))

/-- The rounded product of element `i`. -/
noncomputable def prodR (v : List (ℝ × ℝ)) (i : ℕ) : ℝ := rn (fe v i)

/-- Products `b + j` and `b + j + 4` are added (floats_avx512.c:346-351). -/
noncomputable def bl4 (v : List (ℝ × ℝ)) (b j : ℕ) : ℝ :=
  rn (prodR v (b + j) + prodR v (b + (j + 4)))

/-- Then `j` and `j + 2` (floats_avx512.c:352-354). -/
noncomputable def bl2 (v : List (ℝ × ℝ)) (b j : ℕ) : ℝ := rn (bl4 v b j + bl4 v b (j + 2))

/-- The 8-block: `bl2 0 + bl2 1` (floats_avx512.c:355-358). -/
noncomputable def block8 (v : List (ℝ × ℝ)) (b : ℕ) : ℝ := rn (bl2 v b 0 + bl2 v b 1)

/-- The scalar tail: `m` fused steps from element `i` (floats_avx512.c:362-365). -/
noncomputable def tailAux (v : List (ℝ × ℝ)) : ℕ → ℕ → ℝ → ℝ
  | _, 0, s => s
  | i, m + 1, s => tailAux v (i + 1) m (rn (fe v i + s))

/-- The dot product for `n = 16 E + r` elements. -/
noncomputable def dotCore (v : List (ℝ × ℝ)) (E r : ℕ) : ℝ :=
  if 8 ≤ r then
    tailAux v (16 * E + 8) (r - 8) (rn (reduce16 (fun j => lane v j E) + block8 v (16 * E)))
  else
    tailAux v (16 * E) r (reduce16 (fun j => lane v j E))

/-- The AVX-512 dot product of a list of pairs. -/
noncomputable def dotAvx512 (v : List (ℝ × ℝ)) : ℝ :=
  dotCore v (v.length / 16) (v.length % 16)

/-- The rounding depth of `dotCore`. For `E ≥ 1` the reduction has depth `E + 4`, and for
`E = 0` the lanes are exact zeros. -/
noncomputable def depthCore (E r : ℕ) : ℕ :=
  if 8 ≤ r then (if E = 0 then 4 else E + 4) + 1 + (r - 8)
  else (if E = 0 then 0 else E + 4) + r

/-- The depth of `dotAvx512` on `n` elements. -/
noncomputable def depthAvx512 (n : ℕ) : ℕ := depthCore (n / 16) (n % 16)

/-! ## Error certificates -/

/-- A grid value: an integer multiple of `2^-149`. -/
abbrev Grid (x : ℝ) : Prop := ∃ k : ℤ, x = (k : ℝ) * (2 : ℝ) ^ (-149 : ℤ)

/-- The error certificate `Cert c e A d N`: `c` is within `γ d * A + N (1+u)^d η` of `e`,
with `|e| ≤ A`, `0 ≤ A`. -/
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

/-- `(1+u)^d ≤ 1 + 2du` when `du ≤ 1/2`. -/
theorem pow_unit_le_lin {D : ℕ} (h : (D : ℝ) * unitRoundoff ≤ 1 / 2) :
    (1 + unitRoundoff) ^ D ≤ 1 + 2 * (D : ℝ) * unitRoundoff := by
  induction D with
  | zero => simp
  | succ D ih =>
    have h' : (D : ℝ) * unitRoundoff ≤ 1 / 2 := by
      push_cast at h
      nlinarith [u_nonneg]
    have ih' := ih h'
    have hm := mul_le_mul_of_nonneg_right ih' (show (0 : ℝ) ≤ 1 + unitRoundoff by
      linarith [u_nonneg])
    have hq := mul_nonneg u_nonneg (show (0 : ℝ) ≤ 1 - 2 * (D : ℝ) * unitRoundoff by
      linarith)
    rw [pow_succ]
    push_cast
    nlinarith [hm, hq]

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
    refine mul_le_mul_of_nonneg_right ?_ u_nonneg
    exact_mod_cast hde
  have heu : 2 * ((e : ℝ) * unitRoundoff) ≤ 1 := by nlinarith [u_nonneg]
  have hD1 : (0 : ℝ) < 1 - (d : ℝ) * unitRoundoff := by linarith
  have hE1 : (0 : ℝ) < 1 - (e : ℝ) * unitRoundoff := by linarith
  unfold γ
  first
    | rw [div_le_div_iff hD1 hE1]
    | rw [div_le_div_iff_of_pos hD1 hE1]
  nlinarith [hdu]

theorem cert_eq {c e A : ℝ} {d N d' N' : ℕ} {e' A' : ℝ} (h : Cert c e A d N) (he : e = e')
    (hA : A = A') (hd : d = d') (hN : N = N') : Cert c e' A' d' N' := by
  subst he hA hd hN
  exact h

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

/-- One fused step `rn (x*y + c)`: the product adds one `η`. -/
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
  refine ⟨?_, ?_, add_nonneg hA (abs_nonneg _)⟩
  · linarith
  · exact le_trans (dot_abs_add_le e (x * y)) (by linarith)

/-- Adding two grid values: the rounding adds `u` times the sum and no `η`. The inputs have
depths at most `d`, and the output has depth `d + 1`. -/
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
  have F2 : (1 + unitRoundoff) * (γ d * (A₁ + A₂)
        + ((N₁ : ℝ) + (N₂ : ℝ)) * (1 + unitRoundoff) ^ d * subnormalError)
      = (1 + unitRoundoff) * γ d * (A₁ + A₂)
        + ((N₁ : ℝ) + (N₂ : ℝ)) * (1 + unitRoundoff) ^ (d + 1) * subnormalError := by
    rw [pow_succ]
    ring
  have F3 : ((1 + unitRoundoff) * γ d + unitRoundoff) * (A₁ + A₂) ≤ γ (d + 1) * (A₁ + A₂) :=
    mul_le_mul_of_nonneg_right hsucc (by linarith)
  refine ⟨?_, ?_, add_nonneg hA₁ hA₂⟩
  · linarith
  · exact le_trans (dot_abs_add_le e₁ e₂) (by linarith)

theorem grid_rn (x : ℝ) : Grid (rn x) := rn_multiple x

theorem grid_lane (v : List (ℝ × ℝ)) (j k : ℕ) : Grid (lane v j k) := by
  cases k with
  | zero => exact ⟨0, by simp [lane]⟩
  | succ k =>
    simp only [lane]
    exact grid_rn _

theorem grid_red8 (s : ℕ → ℝ) (j : ℕ) : Grid (red8 s j) := grid_rn _

theorem grid_red4 (s : ℕ → ℝ) (j : ℕ) : Grid (red4 s j) := grid_rn _

theorem grid_reduce16 (s : ℕ → ℝ) : Grid (reduce16 s) := grid_rn _

theorem grid_prodR (v : List (ℝ × ℝ)) (i : ℕ) : Grid (prodR v i) := grid_rn _

theorem grid_bl4 (v : List (ℝ × ℝ)) (b j : ℕ) : Grid (bl4 v b j) := grid_rn _

theorem grid_bl2 (v : List (ℝ × ℝ)) (b j : ℕ) : Grid (bl2 v b j) := grid_rn _

theorem grid_block8 (v : List (ℝ × ℝ)) (b : ℕ) : Grid (block8 v b) := grid_rn _

/-! ## The lanes -/

/-- Lane `j` after `E` epochs: exact sum `Σ_{i<E} x_{16i+j} y_{16i+j}`, depth `E`. -/
theorem lane_cert (v : List (ℝ × ℝ)) (j : ℕ) (k : ℕ) (hk : 2 * (k : ℝ) * unitRoundoff ≤ 1) :
    Cert (lane v j k) (∑ i ∈ Finset.range k, fe v (16 * i + j))
      (∑ i ∈ Finset.range k, |fe v (16 * i + j)|) k k := by
  induction k with
  | zero =>
    simp only [lane, Finset.range_zero, Finset.sum_empty]
    exact cert_zero
  | succ k ih =>
    have hk' : 2 * (k : ℝ) * unitRoundoff ≤ 1 := by
      push_cast at hk
      nlinarith [u_nonneg]
    have hd : 2 * ((k : ℝ) + 1) * unitRoundoff ≤ 1 := by
      push_cast at hk
      linarith
    have hf := cert_fma (x := (elem v (16 * k + j)).1) (y := (elem v (16 * k + j)).2) hd
      (ih hk')
    rw [Finset.sum_range_succ, Finset.sum_range_succ]
    simp only [lane]
    exact hf

/-! ## Regrouping and splitting sums -/

theorem sum_regroup (g : ℕ → ℝ) (E : ℕ) :
    ∑ m ∈ Finset.range (16 * E), g m
      = ∑ j ∈ Finset.range 16, ∑ i ∈ Finset.range E, g (16 * i + j) := by
  induction E with
  | zero => simp
  | succ E ih =>
    rw [show 16 * (E + 1) = 16 * E + 16 by ring, Finset.sum_range_add (fun m => g m) (16 * E) 16,
      ih]
    have hR : ∀ j, ∑ i ∈ Finset.range (E + 1), g (16 * i + j)
        = ∑ i ∈ Finset.range E, g (16 * i + j) + g (16 * E + j) :=
      fun j => Finset.sum_range_succ _ _
    simp only [hR, Finset.sum_add_distrib]

theorem sum_split8 (g : ℕ → ℝ) (a r : ℕ) (h8 : 8 ≤ r) :
    ∑ m ∈ Finset.range (a + r), g m
      = ∑ m ∈ Finset.range a, g m + ∑ k ∈ Finset.range 8, g (a + k)
        + ∑ k ∈ Finset.range (r - 8), g (a + 8 + k) := by
  have hr : a + r = (a + 8) + (r - 8) := by omega
  rw [hr, Finset.sum_range_add, Finset.sum_range_add]

theorem list_sum_eq (v : List (ℝ × ℝ)) (g : ℝ × ℝ → ℝ) :
    (v.map g).sum = ∑ i ∈ Finset.range v.length, g (elem v i) := by
  induction v with
  | nil => simp
  | cons a w ih =>
    rw [List.map_cons, List.sum_cons, ih, List.length_cons, Finset.sum_range_succ']
    simp only [elem, List.getD_cons_succ, List.getD_cons_zero]
    ring

theorem depth_le (n : ℕ) : depthAvx512 n ≤ n := by
  unfold depthAvx512 depthCore
  split_ifs <;> omega

/-! ## The reduction, the block and the tail -/

/-- The 16-lane reduction. Each lane has certificate `(S j, AS j)` at depth `E`. -/
theorem reduce16_cert (v : List (ℝ × ℝ)) (E : ℕ) (S AS : ℕ → ℝ)
    (hS : ∀ j, Cert (lane v j E) (S j) (AS j) E E)
    (hd : 2 * ((E : ℝ) + 4) * unitRoundoff ≤ 1) :
    Cert (reduce16 (fun j => lane v j E)) (∑ j ∈ Finset.range 16, S j)
      (∑ j ∈ Finset.range 16, AS j) (E + 4) (16 * E) := by
  have hu := u_nonneg
  have hE1 : 2 * ((E : ℝ) + 1) * unitRoundoff ≤ 1 := by linarith
  have hE2 : 2 * (((E + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have hE3 : 2 * (((E + 1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have hE4 : 2 * (((E + 1 + 1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have T : ∀ j, Cert (red8 (fun k => lane v k E) j) (S j + S (j + 8))
      (AS j + AS (j + 8)) (E + 1) (E + E) :=
    fun j => cert_add (d := E) (grid_lane v j E) (grid_lane v (j + 8) E) hE1 (le_refl _)
      (le_refl _) (hS j) (hS (j + 8))
  have W : ∀ j, Cert (red4 (fun k => lane v k E) j)
      (S j + S (j + 8) + (S (j + 4) + S (j + 4 + 8)))
      (AS j + AS (j + 8) + (AS (j + 4) + AS (j + 4 + 8))) (E + 1 + 1) (E + E + (E + E)) :=
    fun j => cert_add (d := E + 1) (grid_red8 _ j) (grid_red8 _ (j + 4)) hE2 (le_refl _)
      (le_refl _) (T j) (T (j + 4))
  have X0 := cert_add (d := E + 1 + 1) (grid_red4 _ 0) (grid_red4 _ 2) hE3 (le_refl _)
    (le_refl _) (W 0) (W 2)
  have X1 := cert_add (d := E + 1 + 1) (grid_red4 _ 1) (grid_red4 _ 3) hE3 (le_refl _)
    (le_refl _) (W 1) (W 3)
  have R := cert_add (d := E + 1 + 1 + 1) (grid_rn _) (grid_rn _) hE4 (le_refl _) (le_refl _)
    X0 X1
  refine cert_eq R ?_ ?_ ?_ ?_
  · simp only [Finset.sum_range_succ, Finset.sum_range_zero, zero_add, Nat.reduceAdd]
    ring
  · simp only [Finset.sum_range_succ, Finset.sum_range_zero, zero_add, Nat.reduceAdd]
    ring
  · omega
  · omega

/-- The 8-block: depth 4, `8` products. Needs `16 u ≤ 1`. -/
theorem block8_cert (v : List (ℝ × ℝ)) (b : ℕ) (hb : 16 * unitRoundoff ≤ 1) :
    Cert (block8 v b) (∑ k ∈ Finset.range 8, fe v (b + k))
      (∑ k ∈ Finset.range 8, |fe v (b + k)|) 4 8 := by
  have hu := u_nonneg
  have hP : ∀ i, Cert (prodR v i) (fe v i) |fe v i| 1 1 :=
    fun i => cert_mul (elem v i).1 (elem v i).2
  have hd1 : 2 * (((1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have B4 : ∀ j, Cert (bl4 v b j) (fe v (b + j) + fe v (b + (j + 4)))
      (|fe v (b + j)| + |fe v (b + (j + 4))|) (1 + 1) (1 + 1) :=
    fun j => cert_add (d := 1) (grid_prodR v (b + j)) (grid_prodR v (b + (j + 4))) hd1
      (le_refl _) (le_refl _) (hP (b + j)) (hP (b + (j + 4)))
  have hd2 : 2 * (((1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have C2 := fun j => cert_add (d := 1 + 1) (grid_bl4 v b j) (grid_bl4 v b (j + 2)) hd2
    (le_refl _) (le_refl _) (B4 j) (B4 (j + 2))
  have hd3 : 2 * (((1 + 1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast; linarith
  have C3 := cert_add (d := 1 + 1 + 1) (grid_bl2 v b 0) (grid_bl2 v b 1) hd3 (le_refl _)
    (le_refl _) (C2 0) (C2 1)
  refine cert_eq C3 ?_ ?_ ?_ ?_
  · simp only [Finset.sum_range_succ, Finset.sum_range_zero, zero_add, add_zero, Nat.reduceAdd]
    ring
  · simp only [Finset.sum_range_succ, Finset.sum_range_zero, zero_add, add_zero, Nat.reduceAdd]
    ring
  · norm_num
  · norm_num

/-- The scalar tail: `m` fused steps from element `i`. -/
theorem tail_cert (v : List (ℝ × ℝ)) (m : ℕ) : ∀ (i : ℕ) (c e A : ℝ) (d N : ℕ),
    2 * ((d : ℝ) + m) * unitRoundoff ≤ 1 → Cert c e A d N →
      Cert (tailAux v i m c) (e + ∑ k ∈ Finset.range m, fe v (i + k))
        (A + ∑ k ∈ Finset.range m, |fe v (i + k)|) (d + m) (N + m) := by
  induction m with
  | zero =>
    intro i c e A d N _ hc
    simpa [tailAux] using hc
  | succ m ih =>
    intro i c e A d N hd hc
    have hd0 : 2 * ((d : ℝ) + 1) * unitRoundoff ≤ 1 := by
      push_cast at hd
      nlinarith [mul_nonneg (Nat.cast_nonneg m : (0 : ℝ) ≤ m) u_nonneg, u_nonneg]
    have hd' : 2 * (((d + 1 : ℕ) : ℝ) + m) * unitRoundoff ≤ 1 := by
      push_cast at hd ⊢
      linarith
    have hstep := cert_fma (x := (elem v i).1) (y := (elem v i).2) hd0 hc
    have ih' := ih (i + 1) (rn ((elem v i).1 * (elem v i).2 + c))
      (e + (elem v i).1 * (elem v i).2) (A + |(elem v i).1 * (elem v i).2|) (d + 1) (N + 1)
      hd' hstep
    have hs : ∑ k ∈ Finset.range (m + 1), fe v (i + k)
        = fe v i + ∑ k ∈ Finset.range m, fe v (i + 1 + k) := by
      rw [Finset.sum_range_succ']
      have h' : ∑ k ∈ Finset.range m, fe v (i + (k + 1))
          = ∑ k ∈ Finset.range m, fe v (i + 1 + k) :=
        Finset.sum_congr rfl (fun k _ => by rw [show i + (k + 1) = i + 1 + k by omega])
      rw [h', add_zero, add_comm]
    have hsA : ∑ k ∈ Finset.range (m + 1), |fe v (i + k)|
        = |fe v i| + ∑ k ∈ Finset.range m, |fe v (i + 1 + k)| := by
      rw [Finset.sum_range_succ']
      have h' : ∑ k ∈ Finset.range m, |fe v (i + (k + 1))|
          = ∑ k ∈ Finset.range m, |fe v (i + 1 + k)| :=
        Finset.sum_congr rfl (fun k _ => by rw [show i + (k + 1) = i + 1 + k by omega])
      rw [h', add_zero, add_comm]
    simp only [tailAux]
    rw [hs, hsA]
    refine cert_eq ih' ?_ ?_ ?_ ?_
    · simp only [fe]
      ring
    · simp only [fe]
      ring
    · omega
    · omega

/-- The error certificate of the reduction `s0` of the lanes, for `E` epochs. -/
theorem s0_cert (v : List (ℝ × ℝ)) (E : ℕ) (hl : 2 * (E : ℝ) * unitRoundoff ≤ 1)
    (hd : E ≠ 0 → 2 * ((E : ℝ) + 4) * unitRoundoff ≤ 1) :
    Cert (reduce16 (fun j => lane v j E)) (∑ m ∈ Finset.range (16 * E), fe v m)
      (∑ m ∈ Finset.range (16 * E), |fe v m|) (if E = 0 then 0 else E + 4) (16 * E) := by
  rw [sum_regroup (fun m => fe v m) E, sum_regroup (fun m => |fe v m|) E]
  by_cases hE : E = 0
  · subst hE
    have h0 : reduce16 (fun j => lane v j 0) = 0 := by
      simp [reduce16, red4, red8, lane, rn_zero]
    rw [h0]
    simp [Cert, γ]
  · rw [if_neg hE]
    exact reduce16_cert v E (fun j => ∑ i ∈ Finset.range E, fe v (16 * i + j))
      (fun j => ∑ i ∈ Finset.range E, |fe v (16 * i + j)|) (fun j => lane_cert v j E hl) (hd hE)

/-! ## The full dot product -/

/-- The certificate of `dotCore` for `n = 16 E + r` elements. -/
theorem dotCore_cert (v : List (ℝ × ℝ)) (E r : ℕ) (hv : v.length = 16 * E + r)
    (hu : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotCore v E r) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (depthCore E r) v.length := by
  rw [hv] at hu ⊢
  have hu' : 2 * (16 * (E : ℝ) + r) * unitRoundoff ≤ 1 := by
    push_cast at hu
    linarith
  have hr0 : (0 : ℝ) ≤ r := Nat.cast_nonneg r
  have hE0 : (0 : ℝ) ≤ E := Nat.cast_nonneg E
  have hl : 2 * (E : ℝ) * unitRoundoff ≤ 1 := by
    nlinarith [mul_nonneg hr0 u_nonneg, mul_nonneg hE0 u_nonneg, u_nonneg]
  have hs0 := s0_cert v E hl (fun hE => by
    have h1 : (1 : ℝ) ≤ E := by exact_mod_cast Nat.one_le_iff_ne_zero.mpr hE
    nlinarith [mul_nonneg (show (0 : ℝ) ≤ 15 * (E : ℝ) + r - 4 by linarith) u_nonneg])
  unfold dotCore depthCore
  by_cases h8 : 8 ≤ r
  · rw [if_pos h8, if_pos h8]
    have h8r : (8 : ℝ) ≤ r := by exact_mod_cast h8
    have hb16 : 16 * unitRoundoff ≤ 1 := by
      nlinarith [mul_nonneg (show (0 : ℝ) ≤ (r : ℝ) - 8 by linarith) u_nonneg,
        mul_nonneg hE0 u_nonneg, u_nonneg]
    have hB := block8_cert v (16 * E) hb16
    have hd3 : 2 * (((if E = 0 then 4 else E + 4 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
      split_ifs with hE
      · subst hE
        push_cast
        linarith [u_nonneg]
      · have h1 : (1 : ℝ) ≤ E := by exact_mod_cast Nat.one_le_iff_ne_zero.mpr hE
        push_cast
        nlinarith [mul_nonneg (show (0 : ℝ) ≤ 15 * (E : ℝ) + r - 5 by linarith) u_nonneg]
    have hdd1 : (if E = 0 then 0 else E + 4) ≤ (if E = 0 then 4 else E + 4) := by
      split_ifs <;> omega
    have hdd2 : 4 ≤ (if E = 0 then 4 else E + 4) := by
      split_ifs <;> omega
    have hC := cert_add (d := if E = 0 then 4 else E + 4) (grid_reduce16 _)
      (grid_block8 v (16 * E)) hd3 hdd1 hdd2 hs0 hB
    have hdT2 : 2 * ((((if E = 0 then 4 else E + 4) + 1 : ℕ) : ℝ)
        + ((r - 8 : ℕ) : ℝ)) * unitRoundoff ≤ 1 := by
      rw [Nat.cast_sub h8]
      split_ifs with hE
      · subst hE
        push_cast at hu' ⊢
        linarith [u_nonneg]
      · have h1 : (1 : ℝ) ≤ E := by exact_mod_cast Nat.one_le_iff_ne_zero.mpr hE
        push_cast
        nlinarith [mul_nonneg (show (0 : ℝ) ≤ 15 * (E : ℝ) + 3 by linarith) u_nonneg]
    have hT2 := tail_cert v (r - 8) (16 * E + 8) _ _ _ _ _ hdT2 hC
    exact cert_eq hT2 (sum_split8 (fun m => fe v m) (16 * E) r h8).symm
      (sum_split8 (fun m => |fe v m|) (16 * E) r h8).symm rfl (by omega)
  · rw [if_neg h8, if_neg h8]
    have hdT : 2 * (((if E = 0 then 0 else E + 4 : ℕ) : ℝ) + (r : ℝ)) * unitRoundoff ≤ 1 := by
      split_ifs with hE
      · subst hE
        push_cast at hu' ⊢
        linarith [u_nonneg]
      · have h1 : (1 : ℝ) ≤ E := by exact_mod_cast Nat.one_le_iff_ne_zero.mpr hE
        push_cast
        nlinarith [mul_nonneg (show (0 : ℝ) ≤ 15 * (E : ℝ) - 4 by linarith) u_nonneg]
    have hT := tail_cert v r (16 * E) _ _ _ _ _ hdT hs0
    exact cert_eq hT (Finset.sum_range_add (fun m => fe v m) (16 * E) r).symm
      (Finset.sum_range_add (fun m => |fe v m|) (16 * E) r).symm rfl rfl

/-- The sequential AVX-512 dot product with the stated error bound (D-344). -/
theorem dotAvx512_err (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    |dotAvx512 v - (v.map fun p => p.1 * p.2).sum| ≤
      γ (depthAvx512 v.length) * (v.map fun p => |p.1 * p.2|).sum
        + 2 * v.length * subnormalError := by
  have hc : Cert (dotAvx512 v) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (depthAvx512 v.length) v.length :=
    dotCore_cert v (v.length / 16) (v.length % 16) (by omega) hn
  have e1 : (v.map fun p => p.1 * p.2).sum = ∑ m ∈ Finset.range v.length, fe v m :=
    list_sum_eq v (fun p => p.1 * p.2)
  have e2 : (v.map fun p => |p.1 * p.2|).sum = ∑ m ∈ Finset.range v.length, |fe v m| :=
    list_sum_eq v (fun p => |p.1 * p.2|)
  rw [e1, e2]
  unfold Cert at hc
  obtain ⟨hbd, -, -⟩ := hc
  have hD := depth_le v.length
  have hDu : ((depthAvx512 v.length : ℕ) : ℝ) * unitRoundoff ≤ 1 / 2 := by
    have h1 : ((depthAvx512 v.length : ℕ) : ℝ) ≤ (v.length : ℝ) := by exact_mod_cast hD
    have h2 := mul_le_mul_of_nonneg_right h1 u_nonneg
    linarith
  have hp := pow_unit_le_lin hDu
  have hp2 : (1 + unitRoundoff) ^ depthAvx512 v.length ≤ 2 := by
    linarith
  have hη := eta_nonneg
  have hbound : (v.length : ℝ) * (1 + unitRoundoff) ^ depthAvx512 v.length * subnormalError
      ≤ 2 * (v.length : ℝ) * subnormalError := by
    have := mul_le_mul_of_nonneg_right hp2 (mul_nonneg (Nat.cast_nonneg v.length) hη)
    linarith
  linarith [hbd, hbound]

end Gorse.Spec.DotAvx512
