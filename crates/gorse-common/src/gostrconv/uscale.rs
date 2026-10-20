// Ports: $GOROOT/src/internal/strconv/uscale.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2026 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Floating point binary↔decimal conversion by fast unrounded scaling.
//! See “Floating-Point Printing and Parsing Can Be Simple And Fast”,
//! <https://research.swtch.com/fp>
//!
//! The parsing half of `uscale.go` and the primitives both halves share
//! ([`Unrounded`], [`prescale`], [`uscale`], the logarithms). Go's formatting
//! functions (`fixedWidthFloat`, `shortFloat`, `skewed`, `setDigits`,
//! `numDigits`) belong with `FormatFloat` and are not ported here.

use super::pow10tab::{POW10_MIN, POW10_TAB};

/// Go's `bool2`: 1 for true, 0 for false.
pub const fn bool2(b: bool) -> u64 {
    b as u64
}

/// The bits Go's `ErrRange` comes with: whether the result overflowed.
type Range = bool;

/// Go's `pack64`: takes m, e and returns f = m * 2**e.
/// It assumes the caller has provided a 53-bit mantissa m
/// and an exponent that is in range for the mantissa.
/// The flag is Go's `ErrRange`.
pub fn pack64(m: u64, e: i64) -> (f64, Range) {
    if m & (1 << 52) == 0 {
        return (f64::from_bits(m), false);
    }
    if e >= 0x7FF - 1075 {
        return (f64::from_bits(m & (1 << 63) | 0x7ff << 52), true);
    }
    (
        f64::from_bits(m & !(1 << 52) | ((1075 + e) as u64) << 52),
        false,
    )
}

/// Go's `pack32`: takes m, e and returns f = m * 2**e.
/// It assumes the caller has provided a 24-bit mantissa m
/// and an exponent that is in range for the mantissa.
/// The flag is Go's `ErrRange`.
pub fn pack32(m: u32, e: i64) -> (f32, Range) {
    if m & (1 << 23) == 0 {
        return (f32::from_bits(m), false);
    }
    if e >= 0xFF - 150 {
        return (f32::from_bits(m & (1 << 31) | 0xff << 23), true);
    }
    (
        f32::from_bits(m & !(1 << 23) | ((150 + e) as u32) << 23),
        false,
    )
}

/// An unrounded represents an unrounded value: the value times 4, with the
/// low bit set when any discarded bit was nonzero. Go's `uint64` arithmetic
/// wraps, so these do too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unrounded(pub u64);

impl Unrounded {
    /// Go's `floor`.
    pub const fn floor(self) -> u64 {
        self.0 >> 2
    }

    /// Go's `roundHalfDown`.
    pub const fn round_half_down(self) -> u64 {
        self.0.wrapping_add(1) >> 2
    }

    /// Go's `round`: round half to even.
    pub const fn round(self) -> u64 {
        self.0.wrapping_add(1).wrapping_add((self.0 >> 2) & 1) >> 2
    }

    /// Go's `roundHalfUp`.
    pub const fn round_half_up(self) -> u64 {
        self.0.wrapping_add(2) >> 2
    }

    /// Go's `ceil`.
    pub const fn ceil(self) -> u64 {
        self.0.wrapping_add(3) >> 2
    }

    /// Go's `nudge`.
    pub const fn nudge(self, delta: i64) -> Self {
        Self(self.0.wrapping_add(delta as u64))
    }

    /// Go's `div`.
    pub const fn div(self, d: u64) -> Self {
        let x = self.0;
        Self((x / d) | self.0 & 1 | bool2(!x.is_multiple_of(d)))
    }

    /// Go's `rsh`.
    pub const fn rsh(self, s: i64) -> Self {
        let u = self.0;
        Self(shr64(u, s) | u & 1 | bool2(u & shl64(1, s).wrapping_sub(1) != 0))
    }
}

/// `log10_pow2(x)` returns ⌊log₁₀ 2**x⌋ = ⌊x * log₁₀ 2⌋.
pub const fn log10_pow2(x: i64) -> i64 {
    // log₁₀ 2 ≈ 0.30102999566 ≈ 78913 / 2^18
    (x * 78913) >> 18
}

/// `log2_pow10(x)` returns ⌊log₂ 10**x⌋ = ⌊x * log₂ 10⌋.
pub const fn log2_pow10(x: i64) -> i64 {
    // log₂ 10 ≈ 3.32192809489 ≈ 108853 / 2^15
    (x * 108853) >> 15
}

/// `UINT64POW10[x]` is 10**x.
pub static UINT64POW10: [u64; 20] = [
    1,
    1e1 as u64,
    1e2 as u64,
    1e3 as u64,
    1e4 as u64,
    1e5 as u64,
    1e6 as u64,
    1e7 as u64,
    1e8 as u64,
    1e9 as u64,
    1e10 as u64,
    1e11 as u64,
    1e12 as u64,
    1e13 as u64,
    1e14 as u64,
    1e15 as u64,
    1e16 as u64,
    1e17 as u64,
    1e18 as u64,
    1e19 as u64,
];

/// Go's `bits.Len64`.
const fn len64(x: u64) -> i64 {
    (64 - x.leading_zeros()) as i64
}

/// Go's `x << s` for an unsigned x and s >= 0: 0 once s reaches the width.
const fn shl64(x: u64, s: i64) -> u64 {
    if s >= 64 { 0 } else { x << s }
}

/// Go's `x >> s` for an unsigned x and s >= 0: 0 once s reaches the width.
const fn shr64(x: u64, s: i64) -> u64 {
    if s >= 64 { 0 } else { x >> s }
}

/// `parse_float64` rounds d * 10**p to the nearest float64 f.
/// d can have at most 19 digits.
/// The flag is Go's `ErrRange`: the result rounds to infinity.
pub fn parse_float64(d: u64, p: i64, sign: u64) -> (f64, Range) {
    let b = len64(d);
    let lp = log2_pow10(p);
    let mut e = (53 - b - lp).min(1074);
    let pre = prescale(e - (64 - b), p, lp);
    if pre.s >= 64 {
        return (f64::from_bits(sign), false);
    }
    let mut u = uscale(shl64(d, 64 - b), &pre);

    // This block is branch-free code for:
    //	if u.round() >= 1<<53 {
    //		u = u.rsh(1)
    //		e = e - 1
    //	}
    let s = bool2(u >= unmin(1 << 53));
    u = Unrounded(u.0 >> s | u.0 & 1);
    e -= s as i64;

    pack64(sign | u.round(), -e)
}

/// `parse_float32` rounds d * 10**p to the nearest float32 f.
/// d can have at most 19 digits.
/// The flag is Go's `ErrRange`: the result rounds to infinity.
pub fn parse_float32(d: u64, p: i64, sign: u32) -> (f32, Range) {
    let b = len64(d);
    let lp = log2_pow10(p);
    let mut e = (24 - b - lp).min(149);
    let pre = prescale(e - (64 - b), p, lp);
    if pre.s >= 64 {
        return (f32::from_bits(sign), false);
    }
    let mut u = uscale(shl64(d, 64 - b), &pre);

    // This block is branch-free code for:
    //	if u.round() >= 1<<24 {
    //		u = u.rsh(1)
    //		e = e - 1
    //	}
    let s = bool2(u >= unmin(1 << 24));
    u = Unrounded(u.0 >> s | u.0 & 1);
    e -= s as i64;

    pack32(sign | u.round() as u32, -e)
}

/// `unmin` returns the minimum unrounded that rounds to x.
pub const fn unmin(x: u64) -> Unrounded {
    Unrounded((x << 2).wrapping_sub(2))
}

/// A scaler holds derived scaling constants for a given e, p pair.
#[derive(Debug, Clone, Copy)]
pub struct Scaler {
    /// The high 64 bits of the 128-bit mantissa of 10**p.
    pub pm_hi: u64,
    /// The low 64 bits, subtracted: the mantissa is `pm_hi<<64 - pm_lo`.
    pub pm_lo: u64,
    /// The right shift.
    pub s: i64,
}

/// `prescale` returns the scaling constants for e, p.
/// lp must be `log2_pow10(p)`.
/// The caller is responsible for either avoiding e, p pairs
/// that cause pre.s < 0 or pre.s >= 64, or else handling
/// those cases before passing the result to uscale.
/// In practice, pre.s < 0 would indicate a buggy caller
/// and pre.s >= 64 can only happen for parsing and is
/// picked off at those call sites.
pub fn prescale(e: i64, p: i64, lp: i64) -> Scaler {
    let (hi, lo) = POW10_TAB[(p - POW10_MIN) as usize];
    Scaler {
        pm_hi: hi,
        pm_lo: lo,
        s: -(e + lp + 3),
    }
}

/// Go's `bits.Mul64`: the high and low halves of x*y.
const fn mul64(x: u64, y: u64) -> (u64, u64) {
    let p = (x as u128) * (y as u128);
    ((p >> 64) as u64, p as u64)
}

/// `uscale` returns unround(x * 2**e * 10**p).
/// The caller should pass &pre for prescale(e, p, log2Pow10(p))
/// and should have left-justified x so its high bit is set.
/// The caller is also responsible for checking that c.s < 64.
/// For formatting, that's always true.
/// For parsing, the caller needs to pick it off early and return a signed 0.
pub fn uscale(x: u64, c: &Scaler) -> Unrounded {
    let (mut hi, mid) = mul64(x, c.pm_hi);
    let s = c.s & 63; // make shifts cheaper
    if hi >> s << s != hi {
        return Unrounded(hi >> s | 1);
    }
    let (mid2, _) = mul64(x, c.pm_lo);
    hi = hi.wrapping_sub(bool2(mid < mid2));
    Unrounded(hi >> s | bool2(mid.wrapping_sub(mid2) > 1))
}
