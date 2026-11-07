import Lean.Data.Json
import Gorse.Spec.DotRat

/-!
# The Lean oracle for the dot kernels (G9, D-349)

The executable reads one JSON case per line, and writes one line per case. A case is
`{"model": M, "xs": [[m, e], ...], "ys": [[m, e], ...]}`, where `M` is `seq`, `avx512`, `avx` or
`neon`. An element `[m, e]` is the rational `m * 2^e`. Every binary32 value has this form, so the
Rust side can send its inputs exactly.

Each case is computed by the ℚ twin of the model (`Gorse.Spec.DotRat`, D-348). The twins round with
`rnQ`, so the result is the binary32 value that the model produces. The line written for a case is
`m e`, which means the result is `m * 2^e`, with `e = ulpExpQ` (the unit in the last place) and `m`
an integer below 2^24 in size. A result of zero is written as `0 0`, so the sign of zero is not
written. A result that is not a multiple of its unit cannot come from `rnQ`, so the run stops with
an error if one appears.
-/

namespace Oracle.Dot

open Lean Gorse.Spec.DotRat Gorse.Spec.Float32

/-- An element `[m, e]`, read as the pair of integers `(m, e)`. -/
def parseElem (j : Json) : Except String (Int × Int) := do
  let a ← j.getArr?
  match a[0]?, a[1]?, a.size with
  | some m, some e, 2 => pure (← m.getInt?, ← e.getInt?)
  | _, _, _ => throw "an element is not a pair [m, e]"

/-- The array of elements in field `field`. -/
def parseList (j : Json) (field : String) : Except String (Array (Int × Int)) := do
  let arr ← (← j.getObjVal? field).getArr?
  arr.mapM parseElem

/-- The rational value `m * 2^e` of an element. -/
def valQ (p : Int × Int) : ℚ := (p.1 : ℚ) * (2 : ℚ) ^ p.2

/-- The output of a result `q`: `m e` with `q = m * 2^e` and `e = ulpExpQ q`. A result that is
not an integer multiple of its unit is refused, because it is not a binary32 value. -/
def fmtQ (q : ℚ) : Except String String := do
  if q = 0 then
    return "0 0"
  let e := ulpExpQ q
  let m := q / (2 : ℚ) ^ e
  unless m.den = 1 do
    throw s!"not a binary32 value: {q}"
  return s!"{m.num} {e}"

/-- The output line for one JSON case line. -/
def evalLine (line : String) : Except String String := do
  let j ← Json.parse line
  let model ← j.getObjValAs? String "model"
  let xs ← parseList j "xs"
  let ys ← parseList j "ys"
  unless xs.size = ys.size do
    throw "xs and ys differ in length"
  let v : List (ℚ × ℚ) := (xs.zip ys).toList.map fun (x, y) => (valQ x, valQ y)
  let r ← if model = "seq" then pure (dotSeqQ v)
    else if model = "avx512" then pure (dotAvx512Q v)
    else if model = "avx" then pure (dotAvxQ v)
    else if model = "neon" then pure (dotNeonQ v)
    else throw s!"unknown model {model}"
  fmtQ r

/-- Reads cases until the end of the input, writes one line for each, and returns the number of
cases. -/
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

end Oracle.Dot

def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  let n ← Oracle.Dot.process stdin stdout 0
  stdout.flush
  IO.eprintln s!"oracle_dot: {n} cases"
  return 0
