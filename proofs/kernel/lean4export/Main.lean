import Export
open Lean

def semver := "0.1.2"

def main (args : List String) : IO Unit := do
  initSearchPath (← findSysroot)
  let (imports, constants) := args.span (· != "--")
  let imports := imports.toArray.map fun mod => { module := Syntax.decodeNameLit ("`" ++ mod) |>.get! }
  let env ← importModules imports {}
  let constants := match constants.tail? with
    | some cs => cs.map fun c => Syntax.decodeNameLit ("`" ++ c) |>.get!
    | none    => env.constants.toList.map Prod.fst |>.filter (!·.isInternal)
  -- nanoda looks these constants up by name, whether or not a checked declaration reaches
  -- them. A constant that no declaration uses is not dumped otherwise, and nanoda then
  -- panics, as it does for `Quot.ind` (D-321). Each is dumped first, so every check finds it.
  let preamble := ["Quot", "Quot.mk", "Quot.lift", "Quot.ind", "String", "String.mk",
    "Nat", "Nat.zero", "Nat.succ", "Nat.add", "Nat.sub", "Nat.mul", "Nat.pow", "Nat.mod",
    "Nat.div", "Nat.beq", "Nat.ble", "Bool.true", "Bool.false", "Char", "Char.ofNat",
    "List", "List.nil", "List.cons", "Eq", "Eq.refl"]
  let preamble := preamble.map fun c => Syntax.decodeNameLit ("`" ++ c) |>.get!
  M.run env do
    IO.println semver
    for c in preamble ++ constants do
      let _ ← dumpConstant c
