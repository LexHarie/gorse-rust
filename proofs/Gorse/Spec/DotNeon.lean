import Gorse.Spec.DotAvx512

/-! # The NEON binary32 dot product: model and error bound (P4-6b, T3)

`dotNeon` models `vdot` in common/floats/src/floats_neon.c:145-147, which calls the inline
`dot` at lines 115-143. The lanes are four float32x4 accumulators of one register, so lane `j`
takes the elements `4 i + j`.

- Epoch 1 is the product alone: `vmulq_f32` at line 122. It is modeled as `rn (x*y + 0)`, which
  is `rn (x*y)`.
- Every later epoch is `s = vmlaq_f32(s, v1, v2)` (line 129). `vmlaq_f32` is `a + b*c` in
  clang's `arm_neon.h`, so with `-ffp-contract=on` (clang's default) it is one fused `fmla`. It
  is modeled as `rn (x*y + s)`, one rounding.
- The four lanes are stored and then summed sequentially, starting from `sum = 0`
  (lines 133-138). That is `rn (rn (rn (rn (0 + s0) + s1) + s2) + s3)`, not a tree.
- The scalar tail `sum += a[i] * b[i]` (lines 139-141) is `fmadd` under clang, so it is one
  fused step `rn (x*y + s)`.

The disassembly confirms this for the build's flags (D-346): clang `--target=aarch64`, `-O3`,
`-fno-jump-tables -mllvm -inline-threshold=1000 -fno-asynchronous-unwind-tables -fno-exceptions
-fno-builtin -ffreestanding -ffixed-x18`. In `vdot` the first epoch is one `fmul`, the epoch loop
is one `fmla` per block, the reduction is a chain of four `fadd`, and the tail is one `fmadd` per
element. The 8-wide vector path in the tail is not reached when `n ≥ 0`, because the remainder
`n % 4` is at most 3.

The error bound reuses the certificates of `DotAvx512`: `cert_fma` for a fused step (depth +1,
one `η`), `cert_add` for a sum of two grid values (depth +1, no `η`), and `tail_cert` for the
fused tail. -/

namespace Gorse.Spec.DotNeon

open Gorse.Spec.Float32
open Gorse.Spec.Dot
open Gorse.Spec.DotAvx512

/-! ## Definitions -/

/-- Lane `j` after `k` epochs: `s ← rn (x*y + s)` with the element `4 i + j`
(floats_neon.c:120-132). Epoch 1 is `rn (x*y + 0)`, which is the `vmulq_f32` of line 122. -/
noncomputable def laneN (v : List (ℝ × ℝ)) (j : ℕ) : ℕ → ℝ
  | 0 => 0
  | k + 1 => rn (fe v (4 * k + j) + laneN v j k)

/-- The four lanes are summed in order, starting from `0` (floats_neon.c:133-138). -/
noncomputable def reduce4N (s : ℕ → ℝ) : ℝ :=
  rn (rn (rn (rn (0 + s 0) + s 1) + s 2) + s 3)

/-- The dot product for `n = 4 E + r` elements: the sequential reduction of the lanes, then the
fused tail of `r` elements from element `4 E` (floats_neon.c:139-141). -/
noncomputable def dotCoreN (v : List (ℝ × ℝ)) (E r : ℕ) : ℝ :=
  tailAux v (4 * E) r (reduce4N (fun j => laneN v j E))

/-- The NEON dot product of a list of pairs (floats_neon.c:145-147). -/
noncomputable def dotNeon (v : List (ℝ × ℝ)) : ℝ :=
  dotCoreN v (v.length / 4) (v.length % 4)

/-- The rounding depth. With `E ≥ 1` the lanes have depth `E`, the reduction adds 3 levels, and
the tail adds `r`. With `E = 0` the lanes are exact zeros, and the tail adds `r`. -/
def depthNeon (n : ℕ) : ℕ := if n / 4 = 0 then n % 4 else n / 4 + 3 + n % 4

/-! ## Elementary facts -/

theorem grid_laneN (v : List (ℝ × ℝ)) (j k : ℕ) : Grid (laneN v j k) := by
  cases k with
  | zero => exact ⟨0, by simp [laneN]⟩
  | succ k =>
    show Grid (rn (fe v (4 * k + j) + laneN v j k))
    exact grid_rn _

theorem rn_laneN (v : List (ℝ × ℝ)) (j k : ℕ) : rn (laneN v j k) = laneN v j k := by
  cases k with
  | zero => simp [laneN, rn_zero]
  | succ k =>
    show rn (rn (fe v (4 * k + j) + laneN v j k)) = rn (fe v (4 * k + j) + laneN v j k)
    exact rn_idem _

/-- The reduction, with the leading `0 +` removed: `0 + s0` is `s0`, because a lane value is
already a rounded value (`rn_laneN`). -/
theorem reduce4N_eq (v : List (ℝ × ℝ)) (E : ℕ) :
    reduce4N (fun j => laneN v j E)
      = rn (rn (rn (laneN v 0 E + laneN v 1 E) + laneN v 2 E) + laneN v 3 E) := by
  unfold reduce4N
  simp only [zero_add, rn_laneN]

/-- Regrouping a sum over `4 E` elements by lane: lane `j` collects `4 i + j`. -/
theorem sum_regroup4 (g : ℕ → ℝ) (E : ℕ) :
    ∑ m ∈ Finset.range (4 * E), g m
      = ∑ j ∈ Finset.range 4, ∑ i ∈ Finset.range E, g (4 * i + j) := by
  induction E with
  | zero => simp
  | succ E ih =>
    rw [show 4 * (E + 1) = 4 * E + 4 by ring, Finset.sum_range_add (fun m => g m) (4 * E) 4, ih]
    have hR : ∀ j, ∑ i ∈ Finset.range (E + 1), g (4 * i + j)
        = ∑ i ∈ Finset.range E, g (4 * i + j) + g (4 * E + j) :=
      fun j => Finset.sum_range_succ _ _
    simp only [hR, Finset.sum_add_distrib]

theorem depthNeon_le (n : ℕ) : depthNeon n ≤ n := by
  unfold depthNeon
  split_ifs <;> omega

/-! ## The lanes -/

/-- Lane `j` after `k` epochs has certificate: exact sum `Σ_{i<k} x_{4i+j} y_{4i+j}`, depth `k`,
`k` fused steps. -/
theorem laneN_cert (v : List (ℝ × ℝ)) (j : ℕ) (k : ℕ) (hk : 2 * (k : ℝ) * unitRoundoff ≤ 1) :
    Cert (laneN v j k) (∑ i ∈ Finset.range k, fe v (4 * i + j))
      (∑ i ∈ Finset.range k, |fe v (4 * i + j)|) k k := by
  induction k with
  | zero =>
    simp only [laneN, Finset.range_zero, Finset.sum_empty]
    exact cert_zero
  | succ k ih =>
    have hk' : 2 * (k : ℝ) * unitRoundoff ≤ 1 := by
      push_cast at hk
      nlinarith [u_nonneg]
    have hd : 2 * ((k : ℝ) + 1) * unitRoundoff ≤ 1 := by
      push_cast at hk
      linarith
    have hf := cert_fma (x := (elem v (4 * k + j)).1) (y := (elem v (4 * k + j)).2) hd
      (ih hk')
    rw [Finset.sum_range_succ, Finset.sum_range_succ]
    simp only [laneN]
    exact hf

/-! ## The reduction -/

/-- The sequential reduction has certificate: exact sum over `4 E` products, depth `E + 3`,
`4 E` products. Each of the three additions is a sum of two grid values (`cert_add`). -/
theorem reduce4N_cert (v : List (ℝ × ℝ)) (E : ℕ)
    (hd : 2 * ((E : ℝ) + 3) * unitRoundoff ≤ 1) :
    Cert (reduce4N (fun j => laneN v j E)) (∑ m ∈ Finset.range (4 * E), fe v m)
      (∑ m ∈ Finset.range (4 * E), |fe v m|) (E + 3) (4 * E) := by
  have hk : 2 * (E : ℝ) * unitRoundoff ≤ 1 := by linarith [u_nonneg]
  have hl : ∀ j, Cert (laneN v j E) (∑ i ∈ Finset.range E, fe v (4 * i + j))
      (∑ i ∈ Finset.range E, |fe v (4 * i + j)|) E E :=
    fun j => laneN_cert v j E hk
  have hE1 : 2 * ((E : ℝ) + 1) * unitRoundoff ≤ 1 := by linarith [u_nonneg]
  have hE2 : 2 * (((E + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast
    linarith [u_nonneg]
  have hE3 : 2 * (((E + 1 + 1 : ℕ) : ℝ) + 1) * unitRoundoff ≤ 1 := by
    push_cast
    linarith [u_nonneg]
  have c01 := cert_add (d := E) (grid_laneN v 0 E) (grid_laneN v 1 E) hE1 (by omega) (by omega)
    (hl 0) (hl 1)
  have c012 := cert_add (d := E + 1) (grid_rn _) (grid_laneN v 2 E) hE2 (by omega) (by omega)
    c01 (hl 2)
  have R := cert_add (d := E + 1 + 1) (grid_rn _) (grid_laneN v 3 E) hE3 (by omega) (by omega)
    c012 (hl 3)
  rw [reduce4N_eq]
  refine cert_eq R ?_ ?_ ?_ ?_
  · rw [sum_regroup4 (fun m => fe v m) E]
    simp only [Finset.sum_range_succ, Finset.sum_range_zero]
    ring
  · rw [sum_regroup4 (fun m => |fe v m|) E]
    simp only [Finset.sum_range_succ, Finset.sum_range_zero]
    ring
  · omega
  · omega

/-! ## The full dot product -/

/-- The certificate of `dotCoreN` for `E ≥ 1` epochs and `r` tail elements. -/
theorem dotCoreN_cert_pos (v : List (ℝ × ℝ)) (E r : ℕ) (hv : v.length = 4 * E + r)
    (hE : 1 ≤ E) (hu : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotCoreN v E r) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (E + 3 + r) v.length := by
  have hr0 : (0 : ℝ) ≤ (r : ℝ) := Nat.cast_nonneg r
  have hle : ((E + 3 + r : ℕ) : ℝ) ≤ (v.length : ℝ) := by
    rw [hv]
    exact_mod_cast (by omega : E + 3 + r ≤ 4 * E + r)
  have hu' : 2 * ((E : ℝ) + 3 + (r : ℝ)) * unitRoundoff ≤ 1 := by
    have hm := mul_le_mul_of_nonneg_right hle u_nonneg
    push_cast at hm ⊢
    linarith
  have hdE : 2 * ((E : ℝ) + 3) * unitRoundoff ≤ 1 := by
    have hm := mul_nonneg hr0 u_nonneg
    linarith
  have hs := reduce4N_cert v E hdE
  have hdT : 2 * (((E + 3 : ℕ) : ℝ) + (r : ℝ)) * unitRoundoff ≤ 1 := by
    push_cast
    linarith
  have hT := tail_cert v r (4 * E) (reduce4N (fun j => laneN v j E))
    (∑ m ∈ Finset.range (4 * E), fe v m) (∑ m ∈ Finset.range (4 * E), |fe v m|) (E + 3) (4 * E)
    hdT hs
  have hsplit : (∑ m ∈ Finset.range v.length, fe v m)
      = ∑ m ∈ Finset.range (4 * E), fe v m + ∑ k ∈ Finset.range r, fe v (4 * E + k) := by
    rw [hv]
    exact Finset.sum_range_add (fun m => fe v m) (4 * E) r
  have hsplitA : (∑ m ∈ Finset.range v.length, |fe v m|)
      = ∑ m ∈ Finset.range (4 * E), |fe v m| + ∑ k ∈ Finset.range r, |fe v (4 * E + k)| := by
    rw [hv]
    exact Finset.sum_range_add (fun m => |fe v m|) (4 * E) r
  unfold dotCoreN
  exact cert_eq hT hsplit.symm hsplitA.symm rfl (by omega)

/-- The certificate of `dotCoreN` for `E = 0` epochs. The lanes are exact zeros, so the reduction
is `0`, and the tail starts from depth `0`: each fused step adds one depth. -/
theorem dotCoreN_cert_zero (v : List (ℝ × ℝ)) (r : ℕ) (hv : v.length = r)
    (hu : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotCoreN v 0 r) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) r v.length := by
  have hval : reduce4N (fun j => laneN v j 0) = 0 := by
    simp [reduce4N, laneN, rn_zero]
  have hD : dotCoreN v 0 r = tailAux v 0 r 0 := by
    simp only [dotCoreN, hval, Nat.mul_zero]
  have hdr : 2 * (((0 : ℕ) : ℝ) + (r : ℝ)) * unitRoundoff ≤ 1 := by
    rw [hv] at hu
    push_cast
    linarith
  have hc := tail_cert v r 0 0 0 0 0 0 hdr cert_zero
  rw [hD]
  refine cert_eq hc ?_ ?_ ?_ ?_
  · rw [hv]
    simp
  · rw [hv]
    simp
  · omega
  · omega

/-- The certificate of the NEON dot product, for every length. -/
theorem dotNeon_cert (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    Cert (dotNeon v) (∑ m ∈ Finset.range v.length, fe v m)
      (∑ m ∈ Finset.range v.length, |fe v m|) (depthNeon v.length) v.length := by
  have hv : v.length = 4 * (v.length / 4) + v.length % 4 := by omega
  unfold dotNeon depthNeon
  by_cases hE : v.length / 4 = 0
  · rw [if_pos hE, hE]
    exact dotCoreN_cert_zero v (v.length % 4) (by omega) hn
  · rw [if_neg hE]
    exact dotCoreN_cert_pos v (v.length / 4) (v.length % 4) hv (by omega) hn

/-- The sequential NEON dot product with its error bound (D-346). For `2·n·u ≤ 1`, the computed
`dotNeon v` differs from the exact `Σ xᵢyᵢ` by at most `γ(depthNeon n)·Σ|xᵢyᵢ| + 2·n·η`. -/
theorem dotNeon_err (v : List (ℝ × ℝ)) (hn : 2 * (v.length : ℝ) * unitRoundoff ≤ 1) :
    |dotNeon v - (v.map fun p => p.1 * p.2).sum| ≤
      γ (depthNeon v.length) * (v.map fun p => |p.1 * p.2|).sum
        + 2 * v.length * subnormalError := by
  have hc := dotNeon_cert v hn
  have e1 : (v.map fun p => p.1 * p.2).sum = ∑ m ∈ Finset.range v.length, fe v m :=
    list_sum_eq v (fun p => p.1 * p.2)
  have e2 : (v.map fun p => |p.1 * p.2|).sum = ∑ m ∈ Finset.range v.length, |fe v m| :=
    list_sum_eq v (fun p => |p.1 * p.2|)
  rw [e1, e2]
  unfold Cert at hc
  obtain ⟨hbd, -, -⟩ := hc
  have hD := depthNeon_le v.length
  have hDu : ((depthNeon v.length : ℕ) : ℝ) * unitRoundoff ≤ 1 / 2 := by
    have h1 : ((depthNeon v.length : ℕ) : ℝ) ≤ (v.length : ℝ) := by exact_mod_cast hD
    have h2 := mul_le_mul_of_nonneg_right h1 u_nonneg
    linarith
  have hp := pow_unit_le_lin hDu
  have hp2 : (1 + unitRoundoff) ^ depthNeon v.length ≤ 2 := by
    linarith
  have hη := eta_nonneg
  have hbound : (v.length : ℝ) * (1 + unitRoundoff) ^ depthNeon v.length * subnormalError
      ≤ 2 * (v.length : ℝ) * subnormalError := by
    have := mul_le_mul_of_nonneg_right hp2 (mul_nonneg (Nat.cast_nonneg v.length) hη)
    linarith
  linarith [hbd, hbound]

end Gorse.Spec.DotNeon
