import Mathlib
-- between these markers byte for byte, into porting/aeneas/refine/Gorse/Spec/TopKRegion.lean.
namespace Gorse.Spec.TopK

variable {α W ι : Type} [DecidableEq α] [LinearOrder W] [LinearOrder ι]
  (w : α → W) (key : α → ι)

/-- The tie-break of P3-3 (D-329): `TieBefore w key a b` says that `a` comes before `b`,
a higher weight first and, between equal weights, the smaller `key` first. Go has no such
order: its equal weights come out in heap order (`common/heap/pq.go:42-48`). -/
def TieBefore (a b : α) : Prop :=
  w b < w a ∨ (w a = w b ∧ key a < key b)

/-- A list is held in D-329 order when no later element is `TieBefore` an earlier one. -/
def SortedBest (L : List α) : Prop :=
  List.Pairwise (fun a c => ¬ TieBefore w key c a) L

omit [DecidableEq α] in
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

omit [DecidableEq α] in
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

omit [DecidableEq α] in
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

omit [DecidableEq α] in
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

end Gorse.Spec.TopK
