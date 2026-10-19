// Ports: $GOROOT/src/strconv/atof.go (ParseFloat), $GOROOT/src/internal/strconv/atof.go (readFloat, special, atofHex) @ go1.27.0
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `strconv.ParseFloat`, with Go's accepted syntax, rounding and error
//! texts.

use super::{NumError, NumErrorKind, lower, num_error, underscore_ok};

/// Mantissa width, exponent width and bias of a binary float format.
struct FloatInfo {
    mantbits: u32,
    expbits: u32,
    bias: i64,
}

const FLOAT32_INFO: FloatInfo = FloatInfo {
    mantbits: 23,
    expbits: 8,
    bias: -127,
};
const FLOAT64_INFO: FloatInfo = FloatInfo {
    mantbits: 52,
    expbits: 11,
    bias: -1023,
};

/// Length of the common prefix of `s` and the lower-case `prefix`, ignoring
/// the case of `s`.
fn common_prefix_len_ignore_case(s: &[u8], prefix: &[u8]) -> usize {
    s.iter()
        .zip(prefix)
        .take_while(|(c, p)| c.to_ascii_lowercase() == **p)
        .count()
}

/// Go's `math.NaN()`, the NaN `ParseFloat` returns.
const GO_NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0001);

/// Go's `special`: `inf`, `infinity` (optionally signed) and `nan`, ignoring
/// case, as a prefix of `s`. Returns the value and the bytes it took.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let (sign, nsign, rest) = match s.first()? {
        b'+' => (1.0, 1, &s[1..]),
        b'-' => (-1.0, 1, &s[1..]),
        b'i' | b'I' => (1.0, 0, s),
        b'n' | b'N' => {
            return (common_prefix_len_ignore_case(s, b"nan") == 3).then_some((GO_NAN, 3));
        }
        _ => return None,
    };
    let mut n = common_prefix_len_ignore_case(rest, b"infinity");
    // Anything longer than "inf" is ok, but without all of "infinity" only
    // "inf" is taken.
    if 3 < n && n < 8 {
        n = 3;
    }
    (n == 3 || n == 8).then_some((sign * f64::INFINITY, nsign + n))
}

/// What Go's `readFloat` reads, plus every significant decimal digit.
struct Mantissa {
    /// The first 19 decimal or 16 hex significant digits.
    mantissa: u64,
    /// The power of 10 (or of 2 for hex) to multiply `mantissa` by.
    exp: i64,
    neg: bool,
    /// Non-zero digits were left out of `mantissa`.
    trunc: bool,
    hex: bool,
    /// Bytes consumed.
    n: usize,
    /// All significant decimal digits (empty for hex), without leading zeros.
    digits: Vec<u8>,
    /// The decimal point: the value is `0.<digits> * 10^dp`.
    dp: i64,
}

/// Go's `readFloat`: a decimal or hex mantissa and exponent at the start of
/// `s`, or `None` for a syntax error.
fn read_float(s: &[u8]) -> Option<Mantissa> {
    let mut i = 0;
    let mut underscores = false;
    let mut neg = false;
    match *s.first()? {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            neg = true;
        }
        _ => {}
    }
    let mut base = 10;
    let mut max_mant_digits = 19;
    let mut exp_char = b'e';
    let mut hex = false;
    if i + 2 < s.len() && s[i] == b'0' && lower(s[i + 1]) == b'x' {
        base = 16;
        max_mant_digits = 16;
        i += 2;
        exp_char = b'p';
        hex = true;
    }
    let mut sawdot = false;
    let mut sawdigits = false;
    let mut nd: i64 = 0;
    let mut nd_mant: i64 = 0;
    let mut dp: i64 = 0;
    let mut mantissa: u64 = 0;
    let mut trunc = false;
    let mut digits = Vec::new();
    while i < s.len() {
        let c = s[i];
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
                // Leading zeros.
                dp -= 1;
            } else {
                nd += 1;
                if nd_mant < max_mant_digits {
                    mantissa = mantissa * base + u64::from(c - b'0');
                    nd_mant += 1;
                } else if c != b'0' {
                    trunc = true;
                }
                if !hex {
                    digits.push(c);
                }
            }
        } else if hex && (b'a'..=b'f').contains(&lower(c)) {
            sawdigits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                mantissa = mantissa * 16 + u64::from(lower(c) - b'a' + 10);
                nd_mant += 1;
            } else {
                trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !sawdigits {
        return None;
    }
    if !sawdot {
        dp = nd;
    }
    if hex {
        dp *= 4;
        nd_mant *= 4;
    }
    if i < s.len() && lower(s[i]) == exp_char {
        i += 1;
        let mut esign = 1;
        match *s.get(i)? {
            b'+' => i += 1,
            b'-' => {
                i += 1;
                esign = -1;
            }
            _ => {}
        }
        if !s.get(i)?.is_ascii_digit() {
            return None;
        }
        // A very long exponent only needs to move the point a lot, so Go
        // stops accumulating it at 10000.
        let mut e: i64 = 0;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if e < 10000 {
                e = e * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        dp += e * esign;
    } else if hex {
        // A hex mantissa must have an exponent.
        return None;
    }
    let exp = if mantissa != 0 { dp - nd_mant } else { 0 };
    if underscores && !underscore_ok(&s[..i]) {
        return None;
    }
    Some(Mantissa {
        mantissa,
        exp,
        neg,
        trunc,
        hex,
        n: i,
        digits,
        dp,
    })
}

/// Go's `atofHex`: round a hex mantissa and binary exponent to the format,
/// returning the value and whether it overflowed.
fn atof_hex(
    flt: &FloatInfo,
    mut mantissa: u64,
    mut exp: i64,
    neg: bool,
    trunc: bool,
) -> (f64, bool) {
    let max_exp = (1i64 << flt.expbits) + flt.bias - 2;
    let min_exp = flt.bias + 1;
    exp += i64::from(flt.mantbits);
    // Normalise to a leading 1 followed by mantbits bits and two rounding
    // bits, the last of which is sticky.
    while mantissa != 0 && mantissa >> (flt.mantbits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + flt.mantbits + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Denormalise when the exponent is too small.
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Round to nearest even with the two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
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
    let mut overflow = false;
    if exp > max_exp {
        mantissa = 1 << flt.mantbits;
        exp = max_exp + 1;
        overflow = true;
    }
    let mut bits = mantissa & ((1 << flt.mantbits) - 1);
    bits |= (((exp - flt.bias) & ((1 << flt.expbits) - 1)) as u64) << flt.mantbits;
    if neg {
        bits |= 1 << flt.mantbits << flt.expbits;
    }
    if flt.mantbits == FLOAT32_INFO.mantbits {
        (f64::from(f32::from_bits(bits as u32)), overflow)
    } else {
        (f64::from_bits(bits), overflow)
    }
}

/// The value of a parsed float of `bit_size` bits, and whether it overflowed
/// to an infinity.
fn convert(m: &Mantissa, bit_size: i64) -> (f64, bool) {
    let flt = if bit_size == 32 {
        &FLOAT32_INFO
    } else {
        &FLOAT64_INFO
    };
    if m.hex {
        return atof_hex(flt, m.mantissa, m.exp, m.neg, m.trunc);
    }
    if m.digits.is_empty() {
        return (if m.neg { -0.0 } else { 0.0 }, false);
    }
    // Go rounds the exact value 0.<digits> * 10^dp correctly (the exponent
    // already capped as Go caps it), and so does Rust's parser, so the
    // digits are handed to it in that form.
    let digits = std::str::from_utf8(&m.digits).unwrap_or_default();
    let text = format!("{}0.{digits}e{}", if m.neg { "-" } else { "" }, m.dp);
    let value = if bit_size == 32 {
        text.parse::<f32>().map(f64::from)
    } else {
        text.parse::<f64>()
    };
    // The text is well-formed by construction.
    let value = value.unwrap_or(f64::NAN);
    (value, value.is_infinite())
}

/// `strconv.ParseFloat(s, bit_size)`: Go's decimal and hex float syntax with
/// `_` separators, `inf`, `infinity` and `nan`, rounded to nearest even. A
/// bit size of 32 rounds to `f32` (the result converts to `f32` exactly);
/// any other size means 64. As in Go, a syntax error comes with 0 and an
/// out-of-range value with the infinity of its sign.
pub fn parse_float(s: impl AsRef<[u8]>, bit_size: i64) -> (f64, Option<NumError>) {
    let s = s.as_ref();
    let syntax = || (0.0, Some(num_error("ParseFloat", s, NumErrorKind::Syntax)));
    if let Some((value, n)) = special(s) {
        if n != s.len() {
            return syntax();
        }
        return (
            if bit_size == 32 {
                f64::from(value as f32)
            } else {
                value
            },
            None,
        );
    }
    let Some(m) = read_float(s) else {
        return syntax();
    };
    if m.n != s.len() {
        return syntax();
    }
    match convert(&m, bit_size) {
        (value, true) => (value, Some(num_error("ParseFloat", s, NumErrorKind::Range))),
        (value, false) => (value, None),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_like_go() {
        let (value, err) = parse_float("1x", 64);
        assert_eq!(value.to_bits(), 0);
        assert_eq!(
            err.expect("syntax error").to_string(),
            r#"strconv.ParseFloat: parsing "1x": invalid syntax"#
        );
        let (value, err) = parse_float("-1e400", 64);
        assert_eq!(value, f64::NEG_INFINITY);
        assert_eq!(
            err.expect("range error").to_string(),
            r#"strconv.ParseFloat: parsing "-1e400": value out of range"#
        );
    }
}
