// Ports: $GOROOT/src/internal/strconv/ftoa.go (FormatFloat, ftoa32, ftoa64, fmtEFG, fmtB, fmtX), itoa.go (FormatInt, FormatUint) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `strconv.FormatFloat`, `FormatInt` and `FormatUint`.
//!
//! Go finds the digits of a float with its own algorithms (unrounded scaling
//! for the shortest digits and for up to 18 digits, a big decimal beyond).
//! Each gives a fixed answer: the shortest digits that read back as the same
//! float, the closest of them when there is a choice and the even one of two
//! equally close; or the exact value rounded half to even. Rust's `{:.N}`
//! formatting gives the same rounded digits, and its `{:e}` the same shortest
//! digits except at that tie, where it takes the larger; so this port takes
//! the digits from Rust, settles the tie as Go does, and ports Go's layout
//! (`fmtEFG`, `fmtB`, `fmtX`). `ftoa_golden.json` checks the result against
//! Go.

/// The significant decimal digits of a float's magnitude, Go's
/// `decimalSlice`: the value is `0.d * 10^dp`, with no leading or trailing
/// zeros in `d`. Zero has no digits.
#[derive(PartialEq)]
struct Digits {
    d: Vec<u8>,
    dp: i64,
}

impl Digits {
    /// Reads Rust's `{:e}` form, `d[.ddd]e<exp>`.
    fn from_exp(text: &str) -> Digits {
        let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
        let exp: i64 = exp.parse().unwrap_or(0);
        let mut d: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
        let dp = exp + 1;
        trim(&mut d);
        if d.is_empty() {
            return Digits { d, dp: 0 };
        }
        Digits { d, dp }
    }

    /// Reads Rust's `{:.N}` form, `ddd[.ddd]`.
    fn from_fixed(text: &str) -> Digits {
        let (int, frac) = text.split_once('.').unwrap_or((text, ""));
        let mut d: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
        let mut dp = int.len() as i64;
        let lead = d.iter().take_while(|&&c| c == b'0').count();
        d.drain(..lead);
        dp -= lead as i64;
        trim(&mut d);
        if d.is_empty() {
            return Digits { d, dp: 0 };
        }
        Digits { d, dp }
    }
}

/// The digits `d` (the value `0.d * 10^dp`) plus one in their last place.
fn round_up(d: &[u8], dp: i64) -> Digits {
    let mut d = d.to_vec();
    while let Some(last) = d.pop() {
        if last < b'9' {
            d.push(last + 1);
            return Digits { d, dp };
        }
    }
    // All nines: 0.99 becomes 0.1 * 10^(dp+1).
    Digits {
        d: vec![b'1'],
        dp: dp + 1,
    }
}

/// Drops trailing zeros, as Go's `setDigits` and `decimal.trim` do.
fn trim(d: &mut Vec<u8>) {
    while d.last() == Some(&b'0') {
        d.pop();
    }
}

/// A float's magnitude at the precision of its bit size.
#[derive(Clone, Copy)]
enum Magnitude {
    F32(f32),
    F64(f64),
}

impl Magnitude {
    /// The shortest digits that read back as the same float: the closest
    /// of them, and the even one of two equally close.
    fn shortest(self) -> Digits {
        let digits = Digits::from_exp(&match self {
            Magnitude::F32(f) => format!("{f:e}"),
            Magnitude::F64(f) => format!("{f:e}"),
        });
        // Two candidates of n digits are equally close only when the exact
        // value has n+1 digits and ends in 5. The shortest form has at most
        // 17 digits, so a value with more than 18 cannot tie; 40 digits
        // screen out nearly all of those, and the exact value decides.
        let screen = self.significant(40);
        if screen.d.len() > 18 || screen.d.last() != Some(&b'5') {
            return digits;
        }
        let exact = self.significant(800);
        let n = exact.d.len().saturating_sub(1);
        if n == 0 || exact.d.len() > 18 || exact.d.last() != Some(&b'5') {
            return digits;
        }
        let mut down = exact.d[..n].to_vec();
        trim(&mut down);
        let down = Digits {
            d: down,
            dp: exact.dp,
        };
        let up = round_up(&exact.d[..n], exact.dp);
        if digits != down && digits != up {
            return digits;
        }
        let (even, odd) = if (exact.d[n - 1] - b'0').is_multiple_of(2) {
            (down, up)
        } else {
            (up, down)
        };
        if self.reads_back(&even) { even } else { odd }
    }

    /// Whether `digits` read back as this float.
    fn reads_back(self, digits: &Digits) -> bool {
        let text = format!("0.{}e{}", String::from_utf8_lossy(&digits.d), digits.dp);
        match self {
            Magnitude::F32(f) => text.parse::<f32>() == Ok(f),
            Magnitude::F64(f) => text.parse::<f64>() == Ok(f),
        }
    }

    /// The exact value rounded half to even to `n` significant digits.
    fn significant(self, n: usize) -> Digits {
        let prec = n.saturating_sub(1);
        Digits::from_exp(&match self {
            Magnitude::F32(f) => format!("{f:.prec$e}"),
            Magnitude::F64(f) => format!("{f:.prec$e}"),
        })
    }

    /// The exact value rounded half to even to `prec` decimals.
    fn decimals(self, prec: usize) -> Digits {
        Digits::from_fixed(&match self {
            Magnitude::F32(f) => format!("{f:.prec$}"),
            Magnitude::F64(f) => format!("{f:.prec$}"),
        })
    }
}

/// Go's `strconv.FormatFloat(f, fmt, prec, bitSize)`. The format is one of
/// `b` (`-ddddp±ddd`), `e`/`E` (`-d.dddde±dd`), `f` (`-ddd.dddd`), `g`/`G`
/// (`e` for large exponents, `f` otherwise) or `x`/`X`
/// (`-0x1.ddddp±dd`); another byte gives Go's `%` and the byte. A negative
/// `prec` asks for the shortest digits that read back as the same value;
/// otherwise `prec` is the number of digits after the point (`e`, `f`, `x`)
/// or of significant digits (`g`). A `bit_size` of 32 formats `f` as a
/// `float32` (rounded first, as Go's `float32(f)` does).
///
/// # Panics
///
/// For a `bit_size` other than 32 or 64, as Go does.
pub fn format_float(f: f64, fmt: u8, prec: i64, bit_size: i64) -> String {
    // Go's ftoa32 and ftoa64 take the float apart the same way.
    let (bits, exp_bits, mant_bits, bias, magnitude) = match bit_size {
        32 => {
            #[allow(clippy::cast_possible_truncation)]
            let v = f as f32;
            (u64::from(v.to_bits()), 8, 23, -127, Magnitude::F32(v.abs()))
        }
        64 => (f.to_bits(), 11, 52, -1023, Magnitude::F64(f.abs())),
        _ => panic!("strconv: illegal FormatFloat bitSize"),
    };
    let neg = bits >> (exp_bits + mant_bits) != 0;
    let mut exp = ((bits >> mant_bits) & ((1 << exp_bits) - 1)) as i64;
    let mut mant = bits & ((1 << mant_bits) - 1);
    if exp == (1 << exp_bits) - 1 {
        if mant != 0 {
            return "NaN".to_owned();
        }
        return if neg { "-Inf" } else { "+Inf" }.to_owned();
    }
    if exp == 0 {
        exp += 1;
    } else {
        mant |= 1 << mant_bits;
    }
    exp += bias;

    // Pick off easy binary, hex formats.
    if fmt == b'b' {
        return fmt_b(neg, mant, exp - mant_bits);
    }
    if fmt == b'x' || fmt == b'X' {
        return fmt_x(prec, fmt, neg, mant, exp, mant_bits);
    }

    // Pick off zero.
    if mant == 0 {
        return fmt_efg(
            neg,
            &Digits {
                d: Vec::new(),
                dp: 0,
            },
            prec,
            fmt,
            prec < 0,
        );
    }

    // Negative precision means "only as much as needed to be exact."
    if prec < 0 {
        let digits = magnitude.shortest();
        let nd = digits.d.len() as i64;
        let prec = match fmt {
            b'e' | b'E' => (nd - 1).max(0),
            b'f' => (nd - digits.dp).max(0),
            b'g' | b'G' => nd,
            _ => prec,
        };
        return fmt_efg(neg, &digits, prec, fmt, true);
    }
    let mut prec = prec;
    let digits = match fmt {
        b'f' => magnitude.decimals(usize::try_from(prec).unwrap_or(usize::MAX)),
        b'e' | b'E' => magnitude.significant(usize::try_from(prec + 1).unwrap_or(usize::MAX)),
        b'g' | b'G' => {
            if prec == 0 {
                prec = 1;
            }
            magnitude.significant(usize::try_from(prec).unwrap_or(usize::MAX))
        }
        // An unknown format prints no digits.
        _ => Digits {
            d: Vec::new(),
            dp: 0,
        },
    };
    fmt_efg(neg, &digits, prec, fmt, false)
}

/// Go's `fmtEFG`: lays out the digits.
fn fmt_efg(neg: bool, digits: &Digits, mut prec: i64, mut fmt: u8, shortest: bool) -> String {
    let s = &digits.d;
    let nd = s.len() as i64;
    let dp = digits.dp;
    if fmt == b'g' || fmt == b'G' {
        // Trailing fractional zeros in 'e' form will be trimmed.
        let mut eprec = prec;
        if eprec > nd && nd >= dp {
            eprec = nd;
        }
        // %e is used if the exponent from the conversion is less than -4 or
        // greater than or equal to the precision. If precision was the
        // shortest possible, use precision 6 for this decision.
        if shortest {
            eprec = 6;
        }
        let exp = dp - 1;
        if exp < -4 || exp >= eprec {
            if prec > nd {
                prec = nd;
            }
            prec -= 1;
            fmt = fmt + b'e' - b'g';
        } else {
            if prec > dp {
                prec = nd;
            }
            prec = (prec - dp).max(0);
            fmt = b'f';
        }
    }
    let mut out = Vec::new();
    match fmt {
        b'e' | b'E' => {
            if neg {
                out.push(b'-');
            }
            out.push(s.first().copied().unwrap_or(b'0'));
            if prec > 0 {
                out.push(b'.');
                let m = nd.min(prec + 1);
                if m > 1 {
                    out.extend_from_slice(&s[1..m as usize]);
                }
                out.extend(std::iter::repeat_n(b'0', (prec + 1 - m.max(1)) as usize));
            }
            out.push(fmt);
            // Zero has exponent 0.
            let mut exp = if nd == 0 { 0 } else { dp - 1 };
            if exp < 0 {
                out.push(b'-');
                exp = -exp;
            } else {
                out.push(b'+');
            }
            let digit = |v: i64| b'0' + (v % 10) as u8;
            if exp < 10 {
                out.extend_from_slice(&[b'0', digit(exp)]);
            } else if exp < 100 {
                out.extend_from_slice(&[digit(exp / 10), digit(exp)]);
            } else {
                out.extend_from_slice(&[digit(exp / 100), digit(exp / 10), digit(exp)]);
            }
        }
        b'f' => {
            if neg {
                out.push(b'-');
            }
            // Integer, padded with zeros as needed.
            if dp > 0 {
                let m = nd.min(dp);
                out.extend_from_slice(&s[..m as usize]);
                out.extend(std::iter::repeat_n(b'0', (dp - m) as usize));
            } else {
                out.push(b'0');
            }
            // Fraction.
            if prec > 0 {
                out.push(b'.');
                let lz = prec.min((-dp).max(0));
                let off = dp + lz;
                let m = (prec - lz).min((nd - off).max(0));
                let tz = (prec - lz - m).max(0);
                out.extend(std::iter::repeat_n(b'0', lz as usize));
                if m > 0 {
                    out.extend_from_slice(&s[off as usize..(off + m) as usize]);
                }
                out.extend(std::iter::repeat_n(b'0', tz as usize));
            }
        }
        _ => {
            out.push(b'%');
            out.push(fmt);
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Go's `fmtB`: `-ddddddddp±ddd`.
fn fmt_b(neg: bool, mant: u64, exp: i64) -> String {
    let sign = if neg { "-" } else { "" };
    let exp_sign = if exp >= 0 { "+" } else { "" };
    format!("{sign}{mant}p{exp_sign}{exp}")
}

/// Go's `fmtX`: `-0x1.yyyyyyyyp±ddd` or `-0x0p+00` (`y` a hex digit, `d` a
/// decimal digit).
fn fmt_x(prec: i64, fmt: u8, neg: bool, mut mant: u64, mut exp: i64, mant_bits: i64) -> String {
    if mant == 0 {
        exp = 0;
    }

    // Shift digits so leading 1 (if any) is at bit 1<<60.
    mant <<= 60 - mant_bits;
    while mant != 0 && mant & (1 << 60) == 0 {
        mant <<= 1;
        exp -= 1;
    }

    // Round if requested.
    if (0..15).contains(&prec) {
        let shift = (prec * 4) as u32;
        let extra = (mant << shift) & ((1 << 60) - 1);
        mant >>= 60 - shift;
        if extra | (mant & 1) > 1 << 59 {
            mant += 1;
        }
        mant <<= 60 - shift;
        if mant & (1 << 61) != 0 {
            // Wrapped around.
            mant >>= 1;
            exp += 1;
        }
    }

    let hex: &[u8; 16] = if fmt == b'X' {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };

    // sign, 0x, leading digit
    let mut out = Vec::new();
    if neg {
        out.push(b'-');
    }
    out.extend_from_slice(&[b'0', fmt, b'0' + ((mant >> 60) & 1) as u8]);

    // .fraction
    mant <<= 4; // remove leading 0 or 1
    if prec < 0 && mant != 0 {
        out.push(b'.');
        while mant != 0 {
            out.push(hex[((mant >> 60) & 15) as usize]);
            mant <<= 4;
        }
    } else if prec > 0 {
        out.push(b'.');
        for _ in 0..prec {
            out.push(hex[((mant >> 60) & 15) as usize]);
            mant <<= 4;
        }
    }

    // p±
    out.push(if fmt == b'X' { b'P' } else { b'p' });
    if exp < 0 {
        out.push(b'-');
        exp = -exp;
    } else {
        out.push(b'+');
    }

    // dd or ddd or dddd
    let digit = |v: i64| b'0' + (v % 10) as u8;
    if exp < 100 {
        out.extend_from_slice(&[digit(exp / 10), digit(exp)]);
    } else if exp < 1000 {
        out.extend_from_slice(&[digit(exp / 100), digit(exp / 10), digit(exp)]);
    } else {
        out.extend_from_slice(&[
            digit(exp / 1000),
            digit(exp / 100),
            digit(exp / 10),
            digit(exp),
        ]);
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Go's `strconv.FormatInt`: `i` in `base`, with lower-case letters for
/// digits above 9.
///
/// # Panics
///
/// For a base outside 2 to 36, as Go does.
pub fn format_int(i: i64, base: u32) -> String {
    let digits = format_uint(i.unsigned_abs(), base);
    if i < 0 { format!("-{digits}") } else { digits }
}

/// Go's `strconv.FormatUint`: `i` in `base`, with lower-case letters for
/// digits above 9.
///
/// # Panics
///
/// For a base outside 2 to 36, as Go does.
pub fn format_uint(mut i: u64, base: u32) -> String {
    assert!(
        (2..=36).contains(&base),
        "strconv: illegal AppendInt/FormatInt base"
    );
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let base = u64::from(base);
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(i % base) as usize]);
        i /= base;
        if i == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::approx_constant)]
    fn go_doc_examples() {
        // From Go's ExampleFormatFloat.
        let v = 3.1415926535;
        assert_eq!(format_float(v, b'E', -1, 32), "3.1415927E+00");
        assert_eq!(format_float(v, b'E', -1, 64), "3.1415926535E+00");
        assert_eq!(format_float(1e21, b'f', -1, 64), "1000000000000000000000");
        assert_eq!(format_float(0.0, b'e', 3, 64), "0.000e+00");
        assert_eq!(format_float(-0.0, b'g', -1, 64), "-0");
        assert_eq!(format_float(0.125, b'f', 2, 64), "0.12");
        assert_eq!(format_float(1.0, b'g', 5, 64), "1");
        assert_eq!(format_float(f64::NAN, b'f', 3, 64), "NaN");
        assert_eq!(format_float(1.5, b'q', 2, 64), "%q");
        assert_eq!(format_int(-255, 16), "-ff");
        assert_eq!(format_int(i64::MIN, 2).len(), 65);
        assert_eq!(format_uint(u64::MAX, 36), "3w5e11264sgsf");
    }
}
