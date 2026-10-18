// Ports: $GOROOT/src/time/format.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package: Copyright 2010 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's layout-based formatting and parsing: `Time.Format`, `Time.String`,
//! `time.Parse`, `time.ParseInLocation` and `ParseError`.
//!
//! Layouts and values are byte strings, as Go strings are: `dateparse` builds
//! layouts byte by byte and may cut through a multi-byte character, and the
//! error texts escape such bytes the way Go's `quote` does.

use std::fmt;

use super::{Location, Month, Time, date, days_before, days_in, fixed_zone, is_leap, local, utc};

/// The reference time, in numerical order.
pub const LAYOUT: &str = "01/02 03:04:05PM '06 -0700";
/// `time.ANSIC`.
pub const ANSIC: &str = "Mon Jan _2 15:04:05 2006";
/// `time.UnixDate`.
pub const UNIX_DATE: &str = "Mon Jan _2 15:04:05 MST 2006";
/// `time.RubyDate`.
pub const RUBY_DATE: &str = "Mon Jan 02 15:04:05 -0700 2006";
/// `time.RFC822`.
pub const RFC822: &str = "02 Jan 06 15:04 MST";
/// `time.RFC822Z`: RFC822 with a numeric zone.
pub const RFC822Z: &str = "02 Jan 06 15:04 -0700";
/// `time.RFC850`.
pub const RFC850: &str = "Monday, 02-Jan-06 15:04:05 MST";
/// `time.RFC1123`.
pub const RFC1123: &str = "Mon, 02 Jan 2006 15:04:05 MST";
/// `time.RFC1123Z`: RFC1123 with a numeric zone.
pub const RFC1123Z: &str = "Mon, 02 Jan 2006 15:04:05 -0700";
/// `time.RFC3339`.
pub const RFC3339: &str = "2006-01-02T15:04:05Z07:00";
/// `time.RFC3339Nano`.
pub const RFC3339_NANO: &str = "2006-01-02T15:04:05.999999999Z07:00";
/// `time.Kitchen`.
pub const KITCHEN: &str = "3:04PM";
/// `time.Stamp`.
pub const STAMP: &str = "Jan _2 15:04:05";
/// `time.StampMilli`.
pub const STAMP_MILLI: &str = "Jan _2 15:04:05.000";
/// `time.StampMicro`.
pub const STAMP_MICRO: &str = "Jan _2 15:04:05.000000";
/// `time.StampNano`.
pub const STAMP_NANO: &str = "Jan _2 15:04:05.000000000";
/// `time.DateTime`.
pub const DATE_TIME: &str = "2006-01-02 15:04:05";
/// `time.DateOnly`.
pub const DATE_ONLY: &str = "2006-01-02";
/// `time.TimeOnly`.
pub const TIME_ONLY: &str = "15:04:05";

const STD_NEED_DATE: i64 = 1 << 8; // need month, day, year
const STD_NEED_YDAY: i64 = 1 << 9; // need yday
const STD_NEED_CLOCK: i64 = 1 << 10; // need hour, minute, second
const STD_ARG_SHIFT: i64 = 16; // extra argument in high bits, above low STD_ARG_SHIFT
const STD_SEPARATOR_SHIFT: i64 = 28; // extra argument in high 4 bits for fractional second separators
const STD_MASK: i64 = (1 << STD_ARG_SHIFT) - 1; // mask out argument

const STD_LONG_MONTH: i64 = 1 + STD_NEED_DATE; // "January"
const STD_MONTH: i64 = 2 + STD_NEED_DATE; // "Jan"
const STD_NUM_MONTH: i64 = 3 + STD_NEED_DATE; // "1"
const STD_ZERO_MONTH: i64 = 4 + STD_NEED_DATE; // "01"
const STD_LONG_WEEK_DAY: i64 = 5 + STD_NEED_DATE; // "Monday"
const STD_WEEK_DAY: i64 = 6 + STD_NEED_DATE; // "Mon"
const STD_DAY: i64 = 7 + STD_NEED_DATE; // "2"
const STD_UNDER_DAY: i64 = 8 + STD_NEED_DATE; // "_2"
const STD_ZERO_DAY: i64 = 9 + STD_NEED_DATE; // "02"
const STD_UNDER_YEAR_DAY: i64 = 10 + STD_NEED_YDAY; // "__2"
const STD_ZERO_YEAR_DAY: i64 = 11 + STD_NEED_YDAY; // "002"
const STD_HOUR: i64 = 12 + STD_NEED_CLOCK; // "15"
const STD_HOUR12: i64 = 13 + STD_NEED_CLOCK; // "3"
const STD_ZERO_HOUR12: i64 = 14 + STD_NEED_CLOCK; // "03"
const STD_MINUTE: i64 = 15 + STD_NEED_CLOCK; // "4"
const STD_ZERO_MINUTE: i64 = 16 + STD_NEED_CLOCK; // "04"
const STD_SECOND: i64 = 17 + STD_NEED_CLOCK; // "5"
const STD_ZERO_SECOND: i64 = 18 + STD_NEED_CLOCK; // "05"
const STD_LONG_YEAR: i64 = 19 + STD_NEED_DATE; // "2006"
const STD_YEAR: i64 = 20 + STD_NEED_DATE; // "06"
const STD_PM: i64 = 21 + STD_NEED_CLOCK; // "PM"
const STD_PM_LOWER: i64 = 22 + STD_NEED_CLOCK; // "pm"
const STD_TZ: i64 = 23; // "MST"
const STD_ISO8601_TZ: i64 = 24; // "Z0700"  // prints Z for UTC
const STD_ISO8601_SECONDS_TZ: i64 = 25; // "Z070000"
const STD_ISO8601_SHORT_TZ: i64 = 26; // "Z07"
const STD_ISO8601_COLON_TZ: i64 = 27; // "Z07:00" // prints Z for UTC
const STD_ISO8601_COLON_SECONDS_TZ: i64 = 28; // "Z07:00:00"
const STD_NUM_TZ: i64 = 29; // "-0700"  // always numeric
const STD_NUM_SECONDS_TZ: i64 = 30; // "-070000"
const STD_NUM_SHORT_TZ: i64 = 31; // "-07"    // always numeric
const STD_NUM_COLON_TZ: i64 = 32; // "-07:00" // always numeric
const STD_NUM_COLON_SECONDS_TZ: i64 = 33; // "-07:00:00"
const STD_FRAC_SECOND0: i64 = 34; // ".0", ".00", ... , trailing zeros included
const STD_FRAC_SECOND9: i64 = 35; // ".9", ".99", ..., trailing zeros omitted

/// `stdFracSecond(stdFracSecond9, 9, '.')`, the fraction RFC3339Nano writes.
pub(crate) const FRAC_SECOND9_NANOS: i64 = STD_FRAC_SECOND9 | 9 << STD_ARG_SHIFT;

// The std values for "01", "02", ..., "06".
const STD0X: [i64; 6] = [
    STD_ZERO_MONTH,
    STD_ZERO_DAY,
    STD_ZERO_HOUR12,
    STD_ZERO_MINUTE,
    STD_ZERO_SECOND,
    STD_YEAR,
];

/// The layout element a `next_std_chunk` code stands for (Go's
/// `StdChunkNames` from export_test.go), or `None` for an unknown code.
pub fn std_chunk_name(std: i64) -> Option<String> {
    let name = match std {
        0 => "",
        STD_LONG_MONTH => "January",
        STD_MONTH => "Jan",
        STD_NUM_MONTH => "1",
        STD_ZERO_MONTH => "01",
        STD_LONG_WEEK_DAY => "Monday",
        STD_WEEK_DAY => "Mon",
        STD_DAY => "2",
        STD_UNDER_DAY => "_2",
        STD_ZERO_DAY => "02",
        STD_UNDER_YEAR_DAY => "__2",
        STD_ZERO_YEAR_DAY => "002",
        STD_HOUR => "15",
        STD_HOUR12 => "3",
        STD_ZERO_HOUR12 => "03",
        STD_MINUTE => "4",
        STD_ZERO_MINUTE => "04",
        STD_SECOND => "5",
        STD_ZERO_SECOND => "05",
        STD_LONG_YEAR => "2006",
        STD_YEAR => "06",
        STD_PM => "PM",
        STD_PM_LOWER => "pm",
        STD_TZ => "MST",
        STD_ISO8601_TZ => "Z0700",
        STD_ISO8601_SECONDS_TZ => "Z070000",
        STD_ISO8601_SHORT_TZ => "Z07",
        STD_ISO8601_COLON_TZ => "Z07:00",
        STD_ISO8601_COLON_SECONDS_TZ => "Z07:00:00",
        STD_NUM_TZ => "-0700",
        STD_NUM_SECONDS_TZ => "-070000",
        STD_NUM_SHORT_TZ => "-07",
        STD_NUM_COLON_TZ => "-07:00",
        STD_NUM_COLON_SECONDS_TZ => "-07:00:00",
        _ => {
            let code = std & STD_MASK;
            let n = digits_len(std);
            if (code != STD_FRAC_SECOND0 && code != STD_FRAC_SECOND9)
                || !(1..=9).contains(&n)
                || std >> STD_SEPARATOR_SHIFT != 0
            {
                return None;
            }
            let digit = if code == STD_FRAC_SECOND0 { "0" } else { "9" };
            return Some(format!(".{}", digit.repeat(n as usize)));
        }
    };
    Some(name.to_owned())
}

/// Reports whether s starts with a lower-case ASCII letter; keeps "Month"
/// from matching "Mon".
fn starts_with_lower_case(s: &[u8]) -> bool {
    s.first().is_some_and(u8::is_ascii_lowercase)
}

/// Finds the first std element in layout and returns the text before it, its
/// code, and the text after it.
pub fn next_std_chunk(layout: &[u8]) -> (&[u8], i64, &[u8]) {
    let n = layout.len();
    let at = |i: usize, s: &[u8]| n >= i + s.len() && &layout[i..i + s.len()] == s;
    for i in 0..n {
        match layout[i] {
            b'J' => {
                // January, Jan
                if at(i, b"Jan") {
                    if at(i, b"January") {
                        return (&layout[..i], STD_LONG_MONTH, &layout[i + 7..]);
                    }
                    if !starts_with_lower_case(&layout[i + 3..]) {
                        return (&layout[..i], STD_MONTH, &layout[i + 3..]);
                    }
                }
            }
            b'M' => {
                // Monday, Mon, MST
                if n >= i + 3 {
                    if &layout[i..i + 3] == b"Mon" {
                        if at(i, b"Monday") {
                            return (&layout[..i], STD_LONG_WEEK_DAY, &layout[i + 6..]);
                        }
                        if !starts_with_lower_case(&layout[i + 3..]) {
                            return (&layout[..i], STD_WEEK_DAY, &layout[i + 3..]);
                        }
                    }
                    if &layout[i..i + 3] == b"MST" {
                        return (&layout[..i], STD_TZ, &layout[i + 3..]);
                    }
                }
            }
            b'0' => {
                // 01, 02, 03, 04, 05, 06, 002
                if n >= i + 2 && (b'1'..=b'6').contains(&layout[i + 1]) {
                    return (
                        &layout[..i],
                        STD0X[(layout[i + 1] - b'1') as usize],
                        &layout[i + 2..],
                    );
                }
                if n >= i + 3 && layout[i + 1] == b'0' && layout[i + 2] == b'2' {
                    return (&layout[..i], STD_ZERO_YEAR_DAY, &layout[i + 3..]);
                }
            }
            b'1' => {
                // 15, 1
                if n >= i + 2 && layout[i + 1] == b'5' {
                    return (&layout[..i], STD_HOUR, &layout[i + 2..]);
                }
                return (&layout[..i], STD_NUM_MONTH, &layout[i + 1..]);
            }
            b'2' => {
                // 2006, 2
                if at(i, b"2006") {
                    return (&layout[..i], STD_LONG_YEAR, &layout[i + 4..]);
                }
                return (&layout[..i], STD_DAY, &layout[i + 1..]);
            }
            b'_' => {
                // _2, _2006, __2
                if n >= i + 2 && layout[i + 1] == b'2' {
                    // _2006 is really a literal _, followed by stdLongYear.
                    if n >= i + 5 && &layout[i + 1..i + 5] == b"2006" {
                        return (&layout[..i + 1], STD_LONG_YEAR, &layout[i + 5..]);
                    }
                    return (&layout[..i], STD_UNDER_DAY, &layout[i + 2..]);
                }
                if n >= i + 3 && layout[i + 1] == b'_' && layout[i + 2] == b'2' {
                    return (&layout[..i], STD_UNDER_YEAR_DAY, &layout[i + 3..]);
                }
            }
            b'3' => return (&layout[..i], STD_HOUR12, &layout[i + 1..]),
            b'4' => return (&layout[..i], STD_MINUTE, &layout[i + 1..]),
            b'5' => return (&layout[..i], STD_SECOND, &layout[i + 1..]),
            b'P' => {
                // PM
                if n >= i + 2 && layout[i + 1] == b'M' {
                    return (&layout[..i], STD_PM, &layout[i + 2..]);
                }
            }
            b'p' => {
                // pm
                if n >= i + 2 && layout[i + 1] == b'm' {
                    return (&layout[..i], STD_PM_LOWER, &layout[i + 2..]);
                }
            }
            b'-' => {
                // -070000, -07:00:00, -0700, -07:00, -07
                if at(i, b"-070000") {
                    return (&layout[..i], STD_NUM_SECONDS_TZ, &layout[i + 7..]);
                }
                if at(i, b"-07:00:00") {
                    return (&layout[..i], STD_NUM_COLON_SECONDS_TZ, &layout[i + 9..]);
                }
                if at(i, b"-0700") {
                    return (&layout[..i], STD_NUM_TZ, &layout[i + 5..]);
                }
                if at(i, b"-07:00") {
                    return (&layout[..i], STD_NUM_COLON_TZ, &layout[i + 6..]);
                }
                if at(i, b"-07") {
                    return (&layout[..i], STD_NUM_SHORT_TZ, &layout[i + 3..]);
                }
            }
            b'Z' => {
                // Z070000, Z07:00:00, Z0700, Z07:00,
                if at(i, b"Z070000") {
                    return (&layout[..i], STD_ISO8601_SECONDS_TZ, &layout[i + 7..]);
                }
                if at(i, b"Z07:00:00") {
                    return (&layout[..i], STD_ISO8601_COLON_SECONDS_TZ, &layout[i + 9..]);
                }
                if at(i, b"Z0700") {
                    return (&layout[..i], STD_ISO8601_TZ, &layout[i + 5..]);
                }
                if at(i, b"Z07:00") {
                    return (&layout[..i], STD_ISO8601_COLON_TZ, &layout[i + 6..]);
                }
                if at(i, b"Z07") {
                    return (&layout[..i], STD_ISO8601_SHORT_TZ, &layout[i + 3..]);
                }
            }
            // ,000, or .000, or ,999, or .999 - repeated digits for fractional
            // seconds.
            c @ (b'.' | b',') if i + 1 < n && (layout[i + 1] == b'0' || layout[i + 1] == b'9') => {
                let ch = layout[i + 1];
                let mut j = i + 1;
                while j < n && layout[j] == ch {
                    j += 1;
                }
                // String of digits must end here - only fractional second is
                // all digits.
                if !is_digit(layout, j) {
                    let code = if layout[i + 1] == b'9' {
                        STD_FRAC_SECOND9
                    } else {
                        STD_FRAC_SECOND0
                    };
                    let std = std_frac_second(code, (j - (i + 1)) as i64, c);
                    return (&layout[..i], std, &layout[j..]);
                }
            }
            _ => {}
        }
    }
    (layout, 0, &[])
}

pub(crate) const LONG_DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const SHORT_DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const SHORT_MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(crate) const LONG_MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Reports whether s1 and s2 (of equal length) match ignoring ASCII case.
fn match_fold(s1: &[u8], s2: &[u8]) -> bool {
    s1.iter().zip(s2).all(|(&c1, &c2)| {
        if c1 == c2 {
            return true;
        }
        // Switch to lower-case; 'a'-'A' is known to be a single bit.
        let c1 = c1 | (b'a' - b'A');
        let c2 = c2 | (b'a' - b'A');
        c1 == c2 && c1.is_ascii_lowercase()
    })
}

fn lookup<'a>(tab: &[&str], val: &'a [u8]) -> Result<(i64, &'a [u8]), ()> {
    for (i, v) in tab.iter().enumerate() {
        let v = v.as_bytes();
        if val.len() >= v.len() && match_fold(&val[..v.len()], v) {
            return Ok((i as i64, &val[v.len()..]));
        }
    }
    Err(())
}

/// Appends the decimal form of x to b, zero-padded to `width` digits
/// (excluding the sign).
pub fn append_int(b: &mut Vec<u8>, x: i64, width: i64) {
    let mut u = x as u64;
    if x < 0 {
        b.push(b'-');
        u = x.wrapping_neg() as u64;
    }

    // 2-digit and 4-digit fields are the most common in time formats.
    let utod = |u: u64| b'0' + u as u8;
    match width {
        2 if u < 100 => {
            b.extend_from_slice(&[utod(u / 10), utod(u % 10)]);
            return;
        }
        4 if u < 10_000 => {
            b.extend_from_slice(&[
                utod(u / 1000),
                utod(u / 100 % 10),
                utod(u / 10 % 10),
                utod(u % 10),
            ]);
            return;
        }
        _ => {}
    }

    // Compute the number of decimal digits.
    let mut n: i64 = i64::from(u == 0);
    let mut u2 = u;
    while u2 > 0 {
        n += 1;
        u2 /= 10;
    }

    // Add 0-padding.
    let mut pad = width - n;
    while pad > 0 {
        b.push(b'0');
        pad -= 1;
    }

    // Assemble the decimal in reverse order.
    let start = b.len();
    b.resize(start + n as usize, 0);
    let mut i = b.len() - 1;
    while u >= 10 && i > 0 {
        let q = u / 10;
        b[i] = utod(u - q * 10);
        u = q;
        i -= 1;
    }
    b[i] = utod(u);
}

/// Go's `atoi`: an optionally signed decimal integer, the whole of s.
pub(crate) fn atoi(s: &[u8]) -> Result<i64, ()> {
    let mut s = s;
    let mut neg = false;
    if let Some(&c) = s.first()
        && (c == b'-' || c == b'+')
    {
        neg = c == b'-';
        s = &s[1..];
    }
    let (q, rem) = leading_int(s)?;
    if !rem.is_empty() {
        return Err(());
    }
    let x = q as i64;
    Ok(if neg { x.wrapping_neg() } else { x })
}

// The std value for a fractional second packs the number of digits and the
// separator (period or comma).
fn std_frac_second(code: i64, n: i64, c: u8) -> i64 {
    // Use 0xfff to make the failure case even more absurd.
    if c == b'.' {
        return code | ((n & 0xfff) << STD_ARG_SHIFT);
    }
    code | ((n & 0xfff) << STD_ARG_SHIFT) | (1 << STD_SEPARATOR_SHIFT)
}

fn digits_len(std: i64) -> i64 {
    (std >> STD_ARG_SHIFT) & 0xfff
}

fn separator(std: i64) -> u8 {
    if std >> STD_SEPARATOR_SHIFT == 0 {
        b'.'
    } else {
        b','
    }
}

/// Appends a fractional second (nanosec in [0, 999999999]) to b.
pub(crate) fn append_nano(b: &mut Vec<u8>, nanosec: i64, std: i64) {
    let trim = std & STD_MASK == STD_FRAC_SECOND9;
    let n = digits_len(std);
    if trim && (n == 0 || nanosec == 0) {
        return;
    }
    let dot = separator(std);
    b.push(dot);
    append_int(b, nanosec, 9);
    if n < 9 {
        b.truncate(b.len() - 9 + n as usize);
    }
    if trim {
        while b.last() == Some(&b'0') {
            b.pop();
        }
        if b.last() == Some(&dot) {
            b.pop();
        }
    }
}

impl Time {
    /// `Time.String`: the time formatted as
    /// `2006-01-02 15:04:05.999999999 -0700 MST`, followed by ` m=±<seconds>`
    /// when t has a monotonic clock reading. This is what Gorse's cache stores.
    pub fn string(&self) -> String {
        let mut s = self.format("2006-01-02 15:04:05.999999999 -0700 MST");

        // Format the monotonic clock reading as m=±ddd.nnnnnnnnn.
        if self.wall & super::HAS_MONOTONIC != 0 {
            let mut m2 = self.ext as u64;
            let mut sign = b'+';
            if self.ext < 0 {
                sign = b'-';
                m2 = m2.wrapping_neg();
            }
            let (m1, m2) = (m2 / 1_000_000_000, m2 % 1_000_000_000);
            let (m0, m1) = (m1 / 1_000_000_000, m1 % 1_000_000_000);
            let mut buf = Vec::with_capacity(24);
            buf.extend_from_slice(b" m=");
            buf.push(sign);
            let mut wid = 0;
            if m0 != 0 {
                append_int(&mut buf, m0 as i64, 0);
                wid = 9;
            }
            append_int(&mut buf, m1 as i64, wid);
            buf.push(b'.');
            append_int(&mut buf, m2 as i64, 9);
            s.push_str(&String::from_utf8_lossy(&buf));
        }
        s
    }

    /// `Time.GoString`: t as Go source (`time.Date(...)`).
    pub fn go_string(&self) -> String {
        let abs = self.abs_sec();
        let (year, month, day) = super::abs_date(super::abs_days(abs));
        let (hour, minute, second) = super::abs_clock(abs);

        let mut buf = Vec::with_capacity(70);
        buf.extend_from_slice(b"time.Date(");
        append_int(&mut buf, year, 0);
        if (1..=12).contains(&month.0) {
            buf.extend_from_slice(b", time.");
            buf.extend_from_slice(LONG_MONTH_NAMES[(month.0 - 1) as usize].as_bytes());
        } else {
            // Go appends the number without a separator here.
            append_int(&mut buf, month.0, 0);
        }
        buf.extend_from_slice(b", ");
        append_int(&mut buf, day, 0);
        buf.extend_from_slice(b", ");
        append_int(&mut buf, hour, 0);
        buf.extend_from_slice(b", ");
        append_int(&mut buf, minute, 0);
        buf.extend_from_slice(b", ");
        append_int(&mut buf, second, 0);
        buf.extend_from_slice(b", ");
        append_int(&mut buf, self.nanosecond(), 0);
        buf.extend_from_slice(b", ");
        let loc = self.location();
        if loc.is_utc() {
            buf.extend_from_slice(b"time.UTC");
        } else if loc == local() {
            buf.extend_from_slice(b"time.Local");
        } else {
            buf.extend_from_slice(b"time.Location(");
            buf.extend_from_slice(quote(loc.name().as_bytes()).as_bytes());
            buf.push(b')');
        }
        buf.push(b')');
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// `Time.Format`: t formatted by the layout (see [`LAYOUT`]).
    pub fn format(&self, layout: &str) -> String {
        let mut b = Vec::with_capacity(layout.len() + 10);
        self.append_format(&mut b, layout.as_bytes());
        // Literal layout text is copied whole and every element writes ASCII
        // or a zone name, so the bytes are UTF-8 for a UTF-8 layout.
        String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// `Time.AppendFormat`: appends t formatted by the layout to b.
    pub fn append_format(&self, b: &mut Vec<u8>, layout: &[u8]) {
        // Optimize for RFC3339 as it accounts for over half of all
        // representations.
        if layout == RFC3339.as_bytes() {
            self.append_format_rfc3339(b, false);
        } else if layout == RFC3339_NANO.as_bytes() {
            self.append_format_rfc3339(b, true);
        } else {
            self.append_format_any(b, layout);
        }
    }

    /// The general formatter, without the RFC 3339 fast path (Go's
    /// `appendFormat`).
    #[doc(hidden)]
    pub fn append_format_any(&self, b: &mut Vec<u8>, layout: &[u8]) {
        let (name, offset, abs) = self.locabs();
        let days = super::abs_days(abs);

        let mut year: i64 = -1;
        let mut month = Month(0);
        let mut day: i64 = 0;
        let mut yday: i64 = -1;
        let mut hour: i64 = -1;
        let mut min: i64 = 0;
        let mut sec: i64 = 0;

        // Each iteration generates one std value.
        let mut layout = layout;
        while !layout.is_empty() {
            let (prefix, std, suffix) = next_std_chunk(layout);
            b.extend_from_slice(prefix);
            if std == 0 {
                break;
            }
            layout = suffix;

            // Compute year, month, day if needed.
            if year < 0 && std & STD_NEED_DATE != 0 {
                (year, month, day) = super::abs_date(days);
            }
            if yday < 0 && std & STD_NEED_YDAY != 0 {
                (_, yday) = super::abs_year_yday(days);
            }
            // Compute hour, minute, second if needed.
            if hour < 0 && std & STD_NEED_CLOCK != 0 {
                (hour, min, sec) = super::abs_clock(abs);
            }

            match std & STD_MASK {
                STD_YEAR => append_int(b, year.wrapping_abs() % 100, 2),
                STD_LONG_YEAR => append_int(b, year, 4),
                STD_MONTH => b.extend_from_slice(&month.to_string().as_bytes()[..3]),
                STD_LONG_MONTH => b.extend_from_slice(month.to_string().as_bytes()),
                STD_NUM_MONTH => append_int(b, month.0, 0),
                STD_ZERO_MONTH => append_int(b, month.0, 2),
                STD_WEEK_DAY => {
                    b.extend_from_slice(&super::abs_weekday(days).to_string().as_bytes()[..3]);
                }
                STD_LONG_WEEK_DAY => {
                    b.extend_from_slice(super::abs_weekday(days).to_string().as_bytes());
                }
                STD_DAY => append_int(b, day, 0),
                STD_UNDER_DAY => {
                    if day < 10 {
                        b.push(b' ');
                    }
                    append_int(b, day, 0);
                }
                STD_ZERO_DAY => append_int(b, day, 2),
                STD_UNDER_YEAR_DAY => {
                    if yday < 100 {
                        b.push(b' ');
                        if yday < 10 {
                            b.push(b' ');
                        }
                    }
                    append_int(b, yday, 0);
                }
                STD_ZERO_YEAR_DAY => append_int(b, yday, 3),
                STD_HOUR => append_int(b, hour, 2),
                STD_HOUR12 | STD_ZERO_HOUR12 => {
                    // Noon is 12PM, midnight is 12AM.
                    let mut hr = hour % 12;
                    if hr == 0 {
                        hr = 12;
                    }
                    append_int(b, hr, if std == STD_ZERO_HOUR12 { 2 } else { 0 });
                }
                STD_MINUTE => append_int(b, min, 0),
                STD_ZERO_MINUTE => append_int(b, min, 2),
                STD_SECOND => append_int(b, sec, 0),
                STD_ZERO_SECOND => append_int(b, sec, 2),
                STD_PM => b.extend_from_slice(if hour >= 12 { b"PM" } else { b"AM" }),
                STD_PM_LOWER => b.extend_from_slice(if hour >= 12 { b"pm" } else { b"am" }),
                STD_ISO8601_TZ
                | STD_ISO8601_COLON_TZ
                | STD_ISO8601_SECONDS_TZ
                | STD_ISO8601_SHORT_TZ
                | STD_ISO8601_COLON_SECONDS_TZ
                | STD_NUM_TZ
                | STD_NUM_COLON_TZ
                | STD_NUM_SECONDS_TZ
                | STD_NUM_SHORT_TZ
                | STD_NUM_COLON_SECONDS_TZ => {
                    // Ugly special case. We cheat and take the "Z" variants to
                    // mean "the time zone as formatted for ISO 8601".
                    if offset == 0
                        && matches!(
                            std,
                            STD_ISO8601_TZ
                                | STD_ISO8601_COLON_TZ
                                | STD_ISO8601_SECONDS_TZ
                                | STD_ISO8601_SHORT_TZ
                                | STD_ISO8601_COLON_SECONDS_TZ
                        )
                    {
                        b.push(b'Z');
                        continue;
                    }
                    let mut zone = offset / 60; // convert to minutes
                    let mut absoffset = offset;
                    if zone < 0 {
                        b.push(b'-');
                        zone = -zone;
                        absoffset = -absoffset;
                    } else {
                        b.push(b'+');
                    }
                    append_int(b, zone / 60, 2);
                    if matches!(
                        std,
                        STD_ISO8601_COLON_TZ
                            | STD_NUM_COLON_TZ
                            | STD_ISO8601_COLON_SECONDS_TZ
                            | STD_NUM_COLON_SECONDS_TZ
                    ) {
                        b.push(b':');
                    }
                    if std != STD_NUM_SHORT_TZ && std != STD_ISO8601_SHORT_TZ {
                        append_int(b, zone % 60, 2);
                    }
                    // Append seconds if appropriate.
                    if matches!(
                        std,
                        STD_ISO8601_SECONDS_TZ
                            | STD_NUM_SECONDS_TZ
                            | STD_NUM_COLON_SECONDS_TZ
                            | STD_ISO8601_COLON_SECONDS_TZ
                    ) {
                        if std == STD_NUM_COLON_SECONDS_TZ || std == STD_ISO8601_COLON_SECONDS_TZ {
                            b.push(b':');
                        }
                        append_int(b, absoffset % 60, 2);
                    }
                }
                STD_TZ => {
                    if !name.is_empty() {
                        b.extend_from_slice(name.as_bytes());
                        continue;
                    }
                    // No time zone known for this time, but we must print one.
                    // Use the -0700 format.
                    let mut zone = offset / 60; // convert to minutes
                    if zone < 0 {
                        b.push(b'-');
                        zone = -zone;
                    } else {
                        b.push(b'+');
                    }
                    append_int(b, zone / 60, 2);
                    append_int(b, zone % 60, 2);
                }
                STD_FRAC_SECOND0 | STD_FRAC_SECOND9 => {
                    append_nano(b, self.nanosecond(), std);
                }
                _ => {}
            }
        }
    }
}

/// A problem parsing a time string, with Go's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    layout: Vec<u8>,
    value: Vec<u8>,
    layout_elem: Vec<u8>,
    value_elem: Vec<u8>,
    message: String,
}

impl ParseError {
    pub(crate) fn new(
        layout: &[u8],
        value: &[u8],
        layout_elem: &[u8],
        value_elem: &[u8],
        message: String,
    ) -> ParseError {
        ParseError {
            layout: layout.to_vec(),
            value: value.to_vec(),
            layout_elem: layout_elem.to_vec(),
            value_elem: value_elem.to_vec(),
            message,
        }
    }

    /// The layout being parsed with.
    pub fn layout(&self) -> &[u8] {
        &self.layout
    }

    /// The value being parsed.
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// The layout element that failed.
    pub fn layout_elem(&self) -> &[u8] {
        &self.layout_elem
    }

    /// The rest of the value where it failed.
    pub fn value_elem(&self) -> &[u8] {
        &self.value_elem
    }

    /// The extra message (": day out of range", ...), empty for a mismatch.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Go's `quote`: a double-quoted string with `"` and `\` escaped and every
/// control or non-ASCII byte written as `\xHH` (not `strconv.Quote`).
pub fn quote(s: &[u8]) -> String {
    let mut buf = String::with_capacity(s.len() + 2);
    buf.push('"');
    let mut i = 0;
    while i < s.len() {
        let (c, size) = super::decode_rune(&s[i..]);
        if c >= 0x80 || c < u32::from(b' ') {
            // A RuneError is one invalid byte, or a literal U+FFFD (3 bytes).
            let width = if c == 0xFFFD {
                if i + 2 < s.len() && &s[i..i + 3] == "\u{FFFD}".as_bytes() {
                    3
                } else {
                    1
                }
            } else {
                size
            };
            for &byte in &s[i..i + width] {
                buf.push_str(&format!("\\x{byte:02x}"));
            }
        } else {
            if c == u32::from(b'"') || c == u32::from(b'\\') {
                buf.push('\\');
            }
            buf.push(char::from(c as u8));
        }
        i += size;
    }
    buf.push('"');
    buf
}

/// `ParseError.Error`.
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            return write!(
                f,
                "parsing time {} as {}: cannot parse {} as {}",
                quote(&self.value),
                quote(&self.layout),
                quote(&self.value_elem),
                quote(&self.layout_elem)
            );
        }
        write!(f, "parsing time {}{}", quote(&self.value), self.message)
    }
}

impl std::error::Error for ParseError {}

/// Reports whether s[i] is in range and is a decimal digit.
pub(crate) fn is_digit(s: &[u8], i: usize) -> bool {
    s.get(i).is_some_and(u8::is_ascii_digit)
}

/// Parses s[0:1] or s[0:2] (fixed forces s[0:2]) as a decimal integer.
fn getnum(s: &[u8], fixed: bool) -> Result<(i64, &[u8]), ()> {
    if !is_digit(s, 0) {
        return Err(());
    }
    if !is_digit(s, 1) {
        if fixed {
            return Err(());
        }
        return Ok((i64::from(s[0] - b'0'), &s[1..]));
    }
    Ok((
        i64::from(s[0] - b'0') * 10 + i64::from(s[1] - b'0'),
        &s[2..],
    ))
}

/// Parses s[0:1], s[0:2], or s[0:3] (fixed forces s[0:3]) as a decimal
/// integer.
fn getnum3(s: &[u8], fixed: bool) -> Result<(i64, &[u8]), ()> {
    let mut n = 0;
    let mut i = 0;
    while i < 3 && is_digit(s, i) {
        n = n * 10 + i64::from(s[i] - b'0');
        i += 1;
    }
    if i == 0 || fixed && i != 3 {
        return Err(());
    }
    Ok((n, &s[i..]))
}

fn cutspace(mut s: &[u8]) -> &[u8] {
    while s.first() == Some(&b' ') {
        s = &s[1..];
    }
    s
}

/// Removes prefix from value, treating runs of spaces as equivalent.
fn skip<'a>(mut value: &'a [u8], mut prefix: &[u8]) -> Result<&'a [u8], &'a [u8]> {
    while !prefix.is_empty() {
        if prefix[0] == b' ' {
            if !value.is_empty() && value[0] != b' ' {
                return Err(value);
            }
            prefix = cutspace(prefix);
            value = cutspace(value);
            continue;
        }
        if value.is_empty() || value[0] != prefix[0] {
            return Err(value);
        }
        prefix = &prefix[1..];
        value = &value[1..];
    }
    Ok(value)
}

/// Go's `time.Parse`: parses value by layout. Without a zone in the value the
/// result is UTC; a zone offset or abbreviation is matched against
/// [`local`].
pub fn parse(layout: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<Time, ParseError> {
    let (layout, value) = (layout.as_ref(), value.as_ref());
    let local = local();
    // Optimize for RFC3339 as it accounts for over half of all
    // representations.
    if (layout == RFC3339.as_bytes() || layout == RFC3339_NANO.as_bytes())
        && let Some(t) = super::rfc3339::parse_rfc3339(value, &local)
    {
        return Ok(t);
    }
    parse_any(layout, value, &utc(), &local)
}

/// Go's `time.ParseInLocation`: like [`parse`], but a value without zone
/// information is in loc, and zones are matched against loc.
pub fn parse_in_location(
    layout: impl AsRef<[u8]>,
    value: impl AsRef<[u8]>,
    loc: &Location,
) -> Result<Time, ParseError> {
    let (layout, value) = (layout.as_ref(), value.as_ref());
    if (layout == RFC3339.as_bytes() || layout == RFC3339_NANO.as_bytes())
        && let Some(t) = super::rfc3339::parse_rfc3339(value, loc)
    {
        return Ok(t);
    }
    parse_any(layout, value, loc, loc)
}

/// The general parser (Go's `parse`), without the RFC 3339 fast path.
#[doc(hidden)]
pub fn parse_any(
    layout: &[u8],
    value: &[u8],
    default_location: &Location,
    local: &Location,
) -> Result<Time, ParseError> {
    let (alayout, avalue) = (layout, value);
    let mut layout = layout;
    let mut value = value;
    let mut range_err_string = ""; // set if a value is out of range
    let mut am_set = false; // do we need to subtract 12 from the hour for midnight?
    let mut pm_set = false; // do we need to add 12 to the hour?

    // Time being constructed.
    let mut year: i64 = 0;
    let mut month: i64 = -1;
    let mut day: i64 = -1;
    let mut yday: i64 = -1;
    let mut hour: i64 = 0;
    let mut min: i64 = 0;
    let mut sec: i64 = 0;
    let mut nsec: i64 = 0;
    let mut z: Option<Location> = None;
    let mut zone_offset: i64 = -1;
    let mut zone_name: &[u8] = b"";

    // Each iteration processes one std value.
    loop {
        let (prefix, std, suffix) = next_std_chunk(layout);
        let stdstr = &layout[prefix.len()..layout.len() - suffix.len()];
        value = match skip(value, prefix) {
            Ok(v) => v,
            Err(v) => {
                return Err(ParseError::new(alayout, avalue, prefix, v, String::new()));
            }
        };
        if std == 0 {
            if !value.is_empty() {
                return Err(ParseError::new(
                    alayout,
                    avalue,
                    b"",
                    value,
                    format!(": extra text: {}", quote(value)),
                ));
            }
            break;
        }
        layout = suffix;
        let hold = value;
        // Go keeps the error in a variable and checks it after the switch;
        // `bad` plays that part.
        let mut bad = false;
        match std & STD_MASK {
            STD_YEAR => {
                if value.len() < 2 {
                    bad = true;
                } else {
                    let p = &value[..2];
                    value = &value[2..];
                    match atoi(p) {
                        Ok(y) => year = if y >= 69 { y + 1900 } else { y + 2000 },
                        // Go leaves year as atoi's 0 on error.
                        Err(()) => {
                            year = 0;
                            bad = true;
                        }
                    }
                }
            }
            STD_LONG_YEAR => {
                if value.len() < 4 || !is_digit(value, 0) {
                    bad = true;
                } else {
                    let p = &value[..4];
                    value = &value[4..];
                    match atoi(p) {
                        Ok(y) => year = y,
                        Err(()) => {
                            year = 0;
                            bad = true;
                        }
                    }
                }
            }
            STD_MONTH | STD_LONG_MONTH => {
                let tab: &[&str] = if std == STD_MONTH {
                    &SHORT_MONTH_NAMES
                } else {
                    &LONG_MONTH_NAMES
                };
                match lookup(tab, value) {
                    Ok((m, rest)) => {
                        month = m + 1;
                        value = rest;
                    }
                    Err(()) => {
                        month = 0; // -1 + 1, as Go leaves it
                        bad = true;
                    }
                }
            }
            STD_NUM_MONTH | STD_ZERO_MONTH => match getnum(value, std == STD_ZERO_MONTH) {
                Ok((m, rest)) => {
                    month = m;
                    value = rest;
                    if month <= 0 || 12 < month {
                        range_err_string = "month";
                    }
                }
                Err(()) => {
                    month = 0;
                    bad = true;
                }
            },
            STD_WEEK_DAY | STD_LONG_WEEK_DAY => {
                // Ignore weekday except for error checking.
                let tab: &[&str] = if std == STD_WEEK_DAY {
                    &SHORT_DAY_NAMES
                } else {
                    &LONG_DAY_NAMES
                };
                match lookup(tab, value) {
                    Ok((_, rest)) => value = rest,
                    Err(()) => bad = true,
                }
            }
            STD_DAY | STD_UNDER_DAY | STD_ZERO_DAY => {
                if std == STD_UNDER_DAY && value.first() == Some(&b' ') {
                    value = &value[1..];
                }
                // Any one- or two-digit day is allowed here; the date is
                // validated after parsing.
                match getnum(value, std == STD_ZERO_DAY) {
                    Ok((d, rest)) => {
                        day = d;
                        value = rest;
                    }
                    Err(()) => {
                        day = 0;
                        bad = true;
                    }
                }
            }
            STD_UNDER_YEAR_DAY | STD_ZERO_YEAR_DAY => {
                for _ in 0..2 {
                    if std == STD_UNDER_YEAR_DAY && value.first() == Some(&b' ') {
                        value = &value[1..];
                    }
                }
                match getnum3(value, std == STD_ZERO_YEAR_DAY) {
                    Ok((d, rest)) => {
                        yday = d;
                        value = rest;
                    }
                    Err(()) => {
                        yday = 0;
                        bad = true;
                    }
                }
            }
            STD_HOUR => match getnum(value, false) {
                Ok((h, rest)) => {
                    hour = h;
                    value = rest;
                    if !(0..24).contains(&hour) {
                        range_err_string = "hour";
                    }
                }
                Err(()) => {
                    hour = 0;
                    bad = true;
                }
            },
            STD_HOUR12 | STD_ZERO_HOUR12 => match getnum(value, std == STD_ZERO_HOUR12) {
                Ok((h, rest)) => {
                    hour = h;
                    value = rest;
                    if !(0..=12).contains(&hour) {
                        range_err_string = "hour";
                    }
                }
                Err(()) => {
                    hour = 0;
                    bad = true;
                }
            },
            STD_MINUTE | STD_ZERO_MINUTE => match getnum(value, std == STD_ZERO_MINUTE) {
                Ok((m, rest)) => {
                    min = m;
                    value = rest;
                    if !(0..60).contains(&min) {
                        range_err_string = "minute";
                    }
                }
                Err(()) => {
                    min = 0;
                    bad = true;
                }
            },
            STD_SECOND | STD_ZERO_SECOND => match getnum(value, std == STD_ZERO_SECOND) {
                Err(()) => {
                    sec = 0;
                    bad = true;
                }
                Ok((s, rest)) => {
                    sec = s;
                    value = rest;
                    if !(0..60).contains(&sec) {
                        range_err_string = "second";
                    } else if value.len() >= 2 && comma_or_period(value[0]) && is_digit(value, 1) {
                        // A fractional second in the value but not, perhaps,
                        // in the layout.
                        let (_, next, _) = next_std_chunk(layout);
                        let next = next & STD_MASK;
                        if next != STD_FRAC_SECOND0 && next != STD_FRAC_SECOND9 {
                            // No fractional second in the layout but there is
                            // one in the value.
                            let mut n = 2;
                            while n < value.len() && is_digit(value, n) {
                                n += 1;
                            }
                            match parse_nanoseconds(value, n) {
                                Ok((ns, range)) => {
                                    nsec = ns;
                                    range_err_string = range;
                                }
                                Err(()) => bad = true,
                            }
                            value = &value[n..];
                        }
                    }
                }
            },
            STD_PM | STD_PM_LOWER => {
                if value.len() < 2 {
                    bad = true;
                } else {
                    let p = &value[..2];
                    value = &value[2..];
                    let (pm, am): (&[u8], &[u8]) = if std == STD_PM {
                        (b"PM", b"AM")
                    } else {
                        (b"pm", b"am")
                    };
                    if p == pm {
                        pm_set = true;
                    } else if p == am {
                        am_set = true;
                    } else {
                        bad = true;
                    }
                }
            }
            STD_ISO8601_TZ
            | STD_ISO8601_SHORT_TZ
            | STD_ISO8601_COLON_TZ
            | STD_ISO8601_SECONDS_TZ
            | STD_ISO8601_COLON_SECONDS_TZ
            | STD_NUM_TZ
            | STD_NUM_SHORT_TZ
            | STD_NUM_COLON_TZ
            | STD_NUM_SECONDS_TZ
            | STD_NUM_COLON_SECONDS_TZ => {
                let iso = matches!(
                    std,
                    STD_ISO8601_TZ
                        | STD_ISO8601_SHORT_TZ
                        | STD_ISO8601_COLON_TZ
                        | STD_ISO8601_SECONDS_TZ
                        | STD_ISO8601_COLON_SECONDS_TZ
                );
                if iso && value.first() == Some(&b'Z') {
                    value = &value[1..];
                    z = Some(utc());
                } else {
                    match parse_numeric_zone(std, value) {
                        None => bad = true,
                        Some((sign, hh, mm, ss, rest)) => {
                            value = rest;
                            let mut hr = 0;
                            let mut mi = 0;
                            let mut se = 0;
                            match getnum(hh, true) {
                                Err(()) => bad = true,
                                Ok((h, _)) => {
                                    hr = h;
                                    match getnum(mm, true) {
                                        Err(()) => bad = true,
                                        Ok((m, _)) => {
                                            mi = m;
                                            match getnum(ss, true) {
                                                Err(()) => bad = true,
                                                Ok((s, _)) => se = s,
                                            }
                                        }
                                    }
                                }
                            }
                            // The range tests use > rather than >=, as some
                            // people do write offsets of 24 hours or 60
                            // minutes or 60 seconds.
                            if hr > 24 {
                                range_err_string = "time zone offset hour";
                            }
                            if mi > 60 {
                                range_err_string = "time zone offset minute";
                            }
                            if se > 60 {
                                range_err_string = "time zone offset second";
                            }
                            zone_offset = (hr * 60 + mi) * 60 + se; // offset is in seconds
                            match sign {
                                b'+' => {}
                                b'-' => zone_offset = -zone_offset,
                                _ => bad = true,
                            }
                        }
                    }
                }
            }
            STD_TZ => {
                // Does it look like a time zone?
                if value.starts_with(b"UTC") {
                    z = Some(utc());
                    value = &value[3..];
                } else {
                    match parse_time_zone(value) {
                        Some(n) => {
                            zone_name = &value[..n];
                            value = &value[n..];
                        }
                        None => bad = true,
                    }
                }
            }
            STD_FRAC_SECOND0 => {
                // stdFracSecond0 requires the exact number of digits as
                // specified in the layout.
                let ndigit = 1 + digits_len(std) as usize;
                if value.len() < ndigit {
                    bad = true;
                } else {
                    match parse_nanoseconds(value, ndigit) {
                        Ok((ns, range)) => {
                            nsec = ns;
                            range_err_string = range;
                        }
                        Err(()) => bad = true,
                    }
                    value = &value[ndigit..];
                }
            }
            STD_FRAC_SECOND9 => {
                if value.len() < 2 || !comma_or_period(value[0]) || !value[1].is_ascii_digit() {
                    // Fractional second omitted.
                } else {
                    // Take any number of digits, even more than asked for,
                    // because it is what the stdSecond case would do.
                    let mut i = 0;
                    while i + 1 < value.len() && value[i + 1].is_ascii_digit() {
                        i += 1;
                    }
                    match parse_nanoseconds(value, 1 + i) {
                        Ok((ns, range)) => {
                            nsec = ns;
                            range_err_string = range;
                        }
                        Err(()) => bad = true,
                    }
                    value = &value[1 + i..];
                }
            }
            _ => {}
        }
        if !range_err_string.is_empty() {
            return Err(ParseError::new(
                alayout,
                avalue,
                stdstr,
                value,
                format!(": {range_err_string} out of range"),
            ));
        }
        if bad {
            return Err(ParseError::new(
                alayout,
                avalue,
                stdstr,
                hold,
                String::new(),
            ));
        }
    }
    if pm_set && hour < 12 {
        hour += 12;
    } else if am_set && hour == 12 {
        hour = 0;
    }

    // Convert yday to day, month.
    if yday >= 0 {
        let mut d = 0;
        let mut m = 0;
        if is_leap(year) {
            if yday == 31 + 29 {
                m = Month::FEBRUARY.0;
                d = 29;
            } else if yday > 31 + 29 {
                yday -= 1;
            }
        }
        if !(1..=365).contains(&yday) {
            return Err(ParseError::new(
                alayout,
                avalue,
                b"",
                value,
                ": day-of-year out of range".into(),
            ));
        }
        if m == 0 {
            m = (yday - 1) / 31 + 1;
            if days_before(Month(m + 1)) < yday {
                m += 1;
            }
            d = yday - days_before(Month(m));
        }
        // If month, day already seen, yday's m, d must match. Otherwise, set
        // them from m, d.
        if month >= 0 && month != m {
            return Err(ParseError::new(
                alayout,
                avalue,
                b"",
                value,
                ": day-of-year does not match month".into(),
            ));
        }
        month = m;
        if day >= 0 && day != d {
            return Err(ParseError::new(
                alayout,
                avalue,
                b"",
                value,
                ": day-of-year does not match day".into(),
            ));
        }
        day = d;
    } else {
        if month < 0 {
            month = Month::JANUARY.0;
        }
        if day < 0 {
            day = 1;
        }
    }

    // Validate the day of the month.
    if day < 1 || day > days_in(Month(month), year) {
        return Err(ParseError::new(
            alayout,
            avalue,
            b"",
            value,
            ": day out of range".into(),
        ));
    }

    if let Some(z) = z {
        return Ok(date(year, Month(month), day, hour, min, sec, nsec, &z));
    }

    if zone_offset != -1 {
        let mut t = date(year, Month(month), day, hour, min, sec, nsec, &utc());
        t.add_sec(-zone_offset);

        // Look for a local zone with the given offset. If that zone was in
        // effect at the given time, use it.
        let found = local.lookup(t.unix());
        if found.offset == zone_offset
            && (zone_name.is_empty() || found.name.as_bytes() == zone_name)
        {
            t.set_loc(local);
            return Ok(t);
        }

        // Otherwise create a fake zone to record the offset.
        t.set_loc(&fixed_zone(
            &String::from_utf8_lossy(zone_name),
            zone_offset,
        ));
        return Ok(t);
    }

    if !zone_name.is_empty() {
        let mut t = date(year, Month(month), day, hour, min, sec, nsec, &utc());
        // Look for a local zone with the given name. If that zone was in
        // effect at the given time, use it.
        if let Some(offset) = local.lookup_name(zone_name, t.unix()) {
            t.add_sec(-offset);
            t.set_loc(local);
            return Ok(t);
        }

        // Otherwise, create a fake zone with an unknown offset.
        let mut offset = 0;
        if zone_name.len() > 3 && zone_name.starts_with(b"GMT") {
            // Guaranteed OK by parse_gmt; Go ignores the error.
            offset = atoi(&zone_name[3..]).unwrap_or(0);
            offset *= 3600;
        }
        t.set_loc(&fixed_zone(&String::from_utf8_lossy(zone_name), offset));
        return Ok(t);
    }

    // Otherwise, fall back to default.
    Ok(date(
        year,
        Month(month),
        day,
        hour,
        min,
        sec,
        nsec,
        default_location,
    ))
}

/// Splits a numeric zone offset off value by the layout element: sign, hour,
/// minute and second fields, and the rest. None where value is too short or
/// lacks a colon the element needs.
#[allow(clippy::type_complexity)]
fn parse_numeric_zone(std: i64, value: &[u8]) -> Option<(u8, &[u8], &[u8], &[u8], &[u8])> {
    if std == STD_ISO8601_COLON_TZ || std == STD_NUM_COLON_TZ {
        if value.len() < 6 || value[3] != b':' {
            return None;
        }
        Some((value[0], &value[1..3], &value[4..6], b"00", &value[6..]))
    } else if std == STD_NUM_SHORT_TZ || std == STD_ISO8601_SHORT_TZ {
        if value.len() < 3 {
            return None;
        }
        Some((value[0], &value[1..3], b"00", b"00", &value[3..]))
    } else if std == STD_ISO8601_COLON_SECONDS_TZ || std == STD_NUM_COLON_SECONDS_TZ {
        if value.len() < 9 || value[3] != b':' || value[6] != b':' {
            return None;
        }
        Some((
            value[0],
            &value[1..3],
            &value[4..6],
            &value[7..9],
            &value[9..],
        ))
    } else if std == STD_ISO8601_SECONDS_TZ || std == STD_NUM_SECONDS_TZ {
        if value.len() < 7 {
            return None;
        }
        Some((
            value[0],
            &value[1..3],
            &value[3..5],
            &value[5..7],
            &value[7..],
        ))
    } else {
        if value.len() < 5 {
            return None;
        }
        Some((value[0], &value[1..3], &value[3..5], b"00", &value[5..]))
    }
}

/// Go's `parseTimeZone`: the length of the time zone at the start of value,
/// or None. A run of 3 upper-case letters is a zone, 4 or 5 if it ends in T,
/// plus the special cases ChST, MeST, WITA, GMT with an hour offset, and a
/// bare signed hour offset.
pub fn parse_time_zone(value: &[u8]) -> Option<usize> {
    if value.len() < 3 {
        return None;
    }
    // Special case 1: ChST and MeST are the only zones with a lower-case
    // letter.
    if value.len() >= 4 && (&value[..4] == b"ChST" || &value[..4] == b"MeST") {
        return Some(4);
    }
    // Special case 2: GMT may have an hour offset; treat it specially.
    if &value[..3] == b"GMT" {
        return Some(parse_gmt(value));
    }
    // Special case 3: some time zones are not named, but have +/-00 format.
    if value[0] == b'+' || value[0] == b'-' {
        let length = parse_signed_offset(value);
        return (length > 0).then_some(length);
    }
    // How many upper-case letters are there? Need at least three, at most
    // five.
    let mut n_upper = 0;
    while n_upper < 6 {
        if n_upper >= value.len() || !value[n_upper].is_ascii_uppercase() {
            break;
        }
        n_upper += 1;
    }
    match n_upper {
        5 if value[4] == b'T' => Some(5),
        4 if value[3] == b'T' || &value[..4] == b"WITA" => Some(4),
        3 => Some(3),
        _ => None,
    }
}

/// Parses a GMT zone (value starts with "GMT"), with an optional signed hour
/// offset.
fn parse_gmt(value: &[u8]) -> usize {
    let value = &value[3..];
    if value.is_empty() {
        return 3;
    }
    3 + parse_signed_offset(value)
}

/// The length of a signed offset in -23..+23 ("+03", "-4"), or 0.
fn parse_signed_offset(value: &[u8]) -> usize {
    let sign = value[0];
    if sign != b'-' && sign != b'+' {
        return 0;
    }
    let Ok((x, rem)) = leading_int(&value[1..]) else {
        return 0;
    };
    // Fail if nothing was consumed.
    if value[1..].len() == rem.len() {
        return 0;
    }
    if x > 23 {
        return 0;
    }
    value.len() - rem.len()
}

fn comma_or_period(b: u8) -> bool {
    b == b'.' || b == b','
}

/// Parses `value[0..nbytes]`, a separator and digits, as nanoseconds; the
/// second result is a range error name or "".
pub(crate) fn parse_nanoseconds(value: &[u8], nbytes: usize) -> Result<(i64, &'static str), ()> {
    if !comma_or_period(value[0]) {
        return Err(());
    }
    let nbytes = nbytes.min(10);
    let mut ns = atoi(&value[1..nbytes])?;
    if ns < 0 {
        return Ok((ns, "fractional second"));
    }
    // Scale by the number of missing digits, maximum length 10.
    for _ in 0..10 - nbytes {
        ns *= 10;
    }
    Ok((ns, ""))
}

/// Consumes the leading `[0-9]*` of s; an error on overflow past 2^63.
pub(crate) fn leading_int(s: &[u8]) -> Result<(u64, &[u8]), ()> {
    let mut x: u64 = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        if x > (1 << 63) / 10 {
            return Err(()); // overflow
        }
        x = x * 10 + u64::from(c - b'0');
        if x > 1 << 63 {
            return Err(()); // overflow
        }
        i += 1;
    }
    Ok((x, &s[i..]))
}

/// Consumes the leading `[0-9]*` of a fraction; stops accumulating precision
/// on overflow instead of failing. Returns the digits, their scale and the
/// rest.
pub(crate) fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut x: u64 = 0;
    let mut scale = 1.0;
    let mut overflow = false;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            break;
        }
        i += 1;
        if overflow {
            continue;
        }
        if x > ((1u64 << 63) - 1) / 10 {
            // It's possible for overflow to give a positive number, so take
            // care.
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}
