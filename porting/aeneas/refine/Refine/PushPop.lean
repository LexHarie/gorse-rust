import Refine.Down
import Gorse.Spec.TopKRegion

/-!
# `pop_worst` and `push` of `TopKFilter` (P3-2 refinement, D-331)

`pop_worst` exchanges the root with the last element, sifts the root down over the other
positions, and pops the last element, which is the old root. `push` appends the element, sifts
it up, and calls `pop_worst` when the heap holds more than `k` elements. The refinement says
that the heap, read as a multiset, is the model's `pushStep` of `Gorse.Spec.TopK`.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-! ## Usize and list facts -/

/-- `checked_sub` of a `usize` at least one is `some` of the predecessor. -/
theorem usize_checked_sub_eq (x : Usize) (h : 1 ≤ x.val) :
    ∃ z : Usize, Usize.checked_sub x 1#usize = some z ∧ z.val = x.val - 1 := by
  have h1v : (1#usize).val = 1 := by scalar_tac
  obtain ⟨z, hz, hzv⟩ := usize_sub_eq x 1#usize (by omega)
  refine ⟨z, ?_, hzv⟩
  unfold Usize.checked_sub core.num.checked_sub_UScalar
  rw [hz]
  rfl

/-- Exchanging an index with itself does nothing. -/
theorem swapL_self {α : Type} (d : α) (l : List α) (i : Nat) (hi : i < l.length) :
    swapL d l i i = l := by
  apply List.ext_getElem?
  intro k
  rw [swapL_getElem? d l i i hi hi k]
  by_cases hk : k = i
  · subst hk
    simp
  · simp [hk]

/-- A list whose last entry is `x` is its prefix followed by `x`. -/
theorem eq_dropLast_append_of_getElem? {α : Type} (l : List α) (x : α)
    (h : l[l.length - 1]? = some x) : l = l.dropLast ++ [x] := by
  have hpos : 0 < l.length := by
    by_contra hc
    have hz : l.length = 0 := by omega
    have hnil : l = [] := List.eq_nil_of_length_eq_zero hz
    subst hnil
    simp at h
  have hlt : l.length - 1 < l.length := by omega
  rw [List.getElem?_eq_getElem hlt] at h
  have hne : l ≠ [] := by
    intro hl
    rw [hl] at hpos
    simp at hpos
  have hlast : l.getLast hne = x := by
    rw [List.getLast_eq_getElem]
    exact Option.some.inj h
  rw [← hlast]
  exact (List.dropLast_append_getLast hne).symm

/-- A prefix of a heap in order, of length at least `n`, is a heap in order. -/
theorem heapOrd_of_append_singleton {A : List E} {x : E} {n : Nat} (h : n ≤ A.length)
    (H : HeapOrdN (A ++ [x]) n) : HeapOrdN A n := by
  intro c a q hc hcn ha hq
  have ha' : (A ++ [x])[c]? = some a := by
    rw [List.getElem?_append_left (show c < A.length by omega)]
    exact ha
  have hq' : (A ++ [x])[(c - 1) / 2]? = some q := by
    rw [List.getElem?_append_left (show (c - 1) / 2 < A.length by omega)]
    exact hq
  exact H c a q hc hcn ha' hq'

/-- The root of a heap in order is not before any element of the heap. -/
theorem root_worst_mem {xs : List E} {L : Nat} (hH : HeapOrdN xs L) {r : E}
    (hr : xs[0]? = some r) (hlen : xs.length = L) : ∀ y ∈ xs, G r y := by
  intro y hy
  obtain ⟨c, hc⟩ := List.mem_iff_getElem?.1 hy
  obtain ⟨hcl, _⟩ := List.getElem?_eq_some_iff.1 hc
  exact root_worst hH hr c y (by omega) hc

/-- After the root is moved to the last position, `down` may start from the root with the last
position as its bound. -/
theorem downInv_root_swap {xs : List E} {L : Nat} (hH : HeapOrdN xs L) (hL0 : 0 < L)
    (hlen : xs.length = L) : DownInv (swapL default xs 0 (L - 1)) (L - 1) 0 := by
  constructor
  · intro c a q hc hcn hpc ha hq
    rw [swapL_getElem? default xs 0 (L - 1) (by omega) (by omega) c] at ha
    rw [swapL_getElem? default xs 0 (L - 1) (by omega) (by omega) ((c - 1) / 2)] at hq
    rw [if_neg (show ¬ (c = 0) by omega), if_neg (show ¬ (c = L - 1) by omega)] at ha
    rw [if_neg (show ¬ ((c - 1) / 2 = 0) by omega),
      if_neg (show ¬ ((c - 1) / 2 = L - 1) by omega)] at hq
    exact hH c a q hc (by omega) ha hq
  · intro c a g h0
    exact absurd h0 (lt_irrefl 0)

/-- Appending an element after a heap in order keeps the sift invariant at the new position. -/
theorem upInv_append {xs : List E} {n : Nat} (hH : HeapOrdN xs n) (hn : xs.length = n) (x : E) :
    UpInv (xs ++ [x]) n := by
  constructor
  · intro c a q hc hcn ha hq
    have hcl : c < xs.length + 1 := by
      have := (List.getElem?_eq_some_iff.1 ha).1
      simpa using this
    have ha' : xs[c]? = some a := by
      rw [List.getElem?_append_left (show c < xs.length by omega)] at ha
      exact ha
    have hq' : xs[(c - 1) / 2]? = some q := by
      rw [List.getElem?_append_left (show (c - 1) / 2 < xs.length by omega)] at hq
      exact hq
    exact hH c a q hc (by omega) ha' hq'
  · intro c a g hn0 hc hpc ha hg
    exfalso
    have hcl : c < xs.length + 1 := by
      have := (List.getElem?_eq_some_iff.1 ha).1
      simpa using this
    omega

/-! ## Popping the worst element -/

/-- `pop_worst` of a nonempty heap returns its root, the worst element, and the heap without it.
The rest of the heap stays in order, and is a permutation of the input without the root. -/
theorem pop_worst_eq (s : heap.TopKFilter Std.U32 Std.I64) (r : E)
    (hL : s.elems.val.length < 2 ^ 59) (hne : 0 < s.elems.val.length)
    (hH : HeapOrdN s.elems.val s.elems.val.length) (hr : s.elems.val[0]? = some r)
    (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.pop_worst core.cmp.OrdU32 core.cmp.PartialOrdI64 s = ok (some r, s') ∧
        (r :: s'.elems.val).Perm s.elems.val ∧ HeapOrdN s'.elems.val s'.elems.val.length ∧
        s'.elems.val.length = s.elems.val.length - 1 ∧ s'.k = s.k ∧ s'.nan = s.nan ∧
        ∀ y ∈ s'.elems.val, G r y := by
  have h0v : (0#usize).val = 0 := by scalar_tac
  have h1v : (1#usize).val = 1 := by scalar_tac
  have hlenU : (alloc.vec.Vec.len s.elems).val = s.elems.val.length :=
    alloc.vec.Vec.len_val s.elems
  obtain ⟨val, hvalE, hvalv⟩ :=
    usize_checked_sub_eq (alloc.vec.Vec.len s.elems) (by omega)
  have hvalL : val.val = s.elems.val.length - 1 := by omega
  -- exchange the root with the last element
  obtain ⟨sl, hsl, hslv⟩ := swap_eq s.elems.slice 0#usize val
    (by show (0#usize).val < s.elems.val.length; omega)
    (by show val.val < s.elems.val.length; omega)
  have hslv' : sl.val = swapL default s.elems.val 0 val.val := hslv
  have hlenS : sl.val.length = s.elems.val.length := by
    rw [hslv', swapL_length]
  -- sift the new root down over the first `val` positions
  have hI0 : DownInv sl.val val.val (0#usize).val := by
    rw [hslv', hvalL, h0v]
    exact downInv_root_swap hH hne rfl
  obtain ⟨s2, hs2, hP2⟩ := down_loop_eq { s with elems := { slice := sl } } val 0#usize
    (by show sl.val.length < 2 ^ 59; rw [hlenS]; exact hL)
    (by show val.val ≤ sl.val.length; rw [hlenS]; omega)
    (by rw [h0v]; norm_num)
    (by show DownInv sl.val val.val (0#usize).val; exact hI0) hplat
  obtain ⟨hHs2, hperm2, hlen2, hk2, hnan2, hbey2⟩ := hP2
  have hlen2' : s2.elems.val.length = s.elems.val.length := by
    rw [hlen2]
    exact hlenS
  -- the last position of `s2` holds the old root
  have hlast2 : s2.elems.val[val.val]? = some r := by
    rw [hbey2 val.val (Nat.le_refl _)]
    show sl.val[val.val]? = some r
    rw [hslv', swapL_getElem? default s.elems.val 0 val.val (by omega) (by omega) val.val]
    by_cases h0 : val.val = 0
    · rw [if_pos h0, h0]
      exact hr
    · rw [if_neg h0, if_pos (rfl : val.val = val.val)]
      exact hr
  have hidx : s2.elems.val.length - 1 = val.val := by omega
  have hlast2' : s2.elems.val[s2.elems.val.length - 1]? = some r := by
    rw [hidx]
    exact hlast2
  have hv2 : s2.elems.val = s2.elems.val.dropLast ++ [r] :=
    eq_dropLast_append_of_getElem? _ r hlast2'
  -- pop the last element, which is the old root
  obtain ⟨w, hw, hwv⟩ := vec_pop_some alloc.alloc.Global s2.elems _ r hv2
  -- the permutation of the array
  have hswapP : (swapL default s.elems.val 0 val.val).Perm s.elems.val := by
    by_cases h0 : val.val = 0
    · simp only [h0, swapL_self default s.elems.val 0 (by omega), List.Perm.refl]
    · exact swapL_perm default s.elems.val 0 val.val (by omega) (by omega)
  have hpermS : s2.elems.val.Perm s.elems.val := by
    have h1 : s2.elems.val.Perm sl.val := hperm2
    rw [hslv'] at h1
    exact h1.trans hswapP
  have hwlen : w.val.length = s.elems.val.length - 1 := by
    rw [hwv, List.length_dropLast, hlen2']
  have hdl : s2.elems.val.dropLast.length = val.val := by
    rw [List.length_dropLast, hlen2']
    omega
  have hHw : HeapOrdN w.val val.val := by
    rw [hwv]
    apply heapOrd_of_append_singleton (x := r) (le_of_eq hdl.symm)
    rw [← hv2]
    exact hHs2
  -- the pop is the last step of `pop_worst`
  have hpw : heap.TopKFilter.pop_worst core.cmp.OrdU32 core.cmp.PartialOrdI64 s =
      ok (some r, { s2 with elems := w }) := by
    unfold heap.TopKFilter.pop_worst
    simp only [lift_eq, bind_ok]
    rw [hvalE]
    simp only [core.option.Option.Insts.CoreOpsTry_traitTry.branch, bind_ok]
    simp only [alloc.vec.Vec.deref_mut, lift_eq, bind_ok]
    show (core.slice.Slice.swap s.elems.slice 0#usize val >>= _) = _
    simp only [hsl]
    unfold heap.TopKFilter.down
    simp only [hs2, hw]
  refine ⟨{ s2 with elems := w }, hpw, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · show (r :: w.val).Perm s.elems.val
    rw [hwv]
    have hrw : (r :: s2.elems.val.dropLast).Perm s2.elems.val := by
      conv_rhs => rw [hv2]
      exact (List.perm_append_singleton r _).symm
    exact hrw.trans hpermS
  · show HeapOrdN w.val w.val.length
    have hvw : val.val = w.val.length := by
      rw [hwlen]
      omega
    rw [← hvw]
    exact hHw
  · show w.val.length = s.elems.val.length - 1
    exact hwlen
  · exact hk2
  · exact hnan2
  · intro y hy
    have hy' : y ∈ w.val := hy
    rw [hwv] at hy'
    have hy2 : y ∈ s2.elems.val := by
      rw [hv2]
      exact List.mem_append.2 (Or.inl hy')
    have hyS : y ∈ s.elems.val := hpermS.mem_iff.1 hy2
    exact root_worst_mem hH hr rfl y hyS

/-! ## Pushing an element -/

/-- `push` of a weight that is not NaN appends the element, sifts it up, and pops the worst
element when the heap holds more than `k`. The heap then holds the model's `pushStep` of the
elements before, as a permutation, and stays in order. -/
theorem push_eq (s : heap.TopKFilter Std.U32 Std.I64) (k : Nat) (buf : List E)
    (item : Std.U32) (weight : Std.I64)
    (hk : s.k.val = k) (hbuf : SortedBest wv kv buf) (hperm : s.elems.val.Perm buf)
    (hH : HeapOrdN s.elems.val s.elems.val.length) (hlen : buf.length ≤ k)
    (hL : s.elems.val.length + 1 < 2 ^ 59) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.push core.cmp.OrdU32 core.cmp.PartialOrdI64 s item weight = ok s' ∧
        s'.elems.val.Perm (pushStep wv kv k buf ⟨item, weight⟩) ∧
        HeapOrdN s'.elems.val s'.elems.val.length ∧ s'.k = s.k ∧ s'.nan = s.nan := by
  have h1v : (1#usize).val = 1 := by scalar_tac
  have hbl : buf.length = s.elems.val.length := hperm.length_eq.symm
  obtain ⟨v, hv, hvv⟩ := vec_push_eq s.elems ⟨item, weight⟩ (by scalar_tac)
  have hvlen : v.val.length = s.elems.val.length + 1 := by
    rw [hvv]
    simp
  have hvLen : (alloc.vec.Vec.len v).val = s.elems.val.length + 1 := by
    rw [alloc.vec.Vec.len_val]
    exact hvlen
  obtain ⟨last, hlast, hlastv⟩ := usize_sub_eq (alloc.vec.Vec.len v) 1#usize (by omega)
  have hlastL : last.val = s.elems.val.length := by omega
  -- sift the new element up from the last position
  have hUpI : UpInv v.val last.val := by
    rw [hlastL, hvv]
    exact upInv_append hH rfl ⟨item, weight⟩
  have hjl : last.val < ({ s with elems := v } : heap.TopKFilter Std.U32 Std.I64).elems.val.length := by
    show last.val < v.val.length
    omega
  obtain ⟨s1, hs1, hUP⟩ := up_loop_eq { s with elems := v } last hjl hUpI
  have hs1len : s1.elems.val.length = s.elems.val.length + 1 := (hUP.2.2.1).trans hvlen
  have hks1 : s1.k = s.k := hUP.2.2.2.1
  have hnan1 : s1.nan = s.nan := hUP.2.2.2.2
  have hH1 : HeapOrdN s1.elems.val s1.elems.val.length := hUP.1
  have hPerm1 : s1.elems.val.Perm v.val := hUP.2.1
  -- the heap after the sift holds the elements of `ins x buf`
  have hA : (s.elems.val ++ [(⟨item, weight⟩ : heap.Elem Std.U32 Std.I64)]).Perm
      (buf ++ [(⟨item, weight⟩ : heap.Elem Std.U32 Std.I64)]) :=
    hperm.append (List.Perm.refl _)
  have hv' : v.val.Perm (buf ++ [⟨item, weight⟩]) := by
    rw [hvv]
    exact hA
  have hP1 : s1.elems.val.Perm (ins wv kv ⟨item, weight⟩ buf) :=
    hPerm1.trans (hv'.trans (List.perm_append_comm.trans (ins_perm wv kv _ buf).symm))
  have hCl : (ins wv kv ⟨item, weight⟩ buf).length = buf.length + 1 :=
    (ins_perm wv kv _ buf).length_eq.trans (by simp)
  have hLenS1 : (alloc.vec.Vec.len s1.elems).val = s.elems.val.length + 1 := by
    rw [alloc.vec.Vec.len_val]
    exact hs1len
  unfold heap.TopKFilter.push
  rw [is_nan_eq]
  simp only [bind_ok, Bool.false_eq_true, ↓reduceIte]
  rw [hv]
  simp only [bind_ok]
  rw [hlast]
  simp only [bind_ok]
  unfold heap.TopKFilter.up
  rw [hs1]
  simp only [bind_ok]
  unfold heap.TopKFilter.len
  simp only [bind_ok]
  by_cases hpop : s.elems.val.length + 1 > k
  · -- the heap holds `k + 1` elements: the worst is popped
    have hcond : alloc.vec.Vec.len s1.elems > s1.k := by
      show s1.k.val < (alloc.vec.Vec.len s1.elems).val
      rw [hLenS1, hks1, hk]
      omega
    obtain ⟨r, hr⟩ : ∃ r, s1.elems.val[0]? = some r :=
      ⟨_, List.getElem?_eq_getElem (by omega)⟩
    obtain ⟨s2, hpw, hperm2, hHo2, hlen2, hk2, hnan2, _⟩ :=
      pop_worst_eq s1 r (by omega) (by omega) hH1 hr hplat
    obtain ⟨A, hLA⟩ := root_last (ins_sorted wv kv _ buf hbuf)
      (hP1.mem_iff.1 (List.mem_iff_getElem?.2 ⟨0, hr⟩))
      (fun y hy => root_worst_mem hH1 hr rfl y (hP1.mem_iff.2 hy))
    have hAl : A.length = k := by
      have h1 := congrArg List.length hLA
      rw [hCl, List.length_append] at h1
      simp at h1
      omega
    have hpush : pushStep wv kv k buf ⟨item, weight⟩ = A := by
      unfold pushStep
      rw [hLA, ← hAl]
      simp
    refine ⟨s2, ?_, ?_, hHo2, hk2.trans hks1, hnan2.trans hnan1⟩
    · rw [if_pos hcond, hpw] <;> rfl
    · rw [hpush]
      have hX : (ins wv kv ⟨item, weight⟩ buf).Perm (r :: A) := by
        rw [hLA]
        exact List.perm_append_comm
      have hc : (r :: s2.elems.val).Perm (r :: A) := hperm2.trans (hP1.trans hX)
      exact hc.cons_inv
  · -- the heap holds at most `k` elements: nothing is popped
    have hcond : ¬ (alloc.vec.Vec.len s1.elems > s1.k) := by
      show ¬ (s1.k.val < (alloc.vec.Vec.len s1.elems).val)
      rw [hLenS1, hks1, hk]
      omega
    refine ⟨s1, ?_, ?_, hH1, hks1, hnan1⟩
    · rw [if_neg hcond]
    · have hcl : pushStep wv kv k buf ⟨item, weight⟩ = ins wv kv ⟨item, weight⟩ buf := by
        unfold pushStep
        exact List.take_of_length_le (by rw [hCl]; omega)
      rw [hcl]
      exact hP1

end Refine
