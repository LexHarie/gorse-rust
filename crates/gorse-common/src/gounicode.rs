// Ports: $GOROOT/src/unicode/letter.go (ToLower, SimpleFold), $GOROOT/src/strings/strings.go (ToLower, EqualFold) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's unicode and strings packages: Copyright 2009 The Go
// Authors. All rights reserved. Use of this source code is governed by a
// BSD-style license that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's simple case mappings, which viper lower-cases configuration keys with
//! and mapstructure matches them to struct fields with.
//!
//! Go maps one code point to one code point from its own Unicode tables
//! (Unicode 17.0.0), with none of the special casing Rust's
//! [`str::to_lowercase`] applies (`İ` lower-cases to `i`, not `i̇`; `Σ` always
//! to `σ`). The tables are generated from Go (`contracts/tools/casegolden`).

mod tables;

/// Looks `c` up in a sorted table of (code point, mapping) pairs.
fn lookup(table: &[(u32, u32)], c: char) -> char {
    let c32 = u32::from(c);
    match table.binary_search_by_key(&c32, |&(k, _)| k) {
        Ok(i) => char::from_u32(table[i].1).unwrap_or(c),
        Err(_) => c,
    }
}

/// Go's `unicode.ToLower`.
pub fn to_lower(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    lookup(tables::LOWER, c)
}

/// Go's `unicode.SimpleFold`: the next code point of `c`'s case orbit (the
/// smallest one above `c`, or the smallest of all if there is none), or `c`
/// itself if it has no other case.
pub fn simple_fold(c: char) -> char {
    lookup(tables::FOLD, c)
}

/// Go's `strings.ToLower` (of valid UTF-8).
pub fn strings_to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars().map(to_lower).collect()
}

/// Go's `strings.EqualFold`: whether `s` and `t` are equal under simple
/// Unicode case folding.
pub fn equal_fold(s: &str, t: &str) -> bool {
    let mut t = t.chars();
    for sr in s.chars() {
        let Some(tr) = t.next() else {
            return false;
        };
        if sr == tr {
            continue;
        }
        let (sr, tr) = if tr < sr { (tr, sr) } else { (sr, tr) };
        // Fast check for ASCII.
        if tr.is_ascii() {
            // ASCII only, sr/tr must be upper/lower case.
            if sr.is_ascii_uppercase() && u32::from(tr) == u32::from(sr) + 32 {
                continue;
            }
            return false;
        }
        // General case. SimpleFold(x) returns the next equivalent rune > x
        // or wraps around to smaller values.
        let mut r = simple_fold(sr);
        while r != sr && r < tr {
            r = simple_fold(r);
        }
        if r == tr {
            continue;
        }
        return false;
    }
    t.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports: $GOROOT/src/strings/strings_test.go lowerTests, EqualFoldTests @ go1.27.0
    #[test]
    fn go_lower_tests() {
        for (input, want) in [
            ("", ""),
            ("abc", "abc"),
            ("AbC123", "abc123"),
            ("azAZ09_", "azaz09_"),
            (
                "longStrinGwitHmixofsmaLLandcAps",
                "longstringwithmixofsmallandcaps",
            ),
            (
                "renan bastos 93 AOSDAJDJAIDJAIDAJIaidsjjaidijadsjiadjiOOKKO",
                "renan bastos 93 aosdajdjaidjaidajiaidsjjaidijadsjiadjiookko",
            ),
            (
                "LONG\u{2C6F}STRING\u{2C6F}WITH\u{2C6F}NONASCII\u{2C6F}CHARS",
                "long\u{0250}string\u{0250}with\u{0250}nonascii\u{0250}chars",
            ),
            (
                "\u{2C6D}\u{2C6D}\u{2C6D}\u{2C6D}\u{2C6D}",
                "\u{0251}\u{0251}\u{0251}\u{0251}\u{0251}",
            ),
            ("A\u{0080}\u{10FFFF}", "a\u{0080}\u{10FFFF}"),
        ] {
            assert_eq!(strings_to_lower(input), want, "ToLower({input:?})");
        }
    }

    #[test]
    fn go_equal_fold_tests() {
        for (s, t, want) in [
            ("abc", "abc", true),
            ("ABcd", "ABcd", true),
            ("123abc", "123ABC", true),
            ("αβδ", "ΑΒΔ", true),
            ("abc", "xyz", false),
            ("abc", "XYZ", false),
            ("abcdefghijk", "abcdefghijX", false),
            ("abcdefghijk", "abcdefghij\u{212A}", true),
            ("abcdefghijK", "abcdefghij\u{212A}", true),
            ("abcdefghijkz", "abcdefghij\u{212A}y", false),
            ("abcdefghijKz", "abcdefghij\u{212A}y", false),
            ("1", "2", false),
            ("utf-8", "US-ASCII", false),
        ] {
            assert_eq!(equal_fold(s, t), want, "EqualFold({s:?}, {t:?})");
            assert_eq!(equal_fold(t, s), want, "EqualFold({t:?}, {s:?})");
        }
    }
}
