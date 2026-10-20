// Ports: github.com/chewxy/math32/{abs,bits,const,copysign,exp,floor,frexp,ldexp,log,modf,pow,sincos,sqrt,tanh}.go, exp_amd64.s, log_amd64.s, sqrt_amd64.s @ add21b5 (v1.11.2 in go.mod)

//! `float32` math functions as `github.com/chewxy/math32` computes them, which
//! Gorse calls from `common/nn`, `common/ann`, `dataset` and the models.
//!
//! These are not the correctly rounded `std` functions. On amd64 math32's `Exp`
//! and `Log` are assembly that computes differently from the Go formulas in the
//! same package:
//!
//! - `Exp` drops the load of the first Taylor coefficient, so the exponent `k`
//!   leaks into the polynomial and results are off by a few ulps. It builds
//!   the result as `2^k` times the polynomial, `k` the nearest integer to
//!   `x·log2(e)`, so it returns `+Inf` from `x = 88.37627` on (the true value is
//!   finite up to 88.72) and 0 from `x = -87.68313` down, where the Go code
//!   goes on with subnormals to about -104. For `|x·log2(e)| ≥ 2^31` the
//!   conversion overflows, which turns finite `x` from about `±1.49e9` on
//!   into 0.
//! - `Log` reads the exponent bits without normalising subnormals, so its
//!   results for subnormal inputs are wrong. It halves the reduced fraction
//!   when `f1 <= √2/2` where the Go code uses `<`. It returns the NaN
//!   `0x7FE00000` for every negative input, a negative NaN included.
//!
//! [`exp`], [`log`] and [`sqrt`] reproduce what Go runs on the target
//! architecture: the assembly on x86-64, and the Go code elsewhere. Go on arm64
//! compiles the Go code with fused multiply-adds, which Rust does not do, so
//! results there can differ in the last bit (`PORTING.md` D-25 makes
//! linux/amd64 the reference). The other functions are pure Go on every
//! architecture and are ported line by line.
//!
//! Go's float-to-integer conversions (`int32(x)`, `int(x)`) are reproduced with
//! the values amd64 gives for NaN and out-of-range inputs.

// The constants are Go's decimal literals, digit for digit.
#![allow(clippy::excessive_precision, clippy::approx_constant)]

/// π rounded to `f32` (Go `math32.Pi`).
pub const PI: f32 = std::f32::consts::PI;
/// √2 rounded to `f32` (Go `math32.Sqrt2`).
pub const SQRT2: f32 = std::f32::consts::SQRT_2;
/// ln 10 rounded to `f32` (Go `math32.Ln10`).
pub const LN10: f32 = std::f32::consts::LN_10;
/// The largest finite `f32` (Go `math32.MaxFloat32`).
pub const MAX_FLOAT32: f32 = f32::MAX;

const UVNAN: u32 = 0x7FE0_0000;
const UVINF: u32 = 0x7F80_0000;
const UVNEGINF: u32 = 0xFF80_0000;
const MASK: u32 = 0xFF;
const SHIFT: u32 = 32 - 8 - 1;
const BIAS: i64 = 127;
const SIGN_MASK: u32 = 1 << 31;

/// `+Inf` if `sign >= 0`, `-Inf` otherwise (Go `Inf`).
pub fn inf(sign: i32) -> f32 {
    f32::from_bits(if sign >= 0 { UVINF } else { UVNEGINF })
}

/// math32's NaN, the bits `0x7FE00000` (Go `NaN`).
pub fn nan() -> f32 {
    f32::from_bits(UVNAN)
}

/// Whether `f` is a NaN (Go `IsNaN`).
#[allow(clippy::eq_op)]
pub fn is_nan(f: f32) -> bool {
    f != f
}

/// Whether `f` is an infinity of the given sign: `+Inf` for `sign > 0`, `-Inf`
/// for `sign < 0`, either for 0 (Go `IsInf`).
pub fn is_inf(f: f32, sign: i32) -> bool {
    sign >= 0 && f > MAX_FLOAT32 || sign <= 0 && f < -MAX_FLOAT32
}

/// `|x|`, by clearing the sign bit (Go `Abs`).
pub fn abs(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & !SIGN_MASK)
}

/// `x` with the sign of `y` (Go `Copysign`).
pub fn copysign(x: f32, y: f32) -> f32 {
    f32::from_bits(x.to_bits() & !SIGN_MASK | y.to_bits() & SIGN_MASK)
}

/// Scales a subnormal `x` into the normal range (Go `normalize`).
fn normalize(x: f32) -> (f32, i64) {
    const SMALLEST_NORMAL: f32 = 1.175_494_350_822_287_5e-38;
    if abs(x) < SMALLEST_NORMAL {
        return (x * (1u32 << SHIFT) as f32, -(SHIFT as i64));
    }
    (x, 0)
}

/// Splits `f` into a fraction in `[½, 1)` and a power of two (Go `Frexp`).
/// `±0`, `±Inf` and NaN come back unchanged with exponent 0.
pub fn frexp(f: f32) -> (f32, i64) {
    if f == 0.0 || is_inf(f, 0) || is_nan(f) {
        return (f, 0);
    }
    let (f, mut exp) = normalize(f);
    let mut x = f.to_bits();
    exp += ((x >> SHIFT) & MASK) as i64 - BIAS + 1;
    x &= !(MASK << SHIFT);
    x |= ((-1 + BIAS) as u32) << SHIFT;
    (f32::from_bits(x), exp)
}

/// `frac · 2^exp` (Go `Ldexp`).
pub fn ldexp(frac: f32, exp: i64) -> f32 {
    if frac == 0.0 || is_inf(frac, 0) || is_nan(frac) {
        return frac;
    }
    let (frac, e) = normalize(frac);
    let mut exp = exp.wrapping_add(e);
    let mut x = frac.to_bits();
    exp = exp.wrapping_add(((x >> SHIFT) as i64 & MASK as i64) - BIAS);
    if exp < -149 {
        return copysign(0.0, frac);
    }
    if exp > 127 {
        return if frac < 0.0 { inf(-1) } else { inf(1) };
    }
    let mut m: f32 = 1.0;
    if exp < -(127 - 1) {
        exp += SHIFT as i64;
        m = 1.0 / (1u32 << 23) as f32;
    }
    x &= !(MASK << SHIFT);
    x |= ((exp + BIAS) as u32) << SHIFT;
    m * f32::from_bits(x)
}

/// The integer and fractional parts of `f`, both with its sign (Go `Modf`).
pub fn modf(f: f32) -> (f32, f32) {
    if f < 1.0 {
        if f < 0.0 {
            let (int, frac) = modf(-f);
            return (-int, -frac);
        } else if f == 0.0 {
            return (f, f);
        }
        return (0.0, f);
    }
    let mut x = f.to_bits();
    // Go computes `e` as a `uint`; it cannot wrap here since `f >= 1` or NaN.
    let e = ((x >> SHIFT) & MASK).wrapping_sub(BIAS as u32);
    if e < 32 - 9 {
        x &= !((1u32 << (32 - 9 - e)) - 1);
    }
    let int = f32::from_bits(x);
    (int, f - int)
}

/// The greatest integer value not above `x` (Go `Floor`).
pub fn floor(x: f32) -> f32 {
    if x == 0.0 || is_nan(x) || is_inf(x, 0) {
        return x;
    }
    if x < 0.0 {
        let (mut d, fract) = modf(-x);
        if fract != 0.0 {
            d += 1.0;
        }
        return -d;
    }
    modf(x).0
}

/// The square root of `x` (Go `Sqrt`).
///
/// Go uses the hardware instruction on x86-64 (`SQRTSS`) and arm64, both
/// correctly rounded like [`f32::sqrt`]. The NaNs are spelled out so they do
/// not depend on how the compiler folds `sqrt` of a negative constant: SSE
/// returns the default NaN `0xFFC00000` for negative inputs and the input
/// quieted for a NaN.
pub fn sqrt(x: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_nan(x) {
            return f32::from_bits(x.to_bits() | 0x0040_0000);
        }
        if x < 0.0 {
            return f32::from_bits(0xFFC0_0000);
        }
        x.sqrt()
    }
    #[cfg(target_arch = "aarch64")]
    {
        if is_nan(x) {
            return f32::from_bits(x.to_bits() | 0x0040_0000);
        }
        if x < 0.0 {
            return f32::from_bits(0x7FC0_0000);
        }
        x.sqrt()
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        sqrt_go(x)
    }
}

/// The Go code of `Sqrt` (bit by bit, Go `sqrt`), which Go runs where no
/// assembly is available.
#[cfg_attr(any(target_arch = "x86_64", target_arch = "aarch64"), allow(dead_code))]
fn sqrt_go(x: f32) -> f32 {
    if x == 0.0 || is_nan(x) || is_inf(x, 1) {
        return x;
    }
    if x < 0.0 {
        return nan();
    }
    let mut ix = x.to_bits();
    let mut exp = ((ix >> SHIFT) & MASK) as i64;
    if exp == 0 {
        while ix & (1 << SHIFT) == 0 {
            ix <<= 1;
            exp -= 1;
        }
        exp += 1;
    }
    exp -= BIAS;
    ix &= !(MASK << SHIFT);
    ix |= 1 << SHIFT;
    if exp & 1 == 1 {
        ix <<= 1;
    }
    exp >>= 1;
    ix <<= 1;
    let (mut q, mut s) = (0u32, 0u32);
    let mut r: u32 = 1 << (SHIFT + 1);
    while r != 0 {
        let t = s + r;
        if t <= ix {
            s = t + r;
            ix -= t;
            q += r;
        }
        ix <<= 1;
        r >>= 1;
    }
    if ix != 0 {
        q += q & 1;
    }
    ix = (q >> 1).wrapping_add(((exp - 1 + BIAS) as u32) << SHIFT);
    f32::from_bits(ix)
}

/// `e^x` (Go `Exp`); see the module documentation for how it differs from
/// the true value on x86-64.
pub fn exp(x: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        exp_amd64(x)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        exp_go(x)
    }
}

/// `CVTSS2SL`: rounds to the nearest integer, ties to even (the default
/// MXCSR mode), and gives `i32::MIN` for NaN and out-of-range values.
fn cvtss2sl(x: f32) -> i32 {
    let r = x.round_ties_even();
    if r.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&r) {
        i32::MIN
    } else {
        r as i32
    }
}

/// `archExp` of `exp_amd64.s`, instruction by instruction.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn exp_amd64(x: f32) -> f32 {
    // The constants are assembled as float64 values rounded to float32.
    const LOG2E: f32 = 1.442_695_021_629_333_496_093_75_f64 as f32;
    const LN2U: f32 = 0.693_145_751_953_125_f64 as f32;
    const LN2L: f32 = 1.428_606_765_330_187_045e-06_f64 as f32;
    const T0: f32 = 1.0;
    const T1: f32 = 0.5;
    const T2: f32 = 0.166_665_524_244_308_471_679_688_f64 as f32;
    const T3: f32 = 0.041_671_082_377_433_776_855_468_8_f64 as f32;
    const T4: f32 = 0.008_365_969_173_610_210_418_701_17_f64 as f32;

    let bx = x.to_bits();
    // CMPL $PosInf, |x|; JLE notFinite
    if bx & !SIGN_MASK >= UVINF {
        if bx == UVNEGINF {
            return 0.0;
        }
        return x;
    }
    let k = cvtss2sl(LOG2E * x);
    let kf = k as f32;
    let mut x0 = x - LN2U * kf;
    x0 -= LN2L * kf;
    x0 *= 0.0625;
    // The assembly adds T4 to the register that still holds k instead of
    // loading T4 into it.
    let mut x1 = kf + T4;
    x1 *= x0;
    x1 += T3;
    x1 *= x0;
    x1 += T2;
    x1 *= x0;
    x1 += T1;
    x1 *= x0;
    x1 += T0;
    x0 *= x1;
    for _ in 0..4 {
        let t = 2.0 + x0;
        x0 *= t;
    }
    x0 += 1.0;
    // ADDL $0x7F, BX; JLE underflow; CMPL BX, $0xFF; JGE overflow. The
    // addition cannot overflow: below 2^31 a float32 is a multiple of 128, so
    // k <= 2^31 - 128.
    let e = k.wrapping_add(0x7F);
    if e <= 0 {
        return 0.0;
    }
    if e >= 0xFF {
        return inf(1);
    }
    // SHLQ $23 of the zero-extended register, then the low 32 bits.
    x0 * f32::from_bits((e as u32) << 23)
}

/// The Go code of `Exp` (Go `exp`), which Go runs where no assembly is
/// available (arm64 among them).
#[cfg_attr(target_arch = "x86_64", allow(dead_code))]
fn exp_go(x: f32) -> f32 {
    const LN2_HI: f32 = 6.931_381_225_6e-01;
    const LN2_LO: f32 = 9.058_000_614_5e-06;
    const LOG2E: f32 = 1.442_695_021_6e+00;
    const OVERFLOW: f32 = 7.097_827_128_933_839_730_96e+02;
    const UNDERFLOW: f32 = -7.451_332_191_019_411_084_20e+02;
    const NEAR_ZERO: f32 = 1.0 / (1u32 << 28) as f32;

    if is_nan(x) || is_inf(x, 1) {
        return x;
    }
    if is_inf(x, -1) {
        return 0.0;
    }
    if x > OVERFLOW {
        return inf(1);
    }
    if x < UNDERFLOW {
        return 0.0;
    }
    if -NEAR_ZERO < x && x < NEAR_ZERO {
        return 1.0 + x;
    }
    // Go's `int(f)` truncates; `x` is in range here.
    let k: i64 = if x < 0.0 {
        (LOG2E * x - 0.5) as i64
    } else if x > 0.0 {
        (LOG2E * x + 0.5) as i64
    } else {
        0
    };
    let hi = x - k as f32 * LN2_HI;
    let lo = k as f32 * LN2_LO;
    expmulti(hi, lo, k)
}

/// `e^r · 2^k` where `r = hi - lo` (Go `expmulti`).
#[cfg_attr(target_arch = "x86_64", allow(dead_code))]
fn expmulti(hi: f32, lo: f32, k: i64) -> f32 {
    const P1: f32 = 1.666_666_716_3e-01;
    const P2: f32 = -2.777_777_845_0e-03;
    const P3: f32 = 6.613_755_977_0e-05;
    const P4: f32 = -1.653_390_199_9e-06;
    const P5: f32 = 4.138_136_944_2e-08;

    let r = hi - lo;
    let t = r * r;
    let c = r - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let y = 1.0 - ((lo - (r * c) / (2.0 - c)) - hi);
    ldexp(y, k)
}

/// The natural logarithm (Go `Log`); see the module documentation for how it
/// differs from the true value on x86-64.
pub fn log(x: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        log_amd64(x)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        log_go(x)
    }
}

const LN2_HI: f32 = 6.931_381_225_6e-01;
const LN2_LO: f32 = 9.058_000_614_5e-06;
const L1: f32 = 6.666_666_865_3e-01;
const L2: f32 = 4.000_000_059_6e-01;
const L3: f32 = 2.857_142_984_9e-01;
const L4: f32 = 2.222_219_854_6e-01;
const L5: f32 = 1.818_357_259_0e-01;
const L6: f32 = 1.531_383_842_2e-01;
const L7: f32 = 1.479_819_864_0e-01;

/// `archLog` of `log_amd64.s`, instruction by instruction.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn log_amd64(x: f32) -> f32 {
    // The assembly's constants, assembled as float64 values rounded to
    // float32 (the same bits as the Go code's, see the tests).
    const HSQRT2: f32 = 7.071_067_811_865_475_244_01e-01_f64 as f32;
    const LN2_HI: f32 = 6.931_381_225_6e-01_f64 as f32;
    const LN2_LO: f32 = 9.058_000_614_5e-06_f64 as f32;
    const L1: f32 = 6.666_666_865_3e-01_f64 as f32;
    const L2: f32 = 4.000_000_059_6e-01_f64 as f32;
    const L3: f32 = 2.857_142_984_9e-01_f64 as f32;
    const L4: f32 = 2.222_219_854_6e-01_f64 as f32;
    const L5: f32 = 1.818_357_259_0e-01_f64 as f32;
    const L6: f32 = 1.531_383_842_2e-01_f64 as f32;
    const L7: f32 = 1.479_819_864_0e-01_f64 as f32;

    let bx = x.to_bits();
    if bx & !SIGN_MASK == 0 {
        return inf(-1);
    }
    if (bx as i32) < 0 {
        return nan();
    }
    if bx >= UVINF {
        return x;
    }
    // The exponent bits are taken as they are, subnormals included.
    let mut f1 = f32::from_bits((bx & 0x007F_FFFF) | 0.5f32.to_bits());
    let mut k = (((bx >> 23) & 0xFF) as i32 - 0x7E) as f32;
    // CMPSS X2, X0, 5 sets the mask when !(HSqrt2 < f1).
    let halve: f32 = if HSQRT2 < f1 { 0.0 } else { 1.0 };
    k -= halve;
    f1 *= halve + 1.0;
    let f = f1 - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let t1 = s2 * (L1 + s4 * (L3 + s4 * (L5 + s4 * L7)));
    let t2 = s4 * (L2 + s4 * (L4 + s4 * L6));
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    k * LN2_HI - ((hfsq - (s * (hfsq + r) + k * LN2_LO)) - f)
}

/// The Go code of `Log` (Go `log`), which arm64 runs through its stub.
#[cfg_attr(target_arch = "x86_64", allow(dead_code))]
fn log_go(x: f32) -> f32 {
    if is_nan(x) || is_inf(x, 1) {
        return x;
    }
    if x < 0.0 {
        return nan();
    }
    if x == 0.0 {
        return inf(-1);
    }
    let (mut f1, mut ki) = frexp(x);
    if f1 < SQRT2 / 2.0 {
        f1 *= 2.0;
        ki -= 1;
    }
    let f = f1 - 1.0;
    let k = ki as f32;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let t1 = s2 * (L1 + s4 * (L3 + s4 * (L5 + s4 * L7)));
    let t2 = s4 * (L2 + s4 * (L4 + s4 * L6));
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    k * LN2_HI - ((hfsq - (s * (hfsq + r) + k * LN2_LO)) - f)
}

/// The functions math32 implements in assembly on some architectures, which
/// [`pow`] and [`tanh`] call.
trait Kernels {
    fn exp(x: f32) -> f32;
    fn log(x: f32) -> f32;
    fn sqrt(x: f32) -> f32;
}

/// What Go runs on this architecture.
struct Native;

impl Kernels for Native {
    fn exp(x: f32) -> f32 {
        exp(x)
    }
    fn log(x: f32) -> f32 {
        log(x)
    }
    fn sqrt(x: f32) -> f32 {
        sqrt(x)
    }
}

/// math32's Go code (the `noasm` build).
#[cfg_attr(not(test), allow(dead_code))]
struct GoCode;

impl Kernels for GoCode {
    fn exp(x: f32) -> f32 {
        exp_go(x)
    }
    fn log(x: f32) -> f32 {
        log_go(x)
    }
    fn sqrt(x: f32) -> f32 {
        sqrt_go(x)
    }
}

/// Go's `int32(x)` for a `float32`: truncation toward zero, and on amd64
/// (`CVTTSS2SL`) `i32::MIN` for NaN and values out of range. Other
/// architectures saturate, as Rust's `as` does.
pub fn go_f32_to_i32(x: f32) -> i32 {
    if cfg!(target_arch = "x86_64")
        && (x.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&x.trunc()))
    {
        return i32::MIN;
    }
    x as i32
}

/// Go's `int(x)` (`int64`) for a `float32`: truncation toward zero, and on
/// amd64 (`CVTTSS2SQ`) `i64::MIN` for NaN and values out of range. Other
/// architectures saturate, as Rust's `as` does.
pub fn go_f32_to_i64(x: f32) -> i64 {
    if cfg!(target_arch = "x86_64")
        && (x.is_nan()
            || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&x.trunc()))
    {
        return i64::MIN;
    }
    x as i64
}

fn is_odd_int(x: f32) -> bool {
    let (xi, xf) = modf(x);
    xf == 0.0 && go_f32_to_i32(xi) & 1 == 1
}

/// `x^y` (Go `Pow`), with Go's special cases in Go's order.
pub fn pow(x: f32, y: f32) -> f32 {
    pow_with::<Native>(x, y)
}

fn pow_with<K: Kernels>(x: f32, y: f32) -> f32 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    } else if y == 1.0 {
        return x;
    } else if y == 0.5 {
        return K::sqrt(x);
    } else if y == -0.5 {
        return 1.0 / K::sqrt(x);
    } else if is_nan(x) || is_nan(y) {
        return nan();
    } else if x == 0.0 {
        if y < 0.0 {
            if is_odd_int(y) {
                return copysign(inf(1), x);
            }
            return inf(1);
        } else if y > 0.0 {
            if is_odd_int(y) {
                return x;
            }
            return 0.0;
        }
    } else if is_inf(y, 0) {
        if x == -1.0 {
            return 1.0;
        } else if (abs(x) < 1.0) == is_inf(y, 1) {
            return 0.0;
        }
        return inf(1);
    } else if is_inf(x, 0) {
        if is_inf(x, -1) {
            return pow_with::<K>(1.0 / x, -y);
        }
        if y < 0.0 {
            return 0.0;
        } else if y > 0.0 {
            return inf(1);
        }
    }

    let mut absy = y;
    let mut flip = false;
    if absy < 0.0 {
        absy = -absy;
        flip = true;
    }
    let (mut yi, mut yf) = modf(absy);
    if yf != 0.0 && x < 0.0 {
        return nan();
    }
    if yi >= 2_147_483_648.0 {
        return K::exp(y * K::log(x));
    }

    let mut a1: f32 = 1.0;
    let mut ae: i64 = 0;
    if yf != 0.0 {
        if yf > 0.5 {
            yf -= 1.0;
            yi += 1.0;
        }
        a1 = K::exp(yf * K::log(x));
    }

    let (mut x1, mut xe) = frexp(x);
    let mut i = go_f32_to_i32(yi);
    while i != 0 {
        if i & 1 == 1 {
            a1 *= x1;
            ae += xe;
        }
        x1 *= x1;
        xe <<= 1;
        if x1 < 0.5 {
            x1 += x1;
            xe -= 1;
        }
        i >>= 1;
    }

    if flip {
        a1 = 1.0 / a1;
        ae = -ae;
    }
    ldexp(a1, ae)
}

const SIN: [f32; 6] = [
    1.589_623_015_765_465_680_60e-10,
    -2.505_074_776_285_780_728_66e-8,
    2.755_731_362_138_572_452_13e-6,
    -1.984_126_982_958_953_859_96e-4,
    8.333_333_333_322_118_588_78e-3,
    -1.666_666_666_666_663_072_95e-1,
];

const COS: [f32; 6] = [
    -1.135_853_652_138_768_173_00e-11,
    2.087_570_084_197_473_167_78e-9,
    -2.755_731_417_929_673_881_12e-7,
    2.480_158_728_885_170_453_48e-5,
    -1.388_888_888_887_305_641_16e-3,
    4.166_666_666_666_659_292_18e-2,
];

// Pi/4 split into three parts, each rounded to float32 where it is used.
const PI4A: f32 = 7.853_981_256_484_985_351_56e-1;
const PI4B: f32 = 3.774_894_707_930_798_176_68e-8;
const PI4C: f32 = 2.695_151_429_079_059_526_45e-15;

const REDUCE_THRESHOLD: f32 = (1u32 << 29) as f32;

fn sin_poly(z: f32, zz: f32) -> f32 {
    z + z
        * zz
        * (((((SIN[0] * zz + SIN[1]) * zz + SIN[2]) * zz + SIN[3]) * zz + SIN[4]) * zz + SIN[5])
}

fn cos_poly(zz: f32) -> f32 {
    1.0 - 0.5 * zz
        + zz * zz
            * (((((COS[0] * zz + COS[1]) * zz + COS[2]) * zz + COS[3]) * zz + COS[4]) * zz + COS[5])
}

/// The octant `j` and reduced argument `z` of a non-negative `x` below the
/// reduction threshold.
fn octant(x: f32) -> (u64, f32) {
    // `uint64(x * (4 / Pi))`: in range, so a plain truncation.
    let mut j = (x * (4.0 / PI)) as u64;
    let mut y = j as f32;
    if j & 1 == 1 {
        j += 1;
        y += 1.0;
    }
    j &= 7;
    (j, ((x - y * PI4A) - y * PI4B) - y * PI4C)
}

/// The sine of `x` in radians (Go `Sin`).
pub fn sin(x: f32) -> f32 {
    if x == 0.0 || is_nan(x) {
        return x;
    }
    if is_inf(x, 0) {
        return nan();
    }
    let mut x = x;
    let mut sign = false;
    if x < 0.0 {
        x = -x;
        sign = true;
    }
    let (mut j, z) = if x >= REDUCE_THRESHOLD {
        trig_reduce(x)
    } else {
        octant(x)
    };
    if j > 3 {
        sign = !sign;
        j = j.wrapping_sub(4);
    }
    let zz = z * z;
    let mut y = if j == 1 || j == 2 {
        cos_poly(zz)
    } else {
        sin_poly(z, zz)
    };
    if sign {
        y = -y;
    }
    y
}

/// The cosine of `x` in radians (Go `Cos`).
pub fn cos(x: f32) -> f32 {
    if is_nan(x) || is_inf(x, 0) {
        return nan();
    }
    let mut sign = false;
    let x = abs(x);
    let (mut j, z) = if x >= REDUCE_THRESHOLD {
        trig_reduce(x)
    } else {
        octant(x)
    };
    if j > 3 {
        j = j.wrapping_sub(4);
        sign = !sign;
    }
    if j > 1 {
        sign = !sign;
    }
    let zz = z * z;
    let mut y = if j == 1 || j == 2 {
        sin_poly(z, zz)
    } else {
        cos_poly(zz)
    };
    if sign {
        y = -y;
    }
    y
}

/// The bits of 4/π (Go `mPi4`).
const M_PI4: [u64; 20] = [
    0x0000_0000_0000_0001,
    0x45f3_06dc_9c88_2a53,
    0xf84e_afa3_ea69_bb81,
    0xb6c5_2b32_7887_2083,
    0xfca2_c757_bd77_8ac3,
    0x6e48_dc74_849b_a5c0,
    0x0c92_5dd4_13a3_2439,
    0xfc3b_d639_6253_4e7d,
    0xd104_6bea_5d76_8909,
    0xd338_e04d_68be_fc82,
    0x7323_ac73_06a6_73e9,
    0x3908_bf17_7bf2_5076,
    0x3ff1_2fff_bc0b_301f,
    0xde5e_2316_b414_da3e,
    0xda6c_fd9e_4f96_136e,
    0x9e8c_7ecd_3cbf_d45a,
    0xea4f_758f_d7cb_e2f6,
    0x7a0e_73ef_14a5_25d4,
    0xd7f6_bf62_3f1a_ba10,
    0xac06_608d_f8f6_d757,
];

/// Go's `a << s` for `uint64`, where a count of 64 or more gives 0.
fn shl(a: u64, s: u64) -> u64 {
    if s >= 64 { 0 } else { a << s }
}

/// Go's `a >> s` for `uint64`, where a count of 64 or more gives 0.
fn shr(a: u64, s: u64) -> u64 {
    if s >= 64 { 0 } else { a >> s }
}

/// Payne–Hanek reduction for large `x` (Go `trigReduce`). math32 adapted the
/// float64 code with 32-bit shifts over the 64-bit digits of 4/π, and only
/// masks `j` to an octant when it maps zeros to the origin; both are kept.
fn trig_reduce(x: f32) -> (u64, f32) {
    const PI4: f32 = PI / 4.0;
    const FLOATING_BITS: u64 = 32 - 3;
    if x < PI4 {
        return (0, x);
    }
    let mut ix = x.to_bits();
    let exp = ((ix >> SHIFT) & MASK) as i64 - BIAS - SHIFT as i64;
    ix &= !(MASK << SHIFT);
    ix |= 1 << SHIFT;
    let pos = (exp + FLOATING_BITS as i64) as u64;
    let (digit, bitshift) = ((pos / 32) as usize, pos % 32);
    let z0 = shl(M_PI4[digit], bitshift) | shr(M_PI4[digit + 1], 32 - bitshift);
    let z1 = shl(M_PI4[digit + 1], bitshift) | shr(M_PI4[digit + 2], 32 - bitshift);
    let z2 = shl(M_PI4[digit + 2], bitshift) | shr(M_PI4[digit + 3], 32 - bitshift);
    let ix64 = u64::from(ix);
    let z2hi = ((u128::from(z2) * u128::from(ix64)) >> 64) as u64;
    let z1prod = u128::from(z1) * u128::from(ix64);
    let (z1hi, z1lo) = ((z1prod >> 64) as u64, z1prod as u64);
    let z0lo = z0.wrapping_mul(ix64);
    let (lo, carry) = z1lo.overflowing_add(z2hi);
    let mut hi = z0lo.wrapping_add(z1hi).wrapping_add(u64::from(carry));
    let mut j = hi >> FLOATING_BITS;
    hi = hi << 3 | lo >> FLOATING_BITS;
    let lz = u64::from(hi.leading_zeros());
    let e = (BIAS as u64).wrapping_sub(lz + 1);
    hi = shl(hi, lz + 1) | shr(lo, 32u64.wrapping_sub(lz + 1));
    hi >>= 43 - SHIFT as u64;
    hi |= e << SHIFT;
    let mut z = f32::from_bits(hi as u32);
    if j & 1 == 1 {
        j += 1;
        j &= 7;
        z -= 1.0;
    }
    (j, z * PI4)
}

/// The hyperbolic tangent (Go `Tanh`, the Cephes single-precision code).
pub fn tanh(x: f32) -> f32 {
    tanh_with::<Native>(x)
}

fn tanh_with<K: Kernels>(x: f32) -> f32 {
    const HALF_MAXLOG: f32 = 44.014_845_935_754_205;
    let mut z = abs(x);
    if z > HALF_MAXLOG {
        if x < 0.0 {
            return -1.0;
        }
        return 1.0;
    } else if z >= 0.625 {
        let s = K::exp(z + z);
        z = 1.0 - 2.0 / (s + 1.0);
        if x < 0.0 {
            z = -z;
        }
    } else {
        if x == 0.0 {
            return x;
        }
        let s = x * x;
        z = ((((-5.704_988_727_45e-3 * s + 2.063_908_879_54e-2) * s - 5.373_971_555_31e-2) * s
            + 1.333_144_220_36e-1)
            * s
            - 3.333_328_194_22e-1)
            * s
            * x
            + x;
    }
    z
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod golden_tests;
