// Ports: resolve.go @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3: Copyright 2011-2016 Canonical Ltd,
// licensed under the Apache License, Version 2.0; see
// crates/gorse-config/licenses/YAML-V3-LICENSE.

//! yaml.v3's tag resolution: which Go value a plain scalar stands for
//! (`resolve`), and the short and long forms of tags.

use gorse_common::gostrconv::{parse_float, parse_int, parse_uint};
use gorse_common::gotime::{self, Time};

pub(crate) const NULL_TAG: &str = "!!null";
pub(crate) const BOOL_TAG: &str = "!!bool";
pub(crate) const STR_TAG: &str = "!!str";
pub(crate) const INT_TAG: &str = "!!int";
pub(crate) const FLOAT_TAG: &str = "!!float";
pub(crate) const TIMESTAMP_TAG: &str = "!!timestamp";
pub(crate) const SEQ_TAG: &str = "!!seq";
pub(crate) const MAP_TAG: &str = "!!map";
pub(crate) const BINARY_TAG: &str = "!!binary";
pub(crate) const MERGE_TAG: &str = "!!merge";

const LONG_TAG_PREFIX: &str = "tag:yaml.org,2002:";

/// A resolved scalar: the Go value `resolve` returns.
#[derive(Clone, Debug)]
pub(crate) enum Resolved {
    Null,
    Bool(bool),
    /// `int`.
    Int(i64),
    Uint64(u64),
    Float64(f64),
    String(String),
    Time(Time),
}

/// `shortTag`.
pub(crate) fn short_tag(tag: &str) -> String {
    match tag.strip_prefix(LONG_TAG_PREFIX) {
        // The table of known long tags maps each to "!!" + the rest too.
        Some(rest) => format!("!!{rest}"),
        None => tag.to_owned(),
    }
}

/// `resolvableTag`.
fn resolvable_tag(tag: &str) -> bool {
    matches!(tag, "" | STR_TAG | BOOL_TAG | INT_TAG | FLOAT_TAG | NULL_TAG | TIMESTAMP_TAG)
}

/// `resolveTable`: the hint for the first byte of a scalar.
fn hint(b: u8) -> u8 {
    match b {
        b'+' | b'-' => b'S',
        b'0'..=b'9' => b'D',
        b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~' => b'M',
        b'.' => b'.',
        _ => 0,
    }
}

/// Go's `math.NaN()`: the bits 0x7FF8000000000001, not Rust's `f64::NAN`.
const GO_NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0001);

/// `resolveMap`.
fn resolve_map(s: &str) -> Option<(&'static str, Resolved)> {
    Some(match s {
        "true" | "True" | "TRUE" => (BOOL_TAG, Resolved::Bool(true)),
        "false" | "False" | "FALSE" => (BOOL_TAG, Resolved::Bool(false)),
        "" | "~" | "null" | "Null" | "NULL" => (NULL_TAG, Resolved::Null),
        ".nan" | ".NaN" | ".NAN" => (FLOAT_TAG, Resolved::Float64(GO_NAN)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => (FLOAT_TAG, Resolved::Float64(f64::INFINITY)),
        "-.inf" | "-.Inf" | "-.INF" => (FLOAT_TAG, Resolved::Float64(f64::NEG_INFINITY)),
        "<<" => (MERGE_TAG, Resolved::String("<<".to_owned())),
        _ => return None,
    })
}

/// `yamlStyleFloat`: `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`.
fn yaml_style_float(s: &[u8]) -> bool {
    let digits = |from: usize| s[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let mut i = 0;
    if matches!(s.first(), Some(b'-' | b'+')) {
        i += 1;
    }
    if s.get(i) == Some(&b'.') {
        let d = digits(i + 1);
        if d == 0 {
            return false;
        }
        i += 1 + d;
    } else {
        let d = digits(i);
        if d == 0 {
            return false;
        }
        i += d;
        if s.get(i) == Some(&b'.') {
            i += 1 + digits(i + 1);
        }
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(s.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        let d = digits(i);
        if d == 0 {
            return false;
        }
        i += d;
    }
    i == s.len()
}

/// `resolve`: the resolved tag and value of a scalar with the given tag, or
/// the `failf` text (without `yaml: `) when the value does not fit the tag.
pub(crate) fn resolve(tag: &str, input: &str) -> Result<(String, Resolved), String> {
    let tag = short_tag(tag);
    if !resolvable_tag(&tag) {
        return Ok((tag, Resolved::String(input.to_owned())));
    }
    let (rtag, out) = resolve_value(&tag, input);
    // The deferred check.
    if tag.is_empty() || tag == rtag || tag == STR_TAG || tag == BINARY_TAG {
        return Ok((rtag.to_owned(), out));
    }
    if tag == FLOAT_TAG && rtag == INT_TAG {
        if let Resolved::Int(v) = out {
            return Ok((FLOAT_TAG.to_owned(), Resolved::Float64(v as f64)));
        }
    }
    Err(format!("cannot decode {} `{}` as a {}", short_tag(rtag), input, short_tag(&tag)))
}

fn int_result(v: i64) -> (&'static str, Resolved) {
    // Go's int is 64 bits wide, so int(intv) == intv always holds.
    (INT_TAG, Resolved::Int(v))
}

fn resolve_value(tag: &str, input: &str) -> (&'static str, Resolved) {
    let hint = input.as_bytes().first().map_or(b'N', |&b| hint(b));
    if hint != 0 && tag != STR_TAG && tag != BINARY_TAG {
        if let Some(item) = resolve_map(input) {
            return item;
        }
        match hint {
            b'.' => {
                let (v, err) = parse_float(input, 64);
                if err.is_none() {
                    return (FLOAT_TAG, Resolved::Float64(v));
                }
            }
            b'D' | b'S' => {
                if tag.is_empty() || tag == TIMESTAMP_TAG {
                    if let Some(t) = parse_timestamp(input) {
                        return (TIMESTAMP_TAG, Resolved::Time(t));
                    }
                }
                let plain = input.replace('_', "");
                let (v, err) = parse_int(&plain, 0, 64);
                if err.is_none() {
                    return int_result(v);
                }
                let (v, err) = parse_uint(&plain, 0, 64);
                if err.is_none() {
                    return (INT_TAG, Resolved::Uint64(v));
                }
                if yaml_style_float(plain.as_bytes()) {
                    let (v, err) = parse_float(&plain, 64);
                    if err.is_none() {
                        return (FLOAT_TAG, Resolved::Float64(v));
                    }
                }
                if let Some(rest) = plain.strip_prefix("0b") {
                    let (v, err) = parse_int(rest, 2, 64);
                    if err.is_none() {
                        return int_result(v);
                    }
                    let (v, err) = parse_uint(rest, 2, 64);
                    if err.is_none() {
                        return (INT_TAG, Resolved::Uint64(v));
                    }
                } else if let Some(rest) = plain.strip_prefix("-0b") {
                    let (v, err) = parse_int(format!("-{rest}"), 2, 64);
                    if err.is_none() {
                        return int_result(v);
                    }
                }
                if let Some(rest) = plain.strip_prefix("0o") {
                    let (v, err) = parse_int(rest, 8, 64);
                    if err.is_none() {
                        return int_result(v);
                    }
                    let (v, err) = parse_uint(rest, 8, 64);
                    if err.is_none() {
                        return (INT_TAG, Resolved::Uint64(v));
                    }
                } else if let Some(rest) = plain.strip_prefix("-0o") {
                    let (v, err) = parse_int(format!("-{rest}"), 8, 64);
                    if err.is_none() {
                        return int_result(v);
                    }
                }
            }
            // 'M': only the map entries above.
            _ => {}
        }
    }
    (STR_TAG, Resolved::String(input.to_owned()))
}

/// `allowedTimestampFormats`.
const TIMESTAMP_FORMATS: [&str; 4] = [
    "2006-1-2T15:4:5.999999999Z07:00",
    "2006-1-2t15:4:5.999999999Z07:00",
    "2006-1-2 15:4:5.999999999",
    "2006-1-2",
];

/// `parseTimestamp`: only values starting with four digits and `-` are
/// tried, in the formats above, with `time.Parse` (so a zone offset equal to
/// the local one gives `time.Local`).
fn parse_timestamp(s: &str) -> Option<Time> {
    let b = s.as_bytes();
    let i = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if i != 4 || i == b.len() || b[i] != b'-' {
        return None;
    }
    TIMESTAMP_FORMATS.iter().find_map(|layout| gotime::parse(layout, s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_style_float_matches_the_regexp() {
        for (s, want) in [
            ("1", true),
            ("1.", true),
            ("-1.5e3", true),
            (".5", true),
            ("+.5E-2", true),
            (".", false),
            ("e3", false),
            ("1e", false),
            ("1.5.", false),
            ("-.e1", false),
            ("", false),
        ] {
            assert_eq!(yaml_style_float(s.as_bytes()), want, "{s}");
        }
    }
}
