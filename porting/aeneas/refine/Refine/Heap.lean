import Refine.Order
import Refine.ListFacts

/-!
# The heap invariants (P3-2 refinement, D-331)

`TopKFilter` keeps its elements in a binary heap whose root is the worst element held. A
heap is in order when no parent is before one of its children in D-329 order (`HeapOrdN`).
`up` and `down` restore that order after an element is pushed to the end or moved to the
root. Each has an invariant with one exception, the pair the sift is working on.

Everything here is about lists of elements and their `Option` lookups. The loops are proved
against these lemmas in `Refine.Loops`.
-/

open Gorse.Spec.TopK

namespace Refine

/-- The heap order over the first `n` positions: no parent is before one of its children. -/
def HeapOrdN (xs : List E) (n : Nat) : Prop :=
  ∀ c a q, 0 < c → c < n → xs[c]? = some a → xs[(c - 1) / 2]? = some q → G q a

/-- The invariant of `up` at position `j`: every parent-child pair is in order except the pair
that ends at `j`, and the children of `j` are in order with `j`'s parent. -/
def UpInv (xs : List E) (j : Nat) : Prop :=
  (∀ c a q, 0 < c → c ≠ j → xs[c]? = some a → xs[(c - 1) / 2]? = some q → G q a) ∧
  (∀ c a g, 0 < j → 0 < c → (c - 1) / 2 = j → xs[c]? = some a →
      xs[(j - 1) / 2]? = some g → G g a)

/-- The invariant of `down` at position `i`, over the first `n` positions: every pair is in
order except the pairs whose parent is `i`, and the children of `i` are in order with `i`'s
parent. -/
def DownInv (xs : List E) (n i : Nat) : Prop :=
  (∀ c a q, 0 < c → c < n → (c - 1) / 2 ≠ i → xs[c]? = some a → xs[(c - 1) / 2]? = some q →
      G q a) ∧
  (∀ c a g, 0 < i → 0 < c → c < n → (c - 1) / 2 = i → xs[c]? = some a →
      xs[(i - 1) / 2]? = some g → G g a)

/-! ## Sifting up -/

/-- When `up` stops, the heap is in order. -/
theorem upInv_exit {xs : List E} {j n : Nat} (hI : UpInv xs j)
    (hexit : j = 0 ∨ ∀ z x, xs[(j - 1) / 2]? = some z → xs[j]? = some x → G z x) :
    HeapOrdN xs n := by
  intro c a q hc _ ha hq
  by_cases hcj : c = j
  · rcases hexit with h0 | hG
    · omega
    · rw [hcj] at hq ha
      exact hG q a hq ha
  · exact hI.1 c a q hc hcj ha hq

/-- A sift-up step. The element at `j` is before its parent `p`, so the two are exchanged and
the invariant moves up to `p`. -/
theorem upInv_swap {xs : List E} {j p : Nat} (hpj : (j - 1) / 2 = p) (hj : 0 < j)
    (hjl : j < xs.length) (hI : UpInv xs j) {z x : E}
    (hz : xs[p]? = some z) (hx : xs[j]? = some x) (hw : TieBefore wv kv z x) :
    UpInv (swapL default xs p j) p := by
  have hp : p < j := by omega
  have hpl : p < xs.length := by omega
  have hget : ∀ k, (swapL (default : E) xs p j)[k]? =
      if k = p then xs[j]? else if k = j then xs[p]? else xs[k]? :=
    fun k => swapL_getElem? default xs p j hpl hjl k
  have hP : (swapL (default : E) xs p j)[p]? = some x := by
    rw [hget p]
    simp [hx]
  have hJ : (swapL (default : E) xs p j)[j]? = some z := by
    rw [hget j]
    simp [hz, hp.ne']
  have hO : ∀ k, k ≠ p → k ≠ j → (swapL (default : E) xs p j)[k]? = xs[k]? := by
    intro k hkp hkj
    rw [hget k]
    simp [hkp, hkj]
  obtain ⟨hI1, hI2⟩ := hI
  constructor
  · intro c a q hc hcp ha hq
    by_cases hcj : c = j
    · -- The pair that ends at `j`: its parent is now `x`, and `j` holds `z`.
      rw [hcj, hJ] at ha
      have hza : z = a := Option.some.inj ha
      rw [hcj, hpj, hP] at hq
      have hxq : x = q := Option.some.inj hq
      rw [← hza, ← hxq]
      exact tieBefore_asymm wv kv hw
    · rw [hO c hcp hcj] at ha
      by_cases hpc : (c - 1) / 2 = p
      · -- A sibling of `j`: its parent is now `x`.
        rw [hpc, hP] at hq
        have hxq : x = q := Option.some.inj hq
        rw [← hxq]
        intro hxa
        have hzc : xs[(c - 1) / 2]? = some z := by
          rw [hpc]
          exact hz
        exact hI1 c a z hc hcj ha hzc (tieBefore_trans wv kv hw hxa)
      · by_cases hjc : (c - 1) / 2 = j
        · -- A child of `j`: its parent is now `z`.
          rw [hjc, hJ] at hq
          have hzq : z = q := Option.some.inj hq
          rw [← hzq]
          exact hI2 c a z hj hc hjc ha (by rw [hpj]; exact hz)
        · rw [hO ((c - 1) / 2) hpc hjc] at hq
          exact hI1 c a q hc hcj ha hq
  · intro c a g hp0 hc hcp ha hg
    have hcp' : c ≠ p := by omega
    have hpp : (p - 1) / 2 ≠ p := by omega
    have hpj' : (p - 1) / 2 ≠ j := by omega
    rw [hO ((p - 1) / 2) hpp hpj'] at hg
    by_cases hcj : c = j
    · -- The child `j` holds `z`, and `z`'s parent is `g`.
      rw [hcj, hJ] at ha
      have hza : z = a := Option.some.inj ha
      rw [← hza]
      exact hI1 p z g hp0 (by omega) hz hg
    · rw [hO c hcp' hcj] at ha
      have hzc : xs[(c - 1) / 2]? = some z := by
        rw [hcp]
        exact hz
      exact G_trans (hI1 p z g hp0 (by omega) hz hg) (hI1 c a z hc hcj ha hzc)

/-! ## Sifting down -/

/-- A sift-down step. `j` is the worse child of `i`, and `x` at `i` is before its element `w`.
The two are exchanged, and the invariant moves down to `j`. -/
theorem downInv_swap {xs : List E} {n i j : Nat} (hI : DownInv xs n i)
    (hij : j = 2 * i + 1 ∨ j = 2 * i + 2) (hjn : j < n) (hjl : j < xs.length) {x w : E}
    (hx : xs[i]? = some x) (hw : xs[j]? = some w) (hxw : TieBefore wv kv x w)
    (hworst : ∀ c a, (c = 2 * i + 1 ∨ c = 2 * i + 2) → c < n → c ≠ j →
      xs[c]? = some a → G w a) :
    DownInv (swapL default xs i j) n j := by
  have hij' : i < j := by omega
  have hil : i < xs.length := by omega
  have hget : ∀ k, (swapL (default : E) xs i j)[k]? =
      if k = i then xs[j]? else if k = j then xs[i]? else xs[k]? :=
    fun k => swapL_getElem? default xs i j hil hjl k
  have hI' : (swapL (default : E) xs i j)[i]? = some w := by
    rw [hget i]
    simp [hw]
  have hJ' : (swapL (default : E) xs i j)[j]? = some x := by
    rw [hget j]
    simp [hx, hij'.ne']
  have hO : ∀ k, k ≠ i → k ≠ j → (swapL (default : E) xs i j)[k]? = xs[k]? := by
    intro k hki hkj
    rw [hget k]
    simp [hki, hkj]
  obtain ⟨hI1, hI2⟩ := hI
  constructor
  · intro c a q hc hcn hcp ha hq
    by_cases hcj : c = j
    · -- The pair `(i, j)`: `w` is now the parent of `x`.
      have hpi : (j - 1) / 2 = i := by omega
      rw [hcj, hJ'] at ha
      have hxa : x = a := Option.some.inj ha
      rw [hcj, hpi, hI'] at hq
      have hwq : w = q := Option.some.inj hq
      rw [← hxa, ← hwq]
      exact tieBefore_asymm wv kv hxw
    · by_cases hci : c = i
      · -- The child `i` holds `w`, and its parent is the parent of `i`, which `j` also sees.
        have hg1 : (i - 1) / 2 ≠ i := by omega
        have hg2 : (i - 1) / 2 ≠ j := by omega
        rw [hci, hI'] at ha
        have hwa : w = a := Option.some.inj ha
        rw [hci, hO ((i - 1) / 2) hg1 hg2] at hq
        have hqg : xs[(i - 1) / 2]? = some q := hq
        rw [← hwa]
        exact hI2 j w q (by omega) (by omega) hjn (by omega) hw hqg
      · rw [hO c hci hcj] at ha
        by_cases hpi : (c - 1) / 2 = i
        · -- A child of `i` other than `j`: its parent now holds `w`.
          rw [hpi, hI'] at hq
          have hwq : w = q := Option.some.inj hq
          rw [← hwq]
          have hc' : c = 2 * i + 1 ∨ c = 2 * i + 2 := by omega
          exact hworst c a hc' hcn hcj ha
        · rw [hO ((c - 1) / 2) hpi hcp] at hq
          exact hI1 c a q hc hcn hpi ha hq
  · intro c a g hj0 hc hcn hcpj ha hg
    have hpi : (j - 1) / 2 = i := by omega
    rw [hpi, hI'] at hg
    have hwg : w = g := Option.some.inj hg
    rw [← hwg]
    have hci : c ≠ i := by omega
    have hcj : c ≠ j := by omega
    rw [hO c hci hcj] at ha
    exact hI1 c a w hc hcn (by omega) ha (by rw [hcpj]; exact hw)

/-- When `down` stops, the heap is in order: nothing is before a child of `i` unless the
element at `i` is, and the children are not before it. -/
theorem downInv_exit {xs : List E} {n i : Nat} (hI : DownInv xs n i) {x : E}
    (hx : xs[i]? = some x)
    (hch : ∀ c a, (c = 2 * i + 1 ∨ c = 2 * i + 2) → c < n → xs[c]? = some a → G x a) :
    HeapOrdN xs n := by
  intro c a q hc hcn ha hq
  by_cases hpi : (c - 1) / 2 = i
  · rw [hpi, hx] at hq
    have hxq : x = q := Option.some.inj hq
    rw [← hxq]
    exact hch c a (by omega) hcn ha
  · exact hI.1 c a q hc hcn hpi ha hq

/-- The worse child of `i` (the one `down` moves into `i`) is not before the other child. -/
theorem down_choice_worst {xs : List E} {n i j : Nat} {a1 a2 : E}
    (h1 : xs[2 * i + 1]? = some a1) (h2 : 2 * i + 2 < n → xs[2 * i + 2]? = some a2)
    (hj : (2 * i + 2 < n ∧ TieBefore wv kv a1 a2 ∧ j = 2 * i + 2) ∨
          (¬ (2 * i + 2 < n ∧ TieBefore wv kv a1 a2) ∧ j = 2 * i + 1))
    {w : E} (hw : xs[j]? = some w) :
    ∀ c a, (c = 2 * i + 1 ∨ c = 2 * i + 2) → c < n → c ≠ j → xs[c]? = some a → G w a := by
  intro c a hc hcn hcj ha
  rcases hj with ⟨hn2, hT, hjj⟩ | ⟨hnT, hjj⟩
  · rw [hjj] at hw
    rw [h2 hn2] at hw
    have hw2 : a2 = w := Option.some.inj hw
    have hc1 : c = 2 * i + 1 := by omega
    rw [hc1, h1] at ha
    have haa : a1 = a := Option.some.inj ha
    rw [← hw2, ← haa]
    exact tieBefore_asymm wv kv hT
  · rw [hjj, h1] at hw
    have hw1 : a1 = w := Option.some.inj hw
    have hc2 : c = 2 * i + 2 := by omega
    rw [hc2] at ha hcn
    rw [h2 hcn] at ha
    have haa : a2 = a := Option.some.inj ha
    rw [← hw1, ← haa]
    exact fun hT => hnT ⟨hcn, hT⟩

/-! ## The root of a heap -/

/-- The root of a heap in order is not before any element of the heap. -/
theorem root_worst {xs : List E} {n : Nat} (hH : HeapOrdN xs n) {r : E}
    (hr : xs[0]? = some r) :
    ∀ c a, c < n → xs[c]? = some a → G r a := by
  intro c
  induction c using Nat.strong_induction_on with
  | _ c ih =>
    intro a hcn ha
    rcases Nat.eq_zero_or_pos c with h0 | hc
    · subst h0
      rw [hr] at ha
      have hra : r = a := Option.some.inj ha
      rw [← hra]
      exact tb_irrefl r
    · have hlen : c < xs.length := by
        by_contra hcon
        rw [List.getElem?_eq_none (by omega)] at ha
        simp at ha
      have hpl : (c - 1) / 2 < xs.length := by omega
      obtain ⟨q, hq⟩ : ∃ q, xs[(c - 1) / 2]? = some q :=
        ⟨xs[(c - 1) / 2], List.getElem?_eq_getElem hpl⟩
      exact G_trans (ih ((c - 1) / 2) (by omega) q (by omega) hq) (hH c a q hc hcn ha hq)

/-- The last element of a sorted list is its worst element: a root that is not before any
element of the list is the last one. -/
theorem root_last {L : List E} {r : E} (hL : SortedBest wv kv L) (hr : r ∈ L)
    (hw : ∀ y ∈ L, ¬ TieBefore wv kv r y) : ∃ A, L = A ++ [r] := by
  have hne : L ≠ [] := List.ne_nil_of_mem hr
  have hsplit : L = L.dropLast ++ [L.getLast hne] := (List.dropLast_append_getLast hne).symm
  have hP : List.Pairwise (fun a c => ¬ TieBefore wv kv c a)
      (L.dropLast ++ [L.getLast hne]) := by
    rw [← hsplit]
    exact hL
  have hzr : L.getLast hne = r := by
    have hr' : r ∈ L.dropLast ++ [L.getLast hne] := by
      rw [← hsplit]
      exact hr
    rcases List.mem_append.1 hr' with hA | hz
    · have h1 : ¬ TieBefore wv kv (L.getLast hne) r :=
        (List.pairwise_append.1 hP).2.2 r hA (L.getLast hne) (List.mem_singleton_self _)
      have h2 : ¬ TieBefore wv kv r (L.getLast hne) := hw _ (List.getLast_mem hne)
      by_contra hc
      rcases tb_total (Ne.symm hc) with h | h
      · exact h2 h
      · exact h1 h
    · exact (List.mem_singleton.1 hz).symm
  refine ⟨L.dropLast, ?_⟩
  rw [hzr] at hsplit
  exact hsplit

end Refine
