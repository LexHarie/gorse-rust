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

/-- `alloc::vec::Vec::pop` (`alloc/src/vec/mod.rs`, lines 2901:4-2901:38 as Charon records them,
in the Rust std pinned by `rust-toolchain.toml`, 1.99.1). Pops the last element, or returns
`None` and leaves the vector unchanged when it is empty. The allocator `A` is not modelled. -/
def alloc.vec.Vec.pop {T : Type _} (A : Type _) (v : alloc.vec.Vec T) :
    Result ((Option T) × (alloc.vec.Vec T)) :=
  match v.val.getLast? with
  | none => ok (none, v)
  | some x =>
    ok (some x, alloc.vec.Vec.from v.val.dropLast (by
      have := v.property
      simp only [List.length_dropLast]
      omega))

/-- `<Option<T> as Try>::branch` (`core/src/option.rs`, lines 2875:4-2875:64 as Charon records
them, Rust 1.99.1): `Some` continues with its value, `None` breaks with `None`. -/
def core.option.Option.Insts.CoreOpsTry_traitTry.branch {T : Type _} (o : Option T) :
    Result (core.ops.control_flow.ControlFlow (Option Never) T) :=
  match o with
  | none => ok (core.ops.control_flow.ControlFlow.Break none)
  | some x => ok (core.ops.control_flow.ControlFlow.Continue x)

/-- `<Option<T> as FromResidual<Option<!>>>::from_residual` (`core/src/option.rs`, lines
2889:4-2889:49 as Charon records them, Rust 1.99.1). The residual `Option<!>` has no `Some`
case, so only `None` is a value; it returns `None`. -/
def core.option.Option.Insts.CoreOpsTry_traitFromResidualOptionNever.from_residual
    (T : Type _) (r : Option Never) : Result (Option T) :=
  match r with
  | none => ok none
  | some n => nomatch n

end aeneas_spike_topk
