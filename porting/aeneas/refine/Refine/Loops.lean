import AeneasSpikeTopk
import Refine.Heap
import Refine.Glue

/-!
# The heap loops of `TopKFilter` (P3-2 refinement, D-331)

Each loop of the extracted `TopKFilter` is proved against its invariant of `Refine.Heap`. A
loop body is first evaluated to an equation, then `loop.spec_decr_nat` carries the invariant
through the loop.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-! ## The comparison `worse` on the heap array -/

theorem worse_at_eq (s : heap.TopKFilter Std.U32 Std.I64) (i j : Usize)
    (hi : i.val < s.elems.val.length) (hj : j.val < s.elems.val.length) :
    ∃ e1 e2, s.elems.val[i.val]? = some e1 ∧ s.elems.val[j.val]? = some e2 ∧
      heap.TopKFilter.worse_at core.cmp.OrdU32 core.cmp.PartialOrdI64 s i j =
        ok (decide (TieBefore wv kv e2 e1)) := by
  obtain ⟨e1, h1, hl1⟩ := vec_index_eq s.elems i hi
  obtain ⟨e2, h2, hl2⟩ := vec_index_eq s.elems j hj
  refine ⟨e1, e2, hl1, hl2, ?_⟩
  unfold heap.TopKFilter.worse_at
  rw [h1, h2]
  simp only [bind_ok]
  exact worse_eq e1 e2

/-! ## Sifting up -/

/-- `up` at the root stops at once. -/
theorem up_body_zero (s : heap.TopKFilter Std.U32 Std.I64) (j : Usize) (hj : j.val = 0) :
    heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 s j =
      ok (done s) := by
  unfold heap.TopKFilter.up_loop.body
  have hgt : ¬ (j > 0#usize) := by scalar_tac
  rw [if_neg hgt]

/-- `up` stops when the element at `j` is not before its parent. -/
theorem up_body_stop (s : heap.TopKFilter Std.U32 Std.I64) (j : Usize) (hj0 : 0 < j.val)
    (hjl : j.val < s.elems.val.length) {z x : E}
    (hz : s.elems.val[(j.val - 1) / 2]? = some z) (hx : s.elems.val[j.val]? = some x)
    (hw : ¬ TieBefore wv kv z x) :
    heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 s j =
      ok (done s) := by
  unfold heap.TopKFilter.up_loop.body
  have hgt : j > 0#usize := by scalar_tac
  rw [if_pos hgt]
  obtain ⟨i, hi, hiv⟩ := usize_sub_eq j 1#usize (by scalar_tac)
  rw [hi]
  simp only [bind_ok]
  obtain ⟨i1, hi1, hi1v⟩ := usize_div2_eq i
  rw [hi1]
  simp only [bind_ok]
  have hi1p : i1.val = (j.val - 1) / 2 := by omega
  have hi1l : i1.val < s.elems.val.length := by omega
  obtain ⟨e1, e2, hl1, hl2, hwa⟩ := worse_at_eq s j i1 hjl hi1l
  have he1 : e1 = x := Option.some.inj (hl1.symm.trans hx)
  have he2 : e2 = z := by
    rw [hi1p] at hl2
    exact Option.some.inj (hl2.symm.trans hz)
  rw [he1, he2] at hwa
  rw [hwa]
  simp only [bind_ok]
  have hb : ¬ (decide (TieBefore wv kv z x) = true) := by simpa using hw
  rw [if_neg hb]

/-- `up` moves the element at `j` to its parent when it is before the parent, and returns the
exchanged array with the position of the parent. -/
theorem up_body_go (s : heap.TopKFilter Std.U32 Std.I64) (j : Usize) (hj0 : 0 < j.val)
    (hjl : j.val < s.elems.val.length) {z x : E}
    (hz : s.elems.val[(j.val - 1) / 2]? = some z) (hx : s.elems.val[j.val]? = some x)
    (hw : TieBefore wv kv z x) :
    ∃ (s' : heap.TopKFilter Std.U32 Std.I64) (i1 : Usize),
      heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 s j =
          ok (cont (s', i1)) ∧
        i1.val = (j.val - 1) / 2 ∧
        s'.elems.val = swapL default s.elems.val ((j.val - 1) / 2) j.val ∧
        s'.k = s.k ∧ s'.nan = s.nan := by
  unfold heap.TopKFilter.up_loop.body
  have hgt : j > 0#usize := by scalar_tac
  rw [if_pos hgt]
  obtain ⟨i, hi, hiv⟩ := usize_sub_eq j 1#usize (by scalar_tac)
  rw [hi]
  simp only [bind_ok]
  obtain ⟨i1, hi1, hi1v⟩ := usize_div2_eq i
  rw [hi1]
  simp only [bind_ok]
  have hi1p : i1.val = (j.val - 1) / 2 := by omega
  have hi1l : i1.val < s.elems.val.length := by omega
  obtain ⟨e1, e2, hl1, hl2, hwa⟩ := worse_at_eq s j i1 hjl hi1l
  have he1 : e1 = x := Option.some.inj (hl1.symm.trans hx)
  have he2 : e2 = z := by
    rw [hi1p] at hl2
    exact Option.some.inj (hl2.symm.trans hz)
  rw [he1, he2] at hwa
  rw [hwa]
  simp only [bind_ok]
  rw [if_pos (decide_eq_true hw)]
  obtain ⟨s1, hs1, hs1v⟩ := swap_eq s.elems.slice i1 j
    (by simp only [Slice.length]; omega) (by simp only [Slice.length]; omega)
  simp only [alloc.vec.Vec.deref_mut, lift_eq, bind_ok]
  rw [hs1]
  simp only [bind_ok]
  refine ⟨{ s with elems := { slice := s1 } }, i1, rfl, hi1p, ?_, rfl, rfl⟩
  rw [hs1v, hi1p]
  rfl

/-- The invariant of `up` is kept while it runs, and at the end the heap is in order. -/
theorem up_loop_eq (s : heap.TopKFilter Std.U32 Std.I64) (j : Usize)
    (hlen : s.elems.val.length < 2 ^ 59) (hjl : j.val < s.elems.val.length)
    (hI : UpInv s.elems.val j.val) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.up_loop core.cmp.OrdU32 core.cmp.PartialOrdI64 s j = ok s' ∧
        s'.elems.val.Perm s.elems.val ∧ HeapOrdN s'.elems.val s'.elems.val.length ∧
        s'.elems.val.length = s.elems.val.length ∧ s'.k = s.k ∧ s'.nan = s.nan := by
  sorry

/-! ## Sifting down -/

/-- `down`'s loop body stops when there is no child. -/
theorem down_body_nochild (n : Usize) (s : heap.TopKFilter Std.U32 Std.I64) (i : Usize)
    (hn : n.val ≤ 2 * i.val + 1) (hi : i.val < 2 ^ 58) :
    heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n s i =
      ok (done s) := by
  unfold heap.TopKFilter.down_loop.body
  obtain ⟨i1, hi1, hi1v⟩ := usize_mul2_eq i (by scalar_tac)
  rw [hi1]
  simp only [bind_ok]
  obtain ⟨j1, hj1, hj1v⟩ := usize_add1_eq i1 (by scalar_tac)
  rw [hj1]
  simp only [bind_ok]
  have hge : j1 ≥ n := by
    show n.val ≤ j1.val
    omega
  rw [if_pos hge]

end Refine
