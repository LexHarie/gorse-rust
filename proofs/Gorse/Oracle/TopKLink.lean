import Mathlib
import Gorse.Spec.TopKModel
import Gorse.Oracle.TopKMirror

/-!
# The top-k mirror is the model (G9, D-335)

`Gorse.Oracle.TopKMirror.topK` is the computable copy that the G9 oracle runs. This file
proves that it equals `Gorse.Spec.TopK.topKModel` on pairs `(id, weight)`, with the weight
as the order's weight and the id as its key. So a run of the oracle is a run of the model.
-/

namespace Gorse.Oracle.TopKMirror

/-- The mirror's order on pairs is the spec's `TieBefore`. -/
theorem tieBefore_iff (a b : Int × Int) :
    tieBefore a b = true ↔
      Gorse.Spec.TopK.TieBefore (fun p : Int × Int => p.2) (fun p : Int × Int => p.1) a b := by
  simp [tieBefore, Gorse.Spec.TopK.TieBefore]

/-- Insertion in the mirror is insertion in the spec. -/
theorem ins_eq (x : Int × Int) (L : List (Int × Int)) :
    ins x L = Gorse.Spec.TopK.ins (fun p : Int × Int => p.2) (fun p : Int × Int => p.1) x L := by
  induction L with
  | nil => rfl
  | cons b l ih =>
    by_cases h : Gorse.Spec.TopK.TieBefore (fun p : Int × Int => p.2)
        (fun p : Int × Int => p.1) x b
    · have hb : tieBefore x b = true := (tieBefore_iff x b).2 h
      simp [ins, Gorse.Spec.TopK.ins, hb, h]
    · have hb : ¬ tieBefore x b = true := fun e => h ((tieBefore_iff x b).1 e)
      simp [ins, Gorse.Spec.TopK.ins, hb, h, ih]

/-- One push of the mirror is one push of the spec. -/
theorem pushStep_eq (k : ℕ) (buf : List (Int × Int)) (x : Int × Int) :
    pushStep k buf x = Gorse.Spec.TopK.pushStep (fun p : Int × Int => p.2)
      (fun p : Int × Int => p.1) k buf x := by
  unfold pushStep Gorse.Spec.TopK.pushStep
  rw [ins_eq]

/-- The mirror's top-k is the spec's `topKModel`, for every `k` and every list of pairs. -/
theorem topK_eq (k : ℕ) (xs : List (Int × Int)) :
    topK k xs = Gorse.Spec.TopK.topKModel (fun p : Int × Int => p.2)
      (fun p : Int × Int => p.1) k xs := by
  have hf : pushStep k = Gorse.Spec.TopK.pushStep (fun p : Int × Int => p.2)
      (fun p : Int × Int => p.1) k := by
    funext buf x
    exact pushStep_eq k buf x
  unfold topK Gorse.Spec.TopK.topKModel
  rw [hf]

end Gorse.Oracle.TopKMirror
