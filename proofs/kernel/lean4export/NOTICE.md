# Vendored lean4export for Lean v4.15.0

The files in this directory are a copy of `ammkrn/lean4export`, branch `format2024`,
at commit `66be65dab1fcaabddde91a76e82e63beab180846`. They are under the Apache
License 2.0, whose text is in `LICENSE`.

That branch writes the export format which nanoda reads (nanoda's README names it).
It targets Lean `v4.20.0-rc5`. The project pins Lean `v4.15.0` (`proofs/lean-toolchain`),
and a Lean export can only be read by the Lean version that wrote the `.olean`
files, so the copy is changed only where Lean `v4.15.0` requires it:

- `lean-toolchain`: `leanprover/lean4:v4.15.0` instead of `v4.20.0-rc5`.
- `Export.lean`: the hash maps are written `Std.HashMap`, and the line
  `open Std (HashMap)` is removed. Lean `v4.15.0` still has the deprecated
  `Lean.HashMap`, which `open Lean` brings into scope, so the bare name is ambiguous
  there (CI run 42035215698 failed on it).

`Main.lean` is changed for D-321 (the second section below). The export format itself is
unchanged.

The format is the one nanoda `v0.2.0` reads, which is the version pinned in
`proofs/tools.env`. Its `#IND` lines carry two flags, `isRec` and `isNested`. Nanoda
`v0.3.x` expects a third, a reflexivity flag, and it rejects a first line below `2.0.0`.
Those tags do not read this export, so the pin, not the exporter, carries the
version choice.

## Change for D-321: constructors and recursors are dumped with their inductive

`Export.lean`, in the `.inductInfo` case of `dumpConstant`: the copy dumped each
constructor's type, but not the constructor itself, and it did not dump the
recursor. A constructor was exported only when another declaration used it. The
export format is unchanged. The change makes the export complete.

The change calls `dumpConstant` on each constructor and on `<inductive>.rec` before the
`#IND` line is printed.

**Why it matters.** nanoda `v0.2.0` checks a nested inductive by looking up every
constructor named in `#IND` (`src/inductive.rs`, `collect_unmodified_mutuals` and
`header_of_ty`). A missing constructor is an `Option::unwrap` on `None`, a panic. Every
Mathlib-dependent export of D-321 hit it, because the Mathlib closure contains nested
inductives whose constructors no theorem uses.

**Reproduction (sandbox, Lean `v4.15.0`, Lean core only).** A file with `Lean.Syntax`
(a core nested inductive: `node` carries `Array Syntax`) and a user nested type
`T | node : Array T → T | leaf : Nat → T`, exported by the copy before the change:
`nanoda_bin` exits 101 at `inductive.rs:468:77` (`Option::unwrap()` on `None`) on
both. After the change, the same two exports check with no errors: `Checked 127
declarations` for `Lean.Syntax`, `Checked 33` for `T`. Core `Nat.add_comm` checked
before and after. For the test export that holds `Lean.Syntax` and its closure, the
`#CTOR` count moved from 12 to 39 and the `#REC` count from 1 to 28.

**What it does not change.** The export of a theorem still contains only what it
reaches. The change adds the missing constructors and recursors of an inductive that
the export already contains, so nanoda checks more declarations and none are dropped.

**Regression test.** `scripts/check-kernels.sh` exports `Lean.Syntax` from `Init` and
checks it with nanoda before the project's export, so a copy without the change fails in
CI.

## Change for D-321: the constants nanoda looks up are dumped first

`Main.lean` dumps the requested constants and whatever they reach. nanoda `v0.2.0`
looks up some constants by name whether or not a checked declaration reaches them
(`src/util.rs`, `find_name`, and the `name_cache` unwraps). A check that reaches
`Quot.mk` and `Quot.lift` but not `Quot.ind` panics at `src/quot.rs:141:61` (an
`Option::unwrap` on the missing `Quot.ind`). Nothing in a Mathlib-free export is
guaranteed to reach all of them, so the same panic can appear on any export that uses
`funext`, `propext` or `Quot.sound`.

`Main.lean` now dumps these constants before the requested ones: `Quot`, `Quot.mk`,
`Quot.lift`, `Quot.ind`, `String`, `String.mk`, `Nat`, `Nat.zero`, `Nat.succ`, `Nat.add`,
`Nat.sub`, `Nat.mul`, `Nat.pow`, `Nat.mod`, `Nat.div`, `Nat.beq`, `Nat.ble`, `Bool.true`,
`Bool.false`, `Char`, `Char.ofNat`, `List`, `List.nil`, `List.cons`, `Eq` and `Eq.refl`.
The 24 names of `find_name` in nanoda's `src/util.rs` are among them. These are
declarations of Lean's core, and nanoda checks each one, so the change adds checks and
removes none.

**Evidence (sandbox, Lean `v4.15.0`, core only).** A test file of 120 theorems (60 `simp`
proofs over `List`, 60 `omega` proofs over `Nat`) gave an export of 79,414 lines. Before the preamble, nanoda panicked at `src/quot.rs:141:61`. After it, the
export is 80,188 lines, and nanoda checks 1,329 declarations with no errors in about one
second.
