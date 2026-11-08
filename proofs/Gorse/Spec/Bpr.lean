import Mathlib

/-!
# The BPR step is one gradient step on each factor (T4 item 1, P5-4, D-361)

`bpr_update` (`crates/gorse-model/src/cf/model.rs`) draws a user `u`, a positive item `i` and a
negative item `j`, and moves the three factors `wᵤ`, `hᵢ` and `hⱼ` along the gradient of Rendle's
per-triple objective. All three use the old values. This file proves, over ℝ:

* `bprObj` is the per-triple objective, with the margin `⟨w, hᵢ⟩ − ⟨w, hⱼ⟩` as the code computes it;
* `bprStep` is the code's update, with the weight `σ(−margin)`;
* each factor's new value is its old value plus `lr` times the partial derivative of the
  objective along that factor, taken with Mathlib's `deriv` (`bprStep_user`, `bprStep_pos`,
  `bprStep_neg`).

In coordinates, the partial derivatives along the standard basis are the gradient. The state is
`Fin k → ℝ`, the same shape as the Rust `Vec<f32>`.

The stable sigmoid is modelled by `σ` itself. Its equality with the code's stable form is
`Gorse.Spec.Sigmoid.sigStable_eq`, and its f32 error is tier II (out of scope here).

There is no ℝ-level bug in this step: the code's update is the gradient step, so there is no
counterexample to prove first.
-/

namespace Gorse.Spec.Bpr

open Finset

variable {k : ℕ}

/-- The dot product of two factor vectors over ℝ. -/
noncomputable def dot (x y : Fin k → ℝ) : ℝ := ∑ g, x g * y g

/-- The squared Euclidean norm. -/
noncomputable def nrm2 (x : Fin k → ℝ) : ℝ := ∑ g, x g ^ 2

/-- The sigmoid `σ(x) = 1 / (1 + e^{−x})`. -/
noncomputable def sig (x : ℝ) : ℝ := 1 / (1 + Real.exp (-x))

/-- The BPR margin `⟨w, hᵢ⟩ − ⟨w, hⱼ⟩`, as `bpr_update` computes `diff`. -/
noncomputable def margin (w hi hj : Fin k → ℝ) : ℝ := dot w hi - dot w hj

/-- The per-triple BPR objective `log σ(margin) − (λ/2)(‖w‖² + ‖hᵢ‖² + ‖hⱼ‖²)` (Rendle 2009). -/
noncomputable def bprObj (reg : ℝ) (w hi hj : Fin k → ℝ) : ℝ :=
  Real.log (sig (margin w hi hj)) - reg / 2 * (nrm2 w + nrm2 hi + nrm2 hj)

/-- The partial derivative of `g` at `x` along coordinate `f`. -/
noncomputable def pderiv (g : (Fin k → ℝ) → ℝ) (x : Fin k → ℝ) (f : Fin k) : ℝ :=
  deriv (fun t : ℝ => g (Function.update x f t)) (x f)

/-- The BPR step of `bpr_update` over ℝ (`model.rs:767-787`). Each new factor is its old value
plus `lr` times its update direction, with `c = σ(−margin)` as the common weight. The three
directions read the old values, as the code's copies do. -/
noncomputable def bprStep (lr reg : ℝ) (w hi hj : Fin k → ℝ) :
    (Fin k → ℝ) × (Fin k → ℝ) × (Fin k → ℝ) :=
  let c := sig (-margin w hi hj)
  (fun f => w f + lr * (c * (hi f - hj f) - reg * w f),
   fun f => hi f + lr * (c * w f - reg * hi f),
   fun f => hj f + lr * (-c * w f - reg * hj f))

/-! ## The bridge from `Function.update` to the dot products -/

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

theorem dot_comm (x y : Fin k → ℝ) : dot x y = dot y x := by
  unfold dot
  exact Finset.sum_congr rfl fun g _ => mul_comm _ _

/-- Setting the first factor's coordinate `f` to `t`: the dot product moves by `(t − x_f) y_f`. -/
theorem dot_update (x y : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    dot (Function.update x f t) y = dot x y + (t - x f) * y f := by
  unfold dot
  rw [sum_update_mul x y f t, ← Finset.add_sum_erase univ _ (Finset.mem_univ f)]
  ring

/-- Setting the second factor's coordinate `f` to `t`: the dot product moves by `(t − x_f) y_f`. -/
theorem dot_update_right (y x : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    dot y (Function.update x f t) = dot y x + (t - x f) * y f := by
  rw [dot_comm y (Function.update x f t), dot_update, dot_comm x y]

theorem nrm2_update (x : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    nrm2 (Function.update x f t) = nrm2 x + (t ^ 2 - x f ^ 2) := by
  unfold nrm2
  rw [sum_update_sq x f t, ← Finset.add_sum_erase univ _ (Finset.mem_univ f)]
  ring

theorem margin_update_left (w hi hj : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    margin (Function.update w f t) hi hj = margin w hi hj + (t - w f) * (hi f - hj f) := by
  unfold margin
  rw [dot_update w hi f t, dot_update w hj f t]
  ring

theorem margin_update_right_pos (w hi hj : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    margin w (Function.update hi f t) hj = margin w hi hj + (t - hi f) * w f := by
  unfold margin
  rw [dot_update_right w hi f t]
  ring

theorem margin_update_right_neg (w hi hj : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    margin w hi (Function.update hj f t) = margin w hi hj - (t - hj f) * w f := by
  unfold margin
  rw [dot_update_right w hj f t]
  ring

/-! ## The one-dimensional derivative -/

/-- `log σ(x)` is `−log(1 + e^{−x})`, since `σ(x)` is the inverse of `1 + e^{−x}`. -/
theorem log_sig (x : ℝ) : Real.log (sig x) = -Real.log (1 + Real.exp (-x)) := by
  unfold sig
  rw [one_div, Real.log_inv]

/-- The coefficient of the derivative of `−log(1 + e^{−m})` is `σ(−m)`:
`e^{−m} / (1 + e^{−m}) = 1 / (1 + e^{m})`. -/
theorem exp_neg_div (m : ℝ) :
    Real.exp (-m) / (1 + Real.exp (-m)) = 1 / (1 + Real.exp m) := by
  have he : Real.exp m ≠ 0 := (Real.exp_pos m).ne'
  have h1 : 1 + (Real.exp m)⁻¹ ≠ 0 := by positivity
  have h2 : 1 + Real.exp m ≠ 0 := by positivity
  rw [Real.exp_neg m, div_eq_div_iff h1 h2]
  rw [mul_add, mul_one, one_mul, inv_mul_cancel₀ he]
  ring

/-- Along one coordinate, the objective with margin `m0 + (t − x0) a` and squared-norm part
`t² + c` has derivative `σ(−m0) a − λ x0` at `t = x0`. -/
theorem hasDerivAt_line (m0 a c reg x0 : ℝ) :
    HasDerivAt (fun t : ℝ => Real.log (sig (m0 + (t - x0) * a)) - reg / 2 * (t ^ 2 + c))
      (sig (-m0) * a - reg * x0) x0 := by
  have hfun : (fun t : ℝ => Real.log (sig (m0 + (t - x0) * a)) - reg / 2 * (t ^ 2 + c))
      = fun t : ℝ => -Real.log (1 + Real.exp (-(m0 + (t - x0) * a))) - reg / 2 * (t ^ 2 + c) := by
    funext t
    rw [log_sig]
  rw [hfun]
  have hl : HasDerivAt (fun t : ℝ => t - x0) 1 x0 := (hasDerivAt_id' x0).sub_const x0
  have hu : HasDerivAt (fun t : ℝ => -(m0 + (t - x0) * a)) (-(1 * a)) x0 :=
    ((hl.mul_const a).const_add m0).neg
  have he : HasDerivAt (fun t : ℝ => 1 + Real.exp (-(m0 + (t - x0) * a)))
      (Real.exp (-(m0 + (x0 - x0) * a)) * -(1 * a)) x0 :=
    hu.exp.const_add 1
  have hne : 1 + Real.exp (-(m0 + (x0 - x0) * a)) ≠ 0 := by positivity
  have hlog := (he.log hne).neg
  have hreg : HasDerivAt (fun t : ℝ => reg / 2 * (t ^ 2 + c)) (reg / 2 * (2 * x0)) x0 := by
    have h := ((hasDerivAt_pow 2 x0).add_const c).const_mul (reg / 2)
    convert h using 1
    norm_num
  have hall := hlog.sub hreg
  refine hall.congr_deriv ?_
  have h0 : m0 + (x0 - x0) * a = m0 := by ring
  rw [h0]
  unfold sig
  rw [neg_neg, ← exp_neg_div m0]
  ring

/-! ## The partial derivatives of the objective -/

/-- The objective along the user's coordinate `f`. -/
theorem partial_user (reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    pderiv (fun w' => bprObj reg w' hi hj) w f
      = sig (-margin w hi hj) * (hi f - hj f) - reg * w f := by
  have hfun : (fun t : ℝ => bprObj reg (Function.update w f t) hi hj)
      = fun t : ℝ => Real.log (sig (margin w hi hj + (t - w f) * (hi f - hj f)))
          - reg / 2 * (t ^ 2 + (nrm2 w - w f ^ 2 + nrm2 hi + nrm2 hj)) := by
    funext t
    unfold bprObj
    rw [margin_update_left, nrm2_update]
    ring
  unfold pderiv
  show deriv (fun t : ℝ => bprObj reg (Function.update w f t) hi hj) (w f) = _
  rw [hfun, (hasDerivAt_line (margin w hi hj) (hi f - hj f)
    (nrm2 w - w f ^ 2 + nrm2 hi + nrm2 hj) reg (w f)).deriv]

/-- The objective along the positive item's coordinate `f`. -/
theorem partial_pos (reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    pderiv (fun h => bprObj reg w h hj) hi f
      = sig (-margin w hi hj) * w f - reg * hi f := by
  have hfun : (fun t : ℝ => bprObj reg w (Function.update hi f t) hj)
      = fun t : ℝ => Real.log (sig (margin w hi hj + (t - hi f) * w f))
          - reg / 2 * (t ^ 2 + (nrm2 w + (nrm2 hi - hi f ^ 2) + nrm2 hj)) := by
    funext t
    unfold bprObj
    rw [margin_update_right_pos, nrm2_update]
    ring
  unfold pderiv
  show deriv (fun t : ℝ => bprObj reg w (Function.update hi f t) hj) (hi f) = _
  rw [hfun, (hasDerivAt_line (margin w hi hj) (w f)
    (nrm2 w + (nrm2 hi - hi f ^ 2) + nrm2 hj) reg (hi f)).deriv]

/-- The objective along the negative item's coordinate `f`. -/
theorem partial_neg (reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    pderiv (fun h => bprObj reg w hi h) hj f
      = -sig (-margin w hi hj) * w f - reg * hj f := by
  have hfun : (fun t : ℝ => bprObj reg w hi (Function.update hj f t))
      = fun t : ℝ => Real.log (sig (margin w hi hj + (t - hj f) * (-(w f))))
          - reg / 2 * (t ^ 2 + (nrm2 w + nrm2 hi + (nrm2 hj - hj f ^ 2))) := by
    funext t
    unfold bprObj
    rw [margin_update_right_neg, nrm2_update]
    have hm : margin w hi hj - (t - hj f) * w f
        = margin w hi hj + (t - hj f) * (-(w f)) := by
      ring
    rw [hm]
    ring
  unfold pderiv
  show deriv (fun t : ℝ => bprObj reg w hi (Function.update hj f t)) (hj f) = _
  rw [hfun, (hasDerivAt_line (margin w hi hj) (-(w f))
    (nrm2 w + nrm2 hi + (nrm2 hj - hj f ^ 2)) reg (hj f)).deriv]
  ring

/-! ## The BPR step is a gradient step (T4 item 1) -/

/-- The user's new factor is its old value plus `lr` times the gradient of the objective along
its coordinate. -/
theorem bprStep_user (lr reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    (bprStep lr reg w hi hj).1 f = w f + lr * pderiv (fun w' => bprObj reg w' hi hj) w f := by
  show w f + lr * (sig (-margin w hi hj) * (hi f - hj f) - reg * w f) = _
  rw [partial_user]

/-- The positive item's new factor is its old value plus `lr` times the gradient of the
objective along its coordinate. -/
theorem bprStep_pos (lr reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    (bprStep lr reg w hi hj).2.1 f = hi f + lr * pderiv (fun h => bprObj reg w h hj) hi f := by
  show hi f + lr * (sig (-margin w hi hj) * w f - reg * hi f) = _
  rw [partial_pos]

/-- The negative item's new factor is its old value plus `lr` times the gradient of the
objective along its coordinate. -/
theorem bprStep_neg (lr reg : ℝ) (w hi hj : Fin k → ℝ) (f : Fin k) :
    (bprStep lr reg w hi hj).2.2 f = hj f + lr * pderiv (fun h => bprObj reg w hi h) hj f := by
  show hj f + lr * (-sig (-margin w hi hj) * w f - reg * hj f) = _
  rw [partial_neg]

end Gorse.Spec.Bpr
