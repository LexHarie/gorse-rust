import Mathlib

/-!
# One record per (user, item) pair in the training data (audit S3, D-366)

The DB key for feedback is (feedback type, user, item), so two positive types on the same pair
are two rows. The loader used to add every row to the dataset, so a split could hold out a pair
that training still saw (`crates/gorse-dataset/src/dataset.rs`, `split_user_feedback`). The fix
keeps one record per pair: a repeated pair updates the existing record's timestamp instead of
adding a second record.

This file models the load (a fold over the records), the bug (`preSplit_leaks`), the fixed
load (`loadAll`), and the two facts that make the fix work: a loaded list has no repeats
(`loadAll_nodup`), and a split of a list with no repeats holds out a record that is absent from
the train set (`split_test_not_in_train`).

The order of the records is not modelled. `split_test_not_in_train` holds for every order, so the
result does not depend on it.
-/

namespace Gorse.Spec.LoadPairs

/-- The pre-fix load: every record is kept, repeats included. -/
def preLoad {α : Type} (rs : List α) : List α :=
  rs

/-- Adds a record only when the pair is absent. This is the rule of the fixed loader. -/
def addOnce {α : Type} [DecidableEq α] (xs : List α) (p : α) : List α :=
  if p ∈ xs then xs else p :: xs

/-- The fixed load: fold `addOnce` over the records. -/
def loadAll {α : Type} [DecidableEq α] (rs : List α) : List α :=
  rs.foldl addOnce []

/-- **Bug (S3).** The records `[(0,0),(0,0)]` with `k = 0`: the held-out record is `(0,0)`, and
the train set, which keeps the other record, still contains `(0,0)`. -/
theorem preSplit_leaks :
    (preLoad ([((0 : ℕ), (0 : ℕ)), (0, 0)] : List (ℕ × ℕ)))[0]? = some (0, 0) ∧
      (0, 0) ∈ (preLoad ([((0 : ℕ), (0 : ℕ)), (0, 0)] : List (ℕ × ℕ))).eraseIdx 0 := by
  unfold preLoad
  constructor
  · simp
  · simp

/-- **Fix (S3).** The fixed load of the same records keeps one record, so the split has no train
copy of the held-out pair. -/
theorem loaded_no_leak_example :
    (0, 0) ∉ (loadAll ([((0 : ℕ), (0 : ℕ)), (0, 0)] : List (ℕ × ℕ))).eraseIdx 0 := by
  decide

/-- A record that is absent is added, and the list stays free of repeats. -/
theorem addOnce_nodup {α : Type} [DecidableEq α] {xs : List α} (p : α) (h : xs.Nodup) :
    (addOnce xs p).Nodup := by
  unfold addOnce
  split_ifs with hp
  · exact h
  · exact List.nodup_cons.mpr ⟨hp, h⟩

/-- Folding `addOnce` over any records, from a list with no repeats, gives a list with no
repeats. -/
theorem foldl_addOnce_nodup {α : Type} [DecidableEq α] (rs : List α) :
    ∀ (acc : List α), acc.Nodup → (rs.foldl addOnce acc).Nodup := by
  induction rs with
  | nil =>
    intro acc h
    exact h
  | cons r rs ih =>
    intro acc h
    exact ih (addOnce acc r) (addOnce_nodup r h)

/-- **Fix (S3).** The fixed load has no repeats. -/
theorem loadAll_nodup {α : Type} [DecidableEq α] (rs : List α) : (loadAll rs).Nodup :=
  foldl_addOnce_nodup rs [] List.nodup_nil

/-- An element at position `k` of a list is in the list. -/
theorem pair_mem_of_getElem? {α : Type} (xs : List α) :
    ∀ (k : ℕ) (p : α), xs[k]? = some p → p ∈ xs := by
  induction xs with
  | nil =>
    intro k p h
    simp at h
  | cons x xs ih =>
    intro k p h
    cases k with
    | zero =>
      simp at h
      simp [h]
    | succ k =>
      simp only [List.getElem?_cons_succ] at h
      exact List.mem_cons.mpr (Or.inr (ih k p h))

/-- **Fix (S3).** In a list with no repeats, a split holds out `p` at position `k`, and no other
record of the train set (`xs.eraseIdx k`) is `p`. -/
theorem split_test_not_in_train {α : Type} (xs : List α) :
    ∀ (k : ℕ) (p : α), xs.Nodup → xs[k]? = some p → ∀ q ∈ xs.eraseIdx k, q ≠ p := by
  induction xs with
  | nil =>
    intro k p _ h
    simp at h
  | cons x xs ih =>
    intro k p hnd h
    cases k with
    | zero =>
      simp at h
      subst h
      intro q hq
      rw [List.eraseIdx_cons_zero] at hq
      intro hqx
      rw [hqx] at hq
      exact (List.nodup_cons.mp hnd).1 hq
    | succ k =>
      simp only [List.getElem?_cons_succ] at h
      intro q hq
      rw [List.eraseIdx_cons_succ, List.mem_cons] at hq
      rcases hq with hq | hq
      · rw [hq]
        intro hxp
        rw [← hxp] at h
        exact (List.nodup_cons.mp hnd).1 (pair_mem_of_getElem? xs k x h)
      · exact ih k p (List.nodup_cons.mp hnd).2 h q hq

/-- **Fix (S3).** After the fixed load, a split has no leak: the held-out record is not in the
train set. -/
theorem split_after_load {α : Type} [DecidableEq α] (rs : List α) (k : ℕ) (p : α)
    (h : (loadAll rs)[k]? = some p) : ∀ q ∈ (loadAll rs).eraseIdx k, q ≠ p :=
  split_test_not_in_train (loadAll rs) k p (loadAll_nodup rs) h

end Gorse.Spec.LoadPairs
