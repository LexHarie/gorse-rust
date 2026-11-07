import Lean.Data.Json
import Gorse.Spec.MapRat

/-!
# The Lean oracle for `map` (G9, D-352)

The executable reads one JSON case per line, `{"t": T, "hits": [b, ...]}`, where `T = |T|` is the
size of the target set and each `b` says whether a ranked entry is relevant, in rank order. It
writes one line per case: `m e`, meaning `m * 2^e` (as `oracle_dot`, D-349), or `none` when `map` is
undefined. The value is never negative, because every term and every sum is zero or positive, so a
zero is written `0 0`, and its sign is not a question.

The value is the ℚ twin `Gorse.Spec.MapRat.mapQ`, which rounds each step as Rust does. The output
is refused, not printed, when a value is not a multiple of its unit.
-/

namespace Oracle.Map

open Lean Gorse.Spec.MapRat Gorse.Spec.Float32

/-- The output of a value `q` in binary32: `m e`, with `q = m * 2^e` and `e = ulpExpQ q`. -/
def fmtQ (q : ℚ) : Except String String := do
  if q = 0 then
    return "0 0"
  let e := ulpExpQ q
  let m := q / (2 : ℚ) ^ e
  unless m.den = 1 do
    throw s!"not a binary32 value: {q}"
  return s!"{m.num} {e}"

/-- The output line for one case line. -/
def evalLine (line : String) : Except String String := do
  let j ← Json.parse line
  let t ← j.getObjValAs? Nat "t"
  let flags ← (← j.getObjVal? "hits").getArr?
  let hits ← flags.toList.mapM fun b => b.getBool?
  match mapQ t hits with
  | none => return "none"
  | some q => fmtQ q

/-- Reads cases until the end of the input, writes one line for each, and returns their number. -/
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

end Oracle.Map

def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  let n ← Oracle.Map.process stdin stdout 0
  stdout.flush
  IO.eprintln s!"oracle_map: {n} cases"
  return 0
