import Refine.PushPop

/-!
# `pop_all` of `TopKFilter` (P3-2 refinement, D-331)

`pop_all` pops the heap worst first into a vector, then reverses the vector. The loop keeps a
state that is the model's sorted list, split into what is still in the heap and what has been
popped: the popped elements, reversed, are the tail of the model's list.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-- A `usize` of value zero minus one fails. -/
theorem usize_sub_one_fail (x : Usize) (h : x.val = 0) : x - 1#usize = fail .integerOverflow := by
  have h1v : (1#usize).val = 1 := by scalar_tac
  show UScalar.sub x 1#usize = _
  unfold UScalar.sub
  rw [if_pos (show x.val < (1#usize).val by rw [h1v]; omega)]

/-- `pop_worst` of an empty heap returns `None` and the heap. -/
theorem pop_worst_empty (s : heap.TopKFilter Std.U32 Std.I64) (hs : s.elems.val = []) :
    heap.TopKFilter.pop_worst core.cmp.OrdU32 core.cmp.PartialOrdI64 s = ok (none, s) := by
  have hlenU : (alloc.vec.Vec.len s.elems).val = 0 := by
    rw [alloc.vec.Vec.len_val]
    show s.elems.val.length = 0
    simp [hs]
  have hco : Usize.checked_sub (alloc.vec.Vec.len s.elems) 1#usize = none := by
    unfold Usize.checked_sub core.num.checked_sub_UScalar
    rw [usize_sub_one_fail _ hlenU] <;> rfl
  unfold heap.TopKFilter.pop_worst
  simp only [lift_eq, bind_ok]
  rw [hco]
  simp only [core.option.Option.Insts.CoreOpsTry_traitTry.branch, bind_ok,
    core.option.Option.Insts.CoreOpsTry_traitFromResidualOptionNever.from_residual]

/-- The state of the `pop_all` loop: the heap holds `rem`, a sorted list, and the popped
elements, reversed, follow it to make up the model's list `buf`. The heap is in order. -/
abbrev PopState (buf : List E) (p : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) : Prop :=
  (∃ rem : List E, p.1.elems.val.Perm rem ∧ SortedBest wv kv rem ∧
      buf = rem ++ p.2.val.reverse) ∧
    HeapOrdN p.1.elems.val p.1.elems.val.length

/-- The loop of `pop_all` ends with the popped elements, reversed, equal to `buf`, provided the
model's list is shorter than `2^59`. -/
theorem pop_all_loop_eq (buf : List E) (hbuf : buf.length < 2 ^ 59)
    (h0 : heap.TopKFilter Std.U32 Std.I64) (res0 : alloc.vec.Vec E)
    (hI : PopState buf (h0, res0)) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ p : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E,
      heap.TopKFilter.pop_all_loop core.cmp.OrdU32 core.cmp.PartialOrdI64 h0 res0 = ok p ∧
        p.2.val.reverse = buf := by
  have hspec : WP.spec
      (loop (fun (q : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) =>
        heap.TopKFilter.pop_all_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 q.1 q.2) (h0, res0))
      (fun (y : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) => y.2.val.reverse = buf) := by
    refine loop.spec_decr_nat
      (fun (p : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) => p.1.elems.val.length)
      (PopState buf) (fun (y : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) => y.2.val.reverse = buf)
      (fun (q : heap.TopKFilter Std.U32 Std.I64 × alloc.vec.Vec E) =>
        heap.TopKFilter.pop_all_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 q.1 q.2)
      (h0, res0) ?_ hI
    rintro ⟨h, res⟩ ⟨⟨rem, hperm, hsorted, hbufe⟩, hH⟩
    show WP.spec (heap.TopKFilter.pop_all_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 h res) _
    unfold heap.TopKFilter.pop_all_loop.body
    by_cases hemp : h.elems.val = []
    · -- The heap is empty: the loop ends with the popped elements.
      have hrem : rem = [] := by
        have hps := hperm.symm
        rw [hemp] at hps
        exact List.perm_nil.1 hps
      rw [pop_worst_empty h hemp]
      simp only [bind_ok]
      refine (WP.spec_ok _).2 ?_
      show res.val.reverse = buf
      rw [hbufe, hrem]
      simp
    · -- The heap holds elements: the worst is popped and pushed onto the results.
      have hne : 0 < h.elems.val.length :=
        Nat.pos_of_ne_zero (fun h0 => hemp (List.eq_nil_of_length_eq_zero h0))
      obtain ⟨r, hr⟩ : ∃ r, h.elems.val[0]? = some r := ⟨_, List.getElem?_eq_getElem hne⟩
      have hremlen : rem.length = h.elems.val.length := hperm.length_eq.symm
      have hbl : buf.length = rem.length + res.val.reverse.length := by
        rw [hbufe, List.length_append]
      have hL : h.elems.val.length < 2 ^ 59 := by
        have hrl : res.val.reverse.length = res.val.length := List.length_reverse
        omega
      have hresl : res.val.length < 2 ^ 59 := by
        have hrl : res.val.reverse.length = res.val.length := List.length_reverse
        omega
      obtain ⟨h2, hpw, hperm2, hH2, hlen2, hk2, hnan2, hG⟩ :=
        pop_worst_eq h r hL hne hH hr hplat
      rw [hpw]
      simp only [bind_ok]
      obtain ⟨res1, hpush, hres1⟩ := vec_push_eq res r (by omega)
      -- reduce the `let (o, self1) := (some r, h2)` match first, so that `res.push` is visible
      first
        | (dsimp only; simp only [hpush, bind_tc_ok, bind_ok])
        | (dsimp only [hpush]; simp only [bind_tc_ok, bind_ok])
      -- the popped element is the last of `rem`
      have hrmem : r ∈ rem := hperm.mem_iff.1 (List.mem_iff_getElem?.2 ⟨0, hr⟩)
      have hwmem : ∀ y ∈ rem, ¬ TieBefore wv kv r y :=
        fun y hy => root_worst_mem hH hr rfl y (hperm.mem_iff.2 hy)
      obtain ⟨A, hA⟩ := root_last hsorted hrmem hwmem
      have hX : (r :: h2.elems.val).Perm (r :: A) :=
        hperm2.trans (hperm.trans (by rw [hA]; exact List.perm_append_comm))
      have hH2perm : h2.elems.val.Perm A := hX.cons_inv
      have hsA : SortedBest wv kv A := by
        rw [hA] at hsorted
        exact List.Pairwise.sublist (List.sublist_append_left A [r]) hsorted
      refine (WP.spec_ok _).2 ?_
      show PopState buf (h2, res1) ∧ h2.elems.val.length < h.elems.val.length
      refine ⟨⟨⟨A, hH2perm, hsA, ?_⟩, hH2⟩, by omega⟩
      rw [hbufe, hA, hres1]
      simp [List.append_assoc]
  obtain ⟨p, hp, hpost⟩ := exists_ok_of_spec hspec
  exact ⟨p, by unfold heap.TopKFilter.pop_all_loop; exact hp, hpost⟩

/-- A new vector is empty. -/
theorem vec_new_val (T : Type) : (alloc.vec.Vec.new T).val = [] := rfl

/-- `pop_all` returns the model's list `buf` as a vector, best first, and returns the heap it
emptied. `pop_all` reverses the popped vector, so `results` holds the worst first. -/
theorem pop_all_eq (self : heap.TopKFilter Std.U32 Std.I64) (buf : List E)
    (hbuf : buf.length < 2 ^ 59) (hperm : self.elems.val.Perm buf) (hsorted : SortedBest wv kv buf)
    (hH : HeapOrdN self.elems.val self.elems.val.length) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ (v : alloc.vec.Vec (heap.Elem Std.U32 Std.I64)) (self' : heap.TopKFilter Std.U32 Std.I64),
      heap.TopKFilter.pop_all core.cmp.OrdU32 core.cmp.PartialOrdI64 self = ok (v, self') ∧
        v.val = buf := by
  have hI : PopState buf (self, alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64)) :=
    ⟨⟨buf, hperm, hsorted, by
      show buf = buf ++ (alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64)).val.reverse
      simp [vec_new_val]⟩, hH⟩
  obtain ⟨⟨self1, res1⟩, hp, hpv⟩ := pop_all_loop_eq buf hbuf self _ hI hplat
  have hpv' : res1.val.reverse = buf := hpv
  refine ⟨{ slice := core.slice.Slice.reverse res1.slice }, self1, ?_, ?_⟩
  · have hlen : heap.TopKFilter.len core.cmp.OrdU32 core.cmp.PartialOrdI64 self =
        ok (alloc.vec.Vec.len self.elems) := rfl
    have hwc : alloc.vec.Vec.with_capacity (heap.Elem Std.U32 Std.I64)
        (alloc.vec.Vec.len self.elems) = alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64) := rfl
    unfold heap.TopKFilter.pop_all
    simp only [hlen, bind_ok, hwc, hp, lift_eq, alloc.vec.Vec.deref_mut]
    all_goals (first | with_unfolding_all rfl | simp)
  · have hr : (core.slice.Slice.reverse res1.slice).val = res1.val.reverse := by
      unfold core.slice.Slice.reverse
      simp [alloc.vec.Vec.val, Slice.from_val]
    show (core.slice.Slice.reverse res1.slice).val = buf
    rw [hr, hpv']

end Refine
