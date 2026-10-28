// Ports: common/floats/floats.go, floats_amd64.go, floats_arm64.go, floats_noasm.go, mm.go @ add21b5
// Ports: github.com/x448/float16 float16.go (f32bitsToF16bits, f16bitsToF32bits) @ v0.8.4
// Ports: golang.org/x/sys/cpu cpu.go (processOptions, for the x86 options floats reads) @ v0.48.0

//! Vector and matrix operations on `f32` slices, with the SIMD kernels Go
//! uses.
//!
//! On x86-64 the AVX and AVX-512 kernels, and on AArch64 the NEON kernels, are
//! the C sources of `common/floats/src` compiled by `build.rs`, so every level
//! computes exactly what Go computes at that level. As in Go, the level is
//! chosen once from the CPU and can be lowered with `GODEBUG=cpu.avx512f=off`,
//! `cpu.fma=off`, `cpu.avx=off` or `cpu.all=off`. Other architectures use the
//! scalar loops (`PORTING_NOTES.md` X-9).
//!
//! The functions panic with Go's message when slice lengths do not match. On
//! empty slices they return what the scalar loops return, where Go's SIMD
//! builds panic (X-9).

mod simd;

use std::fmt;
use std::sync::LazyLock;

/// A set of CPU features that selects the kernels, like Go's `floats.Feature`.
///
/// Any value is safe to use: the kernels only use the features the CPU
/// actually has, so a `Feature` naming more than the CPU provides behaves like
/// the features both have in common.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Feature(u64);

#[cfg(target_arch = "x86_64")]
impl Feature {
    /// AVX.
    pub const AVX: Feature = Feature(1);
    /// FMA.
    pub const FMA: Feature = Feature(1 << 1);
    /// AVX-512 Foundation.
    pub const AVX512F: Feature = Feature(1 << 2);
    /// Half-precision conversions (with AVX).
    pub const F16C: Feature = Feature(1 << 5);
    /// What the AVX-512 kernels need.
    pub const AVX512: Feature = Feature(Self::AVX.0 | Self::FMA.0 | Self::AVX512F.0);

    /// What the kernels may use: the features both `self` and the CPU have.
    fn usable(self) -> Feature {
        self & hardware()
    }
}

impl std::ops::BitOr for Feature {
    type Output = Feature;
    fn bitor(self, rhs: Feature) -> Feature {
        Feature(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Feature {
    type Output = Feature;
    fn bitand(self, rhs: Feature) -> Feature {
        Feature(self.0 & rhs.0)
    }
}

impl Feature {
    /// No SIMD kernels on x86-64: the scalar loops. (AArch64 always has NEON,
    /// so there every value selects the NEON kernels, as in Go.)
    pub const NONE: Feature = Feature(0);

    /// Whether every feature of `other` is in `self`.
    pub fn contains(self, other: Feature) -> bool {
        self.0 & other.0 == other.0
    }

    /// The features the package functions use: the CPU's, lowered by
    /// `GODEBUG` as Go does. Read once.
    pub fn current() -> Feature {
        static CURRENT: LazyLock<Feature> =
            LazyLock::new(|| Feature::detect(std::env::var("GODEBUG").ok().as_deref()));
        *CURRENT
    }

    /// The features of this CPU, lowered by a `GODEBUG` value the way
    /// `golang.org/x/sys/cpu` and `common/floats` read it.
    pub fn detect(godebug: Option<&str>) -> Feature {
        detect(hardware(), godebug.unwrap_or(""))
    }
}

/// The CPU's features, as Go's `init` in `floats_amd64.go` reads them.
#[cfg(target_arch = "x86_64")]
fn hardware() -> Feature {
    static HARDWARE: LazyLock<Feature> = LazyLock::new(|| {
        let mut f = Feature::NONE;
        if std::arch::is_x86_feature_detected!("avx") {
            f = f | Feature::AVX;
        }
        if std::arch::is_x86_feature_detected!("fma") {
            f = f | Feature::FMA;
        }
        if std::arch::is_x86_feature_detected!("avx512f") {
            f = f | Feature::AVX512F;
        }
        if std::arch::is_x86_feature_detected!("f16c") {
            f = f | Feature::F16C;
        }
        f
    });
    *HARDWARE
}

#[cfg(not(target_arch = "x86_64"))]
fn hardware() -> Feature {
    Feature::NONE
}

/// Applies `GODEBUG` to the CPU's features. Go processes the `cpu.<name>=on|off`
/// fields in order (a later field wins, `all` sets every option), and can only
/// turn a feature off; F16C is used only together with AVX.
#[cfg(target_arch = "x86_64")]
fn detect(cpu: Feature, godebug: &str) -> Feature {
    let options = [
        ("avx", Feature::AVX),
        ("fma", Feature::FMA),
        ("avx512f", Feature::AVX512F),
    ];
    let mut enabled = [None; 3];
    for field in godebug.split(',') {
        let Some(option) = field.strip_prefix("cpu.") else {
            continue;
        };
        let Some((key, value)) = option.split_once('=') else {
            continue;
        };
        let enable = match value {
            "on" => true,
            "off" => false,
            _ => continue,
        };
        if key == "all" {
            enabled = [Some(enable); 3];
        } else if let Some(i) = options.iter().position(|(name, _)| *name == key) {
            enabled[i] = Some(enable);
        }
    }
    let mut f = Feature::NONE;
    for ((_, feature), enable) in options.iter().zip(enabled) {
        if cpu.contains(*feature) && enable.unwrap_or(true) {
            f = f | *feature;
        }
    }
    // floats_amd64.go: `if cpu.X86.HasAVX && cpuid.CPU.Supports(cpuid.F16C)`.
    if f.contains(Feature::AVX) && cpu.contains(Feature::F16C) {
        f = f | Feature::F16C;
    }
    f
}

#[cfg(not(target_arch = "x86_64"))]
fn detect(_: Feature, _: &str) -> Feature {
    Feature::NONE
}

impl fmt::Display for Feature {
    /// Go's `Feature.String`.
    #[cfg(target_arch = "x86_64")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = Vec::new();
        if self.contains(Feature::AVX512) {
            names.push("AVX512");
        } else if self.contains(Feature::AVX) {
            names.push("AVX");
        }
        if self.contains(Feature::F16C) {
            names.push("F16C");
        }
        if names.is_empty() {
            return f.write_str("AMD64");
        }
        f.write_str(&names.join("+"))
    }

    /// Go's `Feature.String`.
    #[cfg(target_arch = "aarch64")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ARM64")
    }

    /// Go's `Feature.String` in `floats_noasm.go`.
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NOASM")
    }
}

const LENGTHS_DO_NOT_MATCH: &str = "floats: slice lengths do not match";

fn check(ok: bool) {
    assert!(ok, "{LENGTHS_DO_NOT_MATCH}");
}

/// One past the last element of a row-major matrix with `rows` rows of `cols`
/// used columns and leading dimension `ld`.
fn extent(rows: usize, ld: usize, cols: usize) -> usize {
    if rows == 0 || cols == 0 {
        return 0;
    }
    (rows - 1)
        .checked_mul(ld)
        .and_then(|x| x.checked_add(cols))
        .expect("floats: matrix too large")
}

/// Checks that slices of lengths `a`, `b` and `c` hold every element the
/// kernels' `mm` touches.
#[allow(clippy::too_many_arguments)]
fn check_mm(
    trans_a: bool,
    trans_b: bool,
    m: usize,
    n: usize,
    k: usize,
    a: usize,
    lda: usize,
    b: usize,
    ldb: usize,
    c: usize,
    ldc: usize,
) {
    let a_extent = if trans_a {
        extent(k, lda, m)
    } else {
        extent(m, lda, k)
    };
    let b_extent = if trans_b {
        extent(n, ldb, k)
    } else {
        extent(k, ldb, n)
    };
    assert!(
        a >= a_extent && b >= b_extent && c >= extent(m, ldc, n),
        "floats: matrix slices are too short for their dimensions"
    );
}

// ---------------------------------------------------------------------------
// The kernels at each level (`Feature` methods in Go). Each one runs the SIMD
// kernel the level selects and otherwise the scalar loop of `native`. Go's
// SIMD builds index `a[0]`, so they panic on empty slices; here empty slices
// go to the loops.

impl Feature {
    /// FP32 to FP16 bits: `dst[i] = f16(a[i])`, rounding to nearest even.
    pub fn from_float32(self, a: &[f32], dst: &mut [u16]) {
        check(a.len() == dst.len());
        if !a.is_empty() && simd::from_float32(self, a, dst) {
            return;
        }
        native::from_float32(a, dst);
    }

    /// FP16 bits to FP32: `dst[i] = f32(a[i])`.
    pub fn to_float32(self, a: &[u16], dst: &mut [f32]) {
        check(a.len() == dst.len());
        if !a.is_empty() && simd::to_float32(self, a, dst) {
            return;
        }
        native::to_float32(a, dst);
    }

    /// `dst = a * b + c`.
    pub fn mul_const_add_to(self, a: &[f32], b: f32, c: &[f32], dst: &mut [f32]) {
        check(a.len() == c.len() && a.len() == dst.len());
        if !a.is_empty() && simd::mul_const_add_to(self, a, b, c, dst) {
            return;
        }
        native::mul_const_add_to(a, b, c, dst);
    }

    /// `dst += a * b`.
    pub fn mul_const_add(self, a: &[f32], b: f32, dst: &mut [f32]) {
        check(a.len() == dst.len());
        if !a.is_empty() && simd::mul_const_add(self, a, b, dst) {
            return;
        }
        native::mul_const_add(a, b, dst);
    }

    /// `dst = a * b`.
    pub fn mul_const_to(self, a: &[f32], b: f32, dst: &mut [f32]) {
        check(a.len() == dst.len());
        if !a.is_empty() && simd::mul_const_to(self, a, b, dst) {
            return;
        }
        native::mul_const_to(a, b, dst);
    }

    /// `a *= b`.
    pub fn mul_const(self, a: &mut [f32], b: f32) {
        if !a.is_empty() && simd::mul_const(self, a, b) {
            return;
        }
        native::mul_const(a, b);
    }

    /// `a += b`.
    pub fn add_const(self, a: &mut [f32], b: f32) {
        if !a.is_empty() && simd::add_const(self, a, b) {
            return;
        }
        native::add_const(a, b);
    }

    /// `a -= b`.
    pub fn sub(self, a: &mut [f32], b: &[f32]) {
        check(a.len() == b.len());
        if !a.is_empty() && simd::sub(self, a, b) {
            return;
        }
        native::sub(a, b);
    }

    /// `c = a - b`.
    pub fn sub_to(self, a: &[f32], b: &[f32], c: &mut [f32]) {
        check(a.len() == b.len() && a.len() == c.len());
        if !a.is_empty() && simd::sub_to(self, a, b, c) {
            return;
        }
        native::sub_to(a, b, c);
    }

    /// `c = a * b`.
    pub fn mul_to(self, a: &[f32], b: &[f32], c: &mut [f32]) {
        check(a.len() == b.len() && a.len() == c.len());
        if !a.is_empty() && simd::mul_to(self, a, b, c) {
            return;
        }
        native::mul_to(a, b, c);
    }

    /// `c = a / b`.
    pub fn div_to(self, a: &[f32], b: &[f32], c: &mut [f32]) {
        check(a.len() == b.len() && a.len() == c.len());
        if !a.is_empty() && simd::div_to(self, a, b, c) {
            return;
        }
        native::div_to(a, b, c);
    }

    /// `b = sqrt(a)`.
    pub fn sqrt_to(self, a: &[f32], b: &mut [f32]) {
        check(a.len() == b.len());
        if !a.is_empty() && simd::sqrt_to(self, a, b) {
            return;
        }
        native::sqrt_to(a, b);
    }

    /// The dot product of `a` and `b`.
    pub fn dot(self, a: &[f32], b: &[f32]) -> f32 {
        check(a.len() == b.len());
        if !a.is_empty()
            && let Some(ret) = simd::dot(self, a, b)
        {
            return ret;
        }
        native::dot(a, b)
    }

    /// The Euclidean distance between `a` and `b`.
    pub fn euclidean(self, a: &[f32], b: &[f32]) -> f32 {
        check(a.len() == b.len());
        if !a.is_empty()
            && let Some(ret) = simd::euclidean(self, a, b)
        {
            return ret;
        }
        native::euclidean(a, b)
    }

    /// The matrix product `c (+)= op(a) * op(b)` with row-major `m`×`k` and
    /// `k`×`n` operands (`op` transposes when asked) and leading dimensions
    /// `lda`, `ldb`, `ldc`. As in Go, the product is added to `c` except when
    /// only `b` is transposed, where it replaces `c`.
    ///
    /// # Panics
    ///
    /// When a slice is too short for the dimensions. At the scalar level, as in
    /// Go, also when a row of `a`, `b` or `c` cut at its leading dimension runs
    /// past its slice, or when the leading dimensions of the rows a row
    /// operation combines differ.
    #[allow(clippy::too_many_arguments)]
    pub fn mm(
        self,
        trans_a: bool,
        trans_b: bool,
        m: usize,
        n: usize,
        k: usize,
        a: &[f32],
        lda: usize,
        b: &[f32],
        ldb: usize,
        c: &mut [f32],
        ldc: usize,
    ) {
        if k == 0 {
            // With no inner dimension every level leaves `c` as it is, except
            // that `a * bᵀ` stores empty dot products. Handled here so the
            // kernels never get pointers into empty `a` or `b`.
            check_mm(
                trans_a,
                trans_b,
                m,
                n,
                k,
                a.len(),
                lda,
                b.len(),
                ldb,
                c.len(),
                ldc,
            );
            if !trans_a && trans_b {
                for i in 0..m {
                    c[i * ldc..i * ldc + n].fill(0.0);
                }
            }
            return;
        }
        if simd::mm(self, trans_a, trans_b, m, n, k, a, lda, b, ldb, c, ldc) {
            return;
        }
        native::mm(self, trans_a, trans_b, m, n, k, a, lda, b, ldb, c, ldc);
    }
}

/// The scalar loops of `floats.go` and `mm.go`.
mod native {
    use super::{Feature, add_first, f16_bits_to_f32, f32_to_f16_bits, mul_first};

    pub(super) fn from_float32(a: &[f32], dst: &mut [u16]) {
        for (d, &x) in dst.iter_mut().zip(a) {
            *d = f32_to_f16_bits(x.to_bits());
        }
    }

    pub(super) fn to_float32(a: &[u16], dst: &mut [f32]) {
        for (d, &x) in dst.iter_mut().zip(a) {
            *d = f32::from_bits(f16_bits_to_f32(x));
        }
    }

    // The operands of `+` and `*` are in the order of the instructions Go
    // compiles, which decides which NaN a result carries (see `add_first`).

    pub(super) fn dot(a: &[f32], b: &[f32]) -> f32 {
        let mut ret = 0.0_f32;
        for (&x, &y) in a.iter().zip(b) {
            ret = add_first(ret, mul_first(x, y));
        }
        ret
    }

    pub(super) fn euclidean(a: &[f32], b: &[f32]) -> f32 {
        let mut ret = 0.0_f32;
        for (&x, &y) in a.iter().zip(b) {
            ret = add_first(ret, (x - y) * (x - y));
        }
        ret.sqrt()
    }

    pub(super) fn add_const(a: &mut [f32], c: f32) {
        for x in a {
            *x = add_first(*x, c);
        }
    }

    pub(super) fn sub(a: &mut [f32], b: &[f32]) {
        for (x, &y) in a.iter_mut().zip(b) {
            *x -= y;
        }
    }

    pub(super) fn sub_to(a: &[f32], b: &[f32], c: &mut [f32]) {
        for ((z, &x), &y) in c.iter_mut().zip(a).zip(b) {
            *z = x - y;
        }
    }

    pub(super) fn mul_to(a: &[f32], b: &[f32], c: &mut [f32]) {
        for ((z, &x), &y) in c.iter_mut().zip(a).zip(b) {
            *z = mul_first(x, y);
        }
    }

    pub(super) fn mul_const_add_to(a: &[f32], b: f32, c: &[f32], dst: &mut [f32]) {
        for ((d, &x), &y) in dst.iter_mut().zip(a).zip(c) {
            *d = add_first(mul_first(x, b), y);
        }
    }

    /// `dst[i] += a[i] * c`, which Go computes as `a[i] * c + dst[i]`.
    pub(super) fn mul_const_add(a: &[f32], c: f32, dst: &mut [f32]) {
        for (d, &x) in dst.iter_mut().zip(a) {
            *d = add_first(mul_first(x, c), *d);
        }
    }

    pub(super) fn mul_const_to(a: &[f32], b: f32, c: &mut [f32]) {
        for (z, &x) in c.iter_mut().zip(a) {
            *z = mul_first(x, b);
        }
    }

    pub(super) fn mul_const(a: &mut [f32], b: f32) {
        for x in a {
            *x = mul_first(*x, b);
        }
    }

    pub(super) fn div_to(a: &[f32], b: &[f32], c: &mut [f32]) {
        for ((z, &x), &y) in c.iter_mut().zip(a).zip(b) {
            *z = x / y;
        }
    }

    pub(super) fn sqrt_to(a: &[f32], b: &mut [f32]) {
        for (z, &x) in b.iter_mut().zip(a) {
            *z = x.sqrt();
        }
    }

    /// `mm.go`. Go's version calls the exported `MulConstAdd` and `Dot`, which
    /// run at the process's level; that level is the one that chose this
    /// loop, so here the row operations run at `f`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mm(
        f: Feature,
        trans_a: bool,
        trans_b: bool,
        m: usize,
        n: usize,
        k: usize,
        a: &[f32],
        lda: usize,
        b: &[f32],
        ldb: usize,
        c: &mut [f32],
        ldc: usize,
    ) {
        match (trans_a, trans_b) {
            (false, false) => {
                for i in 0..m {
                    for l in 0..k {
                        // C_l += A_{il} * B_i
                        f.mul_const_add(
                            &b[l * ldb..(l + 1) * ldb],
                            a[i * lda + l],
                            &mut c[i * ldc..(i + 1) * ldc],
                        );
                    }
                }
            }
            (false, true) => {
                for i in 0..m {
                    for j in 0..n {
                        c[i * ldc + j] =
                            f.dot(&a[i * lda..(i + 1) * lda], &b[j * ldb..(j + 1) * ldb]);
                    }
                }
            }
            (true, false) => {
                for i in 0..m {
                    for l in 0..k {
                        // C_j += A_{ji} * B_i
                        f.mul_const_add(
                            &b[l * ldb..(l + 1) * ldb],
                            a[l * lda + i],
                            &mut c[i * ldc..(i + 1) * ldc],
                        );
                    }
                }
            }
            (true, true) => {
                for i in 0..m {
                    for j in 0..n {
                        for l in 0..k {
                            c[i * ldc + j] = add_first(
                                c[i * ldc + j],
                                mul_first(a[l * lda + i], b[j * ldb + l]),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// x448/float16 `f32bitsToF16bits`: round to nearest, ties to even; NaNs stay
/// NaNs with the quiet bit set.
fn f32_to_f16_bits(u32: u32) -> u16 {
    let sign = u32 & 0x8000_0000;
    let exp = u32 & 0x7f80_0000;
    let coef = u32 & 0x007f_ffff;
    if exp == 0x7f80_0000 {
        // NaN or Infinity
        let nan_bit = if coef != 0 { 0x0200 } else { 0 };
        return low16((sign >> 16) | 0x7c00 | nan_bit | (coef >> 13));
    }
    let half_sign = sign >> 16;
    // `exp >> 23` is at most 0xff.
    let unbiased_exp = i32::try_from(exp >> 23).expect("8-bit exponent") - 127;
    let half_exp = unbiased_exp + 15;
    if half_exp >= 0x1f {
        return low16(half_sign | 0x7c00);
    }
    if half_exp <= 0 {
        if 14 - half_exp > 24 {
            return low16(half_sign);
        }
        let coef = coef | 0x0080_0000;
        // 14 - half_exp is in 14..=24 here.
        let shift = (14 - half_exp).unsigned_abs();
        let half_coef = coef >> shift;
        let round_bit = 1_u32 << (shift - 1);
        let half_coef = if (coef & round_bit) != 0 && (coef & (3 * round_bit - 1)) != 0 {
            half_coef + 1
        } else {
            half_coef
        };
        return low16(half_sign | half_coef);
    }
    // half_exp is in 1..=30 here.
    let u_half_exp = half_exp.unsigned_abs() << 10;
    let half_coef = coef >> 13;
    let round_bit = 0x0000_1000_u32;
    if (coef & round_bit) != 0 && (coef & (3 * round_bit - 1)) != 0 {
        return low16((half_sign | u_half_exp | half_coef) + 1);
    }
    low16(half_sign | u_half_exp | half_coef)
}

/// Go's `uint16(x)`: the low 16 bits.
fn low16(x: u32) -> u16 {
    u16::try_from(x & 0xffff).expect("masked to 16 bits")
}

/// x448/float16 `f16bitsToF32bits`.
fn f16_bits_to_f32(input: u16) -> u32 {
    let sign = u32::from(input & 0x8000) << 16;
    let mut exp = u32::from(input & 0x7c00) >> 10;
    let mut coef = u32::from(input & 0x03ff) << 13;
    if exp == 0x1f {
        if coef == 0 {
            // infinity
            return sign | 0x7f80_0000 | coef;
        }
        // NaN
        return sign | 0x7fc0_0000 | coef;
    }
    if exp == 0 {
        if coef == 0 {
            // zero
            return sign;
        }
        // normalize subnormal numbers
        exp += 1;
        while coef & 0x7f80_0000 == 0 {
            coef <<= 1;
            exp = exp.wrapping_sub(1);
        }
        coef &= 0x007f_ffff;
    }
    sign | (exp.wrapping_add(0x7f - 0xf) << 23) | coef
}

// ---------------------------------------------------------------------------
// The package functions.

/// `FromFloat32`: FP32 values to IEEE 754 FP16 bits.
pub fn from_float32(a: &[f32]) -> Vec<u16> {
    let mut ret = vec![0; a.len()];
    Feature::current().from_float32(a, &mut ret);
    ret
}

/// `ToFloat32`: IEEE 754 FP16 bits to FP32 values.
pub fn to_float32(a: &[u16]) -> Vec<f32> {
    let mut ret = vec![0.0; a.len()];
    Feature::current().to_float32(a, &mut ret);
    ret
}

/// `MatZero`.
pub fn mat_zero(x: &mut [Vec<f32>]) {
    for row in x {
        zero(row);
    }
}

/// `Zero`.
pub fn zero(a: &mut [f32]) {
    a.fill(0.0);
}

/// `SubTo`: `dst = a - b`.
pub fn sub_to(a: &[f32], b: &[f32], dst: &mut [f32]) {
    Feature::current().sub_to(a, b, dst);
}

/// `Add`: `dst += s`.
pub fn add(dst: &mut [f32], s: &[f32]) {
    check(dst.len() == s.len());
    for (d, &x) in dst.iter_mut().zip(s) {
        *d = add_first(*d, x);
    }
}

/// `MulConst`: `dst *= c`.
pub fn mul_const(dst: &mut [f32], c: f32) {
    Feature::current().mul_const(dst, c);
}

/// `Div`: `dst /= s`.
pub fn div(dst: &mut [f32], s: &[f32]) {
    check(dst.len() == s.len());
    for (d, &x) in dst.iter_mut().zip(s) {
        *d /= x;
    }
}

/// `MulTo`: `c = a * b`.
pub fn mul_to(a: &[f32], b: &[f32], c: &mut [f32]) {
    Feature::current().mul_to(a, b, c);
}

/// `Sub`: `dst -= s`.
pub fn sub(dst: &mut [f32], s: &[f32]) {
    Feature::current().sub(dst, s);
}

/// `MulConstTo`: `dst = a * c`.
pub fn mul_const_to(a: &[f32], c: f32, dst: &mut [f32]) {
    Feature::current().mul_const_to(a, c, dst);
}

/// `MulConstAddTo`: `dst = a * c + b`.
pub fn mul_const_add_to(a: &[f32], c: f32, b: &[f32], dst: &mut [f32]) {
    Feature::current().mul_const_add_to(a, c, b, dst);
}

/// `MulConstAdd`: `dst += a * c`.
pub fn mul_const_add(a: &[f32], c: f32, dst: &mut [f32]) {
    Feature::current().mul_const_add(a, c, dst);
}

/// `MulAddTo`: `c += a * b`, which Go computes as `a * b + c`.
pub fn mul_add_to(a: &[f32], b: &[f32], c: &mut [f32]) {
    check(a.len() == b.len() && a.len() == c.len());
    for ((z, &x), &y) in c.iter_mut().zip(a).zip(b) {
        *z = add_first(mul_first(x, y), *z);
    }
}

/// `AddTo`: `dst = a + b`.
pub fn add_to(a: &[f32], b: &[f32], dst: &mut [f32]) {
    check(a.len() == b.len() && a.len() == dst.len());
    for ((d, &x), &y) in dst.iter_mut().zip(a).zip(b) {
        *d = add_first(x, y);
    }
}

/// `x + y` as Go computes it on amd64, with `ADDSS y, x`: when both are NaNs
/// the result is `x` quieted, as x86 returns the NaN of the first operand
/// (Intel SDM vol. 1, table 4-7). LLVM may swap the operands of `+`, which
/// would carry `y`'s NaN instead, so the NaN is chosen here.
#[inline]
pub(crate) fn add_first(x: f32, y: f32) -> f32 {
    first_nan(x, y, x + y)
}

/// `x * y` with `x`'s NaN first, as [`add_first`].
#[inline]
pub(crate) fn mul_first(x: f32, y: f32) -> f32 {
    first_nan(x, y, x * y)
}

/// `r`, the result of an x86 arithmetic instruction on `x` and `y`, with the
/// NaN that instruction returns when `x` or `y` is one: `x`'s, else `y`'s,
/// quieted.
#[inline]
fn first_nan(x: f32, y: f32, r: f32) -> f32 {
    const QUIET: u32 = 0x0040_0000;
    if x.is_nan() {
        f32::from_bits(x.to_bits() | QUIET)
    } else if y.is_nan() {
        f32::from_bits(y.to_bits() | QUIET)
    } else {
        r
    }
}

/// `AddConst`: `dst += c`.
pub fn add_const(dst: &mut [f32], c: f32) {
    Feature::current().add_const(dst, c);
}

/// `DivTo`: `c = a / b`.
pub fn div_to(a: &[f32], b: &[f32], c: &mut [f32]) {
    Feature::current().div_to(a, b, c);
}

/// `SqrtTo`: `b = sqrt(a)`.
pub fn sqrt_to(a: &[f32], b: &mut [f32]) {
    Feature::current().sqrt_to(a, b);
}

/// `Sqrt`: `a = sqrt(a)`.
pub fn sqrt(a: &mut [f32]) {
    for x in a {
        *x = x.sqrt();
    }
}

/// `Dot`.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    Feature::current().dot(a, b)
}

/// `Euclidean`.
pub fn euclidean(a: &[f32], b: &[f32]) -> f32 {
    Feature::current().euclidean(a, b)
}

/// `MM`; see [`Feature::mm`].
#[allow(clippy::too_many_arguments)]
pub fn mm(
    trans_a: bool,
    trans_b: bool,
    m: usize,
    n: usize,
    k: usize,
    a: &[f32],
    lda: usize,
    b: &[f32],
    ldb: usize,
    c: &mut [f32],
    ldc: usize,
) {
    Feature::current().mm(trans_a, trans_b, m, n, k, a, lda, b, ldb, c, ldc);
}

#[cfg(test)]
mod tests;
