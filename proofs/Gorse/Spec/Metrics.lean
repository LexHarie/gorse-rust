import Mathlib.Analysis.SpecialFunctions.Log.Base
import Mathlib.Tactic.Linarith
import Mathlib.Tactic.NormNum

/-! The T1 specification of the verification plan (§3.1, T1): the ranking metrics,
stated over finite sets of item IDs and lists of recommended IDs. The definitions
follow the Rust evaluator in its plain-English reading. The statements in this file
are frozen by PORTING.md D-317 and recorded in proofs/statements.txt. The keystone
theorem, `ndcg_mem_unitInterval`, and the equality case come in a later batch, and
the link to the Rust f32 value comes with the refactor. -/

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

/-- Precision, undefined (`none`) for an empty list. -/
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

end Gorse.Spec.Metrics
