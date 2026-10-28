// Ports: $GOROOT/src/fmt/print.go and format.go (%#v of the values yaml.v3 decodes), $GOROOT/src/internal/fmtsort/sort.go (map key order), $GOROOT/src/encoding/base64/base64.go (StdEncoding.DecodeString) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go: Copyright 2009 The Go Authors. All rights reserved. Use
// of this source code is governed by a BSD-style license that can be found
// in crates/gorse-common/licenses/GO-LICENSE.

//! The parts of Go's standard library yaml.v3's decoder leans on: `%#v`,
//! which `invalid map key: %#v` prints a key with, and the base64 decoding
//! of `!!binary` scalars.

use std::cmp::Ordering;

use gorse_common::gostrconv::quote;
use gorse_common::gotime::time_parts;

use super::decoder::GoValue;

/// `fmt.Sprintf("%#v", v)` of a value held in an `interface{}`.
pub(crate) fn sharp_v(v: &GoValue) -> String {
    let mut out = String::new();
    write_sharp_v(&mut out, v, true);
    out
}

/// Writes `%#v` of `v`; `top` is false for the elements of slices and
/// maps, where a nil interface prints as `interface {}(nil)`.
fn write_sharp_v(out: &mut String, v: &GoValue, top: bool) {
    match v {
        GoValue::Nil => out.push_str(if top { "<nil>" } else { "interface {}(nil)" }),
        GoValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        GoValue::Int(i) => out.push_str(&i.to_string()),
        // %#v prints unsigned integers in hexadecimal.
        GoValue::Uint64(u) => out.push_str(&format!("{u:#x}")),
        GoValue::Float64(f) => out.push_str(&format_float_g(*f)),
        GoValue::String(s) => out.push_str(&quote(s)),
        GoValue::Time(t) => out.push_str(&t.go_string()),
        GoValue::Slice(items) => {
            out.push_str("[]interface {}{");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_sharp_v(out, item, false);
            }
            out.push('}');
        }
        GoValue::StringMap(m) => {
            out.push_str("map[string]interface {}{");
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&quote(k));
                out.push(':');
                write_sharp_v(out, v, false);
            }
            out.push('}');
        }
        GoValue::AnyMap(m) => {
            out.push_str("map[interface {}]interface {}{");
            let mut entries: Vec<_> = m.iter().collect();
            // sort.Stable, as fmtsort.Sort.
            entries.sort_by(|a, b| compare_interface(a.0, b.0));
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_sharp_v(out, k, false);
                out.push(':');
                write_sharp_v(out, v, false);
            }
            out.push('}');
        }
    }
}

/// The rank of a dynamic type in fmtsort's order of interface keys.
///
/// fmtsort orders keys of different dynamic types by the address of their
/// type descriptors, which the Go linker chooses. This is the order the
/// golden tool's binary (go1.27.0, linux/amd64) shows; keys of one type
/// compare by value. A map key that is itself a map with keys of several
/// types is the only place it shows.
fn type_rank(v: &GoValue) -> u8 {
    match v {
        GoValue::Nil => 0,
        GoValue::String(_) => 1,
        GoValue::Uint64(_) => 2,
        GoValue::Int(_) => 3,
        GoValue::Float64(_) => 4,
        GoValue::Bool(_) => 5,
        GoValue::Time(_) => 6,
        // Not hashable: never a map key.
        GoValue::Slice(_) | GoValue::StringMap(_) | GoValue::AnyMap(_) => 7,
    }
}

/// fmtsort's `compare` for two `interface{}` keys.
fn compare_interface(a: &GoValue, b: &GoValue) -> Ordering {
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        (GoValue::String(x), GoValue::String(y)) => x.cmp(y),
        (GoValue::Uint64(x), GoValue::Uint64(y)) => x.cmp(y),
        (GoValue::Int(x), GoValue::Int(y)) => x.cmp(y),
        // cmp.Compare: NaN first, -0 equal to 0.
        (GoValue::Float64(x), GoValue::Float64(y)) => match (x.is_nan(), y.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        },
        (GoValue::Bool(x), GoValue::Bool(y)) => x.cmp(y),
        // A struct compares field by field: wall, ext, then the location
        // pointer, whose order the port cannot know (equal here).
        (GoValue::Time(x), GoValue::Time(y)) => {
            let (wx, ex, _) = time_parts(x);
            let (wy, ey, _) = time_parts(y);
            wx.cmp(&wy).then(ex.cmp(&ey))
        }
        _ => Ordering::Equal,
    }
}

/// `strconv.FormatFloat(f, 'g', -1, 64)` with fmt's `+Inf`, `-Inf` and
/// `NaN`: the shortest digits that read back to `f`, in exponent form when
/// the decimal exponent is below -4 or at least 6.
pub(crate) fn format_float_g(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let sign = if v.is_sign_negative() { "-" } else { "" };
    if v == 0.0 {
        return format!("{sign}0");
    }
    // Rust's `{:e}` writes the shortest round-trip digits too, as
    // `d[.ddd]e<exp>`; only the layout differs from Go's.
    let sci = format!("{:e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mantissa.bytes().filter(|&b| b != b'.').collect();
    let nd = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let mut out = String::from(sign);
    if !(-4..6).contains(&exp) {
        out.extend(digits.first().map(|&b| char::from(b)));
        if digits.len() > 1 {
            out.push('.');
            out.extend(digits[1..].iter().map(|&b| char::from(b)));
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let exp = exp.unsigned_abs();
        if exp < 10 {
            out.push('0');
        }
        out.push_str(&exp.to_string());
    } else {
        // Digit `i` of the number, counting from the first significant one
        // at position `dp - 1` before the point.
        let dp = exp + 1;
        let digit = |i: i32| usize::try_from(i).ok().and_then(|i| digits.get(i)).map_or('0', |&b| char::from(b));
        if dp > 0 {
            out.extend((0..dp).map(digit));
        } else {
            out.push('0');
        }
        let decimals = (nd - dp).max(0);
        if decimals > 0 {
            out.push('.');
            out.extend((dp..dp + decimals).map(digit));
        }
    }
    out
}

/// The standard base64 alphabet's values (`decodeMap`), 0xFF for bytes
/// outside it.
fn decode_map(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' => b - b'A',
        b'a'..=b'z' => b - b'a' + 26,
        b'0'..=b'9' => b - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0xFF,
    }
}

/// `base64.StdEncoding.DecodeString`: the decoded bytes, or `None` on a
/// `CorruptInputError`. Line breaks (`\r`, `\n`) are skipped anywhere.
pub(crate) fn base64_decode(src: &[u8]) -> Option<Vec<u8>> {
    let mut dst = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        let (nsi, ok) = decode_quantum(&mut dst, src, si);
        if !ok {
            return None;
        }
        si = nsi;
    }
    Some(dst)
}

/// `decodeQuantum`: decodes up to four characters from `src[si..]` into
/// `dst`; returns the next index and whether the input is well-formed.
fn decode_quantum(dst: &mut Vec<u8>, src: &[u8], mut si: usize) -> (usize, bool) {
    let mut dbuf = [0u8; 4];
    let mut dlen = 4;
    let mut ok = true;
    let mut j = 0;
    while j < dbuf.len() {
        if src.len() == si {
            // The padding character is '=' (not NoPadding).
            if j == 0 {
                return (si, true);
            }
            return (si, false);
        }
        let input = src[si];
        si += 1;
        let out = decode_map(input);
        if out != 0xFF {
            dbuf[j] = out;
            j += 1;
            continue;
        }
        if input == b'\n' || input == b'\r' {
            continue;
        }
        if input != b'=' {
            return (si, false);
        }
        // We've reached the end and there's padding.
        match j {
            0 | 1 => return (si, false),
            2 => {
                // "==" is expected, the first "=" is already consumed.
                while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                    si += 1;
                }
                if si == src.len() || src[si] != b'=' {
                    return (si, false);
                }
                si += 1;
            }
            _ => {}
        }
        while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
            si += 1;
        }
        if si < src.len() {
            // Trailing garbage.
            ok = false;
        }
        dlen = j;
        break;
    }
    let val = (u32::from(dbuf[0]) << 18) | (u32::from(dbuf[1]) << 12) | (u32::from(dbuf[2]) << 6) | u32::from(dbuf[3]);
    let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
    // StdEncoding is not strict: the unused bits are not checked.
    dst.extend_from_slice(&bytes[..dlen - 1]);
    (si, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decode_follows_go() {
        let ok = |s: &str| base64_decode(s.as_bytes());
        assert_eq!(ok("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(ok("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(ok("aGVsbA=="), Some(b"hell".to_vec()));
        assert_eq!(ok("aGVsbA=\n="), Some(b"hell".to_vec()));
        assert_eq!(ok(""), Some(Vec::new()));
        assert_eq!(ok("\n\r"), Some(Vec::new()));
        assert_eq!(ok("aGVsbA"), None);
        assert_eq!(ok("aGVsbA="), None);
        assert_eq!(ok("aGVsbG8=x"), None);
        assert_eq!(ok("a"), None);
        assert_eq!(ok("a==="), None);
        assert_eq!(ok("a b="), None);
    }

    #[test]
    fn format_float_g_follows_go() {
        for (v, want) in [
            (1.0, "1"),
            (1e21, "1e+21"),
            (1e20, "1e+20"),
            (123456789.0, "1.23456789e+08"),
            (0.0001, "0.0001"),
            (1e-5, "1e-05"),
            (-0.0, "-0"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(format_float_g(v), want);
        }
    }
}
