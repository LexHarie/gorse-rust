// Ports: $GOROOT/src/strconv/quote.go (Quote, AppendQuote, QuoteRune, IsPrint) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! The parts of Go's `strconv` that error texts depend on: `%q` formats with
//! [`quote`], so every Go error that quotes user input needs it to read the
//! same.
//!
//! Go strings are bytes, so [`quote`] takes bytes: a byte that does not start
//! a valid UTF-8 sequence is written as `\xNN`, as Go writes it.

mod tables;

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
}
