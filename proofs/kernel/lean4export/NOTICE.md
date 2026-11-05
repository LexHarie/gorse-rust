# Vendored lean4export for Lean v4.15.0

The files in this directory are a copy of `ammkrn/lean4export`, branch `format2024`,
at commit `66be65dab1fcaabddde91a76e82e63beab180846`. They are under the Apache
License 2.0, whose text is in `LICENSE`.

That branch writes the export format which nanoda reads (nanoda's README names it).
It targets Lean `v4.20.0-rc5`. The project pins Lean `v4.15.0` (`proofs/lean-toolchain`),
and a Lean export can only be read by the Lean version that wrote the `.olean`
files, so the copy is changed only where Lean `v4.15.0` requires it:

- `lean-toolchain`: `leanprover/lean4:v4.15.0` instead of `v4.20.0-rc5`.

The changes to `Export.lean` and `Main.lean` are listed below once CI has built them.
The export format itself is unchanged.
