import Lean.Data.Json
import Gorse.Oracle.TopKMirror

/-!
# The Lean oracle for the top-k filter (G9, D-335)

The executable reads one JSON case per line and writes one line per case: the kept elements,
best first, as `id:weight` tokens separated by single spaces, or an empty line when nothing
is kept. A case is `{"k": k, "ids": [...], "ws": [...]}`, where `ids` and `ws` have the same
length. The Rust example `crates/gorse-common/examples/g9_topk.rs` writes the cases and
prints the `TopKFilter` result of each, in the same format.

Each line is computed by `Gorse.Oracle.TopKMirror.topK`. `Gorse/Oracle/TopKLink.lean` proves
that copy equal to `topKModel` in the Mathlib spec, so the run checks the Rust filter against
the model itself. The results are discrete, so the check is exact, with no rounding.
-/

namespace Oracle

open Lean Gorse.Oracle.TopKMirror

/-- The output line for one JSON case line. -/
def evalLine (line : String) : Except String String := do
  let j ← Json.parse line
  let k ← j.getObjValAs? Int "k"
  let ids ← j.getObjValAs? (List Int) "ids"
  let ws ← j.getObjValAs? (List Int) "ws"
  unless ids.length = ws.length do
    throw "ids and ws differ in length"
  if k < 0 then
    throw s!"k is negative: {k}"
  let kept := topK k.toNat (ids.zip ws)
  pure (String.intercalate " " (kept.map fun p => s!"{p.1}:{p.2}"))

/-- Reads cases until the end of the input, writes one line for each, and returns the
number of cases. -/
partial def process (stdin stdout : IO.FS.Stream) (count : Nat) : IO Nat := do
  let raw ← stdin.getLine
  if raw.isEmpty then
    return count
  let line := raw.trimRight
  if line.isEmpty then
    return (← process stdin stdout count)
  let out ← match evalLine line with
    | .ok out => pure out
    | .error e => throw (IO.userError s!"case {count + 1}: {e}")
  stdout.putStr (out ++ "\n")
  process stdin stdout (count + 1)

end Oracle

def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  let n ← Oracle.process stdin stdout 0
  stdout.flush
  IO.eprintln s!"oracle_topk: {n} cases"
  return 0
