import Mathlib.Logic.Basic

/-! The empty proof scaffold of the verification plan (P1-2). It holds two
small theorems, so that the axiom check has something to check. The spec
modules for T1 and T2 come with P2 and P3. -/

namespace Gorse

theorem scaffold_true : True := trivial

theorem scaffold_or_comm (p q : Prop) : p ∨ q ↔ q ∨ p := Or.comm

end Gorse
