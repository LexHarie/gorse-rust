// Ports: $GOROOT/src/unicode/utf8/utf8.go (DecodeRune, FullRune, RuneCount), $GOROOT/src/unicode/graphic.go (IsSpace) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's unicode/utf8 and unicode packages: Copyright 2009 The Go
// Authors. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! The parts of Go's `unicode/utf8` and `unicode` that byte-level ports
//! need: Go strings are bytes, and Go reads an invalid byte as U+FFFD of
//! width one.

/// Go's `utf8.RuneError`, U+FFFD.
pub const RUNE_ERROR: u32 = 0xFFFD;

/// Go's `utf8.DecodeRune`: the first rune of s and its width; `(U+FFFD, 1)`
/// for an invalid byte and `(U+FFFD, 0)` for empty input.
pub fn decode_rune(s: &[u8]) -> (u32, usize) {
    if s.is_empty() {
        return (RUNE_ERROR, 0);
    }
    // Rust and Go agree on what is valid UTF-8 (no surrogates, no overlong
    // forms, nothing past U+10FFFF); Go skips one byte of an invalid sequence.
    match s
        .utf8_chunks()
        .next()
        .and_then(|c| c.valid().chars().next())
    {
        Some(c) => (u32::from(c), c.len_utf8()),
        None => (RUNE_ERROR, 1),
    }
}

/// [`decode_rune`] as a `char`: U+FFFD for an invalid byte. Empty input is
/// `(U+FFFD, 0)`.
pub fn decode_char(s: &[u8]) -> (char, usize) {
    if s.is_empty() {
        return (char::REPLACEMENT_CHARACTER, 0);
    }
    match s
        .utf8_chunks()
        .next()
        .and_then(|c| c.valid().chars().next())
    {
        Some(c) => (c, c.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// Go's `utf8.FullRune`: whether s begins with a full UTF-8 encoding of a
/// rune. An invalid encoding counts as full, since it decodes as a width-1
/// error rune; only a valid prefix cut short by the end of s is not full.
pub fn full_rune(s: &[u8]) -> bool {
    let n = s.len();
    if n == 0 {
        return false;
    }
    // Go's `first` table: the width of a sequence by its first byte, and in
    // the high nibble the index of the range its second byte must be in.
    let x: u8 = match s[0] {
        0x00..=0x7F => 0xF0,
        0xC2..=0xDF => 0x02,
        0xE0 => 0x13,
        0xE1..=0xEC | 0xEE..=0xEF => 0x03,
        0xED => 0x23,
        0xF0 => 0x34,
        0xF1..=0xF3 => 0x04,
        0xF4 => 0x44,
        _ => 0xF1,
    };
    if n >= usize::from(x & 7) {
        return true; // ASCII, invalid or valid.
    }
    // Must be short or invalid: Go's acceptRanges.
    let (lo, hi) = [
        (0x80, 0xBF),
        (0xA0, 0xBF),
        (0x80, 0x9F),
        (0x90, 0xBF),
        (0x80, 0x8F),
    ][usize::from(x >> 4)];
    if n > 1 && (s[1] < lo || hi < s[1]) {
        return true;
    }
    if n > 2 && !(0x80..=0xBF).contains(&s[2]) {
        return true;
    }
    false
}

/// Go's `utf8.RuneCount`: each invalid byte counts as one rune.
pub fn rune_count(s: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < s.len() {
        i += decode_rune(&s[i..]).1;
        n += 1;
    }
    n
}

/// The runes of s as Go's `for range` yields them: an invalid byte is U+FFFD.
pub fn runes(s: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= s.len() {
            return None;
        }
        let (r, n) = decode_rune(&s[i..]);
        i += n;
        Some(r)
    })
}

/// Go's `unicode.IsSpace`: the Latin-1 spaces, then the `White_Space`
/// property of Go's Unicode 17.0.0 tables.
pub fn is_space(r: u32) -> bool {
    if r <= 0xFF {
        return matches!(r, 0x09..=0x0D | 0x20 | 0x85 | 0xA0);
    }
    // unicode.White_Space beyond Latin-1.
    matches!(
        r,
        0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rune_is_go() {
        assert_eq!(decode_rune(b""), (RUNE_ERROR, 0));
        assert_eq!(decode_rune(b"a"), (u32::from('a'), 1));
        assert_eq!(decode_rune("é".as_bytes()), (0xE9, 2));
        assert_eq!(decode_rune(b"\xff"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xc3"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xed\xa0\x80"), (RUNE_ERROR, 1));
        assert_eq!(decode_rune(b"\xef\xbf\xbd"), (RUNE_ERROR, 3));
        assert_eq!(rune_count(b"a\xffb\xe6\x97"), 5);
        assert_eq!(decode_char(b"\xff"), (char::REPLACEMENT_CHARACTER, 1));
        assert_eq!(decode_char("日".as_bytes()), ('日', 3));
    }

    #[test]
    fn full_rune_is_go() {
        // Truncated and invalid forms of each width; the expected values are
        // what go1.27.0's utf8.FullRune prints for them.
        for (s, want) in [
            (&b""[..], false),
            (b"a", true),
            (b"\xc3", false),
            (b"\xc3\xa9", true),
            (b"\xe6\x97", false),
            (b"\xe6\x97\xa5", true),
            (b"\xf0\x9f\x98", false),
            (b"\xf0\x9f\x98\x80", true),
            (b"\xff", true),
            (b"\xc0", true),
            (b"\xc0\x80", true),
            (b"\xe0\x80", true),
            (b"\xe0\xa0", false),
            (b"\xed\xa0", true),
            (b"\xed\x9f", false),
            (b"\xf0\x80", true),
            (b"\xf4\x90", true),
            (b"\xf4\x8f", false),
            (b"\xe6\x97\x41", true),
            (b"\xf0\x9f\x41", true),
        ] {
            assert_eq!(full_rune(s), want, "{s:?}");
        }
    }

    #[test]
    fn is_space_is_go() {
        // Every rune go1.27.0's unicode.IsSpace accepts.
        let spaces: Vec<u32> = (0..=0x10FFFF).filter(|&r| is_space(r)).collect();
        assert_eq!(
            spaces,
            [
                0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0x85, 0xA0, 0x1680, 0x2000, 0x2001, 0x2002,
                0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A, 0x2028, 0x2029,
                0x202F, 0x205F, 0x3000
            ]
        );
    }
}
