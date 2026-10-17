// Ports: $GOROOT/src/time/time.go (Duration) and format.go (ParseDuration) @ go1.27.0
//
// Derived from Go's time package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `time.Duration`: an `i64` count of nanoseconds, its `String` form
//! ("72h3m0.5s") and `ParseDuration`, which Gorse configs and REST parameters
//! go through.

use std::fmt;

use super::format::{leading_fraction, leading_int, quote};

/// Elapsed time as an `i64` nanosecond count, like Go's `time.Duration`.
/// Arithmetic on the field wraps as Go's does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(pub i64);

/// One nanosecond.
pub const NANOSECOND: Duration = Duration(1);
/// One microsecond.
pub const MICROSECOND: Duration = Duration(1000 * NANOSECOND.0);
/// One millisecond.
pub const MILLISECOND: Duration = Duration(1000 * MICROSECOND.0);
/// One second.
pub const SECOND: Duration = Duration(1000 * MILLISECOND.0);
/// One minute.
pub const MINUTE: Duration = Duration(60 * SECOND.0);
/// One hour.
pub const HOUR: Duration = Duration(60 * MINUTE.0);

impl Duration {
    /// The smallest Duration.
    pub const MIN: Duration = Duration(i64::MIN);
    /// The largest Duration.
    pub const MAX: Duration = Duration(i64::MAX);

    /// The duration as a nanosecond count.
    pub fn nanoseconds(self) -> i64 {
        self.0
    }

    /// The duration as a microsecond count, truncated toward zero.
    pub fn microseconds(self) -> i64 {
        self.0 / 1000
    }

    /// The duration as a millisecond count, truncated toward zero.
    pub fn milliseconds(self) -> i64 {
        self.0 / 1_000_000
    }

    /// The duration as a floating point number of seconds.
    pub fn seconds(self) -> f64 {
        let sec = self.0 / SECOND.0;
        let nsec = self.0 % SECOND.0;
        sec as f64 + nsec as f64 / 1e9
    }

    /// The duration as a floating point number of minutes.
    pub fn minutes(self) -> f64 {
        let min = self.0 / MINUTE.0;
        let nsec = self.0 % MINUTE.0;
        min as f64 + nsec as f64 / (60.0 * 1e9)
    }

    /// The duration as a floating point number of hours.
    pub fn hours(self) -> f64 {
        let hour = self.0 / HOUR.0;
        let nsec = self.0 % HOUR.0;
        hour as f64 + nsec as f64 / (60.0 * 60.0 * 1e9)
    }

    /// Rounds d toward zero to a multiple of m; d unchanged if m <= 0.
    #[must_use]
    pub fn truncate(self, m: Duration) -> Duration {
        if m.0 <= 0 {
            return self;
        }
        Duration(self.0 - self.0 % m.0)
    }

    /// Rounds d to the nearest multiple of m, halfway values away from zero,
    /// saturating at the Duration range; d unchanged if m <= 0.
    #[must_use]
    pub fn round(self, m: Duration) -> Duration {
        if m.0 <= 0 {
            return self;
        }
        let d = self.0;
        let mut r = d % m.0;
        if d < 0 {
            r = -r;
            if less_than_half(Duration(r), m) {
                return Duration(d + r);
            }
            let d1 = d.wrapping_sub(m.0).wrapping_add(r);
            if d1 < d {
                return Duration(d1);
            }
            return Duration::MIN; // overflow
        }
        if less_than_half(Duration(r), m) {
            return Duration(d - r);
        }
        let d1 = d.wrapping_add(m.0).wrapping_sub(r);
        if d1 > d {
            return Duration(d1);
        }
        Duration::MAX // overflow
    }

    /// The absolute value; `Duration::MIN` becomes `Duration::MAX`.
    #[must_use]
    pub fn abs(self) -> Duration {
        match self.0 {
            d if d >= 0 => self,
            i64::MIN => Duration::MAX,
            d => Duration(-d),
        }
    }

    // Formats d into the end of buf and returns the index of the first byte.
    fn format(self, buf: &mut [u8; 32]) -> usize {
        // Largest time is 2540400h10m10.000000000s.
        let mut w = buf.len();
        let mut u = self.0 as u64;
        let neg = self.0 < 0;
        if neg {
            u = u.wrapping_neg();
        }

        if u < SECOND.0 as u64 {
            // Smaller than a second: use a smaller unit, like 1.2ms.
            let prec;
            w -= 1;
            buf[w] = b's';
            w -= 1;
            if u == 0 {
                buf[w] = b'0';
                return w;
            } else if u < MICROSECOND.0 as u64 {
                prec = 0;
                buf[w] = b'n';
            } else if u < MILLISECOND.0 as u64 {
                prec = 3;
                // U+00B5 'µ' micro sign == 0xC2 0xB5.
                w -= 1;
                buf[w..w + 2].copy_from_slice("µ".as_bytes());
            } else {
                prec = 6;
                buf[w] = b'm';
            }
            (w, u) = fmt_frac(&mut buf[..w], u, prec);
            w = fmt_int(&mut buf[..w], u);
        } else {
            w -= 1;
            buf[w] = b's';
            (w, u) = fmt_frac(&mut buf[..w], u, 9);
            // u is now whole seconds.
            w = fmt_int(&mut buf[..w], u % 60);
            u /= 60;
            // u is now whole minutes.
            if u > 0 {
                w -= 1;
                buf[w] = b'm';
                w = fmt_int(&mut buf[..w], u % 60);
                u /= 60;
                // u is now whole hours; stop there, days vary in length.
                if u > 0 {
                    w -= 1;
                    buf[w] = b'h';
                    w = fmt_int(&mut buf[..w], u);
                }
            }
        }

        if neg {
            w -= 1;
            buf[w] = b'-';
        }
        w
    }
}

/// `Duration.String`: "72h3m0.5s", "1.2ms", "0s".
impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; 32];
        let n = self.format(&mut buf);
        // Only ASCII and "µ" are written.
        f.write_str(std::str::from_utf8(&buf[n..]).map_err(|_| fmt::Error)?)
    }
}

/// Go marshals a Duration as its integer nanosecond count.
impl serde::Serialize for Duration {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Duration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        i64::deserialize(deserializer).map(Duration)
    }
}

/// Reports whether x+x < y without overflow, for non-negative x and y.
pub(super) fn less_than_half(x: Duration, y: Duration) -> bool {
    (x.0 as u64).wrapping_add(x.0 as u64) < y.0 as u64
}

/// Formats the fraction of v/10^prec (".12345") into the tail of buf, without
/// trailing zeros or a lone decimal point; returns the start index and
/// v/10^prec.
fn fmt_frac(buf: &mut [u8], mut v: u64, prec: usize) -> (usize, u64) {
    let mut w = buf.len();
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            w -= 1;
            buf[w] = digit as u8 + b'0';
        }
        v /= 10;
    }
    if print {
        w -= 1;
        buf[w] = b'.';
    }
    (w, v)
}

/// Formats v into the tail of buf; returns the start index.
fn fmt_int(buf: &mut [u8], mut v: u64) -> usize {
    let mut w = buf.len();
    if v == 0 {
        w -= 1;
        buf[w] = b'0';
    } else {
        while v > 0 {
            w -= 1;
            buf[w] = (v % 10) as u8 + b'0';
            v /= 10;
        }
    }
    w
}

/// An error from [`parse_duration`]: `time: <message> "<value>"`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("time: {message} {}", quote(value))]
pub struct ParseDurationError {
    message: String,
    value: Vec<u8>,
}

fn unit(u: &[u8]) -> Option<u64> {
    Some(match u {
        b"ns" => NANOSECOND.0 as u64,
        b"us" => MICROSECOND.0 as u64,
        b"\xc2\xb5s" => MICROSECOND.0 as u64, // U+00B5 micro sign
        b"\xce\xbcs" => MICROSECOND.0 as u64, // U+03BC Greek mu
        b"ms" => MILLISECOND.0 as u64,
        b"s" => SECOND.0 as u64,
        b"m" => MINUTE.0 as u64,
        b"h" => HOUR.0 as u64,
        _ => return None,
    })
}

/// Go's `time.ParseDuration`: a possibly signed sequence of decimal numbers,
/// each with an optional fraction and a unit ("300ms", "-1.5h", "2h45m").
/// Units are "ns", "us" (or "µs"), "ms", "s", "m", "h". Takes bytes because a
/// Go string can hold any; the error quotes the input as Go does.
pub fn parse_duration(s: impl AsRef<[u8]>) -> Result<Duration, ParseDurationError> {
    // [-+]?([0-9]*(\.[0-9]*)?[a-z]+)+
    let orig = s.as_ref();
    let mut s = orig;
    let err = |message: &str| ParseDurationError {
        message: message.to_owned(),
        value: orig.to_vec(),
    };
    let mut d: u64 = 0;
    let mut neg = false;

    // Consume [-+]?
    if let Some(&c) = s.first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    // Special case: if all that is left is "0", this is zero.
    if s == b"0" {
        return Ok(Duration(0));
    }
    if s.is_empty() {
        return Err(err("invalid duration"));
    }
    while !s.is_empty() {
        let mut f: u64 = 0; // digits after the decimal point
        let mut scale: f64 = 1.0; // value = v + f/scale

        // The next character must be [0-9.].
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(err("invalid duration"));
        }
        // Consume [0-9]*
        let pl = s.len();
        let Ok((mut v, rest)) = leading_int(s) else {
            return Err(err("invalid duration"));
        };
        s = rest;
        let pre = pl != s.len(); // consumed anything before a period

        // Consume (\.[0-9]*)?
        let mut post = false;
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let pl = s.len();
            let rest;
            (f, scale, rest) = leading_fraction(s);
            s = rest;
            post = pl != s.len();
        }
        if !pre && !post {
            // No digits (e.g. ".s" or "-.s").
            return Err(err("invalid duration"));
        }

        // Consume unit.
        let i = s
            .iter()
            .position(|&c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(err("missing unit in duration"));
        }
        let u = &s[..i];
        s = &s[i..];
        let Some(unit) = unit(u) else {
            return Err(err(&format!("unknown unit {} in duration", quote(u))));
        };
        if v > (1u64 << 63) / unit {
            return Err(err("invalid duration")); // overflow
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of
            // hours: v >= 0 && (f*unit/scale) <= 3.6e+12 (ns/h).
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1 << 63 {
                return Err(err("invalid duration")); // overflow
            }
        }
        d += v;
        if d > 1 << 63 {
            return Err(err("invalid duration"));
        }
    }
    if neg {
        return Ok(Duration((d as i64).wrapping_neg()));
    }
    if d > (1 << 63) - 1 {
        return Err(err("invalid duration"));
    }
    Ok(Duration(d as i64))
}
