// Ports: $GOROOT/src/time/format_rfc3339.go and time.go (MarshalJSON,
// UnmarshalJSON, MarshalText, AppendText, UnmarshalText) @ go1.27.0 (the
// toolchain add21b5 builds with)
//
// Derived from Go's time package: Copyright 2022 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! RFC 3339, the most common format: the fast formatting and parsing paths
//! [`Time::format`] and [`super::parse`] take for it, and the JSON and text
//! marshalers, which Gorse's REST bodies go through.

use std::fmt;

use super::format::{ParseError, RFC3339, append_int, append_nano, is_digit, parse_nanoseconds};
use super::{Location, Month, Time, TimeError, date, days_in, fixed_zone, local, parse, utc};

impl Time {
    /// The RFC 3339 formatter (Go's `appendFormatRFC3339`): RFC3339, or
    /// RFC3339Nano with `nanos`.
    #[doc(hidden)]
    pub fn append_format_rfc3339(&self, b: &mut Vec<u8>, nanos: bool) {
        let (_, offset, abs) = self.locabs();

        // Format date.
        let (year, month, day) = super::abs_date(super::abs_days(abs));
        append_int(b, year, 4);
        b.push(b'-');
        append_int(b, month.0, 2);
        b.push(b'-');
        append_int(b, day, 2);

        b.push(b'T');

        // Format time.
        let (hour, min, sec) = super::abs_clock(abs);
        append_int(b, hour, 2);
        b.push(b':');
        append_int(b, min, 2);
        b.push(b':');
        append_int(b, sec, 2);

        if nanos {
            // stdFracSecond(stdFracSecond9, 9, '.')
            append_nano(b, self.nanosecond(), super::format::FRAC_SECOND9_NANOS);
        }

        if offset == 0 {
            b.push(b'Z');
            return;
        }

        // Format zone.
        let mut zone = offset / 60; // convert to minutes
        if zone < 0 {
            b.push(b'-');
            zone = -zone;
        } else {
            b.push(b'+');
        }
        append_int(b, zone / 60, 2);
        b.push(b':');
        append_int(b, zone % 60, 2);
    }

    /// RFC3339Nano, refusing what RFC 3339 cannot express. On error b holds
    /// the formatted text, as in Go.
    fn append_strict_rfc3339(&self, b: &mut Vec<u8>) -> Result<(), TimeError> {
        let n0 = b.len();
        self.append_format_rfc3339(b, true);

        // Not all valid Go timestamps can be serialized as valid RFC 3339.
        // Explicitly check for these edge cases. See
        // https://go.dev/issue/4556 and https://go.dev/issue/54580.
        let num2 = |b: &[u8]| {
            10u8.wrapping_mul(b[0].wrapping_sub(b'0'))
                .wrapping_add(b[1].wrapping_sub(b'0'))
        };
        if b[n0 + "9999".len()] != b'-' {
            // year must be exactly 4 digits wide
            return Err(TimeError("year outside of range [0,9999]".into()));
        }
        if b[b.len() - 1] != b'Z' {
            let c = b[b.len() - "Z07:00".len()];
            if c.is_ascii_digit() || num2(&b[b.len() - "07:00".len()..]) >= 24 {
                return Err(TimeError("timezone hour outside of range [0,23]".into()));
            }
        }
        Ok(())
    }

    /// Go's `MarshalJSON`: the RFC3339Nano time as a quoted JSON string.
    pub fn marshal_json(&self) -> Result<Vec<u8>, TimeError> {
        let mut b = Vec::with_capacity(RFC3339.len() + 12);
        b.push(b'"');
        let r = self.append_strict_rfc3339(&mut b);
        b.push(b'"');
        if let Err(err) = r {
            return Err(TimeError(format!("Time.MarshalJSON: {err}")));
        }
        Ok(b)
    }

    /// Go's `UnmarshalJSON`: `null` leaves t as it is; a JSON string is parsed
    /// as RFC 3339 and t becomes the zero time if that fails. As in Go, the
    /// string is not unescaped (go.dev/issue/47353).
    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), UnmarshalError> {
        if data == b"null" {
            return Ok(());
        }
        if data.len() < 2 || data[0] != b'"' || data[data.len() - 1] != b'"' {
            return Err(UnmarshalError::NotJsonString);
        }
        let data = &data[1..data.len() - 1];
        match parse_strict_rfc3339(data) {
            Ok(t) => {
                *self = t;
                Ok(())
            }
            Err(err) => {
                *self = Time::default();
                Err(UnmarshalError::Parse(err))
            }
        }
    }

    /// Go's `AppendText`: appends the RFC3339Nano text to b; on error b is
    /// left as it was.
    pub fn append_text(&self, b: &mut Vec<u8>) -> Result<(), TimeError> {
        self.append_to(b, "Time.AppendText: ")
    }

    /// Go's `MarshalText`: the RFC3339Nano text.
    pub fn marshal_text(&self) -> Result<Vec<u8>, TimeError> {
        let mut b = Vec::with_capacity(35);
        self.append_to(&mut b, "Time.MarshalText: ")?;
        Ok(b)
    }

    fn append_to(&self, b: &mut Vec<u8>, err_prefix: &str) -> Result<(), TimeError> {
        let n0 = b.len();
        if let Err(err) = self.append_strict_rfc3339(b) {
            b.truncate(n0);
            return Err(TimeError(format!("{err_prefix}{err}")));
        }
        Ok(())
    }

    /// Go's `UnmarshalText`: parses RFC 3339; t becomes the zero time if that
    /// fails.
    pub fn unmarshal_text(&mut self, data: &[u8]) -> Result<(), ParseError> {
        match parse_strict_rfc3339(data) {
            Ok(t) => {
                *self = t;
                Ok(())
            }
            Err(err) => {
                *self = Time::default();
                Err(err)
            }
        }
    }
}

/// Parses s as an unsigned decimal integer and verifies that it is within
/// some range. If it is invalid or out-of-range, it sets ok to false and
/// returns the min value.
fn parse_uint(s: &[u8], min: i64, max: i64, ok: &mut bool) -> i64 {
    let mut x: i64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            *ok = false;
            return min;
        }
        x = x * 10 + i64::from(c - b'0');
    }
    if x < min || max < x {
        *ok = false;
        return min;
    }
    x
}

/// Go's `parseRFC3339`: the RFC 3339 fast path, None where the general
/// parser must decide.
#[doc(hidden)]
pub fn parse_rfc3339(s: &[u8], local: &Location) -> Option<Time> {
    let mut ok = true;

    // Parse the date and time.
    if s.len() < "2006-01-02T15:04:05".len() {
        return None;
    }
    let year = parse_uint(&s[0..4], 0, 9999, &mut ok); // e.g., 2006
    let month = parse_uint(&s[5..7], 1, 12, &mut ok); // e.g., 01
    let day = parse_uint(&s[8..10], 1, days_in(Month(month), year), &mut ok); // e.g., 02
    let hour = parse_uint(&s[11..13], 0, 23, &mut ok); // e.g., 15
    let min = parse_uint(&s[14..16], 0, 59, &mut ok); // e.g., 04
    let sec = parse_uint(&s[17..19], 0, 59, &mut ok); // e.g., 05
    if !ok || !(s[4] == b'-' && s[7] == b'-' && s[10] == b'T' && s[13] == b':' && s[16] == b':') {
        return None;
    }
    let mut s = &s[19..];

    // Parse the fractional second.
    let mut nsec = 0;
    if s.len() >= 2 && s[0] == b'.' && is_digit(s, 1) {
        let mut n = 2;
        while n < s.len() && is_digit(s, n) {
            n += 1;
        }
        // Go ignores the error, which all-digit input cannot produce.
        nsec = parse_nanoseconds(s, n).map_or(0, |(ns, _)| ns);
        s = &s[n..];
    }

    // Parse the time zone.
    let mut t = date(year, Month(month), day, hour, min, sec, nsec, &utc());
    if s.len() != 1 || s[0] != b'Z' {
        if s.len() != "-07:00".len() {
            return None;
        }
        let hr = parse_uint(&s[1..3], 0, 23, &mut ok); // e.g., 07
        let mm = parse_uint(&s[4..6], 0, 59, &mut ok); // e.g., 00
        if !ok || !((s[0] == b'-' || s[0] == b'+') && s[3] == b':') {
            return None;
        }
        let mut zone_offset = (hr * 60 + mm) * 60;
        if s[0] == b'-' {
            zone_offset *= -1;
        }
        t.add_sec(-zone_offset);

        // Use local zone with the given offset if possible.
        if local.lookup(t.unix_sec()).offset == zone_offset {
            t.set_loc(local);
        } else {
            t.set_loc(&fixed_zone("", zone_offset));
        }
    }
    Some(t)
}

/// RFC 3339 as `UnmarshalJSON` and `UnmarshalText` read it: the fast path,
/// else `Parse(RFC3339, …)`. Go's stricter checks after that are disabled
/// (go.dev/issue/54580), so they are not ported.
fn parse_strict_rfc3339(b: &[u8]) -> Result<Time, ParseError> {
    match parse_rfc3339(b, &local()) {
        Some(t) => Ok(t),
        None => parse(RFC3339, b),
    }
}

/// An error from [`Time::unmarshal_json`], with Go's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnmarshalError {
    /// The JSON value is not a string (nor `null`).
    #[error("Time.UnmarshalJSON: input is not a JSON string")]
    NotJsonString,
    /// The string is not an RFC 3339 time.
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// Go's `MarshalJSON`: an RFC3339Nano string.
impl serde::Serialize for Time {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut b = Vec::with_capacity(35);
        if let Err(err) = self.append_strict_rfc3339(&mut b) {
            return Err(serde::ser::Error::custom(format!(
                "Time.MarshalJSON: {err}"
            )));
        }
        // The formatter writes ASCII only.
        serializer.serialize_str(&String::from_utf8_lossy(&b))
    }
}

/// Go's `UnmarshalJSON`: an RFC 3339 string, or `null` for the zero time.
/// Unlike Go, which reads the raw JSON bytes, the string arrives unescaped.
impl<'de> serde::Deserialize<'de> for Time {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TimeVisitor)
    }
}

struct TimeVisitor;

impl TimeVisitor {
    fn not_string<E: serde::de::Error>() -> E {
        E::custom(UnmarshalError::NotJsonString)
    }
}

impl<'de> serde::de::Visitor<'de> for TimeVisitor {
    type Value = Time;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an RFC 3339 time string or null")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Time, E> {
        parse_strict_rfc3339(v.as_bytes()).map_err(E::custom)
    }

    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Time, E> {
        parse_strict_rfc3339(v).map_err(E::custom)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Time, E> {
        Ok(Time::default())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Time, E> {
        Ok(Time::default())
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Time, D::Error> {
        d.deserialize_any(TimeVisitor)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Time, E> {
        Err(Self::not_string())
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Time, E> {
        Err(Self::not_string())
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Time, E> {
        Err(Self::not_string())
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Time, E> {
        Err(Self::not_string())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, _: A) -> Result<Time, A::Error> {
        Err(Self::not_string())
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, _: A) -> Result<Time, A::Error> {
        Err(Self::not_string())
    }
}
