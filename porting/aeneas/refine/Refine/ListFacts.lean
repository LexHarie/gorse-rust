import Gorse.Spec.TopKRegion

/-!
# List facts for the heap proofs (P3-2 refinement, D-331)

`swapL d l i j` is the list `l` with the entries at `i` and `j` exchanged, which is what
`core::slice::swap` does to the heap array. The lemmas here give its lookups and show it is a
permutation, so the multiset of elements is kept.
-/

namespace Refine

/-- The list `l` with the entries at `i` and `j` exchanged. `d` is read for out-of-range
indices, which the uses below never reach. -/
def swapL {α : Type} (d : α) (l : List α) (i j : Nat) : List α :=
  (l.set i (l.getD j d)).set j (l.getD i d)

/-- Moving `a` from the middle of `u` to its head and `b` in its place is a permutation. -/
theorem head_swap_perm {α : Type} (b a : α) (u : List α) :
    ∀ (j : Nat), u[j]? = some a → List.Perm (b :: u) (a :: u.set j b) := by
  induction u with
  | nil =>
    intro j h
    simp at h
  | cons x v ih =>
    intro j h
    cases j with
    | zero =>
      simp only [List.getElem?_cons_zero, Option.some.injEq] at h
      subst h
      show List.Perm (b :: x :: v) (x :: b :: v)
      exact List.Perm.swap x b v
    | succ j' =>
      simp only [List.getElem?_cons_succ] at h
      have ih' := ih j' h
      show List.Perm (b :: x :: v) (a :: x :: v.set j' b)
      exact (List.Perm.swap x b v).trans
        ((List.Perm.cons x ih').trans (List.Perm.swap a x (v.set j' b)))

/-- Exchanging two entries, the first before the second, permutes the list. -/
theorem swapL_perm {α : Type} (d : α) (l : List α) :
    ∀ (i j : Nat), i < j → j < l.length → List.Perm (swapL d l i j) l := by
  induction l with
  | nil =>
    intro i j _ hj
    simp at hj
  | cons x t ih =>
    intro i j hij hj
    cases i with
    | zero =>
      cases j with
      | zero => omega
      | succ j' =>
        have ht : j' < t.length := by
          simp only [List.length_cons] at hj
          omega
        have hb : t[j']? = some (t.getD j' d) := by
          rw [List.getD_eq_getElem?_getD, List.getElem?_eq_getElem ht]
          simp
        have e : swapL d (x :: t) 0 (j' + 1) = (t.getD j' d) :: t.set j' x := by
          simp [swapL, List.getD_cons_succ, List.getD_cons_zero]
        rw [e]
        exact (head_swap_perm x (t.getD j' d) t j' hb).symm
    | succ i' =>
      cases j with
      | zero => omega
      | succ j' =>
        have hij' : i' < j' := by omega
        have ht : j' < t.length := by
          simp only [List.length_cons] at hj
          omega
        have e : swapL d (x :: t) (i' + 1) (j' + 1) = x :: swapL d t i' j' := by
          simp [swapL, List.getD_cons_succ, List.set_cons_succ]
        rw [e]
        exact (ih i' j' hij' ht).cons x

theorem swapL_length {α : Type} (d : α) (l : List α) (i j : Nat) :
    (swapL d l i j).length = l.length := by
  simp [swapL]

/-- The lookups of an exchange, for two in-range indices. -/
theorem swapL_getElem? {α : Type} (d : α) (l : List α) (i j : Nat)
    (hi : i < l.length) (hj : j < l.length) (k : Nat) :
    (swapL d l i j)[k]? = if k = i then l[j]? else if k = j then l[i]? else l[k]? := by
  have hi' : l[i]? = some (l[i]) := List.getElem?_eq_getElem hi
  have hj' : l[j]? = some (l[j]) := List.getElem?_eq_getElem hj
  unfold swapL
  simp only [List.getD_eq_getElem?_getD, List.getElem?_set, hi, hj, hi', hj']
  split_ifs <;> simp_all

end Refine
