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

- `Main.lean`: the first line of the export is `2.0.0` instead of `0.1.2`. nanoda
  `v0.3.2` rejects an export whose version is below `2.0.0` (its parser's
  `MIN_SUPPORTED_SEMVER`), and CI run 42035578046 failed on it. The version line is not
  otherwise checked, and the rest of the format is what nanoda's checker reads.

The export format itself is unchanged.
