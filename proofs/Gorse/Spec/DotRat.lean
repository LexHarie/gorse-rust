import Gorse.Spec.Float32Q
import Gorse.Spec.DotAvx
import Gorse.Spec.DotNeon

/-! # The ℚ twins of the dot models, with agreement (P4-7c, T3)

Each dot model over `ℝ` has a twin over `ℚ` that rounds with `rnQ` (D-340) in place of `rn`.
The twins keep the loop order of the models they copy:

- `dotSeqQ` for `dotSeq` (Dot.lean; floats.rs:457-463);
- `dotAvx512Q` for `dotAvx512` (DotAvx512.lean; floats_avx512.c:306-366);
- `dotAvxQ` for `dotAvx` (DotAvx.lean; floats_avx.c:243-279);
- `dotNeonQ` for `dotNeon` (DotNeon.lean; floats_neon.c:145-147).

The rational list `v` is read as reals by `castV`. Each `…_cast` theorem says that the real
model of `castV v`, cast to `ℝ`, is the rational twin cast to `ℝ`. The proofs use `rn_ratCast`
(`rn (q : ℝ) = (rnQ q : ℝ)`, D-340) and the casts of sums and products.

The twins are `def`s, not `noncomputable def`s, so Lean checks that they compile. `rnQ` is a
`def` as well, and it compiles on master. The executable that runs the twins is chosen in D-349.
-/

namespace Gorse.Spec.DotRat

open Gorse.Spec.Float32
open Gorse.Spec.Dot
open Gorse.Spec.DotAvx512
open Gorse.Spec.DotAvx
open Gorse.Spec.DotNeon

/-! ## Casting and elements -/

/-- A rational list read as reals, pair by pair. -/
noncomputable def castV (v : List (ℚ × ℚ)) : List (ℝ × ℝ) :=
  v.map fun p => ((p.1 : ℝ), (p.2 : ℝ))

/-- The element `i` of a rational list, with `(0, 0)` past the end. -/
def elemQ (v : List (ℚ × ℚ)) (i : ℕ) : ℚ × ℚ := v.getD i (0, 0)

/-- The exact product of element `i`, over `ℚ`. -/
def feQ (v : List (ℚ × ℚ)) (i : ℕ) : ℚ := (elemQ v i).1 * (elemQ v i).2

theorem castV_cons (p : ℚ × ℚ) (v : List (ℚ × ℚ)) :
    castV (p :: v) = (((p.1 : ℝ), (p.2 : ℝ)) :: castV v) := by
  simp [castV]

theorem castV_length (v : List (ℚ × ℚ)) : (castV v).length = v.length := by
  simp [castV]

theorem elem_castV (v : List (ℚ × ℚ)) (i : ℕ) :
    elem (castV v) i = (((elemQ v i).1 : ℝ), ((elemQ v i).2 : ℝ)) := by
  induction v generalizing i with
  | nil => simp [castV, elemQ, elem]
  | cons p v ih =>
    cases i with
    | zero => simp [castV, elemQ, elem]
    | succ i => simpa [castV, elemQ, elem] using ih i

theorem fe_castV (v : List (ℚ × ℚ)) (i : ℕ) : fe (castV v) i = ((feQ v i : ℚ) : ℝ) := by
  simp only [fe, feQ, elem_castV, Rat.cast_mul]

/-! ## Rounding casts -/

theorem rn_castQ (x : ℚ) : rn (x : ℝ) = ((rnQ x : ℚ) : ℝ) := rn_ratCast x

theorem rn_add_castQ (x y : ℚ) : rn ((x : ℝ) + (y : ℝ)) = ((rnQ (x + y) : ℚ) : ℝ) := by
  rw [← Rat.cast_add, rn_ratCast]

theorem rn_mul_castQ (x y : ℚ) : rn ((x : ℝ) * (y : ℝ)) = ((rnQ (x * y) : ℚ) : ℝ) := by
  rw [← Rat.cast_mul, rn_ratCast]

/-! ## The sequential dot product (`dotSeq`) -/

/-- `dotAux` over `ℚ`: each product is rounded, then each sum is rounded. -/
def dotAuxQ : List (ℚ × ℚ) → ℚ → ℚ
  | [], s => s
  | p :: v, s => dotAuxQ v (rnQ (s + rnQ (p.1 * p.2)))

/-- `dotSeq` over `ℚ`, from 0. -/
def dotSeqQ (v : List (ℚ × ℚ)) : ℚ := dotAuxQ v 0

theorem dotAuxQ_cast (v : List (ℚ × ℚ)) : ∀ (s : ℚ),
    dotAux (castV v) (s : ℝ) = ((dotAuxQ v s : ℚ) : ℝ) := by
  induction v with
  | nil =>
    intro s
    simp [castV, dotAux, dotAuxQ]
  | cons p v ih =>
    intro s
    rw [castV_cons]
    simp only [dotAux, dotAuxQ]
    rw [rn_mul_castQ, rn_add_castQ]
    exact ih _

theorem dotSeq_castQ (v : List (ℚ × ℚ)) : dotSeq (castV v) = ((dotSeqQ v : ℚ) : ℝ) := by
  have h := dotAuxQ_cast v 0
  rw [Rat.cast_zero] at h
  unfold dotSeq dotSeqQ
  exact h

/-! ## The AVX-512 dot product (`dotAvx512`) -/

/-- Lane `j` over `ℚ`, fused: each epoch rounds `x*y + s` once (floats_avx512.c:311-326). -/
def laneQ (v : List (ℚ × ℚ)) (j : ℕ) : ℕ → ℚ
  | 0 => 0
  | k + 1 => rnQ (feQ v (16 * k + j) + laneQ v j k)

def red8Q (s : ℕ → ℚ) (j : ℕ) : ℚ := rnQ (s j + s (j + 8))

def red4Q (s : ℕ → ℚ) (j : ℕ) : ℚ := rnQ (red8Q s j + red8Q s (j + 4))

def reduce16Q (s : ℕ → ℚ) : ℚ :=
  rnQ (rnQ (red4Q s 0 + red4Q s 2) + rnQ (red4Q s 1 + red4Q s 3))

/-- The rounded product of element `i` over `ℚ`. -/
def prodRQ (v : List (ℚ × ℚ)) (i : ℕ) : ℚ := rnQ (feQ v i)

def bl4Q (v : List (ℚ × ℚ)) (b j : ℕ) : ℚ := rnQ (prodRQ v (b + j) + prodRQ v (b + (j + 4)))

def bl2Q (v : List (ℚ × ℚ)) (b j : ℕ) : ℚ := rnQ (bl4Q v b j + bl4Q v b (j + 2))

def block8Q (v : List (ℚ × ℚ)) (b : ℕ) : ℚ := rnQ (bl2Q v b 0 + bl2Q v b 1)

/-- The scalar tail over `ℚ`: one fused step per element. -/
def tailAuxQ (v : List (ℚ × ℚ)) : ℕ → ℕ → ℚ → ℚ
  | _, 0, s => s
  | i, m + 1, s => tailAuxQ v (i + 1) m (rnQ (feQ v i + s))

/-- The dot product of `n = 16 E + r` elements over `ℚ`. -/
def dotCoreQ (v : List (ℚ × ℚ)) (E r : ℕ) : ℚ :=
  if 8 ≤ r then
    tailAuxQ v (16 * E + 8) (r - 8) (rnQ (reduce16Q (fun j => laneQ v j E) + block8Q v (16 * E)))
  else
    tailAuxQ v (16 * E) r (reduce16Q (fun j => laneQ v j E))

/-- The AVX-512 dot product over `ℚ` of a list of pairs. -/
def dotAvx512Q (v : List (ℚ × ℚ)) : ℚ := dotCoreQ v (v.length / 16) (v.length % 16)

theorem laneQ_cast (v : List (ℚ × ℚ)) (j k : ℕ) :
    lane (castV v) j k = ((laneQ v j k : ℚ) : ℝ) := by
  induction k with
  | zero => simp [lane, laneQ]
  | succ k ih =>
    show rn (fe (castV v) (16 * k + j) + lane (castV v) j k)
      = ((rnQ (feQ v (16 * k + j) + laneQ v j k) : ℚ) : ℝ)
    rw [fe_castV, ih, rn_add_castQ]

theorem reduce16_cast (v : List (ℚ × ℚ)) (E : ℕ) :
    reduce16 (fun j => lane (castV v) j E) = ((reduce16Q (fun j => laneQ v j E) : ℚ) : ℝ) := by
  simp only [reduce16, red4, red8, reduce16Q, red4Q, red8Q, laneQ_cast, rn_add_castQ]

theorem prodR_cast (v : List (ℚ × ℚ)) (i : ℕ) :
    prodR (castV v) i = ((prodRQ v i : ℚ) : ℝ) := by
  unfold prodR prodRQ
  rw [fe_castV, rn_castQ]

theorem block8_cast (v : List (ℚ × ℚ)) (b : ℕ) :
    block8 (castV v) b = ((block8Q v b : ℚ) : ℝ) := by
  simp only [block8, block8Q, bl2, bl2Q, bl4, bl4Q, prodR_cast, rn_add_castQ]

theorem tailAux_cast (v : List (ℚ × ℚ)) (m i : ℕ) (c : ℚ) :
    tailAux (castV v) i m (c : ℝ) = ((tailAuxQ v i m c : ℚ) : ℝ) := by
  induction m generalizing i c with
  | zero => simp [tailAux, tailAuxQ]
  | succ m ih =>
    show tailAux (castV v) (i + 1) m (rn (fe (castV v) i + (c : ℝ)))
      = ((tailAuxQ v (i + 1) m (rnQ (feQ v i + c)) : ℚ) : ℝ)
    rw [fe_castV, rn_add_castQ]
    exact ih (i + 1) _

theorem dotCore_cast (v : List (ℚ × ℚ)) (E r : ℕ) :
    dotCore (castV v) E r = ((dotCoreQ v E r : ℚ) : ℝ) := by
  unfold dotCore dotCoreQ
  split_ifs
  · simp only [reduce16_cast, block8_cast, rn_add_castQ, tailAux_cast]
  · simp only [reduce16_cast, tailAux_cast]

theorem dotAvx512_cast (v : List (ℚ × ℚ)) :
    dotAvx512 (castV v) = ((dotAvx512Q v : ℚ) : ℝ) := by
  unfold dotAvx512 dotAvx512Q
  rw [castV_length, dotCore_cast]

/-! ## The AVX dot product (`dotAvx`) -/

/-- Lane `j` over `ℚ`, non-fused: a product, then a sum (floats_avx.c:248-263). -/
def laneAQ (v : List (ℚ × ℚ)) (j : ℕ) : ℕ → ℚ
  | 0 => 0
  | 1 => prodRQ v j
  | k + 1 + 1 => rnQ (prodRQ v (8 * (k + 1) + j) + laneAQ v j (k + 1))

def reduce8AQ (s : ℕ → ℚ) : ℚ :=
  rnQ (rnQ (rnQ (s 0 + s 4) + rnQ (s 2 + s 6)) + rnQ (rnQ (s 1 + s 5) + rnQ (s 3 + s 7)))

/-- The scalar tail over `ℚ`: a product, then a sum, per element. -/
def tailAQ (v : List (ℚ × ℚ)) : ℕ → ℕ → ℚ → ℚ
  | _, 0, s => s
  | i, m + 1, s => tailAQ v (i + 1) m (rnQ (prodRQ v i + s))

def dotCoreAQ (v : List (ℚ × ℚ)) (E r : ℕ) : ℚ :=
  tailAQ v (8 * E) r (reduce8AQ (fun j => laneAQ v j E))

/-- The AVX dot product over `ℚ`. -/
def dotAvxQ (v : List (ℚ × ℚ)) : ℚ := dotCoreAQ v (v.length / 8) (v.length % 8)

/-- The cases `k` and `k + 1` together, so that `k + 2` can use `k + 1`. -/
theorem laneA_pair (v : List (ℚ × ℚ)) (j : ℕ) (k : ℕ) :
    laneA (castV v) j k = ((laneAQ v j k : ℚ) : ℝ) ∧
      laneA (castV v) j (k + 1) = ((laneAQ v j (k + 1) : ℚ) : ℝ) := by
  induction k with
  | zero =>
    refine ⟨by simp [laneA, laneAQ], ?_⟩
    show prodR (castV v) j = ((prodRQ v j : ℚ) : ℝ)
    exact prodR_cast v j
  | succ k ih =>
    obtain ⟨-, h2⟩ := ih
    refine ⟨h2, ?_⟩
    show rn (prodR (castV v) (8 * (k + 1) + j) + laneA (castV v) j (k + 1))
      = ((rnQ (prodRQ v (8 * (k + 1) + j) + laneAQ v j (k + 1)) : ℚ) : ℝ)
    rw [prodR_cast, h2, rn_add_castQ]

theorem laneA_cast (v : List (ℚ × ℚ)) (j k : ℕ) :
    laneA (castV v) j k = ((laneAQ v j k : ℚ) : ℝ) :=
  (laneA_pair v j k).1

theorem reduce8A_cast (v : List (ℚ × ℚ)) (E : ℕ) :
    reduce8A (fun j => laneA (castV v) j E) = ((reduce8AQ (fun j => laneAQ v j E) : ℚ) : ℝ) := by
  simp only [reduce8A, reduce8AQ, laneA_cast, rn_add_castQ]

theorem tailA_cast (v : List (ℚ × ℚ)) (m i : ℕ) (c : ℚ) :
    tailA (castV v) i m (c : ℝ) = ((tailAQ v i m c : ℚ) : ℝ) := by
  induction m generalizing i c with
  | zero => simp [tailA, tailAQ]
  | succ m ih =>
    show tailA (castV v) (i + 1) m (rn (prodR (castV v) i + (c : ℝ)))
      = ((tailAQ v (i + 1) m (rnQ (prodRQ v i + c)) : ℚ) : ℝ)
    rw [prodR_cast, rn_add_castQ]
    exact ih (i + 1) _

theorem dotCoreA_cast (v : List (ℚ × ℚ)) (E r : ℕ) :
    dotCoreA (castV v) E r = ((dotCoreAQ v E r : ℚ) : ℝ) := by
  unfold dotCoreA dotCoreAQ
  rw [reduce8A_cast, tailA_cast]

theorem dotAvx_cast (v : List (ℚ × ℚ)) : dotAvx (castV v) = ((dotAvxQ v : ℚ) : ℝ) := by
  unfold dotAvx dotAvxQ
  rw [castV_length, dotCoreA_cast]

/-! ## The NEON dot product (`dotNeon`) -/

/-- Lane `j` over `ℚ`, fused: each epoch rounds `x*y + s` once (floats_neon.c:120-132). -/
def laneNQ (v : List (ℚ × ℚ)) (j : ℕ) : ℕ → ℚ
  | 0 => 0
  | k + 1 => rnQ (feQ v (4 * k + j) + laneNQ v j k)

/-- The four lanes, summed in order (floats_neon.c:133-138). The model's leading `0 +` is left
out: it is exact on a rounded lane value, which `reduce4N_eq` shows on the real side. -/
def reduce4NQ (s : ℕ → ℚ) : ℚ := rnQ (rnQ (rnQ (s 0 + s 1) + s 2) + s 3)

def dotCoreNQ (v : List (ℚ × ℚ)) (E r : ℕ) : ℚ :=
  tailAuxQ v (4 * E) r (reduce4NQ (fun j => laneNQ v j E))

/-- The NEON dot product over `ℚ`. -/
def dotNeonQ (v : List (ℚ × ℚ)) : ℚ := dotCoreNQ v (v.length / 4) (v.length % 4)

theorem laneN_cast (v : List (ℚ × ℚ)) (j k : ℕ) :
    laneN (castV v) j k = ((laneNQ v j k : ℚ) : ℝ) := by
  induction k with
  | zero => simp [laneN, laneNQ]
  | succ k ih =>
    show rn (fe (castV v) (4 * k + j) + laneN (castV v) j k)
      = ((rnQ (feQ v (4 * k + j) + laneNQ v j k) : ℚ) : ℝ)
    rw [fe_castV, ih, rn_add_castQ]

theorem reduce4N_cast (v : List (ℚ × ℚ)) (E : ℕ) :
    reduce4N (fun j => laneN (castV v) j E) = ((reduce4NQ (fun j => laneNQ v j E) : ℚ) : ℝ) := by
  rw [reduce4N_eq]
  simp only [laneN_cast, reduce4NQ, rn_add_castQ]

theorem dotCoreN_cast (v : List (ℚ × ℚ)) (E r : ℕ) :
    dotCoreN (castV v) E r = ((dotCoreNQ v E r : ℚ) : ℝ) := by
  unfold dotCoreN dotCoreNQ
  rw [reduce4N_cast, tailAux_cast]

theorem dotNeon_cast (v : List (ℚ × ℚ)) : dotNeon (castV v) = ((dotNeonQ v : ℚ) : ℝ) := by
  unfold dotNeon dotNeonQ
  rw [castV_length, dotCoreN_cast]

end Gorse.Spec.DotRat
