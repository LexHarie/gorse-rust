# WP-1-11 notes, part 1: chewxy/math32

Provisional ids; the integrator renumbers them into `PORTING_NOTES.md` and
`PORTING.md`. The Rust module is `crates/gorse-common/src/math32.rs` (tests in
`src/math32/`), porting `github.com/chewxy/math32` at the version Gorse's
`go.mod` pins, v1.11.2. The Go oracle data is
`crates/gorse-common/tests/data/math32_golden.json`, written by
`contracts/tools/math32golden`. This part is pushed ahead of `common/nn`
because WP-1-10 needs `Log` and `Floor` for the HNSW level formula.

Gorse calls math32 from `common/nn`, `common/ann`, `common/floats`,
`common/heap`, `dataset`, `model/cf` and `model/ctr`. Ported: `Abs`,
`Copysign`, `Cos`, `Exp`, `Floor`, `Frexp`, `Inf`, `IsInf`, `IsNaN`, `Ldexp`,
`Log`, `Modf`, `NaN`, `Pow`, `Sin`, `Sqrt`, `Tanh` and the constants `Pi`,
`Sqrt2`, `Ln10`, `MaxFloat32`, plus Go's own `int32(x)` and `int(x)`
conversions of a `float32` (`go_f32_to_i32`, `go_f32_to_i64`). Not yet ported:
`Log1p` and `Log2` (see Open).

## Evidence

- **Golden cases.** 13 functions over 21 862 inputs (edge values, math32's own
  test vectors, every exponent with the mantissas around √2/2, Adam's
  `Pow(0.9, t)` and `Pow(0.999, t)` for `t` up to 1000, and splitmix64
  draws). Each output is recorded twice: `asm` from the default build, where
  math32's `Exp`, `Log` and `Sqrt` are assembly on amd64, and `noasm` from a
  child built with `-tags noasm`, where math32 runs its Go code everywhere.
  `math32_matches_go_on_amd64` checks the public functions against `asm` and
  `math32_matches_go_code` checks the Go-code kernels against `noasm`, bit for
  bit. The columns differ for exactly `sqrt`, `exp`, `log`, `tanh` and `pow`,
  which `math32_amd64_differs_from_go_code` asserts.
- **Every input.** For the 10 unary functions and the 2 conversions the
  golden holds an FNV-1a digest of the outputs over all 2^32 input bit
  patterns, per column. The ignored test `math32_exhaustive` recomputes them
  (release build with overflow checks on, so no input panics in debug
  arithmetic either); all 24 digests match. On 4 cores the Go tool takes
  about 6.5 minutes and the Rust test about 9.5.
- **Upstream tests.** The cases and assertions of `all_test.go` for the
  ported functions (`TestAbs`, `TestCopysign`, `TestCos`, `TestExp`,
  `TestFloor`, `TestFrexp`, `TestLdexp`, `TestLog`, `TestModf`, `TestPow`,
  `TestSin`, `TestSqrt`, `TestTanh`) with its `close`, `veryclose` and
  `alike` helpers. In a copy of the module, `go test` passes on amd64 in
  both the default and the `-tags noasm` build.
- **Mutation checks.** 36 single-point mutations of the quirks and boundaries
  (list in the commit message): 29 are caught by `cargo test -p gorse-common
  math32`, one of them as a hang (`Pow` with `yi > 2^31` instead of `>=`
  reaches its loop with `int32(2^31)`, which is `MinInt32` on amd64, and
  `i >>= 1` never reaches 0, as in Go). The 7 survivors are equivalent
  mutants, where no input can tell the two versions apart:
  - `exp_amd64.s` jumps on the overflow flag of `ADDL $0x7F`, which no input
    sets: below 2^31 a float32 is a multiple of 128, so `k <= 2^31 - 128`. The
    Rust code tests `e <= 0` and says why.
  - `e >= 0xFF` against `e > 0xFF` in the same code: at `e = 0xFF` the scale
    factor has the bits of `+Inf`, and the polynomial is positive, so the
    product is `+Inf` too.
  - In `trigReduce`, `32 - (lz + 1)` would wrap (as Go's `uint` does, and the
    port with it) only for `lz >= 32`. A search over every float32 from 2^29
    up found no input with `lz >= 32`.
  - `Tanh`'s `MAXLOG/2` one ulp higher changes the branch for one input,
    where both branches give 1.
  - `normalize` with `<=` instead of `<` at the smallest normal: normalising a
    normal number changes neither `Frexp` nor `Ldexp`.
  - `Floor` testing `fract > 0` instead of `fract != 0`: the fractional part
    of a positive number is never negative.
  - `sqrt`'s rounding `q += q & 1` against `q += 1`: the bit added to an even
    `q` is shifted out.

## Plan corrections

- **C-WP111M-1 math32 is a dependency to port.** `porting/work-packages.md`
  and `porting/crate-selection.md` did not list `github.com/chewxy/math32`,
  though `common/nn`, `common/ann`, `dataset` and both model packages call it
  and their results depend on its exact bits. Rust's `f32::exp` and `f32::ln`
  call the platform libm, whose results differ. It is a hand port in
  `gorse-common`, owned by WP-1-11, as agreed with the integrator (who will not
  write a second one). `crate-selection.md` gains its row.

## Quirks reproduced

- **Q-WP111M-1 amd64 `Exp` adds the exponent into its polynomial.**
  `exp_amd64.s` has `ADDSS $T4, X1` where it means to load `T4` into `X1`, which
  still holds `float32(k)`. Results are off by a few ulps, by more as `|x|`
  grows. Reproduced; golden `exp` (`asm`).
- **Q-WP111M-2 amd64 `Exp` range.** `k` is `x·log2(e)` rounded half to even
  (`CVTSS2SL`), and the result is assembled as `2^k` times the polynomial
  without the Go code's range checks. So `Exp` is `+Inf` from `x = 88.37627`
  on (the true value is finite up to 88.72; the Go code gives `3.3259769e38`
  at 88.7), subnormal only from -87.33655 to -87.68312 and 0 from
  `x = -87.68313` down (the Go code has subnormals to about -104), and 0 for
  every finite `x` with `|x·log2(e)| >= 2^31` (from about `±1.49e9`), where
  the conversion gives `MinInt32`: `Exp(1e9)` is `+Inf` but `Exp(2e9)` is 0,
  while `Exp(+Inf)` is `+Inf`. Reproduced; golden `exp`, and the ranges are
  asserted in `math32_amd64_differs_from_go_code`.
- **Q-WP111M-3 amd64 `Log` takes the exponent bits raw.** `k = e - 126` with
  no normalisation, so subnormal inputs give wrong results (`Log(1e-40)` is
  `-88.02122`, the Go code gives `-92.10341`). Reproduced.
- **Q-WP111M-4 amd64 `Log` halves when `f1 <= √2/2`** (`CMPSS X2, X0, 5`
  computes `!(HSqrt2 < f1)`), where the Go code halves when `f1 < Sqrt2/2`.
  They take different paths for mantissa `0x3504F3`, and the results differ
  at some exponents: `Log(0.70710677)` is `0xBEB17218` from the assembly and
  `0xBEB17219` from the Go code. The golden covers that mantissa and its
  neighbours at every exponent. Reproduced.
- **Q-WP111M-5 amd64 `Log` of any negative input**, a negative NaN included,
  returns math32's NaN `0x7FE00000`; a positive NaN comes back unchanged.
  Reproduced.
- **Q-WP111M-6 NaN bits.** math32's `NaN()` is `0x7FE00000`, not Rust's
  `0x7FC00000`. `Sqrt` on amd64 is `SQRTSS`: `0xFFC00000` for a negative input
  and the input quieted for a NaN; the Go code returns `0x7FE00000`.
  Reproduced (the bits feed `Pow` and `Tanh`, and any NaN a caller stores).
- **Q-WP111M-7 `Pow` and `Tanh` call `Exp`, `Log` and `Sqrt`**, so on amd64
  they inherit Q-WP111M-1 to 6. `Pow`'s `isOddInt` and its loop over the
  bits of `y` convert with `int32(x)`, so they get amd64's out-of-range result
  (Q-WP111M-9). Reproduced.
- **Q-WP111M-8 `Sin` and `Cos` above 2^29.** math32's `trigReduce` adapts the
  float64 Payne–Hanek code with 32-bit shift counts over the 64-bit digits of
  4/π, takes the octant as `hi >> 29` of a 64-bit value and masks it to 0..7
  only when it is odd. `Sin` and `Cos` then pick the polynomial and sign from
  that unmasked octant, so results for `|x| >= 2^29` are not the true sine.
  Reproduced line by line; the exhaustive digests cover every such input.
- **Q-WP111M-9 Go's float-to-integer conversions.** `int32(x)` and `int(x)`
  truncate toward zero; for NaN and out-of-range values amd64 gives `MinInt32`
  and `MinInt64` (`CVTTSS2SL`, `CVTTSS2SQ`), where Rust's `as` saturates and
  maps NaN to 0. `go_f32_to_i32` and `go_f32_to_i64` reproduce amd64; the
  exhaustive digests check them on every float32.

## Deviations

- **X-WP111M-1 arm64 is not bit-exact.** On arm64 math32's `Exp` and `Log` run
  the Go code (`stubs_arm64.s`), which the port selects there, but Go compiles
  every pure-Go function on arm64 with fused multiply-adds and Rust does not
  fuse. Results on arm64 can differ in the last bit. `PORTING.md` D-25 makes
  linux/amd64 the reference; no arm64 machine was available to measure.
- **X-WP111M-2 Only what Gorse calls.** The rest of math32 (`Acos`, `Gamma`,
  `J0`, …) is not ported.
- **X-WP111M-3 `Sqrt` on targets other than x86-64 and aarch64** uses the Go
  code. Go has assembly for more architectures; Gorse ships amd64 and arm64.

## For the integrator

- HNSW (`common/ann/hnsw.go`): `levelFactor` is `1 / math32::log(48.0)`
  computed at run time, and the level is
  `go_f32_to_i64(math32::floor(-math32::log(r) * level_factor))`. When
  `rand.Float32()` returns 0, `Log` is `-Inf`, the product `+Inf`, and Go's
  `int(+Inf)` on amd64 is `MinInt64`; `go_f32_to_i64` reproduces that.
- `go_f32_to_i32` and `go_f32_to_i64` are public for any `int(float32)` in the
  port. `consistent.rs` notes the float64 analogue (X-WP113-4); a
  `go_f64_to_i64` could join them if wanted.
- No new dependencies.
- Regeneration line for `contracts/README.md` (linux/amd64 only; about 6.5
  minutes on 4 cores, most of it the 2^32-input digests):
  `go run ./contracts/tools/math32golden -out crates/gorse-common/tests/data/math32_golden.json`

## Open

- **O-WP111M-1 `Log1p` and `Log2`** (`model/cf` and `model/ctr`) are
  `float32(math.Log1p(float64(x)))` and `float32(math.Log2(float64(x)))`, so
  they need Go's float64 `math.Log` (assembly on amd64) and `math.Log1p`
  ported. `common/nn` needs Go's float64 `math.Exp` (assembly with an FMA
  path) for the same reason. They follow with the nn part of WP-1-11.
