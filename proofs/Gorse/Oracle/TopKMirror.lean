/-!
# The computable mirror of the top-k model (G9, D-335)

`topK` computes what `Gorse.Spec.TopK.topKModel` computes, on pairs `(id, weight)` of
integers, in the same D-329 order: a higher weight first and, between equal weights, the
smaller id first. The G9 oracle (`proofs/Oracle/TopK.lean`) runs this copy, because the spec
is noncomputable: `Classical` decides its order. `Gorse/Oracle/TopKLink.lean` proves the copy
equal to the spec, so a run of the oracle checks the Rust filter against the model itself.

This file imports only Lean's core library, so the oracle executable does not build Mathlib.
-/

namespace Gorse.Oracle.TopKMirror

/-- `tieBefore a b` says that `a` comes before `b`: a higher weight first, and between equal
weights the smaller id first. Elements are `(id, weight)`. -/
def tieBefore (a b : Int × Int) : Bool :=
  decide (b.2 < a.2 ∨ (a.2 = b.2 ∧ a.1 < b.1))

/-- Inserts `x` before the first element of the list that `x` comes before. -/
def ins (x : Int × Int) : List (Int × Int) → List (Int × Int)
  | [] => [x]
  | b :: l => if tieBefore x b = true then x :: b :: l else b :: ins x l

/-- One push of the top-k loop: insert, then keep the `k` best. -/
def pushStep (k : Nat) (buf : List (Int × Int)) (x : Int × Int) : List (Int × Int) :=
  (ins x buf).take k

/-- The top `k` of `xs` in the D-329 order, best first. -/
def topK (k : Nat) (xs : List (Int × Int)) : List (Int × Int) :=
  xs.foldl (pushStep k) []

end Gorse.Oracle.TopKMirror
