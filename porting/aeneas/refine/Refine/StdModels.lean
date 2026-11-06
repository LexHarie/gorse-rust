/-
Models of the three std functions that the extracted `TopKFilter` calls and that the Aeneas
library at c2d2343 does not model (D-331).

Charon marks them opaque, so Aeneas emits them as `axiom` declarations in the generated file.
The lean stage of `scripts/aeneas-spike.sh` removes those axioms from its copy of the
generated file and imports this module in their place. The proofs then depend on these
definitions, which are the Rust std behaviour written out, and not on axioms. Each one is
named after the generated declaration it replaces.
-/
module
public import Aeneas

@[expose] public section
open Aeneas Aeneas.Std Result ControlFlow Error
noncomputable section

namespace aeneas_spike_topk

/-- `alloc::vec::Vec::pop`: the last element, if there is one, and the vector without it. -/
def alloc.vec.Vec.pop {T : Type _} (A : Type _) (v : alloc.vec.Vec T) :
    Result ((Option T) × (alloc.vec.Vec T)) :=
  match v.val.getLast? with
  | none => ok (none, v)
  | some x =>
    ok (some x, alloc.vec.Vec.from v.val.dropLast (by
      have := v.property
      simp only [List.length_dropLast]
      omega))

/-- `<Option<T> as Try>::branch`: `Some` continues with its value, `None` breaks. -/
def core.option.Option.Insts.CoreOpsTry_traitTry.branch {T : Type _} (o : Option T) :
    Result (core.ops.control_flow.ControlFlow (Option Never) T) :=
  match o with
  | none => ok (core.ops.control_flow.ControlFlow.Break none)
  | some x => ok (core.ops.control_flow.ControlFlow.Continue x)

/-- `<Option<T> as FromResidual<Option<!>>>::from_residual`: the residual is always `None`. -/
def core.option.Option.Insts.CoreOpsTry_traitFromResidualOptionNever.from_residual
    (T : Type _) (r : Option Never) : Result (Option T) :=
  match r with
  | none => ok none
  | some n => nomatch n

end aeneas_spike_topk
