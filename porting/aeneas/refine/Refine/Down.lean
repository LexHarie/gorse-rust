import Refine.Loops

/-!
# The sift-down loop of `TopKFilter` (P3-2 refinement, D-331)

`down` moves the element at `i` into its place among the first `n` positions. Each step
exchanges it with its worse child while that child is before it in the heap order. `pop_worst`
calls `down` once, after it has moved the last element to the root.

The loop is proved against `DownInv` of `Refine.Heap`. Its body is first evaluated to an
equation for each of the three outcomes: no child, a stop with a child, and a swap with a child.
-/

open Aeneas Aeneas.Std Result ControlFlow Error
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

/-! ## Usize values -/

/-- A `usize` with a given value below `2^59`. -/
theorem usize_of_val (k : Nat) (h : k < 2 ^ 59) : ∃ x : Usize, x.val = k :=
  ⟨⟨k, by scalar_tac⟩, rfl⟩

/-! ## List facts for a sift-down step -/

/-- With no child in the first `n` positions, the heap is in order. -/
theorem downInv_nochild {xs : List E} {n i : Nat} (hI : DownInv xs n i) (hn : n ≤ 2 * i + 1) :
    HeapOrdN xs n := by
  intro c a q hc hcn ha hq
  by_cases hpi : (c - 1) / 2 = i
  · exfalso
    omega
  · exact hI.1 c a q hc hcn hpi ha hq

/-- If `x` is not before `w`, and `w` is not before `a`, then `x` is not before `a`. -/
theorem not_before_of_worst {x w a : E} (hxw : ¬ TieBefore wv kv x w)
    (hwa : ¬ TieBefore wv kv w a) : ¬ TieBefore wv kv x a := by
  intro hxa
  by_cases hae : a = w
  · subst hae
    exact hxw hxa
  · have haw : TieBefore wv kv a w := by
      rcases tb_total hae with h | h
      · exact h
      · exact absurd h hwa
    exact hxw (tieBefore_trans wv kv hxa haw)

/-- When `down` stops at `i` with a child `j`, and `x` at `i` is not before the child `w`, the
heap is in order. -/
theorem downInv_stop_child {xs : List E} {n i j : Nat} {x w : E} (hI : DownInv xs n i)
    (hx : xs[i]? = some x) (hj : j = 2 * i + 1 ∨ j = 2 * i + 2) (hjn : j < n)
    (hw : xs[j]? = some w) (hxw : ¬ TieBefore wv kv x w)
    (hworst : ∀ c a, (c = 2 * i + 1 ∨ c = 2 * i + 2) → c < n → c ≠ j →
      xs[c]? = some a → G w a) :
    HeapOrdN xs n := by
  apply downInv_exit hI hx
  intro c a hc hcn ha
  by_cases hcj : c = j
  · subst hcj
    rw [hw] at ha
    have hwa : w = a := Option.some.inj ha
    rw [← hwa]
    exact hxw
  · exact not_before_of_worst hxw (hworst c a hc hcn hcj ha)

/-! ## The body of `down` -/

/-- `down`'s loop body stops when there is no child in the first `n` positions. -/
theorem down_body_nochild (n : Usize) (s : heap.TopKFilter Std.U32 Std.I64) (i : Usize)
    (hn : n.val ≤ 2 * i.val + 1) (hi : i.val < 2 ^ 59) :
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

/-- With a child in range, `down`'s loop body is the comparison of `i` with the chosen child `j`,
followed by a stop or a swap. `j` is `2i+2` when that child is in range and before `2i+1`, and
`2i+1` otherwise. -/
theorem down_body_eq (s : heap.TopKFilter Std.U32 Std.I64) (n i j : Usize)
    (hi : i.val < 2 ^ 59) (hL : s.elems.val.length < 2 ^ 59)
    (hn : n.val ≤ s.elems.val.length) (hlt : 2 * i.val + 1 < n.val)
    {a1 a2 : E} (h1 : s.elems.val[2 * i.val + 1]? = some a1)
    (h2 : 2 * i.val + 2 < n.val → s.elems.val[2 * i.val + 2]? = some a2)
    (hj : (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2 ∧ j.val = 2 * i.val + 2) ∨
          (¬ (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2) ∧ j.val = 2 * i.val + 1)) :
    heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n s i =
      (do
        let b ← heap.TopKFilter.worse_at core.cmp.OrdU32 core.cmp.PartialOrdI64 s j i
        if b then
          (do
            let (sl, dmb) ← lift (alloc.vec.Vec.deref_mut s.elems)
            let s1 ← core.slice.Slice.swap sl i j
            ok (cont ({ s with elems := dmb s1 }, j)))
        else ok (done s)) := by
  unfold heap.TopKFilter.down_loop.body
  obtain ⟨i1, hi1, hi1v⟩ := usize_mul2_eq i (by scalar_tac)
  rw [hi1]
  simp only [bind_ok]
  obtain ⟨j1, hj1, hj1v⟩ := usize_add1_eq i1 (by scalar_tac)
  rw [hj1]
  simp only [bind_ok]
  have hnge : ¬ (j1 ≥ n) := by
    show ¬ (n.val ≤ j1.val)
    omega
  rw [if_neg hnge]
  obtain ⟨j2, hj2, hj2v⟩ := usize_add1_eq j1 (by scalar_tac)
  rw [hj2]
  simp only [bind_ok]
  have hj1p : j1.val = 2 * i.val + 1 := by omega
  have hj2p : j2.val = 2 * i.val + 2 := by omega
  by_cases hc2 : j2 < n
  · have hc2v : j2.val < n.val := hc2
    have hc2' : 2 * i.val + 2 < n.val := by omega
    rw [if_pos hc2]
    obtain ⟨e1, e2, he1, he2, hw⟩ := worse_at_eq s j2 j1 (by omega) (by omega)
    have hea2 : e1 = a2 := by
      rw [hj2p] at he1
      exact Option.some.inj (he1.symm.trans (h2 hc2'))
    have heA1 : e2 = a1 := by
      rw [hj1p] at he2
      exact Option.some.inj (he2.symm.trans h1)
    rw [hw]
    simp only [bind_ok]
    by_cases hT : TieBefore wv kv a1 a2
    · have hcond : TieBefore wv kv e2 e1 := by
        rw [heA1, hea2]
        exact hT
      rw [if_pos (decide_eq_true hcond)]
      have hjv : j.val = 2 * i.val + 2 := by
        rcases hj with ⟨_, _, h⟩ | ⟨hno, _⟩
        · exact h
        · exact absurd ⟨hc2', hT⟩ hno
      have hjj : j = j2 := UScalar.eq_of_val_eq (by rw [hjv, hj2p])
      subst hjj
      simp only [bind_ok]
    · have hcond : ¬ TieBefore wv kv e2 e1 := by
        rw [heA1, hea2]
        exact hT
      have hnb : ¬ (decide (TieBefore wv kv e2 e1) = true) := fun h => hcond (of_decide_eq_true h)
      rw [if_neg hnb]
      have hjv : j.val = 2 * i.val + 1 := by
        rcases hj with ⟨_, h, _⟩ | ⟨_, h⟩
        · exact absurd h hT
        · exact h
      have hjj : j = j1 := UScalar.eq_of_val_eq (by rw [hjv, hj1p])
      subst hjj
      simp only [bind_ok]
  · have hc2n : ¬ (2 * i.val + 2 < n.val) := by
      intro h
      apply hc2
      show j2.val < n.val
      omega
    rw [if_neg hc2]
    simp only [bind_ok]
    have hjv : j.val = 2 * i.val + 1 := by
      rcases hj with ⟨h, _, _⟩ | ⟨_, h⟩
      · exact absurd h hc2n
      · exact h
    have hjj : j = j1 := UScalar.eq_of_val_eq (by rw [hjv, hj1p])
    subst hjj
    rfl

/-- `down`'s loop body stops at `i` with a child `j` when `x` at `i` is not before the child `w`. -/
theorem down_body_stop (s : heap.TopKFilter Std.U32 Std.I64) (n i j : Usize)
    (hi : i.val < 2 ^ 59) (hL : s.elems.val.length < 2 ^ 59)
    (hn : n.val ≤ s.elems.val.length) (hlt : 2 * i.val + 1 < n.val)
    {a1 a2 : E} (h1 : s.elems.val[2 * i.val + 1]? = some a1)
    (h2 : 2 * i.val + 2 < n.val → s.elems.val[2 * i.val + 2]? = some a2)
    (hj : (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2 ∧ j.val = 2 * i.val + 2) ∨
          (¬ (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2) ∧ j.val = 2 * i.val + 1))
    {x w : E} (hx : s.elems.val[i.val]? = some x) (hw : s.elems.val[j.val]? = some w)
    (hxw : ¬ TieBefore wv kv x w) :
    heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n s i =
      ok (done s) := by
  have hjn : j.val < s.elems.val.length := by
    rcases hj with ⟨h, _, hjv⟩ | ⟨_, hjv⟩ <;> omega
  rw [down_body_eq s n i j hi hL hn hlt h1 h2 hj]
  obtain ⟨e1, e2, he1, he2, hwe⟩ := worse_at_eq s j i hjn (by omega)
  have ew : e1 = w := Option.some.inj (he1.symm.trans hw)
  have ex : e2 = x := Option.some.inj (he2.symm.trans hx)
  rw [hwe]
  simp only [bind_ok]
  have hnb : ¬ (decide (TieBefore wv kv e2 e1) = true) := by
    intro h
    apply hxw
    have hT := of_decide_eq_true h
    rw [ex, ew] at hT
    exact hT
  rw [if_neg hnb]

/-- `down`'s loop body swaps `i` with the child `j` when `x` at `i` is before the child `w`, and
returns the exchanged array with the position `j`. -/
theorem down_body_swap (s : heap.TopKFilter Std.U32 Std.I64) (n i j : Usize)
    (hi : i.val < 2 ^ 59) (hL : s.elems.val.length < 2 ^ 59)
    (hn : n.val ≤ s.elems.val.length) (hlt : 2 * i.val + 1 < n.val)
    {a1 a2 : E} (h1 : s.elems.val[2 * i.val + 1]? = some a1)
    (h2 : 2 * i.val + 2 < n.val → s.elems.val[2 * i.val + 2]? = some a2)
    (hj : (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2 ∧ j.val = 2 * i.val + 2) ∨
          (¬ (2 * i.val + 2 < n.val ∧ TieBefore wv kv a1 a2) ∧ j.val = 2 * i.val + 1))
    {x w : E} (hx : s.elems.val[i.val]? = some x) (hw : s.elems.val[j.val]? = some w)
    (hxw : TieBefore wv kv x w) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n s i =
          ok (cont (s', j)) ∧
        s'.elems.val = swapL default s.elems.val i.val j.val ∧ s'.k = s.k ∧ s'.nan = s.nan := by
  have hjn : j.val < s.elems.val.length := by
    rcases hj with ⟨h, _, hjv⟩ | ⟨_, hjv⟩ <;> omega
  rw [down_body_eq s n i j hi hL hn hlt h1 h2 hj]
  obtain ⟨e1, e2, he1, he2, hwe⟩ := worse_at_eq s j i hjn (by omega)
  have ew : e1 = w := Option.some.inj (he1.symm.trans hw)
  have ex : e2 = x := Option.some.inj (he2.symm.trans hx)
  rw [hwe]
  simp only [bind_ok]
  have hTB : TieBefore wv kv e2 e1 := by
    rw [ex, ew]
    exact hxw
  rw [if_pos (decide_eq_true hTB)]
  obtain ⟨s1, hs1, hs1v⟩ := swap_eq s.elems.slice i j
    (by show i.val < s.elems.val.length; omega) (by show j.val < s.elems.val.length; omega)
  refine ⟨{ s with elems := { slice := s1 } }, ?_, ?_, rfl, rfl⟩
  · simp only [alloc.vec.Vec.deref_mut, lift_eq, bind_ok]
    rw [hs1]
    simp only [bind_ok]
  · exact hs1v

/-! ## The loop of `down` -/

/-- The state of the `down` loop: the heap with the position it sifts from. Its array is a
permutation of the input, the first `n` positions satisfy the sift invariant, and the positions
from `n` on are unchanged. -/
abbrev DownState (n : Nat) (s : heap.TopKFilter Std.U32 Std.I64)
    (p : heap.TopKFilter Std.U32 Std.I64 × Usize) : Prop :=
  n ≤ p.1.elems.val.length ∧ p.1.elems.val.Perm s.elems.val ∧ p.1.k = s.k ∧
    p.1.nan = s.nan ∧ DownInv p.1.elems.val n p.2.val ∧ p.2.val < 2 ^ 59 ∧
    ∀ k, n ≤ k → p.1.elems.val[k]? = s.elems.val[k]?

/-- What `down` leaves behind: the first `n` positions in order, a permutation of the input with
the same `k` and `nan`, and the positions from `n` on unchanged. -/
abbrev DownPost (n : Nat) (s y : heap.TopKFilter Std.U32 Std.I64) : Prop :=
  HeapOrdN y.elems.val n ∧ y.elems.val.Perm s.elems.val ∧
    y.elems.val.length = s.elems.val.length ∧ y.k = s.k ∧ y.nan = s.nan ∧
    ∀ k, n ≤ k → y.elems.val[k]? = s.elems.val[k]?

/-- The invariant of `down` is kept while it runs, and at the end the first `n` positions are in
order. -/
theorem down_loop_eq (s : heap.TopKFilter Std.U32 Std.I64) (n i : Usize)
    (hL : s.elems.val.length < 2 ^ 59) (hn : n.val ≤ s.elems.val.length) (hi : i.val < 2 ^ 59)
    (hI : DownInv s.elems.val n.val i.val) :
    ∃ s' : heap.TopKFilter Std.U32 Std.I64,
      heap.TopKFilter.down_loop core.cmp.OrdU32 core.cmp.PartialOrdI64 s n i = ok s' ∧
        DownPost n.val s s' := by
  have hBody : ∀ p : heap.TopKFilter Std.U32 Std.I64 × Usize, DownState n.val s p →
      WP.spec (heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n p.1 p.2)
        (fun r => match r with
          | ControlFlow.done y => DownPost n.val s y
          | ControlFlow.cont x' => DownState n.val s x' ∧
              n.val - x'.2.val < n.val - p.2.val) := by
    rintro ⟨s1, i1⟩ hst
    obtain ⟨hn1, hperm, hk, hnan, hI1, hi1, hbey⟩ : n.val ≤ s1.elems.val.length ∧
        s1.elems.val.Perm s.elems.val ∧ s1.k = s.k ∧ s1.nan = s.nan ∧
          DownInv s1.elems.val n.val i1.val ∧ i1.val < 2 ^ 59 ∧
            ∀ k, n.val ≤ k → s1.elems.val[k]? = s.elems.val[k]? := hst
    show WP.spec (heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n s1 i1) _
    have hL1 : s1.elems.val.length < 2 ^ 59 := by
      have := hperm.length_eq
      omega
    by_cases hleaf : n.val ≤ 2 * i1.val + 1
    · -- No child in range: `down` stops, and the heap is in order.
      rw [down_body_nochild n s1 i1 hleaf hi1]
      refine (WP.spec_ok _).2 ?_
      show DownPost n.val s s1
      exact ⟨downInv_nochild hI1 hleaf, hperm, hperm.length_eq, hk, hnan, hbey⟩
    · have hlt : 2 * i1.val + 1 < n.val := by omega
      obtain ⟨a1, h1⟩ : ∃ a, s1.elems.val[2 * i1.val + 1]? = some a :=
        ⟨_, List.getElem?_eq_getElem (by omega)⟩
      obtain ⟨a2, h2⟩ : ∃ a2 : E, 2 * i1.val + 2 < n.val →
          s1.elems.val[2 * i1.val + 2]? = some a2 := by
        by_cases hc : 2 * i1.val + 2 < n.val
        · exact ⟨_, fun _ => List.getElem?_eq_getElem (by omega)⟩
        · exact ⟨a1, fun h => absurd h hc⟩
      -- The child `j` that `down` compares with: the worse of the two children.
      have hchoice : ∃ j : Usize,
          (2 * i1.val + 2 < n.val ∧ TieBefore wv kv a1 a2 ∧ j.val = 2 * i1.val + 2) ∨
          (¬ (2 * i1.val + 2 < n.val ∧ TieBefore wv kv a1 a2) ∧ j.val = 2 * i1.val + 1) := by
        by_cases hT : 2 * i1.val + 2 < n.val ∧ TieBefore wv kv a1 a2
        · obtain ⟨j, hjv⟩ := usize_of_val (2 * i1.val + 2) (by omega)
          exact ⟨j, Or.inl ⟨hT.1, hT.2, hjv⟩⟩
        · obtain ⟨j, hjv⟩ := usize_of_val (2 * i1.val + 1) (by omega)
          exact ⟨j, Or.inr ⟨hT, hjv⟩⟩
      obtain ⟨j, hj⟩ := hchoice
      have hjv : j.val = 2 * i1.val + 1 ∨ j.val = 2 * i1.val + 2 := by
        rcases hj with ⟨_, _, h⟩ | ⟨_, h⟩
        · exact Or.inr h
        · exact Or.inl h
      have hjn : j.val < n.val := by
        rcases hj with ⟨h, _, hjv'⟩ | ⟨_, hjv'⟩ <;> omega
      have hjl : j.val < s1.elems.val.length := by omega
      obtain ⟨x, hx⟩ : ∃ x, s1.elems.val[i1.val]? = some x :=
        ⟨_, List.getElem?_eq_getElem (by omega)⟩
      obtain ⟨w, hw⟩ : ∃ w, s1.elems.val[j.val]? = some w :=
        ⟨_, List.getElem?_eq_getElem hjl⟩
      have hworst := down_choice_worst h1 h2 hj hw
      by_cases hxw : TieBefore wv kv x w
      · -- `x` is before `w`: `down` swaps them and moves on from `j`.
        obtain ⟨s', hb, hsE, hk', hnan'⟩ :=
          down_body_swap s1 n i1 j hi1 hL1 hn1 hlt h1 h2 hj hx hw hxw
        rw [hb]
        refine (WP.spec_ok _).2 ?_
        show DownState n.val s (s', j) ∧ n.val - j.val < n.val - i1.val
        have hi1l : i1.val < s1.elems.val.length := by omega
        have hlen' : s'.elems.val.length = s1.elems.val.length := by
          rw [hsE, swapL_length]
        have hperm' : s'.elems.val.Perm s.elems.val := by
          rw [hsE]
          exact (swapL_perm (default : heap.Elem Std.U32 Std.I64) s1.elems.val i1.val j.val
            (by omega) hjl).trans hperm
        have hI' : DownInv s'.elems.val n.val j.val := by
          rw [hsE]
          exact downInv_swap hI1 hjv hjn hjl hx hw hxw hworst
        have hbey' : ∀ k, n.val ≤ k → s'.elems.val[k]? = s.elems.val[k]? := by
          intro k hk
          rw [hsE, swapL_getElem? _ _ _ _ hi1l hjl k, if_neg (show ¬ (k = i1.val) by omega),
            if_neg (show ¬ (k = j.val) by omega)]
          exact hbey k hk
        exact ⟨⟨by omega, hperm', hk'.trans hk, hnan'.trans hnan, hI', by omega, hbey'⟩, by omega⟩
      · -- `x` is not before `w`: `down` stops, and the heap is in order.
        have hcs := down_body_stop s1 n i1 j hi1 hL1 hn1 hlt h1 h2 hj hx hw hxw
        rw [hcs]
        refine (WP.spec_ok _).2 ?_
        show DownPost n.val s s1
        exact ⟨downInv_stop_child hI1 hx hjv hjn hw hxw hworst, hperm, hperm.length_eq, hk,
          hnan, hbey⟩
  have hInv : DownState n.val s (s, i) :=
    ⟨hn, List.Perm.refl _, rfl, rfl, hI, hi, fun k _ => rfl⟩
  have hspec : WP.spec
      (loop (fun q => heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n
        q.1 q.2) (s, i))
      (DownPost n.val s) :=
    loop.spec_decr_nat (fun p => n.val - p.2.val) (DownState n.val s) (DownPost n.val s)
      (fun q => heap.TopKFilter.down_loop.body core.cmp.OrdU32 core.cmp.PartialOrdI64 n q.1 q.2)
      (s, i) hBody hInv
  obtain ⟨s', hs', hP⟩ := exists_ok_of_spec hspec
  exact ⟨s', by unfold heap.TopKFilter.down_loop; exact hs', hP⟩

end Refine
