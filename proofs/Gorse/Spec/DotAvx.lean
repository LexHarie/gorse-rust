import Gorse.Spec.DotAvx512

/-! # The AVX binary32 dot product: model and error bound (P4-6a, T3)

`dotAvx` models `dot` in common/floats/src/floats_avx.c:243-279, which `_mm256_dot` calls.
Lane `j` of the 8-lane accumulator takes the elements `8 i + j`. The first epoch is `mul` alone
(floats_avx.c:248-255). Every later epoch is `add (mul x y) s` (floats_avx.c:256-263). The
8 lanes reduce by the tree 8 → 4 → 2 → 1 (floats_avx.c:264-273). The scalar tail is
`sum += a[i] * b[i]` (floats_avx.c:274-277).

Each product and each sum rounds separately. The file is compiled with `-mavx -mf16c` and no
FMA instruction set, so the compiler can emit no fused operation. The disassembly confirms it
(D-345): `_mm256_dot` has no `vfmadd` or `vfmsub` instruction under the build.rs flags with clang.

The error bound uses the certificates of `DotAvx512`: `cert_mul` for a product (depth 1, one
`η`), and `cert_add` for a sum of two grid values (no new `η`). -/

namespace Gorse.Spec.DotAvx

open Gorse.Spec.Float32
open Gorse.Spec.Dot
open Gorse.Spec.DotAvx512

/-! ## Definitions -/

/-- Lane `j` after `k` epochs. Epoch 1 is the product alone. Every later epoch `k + 1` is
`rn (x*y) + s`, two roundings (floats_avx.c:248-263). -/
noncomputable def laneA (v : List (ℝ × ℝ)) (j : ℕ) : ℕ → ℝ
  | 0 => 0
  | 1 => prodR v j
  | k + 1 + 1 => rn (prodR v (8 * (k + 1) + j) + laneA v j (k + 1))

/-- The 8-lane reduction (floats_avx.c:264-273): lanes `j` and `j + 4` are added, then `j` and
`j + 2` of the results, and the two remaining sums are added. -/
noncomputable def reduce8A (s : ℕ → ℝ) : ℝ :=
  rn (rn (rn (s 0 + s 4) + rn (s 2 + s 6)) + rn (rn (s 1 + s 5) + rn (s 3 + s 7)))

/-- The scalar tail (floats_avx.c:274-277): each element is `sum += a[i] * b[i]`, which rounds
the product and then the sum. -/
noncomputable def tailA (v : List (ℝ × ℝ)) : ℕ → ℕ → ℝ → ℝ
  | _, 0, s => s
  | i, m + 1, s => tailA v (i + 1) m (rn (prodR v i + s))

/-- The dot product for `n = 8 E + r` elements: the reduction of the lanes, then the tail of `r`
elements from element `8 E`. -/
noncomputable def dotCoreA (v : List (ℝ × ℝ)) (E r : ℕ) : ℝ :=
  tailA v (8 * E) r (reduce8A (fun j => laneA v j E))

/-- The AVX dot product of a list of pairs. -/
noncomputable def dotAvx (v : List (ℝ × ℝ)) : ℝ :=
  dotCoreA v (v.length / 8) (v.length % 8)

/-- The rounding depth. With `E ≥ 1` the lanes have depth `E`, the reduction adds 3 levels, and
the tail adds `r`. With `E = 0` the lanes are exact zeros, and the tail adds `r`. -/
def depthAvx (n : ℕ) : ℕ := if n / 8 = 0 then n % 8 else n / 8 + 3 + n % 8

/-! ## Elementary facts -/

theorem cert_prodR (v : List (ℝ × ℝ)) (i : ℕ) : Cert (prodR v i) (fe v i) |fe v i| 1 1 :=
  cert_mul (elem v i).1 (elem v i).2

theorem grid_laneA (v : List (ℝ × ℝ)) (j k : ℕ) : Grid (laneA v j k) := by
  cases k with
  | zero => exact ⟨0, by simp [laneA]⟩
  | succ k =>
    cases k with
    | zero =>
      show Grid (prodR v j)
      exact grid_prodR v j
    | succ k =>
      show Grid (rn (prodR v (8 * (k + 1) + j) + laneA v j (k + 1)))
      exact grid_rn _

theorem grid_reduce8A (s : ℕ → ℝ) : Grid (reduce8A s) := grid_rn _

theorem rn_prodR (v : List (ℝ × ℝ)) (i : ℕ) : rn (prodR v i) = prodR v i := by
  unfold prodR
  exact rn_idem _

theorem prodR_add_zero (v : List (ℝ × ℝ)) (i : ℕ) : rn (prodR v i + 0) = prodR v i := by
  rw [add_zero]
  exact rn_prodR v i

/-- The tail from a zero sum: its first step is the product alone. -/
theorem tailA_zero_start (v : List (ℝ × ℝ)) (r : ℕ) :
    tailA v 0 (r + 1) 0 = tailA v 1 r (prodR v 0) := by
  simp only [tailA, prodR_add_zero]

/-- Splitting off the first term of a sum over `r + 1` terms, with the remaining terms shifted
by one. -/
theorem sum_shift_fe (v : List (ℝ × ℝ)) (r : ℕ) :
    ∑ m ∈ Finset.range (r + 1), fe v m = fe v 0 + ∑ k ∈ Finset.range r, fe v (1 + k) := by
  rw [Finset.sum_range_succ']
  have h' : ∑ k ∈ Finset.range r, fe v (k + 1) = ∑ k ∈ Finset.range r, fe v (1 + k) :=
    Finset.sum_congr rfl (fun k _ => by rw [Nat.add_comm])
  rw [h']
  ring

theorem sum_shift_abs (v : List (ℝ × ℝ)) (r : ℕ) :
    ∑ m ∈ Finset.range (r + 1), |fe v m| = |fe v 0| + ∑ k ∈ Finset.range r, |fe v (1 + k)| := by
  rw [Finset.sum_range_succ']
  have h' : ∑ k ∈ Finset.range r, |fe v (k + 1)| = ∑ k ∈ Finset.range r, |fe v (1 + k)| :=
    Finset.sum_congr rfl (fun k _ => by rw [Nat.add_comm])
  rw [h']
  ring

/-- Regrouping a sum over `8 E` elements by lane: lane `j` collects `8 i + j`. -/
theorem sum_regroup8 (g : ℕ → ℝ) (E : ℕ) :
    ∑ m ∈ Finset.range (8 * E), g m
      = ∑ j ∈ Finset.range 8, ∑ i ∈ Finset.range E, g (8 * i + j) := by
  induction E with
  | zero => simp
  | succ E ih =>
    rw [show 8 * (E + 1) = 8 * E + 8 by ring, Finset.sum_range_add (fun m => g m) (8 * E) 8, ih]
    have hR : ∀ j, ∑ i ∈ Finset.range (E + 1), g (8 * i + j)
        = ∑ i ∈ Finset.range E, g (8 * i + j) + g (8 * E + j) :=
      fun j => Finset.sum_range_succ _ _
    simp only [hR, Finset.sum_add_distrib]

theorem depthAvx_le (n : ℕ) : depthAvx n ≤ n := by
  unfold depthAvx
  split_ifs <;> omega

/-! ## The lanes -/

/-- Lane `j` after `k` epochs has certificate: exact sum `Σ_{i<k} x_{8i+j} y_{8i+j}`, depth `k`,
`k` products. -/
theorem laneA_cert (v : List (ℝ × ℝ)) (j : ℕ) (k : ℕ) :
    2 * (k : ℝ) * unitRoundoff ≤ 1 →
      Cert (laneA v j k) (∑ i ∈ Finset.range k, fe v (8 * i + j))
        (∑ i ∈ Finset.range k, |fe v (8 * i + j)|) k k := by
  induction k with
  | zero =>
    intro _
    simp only [laneA, Finset.range_zero, Finset.sum_empty]
    exact cert_zero
  | succ k ih =>
    intro hk
    cases k with
    | zero =>
      show Cert (prodR v j) _ _ (0 + 1) (0 + 1)
      simpa using cert_prodR v j
    | succ k =>
      have hk' : 2 * ((k + 1 : ℕ) : ℝ) * unitRoundoff ≤ 1 := by
        push_cast at hk ⊢
        linarith [u_nonneg]
      have hc := ih hk'
      have hd : 2 * (((k + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
        push_cast at hk ⊢
        linarith
      have h := cert_add (d := k + 1) (grid_prodR v (8 * (k + 1) + j)) (grid_laneA v j (k + 1))
        hd (by omega) (le_refl _) (cert_prodR v (8 * (k + 1) + j)) hc
      have hl : laneA v j (k + 1 + 1) = rn (prodR v (8 * (k + 1) + j) + laneA v j (k + 1)) := rfl
      rw [hl]
      refine cert_eq h ?_ ?_ rfl ?_
      · rw [Finset.sum_range_succ (fun i => fe v (8 * i + j)) (k + 1)]
        ring
      · rw [Finset.sum_range_succ (fun i => |fe v (8 * i + j)|) (k + 1)]
        ring
      · omega

/-! ## The reduction -/

/-- The 8-lane reduction has certificate: exact sum over `8 E` products, depth `E + 3`, `8 E`
products. It needs `E ≥ 1`, so that the lanes carry products rather than zeros. -/
theorem reduce8A_cert (v : List (ℝ × ℝ)) (E : ℕ) (hE : 1 ≤ E)
    (hd : 2 * ((E : ℝ) + 3) * unitRoundoff ≤ 1) :
    Cert (reduce8A (fun j => laneA v j E)) (∑ m ∈ Finset.range (8 * E), fe v m)
      (∑ m ∈ Finset.range (8 * E), |fe v m|) (E + 3) (8 * E) := by
  have hk : 2 * (E : ℝ) * unitRoundoff ≤ 1 := by linarith [u_nonneg]
  have hl : ∀ j, Cert (laneA v j E) (∑ i ∈ Finset.range E, fe v (8 * i + j))
      (∑ i ∈ Finset.range E, |fe v (8 * i + j)|) E E :=
    fun j => laneA_cert v j E hk
  have hE1 : 2 * ((E : ℝ) + 1) * unitRoundoff ≤ 1 := by linarith [u_nonneg]
  have hE2 : 2 * (((E + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast
    linarith [u_nonneg]
  have hE3 : 2 * (((E + 1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast
    linarith [u_nonneg]
  have c04 := cert_add (d := E) (grid_laneA v 0 E) (grid_laneA v 4 E) hE1 (le_refl E) (le_refl E)
    (hl 0) (hl 4)
  have c26 := cert_add (d := E) (grid_laneA v 2 E) (grid_laneA v 6 E) hE1 (le_refl E) (le_refl E)
    (hl 2) (hl 6)
  have c15 := cert_add (d := E) (grid_laneA v 1 E) (grid_laneA v 5 E) hE1 (le_refl E) (le_refl E)
    (hl 1) (hl 5)
  have c37 := cert_add (d := E) (grid_laneA v 3 E) (grid_laneA v 7 E) hE1 (le_refl E) (le_refl E)
    (hl 3) (hl 7)
  have w0 := cert_add (d := E + 1) (grid_rn _) (grid_rn _) hE2 (le_refl _) (le_refl _) c04 c26
  have w1 := cert_add (d := E + 1) (grid_rn _) (grid_rn _) hE2 (le_refl _) (le_refl _) c15 c37
  have R := cert_add (d := E + 1 + 1) (grid_rn _) (grid_rn _) hE3 (le_refl _) (le_refl _) w0 w1
  refine cert_eq R ?_ ?_ ?_ ?_
  · rw [sum_regroup8 (fun m => fe v m) E]
    simp only [Finset.sum_range_succ, Finset.sum_range_zero]
    ring
  · rw [sum_regroup8 (fun m => |fe v m|) E]
    simp only [Finset.sum_range_succ, Finset.sum_range_zero]
    ring
  · omega
  · omega

/-! ## The tail -/

/-- The scalar tail. It starts from a certificate of depth `d ≥ 1`, and each step adds one
product and one rounded sum. -/
theorem tailA_cert (v : List (ℝ × ℝ)) (m : ℕ) : ∀ (i : ℕ) (c e A : ℝ) (d N : ℕ),
    1 ≤ d → 2 * ((d : ℝ) + m) * unitRoundoff ≤ 1 → Grid c → Cert c e A d N →
      Cert (tailA v i m c) (e + ∑ k ∈ Finset.range m, fe v (i + k))
        (A + ∑ k ∈ Finset.range m, |fe v (i + k)|) (d + m) (N + m) := by
  induction m with
  | zero =>
    intro i c e A d N _ _ _ hc
    simpa [tailA] using hc
  | succ m ih =>
    intro i c e A d N hd1 hd hg hc
    have hmu : (0 : ℝ) ≤ (m : ℝ) * unitRoundoff := mul_nonneg (Nat.cast_nonneg m) u_nonneg
    have hd_step : 2 * ((d : ℝ) + 1) * unitRoundoff ≤ 1 := by
      push_cast at hd
      nlinarith [hmu, u_nonneg]
    have hstep := cert_add (d := d) (grid_prodR v i) hg hd_step hd1 (le_refl d)
      (cert_prodR v i) hc
    have hd' : 2 * (((d + 1 : ℕ) : ℝ) + m) * unitRoundoff ≤ 1 := by
      push_cast at hd ⊢
      linarith
    have ih' := ih (i + 1) (rn (prodR v i + c)) (fe v i + e) (|fe v i| + A) (d + 1) (1 + N)
      (by omega) hd' (grid_rn _) hstep
    have hs : ∑ k ∈ Finset.range (m + 1), fe v (i + k)
        = fe v i + ∑ k ∈ Finset.range m, fe v (i + 1 + k) := by
      rw [Finset.sum_range_succ']
      have h' : ∑ k ∈ Finset.range m, fe v (i + (k + 1))
          = ∑ k ∈ Finset.range m, fe v (i + 1 + k) :=
        Finset.sum_congr rfl (fun k _ => by rw [show i + (k + 1) = i + 1 + k by omega])
      rw [h', add_zero]
      ring
    have hsA : ∑ k ∈ Finset.range (m + 1), |fe v (i + k)|
        = |fe v i| + ∑ k ∈ Finset.range m, |fe v (i + 1 + k)| := by
      rw [Finset.sum_range_succ']
      have h' : ∑ k ∈ Finset.range m, |fe v (i + (k + 1))|
          = ∑ k ∈ Finset.range m, |fe v (i + 1 + k)| :=
        Finset.sum_congr rfl (fun k _ => by rw [show i + (k + 1) = i + 1 + k by omega])
      rw [h', add_zero]
      ring
    show Cert (tailA v (i + 1) m (rn (prodR v i + c)))
      (e + ∑ k ∈ Finset.range (m + 1), fe v (i + k))
      (A + ∑ k ∈ Finset.range (m + 1), |fe v (i + k)|) (d + (m + 1)) (N + (m + 1))
    refine cert_eq ih' ?_ ?_ ?_ ?_
    · rw [hs]
      ring
    · rw [hsA]
      ring
    · omega
    · omega

/-! ## The full dot product -/

/-- The certificate of `dotCoreA` for `E ≥ 1` epochs and `r` tail elements. -/
theorem dotCoreA_cert_pos (v : List (ℝ × ℝ)) (E r : ℕ) (hv : v.length = 8 * E + r)
    (hE : 1 ≤ E) (hu : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotCoreA v E r) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (E + 3 + r) v.length := by
  have hr0 : (0 : ℝ) ≤ (r : ℝ) := Nat.cast_nonneg r
  have hle : ((E + 3 + r : ℕ) : ℝ) ≤ (v.length : ℝ) := by
    rw [hv]
    exact_mod_cast (by omega : E + 3 + r ≤ 8 * E + r)
  have hu' : 2 * ((E : ℝ) + 3 + (r : ℝ)) * unitRoundoff ≤ 1 := by
    have hm := mul_le_mul_of_nonneg_right hle u_nonneg
    push_cast at hm ⊢
    linarith
  have hdE : 2 * ((E : ℝ) + 3) * unitRoundoff ≤ 1 := by
    have hm := mul_nonneg hr0 u_nonneg
    linarith
  have hs := reduce8A_cert v E hE hdE
  have hdT : 2 * (((E + 3 : ℕ) : ℝ) + (r : ℝ)) * unitRoundoff ≤ 1 := by
    push_cast
    linarith
  have hT := tailA_cert v r (8 * E) (reduce8A (fun j => laneA v j E))
    (∑ m ∈ Finset.range (8 * E), fe v m) (∑ m ∈ Finset.range (8 * E), |fe v m|) (E + 3) (8 * E)
    (by omega) hdT (grid_reduce8A _) hs
  have hsplit : (∑ m ∈ Finset.range v.length, fe v m)
      = ∑ m ∈ Finset.range (8 * E), fe v m + ∑ k ∈ Finset.range r, fe v (8 * E + k) := by
    rw [hv]
    exact Finset.sum_range_add (fun m => fe v m) (8 * E) r
  have hsplitA : (∑ m ∈ Finset.range v.length, |fe v m|)
      = ∑ m ∈ Finset.range (8 * E), |fe v m| + ∑ k ∈ Finset.range r, |fe v (8 * E + k)| := by
    rw [hv]
    exact Finset.sum_range_add (fun m => |fe v m|) (8 * E) r
  unfold dotCoreA
  exact cert_eq hT hsplit.symm hsplitA.symm rfl (by omega)

/-- The certificate of `dotCoreA` for `E = 0` epochs. The lanes are exact zeros, so the reduction
is `0`, and the first tail step is the product alone. -/
theorem dotCoreA_cert_zero (v : List (ℝ × ℝ)) (r : ℕ) (hv : v.length = r)
    (hu : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotCoreA v 0 r) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) r v.length := by
  have hval : reduce8A (fun j => laneA v j 0) = 0 := by
    simp [reduce8A, laneA, rn_zero]
  have hD : dotCoreA v 0 r = tailA v 0 r 0 := by
    simp only [dotCoreA, hval, Nat.mul_zero]
  cases r with
  | zero =>
    have hl : v.length = 0 := hv
    rw [hD, hl]
    unfold Cert
    simp [tailA, γ]
  | succ r' =>
    have hl : v.length = r' + 1 := hv
    rw [hD, tailA_zero_start]
    have hdr : 2 * (((1 : ℕ) : ℝ) + (r' : ℝ)) * unitRoundoff ≤ 1 := by
      have h := hu
      rw [hl] at h
      push_cast at h ⊢
      linarith
    have hc := tailA_cert v r' 1 (prodR v 0) (fe v 0) |fe v 0| 1 1 (le_refl 1) hdr
      (grid_prodR v 0) (cert_prodR v 0)
    refine cert_eq hc ?_ ?_ ?_ ?_
    · rw [hl, sum_shift_fe]
    · rw [hl, sum_shift_abs]
    · omega
    · omega

/-- The certificate of the AVX dot product, for every length. -/
theorem dotAvx_cert (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotAvx v) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (depthAvx v.length) v.length := by
  have hv : v.length = 8 * (v.length / 8) + v.length % 8 := by omega
  unfold dotAvx depthAvx
  by_cases hE : v.length / 8 = 0
  · rw [if_pos hE, hE]
    exact dotCoreA_cert_zero v (v.length % 8) (by omega) hn
  · rw [if_neg hE]
    exact dotCoreA_cert_pos v (v.length / 8) (v.length % 8) hv (by omega) hn

/-- The sequential AVX dot product with its error bound (D-345). For `2·n·u ≤ 1`, the computed
`dotAvx v` differs from the exact `Σ xᵢyᵢ` by at most `γ(depthAvx n)·Σ|xᵢyᵢ| + 2·n·η`. -/
theorem dotAvx_err (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    |dotAvx v - (v.map fun p => p.1 * p.2).sum| ≤
      γ (depthAvx v.length) * (v.map fun p => |p.1 * p.2|).sum
        + 2 * v.length * subnormalError := by
  have hc := dotAvx_cert v hn
  have e1 : (v.map fun p => p.1 * p.2).sum = ∑ m ∈ Finset.range v.length, fe v m :=
    list_sum_eq v (fun p => p.1 * p.2)
  have e2 : (v.map fun p => |p.1 * p.2|).sum = ∑ m ∈ Finset.range v.length, |fe v m| :=
    list_sum_eq v (fun p => |p.1 * p.2|)
  rw [e1, e2]
  unfold Cert at hc
  obtain ⟨hbd, -, -⟩ := hc
  have hD := depthAvx_le v.length
  have hDu : ((depthAvx v.length : ℕ) : ℝ) * unitRoundoff ≤ 1 / 2 := by
    have h1 : ((depthAvx v.length : ℕ) : ℝ) ≤ (v.length : ℝ) := by exact_mod_cast hD
    have h2 := mul_le_mul_of_nonneg_right h1 u_nonneg
    linarith
  have hp := pow_unit_le_lin hDu
  have hp2 : (1 + unitRoundoff) ^ depthAvx v.length ≤ 2 := by
    linarith
  have hη := eta_nonneg
  have hbound : (v.length : ℝ) * (1 + unitRoundoff) ^ depthAvx v.length * subnormalError
      ≤ 2 * (v.length : ℝ) * subnormalError := by
    have := mul_le_mul_of_nonneg_right hp2 (mul_nonneg (Nat.cast_nonneg v.length) hη)
    linarith
  linarith [hbd, hbound]

end Gorse.Spec.DotAvx
