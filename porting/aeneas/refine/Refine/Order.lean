import AeneasSpikeTopk
import Gorse.Spec.TopKRegion

/-!
# The Rust elements in the D-329 order (P3-2 refinement, D-331)

The extracted `TopKFilter` holds `heap.Elem Std.U32 Std.I64`: a `u32` value and an `i64`
weight. The model in `Gorse.Spec.TopK` is over a linear order of weights and a key, so the
weight is read as an integer `wv` and the key as a natural number `kv`. `TieBefore wv kv a b`
is the D-329 order that `topKModel` uses. `heap.rs`'s `worse a b` is `TieBefore b a`.

`G a b` (not `TieBefore a b`) is the heap order: the parent of a child is never before it.
-/

open Aeneas Aeneas.Std Result
open aeneas_spike_topk Gorse.Spec.TopK

namespace Refine

noncomputable instance instDecEqElem : DecidableEq (heap.Elem Std.U32 Std.I64) :=
  Classical.decEq _

instance instInhabitedElem : Inhabited (heap.Elem Std.U32 Std.I64) :=
  ⟨{ value := default, weight := default }⟩

/-- The Rust elements, as the spec's element type. -/
abbrev E := heap.Elem Std.U32 Std.I64

/-- The weight of an element, as an integer. -/
def wv (e : E) : Int := e.weight.val

/-- The value of an element, as a natural number: the key of the D-329 tie-break. -/
def kv (e : E) : Nat := e.value.val

instance tbDec (a b : E) : Decidable (TieBefore wv kv a b) := by
  unfold TieBefore
  infer_instance

/-- `G a b`: `a` is not before `b` in D-329 order. -/
abbrev G (a b : E) : Prop := ¬ TieBefore wv kv a b

theorem tb_irrefl (a : E) : ¬ TieBefore wv kv a a :=
  fun h => tieBefore_asymm wv kv h h

theorem G_of_tb {a b : E} (h : TieBefore wv kv b a) : G a b :=
  tieBefore_asymm wv kv h

/-- Two elements with equal weights and keys are equal. -/
theorem elem_ext {a b : E} (hk : kv a = kv b) (hw : wv a = wv b) : a = b := by
  cases a with
  | mk av aw =>
    cases b with
    | mk bv bw =>
      simp only [kv, wv] at hk hw
      have hv : av = bv := UScalar.eq_of_val_eq hk
      have hw' : aw = bw := IScalar.eq_of_val_eq hw
      subst hv
      subst hw'
      rfl

/-- Totality of the D-329 order on distinct elements. -/
theorem tb_total {a b : E} (h : a ≠ b) : TieBefore wv kv a b ∨ TieBefore wv kv b a := by
  unfold TieBefore
  rcases lt_trichotomy (wv a) (wv b) with hlt | heq | hgt
  · exact Or.inr (Or.inl hlt)
  · rcases lt_trichotomy (kv a) (kv b) with hk | hk | hk
    · exact Or.inl (Or.inr ⟨heq, hk⟩)
    · exact absurd (elem_ext hk heq) h
    · exact Or.inr (Or.inr ⟨heq.symm, hk⟩)
  · exact Or.inl (Or.inl hgt)

/-- The heap order is transitive. -/
theorem G_trans {a b c : E} (h1 : G a b) (h2 : G b c) : G a c := by
  intro h
  by_cases hbc : b = c
  · subst hbc
    exact h1 h
  · rcases tb_total hbc with hb | hc
    · exact h2 hb
    · exact h1 (tieBefore_trans wv kv h hc)

/-- The Rust `worse` on elements is the D-329 order with its arguments swapped. -/
theorem worse_eq (a b : E) :
    heap.worse core.cmp.OrdU32 core.cmp.PartialOrdI64 a b =
      ok (decide (TieBefore wv kv b a)) := by
  unfold heap.worse TieBefore wv kv
  simp only [core.cmp.PartialOrdI64, core.cmp.OrdU32, liftFun2, bind_ok]
  split_ifs <;> simp_all [core.cmp.impls.PartialOrdI64.lt, core.cmp.impls.PartialOrdU32.gt,
    decide_eq_true_eq] <;>
    first | omega | scalar_tac | exact decide_eq_false (by omega) | exact decide_eq_true (by omega)

/-- `is_nan` on an `i64` weight is false: the partial comparison of a weight with itself is
`some` of an ordering, and `is_none` of that is `false`. -/
theorem is_nan_eq (w : Std.I64) : heap.is_nan core.cmp.PartialOrdI64 w = ok false := by
  simp [heap.is_nan, core.cmp.PartialOrdI64, core.cmp.impls.PartialOrdI64.partial_cmp, liftFun2,
    core.option.Option.is_none]

end Refine
