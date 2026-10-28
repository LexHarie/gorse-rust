// Ports: $GOROOT/src/unicode/utf8/utf8.go (DecodeRuneInString, DecodeLastRuneInString, RuneStart), src/unicode/graphic.go (IsSpace) with src/unicode/tables.go (White_Space), src/strings/strings.go (TrimSpace, TrimFunc, Index, LastIndex, ReplaceAll, SplitN) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go: Copyright 2009 The Go Authors. All rights reserved. Use of
// this source code is governed by a BSD-style license that can be found in
// crates/gorse-config/licenses/GO-LICENSE.

//! The parts of Go's `strings`, `unicode` and `unicode/utf8` that gotenv
//! uses, on byte strings: Go strings are bytes and may hold invalid UTF-8,
//! which Go reads one byte at a time as U+FFFD.

/// `utf8.RuneError`, U+FFFD.
pub(crate) const RUNE_ERROR: u32 = 0xFFFD;

/// `utf8.RuneSelf`: bytes below it are ASCII.
pub(crate) const RUNE_SELF: u8 = 0x80;

/// The lowest continuation byte (`locb`).
pub(crate) const LOCB: u8 = 0x80;

/// The highest continuation byte (`hicb`).
pub(crate) const HICB: u8 = 0xBF;

/// `xx` in Go's tables: an invalid first byte (size 1).
pub(crate) const XX: u8 = 0xF1;

/// `as` in Go's tables: an ASCII byte (size 1).
pub(crate) const AS: u8 = 0xF0;

const S1: u8 = 0x02;
const S2: u8 = 0x13;
const S3: u8 = 0x03;
const S4: u8 = 0x23;
const S5: u8 = 0x34;
const S6: u8 = 0x04;
const S7: u8 = 0x44;

/// Go's `first` table (`utf8internal.First` in golang.org/x/text): for each
/// byte, the size of the sequence it starts (low nibble) and the index of the
/// accept range of its second byte (high nibble), or [`AS`] / [`XX`].
#[rustfmt::skip]
pub(crate) const FIRST: [u8; 256] = [
    //   1   2   3   4   5   6   7   8   9   A   B   C   D   E   F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x00-0x0F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x10-0x1F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x20-0x2F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x30-0x3F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x40-0x4F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x50-0x5F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x60-0x6F
    AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, AS, // 0x70-0x7F
    //   1   2   3   4   5   6   7   8   9   A   B   C   D   E   F
    XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, // 0x80-0x8F
    XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, // 0x90-0x9F
    XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, // 0xA0-0xAF
    XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, // 0xB0-0xBF
    XX, XX, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, // 0xC0-0xCF
    S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, S1, // 0xD0-0xDF
    S2, S3, S3, S3, S3, S3, S3, S3, S3, S3, S3, S3, S3, S4, S3, S3, // 0xE0-0xEF
    S5, S6, S6, S6, S7, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, XX, // 0xF0-0xFF
];

/// Go's `acceptRanges`: the range of valid second bytes (`lo`, `hi`).
pub(crate) const ACCEPT_RANGES: [(u8, u8); 5] = [
    (LOCB, HICB),
    (0xA0, HICB),
    (LOCB, 0x9F),
    (0x90, HICB),
    (LOCB, 0x8F),
];

/// `utf8.DecodeRuneInString`: the first rune of `s` and its width. Invalid
/// UTF-8 decodes as (U+FFFD, 1); an empty string as (U+FFFD, 0).
pub(crate) fn decode_rune(s: &[u8]) -> (u32, usize) {
    let Some(&s0) = s.first() else {
        return (RUNE_ERROR, 0);
    };
    let x = FIRST[usize::from(s0)];
    if x >= AS {
        if x == XX {
            return (RUNE_ERROR, 1);
        }
        return (u32::from(s0), 1);
    }
    let size = usize::from(x & 7);
    let (lo, hi) = ACCEPT_RANGES[usize::from(x >> 4)];
    if s.len() < size {
        return (RUNE_ERROR, 1);
    }
    let s1 = s[1];
    if s1 < lo || hi < s1 {
        return (RUNE_ERROR, 1);
    }
    if size <= 2 {
        return ((u32::from(s0 & 0x1F) << 6) | u32::from(s1 & 0x3F), 2);
    }
    let s2 = s[2];
    if !(LOCB..=HICB).contains(&s2) {
        return (RUNE_ERROR, 1);
    }
    if size <= 3 {
        return (
            (u32::from(s0 & 0x0F) << 12) | (u32::from(s1 & 0x3F) << 6) | u32::from(s2 & 0x3F),
            3,
        );
    }
    let s3 = s[3];
    if !(LOCB..=HICB).contains(&s3) {
        return (RUNE_ERROR, 1);
    }
    (
        (u32::from(s0 & 0x07) << 18)
            | (u32::from(s1 & 0x3F) << 12)
            | (u32::from(s2 & 0x3F) << 6)
            | u32::from(s3 & 0x3F),
        4,
    )
}

/// `utf8.RuneStart`: whether `b` can start a rune (it is not a continuation
/// byte).
fn rune_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// `utf8.DecodeLastRuneInString`: the last rune of `s` and its width, with
/// the same rules as [`decode_rune`].
pub(crate) fn decode_last_rune(s: &[u8]) -> (u32, usize) {
    let end = s.len();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let last = s[end - 1];
    if last < RUNE_SELF {
        return (u32::from(last), 1);
    }
    // for start--; start >= lim; start-- { if RuneStart(s[start]) { break } }
    let lim = end.saturating_sub(4) as isize;
    let mut start = end as isize - 2;
    while start >= lim {
        if rune_start(s[start as usize]) {
            break;
        }
        start -= 1;
    }
    let start = start.max(0) as usize;
    let (r, size) = decode_rune(&s[start..end]);
    if start + size != end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// `unicode.IsSpace`: the Latin-1 spaces and the White_Space property
/// (Go's `unicode.White_Space` table).
pub(crate) fn is_space(r: u32) -> bool {
    if r <= 0xFF {
        return matches!(r, 0x09..=0x0D | 0x20 | 0x85 | 0xA0);
    }
    matches!(
        r,
        0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000
    )
}

/// `strings.TrimSpace`: `s` without leading and trailing Unicode white
/// space. Invalid UTF-8 is U+FFFD, which is not a space.
pub(crate) fn trim_space(s: &[u8]) -> &[u8] {
    trim_right_space(trim_left_space(s))
}

/// `strings.TrimLeftFunc(s, unicode.IsSpace)`.
fn trim_left_space(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() {
        let (r, size) = decode_rune(&s[i..]);
        if !is_space(r) {
            return &s[i..];
        }
        i += size;
    }
    &[]
}

/// `strings.TrimRightFunc(s, unicode.IsSpace)`: `lastIndexFunc` walks back
/// with `DecodeLastRuneInString`, then the width of the rune it stopped at is
/// taken again with `DecodeRuneInString`.
fn trim_right_space(s: &[u8]) -> &[u8] {
    let mut i = s.len();
    while i > 0 {
        let (r, size) = decode_last_rune(&s[..i]);
        i -= size;
        if !is_space(r) {
            let (_, width) = decode_rune(&s[i..]);
            return &s[..i + width];
        }
    }
    &[]
}

/// `strings.SplitN(s, sep, 2)` for a one-byte separator: the text before
/// the first `sep` and, when there is one, the text after it.
pub(crate) fn split_once_byte(s: &[u8], sep: u8) -> (&[u8], Option<&[u8]>) {
    match index_byte(s, sep) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    }
}

/// `strings.Index(s, string(b))` for a one-byte separator.
pub(crate) fn index_byte(s: &[u8], b: u8) -> Option<usize> {
    s.iter().position(|&c| c == b)
}

/// `strings.LastIndex(s, string(b))` for a one-byte separator.
pub(crate) fn last_index_byte(s: &[u8], b: u8) -> Option<usize> {
    s.iter().rposition(|&c| c == b)
}

/// `strings.ReplaceAll(s, old, new)` for a non-empty `old`: every
/// non-overlapping `old`, from the left.
pub(crate) fn replace_all(s: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// The text of a Go string as `for _, r := range s` reads it: valid UTF-8 is
/// kept, and each byte that does not start a valid sequence becomes one
/// U+FFFD (deviation X-40: a Rust `String` cannot hold the bytes).
pub(crate) fn to_go_string(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let (r, size) = decode_rune(&s[i..]);
        out.push(char::from_u32(r).unwrap_or(char::REPLACEMENT_CHARACTER));
        i += size;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rune_matches_std_on_every_short_sequence() {
        // Go's DecodeRune and Rust's validator implement the same UTF-8 rules:
        // check every one- and two-byte input, and three-byte inputs whose
        // first byte starts a multi-byte sequence.
        let check = |s: &[u8]| {
            let got = decode_rune(s);
            let want = match std::str::from_utf8(&s[..s.len().min(4)]) {
                Ok(t) => t.chars().next().map(|c| (u32::from(c), c.len_utf8())),
                Err(e) if e.valid_up_to() > 0 => std::str::from_utf8(&s[..e.valid_up_to()])
                    .ok()
                    .and_then(|t| t.chars().next())
                    .map(|c| (u32::from(c), c.len_utf8())),
                Err(_) => None,
            }
            .unwrap_or((RUNE_ERROR, usize::from(!s.is_empty())));
            assert_eq!(got, want, "{s:x?}");
        };
        check(&[]);
        for a in 0..=255u8 {
            check(&[a]);
            for b in 0..=255u8 {
                check(&[a, b]);
                if a >= 0xC0 {
                    for c in [0x00, 0x7F, 0x80, 0x9F, 0xA0, 0xBF, 0xC0, 0xFF] {
                        check(&[a, b, c]);
                        check(&[a, b, c, 0x80]);
                        check(&[a, b, c, 0xC0]);
                    }
                }
            }
        }
    }

    #[test]
    fn decode_last_rune_cases() {
        assert_eq!(decode_last_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_last_rune(b"a"), (u32::from(b'a'), 1));
        assert_eq!(decode_last_rune("x\u{2028}".as_bytes()), (0x2028, 3));
        assert_eq!(decode_last_rune(b"\xe2\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\xe2\x80\xa8\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune(b"\x80\x80\x80\x80\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_last_rune("\u{10348}".as_bytes()), (0x10348, 4));
    }

    #[test]
    fn trim_space_unicode_and_invalid() {
        assert_eq!(trim_space(b" \t\x0b\x0c\r\n a b \n"), b"a b");
        assert_eq!(trim_space("\u{a0}\u{85}x\u{3000}\u{2028}".as_bytes()), b"x");
        assert_eq!(trim_space("\u{200b}x".as_bytes()), "\u{200b}x".as_bytes());
        assert_eq!(trim_space(b" \xff "), b"\xff");
        assert_eq!(trim_space(b"\xe2\x80 "), b"\xe2\x80");
        assert_eq!(trim_space(b"   "), b"");
        assert_eq!(trim_space(b"\x1c\x1dx"), b"\x1c\x1dx");
    }

    #[test]
    fn go_string_replaces_each_invalid_byte() {
        assert_eq!(to_go_string(b"a\xe2\x82b"), "a\u{fffd}\u{fffd}b");
        assert_eq!(to_go_string(b"\xed\xa0\x80"), "\u{fffd}\u{fffd}\u{fffd}");
        assert_eq!(to_go_string("é€".as_bytes()), "é€");
    }

    #[test]
    fn replace_all_left_to_right() {
        assert_eq!(replace_all(br"a\\nb", br"\n", b"\n"), b"a\\\nb");
        assert_eq!(replace_all(br"\n\n", br"\n", b"\n"), b"\n\n");
    }
}
