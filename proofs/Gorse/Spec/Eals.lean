import Mathlib

/-!
# The eALS coordinate update sums over all items (A9, F6, T4 item 3, P5-3, D-360)

`solve_row` (`crates/gorse-model/src/cf/model.rs`) re-solves one factor `p f` of a row `p` by
the element-wise ALS (eALS) of He, Zhang, Kan and Chua 2016, §4, with the uniform missing-data
weight `cᵢ = α`. The missing-data term needs a Gram matrix `s = ∑ x xᵀ` over the other side. The
code sums it over the rows **with feedback** (`gram`), and He et al. sum over **all** items. A9
is decided as "all items" (plan §3.2).

The file has three parts, in this order.

1. **The bug (pre-fix).** `ealsCode α l R T Q p f` is the code's update for factor `f` when its
   Gram matrix sums over the rows `T`. With `T` the feedback items it is not a minimizer of the
   eALS objective over `S`, the set of all items. `ealsCode_feedback_not_min` is the counterexample.
2. **The fix.** With `T = S` the same formula is the unique minimizer over `t` of the objective,
   when `R ⊆ S`, `0 < λ` and `0 ≤ α`: `ealsCode_all_items_min` and `ealsCode_all_items_unique`
   (T4 item 3 of the plan, with `S` all items).
3. **The bridge and the algebra.** The objective is written in the residual form: the prediction
   of row `i` with factor `f` set to `t` is `resid p Q f i + t · Q i f`, where `resid` leaves out
   factor `f`. That is `ealsJ_update`. Then the objective is a quadratic in `t`,
   `K − 2tB + t²M` (`ealsJ_quad`), and the minimizer of a quadratic with `M > 0` is `B / M`.

The objective and the update are over ℝ. The `Function.update` form follows the Rust code's
in-place update, where factor `f` of the row is the only one that changes. -/

namespace Gorse.Spec.Eals

open Finset

variable {ι : Type*} [DecidableEq ι] {k : ℕ}

/-- The eALS objective of the row `p` (He et al. 2016, §4, with uniform `cᵢ = α`). The rows `R`
with feedback contribute `(1 − r̂ᵢ)²`, the rows of `S \ R` contribute `α r̂ᵢ²`, and the regularizer is
`λ‖p‖²`, where `r̂ᵢ = ∑_g p_g Q_ig`. -/
noncomputable def ealsJ (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ) : ℝ :=
  ∑ i ∈ R, (1 - ∑ g, p g * Q i g) ^ 2 + α * ∑ i ∈ S \ R, (∑ g, p g * Q i g) ^ 2
    + l * ∑ g, p g ^ 2

/-- The part of row `i`'s prediction that does not involve factor `f`: `∑_{g ≠ f} p_g Q_ig`. -/
noncomputable def resid (p : Fin k → ℝ) (Q : ι → Fin k → ℝ) (f : Fin k) (i : ι) : ℝ :=
  ∑ g ∈ univ.erase f, p g * Q i g

/-- The coordinate update of `solve_row` for factor `f`, when its Gram matrix sums over the rows
`T`: the numerator `a − b` is `∑_R (1 − (1−α) r_i) Q_if − α ∑_T r_i Q_if`, and the denominator
is `c + α s_ff + λ`, with `c = (1−α) ∑_R Q_if²` and `s_ff = ∑_T Q_if²`. -/
noncomputable def ealsCode (α l : ℝ) (R T : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) : ℝ :=
  (∑ i ∈ R, (1 - (1 - α) * resid p Q f i) * Q i f - α * ∑ i ∈ T, resid p Q f i * Q i f)
    / ((1 - α) * ∑ i ∈ R, Q i f ^ 2 + α * ∑ i ∈ T, Q i f ^ 2 + l)

/-! ## The bridge from `Function.update` to the residual form -/

/-- Setting factor `f` to `t` in a sum of products: the factor `f` term is `t · v_f`, and the other
terms do not change. -/
theorem sum_update_mul (p v : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    ∑ g, Function.update p f t g * v g = t * v f + ∑ g ∈ univ.erase f, p g * v g := by
  rw [← Finset.add_sum_erase univ _ (Finset.mem_univ f), Function.update_self]
  congr 1
  exact Finset.sum_congr rfl fun g hg => by
    rw [Function.update_of_ne (Finset.ne_of_mem_erase hg)]

/-- Setting factor `f` to `t` in a sum of squares. -/
theorem sum_update_sq (p : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    ∑ g, Function.update p f t g ^ 2 = t ^ 2 + ∑ g ∈ univ.erase f, p g ^ 2 := by
  rw [← Finset.add_sum_erase univ _ (Finset.mem_univ f), Function.update_self]
  congr 1
  exact Finset.sum_congr rfl fun g hg => by
    rw [Function.update_of_ne (Finset.ne_of_mem_erase hg)]

/-- The objective with factor `f` set to `t`, in the residual form. -/
theorem ealsJ_update (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) (t : ℝ) :
    ealsJ α l R S Q (Function.update p f t)
      = ∑ i ∈ R, (1 - (resid p Q f i + t * Q i f)) ^ 2
        + α * ∑ i ∈ S \ R, (resid p Q f i + t * Q i f) ^ 2
        + l * (t ^ 2 + ∑ g ∈ univ.erase f, p g ^ 2) := by
  have hpred : ∀ i, ∑ g, Function.update p f t g * Q i g = resid p Q f i + t * Q i f := by
    intro i
    rw [sum_update_mul p (Q i) f t, resid]
    ring
  unfold ealsJ
  simp only [hpred, sum_update_sq]

/-! ## The objective as a quadratic in the coordinate -/

/-- For a finite sum, `∑ (a_i + t b_i)² = ∑ a_i² + 2t ∑ a_i b_i + t² ∑ b_i²`. -/
theorem sum_sq_add (s : Finset ι) (a b : ι → ℝ) (t : ℝ) :
    ∑ i ∈ s, (a i + t * b i) ^ 2
      = ∑ i ∈ s, a i ^ 2 + 2 * t * ∑ i ∈ s, a i * b i + t ^ 2 * ∑ i ∈ s, b i ^ 2 := by
  rw [Finset.mul_sum, Finset.mul_sum, ← Finset.sum_add_distrib, ← Finset.sum_add_distrib]
  exact Finset.sum_congr rfl fun i _ => by ring

/-- The same identity with a minus sign: `∑ (a_i − t b_i)² = ∑ a_i² − 2t ∑ a_i b_i + t² ∑ b_i²`. -/
theorem sum_sq_sub (s : Finset ι) (a b : ι → ℝ) (t : ℝ) :
    ∑ i ∈ s, (a i - t * b i) ^ 2
      = ∑ i ∈ s, a i ^ 2 - 2 * t * ∑ i ∈ s, a i * b i + t ^ 2 * ∑ i ∈ s, b i ^ 2 := by
  rw [Finset.mul_sum, Finset.mul_sum, ← Finset.sum_sub_distrib, ← Finset.sum_add_distrib]
  exact Finset.sum_congr rfl fun i _ => by ring

/-- The eALS objective of the row, as `K − 2tB + t²M` in the coordinate `t` of factor `f`. -/
theorem ealsJ_quad (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) (t : ℝ) :
    ealsJ α l R S Q (Function.update p f t)
      = (∑ i ∈ R, (1 - resid p Q f i) ^ 2 + α * ∑ i ∈ S \ R, resid p Q f i ^ 2
          + l * ∑ g ∈ univ.erase f, p g ^ 2)
        - 2 * t * (∑ i ∈ R, (1 - resid p Q f i) * Q i f
            - α * ∑ i ∈ S \ R, resid p Q f i * Q i f)
        + t ^ 2 * (∑ i ∈ R, Q i f ^ 2 + α * ∑ i ∈ S \ R, Q i f ^ 2 + l) := by
  rw [ealsJ_update]
  have hR : ∑ i ∈ R, (1 - (resid p Q f i + t * Q i f)) ^ 2
      = ∑ i ∈ R, ((1 - resid p Q f i) - t * Q i f) ^ 2 :=
    Finset.sum_congr rfl fun i _ => by ring
  rw [hR, sum_sq_sub, sum_sq_add]
  ring

/-! ## The quadratic has a unique minimizer, `B / M` -/

/-- Completing the square: for `M ≠ 0`, the quadratic `K − 2tB + t²M` is its value at `B / M`
plus `M (t − B/M)²`. -/
theorem quad_complete {K B M t : ℝ} (hM : M ≠ 0) :
    K - 2 * t * B + t ^ 2 * M
      = (K - 2 * (B / M) * B + (B / M) ^ 2 * M) + M * (t - B / M) ^ 2 := by
  field_simp
  ring

/-- The value at `B / M` is the least value of the quadratic, when `M > 0`. -/
theorem quad_min_at {K B M : ℝ} (hM : 0 < M) (t : ℝ) :
    K - 2 * (B / M) * B + (B / M) ^ 2 * M ≤ K - 2 * t * B + t ^ 2 * M := by
  rw [quad_complete (K := K) (B := B) (t := t) hM.ne']
  linarith [mul_nonneg hM.le (sq_nonneg (t - B / M))]

/-- The minimizer of the quadratic, when `M > 0`, is `B / M`. -/
theorem quad_unique_at {K B M t : ℝ} (hM : 0 < M)
    (h : K - 2 * t * B + t ^ 2 * M = K - 2 * (B / M) * B + (B / M) ^ 2 * M) :
    t = B / M := by
  rw [quad_complete (K := K) (B := B) (t := t) hM.ne'] at h
  have h0 : M * (t - B / M) ^ 2 = 0 := by linarith
  have h1 : (t - B / M) ^ 2 = 0 := by
    rcases mul_eq_zero.mp h0 with h2 | h2
    · exact absurd h2 hM.ne'
    · exact h2
  have h3 : t - B / M = 0 := (pow_eq_zero_iff two_ne_zero).mp h1
  linarith

/-! ## The fix: the update with the Gram over all items is the minimizer -/

/-- With the Gram matrix over `S` (all items), the update is `B / M`: the numerator is
`∑_R (1 − r_i) Q_if − α ∑_{S\R} r_i Q_if` and the denominator is `∑_R Q_if² + α ∑_{S\R} Q_if² + λ`.
The identity uses `R ⊆ S`, so that `∑_S = ∑_{S\R} + ∑_R`. -/
theorem ealsCode_all_items (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) (hRS : R ⊆ S) :
    ealsCode α l R S Q p f
      = (∑ i ∈ R, (1 - resid p Q f i) * Q i f - α * ∑ i ∈ S \ R, resid p Q f i * Q i f)
        / (∑ i ∈ R, Q i f ^ 2 + α * ∑ i ∈ S \ R, Q i f ^ 2 + l) := by
  have hS : ∑ i ∈ S, resid p Q f i * Q i f
      = ∑ i ∈ S \ R, resid p Q f i * Q i f + ∑ i ∈ R, resid p Q f i * Q i f :=
    (Finset.sum_sdiff hRS).symm
  have hS2 : ∑ i ∈ S, Q i f ^ 2 = ∑ i ∈ S \ R, Q i f ^ 2 + ∑ i ∈ R, Q i f ^ 2 :=
    (Finset.sum_sdiff hRS).symm
  have hR : ∑ i ∈ R, (1 - (1 - α) * resid p Q f i) * Q i f
      = ∑ i ∈ R, (1 - resid p Q f i) * Q i f + α * ∑ i ∈ R, resid p Q f i * Q i f := by
    rw [Finset.mul_sum, ← Finset.sum_add_distrib]
    exact Finset.sum_congr rfl fun i _ => by ring
  unfold ealsCode
  rw [hS, hS2, hR]
  congr 1
  · ring
  · ring

/-- **Fix.** With the Gram matrix over all items, the update minimizes the eALS objective over the
coordinate `t`, when `R ⊆ S`, `0 < λ` and `0 ≤ α`. -/
theorem ealsCode_all_items_min (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) (hRS : R ⊆ S) (hl : 0 < l) (hα : 0 ≤ α) (t : ℝ) :
    ealsJ α l R S Q (Function.update p f (ealsCode α l R S Q p f))
      ≤ ealsJ α l R S Q (Function.update p f t) := by
  have hMpos : 0 < ∑ i ∈ R, Q i f ^ 2 + α * ∑ i ∈ S \ R, Q i f ^ 2 + l := by
    have h1 : 0 ≤ ∑ i ∈ S \ R, Q i f ^ 2 := Finset.sum_nonneg fun i _ => sq_nonneg _
    have h2 := mul_nonneg hα h1
    have h3 : 0 ≤ ∑ i ∈ R, Q i f ^ 2 := Finset.sum_nonneg fun i _ => sq_nonneg _
    linarith
  rw [ealsCode_all_items α l R S Q p f hRS, ealsJ_quad, ealsJ_quad]
  exact quad_min_at hMpos t

/-- **Fix, uniqueness.** The minimizer is unique: any other `t` with the same value is the update. -/
theorem ealsCode_all_items_unique (α l : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ)
    (f : Fin k) (hRS : R ⊆ S) (hl : 0 < l) (hα : 0 ≤ α) (t : ℝ)
    (h : ealsJ α l R S Q (Function.update p f t)
      = ealsJ α l R S Q (Function.update p f (ealsCode α l R S Q p f))) :
    t = ealsCode α l R S Q p f := by
  have hMpos : 0 < ∑ i ∈ R, Q i f ^ 2 + α * ∑ i ∈ S \ R, Q i f ^ 2 + l := by
    have h1 : 0 ≤ ∑ i ∈ S \ R, Q i f ^ 2 := Finset.sum_nonneg fun i _ => sq_nonneg _
    have h2 := mul_nonneg hα h1
    have h3 : 0 ≤ ∑ i ∈ R, Q i f ^ 2 := Finset.sum_nonneg fun i _ => sq_nonneg _
    linarith
  rw [ealsCode_all_items α l R S Q p f hRS] at h ⊢
  rw [ealsJ_quad, ealsJ_quad] at h
  exact quad_unique_at hMpos h

/-! ## The bug: the update with the Gram over the feedback items is not the minimizer

The counterexample is one factor (`k = 1`), two items, and a user whose only item is `0`. The
feedback items are `{0}`, the missing-data set is `{0, 1}`, `Q = 1`, `α = 1/2`, `λ = 1` and `p = 0`.
The code gives `(1 − 0) / ((1/2)·1 + (1/2)·1 + 1) = 1/2`. The objective is
`(1 − t)² + (1/2)t² + t²`, which is least at `t = 2/5`: `J(2/5) = 3/5 < 5/8 = J(1/2)`. -/

/-- The factor matrix of the counterexample: every entry is `1`. -/
def cexQ : Fin 2 → Fin 1 → ℝ := fun _ _ => 1

/-- The row of the counterexample: its one factor is `0`. -/
def cexP : Fin 1 → ℝ := fun _ => 0

/-- **Bug.** The code's update with the Gram over the feedback items is not the minimizer of the
eALS objective over all items. -/
theorem ealsCode_feedback_not_min :
    ¬ ∀ t : ℝ,
      ealsJ (1 / 2) 1 ({0} : Finset (Fin 2)) ({0, 1} : Finset (Fin 2)) cexQ
          (Function.update cexP 0
            (ealsCode (1 / 2) 1 ({0} : Finset (Fin 2)) ({0} : Finset (Fin 2)) cexQ cexP 0))
        ≤ ealsJ (1 / 2) 1 ({0} : Finset (Fin 2)) ({0, 1} : Finset (Fin 2)) cexQ
            (Function.update cexP 0 t) := by
  intro h
  have hE : (univ : Finset (Fin 1)).erase 0 = ∅ := by decide
  have hS : ({0, 1} : Finset (Fin 2)) \ {0} = {1} := by decide
  have hc : ealsCode (1 / 2) 1 ({0} : Finset (Fin 2)) ({0} : Finset (Fin 2)) cexQ cexP 0
      = 1 / 2 := by
    unfold ealsCode resid
    simp [cexQ, cexP, hE] <;> norm_num
  have hJ : ∀ t : ℝ,
      ealsJ (1 / 2) 1 ({0} : Finset (Fin 2)) ({0, 1} : Finset (Fin 2)) cexQ
          (Function.update cexP 0 t)
        = (1 - t) ^ 2 + 1 / 2 * t ^ 2 + t ^ 2 := by
    intro t
    unfold ealsJ
    rw [hS]
    simp [cexQ, cexP, Fin.sum_univ_one] <;> ring
  have h2 := h (2 / 5)
  rw [hc, hJ, hJ] at h2
  norm_num at h2

end Gorse.Spec.Eals
