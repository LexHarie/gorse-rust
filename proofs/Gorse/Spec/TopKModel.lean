import Mathlib
import Gorse.Spec.TopK

/-! # The top-k keystone (P3-2, T2 of the verification plan)

`topKFilter_correct` states that the model of the Rust `TopKFilter` returns a top-k of its
input, in the sense of `IsTopK` (D-329). The model keeps the `k` best elements in a list
held in D-329 order, best first. Each pushed element is inserted at its place, and the
list is cut to `k` elements. That mirrors `TopKFilter::push` and `pop_all` in
`crates/gorse-common/src/heap.rs`, which keep a heap rather than a sorted list.

The model is not extracted from the Rust code. Aeneas at c2d2343 does not take f32, and the
loop is generic over the weight with a `Vec`-backed heap, so this is the G9 fallback
(D-331, linked to D-328). G9 links the model to the Rust code by the golden replay of
`heap_golden.json` with the D-330 overrides, which compare the Rust output bit for bit.
-/

namespace Gorse.Spec.TopK

variable {α W ι : Type} [DecidableEq α] [LinearOrder W] [LinearOrder ι]
  (w : α → W) (key : α → ι)

/-- A list is held in D-329 order when no later element is `TieBefore` an earlier one. -/
def SortedBest (L : List α) : Prop :=
  List.Pairwise (fun a c => ¬ TieBefore w key c a) L

/-- `TieBefore` is asymmetric. -/
theorem tieBefore_asymm {a b : α} (h : TieBefore w key a b) : ¬ TieBefore w key b a := by
  unfold TieBefore at h ⊢
  intro hb
  rcases h with h | ⟨h1, h2⟩ <;> rcases hb with hb | ⟨hb1, hb2⟩
  · exact lt_asymm h hb
  · rw [hb1] at h
    exact lt_irrefl _ h
  · rw [h1] at hb
    exact lt_irrefl _ hb
  · exact lt_asymm h2 hb2

/-- `TieBefore` is transitive. -/
theorem tieBefore_trans {a b c : α} (h1 : TieBefore w key a b) (h2 : TieBefore w key b c) :
    TieBefore w key a c := by
  unfold TieBefore at h1 h2 ⊢
  rcases h1 with h1 | ⟨h1, h1'⟩ <;> rcases h2 with h2 | ⟨h2, h2'⟩
  · exact Or.inl (lt_trans h2 h1)
  · exact Or.inl (by rw [← h2]; exact h1)
  · exact Or.inl (by rw [h1]; exact h2)
  · exact Or.inr ⟨h1.trans h2, lt_trans h1' h2'⟩

open Classical in
/-- Inserts `x` before the first element of the list that `x` is `TieBefore`. -/
noncomputable def ins (x : α) : List α → List α
  | [] => [x]
  | b :: l => if TieBefore w key x b then x :: b :: l else b :: ins x l

/-- The model of the loop's `push`: insert, then keep the `k` best. -/
noncomputable def pushStep (k : ℕ) (buf : List α) (x : α) : List α :=
  (ins w key x buf).take k

/-- The model of `TopKFilter`: push every element of `xs`, then the kept elements, best
first. -/
noncomputable def topKModel (k : ℕ) (xs : List α) : List α :=
  xs.foldl (pushStep w key k) []

/-- Insertion keeps the elements, and only permutes them. -/
theorem ins_perm (x : α) (L : List α) : List.Perm (ins w key x L) (x :: L) := by
  induction L with
  | nil => exact List.Perm.refl _
  | cons b l ih =>
    by_cases h : TieBefore w key x b
    · simp only [ins, if_pos h]
      exact List.Perm.refl _
    · simp only [ins, if_neg h]
      exact (ih.cons b).trans (List.Perm.swap x b l)

/-- Insertion into a list in D-329 order keeps it in D-329 order. -/
theorem ins_sorted (x : α) (L : List α) (hL : SortedBest w key L) :
    SortedBest w key (ins w key x L) := by
  induction L with
  | nil => simp [SortedBest, ins]
  | cons b l ih =>
    have hfull : List.Pairwise (fun a c => ¬ TieBefore w key c a) (b :: l) := hL
    have hbl : ∀ y ∈ l, ¬ TieBefore w key y b := (List.pairwise_cons.1 hfull).1
    have hl : List.Pairwise (fun a c => ¬ TieBefore w key c a) l :=
      (List.pairwise_cons.1 hfull).2
    by_cases h : TieBefore w key x b
    · have e : ins w key x (b :: l) = x :: b :: l := by simp only [ins, if_pos h]
      rw [e]
      show List.Pairwise (fun a c => ¬ TieBefore w key c a) (x :: b :: l)
      refine List.pairwise_cons.2 ⟨?_, hfull⟩
      intro y hy
      rcases List.mem_cons.1 hy with rfl | hy'
      · exact tieBefore_asymm w key h
      · intro hyx
        exact hbl y hy' (tieBefore_trans w key hyx h)
    · have e : ins w key x (b :: l) = b :: ins w key x l := by simp only [ins, if_neg h]
      rw [e]
      show List.Pairwise (fun a c => ¬ TieBefore w key c a) (b :: ins w key x l)
      refine List.pairwise_cons.2 ⟨?_, ih hl⟩
      intro y hy
      have hy2 : y ∈ x :: l := (ins_perm w key x l).mem_iff.1 hy
      rcases List.mem_cons.1 hy2 with rfl | hy'
      · exact h
      · exact hbl y hy'

/-- Cutting a list in D-329 order to its first `k` elements gives a top-k of it. -/
theorem isTopK_take {k : ℕ} {L : List α} (hL : SortedBest w key L) :
    IsTopK w k L (L.take k) := by
  have hP : List.Pairwise (fun a c => ¬ TieBefore w key c a) L := hL
  have hP' : List.Pairwise (fun a c => ¬ TieBefore w key c a) (L.take k ++ L.drop k) := by
    rw [List.take_append_drop]
    exact hP
  have hcross := (List.pairwise_append.1 hP').2.2
  have hsub' : (L : Multiset α) - ((L.take k : List α) : Multiset α) =
      ((L.drop k : List α) : Multiset α) := by
    have e : (L : Multiset α) =
        ((L.take k : List α) : Multiset α) + ((L.drop k : List α) : Multiset α) := by
      rw [← Multiset.coe_add, List.take_append_drop]
    rw [e, add_tsub_cancel_left]
  unfold IsTopK
  refine ⟨?_, ?_, ?_, ?_⟩
  · exact Multiset.coe_le.2 (List.take_sublist k L).subperm
  · simp [List.length_take]
  · show List.Pairwise (· ≥ ·) ((L.take k).map w)
    rw [List.pairwise_map]
    exact List.Pairwise.imp (fun {a c} h => not_lt.1 (fun hlt => h (Or.inl hlt)))
      (List.Pairwise.sublist (List.take_sublist k L) hP)
  · intro a ha b hb
    rw [hsub'] at ha
    have ha' : a ∈ L.drop k := Multiset.mem_coe.1 ha
    exact not_lt.1 (fun hlt => hcross b hb a ha' (Or.inl hlt))

/-- A permutation of the input has the same top-k. -/
theorem isTopK_perm {k : ℕ} {xs ys r : List α} (h : IsTopK w k xs r)
    (hp : List.Perm xs ys) : IsTopK w k ys r := by
  obtain ⟨hsub, hlen, hsorted, hdrop⟩ := h
  have hM : (xs : Multiset α) = (ys : Multiset α) := Multiset.coe_eq_coe.2 hp
  unfold IsTopK
  refine ⟨?_, ?_, hsorted, ?_⟩
  · rw [← hM]
    exact hsub
  · rw [hlen, hp.length_eq]
  · rw [← hM]
    exact hdrop

/-- One push keeps the invariant: a top-k of the prefix, held in D-329 order. -/
theorem pushStep_inv {k : ℕ} {P buf : List α} {x : α} (hT : IsTopK w k P buf)
    (hO : SortedBest w key buf) :
    IsTopK w k (P ++ [x]) (pushStep w key k buf x) ∧ SortedBest w key (pushStep w key k buf x) := by
  rcases Nat.eq_zero_or_pos k with rfl | hk
  · have e : pushStep w key 0 buf x = [] := by simp [pushStep]
    rw [e]
    refine ⟨?_, ?_⟩
    · unfold IsTopK
      exact ⟨by simp, by simp, by simp [List.Sorted], by simp⟩
    · simp [SortedBest]
  · have hI : SortedBest w key (ins w key x buf) := ins_sorted w key x buf hO
    have hTake : IsTopK w k (ins w key x buf) ((ins w key x buf).take k) :=
      isTopK_take w key hI
    have hPerm : List.Perm (ins w key x buf) (buf ++ [x]) :=
      (ins_perm w key x buf).trans (List.perm_append_singleton x buf).symm
    have hTopBuf : IsTopK w k (buf ++ [x]) ((ins w key x buf).take k) :=
      isTopK_perm w hTake hPerm
    have hx1 : IsTopK w k [x] [x] := by
      unfold IsTopK
      refine ⟨le_refl _, ?_, by simp [List.Sorted], by simp⟩
      rw [List.length_singleton]
      exact (Nat.min_eq_right (Nat.succ_le_of_lt hk)).symm
    have hm := isTopK_merge w hT hx1 hTopBuf
    refine ⟨hm, ?_⟩
    exact List.Pairwise.sublist (List.take_sublist k _) hI

/-- Pushing a list keeps the invariant: a top-k of the prefix seen so far. -/
theorem topKModel_inv {k : ℕ} (xs : List α) :
    ∀ (P buf : List α), IsTopK w k P buf → SortedBest w key buf →
      IsTopK w k (P ++ xs) (xs.foldl (pushStep w key k) buf) ∧
        SortedBest w key (xs.foldl (pushStep w key k) buf) := by
  induction xs with
  | nil =>
    intro P buf hT hO
    exact ⟨by simpa using hT, by simpa using hO⟩
  | cons x xs ih =>
    intro P buf hT hO
    obtain ⟨hT', hO'⟩ := pushStep_inv w key (x := x) hT hO
    have := ih (P ++ [x]) (pushStep w key k buf x) hT' hO'
    simpa [List.foldl_cons, List.append_assoc] using this

/-- The keystone (P3-2, T2): the model of `TopKFilter` returns a top-k of its input, under
the D-329 tie-break. Stated over a generic linear order of weights; the f32 instance needs the
NaN-free subtype (R1, D-328). -/
theorem topKFilter_correct (k : ℕ) (xs : List α) :
    IsTopK w k xs (topKModel w key k xs) := by
  have hT0 : IsTopK w k [] [] := by
    unfold IsTopK
    exact ⟨by simp, by simp, by simp [List.Sorted], by simp⟩
  have hO0 : SortedBest w key ([] : List α) := by simp [SortedBest]
  have h := topKModel_inv w key (k := k) xs [] [] hT0 hO0
  unfold topKModel
  simpa using h.1

end Gorse.Spec.TopK
