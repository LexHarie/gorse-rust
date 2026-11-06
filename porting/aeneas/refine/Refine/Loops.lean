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
  have h1v : (1#usize).val = 1 := by scalar_tac
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
  have h1v : (1#usize).val = 1 := by scalar_tac
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
    (by show i1.val < s.elems.val.length; omega) (by show j.val < s.elems.val.length; omega)
  refine ⟨{ s with elems := { slice := s1 } }, i1, ?_, hi1p, ?_, rfl, rfl⟩
  · show (core.slice.Slice.swap s.elems.slice i1 j >>= fun s1 =>
        ok (cont ({ elems := { slice := s1 }, k := s.k, nan := s.nan }, i1))) = _
    rw [hs1] <;> rfl
  · show s1.val = _
    rw [hs1v, hi1p] <;> rfl

/-- The state of the `up` loop, the heap with the position it sifts from, keeps the invariant:
the position is in the array, the array is a permutation of the input, `k` and `nan` are
unchanged, and the sift invariant holds at the position. -/
abbrev UpState (s : heap.TopKFilter Std.U32 Std.I64)
    (p : heap.TopKFilter Std.U32 Std.I64 × Usize) : Prop :=
  p.2.val < p.1.elems.val.length ∧ p.1.elems.val.Perm s.elems.val ∧ p.1.k = s.k ∧
    p.1.nan = s.nan ∧ UpInv p.1.elems.val p.2.val

/-- What `up` leaves behind: a heap in order, a permutation of the input with the same `k` and
`nan`. -/
abbrev UpPost (s y : heap.TopKFilter Std.U32 Std.I64) : Prop :=
  HeapOrdN y.elems.val y.elems.val.length ∧ y.elems.val.Perm s.elems.val ∧
    y.elems.val.length = s.elems.val.length ∧ y.k = s.k ∧ y.nan = s.nan

set_option maxHeartbeats 4000000 in
/-- The invariant of `up` is kept while it runs, and at the end the heap is in order. -/
theorem up_loop_eq (s : heap.TopKFilter Std.U32 Std.I64) (j : Usize)
    (hjl : j.val < s.elems.val.length) (hI : UpInv s.elems.val j.val) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.up_loop core.cmp.OrdU32 core.cmp.PartialOrdI64 s j = ok s' ∧
        UpPost s s' := by
  have hBody : ∀ p : heap.TopKFilter Std.U32 Std.I64 × Usize, UpState s p →
      WP.spec (heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 p.1 p.2)
        (fun r => match r with
          | ControlFlow.done y => UpPost s y
          | ControlFlow.cont x' => UpState s x' ∧ x'.2.val < p.2.val) := by
    rintro ⟨s1, j1⟩ hst
    obtain ⟨hpos, hperm, hk, hn, hI1⟩ : j1.val < s1.elems.val.length ∧
        s1.elems.val.Perm s.elems.val ∧ s1.k = s.k ∧ s1.nan = s.nan ∧
          UpInv s1.elems.val j1.val := hst
    show WP.spec (heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 s1 j1) _
    by_cases h0 : j1.val = 0
    · -- At the root `up` stops at once, and the heap is in order.
      rw [up_body_zero s1 j1 h0]
      refine (WP.spec_ok _).2 ?_
      show UpPost s s1
      exact ⟨upInv_exit hI1 (Or.inl h0), hperm, hperm.length_eq, hk, hn⟩
    · have hpos0 : 0 < j1.val := Nat.pos_of_ne_zero h0
      obtain ⟨z, hz⟩ : ∃ z, s1.elems.val[(j1.val - 1) / 2]? = some z :=
        ⟨_, List.getElem?_eq_getElem (by omega)⟩
      obtain ⟨x, hx⟩ : ∃ x, s1.elems.val[j1.val]? = some x :=
        ⟨_, List.getElem?_eq_getElem hpos⟩
      by_cases hw : TieBefore wv kv z x
      · -- The element at `j1` is before its parent: the two are exchanged and `up` moves on.
        obtain ⟨s', i1, hb, hi1, hsE, hk', hn'⟩ := up_body_go s1 j1 hpos0 hpos hz hx hw
        rw [hb]
        refine (WP.spec_ok _).2 ?_
        show UpState s (s', i1) ∧ i1.val < j1.val
        have hperm' : s'.elems.val.Perm s.elems.val := by
          rw [hsE]
          exact (swapL_perm (default : heap.Elem Std.U32 Std.I64) s1.elems.val
            ((j1.val - 1) / 2) j1.val (by omega) hpos).trans hperm
        have hlen' : s'.elems.val.length = s.elems.val.length := hperm'.length_eq
        have hl1 : s1.elems.val.length = s.elems.val.length := hperm.length_eq
        have hI' : UpInv s'.elems.val i1.val := by
          rw [hsE, hi1]
          exact upInv_swap rfl hpos0 hpos hI1 hz hx hw
        exact ⟨⟨(by show i1.val < s'.elems.val.length; omega), hperm', hk'.trans hk,
          hn'.trans hn, hI'⟩, by omega⟩
      · -- The element at `j1` is not before its parent: `up` stops and the heap is in order.
        rw [up_body_stop s1 j1 hpos0 hpos hz hx hw]
        refine (WP.spec_ok _).2 ?_
        show UpPost s s1
        have hexit : j1.val = 0 ∨ ∀ z' x', s1.elems.val[(j1.val - 1) / 2]? = some z' →
            s1.elems.val[j1.val]? = some x' → G z' x' := by
          right
          intro z' x' hz' hx'
          have e1 : z = z' := Option.some.inj (hz.symm.trans hz')
          have e2 : x = x' := Option.some.inj (hx.symm.trans hx')
          rw [← e1, ← e2]
          exact hw
        exact ⟨upInv_exit hI1 hexit, hperm, hperm.length_eq, hk, hn⟩
  have hInv : UpState s (s, j) := ⟨hjl, List.Perm.refl _, rfl, rfl, hI⟩
  have hspec : WP.spec
      (loop (fun q => heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 q.1 q.2)
        (s, j))
      (UpPost s) :=
    loop.spec_decr_nat (fun p => p.2.val) (UpState s) (UpPost s)
      (fun q => heap.TopKFilter.up_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 q.1 q.2)
      (s, j) hBody hInv
  obtain ⟨s', hs', hP⟩ := exists_ok_of_spec hspec
  exact ⟨s', by unfold heap.TopKFilter.up_loop; exact hs', hP⟩

end Refine
