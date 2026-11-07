import Lean
import Oracle.Dot

/-!
# The trust listing of `oracle_dot` (D-349)

Run in CI by `lake env lean Oracle/Trust.lean`, after `lake build oracle_dot`. The walk starts at
`Oracle.Dot.main` and follows the values of the definitions it reaches. It prints each constant
whose code does not come from its own definition:

- `implemented_by n m`: the code of `n` is the code of `m`, and the walk follows `m`;
- `extern n`: `n` is implemented by the runtime, which the trust base must cover;
- `csimp n m`: the compiler replaces `n` by `m` (the `@[csimp]` lemmas), and the walk follows `m`.

Theorems and their proofs are not followed, because the code generator does not compile them. The
walk is an over-approximation: a constant that only occurs in a type is followed too, so it may
print more than the executable reaches, never fewer. The last line gives the number of constants
the walk visited.
-/

open Lean Elab Command

#eval show CommandElabM Unit from do
  let env ← getEnv
  let mut stack : Array Name := #[`Oracle.Dot.main]
  let mut seen : NameSet := {}
  let mut count := 0
  while stack.size > 0 do
    let n := stack.back!
    stack := stack.pop
    if seen.contains n then
      continue
    seen := seen.insert n
    count := count + 1
    if let some m := Compiler.getImplementedBy? env n then
      IO.println s!"implemented_by {n} {m}"
      stack := stack.push m
    if isExtern env n then
      IO.println s!"extern {n}"
    if let some m := (Compiler.CSimp.replaceConstants env (mkConst n)).constName? then
      if m != n then
        IO.println s!"csimp {n} {m}"
        stack := stack.push m
    if let some (.defnInfo d) := env.find? n then
      for c in d.value.getUsedConstants do
        stack := stack.push c
    let u := n.str "_unsafe_rec"
    if env.contains u then
      stack := stack.push u
  IO.println s!"constants visited: {count}"
