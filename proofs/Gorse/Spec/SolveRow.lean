import Mathlib

/-!
# The eALS loop keeps `predictions` equal to the dot products (T4 item 4, P5-4, D-361)

`solve_row` (`crates/gorse-model/src/cf/model.rs`) keeps a `predictions` array with one entry per
index of the other side. For each feedback index `i` it holds the current prediction
`⟨row, others[i]⟩`. A coordinate update of factor `f` changes the row in one place, so the code
patches each feedback prediction by the change instead of recomputing the dot product.

This file models that loop over ℝ and proves the invariant after every coordinate update, and so
after any sequence of them:

* `coordStep` is one update: it reads the residual `pred i − row f · O i f` (the code's `res[i]`
  with the old `row f`), sets factor `f` to the new value, and sets `pred i = res i + new · O i f`
  on the feedback set `F`.
* `coordStep_inv` shows that one update preserves `pred i = ⟨row, O i⟩` on `F`.
* `solveRow_invariant` shows that a sweep of any list of updates, started from the code's initial
  predictions, keeps the invariant.

The new values are arbitrary reals, so the proof does not depend on the eALS formula. Over ℝ the
invariant is exact. The f32 drift bound (the plan's "within a stated drift bound in f32") is not
here.

There is no ℝ-level bug in the loop, so there is no counterexample to prove first.
-/

namespace Gorse.Spec.SolveRow

open Finset

variable {ι : Type*} [DecidableEq ι] {k : ℕ}

/-- The dot product `⟨row, o⟩` over ℝ. -/
noncomputable def dot (row o : Fin k → ℝ) : ℝ := ∑ g, row g * o g

/-- The predictions of `solve_row` on entry (`model.rs:1295-1297`): the dot products on the
feedback set `F`. Off `F` the code leaves the entries alone, and the model sets them to `0`. -/
noncomputable def initPred (F : Finset ι) (O : ι → Fin k → ℝ) (row : Fin k → ℝ) : ι → ℝ :=
  fun i => if i ∈ F then dot row (O i) else 0

/-- One coordinate update (`model.rs:1298-1316`): factor `f` of the row becomes `new`, and the
predictions on the feedback set `F` are patched by the change in that factor. -/
noncomputable def coordStep (F : Finset ι) (O : ι → Fin k → ℝ) (row : Fin k → ℝ) (pred : ι → ℝ)
    (f : Fin k) (new : ℝ) : (Fin k → ℝ) × (ι → ℝ) :=
  (Function.update row f new,
   fun i => if i ∈ F then (pred i - row f * O i f) + new * O i f else pred i)

/-- A sweep: the coordinate updates of the list, in order. -/
noncomputable def sweep (F : Finset ι) (O : ι → Fin k → ℝ) :
    List (Fin k × ℝ) → (Fin k → ℝ) × (ι → ℝ) → (Fin k → ℝ) × (ι → ℝ)
  | [], s => s
  | fv :: rest, s => sweep F O rest (coordStep F O s.1 s.2 fv.1 fv.2)

/-! ## The bridge from `Function.update` to the dot product -/

/-- Setting factor `f` to `t` in a sum of products: the factor `f` term is `t · v_f`, and the other
terms do not change. -/
theorem sum_update_mul (p v : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    ∑ g, Function.update p f t g * v g = t * v f + ∑ g ∈ univ.erase f, p g * v g := by
  rw [← Finset.add_sum_erase univ _ (Finset.mem_univ f), Function.update_self]
  congr 1
  exact Finset.sum_congr rfl fun g hg => by
    rw [Function.update_of_ne (Finset.ne_of_mem_erase hg)]

/-- Setting factor `f` of the row to `t` moves the dot product by `(t − row_f) o_f`. -/
theorem dot_update (row o : Fin k → ℝ) (f : Fin k) (t : ℝ) :
    dot (Function.update row f t) o = dot row o + (t - row f) * o f := by
  unfold dot
  rw [sum_update_mul row o f t, ← Finset.add_sum_erase univ _ (Finset.mem_univ f)]
  ring

/-! ## The invariant -/

/-- Initially, the predictions on `F` are the dot products. -/
theorem initPred_eq (F : Finset ι) (O : ι → Fin k → ℝ) (row : Fin k → ℝ) :
    ∀ i ∈ F, initPred F O row i = dot row (O i) := by
  intro i hi
  simp [initPred, hi]

/-- One coordinate update keeps the predictions on `F` equal to the dot products, for any new
value of factor `f`. -/
theorem coordStep_inv (F : Finset ι) (O : ι → Fin k → ℝ) (row : Fin k → ℝ) (pred : ι → ℝ)
    (f : Fin k) (new : ℝ) (h : ∀ i ∈ F, pred i = dot row (O i)) :
    ∀ i ∈ F, (coordStep F O row pred f new).2 i
      = dot (coordStep F O row pred f new).1 (O i) := by
  intro i hi
  show (if i ∈ F then (pred i - row f * O i f) + new * O i f else pred i)
      = dot (Function.update row f new) (O i)
  rw [if_pos hi, dot_update, h i hi]
  ring

/-- A sweep of any list of coordinate updates keeps the invariant. -/
theorem sweep_inv (F : Finset ι) (O : ι → Fin k → ℝ) (steps : List (Fin k × ℝ)) :
    ∀ s : (Fin k → ℝ) × (ι → ℝ), (∀ i ∈ F, s.2 i = dot s.1 (O i)) →
      ∀ i ∈ F, (sweep F O steps s).2 i = dot (sweep F O steps s).1 (O i) := by
  induction steps with
  | nil =>
    intro s hs
    exact hs
  | cons fv rest ih =>
    intro s hs
    exact ih (coordStep F O s.1 s.2 fv.1 fv.2) (coordStep_inv F O s.1 s.2 fv.1 fv.2 hs)

/-- From the code's initial predictions, any sweep of coordinate updates keeps every feedback
prediction equal to its dot product. This is the loop invariant of `solve_row` (T4 item 4). -/
theorem solveRow_invariant (F : Finset ι) (O : ι → Fin k → ℝ) (row : Fin k → ℝ)
    (steps : List (Fin k × ℝ)) :
    ∀ i ∈ F, (sweep F O steps (row, initPred F O row)).2 i
      = dot (sweep F O steps (row, initPred F O row)).1 (O i) :=
  sweep_inv F O steps (row, initPred F O row) (fun i hi => initPred_eq F O row i hi)

end Gorse.Spec.SolveRow
