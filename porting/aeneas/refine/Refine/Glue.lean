import AeneasSpikeTopk
import Refine.Order
import Refine.ListFacts

/-!
# Aeneas glue (P3-2 refinement, D-331)

The extracted code is in the `Result` monad. These lemmas turn each primitive it calls into an
equation: a call returns `ok` of a value with known properties. The loops in `Refine.Loops`
are then reasoned about with equations, not with the WP triples.
-/

open Aeneas Aeneas.Std Result
open aeneas_spike_topk

namespace Refine

/-- A WP triple, read as an equation: a `Result` that satisfies `P` is `ok` of a value that
satisfies `P`. -/
theorem exists_ok_of_spec {α : Type _} {m : Result α} {P : α → Prop}
    (h : WP.spec m P) : ∃ v, m = ok v ∧ P v := by
  cases m with
  | ok v => exact ⟨v, rfl, (WP.spec_ok v).1 h⟩
  | fail e => simp [WP.spec_fail] at h
  | div => simp [WP.spec_div] at h
  | vis e k => simp [WP.spec_vis] at h

/-! ## `usize` arithmetic -/

theorem usize_sub_eq (x y : Usize) (h : y.val ≤ x.val) :
    ∃ z : Usize, x - y = ok z ∧ z.val = x.val - y.val :=
  exists_ok_of_spec (Usize.sub_spec h)

theorem usize_div2_eq (x : Usize) : ∃ z : Usize, x / 2#usize = ok z ∧ z.val = x.val / 2 :=
  exists_ok_of_spec (Usize.div_spec x (by decide))

theorem usize_mul2_eq (x : Usize) (h : 2 * x.val ≤ Usize.max) :
    ∃ z : Usize, 2#usize * x = ok z ∧ z.val = 2 * x.val :=
  exists_ok_of_spec (Usize.mul_spec (x := 2#usize) (y := x) (by simpa using h))

theorem usize_add1_eq (x : Usize) (h : x.val + 1 ≤ Usize.max) :
    ∃ z : Usize, x + 1#usize = ok z ∧ z.val = x.val + 1 :=
  exists_ok_of_spec (Usize.add_spec (x := x) (y := 1#usize) (by simpa using h))

/-! ## Vectors and slices -/

/-- Indexing a vector inside its length returns the element there. -/
theorem vec_index_eq {E' : Type} (v : alloc.vec.Vec E') (i : Usize) (h : i.val < v.val.length) :
    ∃ e, alloc.vec.Vec.index (core.slice.index.SliceIndexUsizeSlice E') v i = ok e ∧
      v.val[i.val]? = some e := by
  obtain ⟨x, hx, hxv⟩ := exists_ok_of_spec (Slice.index_usize_spec v.slice i h)
  refine ⟨x, hx, ?_⟩
  rw [List.getElem?_eq_getElem h, hxv]

/-- `Vec::push` on a vector shorter than `usize::MAX` appends the element. -/
theorem vec_push_eq {E' : Type} (v : alloc.vec.Vec E') (x : E') (h : v.val.length < Usize.max) :
    ∃ v', alloc.vec.Vec.push v x = ok v' ∧ v'.val = v.val ++ [x] :=
  exists_ok_of_spec (alloc.vec.Vec.push_spec v x h)

/-- `Vec::pop` on a vector with a last element removes it and returns it. -/
theorem vec_pop_some {E' : Type} (A : Type) (v : alloc.vec.Vec E') (L : List E') (x : E')
    (hv : v.val = L ++ [x]) :
    ∃ w, alloc.vec.Vec.pop A v = ok (some x, w) ∧ w.val = L := by
  refine ⟨alloc.vec.Vec.from L (by have := v.property; rw [hv] at this; simpa using this), ?_, ?_⟩
  · unfold aeneas_spike_topk.alloc.vec.Vec.pop
    rw [hv]
    simp
  · rfl

/-- `Vec::pop` on an empty vector returns `None` and the vector. -/
theorem vec_pop_none {E' : Type} (A : Type) (v : alloc.vec.Vec E') (hv : v.val = []) :
    aeneas_spike_topk.alloc.vec.Vec.pop A v = ok (none, v) := by
  unfold aeneas_spike_topk.alloc.vec.Vec.pop
  rw [hv]
  simp

/-- `core::slice::swap`: the exchange of two in-range entries. -/
theorem swap_eq {E' : Type} [Inhabited E'] (s : Slice E') (a b : Usize)
    (ha : a.val < s.val.length) (hb : b.val < s.val.length) :
    ∃ s', core.slice.Slice.swap s a b = ok s' ∧ s'.val = swapL default s.val a.val b.val := by
  obtain ⟨s', hs', hP⟩ := exists_ok_of_spec (core.slice.Slice.swap_spec s a b ha hb)
  refine ⟨s', hs', ?_⟩
  obtain ⟨hlen, hA, hB, hO⟩ := hP
  apply List.ext_getElem?
  intro k
  rw [swapL_getElem? default s.val a.val b.val ha hb k]
  by_cases hka : k = a.val
  · subst hka
    simp only [if_pos rfl]
    rw [List.getElem?_eq_getElem (by omega), List.getElem?_eq_getElem hb]
    simp_all
  · by_cases hkb : k = b.val
    · subst hkb
      simp only [if_neg hka, if_pos rfl]
      rw [List.getElem?_eq_getElem (by omega), List.getElem?_eq_getElem ha]
      simp_all
    · simp only [if_neg hka, if_neg hkb]
      by_cases hk : k < s.val.length
      · rw [List.getElem?_eq_getElem hk, List.getElem?_eq_getElem (by omega)]
        simp_all
      · rw [List.getElem?_eq_none (by omega), List.getElem?_eq_none (by omega)]

/-! ## Monadic helpers -/

/-- `lift` of a pure value is `ok` of it. -/
theorem lift_eq {α : Type} (x : α) : lift x = ok x := rfl

end Refine
