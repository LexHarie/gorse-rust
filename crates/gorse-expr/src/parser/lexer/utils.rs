// Ports: github.com/expr-lang/expr/parser/lexer/utils.go @ v1.17.8

//! Unescaping string and bytes literals.
//!
//! Values are Go runes (`i32`) as in Go: `\U` takes eight hex digits and
//! shifts them into an `int32`, so `\U80000000` and above wrap negative, pass
//! the `> MaxRune` check, and are written as their low byte (Q-WP119-1).

const RUNE_SELF: i32 = 0x80;
const MAX_RUNE: i32 = 0x10_FFFF;

const ERR_UNESCAPE: &str = "unable to unescape string";
const ERR_LAST_BACKSLASH: &str = "unable to unescape string, found '\\' as last character";
const ERR_OCTAL: &str = "unable to unescape octal sequence in string";

/// `strings.NewReplacer("\r\n", "\n", "\r", "\n")`: every line ending as
/// `\n`.
fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

/// `utf8.DecodeRune` of a string's bytes: the character at the front and its
/// length.
fn decode_rune(s: &[u8]) -> (i32, usize) {
    let width = match s.first() {
        Some(&b) if b >= 0xF0 => 4,
        Some(&b) if b >= 0xE0 => 3,
        Some(&b) if b >= 0xC0 => 2,
        _ => 1,
    };
    match s.get(..width).and_then(|w| std::str::from_utf8(w).ok()) {
        Some(w) => w.chars().next().map_or((0xFFFD, 1), |c| (c as i32, width)),
        // Not reached for the bytes of a `&str`.
        None => (0xFFFD, 1),
    }
}

/// `utf8.EncodeRune`: a rune that is not a character (a surrogate, or out of
/// range) is written as U+FFFD.
fn push_rune(buf: &mut Vec<u8>, r: i32) {
    let c = u32::try_from(r)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut tmp = [0; 4];
    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

fn unhex(b: u8) -> Option<i32> {
    match b {
        b'0'..=b'9' => Some(i32::from(b - b'0')),
        b'a'..=b'f' => Some(i32::from(b - b'a' + 10)),
        b'A'..=b'F' => Some(i32::from(b - b'A' + 10)),
        _ => None,
    }
}

type Unescaped<'a> = Result<(i32, bool, &'a [u8]), &'static str>;

/// The shared start of `unescape` and `unescapeBytes`: normalizes newlines,
/// checks the quotes, and unescapes what is between them.
fn unquote(
    value: &str,
    unescape_char: fn(&[u8]) -> Unescaped<'_>,
) -> Result<Vec<u8>, &'static str> {
    // All strings normalize newlines to the \n representation.
    let value = normalize_newlines(value);
    let bytes = value.as_bytes();
    let n = bytes.len();
    // Nothing to unescape / decode.
    if n < 2 {
        return Err(ERR_UNESCAPE);
    }
    // Quoted string of some form, must have same first and last char.
    if bytes[0] != bytes[n - 1] || (bytes[0] != b'"' && bytes[0] != b'\'') {
        return Err(ERR_UNESCAPE);
    }
    let mut rest = &bytes[1..n - 1];
    let mut buf = Vec::with_capacity(3 * n / 2);
    while !rest.is_empty() {
        let (c, multibyte, tail) = unescape_char(rest)?;
        rest = tail;
        if c < RUNE_SELF || !multibyte {
            // Go's byte(c): the low byte, also of a negative rune.
            buf.push(c as u8);
        } else {
            push_rune(&mut buf, c);
        }
    }
    Ok(buf)
}

/// `unescape`: the value of a quoted string literal (with its quotes).
pub(crate) fn unescape(value: &str) -> Result<Vec<u8>, &'static str> {
    unquote(value, unescape_char)
}

/// `unescapeBytes`: the value of a quoted bytes literal (with its quotes,
/// without the `b`).
pub(crate) fn unescape_bytes(value: &str) -> Result<Vec<u8>, &'static str> {
    unquote(value, unescape_byte_char)
}

/// `unescapeChar`: the rune at the front of `s`, whether it may need several
/// bytes, and the rest of `s`.
fn unescape_char(s: &[u8]) -> Unescaped<'_> {
    // 1. Character is not an escape sequence.
    match s[0] {
        c if i32::from(c) >= RUNE_SELF => {
            let (r, size) = decode_rune(s);
            return Ok((r, true, &s[size..]));
        }
        b'\\' => {}
        c => return Ok((i32::from(c), false, &s[1..])),
    }

    // 2. Last character is the start of an escape sequence.
    if s.len() <= 1 {
        return Err(ERR_LAST_BACKSLASH);
    }

    let c = s[1];
    let mut s = &s[2..];
    let mut multibyte = false;
    // 3. Common escape sequences shared with Google SQL
    let value = match c {
        b'a' => 0x07,
        b'b' => 0x08,
        b'f' => 0x0C,
        b'n' => i32::from(b'\n'),
        b'r' => i32::from(b'\r'),
        b't' => i32::from(b'\t'),
        b'v' => 0x0B,
        b'\\' | b'\'' | b'"' | b'`' | b'?' => i32::from(c),

        // 4. Unicode escape sequences, reproduced from `strconv/quote.go`
        b'u' if s.first() == Some(&b'{') => {
            // Support Go/Rust-style variable-length form: \u{XXXXXX}
            s = &s[1..];
            let mut v: i32 = 0;
            let mut digits = 0;
            while let Some(&b) = s.first()
                && b != b'}'
            {
                let x = unhex(b).ok_or(ERR_UNESCAPE)?;
                if digits >= 6 {
                    // at most 6 hex digits
                    return Err(ERR_UNESCAPE);
                }
                v = (v << 4) | x;
                s = &s[1..];
                digits += 1;
            }
            // require closing '}' and at least 1 digit
            if s.first() != Some(&b'}') || digits == 0 {
                return Err(ERR_UNESCAPE);
            }
            s = &s[1..];
            if v > MAX_RUNE {
                return Err(ERR_UNESCAPE);
            }
            multibyte = true;
            v
        }
        b'x' | b'X' | b'u' | b'U' => {
            let n = match c {
                b'x' | b'X' => 2,
                b'u' => 4,
                _ => 8,
            };
            if s.len() < n {
                return Err(ERR_UNESCAPE);
            }
            // Go shifts in an int32, which wraps for eight digits from 8.
            let mut v: u32 = 0;
            for &b in &s[..n] {
                let x = unhex(b).ok_or(ERR_UNESCAPE)?;
                v = (v << 4) | x.unsigned_abs();
            }
            let v = v as i32;
            s = &s[n..];
            if v > MAX_RUNE {
                return Err(ERR_UNESCAPE);
            }
            multibyte = true;
            v
        }

        // 5. Octal escape sequences, must be three digits \[0-3][0-7][0-7]
        b'0'..=b'3' => {
            if s.len() < 2 {
                return Err(ERR_OCTAL);
            }
            let mut v = i32::from(c - b'0');
            for &x in &s[..2] {
                if !(b'0'..=b'7').contains(&x) {
                    return Err(ERR_OCTAL);
                }
                v = v * 8 + i32::from(x - b'0');
            }
            s = &s[2..];
            multibyte = true;
            v
        }

        // Unknown escape sequence.
        _ => return Err(ERR_UNESCAPE),
    };
    Ok((value, multibyte, s))
}

/// `unescapeByteChar`: like [`unescape_char`], for a bytes literal, where
/// `\x` and octal escapes are bytes and `\u`/`\U` are not allowed.
fn unescape_byte_char(s: &[u8]) -> Unescaped<'_> {
    // Non-escape: return the character as-is.
    let c = s[0];
    if c != b'\\' {
        if i32::from(c) >= RUNE_SELF {
            let (r, size) = decode_rune(s);
            return Ok((r, true, &s[size..]));
        }
        return Ok((i32::from(c), false, &s[1..]));
    }

    // Escape sequence: need at least one more character.
    if s.len() <= 1 {
        return Err(ERR_LAST_BACKSLASH);
    }

    let c = s[1];
    let s = &s[2..];
    let simple = |v: i32| Ok((v, false, s));
    match c {
        // Simple escape sequences
        b'a' => simple(0x07),
        b'b' => simple(0x08),
        b'f' => simple(0x0C),
        b'n' => simple(i32::from(b'\n')),
        b'r' => simple(i32::from(b'\r')),
        b't' => simple(i32::from(b'\t')),
        b'v' => simple(0x0B),
        b'\\' | b'\'' | b'"' | b'`' | b'?' => simple(i32::from(c)),

        // Hex escape: \xNN (exactly 2 hex digits, value 0-255)
        b'x' | b'X' => {
            let (Some(hi), Some(lo)) = (
                s.first().copied().and_then(unhex),
                s.get(1).copied().and_then(unhex),
            ) else {
                return Err(ERR_UNESCAPE);
            };
            Ok(((hi << 4) | lo, false, &s[2..]))
        }

        // Octal escape: \NNN (3 octal digits, value 0-255)
        b'0'..=b'3' => {
            if s.len() < 2 {
                return Err(ERR_OCTAL);
            }
            if !(b'0'..=b'7').contains(&s[0]) || !(b'0'..=b'7').contains(&s[1]) {
                return Err(ERR_OCTAL);
            }
            let v = i32::from(c - b'0') * 64 + i32::from(s[0] - b'0') * 8 + i32::from(s[1] - b'0');
            Ok((v, false, &s[2..]))
        }

        _ => Err(ERR_UNESCAPE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_wraps_long_unicode_escapes_like_go_int32() {
        assert_eq!(unescape(r#""\UFFFFFFFF""#), Ok(vec![0xFF]));
        assert_eq!(unescape(r#""\U80000041""#), Ok(b"A".to_vec()));
        assert_eq!(unescape(r#""\U7FFFFFFF""#), Err(ERR_UNESCAPE));
        assert_eq!(unescape(r#""\U00110000""#), Err(ERR_UNESCAPE));
        assert_eq!(
            unescape(r#""\U0010FFFF""#),
            Ok("\u{10FFFF}".as_bytes().to_vec())
        );
    }

    #[test]
    fn unescape_writes_surrogates_as_replacement() {
        assert_eq!(unescape(r#""\uD800""#), Ok("\u{FFFD}".as_bytes().to_vec()));
        assert_eq!(
            unescape(r#""\u{DFFF}""#),
            Ok("\u{FFFD}".as_bytes().to_vec())
        );
    }

    #[test]
    fn unescape_bytes_keeps_bytes() {
        assert_eq!(
            unescape_bytes(r#""\xff\377é""#),
            Ok(b"\xff\xff\xc3\xa9".to_vec())
        );
        assert_eq!(
            unescape(r#""\xff\377""#),
            Ok("\u{ff}\u{ff}".as_bytes().to_vec())
        );
        assert_eq!(unescape_bytes(r#""\u0041""#), Err(ERR_UNESCAPE));
    }

    #[test]
    fn unescape_normalizes_newlines() {
        assert_eq!(unescape("'a\r\nb\rc'"), Ok(b"a\nb\nc".to_vec()));
    }

    #[test]
    fn unescape_errors() {
        assert_eq!(unescape("'"), Err(ERR_UNESCAPE));
        assert_eq!(unescape("'a\""), Err(ERR_UNESCAPE));
        assert_eq!(unescape("'\\'"), Err(ERR_LAST_BACKSLASH));
        assert_eq!(unescape("'\\0'"), Err(ERR_OCTAL));
        assert_eq!(unescape("'\\08'"), Err(ERR_OCTAL));
        assert_eq!(unescape("'\\4'"), Err(ERR_UNESCAPE));
        assert_eq!(unescape("'\\u{}'"), Err(ERR_UNESCAPE));
        assert_eq!(unescape("'\\u{1234567}'"), Err(ERR_UNESCAPE));
        assert_eq!(unescape("'\\u{110000}'"), Err(ERR_UNESCAPE));
    }
}
