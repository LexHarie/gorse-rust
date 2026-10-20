// Ports: cmd/compile/internal/ssa/_gen/AMD64.rules (Min32F, Max32F lowering) @ go1.27.0
// Ports: runtime/error.go (boundsError messages) @ go1.27.0
// Ports: fmt/print.go, fmt/format.go (%v of float32 and []int) @ go1.27.0
// Ports: strconv/atoi.go (Atoi) @ go1.27.0
//
// Derived from Go: Copyright 2009 The Go Authors. All rights reserved. Use of
// this source code is governed by a BSD-style license that can be found in
// crates/gorse-common/licenses/GO-LICENSE.

//! The pieces of the Go language and runtime that `common/nn` results and
//! panic messages depend on.

use std::ops::Range;

/// The builtin `max(x, y)` of two `float32` as Go compiles it on amd64:
/// `-min(-x, -y)`, where `min` is `POR(MINSS(MINSS(x, y), x), MINSS(x, y))`
/// and `MINSS(a, b)` is `a < b ? a : b`. A NaN operand gives a NaN, though
/// not always the same NaN, and `max(-0, +0)` is `+0`.
pub(crate) fn max32(x: f32, y: f32) -> f32 {
    fn minss(a: f32, b: f32) -> f32 {
        if a < b { a } else { b }
    }
    let (nx, ny) = (-x, -y);
    let t = minss(nx, ny);
    let min = f32::from_bits(minss(t, nx).to_bits() | t.to_bits());
    -min
}

/// The message of a Go index expression `s[i]` out of range.
pub(crate) fn index_panic(i: i64, len: usize) -> String {
    if i < 0 {
        format!("runtime error: index out of range [{i}]")
    } else {
        format!("runtime error: index out of range [{i}] with length {len}")
    }
}

/// `s[i]` with Go's bounds check: the position of element `i` of a slice of
/// length `len`.
pub(crate) fn index(i: i64, len: usize) -> usize {
    match usize::try_from(i) {
        Ok(u) if u < len => u,
        _ => panic!("{}", index_panic(i, len)),
    }
}

/// `s[low:high]` with Go's bounds checks on a slice with capacity `cap`:
/// `high` against the capacity first, then `low` against `high`.
pub(crate) fn slice(low: i64, high: i64, cap: usize) -> Range<usize> {
    let h = match usize::try_from(high) {
        Ok(h) if h <= cap => h,
        _ if high < 0 => panic!("runtime error: slice bounds out of range [:{high}]"),
        _ => panic!("runtime error: slice bounds out of range [:{high}] with capacity {cap}"),
    };
    match usize::try_from(low) {
        Ok(l) if l <= h => l..h,
        _ if low < 0 => panic!("runtime error: slice bounds out of range [{low}:]"),
        _ => panic!("runtime error: slice bounds out of range [{low}:{high}]"),
    }
}

/// An `int` from a `usize` that fits, as Go's lengths and shapes are.
pub(crate) fn int(u: usize) -> i64 {
    i64::try_from(u).unwrap_or_else(|_| panic!("{u} does not fit in a Go int"))
}

/// `%v` of a `[]int`: `[2 3]`.
pub(crate) fn format_ints(v: &[usize]) -> String {
    let items: Vec<String> = v.iter().map(ToString::to_string).collect();
    format!("[{}]", items.join(" "))
}

/// `%v` of a `float32`: `strconv.FormatFloat(v, 'g', -1, 32)`, the shortest
/// digits that read back to `v`, in exponent form when the decimal exponent
/// is below -4 or at least 6 (`1e+06`, `1e-05`), plus `+Inf`, `-Inf` and
/// `NaN`.
pub(crate) fn format_float32(v: f32) -> String {
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
    let (digits, exp) = shortest_digits(v.abs());
    let nd = i32::try_from(digits.len()).expect("at most 9 digits");
    let mut out = String::from(sign);
    // Go: `exp < -4 || exp >= eprec`, with `eprec` 6 for shortest output.
    if !(-4..6).contains(&exp) {
        out.push(digits[0]);
        if digits.len() > 1 {
            out.push('.');
            out.extend(&digits[1..]);
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
        let digit = |i: i32| {
            usize::try_from(i)
                .ok()
                .and_then(|i| digits.get(i).copied())
                .unwrap_or('0')
        };
        if dp > 0 {
            out.extend((0..dp).map(digit));
        } else {
            out.push('0');
        }
        if nd > dp {
            out.push('.');
            out.extend((dp.min(0)..0).map(|_| '0'));
            out.extend((dp.max(0)..nd).map(digit));
        }
    }
    out
}

/// The shortest decimal digits that read back to `v` (finite, positive), as
/// Go's `shortFloat` picks them, and the decimal exponent of the first.
///
/// These are the digits of Rust's `{:e}`, except where two candidates are
/// equally near `v`: Go takes the even one (`unrounded.round`), Rust the
/// other one. `float32(3493168.25)` is `3.4931682e+06` in Go.
fn shortest_digits(v: f32) -> (Vec<char>, i32) {
    let split = |s: String| {
        let (mantissa, exp) = s.split_once('e').expect("`{:e}` writes an exponent");
        let exp: i32 = exp.parse().expect("`{:e}` writes a decimal exponent");
        let digits: Vec<char> = mantissa.chars().filter(|&c| c != '.').collect();
        (digits, exp)
    };
    let (digits, exp) = split(format!("{:e}", v));
    // The exact value: an `f32` has at most 112 significant digits.
    let (mut exact, exact_exp) = split(format!("{:.120e}", f64::from(v)));
    while exact.last() == Some(&'0') {
        exact.pop();
    }
    let nd = digits.len();
    if exact_exp != exp || exact.len() != nd + 1 || exact[nd] != '5' {
        return (digits, exp);
    }
    // `v` is halfway between the `nd`-digit numbers `low` and `low + 1`.
    let low: u64 = exact[..nd]
        .iter()
        .collect::<String>()
        .parse()
        .expect("digits");
    let even = low + low % 2;
    let mut digits: Vec<char> = even.to_string().chars().collect();
    let exp = exp + i32::try_from(digits.len()).expect("few digits")
        - i32::try_from(nd).expect("few digits");
    while digits.len() > 1 && digits.last() == Some(&'0') {
        digits.pop();
    }
    (digits, exp)
}

/// Why `strconv.Atoi` rejects its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtoiError {
    /// `invalid syntax`.
    Syntax,
    /// `value out of range`.
    Range,
}

/// `strconv.Atoi` on a 64-bit platform: an optional sign and decimal digits,
/// no underscores, within `int64`. As in `ParseUint`, the digits are read in
/// order, so a number that overflows before a bad character is out of range
/// rather than invalid.
pub(crate) fn atoi(s: &str) -> Result<i64, AtoiError> {
    let (neg, digits) = match s.as_bytes() {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    if digits.is_empty() {
        return Err(AtoiError::Syntax);
    }
    let mut n: u64 = 0;
    for &c in digits {
        if !c.is_ascii_digit() {
            return Err(AtoiError::Syntax);
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(c - b'0')))
            .ok_or(AtoiError::Range)?;
    }
    if neg {
        0i64.checked_sub_unsigned(n).ok_or(AtoiError::Range)
    } else {
        i64::try_from(n).map_err(|_| AtoiError::Range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nn_go_max32_follows_the_amd64_lowering() {
        let neg_zero = -0.0f32;
        assert_eq!(max32(neg_zero, 0.0).to_bits(), 0);
        assert_eq!(max32(0.0, neg_zero).to_bits(), 0);
        assert_eq!(max32(neg_zero, neg_zero).to_bits(), 0x8000_0000);
        assert_eq!(max32(1.0, 2.0), 2.0);
        assert_eq!(max32(2.0, 1.0), 2.0);
        // A negative NaN loses its sign against 0; a signalling NaN survives.
        assert_eq!(
            max32(f32::from_bits(0xffc0_0000), 0.0).to_bits(),
            0x7fc0_0000
        );
        assert_eq!(
            max32(f32::from_bits(0x7f80_0001), 0.0).to_bits(),
            0x7f80_0001
        );
        assert_eq!(
            max32(0.0, f32::from_bits(0x7fc1_2345)).to_bits(),
            0x7fc1_2345
        );
    }

    #[test]
    fn nn_go_bounds_messages() {
        let msg = |f: fn()| {
            let e = std::panic::catch_unwind(f).expect_err("panics");
            e.downcast_ref::<String>().expect("message").clone()
        };
        assert_eq!(
            msg(|| {
                let _ = index(-1, 3);
            }),
            "runtime error: index out of range [-1]"
        );
        assert_eq!(
            msg(|| {
                let _ = index(3, 3);
            }),
            "runtime error: index out of range [3] with length 3"
        );
        assert_eq!(
            msg(|| drop(slice(0, 7, 6))),
            "runtime error: slice bounds out of range [:7] with capacity 6"
        );
        assert_eq!(
            msg(|| drop(slice(4, 2, 6))),
            "runtime error: slice bounds out of range [4:2]"
        );
        assert_eq!(
            msg(|| drop(slice(-2, 0, 6))),
            "runtime error: slice bounds out of range [-2:]"
        );
        assert_eq!(
            msg(|| drop(slice(0, -1, 6))),
            "runtime error: slice bounds out of range [:-1]"
        );
        assert_eq!(slice(2, 6, 6), 2..6);
    }

    #[test]
    fn nn_go_atoi() {
        assert_eq!(atoi("12"), Ok(12));
        assert_eq!(atoi("+7"), Ok(7));
        assert_eq!(atoi("-0"), Ok(0));
        assert_eq!(atoi("007"), Ok(7));
        assert_eq!(atoi("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(atoi("9223372036854775808"), Err(AtoiError::Range));
        assert_eq!(atoi("99999999999999999999999"), Err(AtoiError::Range));
        assert_eq!(atoi("99999999999999999999x"), Err(AtoiError::Range));
        assert_eq!(atoi("9x9999999999999999999"), Err(AtoiError::Syntax));
        for bad in ["", "+", "-", "1_000", " 1", "1 ", "x", "0x10", "1.0"] {
            assert_eq!(atoi(bad), Err(AtoiError::Syntax), "{bad:?}");
        }
    }
}
