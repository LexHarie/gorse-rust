import Refine.PopAll

/-!
# The `TopKFilter` wrapper and the final theorem (P3-2 refinement, D-331)

`top_k_u32_i64` pushes the items one by one, then pops the heap into a sorted vector. The loop
keeps the model's list of the items so far: after `i` items the heap holds `topKModel k
(take i items)` and is in heap order. The final theorem says that the extracted function
returns the model's list of all the items, best first.

The model facts used here come from the core of the spec, `Gorse.Spec.TopKRegion` (D-331). The
rest of the spec, including `topKFilter_correct`, is not imported: it needs the older toolchain.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-- The empty list is in heap order. -/
theorem heapOrd_nil (n : ℕ) : HeapOrdN [] n := fun c a q _ _ ha _ => by simp at ha

/-- The model of an empty input is empty. -/
theorem topKModel_nil_eq (kn : ℕ) : topKModel wv kv kn [] = [] := rfl

/-- The model of one more item is one more push onto the model of the prefix. -/
theorem topKModel_snoc (kn : ℕ) (xs : List E) (x : E) :
    topKModel wv kv kn (xs ++ [x]) = pushStep wv kv kn (topKModel wv kv kn xs) x := by
  unfold topKModel
  simp only [List.foldl_append, List.foldl_cons, List.foldl_nil]

/-- Pushing onto a list of at most `k` elements keeps it within `k` elements. -/
theorem foldl_length_le (kn : ℕ) (xs : List E) :
    ∀ (buf : List E), buf.length ≤ kn →
      (xs.foldl (pushStep wv kv kn) buf).length ≤ kn := by
  induction xs with
  | nil => intro buf hb; simpa using hb
  | cons x xs ih =>
    intro buf _
    simp only [List.foldl_cons]
    apply ih
    unfold pushStep
    rw [List.length_take]
    exact min_le_left _ _

/-- Each push adds at most one element. -/
theorem foldl_length_le_len (kn : ℕ) (xs : List E) : ∀ (buf : List E),
    (xs.foldl (pushStep wv kv kn) buf).length ≤ buf.length + xs.length := by
  induction xs with
  | nil => intro buf; simp
  | cons x xs ih =>
    intro buf
    simp only [List.foldl_cons, List.length_cons]
    have h1 := ih (pushStep wv kv kn buf x)
    have h2 : (pushStep wv kv kn buf x).length ≤ buf.length + 1 := by
      have hp := (ins_perm wv kv x buf).length_eq
      simp only [List.length_cons] at hp
      unfold pushStep
      rw [List.length_take]
      omega
    omega

/-- The model's list has at most `k` elements, and at most as many as its input. -/
theorem topK_length_le (kn : ℕ) (xs : List E) :
    (topKModel wv kv kn xs).length ≤ min kn xs.length := by
  unfold topKModel
  refine le_min (foldl_length_le kn xs [] (by simp)) ?_
  simpa using foldl_length_le_len kn xs []

/-- Pushing onto a list in D-329 order keeps it in D-329 order. -/
theorem foldl_sorted (kn : ℕ) (xs : List E) : ∀ (buf : List E),
    SortedBest wv kv buf → SortedBest wv kv (xs.foldl (pushStep wv kv kn) buf) := by
  induction xs with
  | nil => intro buf hb; simpa using hb
  | cons x xs ih =>
    intro buf hb
    simp only [List.foldl_cons]
    apply ih
    unfold pushStep
    exact List.Pairwise.sublist (List.take_sublist kn _) (ins_sorted wv kv x buf hb)

/-- The model's list is in D-329 order. -/
theorem topKModel_sorted (kn : ℕ) (xs : List E) : SortedBest wv kv (topKModel wv kv kn xs) := by
  unfold topKModel
  exact foldl_sorted kn xs [] (by simp [SortedBest])

/-- Indexing a slice inside its length returns the element there. -/
theorem slice_index_eq {E' : Type} (s : Slice E') (i : Usize) (h : i.val < s.val.length) :
    ∃ e, Slice.index_usize s i = ok e ∧ s.val[i.val]? = some e := by
  obtain ⟨x, hx, hxv⟩ := exists_ok_of_spec (Slice.index_usize_spec s i h)
  refine ⟨x, hx, ?_⟩
  rw [List.getElem?_eq_getElem h, hxv]

/-- The state of the wrapper loop after `i` items: the heap has the `k` it was built with, and
it holds the model's top-k of the first `i` items, in heap order. -/
abbrev TopState (kn : ℕ) (items : List E) (p : heap.TopKFilter Std.U32 Std.I64 × Std.Usize) :
    Prop :=
  p.2.val ≤ items.length ∧ p.1.k.val = kn ∧
    p.1.elems.val.Perm (topKModel wv kv kn (items.take p.2.val)) ∧
    HeapOrdN p.1.elems.val p.1.elems.val.length

/-- What the wrapper loop leaves behind: the heap holds the model's top-k of all the items, in
heap order. -/
abbrev TopPost (kn : ℕ) (items : List E) (y : heap.TopKFilter Std.U32 Std.I64) : Prop :=
  y.elems.val.Perm (topKModel wv kv kn items) ∧ HeapOrdN y.elems.val y.elems.val.length

/-- The wrapper loop ends with the model's top-k of the items. -/
theorem top_k_loop_eq (kn : ℕ) (items : Slice (heap.Elem Std.U32 Std.I64))
    (hL : items.val.length < 2 ^ 59)
    (f0 : heap.TopKFilter Std.U32 Std.I64) (i0 : Std.Usize)
    (hI : TopState kn items.val (f0, i0)) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ f, top_k_u32_i64_loop items f0 i0 = ok f ∧ TopPost kn items.val f := by
  have hspec : WP.spec
      (loop (fun (q : heap.TopKFilter Std.U32 Std.I64 × Std.Usize) =>
        top_k_u32_i64_loop.body items q.1 q.2) (f0, i0))
      (TopPost kn items.val) := by
    refine loop.spec_decr_nat
      (fun (p : heap.TopKFilter Std.U32 Std.I64 × Std.Usize) => items.val.length - p.2.val)
      (TopState kn items.val) (TopPost kn items.val)
      (fun (q : heap.TopKFilter Std.U32 Std.I64 × Std.Usize) => top_k_u32_i64_loop.body items q.1 q.2)
      (f0, i0) ?_ hI
    rintro ⟨f, i⟩ ⟨hi, hk, hperm, hH⟩
    show WP.spec (top_k_u32_i64_loop.body items f i) _
    unfold top_k_u32_i64_loop.body
    dsimp only
    have hsl : (Slice.len items).val = items.val.length := Slice.len_val items
    by_cases hlt : i.val < items.val.length
    · have hgt : i < Slice.len items := by scalar_tac
      rw [if_pos hgt]
      obtain ⟨e, he, hee⟩ := slice_index_eq items i hlt
      rw [he]
      simp only [bind_ok]
      have hbl : (topKModel wv kv kn (items.val.take i.val)).length ≤ i.val := by
        have h1 := topK_length_le kn (items.val.take i.val)
        rw [List.length_take] at h1
        omega
      have hlen_le : (topKModel wv kv kn (items.val.take i.val)).length ≤ kn :=
        (topK_length_le kn _).trans (min_le_left _ _)
      have hflen : f.elems.val.length = (topKModel wv kv kn (items.val.take i.val)).length :=
        hperm.length_eq
      have hLf : f.elems.val.length + 1 < 2 ^ 59 := by omega
      obtain ⟨f1, hpush, hperm1, hH1, hk1, _⟩ :=
        push_eq f kn (topKModel wv kv kn (items.val.take i.val)) e.value e.weight hk
          (topKModel_sorted kn _) hperm hH hlen_le hLf hplat
      rw [hpush]
      simp only [bind_ok]
      obtain ⟨i2, hi2, hi2v⟩ := usize_add1_eq i (by omega)
      rw [hi2]
      simp only [bind_ok]
      have hT : items.val.take (i.val + 1) = items.val.take i.val ++ [e] := by
        simp [List.take_succ, hee]
      have hstep : topKModel wv kv kn (items.val.take (i.val + 1)) =
          pushStep wv kv kn (topKModel wv kv kn (items.val.take i.val)) ⟨e.value, e.weight⟩ := by
        rw [hT, topKModel_snoc]
      have hperm' : f1.elems.val.Perm (topKModel wv kv kn (items.val.take i2.val)) := by
        rw [show items.val.take i2.val = items.val.take (i.val + 1) by rw [hi2v], hstep]
        exact hperm1
      have hk2 : f1.k.val = kn := by
        rw [hk1]
        exact hk
      refine (WP.spec_ok _).2 ?_
      refine ⟨⟨?_, hk2, hperm', hH1⟩, ?_⟩
      · show i2.val ≤ items.val.length
        omega
      · show items.val.length - i2.val < items.val.length - i.val
        omega
    · have hge : ¬ (i < Slice.len items) := by scalar_tac
      rw [if_neg hge]
      refine (WP.spec_ok _).2 ?_
      show TopPost kn items.val f
      have htake : items.val.take i.val = items.val := List.take_of_length_le (by omega)
      rw [htake] at hperm
      exact ⟨hperm, hH⟩
  obtain ⟨p, hp, hpost⟩ := exists_ok_of_spec hspec
  exact ⟨p, by unfold top_k_u32_i64_loop; exact hp, hpost⟩

/-- The extracted `top_k_u32_i64` returns the model's top-k of its items, best first, for any
`k` and any input shorter than `2^59`. -/
theorem top_k_u32_i64_eq (k : Std.Usize) (items : Slice (heap.Elem Std.U32 Std.I64))
    (hL : items.val.length < 2 ^ 59) (hplat : 2 ^ 60 ≤ Usize.max) :
    ∃ v, top_k_u32_i64 k items = ok v ∧ v.val = topKModel wv kv k.val items.val := by
  have hnew : heap.TopKFilter.new core.cmp.OrdU32 core.cmp.PartialOrdI64 k =
      ok { elems := alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64), k := k, nan := 0#usize } :=
    rfl
  have h0 : (0#usize).val = 0 := by scalar_tac
  have hI0 : TopState k.val items.val
      ({ elems := alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64), k := k, nan := 0#usize },
        0#usize) := by
    refine ⟨?_, rfl, ?_, ?_⟩
    · show (0#usize).val ≤ items.val.length
      omega
    · show (alloc.vec.Vec.new (heap.Elem Std.U32 Std.I64)).val.Perm
        (topKModel wv kv k.val (items.val.take (0#usize).val))
      simp only [h0, List.take_zero, topKModel_nil_eq, vec_new_val, List.Perm.refl]
    · exact heapOrd_nil _
  obtain ⟨f, hf, hfP⟩ := top_k_loop_eq k.val items hL _ 0#usize hI0 hplat
  have hbuf : (topKModel wv kv k.val items.val).length < 2 ^ 59 := by
    have h1 := topK_length_le k.val items.val
    have h2 : min k.val items.val.length ≤ items.val.length := min_le_right _ _
    omega
  obtain ⟨v, self', hpa, hv⟩ := pop_all_eq f (topKModel wv kv k.val items.val) hbuf hfP.1
    (topKModel_sorted k.val items.val) hfP.2 hplat
  refine ⟨v, ?_, hv⟩
  unfold top_k_u32_i64
  simp only [hnew, bind_ok, hf, hpa]

end Refine
