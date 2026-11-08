# Verification-led modernization plan

Status: **design only, with decisions taken**. Nothing in this file is
implemented. The owner decided D-313 to D-316 on 2026-11-05; the decisions are
recorded in the "Decision record" section below. They are not yet copied
into the `PORTING.md` decision log. Where a later section still says
*proposed*, the decision record wins.

Base: `master` at `eccb0f2` (the D-312 verdict). The last head with the Go
oracle is the branch `go-oracle-final` at `2e9c969` (D-310), which still holds
the `conformance/` crate and `contracts/tools/`.

Rule for evidence. Every claim below cites a file and line in this tree, a
decision ID, or a primary source (listed in §7). Where an X/Q/O entry is
cited, the code it describes was read and is cited next to it. The entry's
summary was not taken on trust.

---

## Decision record (owner, 2026-11-05)

These are the owner's decisions on the plan's proposals. They are recorded
here and nothing has been implemented yet. They become `PORTING.md`
decision-log entries when the work starts.

| ID | Decision | What it means for the work |
|---|---|---|
| **D-313** | **Accepted:** the three-tier contract of §1 replaces "parity with Go". | Tier I exact theorems, tier II proven floating-point bounds, tier III pre-registered statistical bands. Each Rust build stays deterministic with a given seed (tier I). Equality with Go's numbers is no longer required. |
| **D-314** | **Accepted:** the scope of Go compatibility. **Kept:** reading Go-written stored data (model blobs, cache, database schemas) for one migration period, and the REST and gRPC message formats. **Dropped:** byte-for-byte imitation of Go, namely gob writing, Go's random number sequences, heap tie order and the quirks of Go's `exp` code. | §1.2's table holds as written. The Go replay checks (`crates/gorse-model/src/cf/replay.rs`) and the Go-recorded goldens for incidental items become parity-tier checks (G11). They are not per-commit requirements. |
| **D-315** | **Rulings on the spec questions of §3.2:** | |
| | A1 **Accepted:** metrics give *no value* instead of NaN. | `ndcg`, `precision`, `recall`, `map` and `mrr` return "no value" when undefined (empty ranked list, empty target set). `evaluate` averages only the defined values, and gives no value when no user has one. The config is checked for `top_k ≥ 1`. |
| | A4 **Confirm, then fix:** duplicate items in the candidates. | In Lean, prove both `ndcg_gt_one_of_dup` (duplicates can push NDCG above 1) and the bound under `Nodup`. Lean proves the mathematics only; until Aeneas links the Rust code (§2.3), it cannot show that duplicates actually reach the Rust evaluator. That needs a Rust test as well: a dataset with two positive feedback types for one (user, item) pair. If both confirm it, deduplicate the candidates in `evaluate`. |
| | A5 **Fix:** NaN scores in top-k. | Method as proposed: `TopKFilter` rejects NaN weights at the boundary and counts them, instead of keeping them where the heap happens to place them. |
| | A8 **Fix, with Lean:** the BPR gradient that becomes NaN without an error (F5). | Replace `model.rs:704` with the stable sigmoid (`exp` evaluated only at non-positive arguments). In Lean, prove that the stable form equals σ(−d) and lies in [0, 1] for every real d; the tier II f32 bound follows with T3. Add a regression test at d = −89, which gives NaN today. |
| | A9 **Decided: all items.** The eALS missing-data term sums over **all items**, as in He et al. 2016, not only the items with feedback. | Change the Gram matrix at `model.rs:1270` and fix the wrong Hu et al. citation at `model.rs:1091-1092`. ALS's numbers change, so the ALS Go replay must be re-baselined as a parity-tier check (D-314). The ml-1m band (`test_ccd_movielens`) must still pass. |
| | A2, A3, A6, A7, A10 to A15 | **Open.** The proposed rulings in §3.2 stand until the owner decides. |
| **D-316** | **Accepted: bring CI back** (this reverses D-312). | Step P0 commits the gate script and restores the GitHub Actions workflows: the gate stages of D-312, plus G1 to G4 once the Lean project exists. |
| — | **Out of scope, confirmed:** the expression checker and the HNSW index. | Neither is verified or reworked. HNSW keeps only its existing recall test. Revisit only if their production role changes (§5). |

---

## 0. Why the plan is needed now: what the tree shows

| # | Finding | Evidence |
|---|---|---|
| F1 | **The gate is not in the repository.** The D-311 and D-312 gates ran from `/home/user/court-scratch/gate18.log`, a path outside the tree. The stage list (FMT, CLIPPY, DOC, CONFIGGOLDEN, YAMLGOLDEN, SETUP, TEST, CLOUDCLIPPY, MONGOCLIPPY, VECTOR_ACCEPT, PROXY_STORE, REDIS_TLS_UNSET) is recorded only as prose. No script in `scripts/` defines it (`scripts/` holds only `setup-datasets.sh`). | `PORTING.md` D-311, D-312; `ls scripts` |
| F2 | **CI is gone.** All six workflows were removed by `5ca06a3`. D-312 records that nothing now builds images or release binaries. | D-312; `git show --stat 5ca06a3` |
| F3 | **Parity checks were removed and nothing replaced them.** TEST fell from 1471 (D-308) to 1371 (D-311) when `conformance/` was removed. That crate held `cf_golden.rs`, `ctr_replay.rs`, `recommend_oracle.rs`, `item_to_item_oracle.rs` and `rest_golden.rs` (`git ls-tree 2e9c969 conformance/tests`). X-253's reference value, Go's single-job ml-100k BPR recall "measured automatically in the conformance crate on every run", is no longer measured. The committed fixtures cannot be regenerated: `contracts/README.md` still documents `go run ./contracts/tools/...` commands for tools that were deleted (D-311 (e) is still open). | D-308, D-311; `contracts/README.md` "Regenerating" |
| F4 | **The score tolerance is a heuristic, not a bound.** D-234, D-236 and X-262 compare scores within `1e-6 * max(\|s\|, 1)`. For a dot product, the proven error is relative to `Σ\|xᵢyᵢ\|`, not to `\|s\|` (Higham 2002, §3.1). Under cancellation, two correct kernels can therefore differ by more than X-262 allows, and a wrong kernel on small vectors can stay inside it. | D-234, D-236; `crates/gorse-common/src/floats.rs:457-463`; `common/floats/src/floats_avx512.c:306-366` |
| F5 | **BPR's gradient is NaN for large negative margins.** The code is `grad = exp(-diff) / (1 + exp(-diff))` (`crates/gorse-model/src/cf/model.rs:704`). On amd64, `math32::exp` reproduces Go's assembly (`crates/gorse-common/src/math32.rs:251-260, 275-326`), which returns +Inf from x ≈ 88.376 (Q-77, asserted by `math32_amd64_differs_from_go_code`, `math32/golden_tests.rs:177`). IEEE 754-2019 §7.2 makes ∞/∞ invalid, so the result is NaN. NaN factors give NaN scores, and `TopKFilter` accepts NaN weights (`heap.rs:255-256`). Recommendations would then be wrong with no error raised. | as cited |
| F6 | **The ALS doc comment names the wrong paper.** The comment cites "the whole-data update of Hu, Koren and Volinsky" (`model.rs:1091-1092`). `solve_row` (`model.rs:1218-1255`) is the element-wise ALS (eALS) of He et al. 2016, with a uniform missing-data weight `c_i = α`. Its Gram matrix also sums only over rows with feedback (`model.rs:1270`), where He et al. sum over all items. | §2.T4 |
| F7 | **Results depend on the architecture.** The same seed gives different numbers at the AVX-512, AVX, NEON and scalar levels (`floats.rs:8-13`), and from `exp_amd64` versus `exp_go` (`math32.rs:251-260`). Parity with Go was only ever shown on the court's x86-64 machine. | as cited |

F1 to F3 mean the project has no reproducible way to show that it is
correct. F4 to F7 show that "parity with Go" was the wrong target for
the numeric code anyway.

---

## 1. The correctness contract (proposed D-313)

### 1.1 Three tiers

**Tier I: exact theorems (discrete code).**
A claim has the form "for all inputs satisfying P, output = spec(input)",
proved in Lean 4 against a spec over ℕ, ℤ, ℚ, `Finset` or `List`. The code is
linked to the spec by translation (§2.3). Proofs are checked in CI, mirrored
by proptest properties, and run as a differential oracle.

*Optimizations this permits:* any change that keeps the theorem. Examples:
replacing Go's sift order with a documented tie-break, then using
`select_nth_unstable` plus a sort or a SIMD top-k; sharded or parallel
top-k, justified by a merge theorem (T2); bitsets in place of `HashSet<i32>`
in the evaluator; removing the three `Vec` clones per BPR step
(`model.rs:706-708`), which does not change a single bit.

**Tier II: proven floating-point error bounds (numeric kernels).**
A claim has the form "for all finite binary32 inputs satisfying P (no
overflow), |computed − exact| ≤ B(inputs)". The proof is in Lean, against a
rounding model of IEEE 754-2019 binary32 with roundTiesToEven, and with
fusedMultiplyAdd (§5.4.1) where clang contracts (`crates/gorse-common/build.rs:7-12`).
The bound B is a closed formula. It replaces the tolerances in D-234, D-236 and X-262.
Two implementations agree if both satisfy their bound. In that case
|a − b| ≤ B_a + B_b, by the triangle inequality.

*Optimizations this permits:* reordering a reduction (other SIMD widths,
pairwise or tree sums), FMA, f64 accumulators, Rust `std::arch` kernels in
place of the C files and their clang requirement, BLAS or `faer` for
`mm`, and an `exp` or `log` with a stated ulp bound in place of Go's
assembly (Q-76, Q-77). The condition is that the new code carries a bound
proved in the same model.

**Tier III: pre-registered statistical bands (stochastic training).**
This covers BPR, ALS, FM, HNSW construction and TPE. A claim has the form "the
metric M of the procedure, over seeds drawn from a stated distribution, is
equivalent to a reference within δ". It is tested with TOST (Schuirmann 1987)
at a stated α and power, and the n, seeds, datasets and analysis are fixed in a
file committed **before** the run (§4, G12). A per-step tier I or tier II
theorem (T4) covers the update rule, and the band covers only what
randomness leaves open.

*Optimizations this permits:* a different random number generator (for example a
splittable PCG or xoshiro per thread), Hogwild or sharded parallel SGD, a
change of sampling scheme, mini-batches, early stopping, and parallel HNSW
insertion, provided the bands still pass.

**Determinism stays a tier I property, but only within Rust.** One Rust
build with one seed, one job count and one CPU level gives bit-identical
output. That is what makes regressions bisectable. Equality with Go's
streams is dropped.

### 1.2 Classifying the Go-isms

Decision for Ritchie first (*proposed D-314*). Must a Rust node still
interoperate with **upstream Go Gorse** at the pinned commit? The original
plan assumed mixed clusters ("None of them makes a mixed Go/Rust cluster
unsafe", `PORTING.md` decision-log preamble). D-305 then removed every trace
of Go. **Recommendation:** keep *read* compatibility with Go-written
persisted state for one migration window (blobs, cache, schemas), and keep
the client-facing message contract (REST and gRPC semantics). Drop byte-level
Go emulation. The table assumes this recommendation.

| Go-ism | Where | Class | Contract statement / what is freed |
|---|---|---|---|
| gob blobs | `encoding/gob.rs`, `contracts/model-blobs.md`, D-7 | **Contract for decoding, incidental for encoding** | Tier I: `decode` of every Go-written blob in the frozen fixtures (`gob_decode_golden.json`) gives the recorded value. Encoding may move to a versioned native format with a magic number. The bytes are already non-deterministic in Go (`model-blobs.md` lines 6-9), so byte equality was never a contract. |
| `util/rand` streams | `util/rand.rs` (D-9), `rand_golden.json`, `cf/replay.rs:23` | **Incidental** | Rust-internal determinism stays (tier I). The 1e-5 replay against Go (`replay.rs`) becomes a parity-tier check, run on demand (G11). This frees the RNG choice and allows per-thread splittable streams. |
| gotime | `gotime.rs`, `gotime/format.rs`, `dateparse.rs` (D-11) | **Contract for input languages, incidental for internals** | Tier I: the set of timestamp strings accepted by REST, config and import, and the instant each one parses to, must match the frozen fixtures `gotime_golden.json` and `dateparse_golden.json`. Output is RFC 3339. Go's layout engine and its error texts are incidental unless a REST route exposes them (ruling needed). |
| gostrconv | `gostrconv/*` (about 3.7k lines), O-92 | **Contract: round-trip only** | Tier I: for every finite f32 and f64, `parse(format(x))` is bit-equal to `x`, and the output follows the JSON number grammar. Go's choice of digits on a tie (O-92) is incidental. This allows deleting `gostrconv` in favour of the standard library's shortest round-trip formatting. |
| Cache keys and value encodings | `contracts/cache-keys.md` | **Contract** | Tier I, exact. Master, worker and server share Redis, and existing deployments keep their data across upgrades. |
| gRPC wire | `contracts/proto/*.proto` with `SHA256SUMS`, `gorse-protocol/src/wire.rs` | **Contract for message semantics, incidental for bytes** | Messages must decode to equal values under proto3 semantics. The −0.0 emission of `GoWireCodec` (O-93) is incidental. |
| REST | `contracts/rest/*.openapi.json` (D-15) | **Contract** | Routes, field names, types and status codes. Key order and whitespace are incidental. |
| Heap tie order | `heap.rs:6-8`, X-205 | **Incidental** | Replaced by a documented tie-break (weight descending, then id ascending) in the T2 theorem. |
| Go map iteration order | blobs, `consistent.rs:198` `hosts()` | **Incidental** | |
| `math32` assembly quirks | `math32.rs:275-326`, Q-76, Q-77 | **Incidental** | Replaced by a tier II `exp` with a stated ulp bound, which fixes F5. |
| SIMD level differences | `floats.rs:8-13`, `simd.rs` | **Incidental** | Tier II bound per level. |
| Consistent-hash ring (BLAKE2b, 10 points, `host{i}`) | `consistent.rs:1-40`, `worker.rs:611-621` | **Contract within one cluster version** | All workers in a cluster must compute the same ring, so a change needs a lockstep upgrade. Theorem T5. |
| HNSW quirks (one layer of growth per insert; a zero draw gives `MinInt64`) | `ann.rs:12-14, 198-200` | **Incidental** | HNSW is built only in tests at the pin (`ann.rs:5-6`). |
| Feedback-type expression text form | `expression.rs:11-14` (Q-46 to Q-49) | **Contract (fix)** | Values ≥ 1e6 print in a form that does not parse back. The contract should be round-trip, so this needs a ruling. |

---

## 2. Three-layer architecture

```
 S  Lean 4 + Mathlib spec over ℝ / ℚ / Finset        (what the paper says)
 │   refinement theorems: exact (tier I) or within B (tier II)
 A  Lean algorithm model: same loop order, binary32 rounding model
 │   translation (Aeneas) or bit-exact differential test (§2.3)
 C  Rust (and the C kernels) as shipped
```

### 2.1 Layer S: the specs, per target

Each spec file starts with the citation. A spec states what the paper
defines, and **where the code deviates, the spec carries a named
parameter**, not a silent choice. For example, eALS has a parameter `S`, the set
the missing-data term sums over: He et al. take all items, while the code at
`model.rs:1270` takes the items with feedback.

| Target | Spec content (ℝ unless stated) | Primary source |
|---|---|---|
| Ranking metrics | DCG, IDCG, NDCG, precision, recall, AP, RR and HR over `Finset ℕ × List ℕ`, with the counts in ℚ. The discount is `1/log₂(r+1)` | Järvelin & Kekäläinen 2002 for cumulated gain, but **the code follows the discount of Burges et al. 2005** (`evaluator.rs:96-107`). J&K with b = 2 do not discount ranks below b, so rank 2 is weighted 1, not 1/log₂3. The spec defines both and states which one Gorse uses. |
| Top-k | `IsTopK k w xs r`: r is a sub-multiset of xs, sorted by w descending, of length `min k \|xs\|`, and every dropped element weighs ≤ every kept one | Order theory only (no paper) |
| Dot / SIMD kernels | `Σᵢ xᵢyᵢ`, plus a rounding model with `u = 2⁻²⁴`, `η = 2⁻¹⁵⁰` and `γₙ = nu/(1−nu)` | IEEE 754-2019 §§3.3, 4.3.1, 5.4.1, 7; Higham 2002, §§2.2 and 3.1; Jeannerod & Rump 2013 for the sharper `nu` |
| BPR step | The objective per triple, `ln σ(x̂ᵤᵢⱼ) − (λ/2)(‖wᵤ‖²+‖hᵢ‖²+‖hⱼ‖²)`, and its gradient. LearnBPR samples (u,i,j) uniformly from D_S | Rendle et al. 2009, §§4.2-4.3 and Fig. 4. **The code's sampling is uniform over users, not over pairs** (`model.rs:686-700`), so the spec makes the sampling distribution an explicit parameter. |
| eALS step | The coordinate objective `Σ_{i∈Rᵤ}(1 − r̂ᵤᵢ)² + α Σ_{i∈S\Rᵤ} r̂ᵤᵢ² + λ‖pᵤ‖²` and its exact minimizer in `p_uf` | He et al. 2016, §4 (eALS) with uniform `cᵢ = α`. Hu et al. 2008 is cited only to record that it is **not** the variant implemented (F6). |
| FM interaction | `Σ_{i<j}⟨vᵢ,vⱼ⟩xᵢxⱼ = ½Σ_f[(Σᵢv_{if}xᵢ)² − Σᵢv_{if}²xᵢ²]` | Rendle 2010, Eq. 1 and Lemma 3.1; it is computed at `ctr/fm.rs:394-400`. The model is an attentional variant (`fm.rs:3-6`), so Xiao et al. 2017 must be checked against `fm.rs:96-130` before its spec is written. |
| HNSW | Layer draw `⌊−ln U · m_L⌋` with U ~ Unif(0,1]; invariants: the layers are nested, the degree is ≤ M (M₀ on layer 0), and neighbours lie on the same layer | Malkov & Yashunin 2020, Alg. 1 |
| TPE | Split at the γ-quantile into l(x) and g(x), Parzen mixtures (truncated Gaussians plus a prior), then pick argmax l/g among candidates | Bergstra et al. 2011, §4. The prior and "magic clip" come from hyperopt/goptuna (`tpe/parzen.rs:25-34`), not from the paper, and the spec must say so. |
| Bounded-load hashing | The capacity `⌈(1+ε)m/n⌉` | Mirrokni, Thorup & Zadimoghaddam 2018. **The code floors before it scales**: `⌈1.25·max(1,⌊(m+1)/n⌋)⌉` (`consistent.rs:217-243`). |

### 2.2 Layer A: linking the f32 algorithm to the spec

1. **The rounding model is defined in Lean, not assumed.** `rn : ℝ → ℝ`
   rounds to nearest, ties to even, onto the binary32 grid, with the
   subnormals. It is defined with `Int.log` and `zpow`. The core lemma is
   `rn x = x(1+δ) + ε` with `|δ| ≤ u`, `|ε| ≤ η` and `δε = 0`, for |x| below
   the overflow threshold, together with exactness of subnormal addition
   (Higham §2.2). Flocq (Boldo & Melquiond 2011) is the reference design.
   Only the subset needed here is ported, about 1.5k to 2k lines. Nothing is
   introduced with `axiom`.
2. **Each algorithm model copies the loop order of the code exactly**:
   `dotSeq` for `floats.rs:457-463`, which uses separate × and + because Rust does not
   contract; `dotAvx512` for the 16-lane FMA, the 4-level tree, the optional
   8-lane block and the contracted scalar tail of `floats_avx512.c:306-366`;
   then `dotAvx` and `dotNeon`.
3. **A computable twin is used for testing.** `rnQ : ℚ → ℚ` is computable and
   proved equal to `rn` on ℚ. The algorithm models then run exactly over ℚ
   and give the **correctly rounded bits** that the Rust and C code must
   reproduce (G9). IEEE operations are deterministic, so this comparison is
   bit for bit.
4. **FPTaylor and Gappa serve as cross-checks, not as proofs.**
   - FPTaylor (Solovyev et al. 2018) runs on the unrolled `dot` for n ∈ {8, 16, 32, 64},
     the defaults and tuning range of `n_factors` (`model.rs` `N_FACTORS` default
     16), over the input ranges observed after training. The Lean bound must be ≥
     FPTaylor's. If it is far above, the Lean bound is loose. If it is below,
     the model or the proof is wrong, and the release stops.
   - Gappa (de Dinechin, Lauter & Melquiond 2011) runs on the straight-line scalar
     formulas: the stable sigmoid for BPR and the eALS quotient
     `(a−b)/(c+αs_ff+λ)` (`model.rs:1249`). Gappa emits a Coq certificate, which
     the gate checks with `coqc`. This is a second, independent kernel for these
     formulas.

### 2.3 Layer C: linking the Rust code

| Tool | Where its proofs live | Fits here | Does not fit |
|---|---|---|---|
| **Aeneas** + Charon (Ho & Protzenko 2022) | **Lean 4**, the same logic and kernel as the spec | Safe Rust with loops and `&mut` (through backward functions): the heap, metrics, ring and index | `dyn` traits, `HashSet`, closures and concurrency are limited, and f32 support must be confirmed at the pin (the plan does not depend on it) |
| Verus (Lattuada et al. 2023) | SMT, with specs in Verus's own language | Fast proofs of discrete invariants in the crate | The spec would be a second copy, not the Lean one, and someone would have to show by hand that the two agree. Float support is weak. |
| Creusot (Denis, Jourdan & Marché 2022) | Why3 | Why3's `ieee_float` theory suits tier II | A third logic, and the Lean spec would have to be transcribed into it |
| hax (Cryspen) | Mainly F*; its Lean backend is newer | Pure, crypto-style code | `&mut`-heavy loops; Lean backend maturity |

**Recommendation: Aeneas for tiers I and II wherever it can translate the
code, with bit-exact differential testing (G9) as the link elsewhere.**

- *One logic and one kernel.* With Aeneas, the spec, the algorithm model and the translated
  code all live in Lean. One `#print axioms` and one lean4checker run then
  cover the whole chain. With Verus or Creusot, the step from the Lean spec to their spec
  is an untrusted hand transcription, which is exactly the "trust us"
  step the gate is meant to remove.
- *The cost is a refactor, not a rewrite.* The verified kernels move into a
  small crate (working name `gorse-kernels`) with no `dyn`, no `HashSet` and no
  closures. Production code calls that crate, so **the verified code is
  the shipped code**, and thin adapters convert at the boundary (for example
  `&HashSet<i32>` to a sorted slice or a bitset).
- *Fallback.* If Aeneas cannot translate f32 at the pinned version, the f32
  kernels are linked by G9 (exact `rnQ` model against Rust and C, bit for bit) and
  by Kani for the shape properties (no panic, bounds). The gap is listed in the
  trusted computing base. Verus remains the fallback for code Aeneas
  cannot take that still needs a proof (none among the picks).
- The **C kernels** (`common/floats/src/*.c`) are outside every Rust tool.
  Their numeric behaviour is linked by G9 at each CPU level. Their memory safety
  is covered by Kani on the 45 `unsafe` call sites in `floats/simd.rs` (with
  stubbed `extern` contracts) and by sanitizer runs (G7). Rewriting them in
  Rust `std::arch`, which tier II permits, would end this split.

---

## 3. Triage

Cost is measured in engineer-weeks for one engineer fluent in Lean/Mathlib and Rust.
"Bug value" counts what the spec work has **already** surfaced, plus how
much production traffic the code sees. "Optimization value" is what the
proved contract unlocks.

| Rank | Target | Production use | Cost | Bug value | Optimization value | Pick |
|---|---|---|---|---|---|---|
| 1 | Evaluator metrics (`cf/evaluator.rs`) | Model selection: `History`, TPE objective (`model.rs:587-611`) | 3 | **High**: NaN cases, the Burges/J&K discount, two denominator variants, and a suspected NDCG > 1 from duplicate items | Medium: bitset membership, exact counts | **T1** |
| 2 | Top-k / heap (`heap.rs`) | Every recommend path: `logics/recommend.rs:479,543`, `non_personalized.rs:171,229`, `server/rest/recommend.rs:613`, `evaluator.rs:183` | 4 | Medium: NaN accepted here while `PriorityQueue` panics on it (`heap.rs:153-155` versus `255-256`) | **High**: a non-Go tie-break, partial selection, sharded merge | **T2** |
| 3 | Dot and SIMD kernels (`floats.rs`, `simd.rs`, `*.c`) | Every score and every training step | 8 | **High**: F4, F7, and `int epoch = n / 16` truncating an `int64_t` (`floats_avx512.c:20,53`; `floats_avx.c:50`) | **High**: no clang requirement, Rust SIMD, FMA, BLAS | **T3** |
| 4 | BPR and eALS update rules (`cf/model.rs`) | Training | 6 | **High**: F5, F6, the sampling difference, and a negative-sampling loop that never ends for a user who has every item (`model.rs:696-701`) | **High**: tier III unlocks a new RNG and parallelism | **T4** |
| 5 | Consistent-hash ring (`consistent.rs`) | `worker.rs:611-621` (only `new`, `add`, `get`) | 1.5 | Medium: on a 64-bit point collision, `host_at` returns `""` (`consistent.rs:137-143`), so **every** worker skips that user | Low | **T5** (optional) |
| 6 | Codecs and varint (`cf/model.rs:486-505`, `nn/codec.rs`, `gob.rs`) | Blob decoding | 1 | Low: well understood; it matches Go, including the non-canonical forms Go accepts | Low | Proptest and cargo-fuzz only |
| 7 | `unified_index` | FM only | 1 | Low-medium: i32 sum overflow (`unified_index.rs:316-322`); the direct index orders label ranges differently from the map index (`:453-466` versus `:344-372`), in test code only | Low | Kani overflow proof only |
| 8 | Expression checker (`gorse-expr`, about 14k lines, expr-lang v1.17.8) | User config expressions | 20+ | Medium | Low | **Out of scope (owner, 2026-11-05)** |
| 9 | HNSW invariants (`ann.rs`) | **Tests only at the pin** (`ann.rs:5-6`) | 6 | Low | Low | **Out of scope (owner, 2026-11-05)**. The existing recall test stays. |

### 3.1 The theorems for the picks

Lean signatures are against Mathlib (pinned in `lake-manifest.json`).
`Extracted.*` is Aeneas output, and `.ok` is Aeneas's `Result` constructor.

#### T1. Ranking metrics (tier I on counts, tier II on the f32 value)

In plain English:
1. If the target set is non-empty, the ranked list is non-empty and **has no
   duplicates**, then NDCG is defined and lies in [0, 1].
2. Under the same conditions, NDCG = 1 exactly when the first `min(|T|, |L|)`
   entries are all relevant.
3. Duplicates break 1: there are T and L with NDCG > 1. This theorem justifies the
   precondition and documents the suspected bug.
4. Precision ≤ 1 always. Recall ≤ 1 and AP ≤ 1 when L has no duplicates.
5. The Rust f32 NDCG is within relative error `κ(|L|)` of the real value.
6. `evaluate` returns the mean over users whose metric is defined, and
   `none` when there is no such user. Today it returns NaN (`evaluator.rs:90`).

```lean
namespace Gorse.Spec.Metrics
open Finset

noncomputable def disc (i : ℕ) : ℝ := 1 / Real.logb 2 (i + 2)  -- Burges et al. 2005
noncomputable def dcg  (T : Finset ℕ) (L : List ℕ) : ℝ :=
  ∑ i ∈ range L.length, if L[i]! ∈ T then disc i else 0
noncomputable def idcg (T : Finset ℕ) (n : ℕ) : ℝ := ∑ i ∈ range (min T.card n), disc i
noncomputable def ndcg (T : Finset ℕ) (L : List ℕ) : Option ℝ :=
  if T.card = 0 ∨ L = [] then none else some (dcg T L / idcg T L.length)

theorem ndcg_mem_unitInterval {T : Finset ℕ} {L : List ℕ}
    (hT : T.Nonempty) (hL : L ≠ []) (hnd : L.Nodup) :
    ∃ v, ndcg T L = some v ∧ v ∈ Set.Icc (0 : ℝ) 1
theorem ndcg_eq_one_iff {T L} (hT : T.Nonempty) (hL : L ≠ []) (hnd : L.Nodup) :
    ndcg T L = some 1 ↔ ∀ i < min T.card L.length, L[i]! ∈ T
theorem ndcg_gt_one_of_dup : ∃ (T : Finset ℕ) (L : List ℕ) (v : ℝ),
    ¬ L.Nodup ∧ ndcg T L = some v ∧ 1 < v          -- witness: T = {1}, L = [1, 1]

def hits (T : Finset ℕ) (L : List ℕ) : ℕ := (L.filter (· ∈ T)).length
def precision (T : Finset ℕ) (L : List ℕ) : Option ℚ :=
  if L = [] then none else some (hits T L / L.length)
theorem precision_le_one {T L v} : precision T L = some v → v ≤ 1
theorem recall_le_one {T L v} (hnd : L.Nodup) : recall T L = some v → v ≤ 1

-- link (tier II): the Rust function on binary32, through the Layer A model
theorem ndcg32_rel_err {T L v} (hT : T.Nonempty) (hL : L ≠ []) (hnd : L.Nodup)
    (hv : ndcg T L = some v) :
    |Algo.ndcg32 T L - v| ≤ κ L.length * v
theorem extracted_ndcg_refines (T L) :
    Extracted.evaluator.ndcg (enc T) (enc L) = .ok (Algo.ndcg32 T L)  -- if Aeneas takes f32
```

`κ n` is fixed by the proof. It is expected near `(2n+4)u`, from the f64
`log2` rounded to f32 (`evaluator.rs:114-116`) plus two sums of length ≤ n
and one division.

**Status of the f32 link (D-328).** `extracted_ndcg_refines` is not available: f32
is out of verified scope at Aeneas c2d2343, and the pin is not bumped. So the P2
link for f32 is tier II under the G9 fallback of §2.3. That fallback is an exact model
of the f32 kernels, checked bit for bit against the Rust and C code, with Kani for the
shape properties. Neither check is built yet for f32: the G9 check of D-326 covers only
precision, recall, hr and mrr. The tier II work that owns f32, `map` and `ndcg` is
P4 (T3 rounding model and dot), which
builds the f32 rounding model, the fixed summation order that `map` needs, and the
proven bound that `ndcg` needs (D-326).

#### T2. Top-k

In plain English:
1. Pushing a NaN-free list into `TopKFilter k` and popping all elements
   returns a top-k: the right length, sorted descending, and nothing dropped
   outweighs anything kept.
2. All top-k results of one input have the same sequence of weights. Only
   the elements tied with the k-th weight can differ.
3. **Merge:** a top-k of (top-k of A) ++ (top-k of B) is a top-k of A ++ B.
   This licenses sharded and parallel recommendation.
4. With the proposed tie-break (weight descending, then id ascending), the result is unique.

```lean
namespace Gorse.Spec.TopK
variable {α W : Type} [LinearOrder W] (w : α → W)

def IsTopK (k : ℕ) (xs r : List α) : Prop :=
  (r : Multiset α) ≤ xs ∧ r.length = min k xs.length ∧
  (r.map w).Sorted (· ≥ ·) ∧
  ∀ a ∈ (xs : Multiset α) - r, ∀ b ∈ r, w a ≤ w b

theorem isTopK_weights_unique {k xs r r'} (h : IsTopK w k xs r) (h' : IsTopK w k xs r') :
    r.map w = r'.map w
theorem isTopK_merge {k xs ys rx ry r} (hx : IsTopK w k xs rx) (hy : IsTopK w k ys ry)
    (h : IsTopK w k (rx ++ ry) r) : IsTopK w k (xs ++ ys) r
theorem topKFilter_correct (k : Usize) (xs : List (I32 × NoNaN F32)) :
    ∃ r, Extracted.heap.topk_push_all_pop_all k xs = .ok r ∧ IsTopK Prod.snd k.val xs r
```

`NoNaN F32` is a subtype. A ruling is needed on what happens to a NaN at the
boundary: reject it, as `PriorityQueue` does (`heap.rs:153-155`), or map it to
−∞.

#### T3. Dot kernels (tier II)

In plain English:
1. The scalar Rust dot of n binary32 pairs differs from the exact sum by at
   most `γₙ · Σ|xᵢyᵢ| + n·η`, if nothing overflows.
2. The AVX-512 kernel satisfies the same with `γ_{d(n)}`, where `d(n)` is the
   longest chain of roundings in its order: for n = 16 that is 1 multiply and
   4 tree adds, so d = 5.
3. Any two levels differ by at most the sum of their bounds. That bound
   **replaces X-262**. For n = 16, γ₅ + γ₁₆ ≈ 1.25e-6 · Σ|xᵢyᵢ|.

```lean
namespace Gorse.FP
def u : ℝ := 2 ^ (-24 : ℤ)
def η : ℝ := 2 ^ (-150 : ℤ)
noncomputable def γ (n : ℕ) : ℝ := n * u / (1 - n * u)
noncomputable def rn : ℝ → ℝ              -- RNE onto binary32, defined (no axiom)
def IsF32 (x : ℝ) : Prop
theorem rn_model {x : ℝ} (h : |x| < ovfThreshold) :
    ∃ δ ε, rn x = x * (1 + δ) + ε ∧ |δ| ≤ u ∧ |ε| ≤ η ∧ δ * ε = 0

noncomputable def dotSeq    : List (ℝ × ℝ) → ℝ   -- floats.rs:457-463
noncomputable def dotAvx512 : List (ℝ × ℝ) → ℝ   -- floats_avx512.c:306-366
def depthAvx512 : ℕ → ℕ

theorem dotSeq_err (v : List (ℝ × ℝ)) (hF : ∀ p ∈ v, IsF32 p.1 ∧ IsF32 p.2)
    (hn : v.length * u < 1) (hov : NoOverflow v) :
    |dotSeq v - (v.map fun p => p.1 * p.2).sum|
      ≤ γ v.length * (v.map fun p => |p.1 * p.2|).sum + v.length * η
theorem dotAvx512_err (v) (hF) (hn) (hov) :
    |dotAvx512 v - (v.map fun p => p.1 * p.2).sum|
      ≤ γ (depthAvx512 v.length) * (v.map fun p => |p.1 * p.2|).sum + v.length * η
theorem dot_levels_close (v) (hF) (hn) (hov) :
    |dotAvx512 v - dotSeq v|
      ≤ (γ (depthAvx512 v.length) + γ v.length) * (v.map fun p => |p.1 * p.2|).sum
        + 2 * v.length * η
```

#### T4. The BPR and eALS steps

In plain English:
1. Over ℝ, the code's BPR step (`model.rs:702-722`) is exactly one
   gradient-ascent step of size `lr` on Rendle's per-triple objective, for
   each of `wᵤ`, `hᵢ` and `hⱼ`, with the old values used for all three.
2. The **stable** sigmoid (to be written as part of this target) is finite and
   in [0, 1] for every finite binary32 margin, and within `c·u` of σ(−d),
   given an `exp` with a stated ulp bound. The current form fails at d = −89, which is a
   unit test, not a theorem (F5).
3. Over ℝ, the eALS coordinate update (`model.rs:1249`) is the unique
   minimizer of the coordinate objective with missing-data set `S`, if
   `Rᵤ ⊆ S`, λ > 0 and α ≥ 0. The code uses S = the items with feedback;
   He et al. use all items. The theorem holds for both, and the ruling picks one.
4. Loop invariant: after each coordinate update, `predictions[i]` equals
   `⟨row, others[i]⟩` (exactly over ℝ, and within a stated drift bound in f32).
   **Status (D-361):** proved over ℝ (`Gorse.Spec.SolveRow.solveRow_invariant`). The f32 drift bound stays open under D-328 and D-361.

```lean
namespace Gorse.Spec.CF
variable {k : ℕ}
local notation "E" => EuclideanSpace ℝ (Fin k)
noncomputable def σ (x : ℝ) : ℝ := 1 / (1 + Real.exp (-x))
noncomputable def bprObj (reg : ℝ) (w hi hj : E) : ℝ :=
  Real.log (σ (inner w (hi - hj))) - reg / 2 * (‖w‖ ^ 2 + ‖hi‖ ^ 2 + ‖hj‖ ^ 2)
noncomputable def bprStep (lr reg : ℝ) (w hi hj : E) : E × E × E   -- model.rs:702-722 over ℝ

theorem bprStep_user (lr reg : ℝ) (w hi hj : E) :
    (bprStep lr reg w hi hj).1 = w + lr • gradient (fun w' => bprObj reg w' hi hj) w
theorem bprStep_pos  ... : (bprStep lr reg w hi hj).2.1 = hi + lr • gradient (fun h => bprObj reg w h hj) hi
theorem bprStep_neg  ... : (bprStep lr reg w hi hj).2.2 = hj + lr • gradient (fun h => bprObj reg w hi h) hj

variable {ι : Type} [Fintype ι] [DecidableEq ι]
noncomputable def ealsJ (α λ : ℝ) (R S : Finset ι) (Q : ι → Fin k → ℝ) (p : Fin k → ℝ) : ℝ :=
  ∑ i ∈ R, (1 - ∑ f, p f * Q i f) ^ 2 + α * ∑ i ∈ S \ R, (∑ f, p f * Q i f) ^ 2
    + λ * ∑ f, p f ^ 2
noncomputable def ealsCoord (α λ : ℝ) (R S : Finset ι) (Q) (p : Fin k → ℝ) (f : Fin k) : ℝ
  -- (a − b) / (c + α s_ff + λ), model.rs:1239-1249
theorem ealsCoord_isMinOn {α λ R S Q p f} (hRS : R ⊆ S) (hλ : 0 < λ) (hα : 0 ≤ α) :
    IsMinOn (fun t => ealsJ α λ R S Q (Function.update p f t)) Set.univ (ealsCoord α λ R S Q p f)
theorem ealsCoord_unique ... : ∀ t, ealsJ ... (update p f t) = ealsJ ... (update p f (ealsCoord ...)) → t = ealsCoord ...
```

Statistical bands (tier III, §4 G12) cover what the theorems leave open:
whole training runs.

#### T5 (optional). The worker ring partitions the users

In plain English: if the 10·n ring points are distinct, then every key has
exactly one owner among the hosts; the owner does not depend on the order in
which hosts are added; every user is pulled by exactly one worker when all
workers see the same peer set; and adding a host moves only the keys it takes over.

```lean
namespace Gorse.Spec.Ring
variable (h : String → UInt64)   -- BLAKE2b-512 prefix, abstract
def points (hosts : List String) : List (UInt64 × String) :=
  hosts.flatMap fun s => (List.range 10).map fun i => (h (s ++ toString i), s)
def owner (hosts : List String) (key : String) : Option String
theorem owner_mem (hne : hosts ≠ []) (hinj : ((points h hosts).map Prod.fst).Nodup) (key) :
    ∃ s ∈ hosts, owner h hosts key = some s
theorem owner_perm (hp : hosts.Perm hosts') (hinj) : owner h hosts = owner h hosts'
theorem pull_partition (hinj) (hd : hosts.Nodup) (users : List String) :
    users.Perm (hosts.flatMap fun s => users.filter (owner h hosts · = some s))
theorem owner_add_moves_only_to_new (hinj : ((points h (q :: hosts)).map Prod.fst).Nodup) (key) :
    owner h (q :: hosts) key ≠ owner h hosts key → owner h (q :: hosts) key = some q
```

The bounded-load half is not taken: Gorse never calls it (`worker.rs:611-621`).

### 3.2 Spec ambiguities that need a ruling (*proposed D-315*)

| # | Ambiguity | Evidence | Proposed ruling |
|---|---|---|---|
| A1 | `ndcg` returns NaN when the ranked list is empty (`0/0`), and so does `precision`. `evaluate` with zero users computes `sum × (1/0)`, which is `0·∞ = NaN` | `evaluator.rs:108,126,90` | Return `Option`. Average only the defined values. Validate `top_k ≥ 1` in the config. |
| A2 | DCG discount: J&K 2002 versus Burges 2005 | `evaluator.rs:96-107` | Keep Burges and name it. |
| A3 | Precision divides by \|L\|, not k; AP divides by \|T\|, not min(\|T\|, k) | `evaluator.rs:126,161` | Keep, and name them `P@|L|` and `AP/|T|` in the spec. |
| A4 | Duplicate items in the candidates can make NDCG > 1. `add_feedback` does not deduplicate, the loader adds one feedback per row (several positive types give several rows), and the candidates are `feedback ++ negatives` | `dataset.rs:311-319`, `tasks/load.rs:593-597`, `evaluator.rs:66-69` | Confirm with a proptest first. If confirmed, deduplicate the candidates (fix). |
| A5 | NaN weights: `TopKFilter` keeps them, `PriorityQueue` panics | `heap.rs:153-155, 255-256` | Reject at the boundary, with an error metric. |
| A6 | BPR sampling is uniform over users, while Rendle's is uniform over pairs | `model.rs:686-694` | Keep, as a stated parameter. |
| A7 | BPR negative sampling never ends for a user who has interacted with every item | `model.rs:696-701` | Precondition, plus a guard. |
| A8 | The BPR gradient becomes NaN (F5) | `model.rs:704`, `math32.rs:275-326` | Stable sigmoid (fix). |
| A9 | eALS: wrong citation; Gram over the items with feedback; "Alpha" means the missing-data weight, unlike Hu et al.'s confidence α | `model.rs:1091-1092, 1151, 1270` | Fix the doc comment. Rule on `S`. |
| A10 | HNSW: a zero uniform draw gives layer `MinInt64`; the graph grows by one layer at most per insert | `ann.rs:198-200, 12-14` | Draw from (0, 1]. Remove the growth cap if HNSW goes to production. |
| A11 | The bounded-load capacity floors before it scales, unlike the paper | `consistent.rs:217-243` | Moot: the code is unused. |
| A12 | A ring point collision gives owner `""`, and the user is dropped | `consistent.rs:137-143` | Detect the collision in `add` and fail. |
| A13 | `unified_index` length is a sum of i32 values that can overflow; the direct index orders label ranges differently from the map index | `unified_index.rs:316-322, 344-372, 453-466` | Checked arithmetic; document that the direct index is for tests only. |
| A14 | The feedback-type expression text form does not round-trip at ≥ 1e6 | `expression.rs:11-14` | Round-trip contract (fix). |
| A15 | Results depend on the architecture (F7) | `floats.rs:8-13`, `math32.rs:251-260` | Tier II bound per level. Determinism holds only per level. |

**T4 item 4 status (D-361).** The `solve_row` loop invariant `predictions[i] = ⟨row, others[i]⟩` is proved over ℝ (`Gorse.Spec.SolveRow.solveRow_invariant`). The f32 drift bound stays open under D-328 and D-361.

---

## 4. Evidence: the gate (*proposed D-316*)

Principle: every stage can be rerun by a third party from the repository
alone, on pinned toolchains, and its pass criterion is mechanical. What
remains a matter of human judgment, namely whether the statements say the
right thing, is isolated in G4 and G13.

| Stage | What runs | Pass criterion | What it does **not** show |
|---|---|---|---|
| **G0 Gate in the repository** | A committed gate script reproducing the D-312 stage list, in a container pinned by digest. CI restored, which reverses D-312 and is Ritchie's call; otherwise the signed gate logs are committed under `porting/evidence/` | It reproduces D-312 on `eccb0f2`: TEST 1371 passed / 0 failed / 41 ignored; REDIS_TLS_UNSET exits 101 | Anything new |
| **G1 Lean build** | `lake build` with Lean and Mathlib pinned | 0 errors; 0 `sorry` or `admit`; `#print axioms` on every exported theorem lists only `propext`, `Classical.choice` and `Quot.sound`; no `native_decide` (which adds `Lean.ofReduceBool`), no `implemented_by` or `extern` in spec modules, no `axiom` declarations | That the statements are right |
| **G2 Independent check** | lean4checker replays the environment. A second, independent kernel (nanoda, a Rust implementation of the Lean 4 kernel) checks the export | Both accept | Bugs shared by both kernels; the Mathlib definitions used |
| **G3 Aeneas regenerated** | Pinned Charon and Aeneas regenerate the Lean code for `gorse-kernels`; then `git diff --exit-code`; the proofs are rebuilt | No diff; the proofs pass | That Aeneas translates correctly (trusted; G9 is the check) |
| **G4 Statement freeze** | A hash of each theorem's *statement*, stored in a manifest | Any change to a statement needs a matching decision-log ID in the same commit | — |
| **G5 Miri** | `cargo miri test` on `gorse-common` and `gorse-storage`, with the floats routed to scalar under `cfg(miri)` | No undefined behaviour reported | The FFI bodies: Miri cannot run the C kernels or SQLite |
| **G6 Kani** | One harness per `unsafe` block: 45 in `floats/simd.rs`, 10 in `data/sql/sqlite.rs`. The `extern` functions are stubbed with contracts ("reads and writes exactly n elements"). Also overflow proofs for `unified_index` and `read_uvarint` | Every harness verifies within its bounds | Anything past the bound; the C bodies |
| **G7 C kernels** | ASan and UBSan builds of `floats_*.c`, every n in [0, 4096], guard pages; CBMC where the intrinsics are modelled | Clean | The C kernels are not proved; ended by a Rust rewrite |
| **G8 proptest mirrors** | One property per Lean theorem, with the same name (for example `thm_ndcg_mem_unitInterval`); ≥ 10⁵ cases; regressions committed | Pass | A proof (it is a sample) |
| **G9 Lean as oracle** | The compiled Lean executable reads JSON cases. Discrete parts are computed exactly; floating-point models exactly over ℚ with `rnQ`. Rust and C results are compared: bit for bit against the Layer A model, and within the proven B against the spec. 10⁶ cases per target, including adversarial generators (cancellation, subnormals, near overflow), at every CPU level the runner has | 0 mismatches | Inputs not sampled |
| **G10 cargo-mutants** | On the verified modules | Every non-equivalent mutant is killed; each equivalent mutant is listed with a one-line argument. Crate-level baseline recorded, never to fall. This systematizes the hand mutation in D-234. | — |
| **G11 Parity tier** (per release, not per commit) | Hermetic build of `go-oracle-final` at `2e9c969` with go1.27.0 (`PORTING.md` "Upstream pin"); the frozen differential suites from `conformance/tests` run, for the **contract** rows of §1.2 only (REST golden, cache keys, blob decode, timestamp parsing) | 0 unexplained differences, the D-309 standard | Any tier II or III code |
| **G12 Statistical tier** | Pre-registered under `porting/prereg/<target>.md`, committed before the run (the git timestamp is the evidence) | Every band passes at its stated α | — |
| **G13 Spec audit** | An outside reviewer gets only the `Spec/*.lean` statements, §7's papers and §1 | A signed list of discrepancies, each one resolved by a decision ID | — |

**G12 designs.** The numbers are planning estimates. A pilot re-estimates σ,
and the pilot itself is pre-registered.

- *BPR and ALS*, on ml-100k and ml-1m (the `cf/model.rs` tests at 1542 use ml-1m
  with `BENCH_DELTA = 0.01`). Metric: recall@10 and NDCG@10. Reference: 30 runs
  of the reference build (`go-oracle-final` for the first baseline, then a frozen
  Rust release). Test: TOST with δ = 0.01, the X-253 band kept. σ ≈ 0.0056 is
  estimated from the Q-269 range of 0.7582 to 0.7805 over 24 Go runs (range/3.9).
  With α = 0.002 per test and power 0.9, n ≈ 13 runs per arm. This **replaces X-253's
  "12 of 12 inside ±0.01"**, which has no stated error rate: a build whose runs
  land inside the band with p = 0.9 passes 12 of 12 with probability 0.9¹² ≈ 0.28.
- *FM* on frappe: AUC, TOST with δ = 0.005, the same α.
- *HNSW*: out of scope (decision record). The existing single-build test
  asserting `r > 0.99` (`ann/tests.rs:537-564`) stays as it is.
- *TPE*: on a fixed synthetic objective, 50 seeds, a sign test that the best
  after 30 trials beats random search, at α = 0.002. Plus equivalence with the
  goptuna reference built from `go-oracle-final`.
- *Family-wise*: 4 tests (BPR, ALS, FM, TPE) at 0.002 give ≤ 0.8% false alarms per gate run
  (Bonferroni). No optional stopping: n is fixed unless a group-sequential design is
  pre-registered.

**Trusted computing base, stated.** The Lean kernel (mitigated by G2); the
Mathlib definitions used in statements (G13); Charon and Aeneas (G9); rustc
and LLVM codegen, and Rust's IEEE semantics for f32 (the Rust reference and
RFC 3514; mitigated by G9 on compiled binaries); clang's `-ffp-contract=on`
behaviour and the hardware's IEEE conformance (the `floats_golden.json` bit tests
and G9); the C kernels (G6 and G7 only); and BLAKE2b, which is abstract in T5.

---

## 5. Budget, order of work, stopping rule

| Step | Work | Engineer-weeks | Exit |
|---|---|---|---|
| **P0** | **Restore the gate** (G0): a committed script, a pinned container, the hermetic `go-oracle-final` build (G11), and CI if Ritchie reverses D-312. Fix D-311 (e) so that `contracts/README.md` stops naming the deleted tools | 2 | The D-312 numbers reproduced from the repository alone |
| P1 | Rulings: D-313 to D-316 and A1 to A15. Lean project with its pins; G1, G2 and G4 wired; Charon and Aeneas pinned; the `gorse-kernels` crate created with no change in behaviour (gate still green, `floats_golden` and the replay still pass) | 2.5 | Rulings logged; empty proof scaffold green |
| P2 | **T1 metrics** (spec 0.5, proofs 1, refactor and link 1, G8 to G10 0.5) | 3 | G1 to G10 green for T1 |
| P3 | **T2 top-k** (Aeneas loop proofs dominate) | 4 | G1 to G10 green for T2; tie-break switched |
| P4 | **T3 rounding model and dot** (`rn` and its model 3, `dotSeq` 1, SIMD models 2, FPTaylor and Gappa 1, Kani and C 1) | 8 | X-262 replaced by the proven bound |
| P5 | **T4 BPR and eALS** (calculus 2, eALS argmin 1.5, stable sigmoid with Gappa 1, G12 pre-registration and runs 1.5) | 6 | F5 and F6 fixed; bands pass |
| P6 | T5 ring (optional) | 1.5 | — |
| P7 | G13 outside audit (2-3 weeks of calendar time) | 1 internal | Discrepancies resolved |
| | **Total** | **≈ 28** (≈ 42 with 1.5× contingency) | |

Why this order. P0 comes first because without a reproducible gate no later
claim can be checked (F1 to F3). T1 and T2 are cheap, they use the tier I
toolchain, and they test whether Aeneas fits before the expensive
floating-point work. T3 must come before T4, because T4's tier II lemmas
reuse `rn`.

**Stopping rule.**
- *Per target.* A target is done when G1 to G10 are green for it. If its
  spend reaches 1.5× the estimate before its keystone lemma is proved
  (T1: `ndcg_mem_unitInterval`; T2: `topKFilter_correct`; T3: `rn_model`; T4:
  `ealsCoord_isMinOn`), the target drops to spec plus G8 and G9 without a proof,
  and §1 records the downgrade.
- *Program.* Stop after T4 (T5 is optional). Stop earlier if T1 and T2
  **together** find no bug or spec issue beyond A1 to A5 *and* unlock no
  optimization: verification would then not be paying for itself, and
  T3 and T4 would fall back to G9 and G12 alone. Pause the program if the G13
  audit finds that the specs misstate intent in more than three places, and
  fix the specs first.
- *No scope creep.* The expression checker and HNSW stay out unless their
  production role changes, for example if HNSW replaces the exact stores of
  D-6 and X-205.
- *Statistics.* Stop each band at its pre-registered n. Never extend a band
  after seeing the data.

---

## 6. What this plan deliberately does not do

- It does not restore byte-level parity with Go for incidental rows of §1.2.
- It does not prove the C kernels. G6 and G7 bound their memory behaviour,
  G9 their numerics, and P4 makes a Rust rewrite possible.
- It writes no code before P0 and P1 are accepted.

---

## 7. Primary sources

- IEEE Std 754-2019, *IEEE Standard for Floating-Point Arithmetic*.
- S. Rendle, C. Freudenthaler, Z. Gantner, L. Schmidt-Thieme. *BPR: Bayesian Personalized Ranking from Implicit Feedback.* UAI 2009, 452-461.
- Y. Hu, Y. Koren, C. Volinsky. *Collaborative Filtering for Implicit Feedback Datasets.* ICDM 2008, 263-272.
- X. He, H. Zhang, M.-Y. Kan, T.-S. Chua. *Fast Matrix Factorization for Online Recommendation with Implicit Feedback.* SIGIR 2016, 549-558.
- S. Rendle. *Factorization Machines.* ICDM 2010, 995-1000.
- J. Xiao, H. Ye, X. He, H. Zhang, F. Wu, T.-S. Chua. *Attentional Factorization Machines.* IJCAI 2017.
- Yu. A. Malkov, D. A. Yashunin. *Efficient and Robust Approximate Nearest Neighbor Search Using Hierarchical Navigable Small World Graphs.* IEEE TPAMI 42(4), 2020, 824-836 (arXiv:1603.09320).
- J. Bergstra, R. Bardenet, Y. Bengio, B. Kégl. *Algorithms for Hyper-Parameter Optimization.* NeurIPS 2011.
- K. Järvelin, J. Kekäläinen. *Cumulated Gain-Based Evaluation of IR Techniques.* ACM TOIS 20(4), 2002, 422-446.
- C. Burges et al. *Learning to Rank using Gradient Descent.* ICML 2005.
- V. Mirrokni, M. Thorup, M. Zadimoghaddam. *Consistent Hashing with Bounded Loads.* SODA 2018, 587-604.
- N. J. Higham. *Accuracy and Stability of Numerical Algorithms*, 2nd ed. SIAM 2002 (§2.2, §3.1).
- C.-P. Jeannerod, S. M. Rump. *Improved Error Bounds for Inner Products in Floating-Point Arithmetic.* SIAM J. Matrix Anal. Appl. 34(2), 2013.
- S. Boldo, G. Melquiond. *Flocq: A Unified Library for Proving Floating-Point Algorithms in Coq.* ARITH 2011.
- A. Solovyev, M. S. Baranowski, I. Briggs, C. Jacobsen, Z. Rakamarić, G. Gopalakrishnan. *Rigorous Estimation of Floating-Point Round-Off Errors with Symbolic Taylor Expansions.* ACM TOPLAS 41(1), 2018.
- F. de Dinechin, C. Lauter, G. Melquiond. *Certifying the Floating-Point Implementation of an Elementary Function Using Gappa.* IEEE TC 60(2), 2011.
- S. Ho, J. Protzenko. *Aeneas: Rust Verification by Functional Translation.* ICFP 2022.
- A. Lattuada et al. *Verus: Verifying Rust Programs using Linear Ghost Types.* OOPSLA 2023.
- X. Denis, J.-H. Jourdan, C. Marché. *Creusot: A Foundry for the Deductive Verification of Rust Programs.* ICFEM 2022.
- D. J. Schuirmann. *A Comparison of the Two One-Sided Tests Procedure and the Power Approach for Assessing the Equivalence of Average Bioavailability.* J. Pharmacokinet. Biopharm. 15(6), 1987.
