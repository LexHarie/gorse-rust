// Ports: the 'g' (precision -1), 'e' and 'b' formats of $GOROOT/src/internal/strconv/ftoa.go (fmtE, fmtF, the 'g' choice in fmtEFG, fmtB) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! A test-local `strconv.FormatFloat` for the formats Go's float-parsing
//! tests print with. gostrconv has no `FormatFloat` yet, so the digits come
//! from Rust's formatter (`{:e}` gives the shortest digits that round-trip,
//! `{:.N$e}` the correctly rounded N+1 digits) and Go's layout of them is
//! ported on top. `gostrconv_golden.rs` checks it against Go's own
//! `FormatFloat` output in `tests/data/strconv_golden.json`.

#![allow(dead_code, reason = "each test binary uses a subset of the helpers")]

/// Decimal digits and a decimal point: the value is `0.<d> * 10^dp`.
struct Digits {
    d: Vec<u8>,
    dp: i64,
}

/// Parses Rust's `{:e}` form ("1.25e-7", "3e2", "0e0") into digits.
fn digits_of(e_form: &str) -> Digits {
    let (mant, exp) = e_form.split_once('e').expect("exponent");
    let exp: i64 = exp.parse().expect("exponent digits");
    let d: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).collect();
    if d.iter().all(|&c| c == b'0') {
        return Digits {
            d: Vec::new(),
            dp: 0,
        };
    }
    Digits { d, dp: exp + 1 }
}

/// Go's special values, or None for a finite x.
fn special(nan: bool, inf: bool, neg: bool) -> Option<String> {
    if nan {
        return Some("NaN".into());
    }
    if inf {
        return Some(if neg { "-Inf" } else { "+Inf" }.into());
    }
    None
}

/// Go's `fmtE`: `d.ddddde±dd` with prec digits after the point.
fn fmt_e(neg: bool, digs: &Digits, prec: i64) -> String {
    let mut dst = String::new();
    if neg {
        dst.push('-');
    }
    // first digit
    let ch = digs.d.first().copied().unwrap_or(b'0');
    dst.push(char::from(ch));
    // .moredigits
    if prec > 0 {
        dst.push('.');
        let mut i = 1;
        let m = digs.d.len().min(prec as usize + 1);
        if i < m {
            dst.extend(digs.d[i..m].iter().map(|&c| char::from(c)));
            i = m;
        }
        while i as i64 <= prec {
            dst.push('0');
            i += 1;
        }
    }
    // e±
    dst.push('e');
    let mut exp = digs.dp - 1;
    if digs.d.is_empty() {
        // special case: 0 has exponent 0
        exp = 0;
    }
    if exp < 0 {
        dst.push('-');
        exp = -exp;
    } else {
        dst.push('+');
    }
    // dd or ddd
    if exp < 10 {
        dst.push('0');
    }
    dst.push_str(&exp.to_string());
    dst
}

/// Go's `fmtF`: `ddddd.ddddd` with prec digits after the point.
fn fmt_f(neg: bool, digs: &Digits, prec: i64) -> String {
    let mut dst = String::new();
    if neg {
        dst.push('-');
    }
    let nd = digs.d.len() as i64;
    // integer, padded with zeros as needed.
    if digs.dp > 0 {
        let m = nd.min(digs.dp);
        dst.extend(digs.d[..m as usize].iter().map(|&c| char::from(c)));
        for _ in m..digs.dp {
            dst.push('0');
        }
    } else {
        dst.push('0');
    }
    // fraction
    if prec > 0 {
        dst.push('.');
        for i in 1..=prec {
            let j = digs.dp + i - 1;
            let ch = if (0..nd).contains(&j) {
                digs.d[j as usize]
            } else {
                b'0'
            };
            dst.push(char::from(ch));
        }
    }
    dst
}

/// Go's 'g' with the shortest digits: %e when the exponent is below -4 or at
/// least 6 (eprec for the shortest form), %f otherwise.
fn fmt_g_shortest(neg: bool, digs: &Digits) -> String {
    let nd = digs.d.len() as i64;
    let eprec = 6;
    let exp = digs.dp - 1;
    if exp < -4 || exp >= eprec {
        return fmt_e(neg, digs, nd - 1);
    }
    fmt_f(neg, digs, (nd - digs.dp).max(0))
}

/// Go's shortest digits. Rust's `{:e}` gives as few digits as round-trip,
/// but when several such strings of that length round-trip it does not
/// always take the one Go's `shortFloat` takes: the value correctly rounded
/// (half to even) to that many digits. `rounded(n)` is that value with n
/// digits after the point, if it round-trips.
fn shortest(rust: &str, rounded: impl Fn(usize) -> Option<String>) -> Digits {
    let digs = digits_of(rust);
    if digs.d.is_empty() {
        return digs;
    }
    match rounded(digs.d.len() - 1) {
        Some(s) => digits_of(&s),
        None => digs,
    }
}

/// `strconv.FormatFloat(f, 'g', -1, 64)`.
pub fn format_g64(f: f64) -> String {
    if let Some(s) = special(f.is_nan(), f.is_infinite(), f.is_sign_negative()) {
        return s;
    }
    let x = f.abs();
    let digs = shortest(&format!("{x:e}"), |n| {
        let s = format!("{x:.n$e}");
        (s.parse::<f64>() == Ok(x)).then_some(s)
    });
    fmt_g_shortest(f.is_sign_negative(), &digs)
}

/// `strconv.FormatFloat(float64(f), 'g', -1, 32)`.
pub fn format_g32(f: f32) -> String {
    if let Some(s) = special(f.is_nan(), f.is_infinite(), f.is_sign_negative()) {
        return s;
    }
    let x = f.abs();
    let digs = shortest(&format!("{x:e}"), |n| {
        let s = format!("{x:.n$e}");
        (s.parse::<f32>() == Ok(x)).then_some(s)
    });
    fmt_g_shortest(f.is_sign_negative(), &digs)
}

/// `strconv.FormatFloat(f, 'e', prec, 64)`.
pub fn format_e64(f: f64, prec: usize) -> String {
    if let Some(s) = special(f.is_nan(), f.is_infinite(), f.is_sign_negative()) {
        return s;
    }
    let digs = digits_of(&format!("{:.*e}", prec, f.abs()));
    fmt_e(f.is_sign_negative(), &digs, prec as i64)
}

/// `strconv.FormatFloat(float64(f), 'e', prec, 32)`.
pub fn format_e32(f: f32, prec: usize) -> String {
    if let Some(s) = special(f.is_nan(), f.is_infinite(), f.is_sign_negative()) {
        return s;
    }
    let digs = digits_of(&format!("{:.*e}", prec, f.abs()));
    fmt_e(f.is_sign_negative(), &digs, prec as i64)
}

/// Go's `fmtB`: `mantissa p ±exponent`, the mantissa with its implicit bit.
fn fmt_b(neg: bool, bits: u64, mant_bits: u32, exp_bits: u32, bias: i64) -> String {
    let mut exp = ((bits >> mant_bits) & ((1 << exp_bits) - 1)) as i64;
    let mut mant = bits & ((1 << mant_bits) - 1);
    if exp == (1 << exp_bits) - 1 {
        return special(mant != 0, true, neg).expect("special");
    }
    if exp == 0 {
        exp += 1;
    } else {
        mant |= 1 << mant_bits;
    }
    exp += bias;
    let exp = exp - i64::from(mant_bits);
    let mut dst = String::new();
    if neg {
        dst.push('-');
    }
    dst.push_str(&mant.to_string());
    dst.push('p');
    if exp >= 0 {
        dst.push('+');
    }
    dst.push_str(&exp.to_string());
    dst
}

/// `strconv.FormatFloat(f, 'b', -1, 64)`.
pub fn format_b64(f: f64) -> String {
    fmt_b(f.is_sign_negative(), f.to_bits(), 52, 11, -1023)
}

/// `strconv.FormatFloat(float64(f), 'b', -1, 32)`.
pub fn format_b32(f: f32) -> String {
    fmt_b(f.is_sign_negative(), u64::from(f.to_bits()), 23, 8, -127)
}
