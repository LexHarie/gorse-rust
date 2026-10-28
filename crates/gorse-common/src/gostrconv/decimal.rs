// Ports: $GOROOT/src/internal/strconv/decimal.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's strconv package: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Multiprecision decimal numbers, all of Go's `decimal.go`.
//! For floating-point formatting only; not general purpose.
//! Only operations are assign and (binary) left/right shift.
//! Can do binary floating point in multiprecision decimal precisely
//! because 2 divides 10; cannot do decimal floating point
//! in multiprecision binary precisely.
//!
//! `ParseFloat`'s slow path uses it, and so does Go's `FormatFloat`, so it is
//! public (hidden) for the ports of Go's tests and for the formatter.

use std::fmt;

/// The number of digits a [`Decimal`] holds.
pub const DIGITS: usize = 800;

/// Go's `decimal`. Go's `int` fields are `i64` (`nd` is never negative, so it
/// is a `usize`); the digits are ASCII.
#[derive(Clone)]
pub struct Decimal {
    /// Digits, big-endian representation.
    pub d: [u8; DIGITS],
    /// Number of digits used.
    pub nd: usize,
    /// Decimal point.
    pub dp: i64,
    /// Negative flag.
    pub neg: bool,
    /// Discarded nonzero digits beyond `d[..nd]`.
    pub trunc: bool,
}

impl Default for Decimal {
    fn default() -> Self {
        Self {
            d: [0; DIGITS],
            nd: 0,
            dp: 0,
            neg: false,
            trunc: false,
        }
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decimal")
            .field("d", &String::from_utf8_lossy(&self.d[..self.nd]))
            .field("nd", &self.nd)
            .field("dp", &self.dp)
            .field("neg", &self.neg)
            .field("trunc", &self.trunc)
            .finish()
    }
}

impl Decimal {
    /// The test export `NewDecimal(i)`: a zero decimal with `i` assigned.
    pub fn new(i: u64) -> Self {
        let mut d = Self::default();
        d.assign(i);
        d
    }
}

/// Go's `(*decimal).String`.
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let a = self;
        let nd = a.nd as i64;
        let mut buf: Vec<u8> = Vec::with_capacity((10 + nd + a.dp.abs()) as usize);
        if a.nd == 0 {
            return f.write_str("0");
        } else if a.dp <= 0 {
            // zeros fill space between decimal point and digits
            buf.push(b'0');
            buf.push(b'.');
            digit_zero(&mut buf, -a.dp);
            buf.extend_from_slice(&a.d[..a.nd]);
        } else if a.dp < nd {
            // decimal point in middle of digits
            let dp = a.dp as usize;
            buf.extend_from_slice(&a.d[..dp]);
            buf.push(b'.');
            buf.extend_from_slice(&a.d[dp..a.nd]);
        } else {
            // zeros fill space between digits and decimal point
            buf.extend_from_slice(&a.d[..a.nd]);
            digit_zero(&mut buf, a.dp - nd);
        }
        f.write_str(&String::from_utf8_lossy(&buf))
    }
}

/// Go's `digitZero`, appending instead of filling a slice.
fn digit_zero(dst: &mut Vec<u8>, n: i64) {
    dst.extend(std::iter::repeat_n(b'0', n as usize));
}

/// trim trailing zeros from number.
/// (They are meaningless; the decimal point is tracked
/// independent of the number of digits.)
fn trim(a: &mut Decimal) {
    while a.nd > 0 && a.d[a.nd - 1] == b'0' {
        a.nd -= 1;
    }
    if a.nd == 0 {
        a.dp = 0;
    }
}

/// Maximum shift that we can do in one pass without overflow.
/// A uint has 64 bits (Go's `uintSize` on the platforms Gorse ships for), and
/// we have to be able to accommodate 9<<k.
const UINT_SIZE: u32 = 64;
const MAX_SHIFT: u32 = UINT_SIZE - 4;

/// Binary shift right (/ 2) by k bits.  k <= MAX_SHIFT to avoid overflow.
fn right_shift(a: &mut Decimal, k: u32) {
    let mut r = 0; // read pointer
    let mut w = 0; // write pointer

    // Pick up enough leading digits to cover first shift.
    let mut n: u64 = 0;
    while n >> k == 0 {
        if r >= a.nd {
            if n == 0 {
                // a == 0; shouldn't get here, but handle anyway.
                a.nd = 0;
                return;
            }
            while n >> k == 0 {
                n *= 10;
                r += 1;
            }
            break;
        }
        let c = u64::from(a.d[r]);
        n = n * 10 + c - u64::from(b'0');
        r += 1;
    }
    a.dp -= r as i64 - 1;

    let mask: u64 = (1 << k) - 1;

    // Pick up a digit, put down a digit.
    while r < a.nd {
        let c = u64::from(a.d[r]);
        let dig = n >> k;
        n &= mask;
        a.d[w] = dig as u8 + b'0';
        w += 1;
        n = n * 10 + c - u64::from(b'0');
        r += 1;
    }

    // Put down extra digits.
    while n > 0 {
        let dig = n >> k;
        n &= mask;
        if w < a.d.len() {
            a.d[w] = dig as u8 + b'0';
            w += 1;
        } else if dig > 0 {
            a.trunc = true;
        }
        n *= 10;
    }

    a.nd = w;
    trim(a);
}

/// Cheat sheet for left shift: table indexed by shift count giving
/// number of new digits that will be introduced by that shift.
///
/// For example, `LEFTCHEATS[4] = (2, "625")`.  That means that
/// if we are shifting by 4 (multiplying by 16), it will add 2 digits
/// when the string prefix is "625" through "999", and one fewer digit
/// if the string prefix is "000" through "624".
///
/// Credit for this trick goes to Ken.
///
/// Each entry is (delta, cutoff): the number of new digits, minus one digit
/// if the original < cutoff.
static LEFTCHEATS: [(usize, &[u8]); 61] = [
    // Leading digits of 1/2^i = 5^i.
    // 5^23 is not an exact 64-bit floating point number,
    // so have to use bc for the math.
    // Go up to 60 to be large enough for 32bit and 64bit platforms.
    /*
        seq 60 | sed 's/^/5^/' | bc |
        awk 'BEGIN{ print "\t{ 0, \"\" }," }
        {
            log2 = log(2)/log(10)
            printf("\t{ %d, \"%s\" },\t// * %d\n",
                int(log2*NR+1), $0, 2**NR)
        }'
    */
    (0, b""),
    (1, b"5"),                                           // * 2
    (1, b"25"),                                          // * 4
    (1, b"125"),                                         // * 8
    (2, b"625"),                                         // * 16
    (2, b"3125"),                                        // * 32
    (2, b"15625"),                                       // * 64
    (3, b"78125"),                                       // * 128
    (3, b"390625"),                                      // * 256
    (3, b"1953125"),                                     // * 512
    (4, b"9765625"),                                     // * 1024
    (4, b"48828125"),                                    // * 2048
    (4, b"244140625"),                                   // * 4096
    (4, b"1220703125"),                                  // * 8192
    (5, b"6103515625"),                                  // * 16384
    (5, b"30517578125"),                                 // * 32768
    (5, b"152587890625"),                                // * 65536
    (6, b"762939453125"),                                // * 131072
    (6, b"3814697265625"),                               // * 262144
    (6, b"19073486328125"),                              // * 524288
    (7, b"95367431640625"),                              // * 1048576
    (7, b"476837158203125"),                             // * 2097152
    (7, b"2384185791015625"),                            // * 4194304
    (7, b"11920928955078125"),                           // * 8388608
    (8, b"59604644775390625"),                           // * 16777216
    (8, b"298023223876953125"),                          // * 33554432
    (8, b"1490116119384765625"),                         // * 67108864
    (9, b"7450580596923828125"),                         // * 134217728
    (9, b"37252902984619140625"),                        // * 268435456
    (9, b"186264514923095703125"),                       // * 536870912
    (10, b"931322574615478515625"),                      // * 1073741824
    (10, b"4656612873077392578125"),                     // * 2147483648
    (10, b"23283064365386962890625"),                    // * 4294967296
    (10, b"116415321826934814453125"),                   // * 8589934592
    (11, b"582076609134674072265625"),                   // * 17179869184
    (11, b"2910383045673370361328125"),                  // * 34359738368
    (11, b"14551915228366851806640625"),                 // * 68719476736
    (12, b"72759576141834259033203125"),                 // * 137438953472
    (12, b"363797880709171295166015625"),                // * 274877906944
    (12, b"1818989403545856475830078125"),               // * 549755813888
    (13, b"9094947017729282379150390625"),               // * 1099511627776
    (13, b"45474735088646411895751953125"),              // * 2199023255552
    (13, b"227373675443232059478759765625"),             // * 4398046511104
    (13, b"1136868377216160297393798828125"),            // * 8796093022208
    (14, b"5684341886080801486968994140625"),            // * 17592186044416
    (14, b"28421709430404007434844970703125"),           // * 35184372088832
    (14, b"142108547152020037174224853515625"),          // * 70368744177664
    (15, b"710542735760100185871124267578125"),          // * 140737488355328
    (15, b"3552713678800500929355621337890625"),         // * 281474976710656
    (15, b"17763568394002504646778106689453125"),        // * 562949953421312
    (16, b"88817841970012523233890533447265625"),        // * 1125899906842624
    (16, b"444089209850062616169452667236328125"),       // * 2251799813685248
    (16, b"2220446049250313080847263336181640625"),      // * 4503599627370496
    (16, b"11102230246251565404236316680908203125"),     // * 9007199254740992
    (17, b"55511151231257827021181583404541015625"),     // * 18014398509481984
    (17, b"277555756156289135105907917022705078125"),    // * 36028797018963968
    (17, b"1387778780781445675529539585113525390625"),   // * 72057594037927936
    (18, b"6938893903907228377647697925567626953125"),   // * 144115188075855872
    (18, b"34694469519536141888238489627838134765625"),  // * 288230376151711744
    (18, b"173472347597680709441192448139190673828125"), // * 576460752303423488
    (19, b"867361737988403547205962240695953369140625"), // * 1152921504606846976
];

/// Is the leading prefix of b lexicographically less than s?
fn prefix_is_less_than(b: &[u8], s: &[u8]) -> bool {
    for (i, &c) in s.iter().enumerate() {
        if i >= b.len() {
            return true;
        }
        if b[i] != c {
            return b[i] < c;
        }
    }
    false
}

/// Binary shift left (* 2) by k bits.  k <= MAX_SHIFT to avoid overflow.
fn left_shift(a: &mut Decimal, k: u32) {
    let (mut delta, cutoff) = LEFTCHEATS[k as usize];
    if prefix_is_less_than(&a.d[..a.nd], cutoff) {
        delta -= 1;
    }

    let mut w = a.nd + delta; // write index

    // Pick up a digit, put down a digit.
    let mut n: u64 = 0;
    for r in (0..a.nd).rev() {
        n += u64::from(a.d[r] - b'0') << k;
        let quo = n / 10;
        let rem = n - 10 * quo;
        w -= 1;
        if w < a.d.len() {
            a.d[w] = rem as u8 + b'0';
        } else if rem != 0 {
            a.trunc = true;
        }
        n = quo;
    }

    // Put down extra digits.
    while n > 0 {
        let quo = n / 10;
        let rem = n - 10 * quo;
        w -= 1;
        if w < a.d.len() {
            a.d[w] = rem as u8 + b'0';
        } else if rem != 0 {
            a.trunc = true;
        }
        n = quo;
    }

    a.nd += delta;
    if a.nd >= a.d.len() {
        a.nd = a.d.len();
    }
    a.dp += delta as i64;
    trim(a);
}

/// If we chop a at nd digits, should we round up?
fn should_round_up(a: &Decimal, nd: i64) -> bool {
    if nd < 0 || nd >= a.nd as i64 {
        return false;
    }
    let nd = nd as usize;
    if a.d[nd] == b'5' && nd + 1 == a.nd {
        // exactly halfway - round to even
        // if we truncated, a little higher than what's recorded - always round up
        if a.trunc {
            return true;
        }
        return nd > 0 && !(a.d[nd - 1] - b'0').is_multiple_of(2);
    }
    // not halfway - digit tells all
    a.d[nd] >= b'5'
}

impl Decimal {
    /// Assign v to a.
    pub fn assign(&mut self, v: u64) {
        let a = self;
        let mut buf = [0u8; 24];

        // Write reversed decimal in buf.
        let mut n = 0;
        let mut v = v;
        while v > 0 {
            let v1 = v / 10;
            v -= 10 * v1;
            buf[n] = v as u8 + b'0';
            n += 1;
            v = v1;
        }

        // Reverse again to produce forward decimal in a.d.
        a.nd = 0;
        for &c in buf[..n].iter().rev() {
            a.d[a.nd] = c;
            a.nd += 1;
        }
        a.dp = a.nd as i64;
        trim(a);
    }

    /// Binary shift left (k > 0) or right (k < 0).
    pub fn shift(&mut self, k: i64) {
        let a = self;
        let max_shift = i64::from(MAX_SHIFT);
        let mut k = k;
        if a.nd == 0 {
            // nothing to do: a == 0
        } else if k > 0 {
            while k > max_shift {
                left_shift(a, MAX_SHIFT);
                k -= max_shift;
            }
            left_shift(a, k as u32);
        } else if k < 0 {
            while k < -max_shift {
                right_shift(a, MAX_SHIFT);
                k += max_shift;
            }
            right_shift(a, (-k) as u32);
        }
    }

    /// Round a to nd digits (or fewer).
    /// If nd is zero, it means we're rounding
    /// just to the left of the digits, as in
    /// 0.09 -> 0.1.
    pub fn round(&mut self, nd: i64) {
        if nd < 0 || nd >= self.nd as i64 {
            return;
        }
        if should_round_up(self, nd) {
            self.round_up(nd);
        } else {
            self.round_down(nd);
        }
    }

    /// Round a down to nd digits (or fewer).
    pub fn round_down(&mut self, nd: i64) {
        if nd < 0 || nd >= self.nd as i64 {
            return;
        }
        self.nd = nd as usize;
        trim(self);
    }

    /// Round a up to nd digits (or fewer).
    pub fn round_up(&mut self, nd: i64) {
        let a = self;
        if nd < 0 || nd >= a.nd as i64 {
            return;
        }

        // round up
        for i in (0..nd as usize).rev() {
            let c = a.d[i];
            if c < b'9' {
                // can stop after this digit
                a.d[i] += 1;
                a.nd = i + 1;
                return;
            }
        }

        // Number is all 9s.
        // Change to single 1 with adjusted decimal point.
        a.d[0] = b'1';
        a.nd = 1;
        a.dp += 1;
    }

    /// Extract integer part, rounded appropriately.
    /// No guarantees about overflow.
    pub fn rounded_integer(&self) -> u64 {
        let a = self;
        if a.dp > 20 {
            return 0xFFFFFFFFFFFFFFFF;
        }
        let mut i: i64 = 0;
        let mut n: u64 = 0;
        while i < a.dp && i < a.nd as i64 {
            n = n
                .wrapping_mul(10)
                .wrapping_add(u64::from(a.d[i as usize] - b'0'));
            i += 1;
        }
        while i < a.dp {
            n = n.wrapping_mul(10);
            i += 1;
        }
        if should_round_up(a, a.dp) {
            n = n.wrapping_add(1);
        }
        n
    }
}
