import Mathlib

/-!
# The decompressed request body is capped (audit S6, D-364)

`gzip` and `zlib` request bodies are inflated into a `Vec<u8>` in
`crates/gorse-server/src/rest/inflate.rs`. The compressed body is at most 2 MiB (axum's default
limit), but DEFLATE can expand by about 1032 to 1, so the output had no bound.

This file models the output of the inflater. A stream is a list of symbols: a literal byte, or a
copy of up to 258 bytes. The pre-fix model `outLen` has no cap. The fixed decoder `decodeCapped`
refuses any emit that would take the output past the cap.

* **Bug (S6).** `preOut_exceeds_cap`: a stream of `2^17` copies of length 258 has about 16 KiB of
  bits, far under the 2 MiB compressed limit, and produces more than the cap `2^25`.
* **Fix (S6).** `decodeCapped_le`: a decode that is accepted produces at most the cap.

Decoder correctness (that the inflater reproduces DEFLATE) is a separate, larger proof and is not
claimed here.
-/

namespace Gorse.Spec.InflateCap

/-- A decoded symbol: one literal byte, or a copy of `len` bytes. -/
inductive Sym where
  | lit : Sym
  | copy (len : ℕ) : Sym

/-- The number of bytes a symbol emits. -/
def symLen : Sym → ℕ
  | .lit => 1
  | .copy len => len

/-- The output length of a stream of symbols, with no cap. -/
def outLen : List Sym → ℕ
  | [] => 0
  | s :: ss => symLen s + outLen ss

/-- Pre-fix: the output length of a stream, which is unbounded. -/
def preOut (ss : List Sym) : ℕ :=
  outLen ss

/-- Post-fix: the output length of a stream, or `none` when an emit would take the output past
`cap`. `acc` is the output produced so far. -/
def decodeCapped (cap : ℕ) : ℕ → List Sym → Option ℕ
  | acc, [] => some acc
  | acc, s :: ss => if acc + symLen s ≤ cap then decodeCapped cap (acc + symLen s) ss else none

/-- `n` copies of length 258 emit `258 · n` bytes. -/
theorem outLen_replicate_copy (n : ℕ) : outLen (List.replicate n (Sym.copy 258)) = 258 * n := by
  induction n with
  | zero => rfl
  | succ k ih =>
    rw [List.replicate_succ, outLen, ih]
    simp only [symLen]
    ring

/-- **Bug (S6).** The stream of `2^17` copies of length 258 costs at least `2^17` bits (16 KiB,
under the 2 MiB compressed limit), and it produces more than the cap `2^25` (32 MiB). -/
theorem preOut_exceeds_cap : 2 ^ 25 < preOut (List.replicate (2 ^ 17) (Sym.copy 258)) := by
  unfold preOut
  rw [outLen_replicate_copy]
  norm_num

/-- **Fix (S6).** A decode that is accepted produces at most the cap. -/
theorem decodeCapped_le (cap : ℕ) (ss : List Sym) :
    ∀ (acc : ℕ), acc ≤ cap → ∀ n, decodeCapped cap acc ss = some n → n ≤ cap := by
  induction ss with
  | nil =>
    intro acc hacc n h
    simp only [decodeCapped] at h
    have hn := Option.some.inj h
    rw [← hn]
    exact hacc
  | cons s ss ih =>
    intro acc hacc n h
    simp only [decodeCapped] at h
    split_ifs at h with hs
    · exact ih (acc + symLen s) hs n h
    · cases h

end Gorse.Spec.InflateCap
