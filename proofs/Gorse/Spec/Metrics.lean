import Mathlib.Analysis.SpecialFunctions.Log.Base
import Mathlib.Tactic.Linarith
import Mathlib.Tactic.NormNum

/-! The T1 specification of the verification plan (§3.1, T1): the ranking metrics,
stated over finite sets of item IDs and lists of recommended IDs. The definitions
follow the Rust evaluator in its plain-English reading. The statements in this file
are frozen by PORTING.md D-317 and recorded in proofs/statements.txt. The keystone
theorems, `ndcg_mem_unitInterval` and `ndcg_eq_one_iff`, and their helper lemmas are
fixed by PORTING.md D-318. The link to the Rust f32 value comes with the refactor. -/

namespace Gorse.Spec.Metrics

open Finset

/-- The discount of the 0-based position `i` (Burges et al. 2005). -/
noncomputable def disc (i : ℕ) : ℝ := 1 / Real.logb 2 ((i : ℝ) + 2)

/-- The discounted cumulative gain. Only the positions below `L.length` are summed, so
the default `0` of `getD` is never read. -/
noncomputable def dcg (T : Finset ℕ) (L : List ℕ) : ℝ :=
  ∑ i ∈ range L.length, if L.getD i 0 ∈ T then disc i else 0

/-- The ideal discounted cumulative gain for `n` recommendations. -/
noncomputable def idcg (T : Finset ℕ) (n : ℕ) : ℝ := ∑ i ∈ range (min T.card n), disc i

/-- NDCG, undefined (`none`) when the target set or the list is empty. -/
noncomputable def ndcg (T : Finset ℕ) (L : List ℕ) : Option ℝ :=
  if T.card = 0 ∨ L = [] then none else some (dcg T L / idcg T L.length)

/-- The number of entries of `L` that are in `T`, counted with multiplicity. -/
def hits (T : Finset ℕ) (L : List ℕ) : ℕ :=
  (L.filter (fun x => decide (x ∈ T))).length

/-- Precision, written `P@|L|` (D-315, A3): the hits divided by `|L|`, undefined (`none`)
for an empty list. -/
noncomputable def precision (T : Finset ℕ) (L : List ℕ) : Option ℝ :=
  if L = [] then none else some ((hits T L : ℝ) / (L.length : ℝ))

/-- Recall, undefined (`none`) for an empty target set. -/
noncomputable def recall (T : Finset ℕ) (L : List ℕ) : Option ℝ :=
  if T.card = 0 then none else some ((hits T L : ℝ) / (T.card : ℝ))

/-- With no duplicates in `L`, every hit is a distinct element of `T`. -/
theorem hits_le_card {T : Finset ℕ} {L : List ℕ} (hnd : L.Nodup) :
    hits T L ≤ T.card := by
  unfold hits
  have hnodup : (L.filter (fun x => decide (x ∈ T))).Nodup := hnd.filter _
  have hsub : (L.filter (fun x => decide (x ∈ T))).toFinset ⊆ T := by
    intro x hx
    rw [List.mem_toFinset] at hx
    simpa using (List.mem_filter.mp hx).2
  calc (L.filter (fun x => decide (x ∈ T))).length
      = (L.filter (fun x => decide (x ∈ T))).toFinset.card :=
        (List.toFinset_card_of_nodup hnodup).symm
    _ ≤ T.card := Finset.card_le_card hsub

/-- Precision is at most 1. -/
theorem precision_le_one {T : Finset ℕ} {L : List ℕ} {v : ℝ}
    (h : precision T L = some v) : v ≤ 1 := by
  unfold precision at h
  by_cases hL : L = []
  · simp [hL] at h
  · rw [if_neg hL] at h
    obtain rfl := Option.some.inj h
    have hlen : (0 : ℝ) < (L.length : ℝ) := by
      exact_mod_cast List.length_pos_of_ne_nil hL
    have hhits : (hits T L : ℝ) ≤ (L.length : ℝ) := by
      have : hits T L ≤ L.length := by
        unfold hits
        exact List.length_filter_le _ _
      exact_mod_cast this
    exact (div_le_one hlen).mpr hhits

/-- Recall is at most 1 when `L` has no duplicates. -/
theorem recall_le_one {T : Finset ℕ} {L : List ℕ} {v : ℝ} (hnd : L.Nodup)
    (h : recall T L = some v) : v ≤ 1 := by
  unfold recall at h
  by_cases hT : T.card = 0
  · simp [hT] at h
  · rw [if_neg hT] at h
    obtain rfl := Option.some.inj h
    have hpos : (0 : ℝ) < (T.card : ℝ) := by
      exact_mod_cast Nat.pos_of_ne_zero hT
    have hle : (hits T L : ℝ) ≤ (T.card : ℝ) := by
      exact_mod_cast hits_le_card hnd
    exact (div_le_one hpos).mpr hle

/-- Duplicates break the bound: NDCG can exceed 1. The witness is the target `{1}` and
the list `[1, 1]`, whose DCG is `1 + 1/log₂ 3` against an IDCG of `1`. -/
theorem ndcg_gt_one_of_dup : ∃ (T : Finset ℕ) (L : List ℕ) (v : ℝ),
    ¬ L.Nodup ∧ ndcg T L = some v ∧ 1 < v := by
  refine ⟨{1}, [1, 1], 1 + 1 / Real.logb 2 3, ?_, ?_, ?_⟩
  · simp
  · have h2 : Real.logb 2 2 = 1 := Real.logb_self_eq_one (by norm_num)
    have hne : ¬ (({1} : Finset ℕ).card = 0 ∨ ([1, 1] : List ℕ) = []) := by simp
    have hd : dcg {1} [1, 1] = 1 + 1 / Real.logb 2 3 := by
      unfold dcg
      norm_num [Finset.sum_range_succ, disc, h2]
    have hi : idcg {1} [1, 1].length = 1 := by
      unfold idcg
      norm_num [disc, h2]
    unfold ndcg
    rw [if_neg hne, hd, hi, div_one]
  · have hpos : 0 < 1 / Real.logb 2 3 :=
      one_div_pos.mpr (Real.logb_pos (by norm_num) (by norm_num))
    linarith

/-- The discount is positive and strictly decreasing in the position. -/
theorem disc_pos (i : ℕ) : 0 < disc i := by
  unfold disc
  have hi0 : (0 : ℝ) ≤ (i : ℝ) := Nat.cast_nonneg i
  exact one_div_pos.mpr (Real.logb_pos (by norm_num) (by linarith))

theorem disc_lt {i j : ℕ} (h : i < j) : disc j < disc i := by
  unfold disc
  have hi0 : (0 : ℝ) ≤ (i : ℝ) := Nat.cast_nonneg i
  have hij : (i : ℝ) < (j : ℝ) := by exact_mod_cast h
  apply one_div_lt_one_div_of_lt
  · exact Real.logb_pos (by norm_num) (by linarith)
  · first
      | exact Real.logb_lt_logb (by norm_num) (by positivity) (by linarith)
      | exact (Real.logb_lt_logb_iff (by norm_num) (by positivity) (by positivity)).mpr
          (by linarith)

theorem disc_anti : Antitone disc := by
  intro i j h
  by_cases hij : i = j
  · subst hij
    exact le_rfl
  · exact (disc_lt (lt_of_le_of_ne h hij)).le

/-- The discount sum over any finite set of positions is at most the sum over the first
`card` positions. -/
theorem sum_le_sum_range_card (S : Finset ℕ) :
    ∑ i ∈ S, disc i ≤ ∑ i ∈ range S.card, disc i := by
  induction S using Finset.induction_on_max
  · simp
  · rename_i a s hlt ih
    have ha : a ∉ s := fun h => lt_irrefl a (hlt a h)
    have hcard : s.card ≤ a := by
      have hsub : s ⊆ range a := by
        intro x hx
        exact Finset.mem_range.mpr (hlt x hx)
      simpa using Finset.card_le_card hsub
    have hc : (insert a s).card = s.card + 1 := by
      first
        | exact Finset.card_insert_of_not_mem ha
        | exact Finset.card_insert_of_notMem ha
        | simp [ha]
    rw [Finset.sum_insert ha, hc, Finset.sum_range_succ]
    have hanti := disc_anti hcard
    linarith

/-- A set of at most `m` positions has a discount sum at most that of the first `m`. -/
theorem sum_le_range_of_card_le {S : Finset ℕ} {m : ℕ} (h : S.card ≤ m) :
    ∑ i ∈ S, disc i ≤ ∑ i ∈ range m, disc i := by
  refine (sum_le_sum_range_card S).trans ?_
  apply Finset.sum_le_sum_of_subset_of_nonneg
  · intro x hx
    rw [Finset.mem_range] at hx ⊢
    omega
  · intro i _ _
    exact (disc_pos i).le

/-- A set inside `range n` that misses a position `i < n` has a strictly smaller sum. -/
theorem sum_lt_of_missing_subset {n : ℕ} {S : Finset ℕ} (hS : S ⊆ range n) {i : ℕ}
    (hi : i < n) (hnot : i ∉ S) :
    ∑ j ∈ S, disc j < ∑ j ∈ range n, disc j := by
  have hsum : ∑ j ∈ range n \ S, disc j + ∑ j ∈ S, disc j = ∑ j ∈ range n, disc j :=
    Finset.sum_sdiff hS
  have hmem : i ∈ range n \ S := Finset.mem_sdiff.mpr ⟨Finset.mem_range.mpr hi, hnot⟩
  have hpos : disc i ≤ ∑ j ∈ range n \ S, disc j :=
    Finset.single_le_sum (fun j _ => (disc_pos j).le) hmem
  have hd := disc_pos i
  linarith

/-- A set of at most `m` positions that misses one of the first `m` positions has a
strictly smaller discount sum than the first `m`. -/
theorem sum_lt_of_card_le_of_missing (m : ℕ) (S : Finset ℕ) (hcard : S.card ≤ m)
    (hmiss : ∃ i < m, i ∉ S) :
    ∑ j ∈ S, disc j < ∑ j ∈ range m, disc j := by
  cases m with
  | zero =>
    obtain ⟨i, hi, _⟩ := hmiss
    exact absurd hi (Nat.not_lt_zero i)
  | succ m =>
    by_cases hc : ∃ c ∈ S, m < c
    · obtain ⟨c, hcS, hmc⟩ := hc
      have hcard' : (S.erase c).card ≤ m := by
        rw [Finset.card_erase_of_mem hcS]
        omega
      have hN := sum_le_range_of_card_le hcard'
      have hsplit : disc c + ∑ j ∈ S.erase c, disc j = ∑ j ∈ S, disc j := by
        first
          | exact Finset.add_sum_erase S disc hcS
          | (rw [add_comm]; exact Finset.sum_erase_add S disc hcS)
      have hlt : disc c < disc m := disc_lt hmc
      rw [Finset.sum_range_succ]
      linarith
    · push_neg at hc
      obtain ⟨i, hi, hnot⟩ := hmiss
      have hsub : S ⊆ range (m + 1) := by
        intro x hx
        rw [Finset.mem_range]
        have := hc x hx
        omega
      exact sum_lt_of_missing_subset hsub hi hnot

/-- The indices below `L.length` whose entry is in `T`. -/
def hitIdx (T : Finset ℕ) (L : List ℕ) : Finset ℕ :=
  (range L.length).filter (fun i => L.getD i 0 ∈ T)

theorem mem_hitIdx {T : Finset ℕ} {L : List ℕ} {i : ℕ} :
    i ∈ hitIdx T L ↔ i < L.length ∧ L.getD i 0 ∈ T := by
  unfold hitIdx
  simp only [Finset.mem_filter, Finset.mem_range]

theorem dcg_eq (T : Finset ℕ) (L : List ℕ) :
    dcg T L = ∑ i ∈ hitIdx T L, disc i := by
  unfold dcg hitIdx
  rw [Finset.sum_filter]

theorem dcg_nonneg (T : Finset ℕ) (L : List ℕ) : 0 ≤ dcg T L := by
  rw [dcg_eq]
  exact Finset.sum_nonneg (fun i _ => (disc_pos i).le)

/-- An entry of `L` at a position below its length is a member of `L`. -/
theorem getD_mem_of_lt : ∀ (L : List ℕ) (k : ℕ), k < L.length → L.getD k 0 ∈ L := by
  intro L
  induction L with
  | nil =>
    intro k hk
    simp at hk
  | cons b L ih =>
    intro k hk
    cases k with
    | zero => simp
    | succ k =>
      simp only [List.getD_cons_succ, List.mem_cons]
      right
      exact ih k (by simpa using hk)

/-- Without duplicates, distinct positions below the length hold distinct entries. -/
theorem getD_inj_of_nodup : ∀ (L : List ℕ), L.Nodup → ∀ x y : ℕ, x < L.length →
    y < L.length → L.getD x 0 = L.getD y 0 → x = y := by
  intro L
  induction L with
  | nil =>
    intro _ x _ hx _ _
    simp at hx
  | cons a L ih =>
    intro hnd x y hx hy hxy
    obtain ⟨ha, hL⟩ := List.nodup_cons.mp hnd
    cases x with
    | zero =>
      cases y with
      | zero => rfl
      | succ y =>
        exfalso
        simp only [List.getD_cons_zero, List.getD_cons_succ] at hxy
        apply ha
        rw [hxy]
        exact getD_mem_of_lt L y (by simpa using hy)
    | succ x =>
      cases y with
      | zero =>
        exfalso
        simp only [List.getD_cons_zero, List.getD_cons_succ] at hxy
        apply ha
        rw [← hxy]
        exact getD_mem_of_lt L x (by simpa using hx)
      | succ y =>
        simp only [List.getD_cons_succ] at hxy
        have h := ih hL x y (by simpa using hx) (by simpa using hy) hxy
        omega

/-- Without duplicates, the hits of `L` in `T` number at most `min |T| |L|`. -/
theorem card_hitIdx_le (T : Finset ℕ) {L : List ℕ} (hnd : L.Nodup) :
    (hitIdx T L).card ≤ min T.card L.length := by
  apply le_min
  · have hinj : Set.InjOn (fun i => L.getD i 0) (hitIdx T L : Set ℕ) := by
      intro x hx y hy hxy
      have hx' : x < L.length := (mem_hitIdx.mp (Finset.mem_coe.mp hx)).1
      have hy' : y < L.length := (mem_hitIdx.mp (Finset.mem_coe.mp hy)).1
      exact getD_inj_of_nodup L hnd x y hx' hy' hxy
    have hsub : (hitIdx T L).image (fun i => L.getD i 0) ⊆ T := by
      intro t ht
      rw [Finset.mem_image] at ht
      obtain ⟨i, hi, rfl⟩ := ht
      exact (mem_hitIdx.mp hi).2
    calc (hitIdx T L).card
        = ((hitIdx T L).image (fun i => L.getD i 0)).card :=
          (Finset.card_image_of_injOn hinj).symm
      _ ≤ T.card := Finset.card_le_card hsub
  · calc (hitIdx T L).card ≤ (range L.length).card := by
          apply Finset.card_le_card
          intro x hx
          exact Finset.mem_range.mpr (mem_hitIdx.mp hx).1
      _ = L.length := Finset.card_range _

theorem dcg_le_idcg (T : Finset ℕ) (L : List ℕ) (hnd : L.Nodup) :
    dcg T L ≤ idcg T L.length := by
  rw [dcg_eq]
  unfold idcg
  exact sum_le_range_of_card_le (card_hitIdx_le T hnd)

theorem idcg_pos {T : Finset ℕ} {n : ℕ} (hT : 0 < T.card) (hn : 0 < n) :
    0 < idcg T n := by
  unfold idcg
  apply Finset.sum_pos
  · intro i _
    exact disc_pos i
  · exact ⟨0, Finset.mem_range.mpr (lt_min hT hn)⟩

/-- The keystone of T1: with a non-empty target set, a non-empty list without duplicates
has an NDCG in [0, 1]. -/
theorem ndcg_mem_unitInterval {T : Finset ℕ} {L : List ℕ}
    (hT : T.Nonempty) (hL : L ≠ []) (hnd : L.Nodup) :
    ∃ v, ndcg T L = some v ∧ v ∈ Set.Icc (0 : ℝ) 1 := by
  have hTpos : 0 < T.card := Finset.card_pos.mpr hT
  have hn : 0 < L.length := List.length_pos_of_ne_nil hL
  have hnone : ¬ (T.card = 0 ∨ L = []) := by
    rintro (h | h)
    · omega
    · exact hL h
  have hidc : 0 < idcg T L.length := idcg_pos hTpos hn
  refine ⟨dcg T L / idcg T L.length, ?_, ?_⟩
  · unfold ndcg
    rw [if_neg hnone]
  · rw [Set.mem_Icc]
    exact ⟨div_nonneg (dcg_nonneg T L) hidc.le,
      (div_le_one hidc).mpr (dcg_le_idcg T L hnd)⟩

/-- The equality case of T1: with a non-empty target set, a non-empty list without
duplicates has NDCG exactly 1 when each of its first `min |T| |L|` entries is in `T`. -/
theorem ndcg_eq_one_iff {T : Finset ℕ} {L : List ℕ}
    (hT : T.Nonempty) (hL : L ≠ []) (hnd : L.Nodup) :
    ndcg T L = some 1 ↔ ∀ i < min T.card L.length, L.getD i 0 ∈ T := by
  have hTpos : 0 < T.card := Finset.card_pos.mpr hT
  have hn : 0 < L.length := List.length_pos_of_ne_nil hL
  have hnone : ¬ (T.card = 0 ∨ L = []) := by
    rintro (h | h)
    · omega
    · exact hL h
  have hidc : 0 < idcg T L.length := idcg_pos hTpos hn
  have key : ndcg T L = some (dcg T L / idcg T L.length) := by
    unfold ndcg
    rw [if_neg hnone]
  rw [key]
  constructor
  · intro h i hi
    have heq : dcg T L / idcg T L.length = 1 := Option.some.inj h
    rw [div_eq_iff hidc.ne', one_mul] at heq
    by_contra hnot
    have hnotS : i ∉ hitIdx T L := fun hmem => hnot (mem_hitIdx.mp hmem).2
    have hlt := sum_lt_of_card_le_of_missing (min T.card L.length) (hitIdx T L)
      (card_hitIdx_le T hnd) ⟨i, hi, hnotS⟩
    rw [dcg_eq] at heq
    unfold idcg at heq
    linarith
  · intro h
    have hsub : range (min T.card L.length) ⊆ hitIdx T L := by
      intro x hx
      rw [Finset.mem_range] at hx
      exact mem_hitIdx.mpr ⟨lt_of_lt_of_le hx (min_le_right _ _), h x hx⟩
    have hge : idcg T L.length ≤ dcg T L := by
      rw [dcg_eq]
      unfold idcg
      exact Finset.sum_le_sum_of_subset_of_nonneg hsub (fun j _ _ => (disc_pos j).le)
    have heq : dcg T L = idcg T L.length := le_antisymm (dcg_le_idcg T L hnd) hge
    rw [heq, div_self hidc.ne']

/-- The number of hits of `L` among its first `i + 1` positions. -/
def hitsUpTo (T : Finset ℕ) (L : List ℕ) (i : ℕ) : ℕ :=
  ((range (i + 1)).filter (fun j => L.getD j 0 ∈ T)).card

/-- Average precision, written `AP/|T|` (D-315, A3): the precision at each position that
holds a relevant item, summed and divided by `|T|`. The sum is the one the Rust `map`
computes. Undefined (`none`) when the target set or the list is empty. -/
noncomputable def ap (T : Finset ℕ) (L : List ℕ) : Option ℝ :=
  if T.card = 0 ∨ L = [] then none else
    some ((∑ i ∈ range L.length,
      if L.getD i 0 ∈ T then (hitsUpTo T L i : ℝ) / ((i : ℝ) + 1) else 0) / (T.card : ℝ))

theorem hitsUpTo_le (T : Finset ℕ) (L : List ℕ) (i : ℕ) : hitsUpTo T L i ≤ i + 1 := by
  unfold hitsUpTo
  calc ((range (i + 1)).filter (fun j => L.getD j 0 ∈ T)).card
      ≤ (range (i + 1)).card := Finset.card_le_card (Finset.filter_subset _ _)
    _ = i + 1 := Finset.card_range _

/-- Each term of the average-precision sum is at most the indicator of a hit. -/
theorem ap_term_le (T : Finset ℕ) (L : List ℕ) (i : ℕ) :
    (if L.getD i 0 ∈ T then (hitsUpTo T L i : ℝ) / ((i : ℝ) + 1) else 0) ≤
      (if L.getD i 0 ∈ T then (1 : ℝ) else 0) := by
  split_ifs
  · have hle : (hitsUpTo T L i : ℝ) ≤ (i : ℝ) + 1 := by
      exact_mod_cast hitsUpTo_le T L i
    have hpos : (0 : ℝ) < (i : ℝ) + 1 := by positivity
    exact (div_le_one hpos).mpr hle
  · exact le_rfl

/-- The average-precision sum is at most the number of hits. -/
theorem sum_ap_le_card (T : Finset ℕ) (L : List ℕ) :
    ∑ i ∈ range L.length,
        (if L.getD i 0 ∈ T then (hitsUpTo T L i : ℝ) / ((i : ℝ) + 1) else 0) ≤
      ((hitIdx T L).card : ℝ) := by
  calc ∑ i ∈ range L.length,
        (if L.getD i 0 ∈ T then (hitsUpTo T L i : ℝ) / ((i : ℝ) + 1) else 0)
      ≤ ∑ i ∈ range L.length, (if L.getD i 0 ∈ T then (1 : ℝ) else 0) :=
        Finset.sum_le_sum (fun i _ => ap_term_le T L i)
    _ = ((hitIdx T L).card : ℝ) := by
        first
          | simp [hitIdx, Finset.sum_boole]
          | (rw [Finset.sum_boole]; simp [hitIdx])

/-- Average precision is at most 1 when `L` has no duplicates. -/
theorem ap_le_one {T : Finset ℕ} {L : List ℕ} {v : ℝ} (hnd : L.Nodup)
    (h : ap T L = some v) : v ≤ 1 := by
  unfold ap at h
  by_cases hc : T.card = 0 ∨ L = []
  · rw [if_pos hc] at h
    simp at h
  · rw [if_neg hc] at h
    obtain rfl := Option.some.inj h
    have hTpos : (0 : ℝ) < (T.card : ℝ) := by
      have : 0 < T.card := Nat.pos_of_ne_zero (fun h0 => hc (Or.inl h0))
      exact_mod_cast this
    rw [div_le_one hTpos]
    have hsum := sum_ap_le_card T L
    have hcard : (hitIdx T L).card ≤ T.card := (card_hitIdx_le T hnd).trans (min_le_left _ _)
    have hcardR : ((hitIdx T L).card : ℝ) ≤ (T.card : ℝ) := by exact_mod_cast hcard
    linarith

end Gorse.Spec.Metrics
