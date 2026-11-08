import Mathlib

/-!
# The `offset` parameter at the request boundary (audit S4 and S5, D-363)

The handlers in `crates/gorse-server/src/rest/recommend.rs` read `offset` as an `i64`. This file
models the two bugs of the coverage audit (`/mnt/project-files/audit/coverage.md`, §5) and their
fixes, following the Lean bug rule
(Ritchie, 2026-11-08): each bug is proved in the pre-fix model, the fix is made, and the fix is
proved.

* **S4.** A negative `offset` was cast with `as usize`, which wraps on a 64-bit target, so
  `skip` dropped every item and the response was empty. The fix rejects a negative offset.
* **S5.** `offset + n` was an unchecked `i64` addition. In a release build it wraps (two's
  complement), so the limit could become negative. The fix checks the sum and rejects an overflow.

Integers are used for the `i64` values. `wrap64` is the two's-complement reading of an
unchecked add on `i64`.
-/

namespace Gorse.Spec.RequestOffset

/-- The two's-complement reading of an `i64` value: it wraps into `[-2^63, 2^63)`. -/
def wrap64 (z : Int) : Int :=
  (z + 9223372036854775808) % 18446744073709551616 - 9223372036854775808

/-- Pre-fix (S5): the unchecked `offset + n`, as a release build computes it. -/
def preLimit (offset n : Int) : Int :=
  wrap64 (offset + n)

/-- Post-fix (S5): the sum when it is in range, and nothing when it overflows (a 400). -/
def checkedAdd (a b : Int) : Option Int :=
  if -9223372036854775808 ≤ a + b ∧ a + b < 9223372036854775808 then some (a + b) else none

/-- Pre-fix (S4): `skip(offset as usize)` on a 64-bit target, which casts `-1` to `2^64 − 1`. -/
def preSkip {α : Type} (offset : Int) (xs : List α) : List α :=
  xs.drop ((offset % 18446744073709551616).toNat)

/-- Post-fix (S4): a negative offset is rejected (a 400); any other offset is accepted as is. -/
def parseOffset (s : Int) : Option Nat :=
  if 0 ≤ s then some s.toNat else none

/-! ## S5: the unchecked sum wraps, and the checked sum does not -/

/-- **Bug (S5).** The counterexample `offset = 1`, `n = 2^63 − 1`: the unchecked sum is
negative, so the limit is wrong. -/
theorem preLimit_wraps : preLimit 1 9223372036854775807 < 0 := by
  unfold preLimit wrap64
  norm_num

/-- **Fix (S5).** The checked sum rejects the same counterexample. -/
theorem checkedAdd_overflow : checkedAdd 1 9223372036854775807 = none := by
  unfold checkedAdd
  norm_num

/-- **Fix (S5).** An accepted checked sum is the exact sum, with no wrap. -/
theorem checkedAdd_sound {a b s : Int} (h : checkedAdd a b = some s) : s = a + b := by
  unfold checkedAdd at h
  split_ifs at h with hr
  · exact (Option.some.inj h).symm

/-! ## S4: a negative offset empties the page, and the parse rejects it -/

/-- **Bug (S4).** The counterexample `offset = -1`: the pre-fix skip drops every item, so the
result is empty. The hypothesis is the length bound of a 64-bit `usize`. -/
theorem preSkip_neg_empty {α : Type} (xs : List α) (hlen : xs.length ≤ 18446744073709551615) :
    preSkip (-1) xs = [] := by
  unfold preSkip
  have h : ((-1 : Int) % 18446744073709551616).toNat = 18446744073709551615 := by
    norm_num
    rfl
  rw [h]
  exact List.drop_eq_nil_of_le hlen

/-- **Fix (S4).** The parse rejects a negative offset. -/
theorem parseOffset_neg {s : Int} (h : s < 0) : parseOffset s = none := by
  unfold parseOffset
  rw [if_neg (not_le.mpr h)]

/-- **Fix (S4).** An accepted offset is the exact number of items to skip. -/
theorem parseOffset_some {s : Int} (h : 0 ≤ s) : parseOffset s = some s.toNat := by
  unfold parseOffset
  rw [if_pos h]

end Gorse.Spec.RequestOffset
