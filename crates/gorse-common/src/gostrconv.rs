// Ports: $GOROOT/src/strconv/quote.go (Quote, AppendQuote, QuoteRune, IsPrint), number.go (ParseUint, ParseInt, Atoi, ParseFloat, ParseBool, FormatBool, AppendBool, NumError, FormatFloat, FormatInt, FormatUint), $GOROOT/src/internal/strconv/atoi.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! The parts of Go's `strconv` that error texts depend on: `%q` formats with
//! [`quote`], so every Go error that quotes user input needs it to read the
//! same; the integer parsers, [`parse_float`] and [`parse_bool`] with their
//! [`NumError`] texts; and [`format_float`], [`format_int`] and [`format_uint`],
//! which Go's `fmt`, `json` and `mapstructure` print numbers with.
//!
//! Go strings are bytes, so [`quote`] and the parsers take bytes: a byte
//! that does not start a valid UTF-8 sequence is written as `\xNN`, as Go
//! writes it.

mod atob;
mod atof;
#[doc(hidden)]
pub mod decimal;
mod ftoa;
mod pow10tab;
mod tables;
#[doc(hidden)]
pub mod uscale;

pub use ftoa::{format_float, format_int, format_uint};

use thiserror::Error;

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

/// Go's `strconv.IsPrint`: letters, marks, numbers, punctuation, symbols and
/// the ASCII space, as Go's Unicode tables define them.
pub fn is_print(r: char) -> bool {
    let r = u32::from(r);
    let i = tables::PRINT.partition_point(|&(_, hi)| hi < r);
    tables::PRINT.get(i).is_some_and(|&(lo, _)| lo <= r)
}

/// Go's `strconv.Quote`: a double-quoted Go string literal for s.
pub fn quote(s: impl AsRef<[u8]>) -> String {
    let mut buf = Vec::new();
    append_quote(&mut buf, s);
    String::from_utf8(buf).expect("quote writes UTF-8")
}

/// Go's `strconv.AppendQuote`.
pub fn append_quote(buf: &mut Vec<u8>, s: impl AsRef<[u8]>) {
    let s = s.as_ref();
    buf.reserve(s.len() + 2);
    buf.push(b'"');
    for chunk in s.utf8_chunks() {
        for r in chunk.valid().chars() {
            append_escaped_rune(buf, r, b'"');
        }
        // Go decodes an invalid sequence one byte at a time.
        for &b in chunk.invalid() {
            buf.extend_from_slice(b"\\x");
            buf.push(LOWER_HEX[usize::from(b >> 4)]);
            buf.push(LOWER_HEX[usize::from(b & 0xF)]);
        }
    }
    buf.push(b'"');
}

/// Go's `strconv.QuoteRune` for a valid rune: a single-quoted Go character
/// literal. (Go maps an invalid rune to U+FFFD first, which a `char` cannot
/// hold.)
pub fn quote_rune(r: char) -> String {
    let mut buf = vec![b'\''];
    append_escaped_rune(&mut buf, r, b'\'');
    buf.push(b'\'');
    String::from_utf8(buf).expect("quote writes UTF-8")
}

/// Go's `appendEscapedRune` with `ASCIIonly` and `graphicOnly` false.
fn append_escaped_rune(buf: &mut Vec<u8>, r: char, quote: u8) {
    if r == char::from(quote) || r == '\\' {
        // always backslashed
        buf.push(b'\\');
        buf.push(r as u8);
        return;
    }
    if is_print(r) {
        let mut tmp = [0; 4];
        buf.extend_from_slice(r.encode_utf8(&mut tmp).as_bytes());
        return;
    }
    match r {
        '\x07' => buf.extend_from_slice(b"\\a"),
        '\x08' => buf.extend_from_slice(b"\\b"),
        '\x0c' => buf.extend_from_slice(b"\\f"),
        '\n' => buf.extend_from_slice(b"\\n"),
        '\r' => buf.extend_from_slice(b"\\r"),
        '\t' => buf.extend_from_slice(b"\\t"),
        '\x0b' => buf.extend_from_slice(b"\\v"),
        _ => {
            let r = u32::from(r);
            let (prefix, shift): (&[u8], u32) = if r < u32::from(b' ') || r == 0x7f {
                (b"\\x", 4)
            } else if r < 0x10000 {
                // (A char is always a valid rune, so Go's U+FFFD case cannot
                // arise.)
                (b"\\u", 12)
            } else {
                (b"\\U", 28)
            };
            buf.extend_from_slice(prefix);
            for s in (0..=shift).rev().step_by(4) {
                buf.push(LOWER_HEX[(r >> s & 0xF) as usize]);
            }
        }
    }
}

/// The reason inside a [`NumError`]: Go's `strconv.ErrSyntax`,
/// `strconv.ErrRange`, and the "invalid base" and "invalid bit size" errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NumErrorKind {
    /// `strconv.ErrSyntax`.
    #[error("invalid syntax")]
    Syntax,
    /// `strconv.ErrRange`.
    #[error("value out of range")]
    Range,
    /// A base outside 0 and 2 to 36.
    #[error("invalid base {0}")]
    Base(i64),
    /// A bit size outside 0 to 64.
    #[error("invalid bit size {0}")]
    BitSize(i64),
}

/// Go's `*strconv.NumError`: `strconv.<Func>: parsing <quoted input>: <reason>`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("strconv.{func}: parsing {}: {err}", quote(num))]
pub struct NumError {
    /// The failing function (`ParseInt`, `ParseUint`, `Atoi`, `ParseFloat`,
    /// `ParseBool`).
    pub func: &'static str,
    /// The input.
    pub num: Vec<u8>,
    /// Why it failed.
    pub err: NumErrorKind,
}

/// Go's `strconv.IntSize` on the 64-bit platforms Gorse ships for.
pub const INT_SIZE: i64 = 64;

/// `lower(c)` is a lower-case letter if and only if c is either that
/// lower-case letter or the equivalent upper-case letter.
const fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Go's `strconv.ParseUint`. Like Go it returns the value together with the
/// error: on a range error that is the largest value of the bit size.
pub fn parse_uint(s: impl AsRef<[u8]>, base: i64, bit_size: i64) -> (u64, Option<NumError>) {
    let s = s.as_ref();
    match parse_uint_internal(s, base, bit_size) {
        Ok(n) => (n, None),
        Err((n, err)) => (n, Some(num_error("ParseUint", s, err))),
    }
}

/// Go's `strconv.ParseInt`. Like Go it returns the value together with the
/// error: on a range error that is the extreme value of the bit size with
/// the input's sign.
pub fn parse_int(s: impl AsRef<[u8]>, base: i64, bit_size: i64) -> (i64, Option<NumError>) {
    let s = s.as_ref();
    match parse_int_internal(s, base, bit_size) {
        Ok(n) => (n, None),
        Err((n, err)) => (n, Some(num_error("ParseInt", s, err))),
    }
}

/// Go's `strconv.Atoi`: `ParseInt(s, 10, 0)` with `Atoi` in the error.
pub fn atoi(s: impl AsRef<[u8]>) -> (i64, Option<NumError>) {
    let s = s.as_ref();
    match atoi_internal(s) {
        Ok(n) => (n, None),
        Err((n, err)) => (n, Some(num_error("Atoi", s, err))),
    }
}

/// Go's `strconv.ParseFloat`: s as the nearest float of the bit size (32
/// rounds to the nearest `float32` and widens it; any other bit size means
/// 64). Decimal and hexadecimal Go float literals with underscores, and
/// "Inf", "Infinity" and "NaN" in any case, are accepted. Like Go it returns
/// the value together with the error: ±Inf on a range error, 0 on a syntax
/// error.
pub fn parse_float(s: impl AsRef<[u8]>, bit_size: i64) -> (f64, Option<NumError>) {
    parse_float_with_optimize(s, bit_size, true)
}

/// [`parse_float`] with Go's `optimize` switch as a parameter: false forces
/// the slow multiprecision path, as Go's tests do with `SetOptimize(false)`.
#[doc(hidden)]
pub fn parse_float_with_optimize(
    s: impl AsRef<[u8]>,
    bit_size: i64,
    optimize: bool,
) -> (f64, Option<NumError>) {
    let s = s.as_ref();
    let (f, err) = atof::parse_float(s, bit_size, optimize);
    (f, err.map(|err| num_error("ParseFloat", s, err)))
}

/// `internal/strconv.parseFloatPrefix` (Go's tests call it
/// `ParseFloatPrefix`): the float at the start of s, the number of bytes it
/// takes, and the bare error.
#[doc(hidden)]
pub fn parse_float_prefix(
    s: impl AsRef<[u8]>,
    bit_size: i64,
) -> (f64, usize, Option<NumErrorKind>) {
    atof::parse_float_prefix(s.as_ref(), bit_size, true)
}

/// Go's `strconv.ParseBool`: true for "1", "t", "T", "true", "TRUE" and
/// "True", false for "0", "f", "F", "false", "FALSE" and "False", and a
/// syntax error (with false) for anything else.
pub fn parse_bool(s: impl AsRef<[u8]>) -> (bool, Option<NumError>) {
    let s = s.as_ref();
    let (b, err) = atob::parse_bool(s);
    (b, err.map(|err| num_error("ParseBool", s, err)))
}

/// Go's `strconv.FormatBool`: "true" or "false".
pub const fn format_bool(b: bool) -> &'static str {
    atob::format_bool(b)
}

/// Go's `strconv.AppendBool`: appends "true" or "false" to dst.
pub fn append_bool(dst: &mut Vec<u8>, b: bool) {
    atob::append_bool(dst, b);
}

fn num_error(func: &'static str, s: &[u8], err: NumErrorKind) -> NumError {
    NumError {
        func,
        num: s.to_vec(),
        err,
    }
}

/// `internal/strconv.ParseUint`: the value Go returns with each error.
fn parse_uint_internal(s: &[u8], base: i64, bit_size: i64) -> Result<u64, (u64, NumErrorKind)> {
    if s.is_empty() {
        return Err((0, NumErrorKind::Syntax));
    }

    let base0 = base == 0;

    let s0 = s;
    let mut s = s;
    let base: u64 = match base {
        // valid base; nothing to do
        2..=36 => base as u64,
        0 => {
            // Look for octal, hex prefix.
            if s[0] == b'0' {
                if s.len() >= 3 && lower(s[1]) == b'b' {
                    s = &s[2..];
                    2
                } else if s.len() >= 3 && lower(s[1]) == b'o' {
                    s = &s[2..];
                    8
                } else if s.len() >= 3 && lower(s[1]) == b'x' {
                    s = &s[2..];
                    16
                } else {
                    s = &s[1..];
                    8
                }
            } else {
                10
            }
        }
        _ => return Err((0, NumErrorKind::Base(base))),
    };

    let bit_size = if bit_size == 0 {
        INT_SIZE
    } else if !(0..=64).contains(&bit_size) {
        return Err((0, NumErrorKind::BitSize(bit_size)));
    } else {
        bit_size
    };

    // Cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / base + 1;

    // Go's uint64(1)<<64 is 0, so a bit size of 64 gives the all-ones mask.
    let max_val = 1u64
        .checked_shl(bit_size as u32)
        .unwrap_or(0)
        .wrapping_sub(1);

    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in s {
        let d = if c == b'_' && base0 {
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            c - b'0'
        } else if lower(c).is_ascii_lowercase() {
            lower(c) - b'a' + 10
        } else {
            return Err((0, NumErrorKind::Syntax));
        };

        if u64::from(d) >= base {
            return Err((0, NumErrorKind::Syntax));
        }

        if n >= cutoff {
            // n*base overflows
            return Err((max_val, NumErrorKind::Range));
        }
        n *= base;

        let n1 = n.wrapping_add(u64::from(d));
        if n1 < n || n1 > max_val {
            // n+d overflows
            return Err((max_val, NumErrorKind::Range));
        }
        n = n1;
    }

    if underscores && !underscore_ok(s0) {
        return Err((0, NumErrorKind::Syntax));
    }

    Ok(n)
}

/// `internal/strconv.ParseInt`.
fn parse_int_internal(s: &[u8], base: i64, bit_size: i64) -> Result<i64, (i64, NumErrorKind)> {
    if s.is_empty() {
        return Err((0, NumErrorKind::Syntax));
    }

    // Pick off leading sign.
    let mut s = s;
    let mut neg = false;
    match s[0] {
        b'+' => s = &s[1..],
        b'-' => {
            s = &s[1..];
            neg = true;
        }
        _ => {}
    }

    // Convert unsigned and check range.
    let un = match parse_uint_internal(s, base, bit_size) {
        Ok(un) | Err((un, NumErrorKind::Range)) => un,
        Err((_, err)) => return Err((0, err)),
    };

    let bit_size = if bit_size == 0 { INT_SIZE } else { bit_size };

    let cutoff = 1u64 << (bit_size - 1);
    if !neg && un >= cutoff {
        return Err(((cutoff - 1) as i64, NumErrorKind::Range));
    }
    if neg && un > cutoff {
        return Err(((cutoff as i64).wrapping_neg(), NumErrorKind::Range));
    }
    // (Like Go, a range error from ParseUint that passes both checks is
    // dropped; only a bit size of 1 can get here, e.g. "-2".)
    let n = un as i64;
    Ok(if neg { n.wrapping_neg() } else { n })
}

/// `internal/strconv.Atoi`.
fn atoi_internal(s: &[u8]) -> Result<i64, (i64, NumErrorKind)> {
    let s_len = s.len();
    if 0 < s_len && s_len < 19 {
        // Fast path for small integers that fit int type.
        let s0 = s;
        let mut s = s;
        if s[0] == b'-' || s[0] == b'+' {
            s = &s[1..];
            if s.is_empty() {
                return Err((0, NumErrorKind::Syntax));
            }
        }

        let mut n: i64 = 0;
        for &ch in s {
            let ch = ch.wrapping_sub(b'0');
            if ch > 9 {
                return Err((0, NumErrorKind::Syntax));
            }
            n = n * 10 + i64::from(ch);
        }
        if s0[0] == b'-' {
            n = -n;
        }
        return Ok(n);
    }

    // Slow path for invalid, big, or underscored integers.
    parse_int_internal(s, 10, 0)
}

/// Reports whether the underscores in s are allowed. Underscore must appear
/// only between digits or between a base prefix and a digit.
fn underscore_ok(s: &[u8]) -> bool {
    // saw tracks the last character (class) we saw:
    // ^ for beginning of number,
    // 0 for a digit or base prefix,
    // _ for an underscore,
    // ! for none of the above.
    let mut saw = b'^';
    let mut i = 0;

    // Optional sign.
    let mut s = s;
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }

    // Optional base prefix.
    let mut hex = false;
    if s.len() >= 2
        && s[0] == b'0'
        && (lower(s[1]) == b'b' || lower(s[1]) == b'o' || lower(s[1]) == b'x')
    {
        i = 2;
        saw = b'0'; // base prefix counts as a digit for "underscore as digit separator"
        hex = lower(s[1]) == b'x';
    }

    // Number proper.
    while i < s.len() {
        // Digits are always okay.
        if s[i].is_ascii_digit() || hex && (b'a'..=b'f').contains(&lower(s[i])) {
            saw = b'0';
            i += 1;
            continue;
        }
        // Underscore must follow digit.
        if s[i] == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            i += 1;
            continue;
        }
        // Underscore must also be followed by digit.
        if saw == b'_' {
            return false;
        }
        // Saw non-digit, non-underscore.
        saw = b'!';
        i += 1;
    }
    saw != b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports: strconv/quote_test.go quotetests (the Quote column) and TestQuote.
    #[test]
    fn test_quote() {
        let tests: &[(&[u8], &str)] = &[
            (b"\x07\x08\x0c\r\n\t\x0b", r#""\a\b\f\r\n\t\v""#),
            (b"\\", r#""\\""#),
            (b"abc\xffdef", r#""abc\xffdef""#),
            ("\u{263a}".as_bytes(), r#""☺""#),
            ("\u{10ffff}".as_bytes(), r#""\U0010ffff""#),
            (b"\x04", r#""\x04""#),
            // Some non-printable but graphic runes. Final column is
            // double-quoted.
            (
                "!\u{00a0}!\u{2000}!\u{3000}!".as_bytes(),
                "\"!\\u00a0!\\u2000!\\u3000!\"",
            ),
            (b"\x7f", r#""\x7f""#),
        ];
        for &(input, out) in tests {
            assert_eq!(quote(input), out, "Quote({input:?})");
            let mut buf = b"abc".to_vec();
            append_quote(&mut buf, input);
            assert_eq!(
                String::from_utf8(buf).expect("UTF-8"),
                format!("abc{out}"),
                "AppendQuote(\"abc\", {input:?})"
            );
        }
    }

    // Ports: strconv/quote_test.go quoterunetests (the QuoteRune column, for
    // the runes a char can hold) and TestQuoteRune.
    #[test]
    fn test_quote_rune() {
        let tests: &[(char, &str)] = &[
            ('a', "'a'"),
            ('\x07', r"'\a'"),
            ('\\', r"'\\'"),
            ('\u{FF}', "'ÿ'"),
            ('\u{263a}', "'☺'"),
            ('\u{fffd}', "'�'"),
            ('\u{10ffff}', r"'\U0010ffff'"),
            ('\x04', r"'\x04'"),
            // Some differences between graphic and printable. Note the last
            // column is double-quoted.
            ('\u{00a0}', "'\\u00a0'"),
            ('\u{2000}', "'\\u2000'"),
            ('\u{3000}', "'\\u3000'"),
        ];
        for &(input, out) in tests {
            assert_eq!(quote_rune(input), out, "QuoteRune({input:?})");
        }
    }

    // WP-1-4's unit test of parse_float, moved here unchanged from the
    // gostrconv/atof.rs it replaced.
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
