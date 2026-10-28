// Ports: the functions of Go's strings package that builtin/builtin.go calls (Trim, TrimSpace, TrimPrefix, TrimSuffix, ToUpper, Split, SplitAfter, Replace, Index, LastIndex) @ go1.27.0

//! The string functions of the builtins, on bytes as Go's are: a string may
//! hold bytes that are not UTF-8, and each such byte is one rune of width 1.

use gorse_common::{gounicode, goutf8};

/// The runes of `s` that `keep` decides to keep, as a string of their own.
/// Leading and trailing runes that `trim` accepts are cut.
fn trim_runes(s: &[u8], trim: impl Fn(u32) -> bool) -> Vec<u8> {
    let mut start = 0;
    while start < s.len() {
        let (r, size) = goutf8::decode_rune(&s[start..]);
        if !trim(r) {
            break;
        }
        start += size;
    }
    let mut keep_end = start;
    let mut i = start;
    while i < s.len() {
        let (r, size) = goutf8::decode_rune(&s[i..]);
        i += size;
        if !trim(r) {
            keep_end = i;
        }
    }
    s[start..keep_end].to_vec()
}

/// `strings.TrimSpace`.
pub fn trim_space(s: &[u8]) -> Vec<u8> {
    trim_runes(s, goutf8::is_space)
}

/// `strings.Trim`: the runes of `s` at both ends that `cutset` holds.
pub fn trim(s: &[u8], cutset: &[u8]) -> Vec<u8> {
    let mut set = Vec::new();
    let mut i = 0;
    while i < cutset.len() {
        let (r, size) = goutf8::decode_rune(&cutset[i..]);
        set.push(r);
        i += size;
    }
    trim_runes(s, |r| set.contains(&r))
}

/// `strings.TrimPrefix`.
pub fn trim_prefix(s: &[u8], prefix: &[u8]) -> Vec<u8> {
    s.strip_prefix(prefix).unwrap_or(s).to_vec()
}

/// `strings.TrimSuffix`.
pub fn trim_suffix(s: &[u8], suffix: &[u8]) -> Vec<u8> {
    s.strip_suffix(suffix).unwrap_or(s).to_vec()
}

/// Applies a rune mapping to every rune of `s`. A byte that is not UTF-8 maps
/// to U+FFFD, as `strings.Map` writes it.
fn map_runes(s: &[u8], mapping: impl Fn(char) -> char) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let (r, size) = goutf8::decode_rune(&s[i..]);
        i += size;
        let c = match char::from_u32(r) {
            Some(c) if !(r == 0xFFFD && size == 1) => mapping(c),
            _ => '\u{FFFD}',
        };
        let mut buf = [0; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// `unicode.ToUpper`: the simple upper-case mapping, which keeps a character
/// whose upper case is more than one character.
fn simple_upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

/// `strings.ToUpper`.
pub fn to_upper(s: &[u8]) -> Vec<u8> {
    map_runes(s, simple_upper)
}

/// `strings.ToLower`.
pub fn to_lower(s: &[u8]) -> Vec<u8> {
    map_runes(s, gounicode::to_lower)
}

/// The index of the first occurrence of `sub` in `s`.
fn find(s: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.is_empty() {
        return Some(0);
    }
    s.windows(sub.len()).position(|w| w == sub)
}

/// The number of non-overlapping occurrences of `sub` in `s`; an empty `sub`
/// occurs before each rune and at the end.
fn count(s: &[u8], sub: &[u8]) -> usize {
    if sub.is_empty() {
        return goutf8::rune_count(s) + 1;
    }
    let mut n = 0;
    let mut rest = s;
    while let Some(i) = find(rest, sub) {
        n += 1;
        rest = &rest[i + sub.len()..];
    }
    n
}

/// `strings.Index`: the byte index of the first occurrence, or -1.
pub fn index(s: &[u8], sub: &[u8]) -> i64 {
    find(s, sub).map_or(-1, |i| i as i64)
}

/// `strings.LastIndex`: the byte index of the last occurrence, or -1.
pub fn last_index(s: &[u8], sub: &[u8]) -> i64 {
    if sub.is_empty() {
        return s.len() as i64;
    }
    s.windows(sub.len())
        .rposition(|w| w == sub)
        .map_or(-1, |i| i as i64)
}

/// `strings.Split` with `sep_save` bytes of the separator kept on each piece
/// (`strings.SplitAfter` keeps all of it), at most `n` pieces (all when `n`
/// is negative).
pub fn split(s: &[u8], sep: &[u8], sep_save: usize, n: i64) -> Vec<Vec<u8>> {
    if n == 0 {
        return Vec::new();
    }
    if sep.is_empty() {
        return explode(s, n);
    }
    let n = if n < 0 {
        count(s, sep) + 1
    } else {
        (n as usize).min(s.len() + 1)
    };
    let mut pieces = Vec::with_capacity(n);
    let mut rest = s;
    while pieces.len() + 1 < n {
        let Some(m) = find(rest, sep) else {
            break;
        };
        pieces.push(rest[..m + sep_save].to_vec());
        rest = &rest[m + sep.len()..];
    }
    pieces.push(rest.to_vec());
    pieces
}

/// `strings.Split` with an empty separator: one piece per rune, the last one
/// holding the rest when there are at most `n`.
fn explode(s: &[u8], n: i64) -> Vec<Vec<u8>> {
    let length = goutf8::rune_count(s) as i64;
    let n = if n < 0 || n > length { length } else { n } as usize;
    let mut pieces = Vec::with_capacity(n);
    let mut rest = s;
    for _ in 0..n.saturating_sub(1) {
        let (_, size) = goutf8::decode_rune(rest);
        pieces.push(rest[..size].to_vec());
        rest = &rest[size..];
    }
    if n > 0 {
        pieces.push(rest.to_vec());
    }
    pieces
}

/// `strings.Replace`: `old` replaced by `new`, the first `n` times (all of
/// them when `n` is negative).
pub fn replace(s: &[u8], old: &[u8], new: &[u8], n: i64) -> Vec<u8> {
    if old == new || n == 0 {
        return s.to_vec();
    }
    let m = count(s, old);
    if m == 0 {
        return s.to_vec();
    }
    let n = if n < 0 || (m as i64) < n {
        m
    } else {
        n as usize
    };
    let mut out = Vec::with_capacity(s.len());
    let mut start = 0;
    for i in 0..n {
        let mut j = start;
        if old.is_empty() {
            if i > 0 {
                let (_, size) = goutf8::decode_rune(&s[start..]);
                j += size;
            }
        } else {
            j += find(&s[start..], old).expect("counted above");
        }
        out.extend_from_slice(&s[start..j]);
        out.extend_from_slice(new);
        start = j + old.len();
    }
    out.extend_from_slice(&s[start..]);
    out
}
