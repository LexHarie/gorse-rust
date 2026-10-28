// Ports: $GOROOT/src/internal/strconv/atof.go, deps.go (inf, nan) and the float constants of ftoa.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! `internal/strconv.ParseFloat`: decimal to binary floating point
//! conversion. Errors are the bare reasons (`ErrSyntax`, `ErrRange`); the
//! public functions in the parent module wrap them in a `NumError`.
//!
//! Go keeps whether to try the fast paths in a package variable (`optimize`,
//! which its tests flip with `SetOptimize`); here it is a parameter, so tests
//! running in parallel threads cannot see each other's setting.

use super::decimal::Decimal;
use super::uscale::{bool2, parse_float32, parse_float64};
use super::{NumErrorKind, lower, underscore_ok};

/// Go's `float32MantBits`.
pub(super) const FLOAT32_MANT_BITS: u32 = 23;
/// Go's `float32ExpBits`.
pub(super) const FLOAT32_EXP_BITS: u32 = 8;
/// Go's `float32Bias`.
pub(super) const FLOAT32_BIAS: i64 = -127;
/// Go's `float64MantBits`.
pub(super) const FLOAT64_MANT_BITS: u32 = 52;
/// Go's `float64ExpBits`.
pub(super) const FLOAT64_EXP_BITS: u32 = 11;
/// Go's `float64Bias`.
pub(super) const FLOAT64_BIAS: i64 = -1023;

/// Go's `floatInfo`.
#[derive(PartialEq, Eq)]
struct FloatInfo {
    mantbits: u32,
    expbits: u32,
    bias: i64,
}

static FLOAT32INFO: FloatInfo = FloatInfo {
    mantbits: FLOAT32_MANT_BITS,
    expbits: FLOAT32_EXP_BITS,
    bias: FLOAT32_BIAS,
};
static FLOAT64INFO: FloatInfo = FloatInfo {
    mantbits: FLOAT64_MANT_BITS,
    expbits: FLOAT64_EXP_BITS,
    bias: FLOAT64_BIAS,
};

// decimal to binary floating point conversion.
// Algorithm:
//   1) Store input in multiprecision decimal.
//   2) Multiply/divide decimal by powers of two until in range [0.5, 1)
//   3) Multiply by 2^precision and round to get mantissa.

/// Go's `inf(sign)`.
fn inf(sign: i64) -> f64 {
    f64::from_bits(if sign >= 0 {
        0x7FF0000000000000
    } else {
        0xFFF0000000000000
    })
}

/// Go's `nan()`: the NaN Go's `math.NaN` returns.
fn nan() -> f64 {
    f64::from_bits(0x7FF8000000000001)
}

/// Go's `float32(x)` for a float64 x, as amd64 and arm64 convert: a NaN
/// keeps its sign and the top bits of its payload and comes out quiet. (Rust
/// leaves NaN payloads of `as` unspecified, so they are spelled out.)
pub(super) fn narrow(x: f64) -> f32 {
    if x.is_nan() {
        let b = x.to_bits();
        let sign = (b >> 63) as u32;
        let payload = ((b >> 29) as u32) & 0x003F_FFFF;
        return f32::from_bits(sign << 31 | 0x7FC0_0000 | payload);
    }
    x as f32
}

/// Go's `float64(x)` for a float32 x, as amd64 and arm64 convert (see
/// [`narrow`]).
pub(super) fn widen(x: f32) -> f64 {
    if x.is_nan() {
        let b = x.to_bits();
        let sign = u64::from(b >> 31);
        let payload = u64::from(b & 0x003F_FFFF) << 29;
        return f64::from_bits(sign << 63 | 0x7FF8_0000_0000_0000 | payload);
    }
    f64::from(x)
}

/// `common_prefix_len_ignore_case` returns the length of the common
/// prefix of s and prefix, with the character case of s ignored.
/// The prefix argument must be all lower-case.
fn common_prefix_len_ignore_case(s: &[u8], prefix: &[u8]) -> usize {
    let n = prefix.len().min(s.len());
    for i in 0..n {
        let mut c = s[i];
        if c.is_ascii_uppercase() {
            c += b'a' - b'A';
        }
        if c != prefix[i] {
            return i;
        }
    }
    n
}

/// `special` returns the floating-point value for the special,
/// possibly signed floating-point representations inf, infinity,
/// and NaN. The result is `Some` if a prefix of s contains one
/// of these representations, with n the length of that prefix.
/// The character case is ignored.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    if s.is_empty() {
        return None;
    }
    let mut s = s;
    let mut sign = 1;
    let mut nsign = 0;
    match s[0] {
        b'+' | b'-' | b'i' | b'I' => {
            if s[0] == b'+' || s[0] == b'-' {
                if s[0] == b'-' {
                    sign = -1;
                }
                nsign = 1;
                s = &s[1..];
                // fallthrough
            }
            let mut n = common_prefix_len_ignore_case(s, b"infinity");
            // Anything longer than "inf" is ok, but if we
            // don't have "infinity", only consume "inf".
            if 3 < n && n < 8 {
                n = 3;
            }
            if n == 3 || n == 8 {
                return Some((inf(sign), nsign + n));
            }
        }
        b'n' | b'N' if common_prefix_len_ignore_case(s, b"nan") == 3 => {
            return Some((nan(), 3));
        }
        _ => {}
    }
    None
}

impl Decimal {
    /// Go's `(*decimal).set`: reads s into b, keeping at most
    /// [`super::decimal::DIGITS`] digits.
    pub fn set(&mut self, s: &[u8]) -> bool {
        let b = self;
        let mut i = 0;
        b.neg = false;
        b.trunc = false;

        // optional sign
        if i >= s.len() {
            return false;
        }
        match s[i] {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                b.neg = true;
            }
            _ => {}
        }

        // digits
        let mut sawdot = false;
        let mut sawdigits = false;
        while i < s.len() {
            if s[i] == b'_' {
                // readFloat already checked underscores
                i += 1;
                continue;
            } else if s[i] == b'.' {
                if sawdot {
                    return false;
                }
                sawdot = true;
                b.dp = b.nd as i64;
                i += 1;
                continue;
            } else if s[i].is_ascii_digit() {
                sawdigits = true;
                if s[i] == b'0' && b.nd == 0 {
                    // ignore leading zeros
                    b.dp -= 1;
                    i += 1;
                    continue;
                }
                if b.nd < b.d.len() {
                    b.d[b.nd] = s[i];
                    b.nd += 1;
                } else if s[i] != b'0' {
                    b.trunc = true;
                }
                i += 1;
                continue;
            }
            break;
        }
        if !sawdigits {
            return false;
        }
        if !sawdot {
            b.dp = b.nd as i64;
        }

        // optional exponent moves decimal point.
        // if we read a very large, very long number,
        // just be sure to move the decimal point by
        // a lot (say, 100000).  it doesn't matter if it's
        // not the exact number.
        if i < s.len() && lower(s[i]) == b'e' {
            i += 1;
            if i >= s.len() {
                return false;
            }
            let mut esign = 1;
            match s[i] {
                b'+' => i += 1,
                b'-' => {
                    i += 1;
                    esign = -1;
                }
                _ => {}
            }
            if i >= s.len() || !s[i].is_ascii_digit() {
                return false;
            }
            let mut e: i64 = 0;
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
                if s[i] == b'_' {
                    // readFloat already checked underscores
                    i += 1;
                    continue;
                }
                if e < 10000 {
                    e = e * 10 + i64::from(s[i]) - i64::from(b'0');
                }
                i += 1;
            }
            b.dp += e * esign;
        }

        i == s.len()
    }
}

/// What Go's `readFloat` returns.
struct ReadFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    /// The number of bytes consumed.
    i: usize,
    ok: bool,
}

/// `read_float` reads a decimal or hexadecimal mantissa and exponent from a
/// float string representation in s; the number may be followed by other
/// characters. It reports the number of bytes consumed (i), and whether the
/// number is valid (ok).
fn read_float(s: &[u8]) -> ReadFloat {
    let mut r = ReadFloat {
        mantissa: 0,
        exp: 0,
        neg: false,
        trunc: false,
        hex: false,
        i: 0,
        ok: false,
    };
    let mut underscores = false;

    // optional sign
    if r.i >= s.len() {
        return r;
    }
    match s[r.i] {
        b'+' => r.i += 1,
        b'-' => {
            r.i += 1;
            r.neg = true;
        }
        _ => {}
    }

    // digits
    let mut base: u64 = 10;
    let mut max_mant_digits = 19; // 10^19 fits in uint64
    let mut exp_char = b'e';
    if r.i + 2 < s.len() && s[r.i] == b'0' && lower(s[r.i + 1]) == b'x' {
        base = 16;
        max_mant_digits = 16; // 16^16 fits in uint64
        r.i += 2;
        exp_char = b'p';
        r.hex = true;
    }
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    while r.i < s.len() {
        let c = s[r.i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            sawdigits = true;
            if c == b'0' && nd == 0 {
                // ignore leading zeros
                dp -= 1;
                r.i += 1;
                continue;
            }
            nd += 1;
            if nd_mant < max_mant_digits {
                r.mantissa *= base;
                r.mantissa += u64::from(c - b'0');
                nd_mant += 1;
            } else if c != b'0' {
                r.trunc = true;
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower(c)) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                r.mantissa *= 16;
                r.mantissa += u64::from(lower(c) - b'a' + 10);
                nd_mant += 1;
            } else {
                r.trunc = true;
            }
        } else {
            break;
        }
        r.i += 1;
    }
    if !sawdigits {
        return r;
    }
    if !sawdot {
        dp = nd;
    }

    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }

    // optional exponent moves decimal point.
    // if we read a very large, very long number,
    // just be sure to move the decimal point by
    // a lot (say, 100000).  it doesn't matter if it's
    // not the exact number.
    if r.i < s.len() && lower(s[r.i]) == exp_char {
        r.i += 1;
        if r.i >= s.len() {
            return r;
        }
        let mut esign = 1;
        match s[r.i] {
            b'+' => r.i += 1,
            b'-' => {
                r.i += 1;
                esign = -1;
            }
            _ => {}
        }
        if r.i >= s.len() || !s[r.i].is_ascii_digit() {
            return r;
        }
        let mut e: i64 = 0;
        while r.i < s.len() && (s[r.i].is_ascii_digit() || s[r.i] == b'_') {
            if s[r.i] == b'_' {
                underscores = true;
                r.i += 1;
                continue;
            }
            if e < 10000 {
                e = e * 10 + i64::from(s[r.i]) - i64::from(b'0');
            }
            r.i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        // Must have exponent.
        return r;
    }

    if r.mantissa != 0 {
        r.exp = dp - nd_mant;
    }

    if underscores && !underscore_ok(&s[..r.i]) {
        return r;
    }

    r.ok = true;
    r
}

/// decimal power of ten to binary power of two.
static POWTAB: [i64; 9] = [1, 3, 6, 9, 13, 16, 19, 23, 26];

impl Decimal {
    /// Go's `(*decimal).floatBits`: the bits of the float nearest d, and
    /// whether it overflowed to ±Inf.
    fn float_bits(&mut self, flt: &FloatInfo) -> (u64, bool) {
        let d = self;
        let mut exp: i64;
        let mut mant: u64;
        let mut overflow = false;

        'out: {
            'overflow: {
                // Zero is always a special case.
                if d.nd == 0 {
                    mant = 0;
                    exp = flt.bias;
                    break 'out;
                }

                // Obvious overflow/underflow.
                // These bounds are for 64-bit floats.
                // Will have to change if we want to support 80-bit floats in the future.
                if d.dp > 310 {
                    break 'overflow;
                }
                if d.dp < -330 {
                    // zero
                    mant = 0;
                    exp = flt.bias;
                    break 'out;
                }

                // Scale by powers of two until in range [0.5, 1.0)
                exp = 0;
                while d.dp > 0 {
                    let n = if d.dp >= POWTAB.len() as i64 {
                        27
                    } else {
                        POWTAB[d.dp as usize]
                    };
                    d.shift(-n);
                    exp += n;
                }
                while d.dp < 0 || d.dp == 0 && d.d[0] < b'5' {
                    let n = if -d.dp >= POWTAB.len() as i64 {
                        27
                    } else {
                        POWTAB[(-d.dp) as usize]
                    };
                    d.shift(n);
                    exp -= n;
                }

                // Our range is [0.5,1) but floating point range is [1,2).
                exp -= 1;

                // Minimum representable exponent is flt.bias+1.
                // If the exponent is smaller, move it up and
                // adjust d accordingly.
                if exp < flt.bias + 1 {
                    let n = flt.bias + 1 - exp;
                    d.shift(-n);
                    exp += n;
                }

                if exp - flt.bias >= (1 << flt.expbits) - 1 {
                    break 'overflow;
                }

                // Extract 1+flt.mantbits bits.
                d.shift(i64::from(1 + flt.mantbits));
                mant = d.rounded_integer();

                // Rounding might have added a bit; shift down.
                if mant == 2 << flt.mantbits {
                    mant >>= 1;
                    exp += 1;
                    if exp - flt.bias >= (1 << flt.expbits) - 1 {
                        break 'overflow;
                    }
                }

                // Denormalized?
                if mant & (1 << flt.mantbits) == 0 {
                    exp = flt.bias;
                }
                break 'out;
            }

            // overflow:
            // ±Inf
            mant = 0;
            exp = (1 << flt.expbits) - 1 + flt.bias;
            overflow = true;
        }

        // out:
        // Assemble bits.
        let mut bits = mant & ((1u64 << flt.mantbits) - 1);
        bits |= (((exp - flt.bias) & ((1 << flt.expbits) - 1)) as u64) << flt.mantbits;
        if d.neg {
            bits |= 1 << flt.mantbits << flt.expbits;
        }
        (bits, overflow)
    }
}

/// Exact powers of 10.
static FLOAT64POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];
static FLOAT32POW10: [f32; 11] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];

/// If possible to convert decimal representation to 64-bit float f exactly,
/// entirely in floating-point math, do so, avoiding the expense of decimalToFloatBits.
/// Three common cases:
///
/// ```text
/// value is exact integer
/// value is exact integer * exact power of ten
/// value is exact integer / exact power of ten
/// ```
///
/// These all produce potentially inexact but correctly rounded answers.
fn atof64exact(mantissa: u64, exp: i64, neg: bool) -> Option<f64> {
    if mantissa >> FLOAT64INFO.mantbits != 0 {
        return None;
    }
    let mut f = mantissa as f64;
    if neg {
        f = -f;
    }
    let mut exp = exp;
    if exp == 0 {
        // an integer.
        return Some(f);
    }
    // Exact integers are <= 10^15.
    // Exact powers of ten are <= 10^22.
    if exp > 0 && exp <= 15 + 22 {
        // int * 10^k
        // If exponent is big but number of digits is not,
        // can move a few zeros into the integer part.
        if exp > 22 {
            f *= FLOAT64POW10[(exp - 22) as usize];
            exp = 22;
        }
        if !(-1e15..=1e15).contains(&f) {
            // the exponent was really too large.
            return None;
        }
        return Some(f * FLOAT64POW10[exp as usize]);
    }
    if (-22..0).contains(&exp) {
        // int / 10^k
        return Some(f / FLOAT64POW10[(-exp) as usize]);
    }
    None
}

/// If possible to compute mantissa*10^exp to 32-bit float f exactly,
/// entirely in floating-point math, do so, avoiding the machinery above.
fn atof32exact(mantissa: u64, exp: i64, neg: bool) -> Option<f32> {
    if mantissa >> FLOAT32_MANT_BITS != 0 {
        return None;
    }
    let mut f = mantissa as f32;
    if neg {
        f = -f;
    }
    let mut exp = exp;
    if exp == 0 {
        return Some(f);
    }
    // Exact integers are <= 10^7.
    // Exact powers of ten are <= 10^10.
    if exp > 0 && exp <= 7 + 10 {
        // int * 10^k
        // If exponent is big but number of digits is not,
        // can move a few zeros into the integer part.
        if exp > 10 {
            f *= FLOAT32POW10[(exp - 10) as usize];
            exp = 10;
        }
        if !(-1e7..=1e7).contains(&f) {
            // the exponent was really too large.
            return None;
        }
        return Some(f * FLOAT32POW10[exp as usize]);
    }
    if (-10..0).contains(&exp) {
        // int / 10^k
        return Some(f / FLOAT32POW10[(-exp) as usize]);
    }
    None
}

/// `atof_hex` converts the hex floating-point string s
/// to a rounded float32 or float64 value (depending on flt==&FLOAT32INFO or flt==&FLOAT64INFO)
/// and returns it as a float64.
/// The string s has already been parsed into a mantissa, exponent, and sign (neg==true for negative).
/// If trunc is true, trailing non-zero bits have been omitted from the mantissa.
fn atof_hex(
    flt: &FloatInfo,
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
) -> (f64, Option<NumErrorKind>) {
    let max_exp = (1 << flt.expbits) + flt.bias - 2;
    let min_exp = flt.bias + 1;
    let mut mantissa = mantissa;
    let mut exp = exp + i64::from(flt.mantbits); // mantissa now implicitly divided by 2^mantbits.

    // Shift mantissa and exponent to bring representation into float range.
    // Eventually we want a mantissa with a leading 1-bit followed by mantbits other bits.
    // For rounding, we need two more, where the bottom bit represents
    // whether that bit or any later bit was non-zero.
    // (If the mantissa has already lost non-zero bits, trunc is true,
    // and we OR in a 1 below after shifting left appropriately.)
    while mantissa != 0 && mantissa >> (flt.mantbits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + flt.mantbits + 2) != 0 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exp += 1;
    }

    // If exponent is too negative,
    // denormalize in hopes of making it representable.
    // (The -2 is for the rounding bits.)
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = mantissa >> 1 | mantissa & 1;
        exp += 1;
    }

    // Round using two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1; // round to even (round up if mantissa is odd)
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + flt.mantbits) {
            mantissa >>= 1;
            exp += 1;
        }
    }

    if mantissa >> flt.mantbits == 0 {
        // Denormal or zero.
        exp = flt.bias;
    }
    let mut err = None;
    if exp > max_exp {
        // infinity and range error
        mantissa = 1 << flt.mantbits;
        exp = max_exp + 1;
        err = Some(NumErrorKind::Range);
    }

    let mut bits = mantissa & ((1 << flt.mantbits) - 1);
    bits |= (((exp - flt.bias) & ((1 << flt.expbits) - 1)) as u64) << flt.mantbits;
    if neg {
        bits |= 1 << flt.mantbits << flt.expbits;
    }
    if flt == &FLOAT32INFO {
        return (widen(f32::from_bits(bits as u32)), err);
    }
    (f64::from_bits(bits), err)
}

/// Go's `atof32`, with Go's `optimize` as a parameter.
fn atof32(s: &[u8], optimize: bool) -> (f32, usize, Option<NumErrorKind>) {
    if let Some((val, n)) = special(s) {
        return (narrow(val), n, None);
    }

    let ReadFloat {
        mantissa: d,
        exp: p,
        neg,
        trunc,
        hex,
        i: n,
        ok,
    } = read_float(s);
    if !ok {
        return (0.0, n, Some(NumErrorKind::Syntax));
    }

    if hex {
        let (f, err) = atof_hex(&FLOAT32INFO, d, p, neg, trunc);
        return (narrow(f), n, err);
    }

    if optimize {
        let sign = (bool2(neg) as u32) << 31;
        if d == 0 {
            return (f32::from_bits(sign), n, None);
        }
        if p > 40 {
            // overflow to ±Inf
            return (
                f32::from_bits(sign | 0xff << 23),
                n,
                Some(NumErrorKind::Range),
            );
        }
        if p < -70 {
            // underflow to ±0
            return (f32::from_bits(sign), n, None);
        }
        if !trunc {
            // Exact rounding with single multiplication or division.
            if let Some(f) = atof32exact(d, p, neg) {
                return (f, n, None);
            }
        }
        // Use fast unrounded scaling.
        // The only possible err is ErrRange, when the result overflows to ±Inf.
        let (f, range) = parse_float32(d, p, sign);
        let err = range.then_some(NumErrorKind::Range);
        if !trunc {
            return (f, n, err);
        }
        // If additional digits were truncated from d
        // but d+1 converts to the same value,
        // then the additional digits don't matter.
        let (f1, _) = parse_float32(d + 1, p, sign);
        if f == f1 {
            return (f, n, err);
        }
    }

    // Slow fallback.
    let mut dec = Decimal::default();
    if !dec.set(&s[..n]) {
        return (0.0, n, Some(NumErrorKind::Syntax));
    }
    let (b, ovf) = dec.float_bits(&FLOAT32INFO);
    let f = f32::from_bits(b as u32);
    (f, n, ovf.then_some(NumErrorKind::Range))
}

/// Go's `atof64`, with Go's `optimize` as a parameter.
fn atof64(s: &[u8], optimize: bool) -> (f64, usize, Option<NumErrorKind>) {
    if let Some((val, n)) = special(s) {
        return (val, n, None);
    }

    let ReadFloat {
        mantissa: d,
        exp: p,
        neg,
        trunc,
        hex,
        i: n,
        ok,
    } = read_float(s);
    if !ok {
        return (0.0, n, Some(NumErrorKind::Syntax));
    }
    if hex {
        let (f, err) = atof_hex(&FLOAT64INFO, d, p, neg, trunc);
        return (f, n, err);
    }
    if optimize {
        let sign = bool2(neg) << 63;
        if d == 0 {
            return (f64::from_bits(sign), n, None);
        }
        if p > 310 {
            // overflow to ±Inf
            return (
                f64::from_bits(sign | 0x7ff << 52),
                n,
                Some(NumErrorKind::Range),
            );
        }
        if p < -345 {
            // underflow to ±0
            return (f64::from_bits(sign), n, None);
        }
        if !trunc {
            // Exact rounding with single multiplication or division.
            if let Some(f) = atof64exact(d, p, neg) {
                return (f, n, None);
            }
        }
        // Use fast unrounded scaling.
        // The only possible err is ErrRange, when the result overflows to ±Inf.
        let (f, range) = parse_float64(d, p, sign);
        let err = range.then_some(NumErrorKind::Range);
        if !trunc {
            return (f, n, err);
        }
        // If additional digits were truncated from d
        // but d+1 converts to the same value,
        // then the additional digits don't matter.
        let (f1, _) = parse_float64(d + 1, p, sign);
        if f == f1 {
            return (f, n, err);
        }
    }

    // Slow fallback.
    let mut dec = Decimal::default();
    if !dec.set(&s[..n]) {
        return (0.0, n, Some(NumErrorKind::Syntax));
    }
    let (b, ovf) = dec.float_bits(&FLOAT64INFO);
    let f = f64::from_bits(b);
    (f, n, ovf.then_some(NumErrorKind::Range))
}

/// `internal/strconv.ParseFloat`.
pub(super) fn parse_float(s: &[u8], bit_size: i64, optimize: bool) -> (f64, Option<NumErrorKind>) {
    let (f, n, err) = parse_float_prefix(s, bit_size, optimize);
    if n != s.len() {
        return (0.0, Some(NumErrorKind::Syntax));
    }
    (f, err)
}

/// `internal/strconv.parseFloatPrefix`.
pub(super) fn parse_float_prefix(
    s: &[u8],
    bit_size: i64,
    optimize: bool,
) -> (f64, usize, Option<NumErrorKind>) {
    if bit_size == 32 {
        let (f, n, err) = atof32(s, optimize);
        return (widen(f), n, err);
    }
    atof64(s, optimize)
}
