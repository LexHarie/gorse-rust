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
  first
    | exact WP.spec_imp_exists h
    | (cases m <;> first | exact ⟨_, rfl, (WP.spec_ok _).1 h⟩ | simp_all)

/-! ## `usize` arithmetic -/

set_option maxHeartbeats 1000000 in
theorem usize_sub_eq (x y : Usize) (h : y.val ≤ x.val) :
    ∃ z : Usize, x - y = ok z ∧ z.val = x.val - y.val :=
  exists_ok_of_spec (m := x - y) (P := fun z => z.val = x.val - y.val) (Usize.sub_spec h)

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
  rfl

/-- `Vec::push` on a vector shorter than `usize::MAX` appends the element. -/
theorem vec_push_eq {E' : Type} (v : alloc.vec.Vec E') (x : E') (h : v.val.length < Usize.max) :
    ∃ v', alloc.vec.Vec.push v x = ok v' ∧ v'.val = v.val ++ [x] :=
  exists_ok_of_spec (alloc.vec.Vec.push_spec v x h)

/-- `Vec::pop` on a vector with a last element removes it and returns it. -/
theorem vec_pop_some {E' : Type} (A : Type) (v : alloc.vec.Vec E') (L : List E') (x : E')
    (hv : v.val = L ++ [x]) :
    ∃ w, alloc.vec.Vec.pop A v = ok (some x, w) ∧ w.val = L := by
  refine ⟨alloc.vec.Vec.from L (by have h1 := v.property; rw [hv] at h1; simp at h1; omega), ?_, ?_⟩
  · unfold aeneas_spike_topk.alloc.vec.Vec.pop
    simp [hv]
  · first | rfl | simp

/-- `Vec::pop` on an empty vector returns `None` and the vector. -/
theorem vec_pop_none {E' : Type} (A : Type) (v : alloc.vec.Vec E') (hv : v.val = []) :
    aeneas_spike_topk.alloc.vec.Vec.pop A v = ok (none, v) := by
  unfold aeneas_spike_topk.alloc.vec.Vec.pop
  simp [hv]

/-- An in-range `getElem!` is the `getElem` at that index. -/
theorem getElem!_eq_of_lt {E' : Type} [Inhabited E'] (l : List E') (i : Nat) (h : i < l.length) :
    l[i]! = l[i] := by
  first
    | exact getElem!_pos l i h
    | simp [h]

/-- `core::slice::swap`: the exchange of two in-range entries. -/
theorem swap_eq {E' : Type} [Inhabited E'] (s : Slice E') (a b : Usize)
    (ha : a.val < s.val.length) (hb : b.val < s.val.length) :
    ∃ s', core.slice.Slice.swap s a b = ok s' ∧ s'.val = swapL default s.val a.val b.val := by
  obtain ⟨s', hs', hP⟩ := exists_ok_of_spec (core.slice.Slice.swap_spec s a b ha hb)
  refine ⟨s', hs', ?_⟩
  obtain ⟨hlen, hA, hB, hO⟩ := hP
  have hlen' : s'.val.length = s.val.length := by
    first
      | exact hlen
      | simpa using hlen
  have hl'a : a.val < s'.val.length := by rw [hlen']; exact ha
  have hl'b : b.val < s'.val.length := by rw [hlen']; exact hb
  have hA' : s'.val[a.val] = s.val[b.val] := by
    rw [← getElem!_eq_of_lt s'.val a.val hl'a, ← getElem!_eq_of_lt s.val b.val hb]
    exact hA
  have hB' : s'.val[b.val] = s.val[a.val] := by
    rw [← getElem!_eq_of_lt s'.val b.val hl'b, ← getElem!_eq_of_lt s.val a.val ha]
    exact hB
  apply List.ext_getElem?
  intro k
  rw [swapL_getElem? default s.val a.val b.val ha hb k]
  by_cases hka : k = a.val
  · subst hka
    simp only [if_pos rfl]
    rw [List.getElem?_eq_getElem hl'a, List.getElem?_eq_getElem hb, hA']
  · by_cases hkb : k = b.val
    · subst hkb
      simp only [if_neg hka, if_pos rfl]
      rw [List.getElem?_eq_getElem hl'b, List.getElem?_eq_getElem ha, hB']
    · simp only [if_neg hka, if_neg hkb]
      by_cases hk : k < s.val.length
      · have hk' : k < s'.val.length := by rw [hlen']; exact hk
        have e := hO k hka hkb
        have e' : s'.val[k] = s.val[k] := by
          rw [← getElem!_eq_of_lt s'.val k hk', ← getElem!_eq_of_lt s.val k hk]
          exact e
        rw [List.getElem?_eq_getElem hk', List.getElem?_eq_getElem hk, e']
      · have h1 : s'.val[k]? = none := List.getElem?_eq_none (by rw [hlen']; omega)
        have h2 : s.val[k]? = none := List.getElem?_eq_none (by omega)
        rw [h1, h2]

/-! ## Monadic helpers -/

/-- `lift` of a pure value is `ok` of it. -/
theorem lift_eq {α : Type} (x : α) : lift x = ok x := rfl

end Refine
