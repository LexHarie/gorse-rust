/-!
# The Lean oracle for the ndcg log table (G9, D-354)

The executable prints one line for each k in 2..65: `k 0xBITS`, where `0xBITS` is the binary32
bit pattern of the exact `log2 k`, rounded once to the nearest value with ties to even. The G9
check compares these lines, byte for byte, with the table `LOG2_TABLE` of
crates/gorse-model/src/cf/evaluator.rs, printed by the example `g9_log2`.

The executable uses Lean's core library only, so its build does not compile Mathlib. The logarithm
is computed with exact natural-number arithmetic, and each step carries an error bound:

* `ln 2 = 2 · atanh (1/3)`, and for `k` in `[2^e, 2^(e+1))`, `ln (k / 2^e) = 2 · atanh (a / b)` with
  `a = k - 2^e` and `b = k + 2^e`. Here `a / b` is at most `1/3`. Each series is cut after `J` terms.
* `log2 k = e + ln (k / 2^e) / ln 2`, computed in the scale `2^P`.
* The rounding is decided from the enclosure `W ± E`. When the enclosure contains a rounding
  midpoint, the case is reported and the executable fails, so no entry is ever guessed.

A power of two is exact: `log2 k = e` with no error.
-/

namespace Oracle.Log2

/-- The fixed-point scale: a value `x` is held as the natural number `n` with `x ≈ n / 2^P`. -/
def P : Nat := 100

/-- The number of terms of each atanh series. `(1/9)^J` is far below `2^-P`, so the cut is
negligible. -/
def J : Nat := 60

/-- The error bound of an enclosure, in units of `2^-P`. Each series has `J` floored terms, each
off by less than one unit, so each of `ln 2` and `ln (k / 2^e)` is within `2 · J` units. The
division and the propagated error add a few hundred units at most, so `1000` is a safe bound. -/
def E : Nat := 1000

/-- `atanhAux a2 b2 n pa pb d acc` adds `floor (2^P · pa / (pb · d))` to `acc` for each of the next
`n` terms, where `pa = a^(2j+1)`, `pb = b^(2j+1)` and `d = 2j+1`. The powers grow by `a2 = a^2` and
`b2 = b^2` at each step. -/
def atanhAux (a2 b2 : Nat) : Nat → Nat → Nat → Nat → Nat → Nat
  | 0, _, _, _, acc => acc
  | n + 1, pa, pb, d, acc =>
    atanhAux a2 b2 n (pa * a2) (pb * b2) (d + 2) (acc + (2 ^ P * pa) / (pb * d))

/-- `atanh (a / b)` in the scale `2^P`, with `J` terms, for `0 ≤ a < b`. -/
def atanhScaled (a b : Nat) : Nat := atanhAux (a * a) (b * b) J a b 1 0

/-- `ln 2` in the scale `2^P`: `ln 2 = 2 · atanh (1/3)`. -/
def ln2Scaled : Nat := 2 * atanhScaled 1 3

/-- The rounding of the enclosure `W ± err`, where `W` is a value in the scale `2^P` and `W ≥ 2^P`.
Its binade is `q`, with `2^q ≤ W / 2^P < 2^(q+1)`, so one binary32 unit is `2^(P+q-23)`. The result
is the bit pattern `(q + 127) · 2^23 + (significand - 2^23)`, where the significand is in
`[2^23, 2^24]`. A significand of `2^24` carries into the exponent field, which that sum handles.
`none` means the enclosure contains a rounding midpoint. -/
def roundBits (W err : Nat) : Option Nat :=
  let q := Nat.log2 (W / 2 ^ P)
  let unit := 2 ^ (P + q - 23)
  let mant := W / unit
  let rem := W % unit
  let half := unit / 2
  if rem + err < half then
    some ((q + 127) * 2 ^ 23 + (mant - 2 ^ 23))
  else if half + err < rem then
    some ((q + 127) * 2 ^ 23 + (mant + 1 - 2 ^ 23))
  else
    none

/-- The bit pattern of `log2 k` for an integer `k` in `2..65`, or `none` when the rounding is not
decided. -/
def entryBits (k : Nat) : Option Nat :=
  let e := Nat.log2 k
  let p := 2 ^ e
  if k = p then
    -- `log2 k = e` exactly, so the value is a point.
    roundBits (e * 2 ^ P) 0
  else
    let a := k - p
    let b := k + p
    -- `ln (k / 2^e) = 2 · atanh (a / b)` in the scale `2^P`.
    let lnM := 2 * atanhScaled a b
    -- `log2 k = e + ln (k / 2^e) / ln 2` in the scale `2^P`.
    let w := e * 2 ^ P + (lnM * 2 ^ P) / ln2Scaled
    roundBits w E

/-- The eight hexadecimal digits of `n`, in upper case, most significant first. -/
def hex8 (n : Nat) : String :=
  String.mk <| (List.range 8).reverse.map fun i =>
    let d := (n / 16 ^ i) % 16
    if d < 10 then Char.ofNat (48 + d) else Char.ofNat (55 + d)

end Oracle.Log2

open Oracle.Log2

/-- Prints `k 0xBITS` for each k in 2..65. A case whose rounding is undecided is reported on
stderr, and the executable then exits with status 1. `main` is declared outside the namespace,
as the entry point of the executable, as Oracle.Map does. -/
def main : IO UInt32 := do
  let mut failed := false
  for k in [2:66] do
    match entryBits k with
    | some bits => IO.println s!"{k} 0x{hex8 bits}"
    | none =>
      IO.eprintln s!"oracle_log2: the enclosure of log2 {k} contains a rounding midpoint"
      failed := true
  return if failed then 1 else 0
