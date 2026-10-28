// Ports: $GOROOT/src/strconv/ftoa.go (FormatFloat(f, 'g', -1, 64): formatDigits, fmtE, fmtF) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-ratelimit/licenses/GO-LICENSE.

//! Go's `strconv.FormatFloat(f, 'g', -1, 64)`, which juju's panic message for
//! a rate without a suitable quantum uses.

/// Go's `strconv.FormatFloat(f, 'g', -1, 64)`: the shortest decimal that
/// reads back as `f`, in `%e` form when its exponent is below -4 or at least
/// 6 (`1e+25`, `1.5e-07`), else in `%f` form (`0.0001`, `123456`).
pub fn format_g(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let mut dst = String::new();
    if f.is_sign_negative() {
        dst.push('-');
    }
    if f == 0.0 {
        // Go's digits are empty, dp 0: %f with no fraction.
        dst.push('0');
        return dst;
    }
    // Rust's `{:e}` prints the shortest digits that read back as `f`, the
    // closest such, like Go's Ryu shortest mode.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: Vec<u8> = mantissa.bytes().filter(|b| *b != b'.').collect();
    let nd = digits.len() as i64;
    let dp = exponent.parse::<i64>().unwrap_or(0) + 1;

    let exp = dp - 1;
    if exp < -4 || exp >= 6 {
        // fmtE with prec nd-1.
        dst.push(char::from(digits[0]));
        if nd > 1 {
            dst.push('.');
            dst.extend(digits[1..].iter().map(|&b| char::from(b)));
        }
        dst.push('e');
        dst.push(if exp < 0 { '-' } else { '+' });
        let exp = exp.abs();
        if exp < 10 {
            dst.push('0');
        }
        dst.push_str(&exp.to_string());
        return dst;
    }
    // fmtF with prec max(nd-dp, 0).
    if dp > 0 {
        let m = nd.min(dp);
        dst.extend(digits[..m as usize].iter().map(|&b| char::from(b)));
        for _ in m..dp {
            dst.push('0');
        }
    } else {
        dst.push('0');
    }
    let prec = (nd - dp).max(0);
    if prec > 0 {
        dst.push('.');
        for i in 0..prec {
            let j = dp + i;
            dst.push(if (0..nd).contains(&j) {
                char::from(digits[j as usize])
            } else {
                '0'
            });
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::format_g;

    #[test]
    fn formats_like_go() {
        // Expected texts are Go's (go1.27.0 strconv.FormatFloat(f, 'g', -1, 64));
        // the golden test checks many more.
        for (f, want) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (0.5, "0.5"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (123456.0, "123456"),
            (1234567.0, "1.234567e+06"),
            (1e25, "1e+25"),
            (1.5e300, "1.5e+300"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "+Inf"),
            (f64::NEG_INFINITY, "-Inf"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(format_g(f), want, "{f:e}");
        }
    }
}
