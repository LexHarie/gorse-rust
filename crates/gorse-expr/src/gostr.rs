// Ports: the parts of Go's strconv and fmt the parser formats and parses with (go1.27.0: strconv/quote.go appendQuotedWith, appendEscapedRune, IsPrint; strconv/atoi.go ParseUint, ParseInt; strconv/atof.go ParseFloat; fmt %#U; fmt %v of a float64 is gorse_common::gostrconv::format_float)

//! Go's quoting, number parsing and number formatting, as the expr-lang parser
//! and AST printer use them.

use std::fmt::Write as _;

use crate::unicode_tables;

/// Whether `c` is in a table of sorted, disjoint, inclusive ranges.
pub(crate) fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let c = u32::from(c);
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `strconv.IsPrint`.
pub(crate) fn is_print(c: char) -> bool {
    in_table(unicode_tables::PRINT, c)
}

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

fn push_hex_byte(out: &mut String, b: u8) {
    out.push_str("\\x");
    out.push(char::from(LOWER_HEX[usize::from(b >> 4)]));
    out.push(char::from(LOWER_HEX[usize::from(b & 0xf)]));
}

/// `strconv.Quote` of a Go string, which may hold any bytes: `%q`, and `%#v`
/// of a string. A byte that does not start valid UTF-8 is written `\xNN`.
pub(crate) fn quote(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for chunk in s.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_escaped_rune(&mut out, c);
        }
        // Go decodes the bytes one at a time and escapes each that does not
        // begin a valid sequence; none of an invalid chunk's bytes does.
        for &b in chunk.invalid() {
            push_hex_byte(&mut out, b);
        }
    }
    out.push('"');
    out
}

fn push_escaped_rune(out: &mut String, c: char) {
    if c == '"' || c == '\\' {
        out.push('\\');
        out.push(c);
        return;
    }
    if is_print(c) {
        out.push(c);
        return;
    }
    match c {
        '\u{7}' => out.push_str("\\a"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{b}' => out.push_str("\\v"),
        _ => {
            let r = u32::from(c);
            if r < 0x20 || r == 0x7f {
                // Below 0x80, so it fits a byte.
                push_hex_byte(out, r as u8);
            } else if r < 0x10000 {
                let _ = write!(out, "\\u{r:04x}");
            } else {
                let _ = write!(out, "\\U{r:08x}");
            }
        }
    }
}

/// `%#U`: `U+0041 'A'`, or `U+0007` when the character is not printable.
pub(crate) fn format_unicode_sharp(c: char) -> String {
    let mut out = format!("U+{:04X}", u32::from(c));
    if is_print(c) {
        out.push_str(" '");
        out.push(c);
        out.push('\'');
    }
    out
}

/// `%v` of a float64, which is `strconv.FormatFloat(v, 'g', -1, 64)`: the
/// shortest digits that read back as `v`, in `%e` form when the decimal
/// exponent is below -4 or at least 6.
pub(crate) fn format_float_v(v: f64) -> String {
    gorse_common::gostrconv::format_float(v, b'g', -1, 64)
}

/// Why a strconv parse failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumErrorKind {
    Syntax,
    Range,
}

/// `*strconv.NumError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumError {
    func: &'static str,
    num: String,
    kind: NumErrorKind,
}

impl std::fmt::Display for NumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let err = match self.kind {
            NumErrorKind::Syntax => "invalid syntax",
            NumErrorKind::Range => "value out of range",
        };
        write!(
            f,
            "strconv.{}: parsing {}: {err}",
            self.func,
            quote(self.num.as_bytes())
        )
    }
}

fn num_error(func: &'static str, num: &str, kind: NumErrorKind) -> NumError {
    NumError {
        func,
        num: num.to_owned(),
        kind,
    }
}

/// `strconv.ParseUint(s, base, 64)` for base 0 (prefix `0b`, `0o`, `0x` or
/// `0`, any case) and base 10. The parser removes `_` from a number before
/// parsing it, so the underscore rules of base 0 are not ported.
fn parse_uint(s: &str, base0: bool) -> Result<u64, NumError> {
    const FUNC: &str = "ParseUint";
    if s.is_empty() {
        return Err(num_error(FUNC, s, NumErrorKind::Syntax));
    }
    let bytes = s.as_bytes();
    let mut digits = bytes;
    let mut base = 10u64;
    if base0 && bytes[0] == b'0' {
        let prefix = bytes.get(1).map(|b| b | 0x20);
        match prefix {
            Some(b'b') if bytes.len() >= 3 => (base, digits) = (2, &bytes[2..]),
            Some(b'o') if bytes.len() >= 3 => (base, digits) = (8, &bytes[2..]),
            Some(b'x') if bytes.len() >= 3 => (base, digits) = (16, &bytes[2..]),
            _ => (base, digits) = (8, &bytes[1..]),
        }
    }
    // Cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / base + 1;
    let mut n: u64 = 0;
    for &c in digits {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            _ if c.is_ascii_alphabetic() => (c | 0x20) - b'a' + 10,
            _ => return Err(num_error(FUNC, s, NumErrorKind::Syntax)),
        };
        if u64::from(d) >= base {
            return Err(num_error(FUNC, s, NumErrorKind::Syntax));
        }
        if n >= cutoff {
            // n*base overflows
            return Err(num_error(FUNC, s, NumErrorKind::Range));
        }
        n *= base;
        let Some(n1) = n.checked_add(u64::from(d)) else {
            // n+d overflows
            return Err(num_error(FUNC, s, NumErrorKind::Range));
        };
        n = n1;
    }
    Ok(n)
}

/// `strconv.ParseInt(s, 0, 64)` (`base0`) or `strconv.ParseInt(s, 10, 64)`.
pub(crate) fn parse_int(s: &str, base0: bool) -> Result<i64, NumError> {
    const FUNC: &str = "ParseInt";
    if s.is_empty() {
        return Err(num_error(FUNC, s, NumErrorKind::Syntax));
    }
    let (neg, unsigned) = match s.as_bytes()[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = match parse_uint(unsigned, base0) {
        Ok(un) => un,
        Err(e) if e.kind == NumErrorKind::Range => u64::MAX,
        Err(e) => return Err(num_error(FUNC, s, e.kind)),
    };
    const CUTOFF: u64 = 1 << 63;
    if !neg && un >= CUTOFF {
        return Err(num_error(FUNC, s, NumErrorKind::Range));
    }
    if neg && un > CUTOFF {
        return Err(num_error(FUNC, s, NumErrorKind::Range));
    }
    // In range: |n| <= 2^63, and 2^63 only when negative.
    Ok(if neg {
        0i64.wrapping_sub_unsigned(un)
    } else {
        un as i64
    })
}

/// `strconv.ParseFloat(s, 64)` for what the lexer can hand it: a decimal
/// number of digits with at most one point and an optional exponent, or such
/// a string with a `0b`/`0o` prefix (which Go rejects too). Go reads hex
/// floats, signs, `inf` and `nan` as well; the lexer never produces them.
///
/// Rust's parser rounds correctly, like Go's; Go reports a value that
/// rounds to infinity as out of range.
pub(crate) fn parse_float(s: &str) -> Result<f64, NumError> {
    const FUNC: &str = "ParseFloat";
    let syntax = || num_error(FUNC, s, NumErrorKind::Syntax);
    let digits_or_point = |b: &u8| b.is_ascii_digit() || *b == b'.';
    let bytes = s.as_bytes();
    // Rust accepts more forms (`inf`, `nan`, signs) than the lexer gives;
    // keep to the shape both agree on.
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(at) => (&bytes[..at], Some(&bytes[at + 1..])),
        None => (bytes, None),
    };
    let points = mantissa.iter().filter(|&&b| b == b'.').count();
    if mantissa.is_empty()
        || !mantissa.iter().all(digits_or_point)
        || points > 1
        || !mantissa.iter().any(u8::is_ascii_digit)
    {
        return Err(syntax());
    }
    if let Some(exponent) = exponent {
        let digits = match exponent.first() {
            Some(b'+' | b'-') => &exponent[1..],
            _ => exponent,
        };
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(syntax());
        }
    }
    let v: f64 = s.parse().map_err(|_| syntax())?;
    if v.is_infinite() {
        return Err(num_error(FUNC, s, NumErrorKind::Range));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_bounds() {
        assert_eq!(parse_int("9223372036854775807", false), Ok(i64::MAX));
        assert_eq!(
            parse_int("9223372036854775808", false)
                .map_err(|e| e.to_string())
                .unwrap_err(),
            "strconv.ParseInt: parsing \"9223372036854775808\": value out of range"
        );
        assert_eq!(parse_int("0x7fffffffffffffff", true), Ok(i64::MAX));
        assert_eq!(parse_int("0B101", true), Ok(5));
        assert_eq!(parse_int("0o17", true), Ok(15));
        assert_eq!(parse_int("017", true), Ok(15));
        assert_eq!(
            parse_int("0x", true)
                .map_err(|e| e.to_string())
                .unwrap_err(),
            "strconv.ParseInt: parsing \"0x\": invalid syntax"
        );
        assert_eq!(parse_int("-9223372036854775808", false), Ok(i64::MIN));
    }

    #[test]
    fn format_float_v_layout() {
        assert_eq!(format_float_v(2.5), "2.5");
        assert_eq!(format_float_v(1e9), "1e+09");
        assert_eq!(format_float_v(123456.0), "123456");
        assert_eq!(format_float_v(1234567.0), "1.234567e+06");
        assert_eq!(format_float_v(0.0001), "0.0001");
        assert_eq!(format_float_v(0.00001), "1e-05");
        assert_eq!(format_float_v(1e100), "1e+100");
        // Two shortest forms are equally close; Go takes the even one.
        assert_eq!(format_float_v(2f64.powi(-25)), "2.9802322387695312e-08");
    }
}
