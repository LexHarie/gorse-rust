import Gorse.Spec.Float32Q
import Gorse.Spec.Log2Table

/-!
# The ℚ twin of `ndcg` for one user (D-356, P4-11)

`ndcg` in `crates/gorse-model/src/cf/evaluator.rs` (lines 102-119) sums `1 / log2 (i + 2)` over the
relevant entries of the ranked list, which is the DCG, and over the first `min |T| N` positions,
which is the IDCG, and then divides the DCG by the IDCG. Each term is one binary32 division, each
addition is one binary32 addition, and the final division is one binary32 division, so each is
rounded once. This file writes that sequence over the rationals, with `rnQ` (D-340) at each step, so
its value is the binary32 result that Rust computes. The executable `oracle_ndcg`
(proofs/Oracle/Ndcg.lean) prints it.

The logarithm is the table `LOG2_TABLE` of D-354. `log2Bits k` is its entry for `2 ≤ k ≤ 65`, and
`log2Q k` is that binary32 entry as an exact rational. `log2Q_cast` ties it to the proven table of
`Gorse.Spec.Log2Table`, for each such `k`, so the twin's inputs are the proven entries.

The twin covers ranked lists of up to 64 entries, which are the arguments `k ≤ 65` of the table. The
executable refuses longer lists. A longer list uses the float64 logarithm in Rust, a stated
hypothesis (D-326, D-354), and is not covered here.
-/

namespace Gorse.Spec.NdcgRat

open Gorse.Spec.Float32 Gorse.Spec.Log2Table

/-- The bit pattern of `log2 k` in `LOG2_TABLE` (D-354), for `2 ≤ k ≤ 65`. Other arguments give `0`,
and the twin never uses them. -/
def log2Bits : ℕ → ℕ
  | 2 => 0x3F800000
  | 3 => 0x3FCAE00D
  | 4 => 0x40000000
  | 5 => 0x40149A78
  | 6 => 0x40257007
  | 7 => 0x4033ABB4
  | 8 => 0x40400000
  | 9 => 0x404AE00D
  | 10 => 0x40549A78
  | 11 => 0x405D6754
  | 12 => 0x40657007
  | 13 => 0x406CD401
  | 14 => 0x4073ABB4
  | 15 => 0x407A0A7F
  | 16 => 0x40800000
  | 17 => 0x4082CC7F
  | 18 => 0x40857007
  | 19 => 0x4087EF06
  | 20 => 0x408A4D3C
  | 21 => 0x408C8DDD
  | 22 => 0x408EB3AA
  | 23 => 0x4090C105
  | 24 => 0x4092B803
  | 25 => 0x40949A78
  | 26 => 0x40966A01
  | 27 => 0x4098280A
  | 28 => 0x4099D5DA
  | 29 => 0x409B7495
  | 30 => 0x409D053F
  | 31 => 0x409E88C7
  | 32 => 0x40A00000
  | 33 => 0x40A16BAD
  | 34 => 0x40A2CC7F
  | 35 => 0x40A42316
  | 36 => 0x40A57007
  | 37 => 0x40A6B3D8
  | 38 => 0x40A7EF06
  | 39 => 0x40A92204
  | 40 => 0x40AA4D3C
  | 41 => 0x40AB7111
  | 42 => 0x40AC8DDD
  | 43 => 0x40ADA3F6
  | 44 => 0x40AEB3AA
  | 45 => 0x40AFBD43
  | 46 => 0x40B0C105
  | 47 => 0x40B1BF31
  | 48 => 0x40B2B803
  | 49 => 0x40B3ABB4
  | 50 => 0x40B49A78
  | 51 => 0x40B58482
  | 52 => 0x40B66A01
  | 53 => 0x40B74B20
  | 54 => 0x40B8280A
  | 55 => 0x40B900E6
  | 56 => 0x40B9D5DA
  | 57 => 0x40BAA709
  | 58 => 0x40BB7495
  | 59 => 0x40BC3E9D
  | 60 => 0x40BD053F
  | 61 => 0x40BDC89A
  | 62 => 0x40BE88C7
  | 63 => 0x40BF45E1
  | 64 => 0x40C00000
  | 65 => 0x40C0B73D
  | _ => 0

/-- The exact value of a binary32 bit pattern over ℚ, with the exponent and the significand of
`decodeBits` in `Gorse.Spec.Log2Table`. -/
def decodeQ (b : ℕ) : ℚ :=
  ((2 ^ 23 + b % 2 ^ 23 : ℕ) : ℚ) * (2 : ℚ) ^ (((b / 2 ^ 23 % 256 : ℕ) : ℤ) - 150)

/-- `log2 k` of the table, as an exact rational. -/
def log2Q (k : ℕ) : ℚ := decodeQ (log2Bits k)

/-- The term `1 / log2 k` of `ndcg`, rounded once to binary32, as the Rust division. -/
def termQ (k : ℕ) : ℚ := rnQ (1 / log2Q k)

/-- The rounded DCG of `ndcg`. `flags` are the relevance flags of the ranked list, `i` is the index
of the next entry, and `s` is the partial sum. Each addition of a relevant term is rounded. -/
def dcgSumQ : List Bool → ℕ → ℚ → ℚ
  | [], _, s => s
  | b :: bs, i, s => dcgSumQ bs (i + 1) (if b then rnQ (s + termQ (i + 2)) else s)

/-- The rounded IDCG of `ndcg`: the sum of the first `m` terms, from index `i`. -/
def idcgSumQ : ℕ → ℕ → ℚ → ℚ
  | 0, _, s => s
  | m + 1, i, s => idcgSumQ m (i + 1) (rnQ (s + termQ (i + 2)))

/-- `ndcg` of one user, as a binary32 value over the rationals. `t` is `|T|`, and `flags` are the
relevance flags of the ranked list, one per entry, with repeats counted. It is `none` when the target
or the ranked list is empty, as in Rust. Otherwise the DCG is divided by the IDCG, and the quotient
is rounded. -/
def ndcgQ (t : ℕ) (flags : List Bool) : Option ℚ :=
  if t = 0 ∨ flags = [] then none
  else some (rnQ (dcgSumQ flags 0 0 / idcgSumQ (min t flags.length) 0 0))

/-- The exact value of `decodeQ b` is the binary32 value `decodeBits b` of `Gorse.Spec.Log2Table`. -/
theorem decodeQ_cast (b : ℕ) : ((decodeQ b : ℚ) : ℝ) = decodeBits b := by
  unfold decodeQ decodeBits
  rw [Rat.cast_mul, rat_cast_zpow]
  norm_num

/-- For each `k` of the table, `log2Q k` is the rounding of the exact logarithm, `rn (Real.logb 2 k)`,
which the entry lemmas of `Gorse.Spec.Log2Table` prove. -/
theorem log2Q_cast {k : ℕ} (h2 : 2 ≤ k) (h65 : k ≤ 65) :
    ((log2Q k : ℚ) : ℝ) = rn (Real.logb 2 (k : ℝ)) := by
  rw [log2Q, decodeQ_cast]
  interval_cases k <;>
    simp only [log2Bits, Nat.cast_ofNat, entry_2, entry_3, entry_4, entry_5, entry_6, entry_7,
      entry_8, entry_9, entry_10, entry_11, entry_12, entry_13, entry_14, entry_15, entry_16,
      entry_17, entry_18, entry_19, entry_20, entry_21, entry_22, entry_23, entry_24, entry_25,
      entry_26, entry_27, entry_28, entry_29, entry_30, entry_31, entry_32, entry_33, entry_34,
      entry_35, entry_36, entry_37, entry_38, entry_39, entry_40, entry_41, entry_42, entry_43,
      entry_44, entry_45, entry_46, entry_47, entry_48, entry_49, entry_50, entry_51, entry_52,
      entry_53, entry_54, entry_55, entry_56, entry_57, entry_58, entry_59, entry_60, entry_61,
      entry_62, entry_63, entry_64, entry_65]

/-- `ndcgQ` is `none` exactly when the target or the ranked list is empty, as `ndcg` is. -/
theorem ndcgQ_eq_none_iff (t : ℕ) (flags : List Bool) :
    ndcgQ t flags = none ↔ t = 0 ∨ flags = [] := by
  unfold ndcgQ
  by_cases h : t = 0 ∨ flags = []
  · rw [if_pos h]
    exact ⟨fun _ => h, fun _ => rfl⟩
  · rw [if_neg h]
    exact ⟨fun e => absurd e (Option.some_ne_none _), fun e => absurd e h⟩

end Gorse.Spec.NdcgRat
