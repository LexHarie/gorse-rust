// Ports: internal/characters/ascii.go, internal/characters/utf8.go @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! go-toml's UTF-8 validation: Go's `utf8.Valid` tables, tweaked to reject the
//! ASCII control characters TOML does not allow unescaped.

/// `invalidAsciiTable`: the control characters other than tab, line feed and
/// carriage return, and DEL.
fn invalid_ascii(b: u8) -> bool {
    matches!(b, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F)
}

/// `utf8.RuneSelf`.
const RUNE_SELF: u8 = 0x80;

/// `utf8Err`: where the first invalid character is, and how many bytes it
/// spans. A zero size means no error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Utf8Err {
    /// `Index`.
    pub(crate) index: usize,
    /// `Size`.
    pub(crate) size: usize,
}

impl Utf8Err {
    /// `Zero`.
    pub(crate) fn zero(self) -> bool {
        self.size == 0
    }
}

// The default lowest and highest continuation byte.
const LOCB: u8 = 0b1000_0000;
const HICB: u8 = 0b1011_1111;

// The first nibble is an index into ACCEPT_RANGES or F for special one-byte
// cases. The second nibble is the rune length or the status for the special
// one-byte case.
const XX: u8 = 0xF1; // invalid: size 1
const AS: u8 = 0xF0; // ASCII: size 1
const S1: u8 = 0x02; // accept 0, size 2
const S2: u8 = 0x13; // accept 1, size 3
const S3: u8 = 0x03; // accept 0, size 3
const S4: u8 = 0x23; // accept 2, size 3
const S5: u8 = 0x34; // accept 3, size 4
const S6: u8 = 0x04; // accept 0, size 4
const S7: u8 = 0x44; // accept 4, size 4

/// `first`: information about the first byte in a UTF-8 sequence.
fn first(b: u8) -> u8 {
    match b {
        0x00..=0x7F => AS,
        0x80..=0xC1 => XX,
        0xC2..=0xDF => S1,
        0xE0 => S2,
        0xE1..=0xEC | 0xEE..=0xEF => S3,
        0xED => S4,
        0xF0 => S5,
        0xF1..=0xF3 => S6,
        0xF4 => S7,
        0xF5..=0xFF => XX,
    }
}

/// `acceptRanges`: the range of valid values for the second byte.
fn accept_range(i: u8) -> (u8, u8) {
    match i {
        0 => (LOCB, HICB),
        1 => (0xA0, HICB),
        2 => (LOCB, 0x9F),
        3 => (0x90, HICB),
        4 => (LOCB, 0x8F),
        // Go's table has 16 entries, the rest zero; no first byte uses them.
        _ => (0, 0),
    }
}

/// `Utf8TomlValidAlreadyEscaped`: checks that p is UTF-8 made only of the
/// characters a TOML string may hold unescaped (quotation marks and
/// backslashes were already checked), and returns where the first offending
/// character is.
pub(crate) fn utf8_toml_valid_already_escaped(p: &[u8]) -> Utf8Err {
    // Go has an 8-bytes-at-a-time fast path for ASCII that finds the same
    // first invalid byte as this loop.
    let n = p.len();
    let mut i = 0;
    while i < n {
        let pi = p[i];
        if pi < RUNE_SELF {
            if invalid_ascii(pi) {
                return Utf8Err { index: i, size: 1 };
            }
            i += 1;
            continue;
        }
        let x = first(pi);
        if x == XX {
            // Illegal starter byte.
            return Utf8Err { index: i, size: 1 };
        }
        let size = usize::from(x & 7);
        if i + size > n {
            // Short or invalid.
            return Utf8Err {
                index: i,
                size: n - i,
            };
        }
        let (lo, hi) = accept_range(x >> 4);
        let c = p[i + 1];
        if c < lo || hi < c {
            return Utf8Err { index: i, size: 2 };
        } else if size == 2 {
        } else if !(LOCB..=HICB).contains(&p[i + 2]) {
            return Utf8Err { index: i, size: 3 };
        } else if size == 3 {
        } else if !(LOCB..=HICB).contains(&p[i + 3]) {
            return Utf8Err { index: i, size: 4 };
        }
        i += size;
    }
    Utf8Err::default()
}

/// `Utf8ValidNext`: the size of the next rune if it is valid UTF-8 and not
/// a forbidden control character, 0 otherwise. p must not be empty.
pub(crate) fn utf8_valid_next(p: &[u8]) -> usize {
    let c = p[0];

    if c < RUNE_SELF {
        if invalid_ascii(c) {
            return 0;
        }
        return 1;
    }

    let x = first(c);
    if x == XX {
        // Illegal starter byte.
        return 0;
    }
    let size = usize::from(x & 7);
    if size > p.len() {
        // Short or invalid.
        return 0;
    }
    let (lo, hi) = accept_range(x >> 4);
    let c = p[1];
    if c < lo || hi < c {
        return 0;
    } else if size == 2 {
    } else if !(LOCB..=HICB).contains(&p[2]) {
        return 0;
    } else if size == 3 {
    } else if !(LOCB..=HICB).contains(&p[3]) {
        return 0;
    }

    size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matches_go_table() {
        // Spot checks of Go's `first` table rows.
        assert_eq!(first(0x41), AS);
        assert_eq!(first(0x80), XX);
        assert_eq!(first(0xC1), XX);
        assert_eq!(first(0xC2), S1);
        assert_eq!(first(0xE0), S2);
        assert_eq!(first(0xED), S4);
        assert_eq!(first(0xEE), S3);
        assert_eq!(first(0xF0), S5);
        assert_eq!(first(0xF3), S6);
        assert_eq!(first(0xF4), S7);
        assert_eq!(first(0xF5), XX);
    }

    #[test]
    fn valid_next_agrees_with_rust_on_utf8() {
        // For every 1- to 3-byte prefix, a nonzero size is exactly a valid
        // UTF-8 character that is not a forbidden control character.
        for a in 0..=255u8 {
            for b in [0x00, 0x41, 0x80, 0x8F, 0x90, 0x9F, 0xA0, 0xBF, 0xC0] {
                for c in [0x80, 0xBF, 0x41] {
                    for d in [0x80, 0xBF, 0x41] {
                        let p = [a, b, c, d];
                        let size = utf8_valid_next(&p);
                        let rust = p
                            .utf8_chunks()
                            .next()
                            .and_then(|ch| ch.valid().chars().next().map(char::len_utf8));
                        let expect = match rust {
                            Some(1) if invalid_ascii(a) => 0,
                            Some(n) => n,
                            None => 0,
                        };
                        assert_eq!(size, expect, "{p:02x?}");
                    }
                }
            }
        }
    }
}
