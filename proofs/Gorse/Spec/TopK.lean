import Mathlib

/-! # The top-k specification (P3-1, T2 of the verification plan)

This file states the top-k filter of `porting/verification-plan.md` §3.1 (T2) over a
generic linear order of weights, and proves the two theorems the plan names for it:
weight uniqueness, and the merge of two top-k results. The keystone `topKFilter_correct`
(P3-2) and the link to the Rust code are later tasks.

## What the Go code does

`TopKFilter.Push` keeps the `k` heaviest elements. It pushes the element onto a heap and,
once the heap holds more than `k` elements, pops the lightest one
(`common/heap/filter.go:35-40`). The heap is a min-heap on the weight, because
`_heap.Less` compares with `<` when `desc` is false (`common/heap/pq.go:42-48`), and
`NewTopKFilter` leaves `desc` false (`filter.go:28-31`). `PopAllValues` pops from the
lightest end and fills the result from the back, so the output is in decreasing order of
weight (`filter.go:43-50`). `PopAll` does the same and keeps the weights
(`filter.go:52-59`). The source is `common/heap/` at the pinned commit add21b5.

## Decisions (PORTING.md D-329)

* Ties. Go does not define an order on equal weights: they come out in the order the heap
  leaves them. `IsTopK` allows any choice among equal weights, and that is what Go's
  output satisfies. `TieBefore` states the order the Rust port takes (P3-3): weight
  descending, then id ascending. That is a switch from Go's order on ties.
* NaN. Go's `TopKFilter.Push` accepts NaN (`filter.go:35-40` has no check), and every
  comparison with NaN is false, so where a NaN ends up depends on the push order
  (PORTING_NOTES.md Q-38). Go has no NaN result to match. The weights here come from a
  linear order `W`, so NaN is outside the spec and the spec makes no claim about it. A5 and
  P3-3 reject NaN at the boundary and count the rejections. The f32 instance then uses
  the NaN-free subtype (R1, President, 2026-11-06).
* The size. Go takes an `int`, and a negative `k` keeps nothing. The spec takes `k : ℕ`,
  as the Rust port takes `usize`, so the negative case is not in the spec.
-/

namespace Gorse.Spec.TopK

-- Multiset subtraction needs `DecidableEq α`. The port's elements are `i32` ids, which have it.
variable {α W : Type} [DecidableEq α] [LinearOrder W] (w : α → W)

/-- The tie-break of P3-3 (D-329): `TieBefore w key a b` says that `a` comes before `b`,
a higher weight first and, between equal weights, the smaller `key` first. Go has no such
order: its equal weights come out in heap order (`common/heap/pq.go:42-48`). -/
def TieBefore {ι : Type} [LinearOrder ι] (key : α → ι) (a b : α) : Prop :=
  w b < w a ∨ (w a = w b ∧ key a < key b)

/-- `IsTopK w k xs r`: `r` is a top-k of the list `xs` under the weights `w` (D-329). It
is a sub-multiset of `xs` of length `min k |xs|`, its weights are in decreasing order, and
every dropped element weighs at most every kept one. Go's `TopKFilter` output satisfies it
for NaN-free weights, whatever order the equal weights come out in
(`common/heap/filter.go:35-50`). -/
def IsTopK (k : ℕ) (xs r : List α) : Prop :=
  (r : Multiset α) ≤ (xs : Multiset α) ∧ r.length = min k xs.length ∧
    (r.map w).Sorted (· ≥ ·) ∧
    ∀ a ∈ (xs : Multiset α) - (r : Multiset α), ∀ b ∈ r, w a ≤ w b

/-- The counting lemma behind both theorems. When every dropped element weighs at most
every kept one, the kept elements that satisfy an upward-closed predicate `p` number
exactly the least of the size of `R` and the elements of `X` that satisfy `p`. -/
theorem countP_kept {X R : Multiset α} (hR : R ≤ X)
    (hdrop : ∀ d ∈ X - R, ∀ b ∈ R, w d ≤ w b) {p : α → Prop} [DecidablePred p]
    (hp : ∀ a b : α, p a → w a ≤ w b → p b) :
    R.countP p = min (Multiset.card R) (X.countP p) := by
  have hsplit : X.countP p = R.countP p + (X - R).countP p := by
    rw [← Multiset.countP_add, add_tsub_cancel_of_le hR]
  have hle : R.countP p ≤ Multiset.card R := by
    apply Multiset.countP_le_card
  by_cases h : ∃ d ∈ X - R, p d
  · obtain ⟨d, hd, hpd⟩ := h
    have hall : R.countP p = Multiset.card R :=
      Multiset.countP_eq_card.2 (fun b hb => hp d b hpd (hdrop d hd b hb))
    have hX : Multiset.card R ≤ X.countP p := by omega
    rw [hall, min_eq_left hX]
  · have hzero : (X - R).countP p = 0 := by
      rw [Multiset.countP_eq_zero]
      intro d hd hpd
      exact h ⟨d, hd, hpd⟩
    rw [hzero, add_zero] at hsplit
    rw [hsplit, min_eq_right hle]

/-- Counting the elements of `M` by their weight at `v`: those equal to `v` and those
strictly above it make up those at least `v`. -/
theorem countP_eq_add_countP_lt (M : Multiset α) (v : W) :
    M.countP (fun x => v = w x) + M.countP (fun x => v < w x) =
      M.countP (fun x => v ≤ w x) := by
  induction M using Multiset.induction_on with
  | empty => simp
  | cons a M ih =>
    simp only [Multiset.countP_cons]
    rcases lt_trichotomy v (w a) with h | h | h
    · rw [if_neg h.ne, if_pos h, if_pos h.le]
      omega
    · have h2 : ¬ v < w a := by
        rw [h]
        exact lt_irrefl _
      rw [if_pos h, if_neg h2, if_pos h.le]
      omega
    · rw [if_neg h.ne', if_neg (not_lt.2 h.le), if_neg (not_le.2 h)]
      omega

/-- The number of copies of `v` among the weights of `M`, counted by elements. -/
theorem count_map_eq_countP (M : Multiset α) (v : W) :
    (M.map w).count v = M.countP (fun x => v = w x) := by
  induction M using Multiset.induction_on with
  | empty => simp
  | cons a M ih =>
    simp only [Multiset.map_cons, Multiset.count_cons, Multiset.countP_cons, ih]

/-- Weight uniqueness: the weights of a top-k of `xs` do not depend on which top-k it is,
so the weight list of a top-k is determined by `xs` and `k`. -/
theorem isTopK_weights_unique {k : ℕ} {xs r r' : List α}
    (h : IsTopK w k xs r) (h' : IsTopK w k xs r') : r.map w = r'.map w := by
  obtain ⟨hsub, hlen, hsorted, hdrop⟩ := h
  obtain ⟨hsub', hlen', hsorted', hdrop'⟩ := h'
  have hcard : Multiset.card (r : Multiset α) = Multiset.card (r' : Multiset α) := by
    simp [hlen, hlen']
  have hdrop_m : ∀ d ∈ (xs : Multiset α) - (r : Multiset α),
      ∀ b ∈ (r : Multiset α), w d ≤ w b :=
    fun d hd b hb => hdrop d hd b (Multiset.mem_coe.1 hb)
  have hdrop_m' : ∀ d ∈ (xs : Multiset α) - (r' : Multiset α),
      ∀ b ∈ (r' : Multiset α), w d ≤ w b :=
    fun d hd b hb => hdrop' d hd b (Multiset.mem_coe.1 hb)
  have hle : ∀ v : W, (r : Multiset α).countP (fun x => v ≤ w x) =
      (r' : Multiset α).countP (fun x => v ≤ w x) := by
    intro v
    rw [countP_kept w hsub hdrop_m (p := fun x => v ≤ w x) (fun _ _ ha hab => le_trans ha hab),
      countP_kept w hsub' hdrop_m' (p := fun x => v ≤ w x) (fun _ _ ha hab => le_trans ha hab),
      hcard]
  have hlt : ∀ v : W, (r : Multiset α).countP (fun x => v < w x) =
      (r' : Multiset α).countP (fun x => v < w x) := by
    intro v
    rw [countP_kept w hsub hdrop_m (p := fun x => v < w x)
        (fun _ _ ha hab => lt_of_lt_of_le ha hab),
      countP_kept w hsub' hdrop_m' (p := fun x => v < w x)
        (fun _ _ ha hab => lt_of_lt_of_le ha hab),
      hcard]
  have hcount : ∀ v : W, ((r : Multiset α).map w).count v =
      ((r' : Multiset α).map w).count v := by
    intro v
    rw [count_map_eq_countP w, count_map_eq_countP w]
    have e1 := countP_eq_add_countP_lt w (r : Multiset α) v
    have e2 := countP_eq_add_countP_lt w (r' : Multiset α) v
    have h1 := hle v
    have h2 := hlt v
    omega
  have hmap : (r : Multiset α).map w = (r' : Multiset α).map w := Multiset.ext.2 hcount
  have hperm : List.Perm (r.map w) (r'.map w) := by
    rw [← Multiset.coe_eq_coe, ← Multiset.map_coe, ← Multiset.map_coe]
    exact hmap
  haveI : IsAntisymm W (· ≥ ·) := ⟨fun _ _ h1 h2 => le_antisymm h2 h1⟩
  exact List.eq_of_perm_of_sorted hperm hsorted hsorted'

/-- The step of the merge that counts one side. Let `rx` be a top-k of `xs`, and let `r`
keep the element `b`. If the kept elements of `rx` and `ry` above `w b` are no more than
those of `r` above `w b`, then the elements of `xs` above `w b` are no more than the
length of `rx`. -/
theorem countP_le_length {k : ℕ} {xs rx ry r : List α} {p : α → Prop} [DecidablePred p]
    (hx : IsTopK w k xs rx) (hp : ∀ a b : α, p a → w a ≤ w b → p b)
    (hM : (rx : Multiset α).countP p + (ry : Multiset α).countP p ≤
      (r : Multiset α).countP p)
    (hrk : r.length ≤ k) (b : α) (hb : b ∈ r) (hpb : ¬ p b) :
    (xs : Multiset α).countP p ≤ rx.length := by
  obtain ⟨hsub, hlen, -, hdrop⟩ := hx
  have hA := countP_kept w hsub
    (fun d hd b' hb' => hdrop d hd b' (Multiset.mem_coe.1 hb')) hp
  have hcardrx : Multiset.card (rx : Multiset α) = rx.length := by simp
  have hcardr : Multiset.card (r : Multiset α) = r.length := by simp
  have hcardxs : Multiset.card (xs : Multiset α) = xs.length := by simp
  have hXle : (xs : Multiset α).countP p ≤ xs.length := by
    rw [← hcardxs]
    apply Multiset.countP_le_card
  have hRlt : (r : Multiset α).countP p < Multiset.card (r : Multiset α) := by
    apply lt_of_le_of_ne
    · apply Multiset.countP_le_card
    · intro heq
      rw [Multiset.countP_eq_card] at heq
      exact hpb (heq b (Multiset.mem_coe.2 hb))
  by_contra hcon
  push_neg at hcon
  have hk : rx.length = k := by
    rw [Nat.min_def] at hlen
    split_ifs at hlen <;> omega
  rw [hcardrx, min_eq_left hcon.le] at hA
  have hRge : rx.length ≤ (r : Multiset α).countP p := by omega
  rw [hcardr] at hRlt
  omega

/-- The merge (D-329): if `r` is a top-k of `rx ++ ry`, where `rx` and `ry` are top-k
results of `xs` and `ys`, then `r` is a top-k of `xs ++ ys`. This licenses sharded and
parallel recommendation. -/
theorem isTopK_merge {k : ℕ} {xs ys rx ry r : List α}
    (hx : IsTopK w k xs rx) (hy : IsTopK w k ys ry) (h : IsTopK w k (rx ++ ry) r) :
    IsTopK w k (xs ++ ys) r := by
  have hx' := hx
  have hy' := hy
  obtain ⟨hxsub, hxlen, -, hxdrop⟩ := hx'
  obtain ⟨hysub, hylen, -, hydrop⟩ := hy'
  obtain ⟨hrsub, hrlen, hrsorted, hrdrop⟩ := h
  have hMsub : ((rx ++ ry : List α) : Multiset α) ≤ ((xs ++ ys : List α) : Multiset α) := by
    rw [← Multiset.coe_add, ← Multiset.coe_add]
    exact add_le_add hxsub hysub
  have hsub : (r : Multiset α) ≤ ((xs ++ ys : List α) : Multiset α) := le_trans hrsub hMsub
  have hrk : r.length ≤ k := by
    rw [hrlen]
    exact min_le_left _ _
  have hcardrx : Multiset.card (rx : Multiset α) = rx.length := by simp
  have hcardry : Multiset.card (ry : Multiset α) = ry.length := by simp
  unfold IsTopK
  refine ⟨hsub, ?_, hrsorted, ?_⟩
  · rw [hrlen, List.length_append, List.length_append, hxlen, hylen]
    simp only [Nat.min_def]
    split_ifs <;> omega
  · intro a ha b hb
    by_contra hcon
    have hlt : w b < w a := not_le.1 hcon
    have hu : ∀ x y : α, w b < w x → w x ≤ w y → w b < w y :=
      fun _ _ hx'' hxy => lt_of_lt_of_le hx'' hxy
    have hzero : (((rx ++ ry : List α) : Multiset α) - (r : Multiset α)).countP
        (fun x => w b < w x) = 0 := by
      rw [Multiset.countP_eq_zero]
      intro d hd hpd
      exact (not_le.2 hpd) (hrdrop d hd b hb)
    have hsplitM : ((rx ++ ry : List α) : Multiset α).countP (fun x => w b < w x) =
        (r : Multiset α).countP (fun x => w b < w x) +
          (((rx ++ ry : List α) : Multiset α) - (r : Multiset α)).countP
            (fun x => w b < w x) := by
      rw [← Multiset.countP_add, add_tsub_cancel_of_le hrsub]
    have hMsplit : ((rx ++ ry : List α) : Multiset α).countP (fun x => w b < w x) =
        (rx : Multiset α).countP (fun x => w b < w x) +
          (ry : Multiset α).countP (fun x => w b < w x) := by
      rw [← Multiset.coe_add, Multiset.countP_add]
    have hM : (rx : Multiset α).countP (fun x => w b < w x) +
        (ry : Multiset α).countP (fun x => w b < w x) ≤
          (r : Multiset α).countP (fun x => w b < w x) := by
      omega
    have hM' : (ry : Multiset α).countP (fun x => w b < w x) +
        (rx : Multiset α).countP (fun x => w b < w x) ≤
          (r : Multiset α).countP (fun x => w b < w x) := by
      omega
    have hxc : (xs : Multiset α).countP (fun x => w b < w x) ≤ rx.length :=
      countP_le_length w hx (p := fun x => w b < w x) hu hM hrk b hb (lt_irrefl (w b))
    have hyc : (ys : Multiset α).countP (fun x => w b < w x) ≤ ry.length :=
      countP_le_length w hy (p := fun x => w b < w x) hu hM' hrk b hb (lt_irrefl (w b))
    have hxeq : (rx : Multiset α).countP (fun x => w b < w x) =
        (xs : Multiset α).countP (fun x => w b < w x) := by
      rw [countP_kept w hxsub
        (fun d hd b' hb' => hxdrop d hd b' (Multiset.mem_coe.1 hb'))
        (p := fun x => w b < w x) hu, hcardrx, min_eq_right hxc]
    have hyeq : (ry : Multiset α).countP (fun x => w b < w x) =
        (ys : Multiset α).countP (fun x => w b < w x) := by
      rw [countP_kept w hysub
        (fun d hd b' hb' => hydrop d hd b' (Multiset.mem_coe.1 hb'))
        (p := fun x => w b < w x) hu, hcardry, min_eq_right hyc]
    have hXsplit : ((xs ++ ys : List α) : Multiset α).countP (fun x => w b < w x) =
        (xs : Multiset α).countP (fun x => w b < w x) +
          (ys : Multiset α).countP (fun x => w b < w x) := by
      rw [← Multiset.countP_add, Multiset.coe_add]
    have hXdiff : ((xs ++ ys : List α) : Multiset α).countP (fun x => w b < w x) =
        (r : Multiset α).countP (fun x => w b < w x) +
          (((xs ++ ys : List α) : Multiset α) - (r : Multiset α)).countP
            (fun x => w b < w x) := by
      rw [← Multiset.countP_add, add_tsub_cancel_of_le hsub]
    have hne : (((xs ++ ys : List α) : Multiset α) - (r : Multiset α)).countP
        (fun x => w b < w x) ≠ 0 := by
      intro h0
      exact (Multiset.countP_eq_zero.1 h0) a ha hlt
    omega

end Gorse.Spec.TopK
