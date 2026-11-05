import Lean.Data.Json

/-!
# The Lean oracle for the T1 metrics (G9, D-326)

The executable reads one JSON case per line and writes one line per case: the f32 bit
pattern of the metric's value, or `none` where the metric is undefined. A case is
`{"m": metric, "t": [target ids], "l": [ranked ids]}`, with `metric` one of `precision`,
`recall`, `hr` and `mrr`.

Each value is computed exactly over the naturals and rounded once to the nearest f32, ties
to even. `ratioBits` mirrors `ratio_bits` in `crates/gorse-model/examples/g9_oracle.rs`,
step for step, and that file checks it against IEEE division. The definitions follow the
T1 specification in `Gorse/Spec/Metrics.lean`, with the same undefined cases:

* `precision` is undefined only for an empty ranked list;
* `recall` is undefined only for an empty target set;
* `hr` is undefined only for an empty ranked list;
* `mrr` is undefined for an empty target set or an empty ranked list.

`hits` counts entries of the ranked list with multiplicity, as the specification does.
-/

namespace Oracle

open Lean

/-- `n / d ≥ 2 ^ e`, for `d > 0`. -/
def geq (n d : Nat) (e : Int) : Bool :=
  if e ≥ 0 then decide (n ≥ d * 2 ^ e.toNat)
  else decide (n * 2 ^ (-e).toNat ≥ d)

/-- The exponent `e` with `2 ^ e ≤ n / d < 2 ^ (e + 1)`, found by stepping from `e`. -/
partial def findExp (n d : Nat) (e : Int) : Int :=
  if geq n d e then
    if geq n d (e + 1) then findExp n d (e + 1) else e
  else
    findExp n d (e - 1)

/-- The f32 bit pattern of `n / d`, rounded to nearest with ties to even, for `n / d` in
the normal f32 range. -/
def ratioBits (n d : Nat) : Nat :=
  if n = 0 then 0 else
    let e0 := findExp n d 0
    -- m = (n / d) / 2 ^ (e0 - 23), which lies in [2 ^ 23, 2 ^ 24).
    let s : Int := 23 - e0
    let num := if s ≥ 0 then n * 2 ^ s.toNat else n
    let den := if s ≥ 0 then d else d * 2 ^ (-s).toNat
    let m0 := num / den
    let r := num % den
    let m1 := if 2 * r > den ∨ (2 * r = den ∧ m0 % 2 = 1) then m0 + 1 else m0
    let me : Nat × Int := if m1 = 2 ^ 24 then (2 ^ 23, e0 + 1) else (m1, e0)
    ((me.2 + 127).toNat) * 2 ^ 23 + (me.1 - 2 ^ 23)

/-- The number of entries of `l` in `t`, counted with multiplicity. -/
def hits (t l : List Int) : Nat :=
  (l.filter (fun x => t.contains x)).length

/-- The position of the first entry of `l` in `t`, counted from `i`. -/
def firstHitIdx (t : List Int) : List Int → Nat → Option Nat
  | [], _ => none
  | x :: xs, i => if t.contains x then some i else firstHitIdx t xs (i + 1)

def precisionBits (t l : List Int) : Option Nat :=
  if l.isEmpty then none else some (ratioBits (hits t l) l.length)

def recallBits (t l : List Int) : Option Nat :=
  if t.isEmpty then none else some (ratioBits (hits t l) t.length)

def hrBits (t l : List Int) : Option Nat :=
  if l.isEmpty then none
  else if l.any (fun x => t.contains x) then some (ratioBits 1 1)
  else some 0

def mrrBits (t l : List Int) : Option Nat :=
  if t.isEmpty || l.isEmpty then none
  else
    match firstHitIdx t l 0 with
    | none => some 0
    | some i => some (ratioBits 1 (i + 1))

/-- The output token for one JSON case line. -/
def evalLine (line : String) : Except String String := do
  let j ← Json.parse line
  let m ← j.getObjValAs? String "m"
  let t ← j.getObjValAs? (List Int) "t"
  let l ← j.getObjValAs? (List Int) "l"
  let r ← match m with
    | "precision" => pure (precisionBits t l)
    | "recall" => pure (recallBits t l)
    | "hr" => pure (hrBits t l)
    | "mrr" => pure (mrrBits t l)
    | _ => throw s!"unknown metric {m}"
  match r with
  | none => pure "none"
  | some b => pure (toString b)

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
  IO.eprintln s!"oracle: {n} cases"
  return 0
