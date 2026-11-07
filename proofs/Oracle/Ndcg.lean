import Lean.Data.Json
import Gorse.Spec.NdcgRat

/-!
# The Lean oracle for `ndcg` (G9, D-356)

The executable reads one JSON case per line, `{"t": T, "hits": [b, ...]}`, where `T = |T|` is the
size of the target set and each `b` says whether a ranked entry is relevant, in rank order. It
writes one line per case: `m e`, meaning `m * 2^e` (as `oracle_map`, D-352), or `none` when `ndcg` is
undefined. Every term and every sum is zero or positive, so a zero is written `0 0`.

The value is the ℚ twin `Gorse.Spec.NdcgRat.ndcgQ`, which rounds each step as Rust does. The output is
refused, not printed, when a value is not a multiple of its unit. A case with more than 64 entries is
refused too, with a non-zero exit: the twin covers the table of D-354 only, which holds `log2 k` for
`k ≤ 65`, so a longer list is never checked silently (D-326).
-/

namespace Oracle.Ndcg

open Lean Gorse.Spec.NdcgRat Gorse.Spec.Float32

/-- The longest ranked list the twin covers (D-354, D-356). -/
def maxLen : Nat := 64

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
  unless hits.length ≤ maxLen do
    throw s!"a ranked list of {hits.length} entries is longer than {maxLen}"
  match ndcgQ t hits with
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

end Oracle.Ndcg

def main : IO UInt32 := do
  let stdin ← IO.getStdin
  let stdout ← IO.getStdout
  let n ← Oracle.Ndcg.process stdin stdout 0
  stdout.flush
  IO.eprintln s!"oracle_ndcg: {n} cases"
  return 0
