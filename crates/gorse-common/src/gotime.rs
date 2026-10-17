// Ports: $GOROOT/src/time/time.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package: Copyright 2009 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `time` package: [`Time`], [`Duration`], [`Location`], and the layout
//! based [`Time::format`] and [`parse`].
//!
//! Gorse writes times as Go formats them (the cache stores `Time.String()`,
//! REST bodies carry `MarshalJSON`, configs hold `Duration` strings) and reads
//! them back through Go's parsers and `dateparse`. This module reproduces those
//! byte for byte, including the monotonic clock reading `String()` appends and
//! the error texts, so a Rust node and a Go node read each other's values.
//!
//! The shape follows Go: Go's `int` is `i64` here, and arithmetic that wraps in
//! Go wraps here. `time.Date`, `time.Unix` and friends are free functions
//! ([`date`], [`unix`]); Go's mutable `time.Local` is [`local`] and
//! [`set_local`]. The Go source is the specification; where this code differs
//! it says so, and `porting/notes/WP-1-9.md` lists every difference.

mod duration;
mod errno;
mod format;
mod rfc3339;
mod zoneinfo;
mod zoneinfo_read;

use std::cmp::Ordering;
use std::fmt;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub use duration::{
    Duration, HOUR, MICROSECOND, MILLISECOND, MINUTE, NANOSECOND, ParseDurationError, SECOND,
    parse_duration,
};
pub use format::{
    ANSIC, DATE_ONLY, DATE_TIME, KITCHEN, LAYOUT, ParseError, RFC822, RFC822Z, RFC850, RFC1123,
    RFC1123Z, RFC3339, RFC3339_NANO, RUBY_DATE, STAMP, STAMP_MICRO, STAMP_MILLI, STAMP_NANO,
    TIME_ONLY, UNIX_DATE, parse, parse_in_location,
};
pub use rfc3339::UnmarshalError;
pub use zoneinfo::{Location, fixed_zone, local, set_local, utc};
pub use zoneinfo_read::{LocationError, load_location, load_location_from_tz_data};

#[doc(hidden)]
pub mod internal {
    //! Unexported Go helpers that Go's own tests reach through
    //! `export_test.go`; the Rust ports of those tests use them from here.
    //! Not part of the API.

    pub use super::format::{
        append_int, next_std_chunk, parse_any, parse_time_zone, quote, std_chunk_name,
    };
    pub use super::rfc3339::parse_rfc3339;
    pub use super::zoneinfo::{
        Rule, RuleKind, reset_local, tzset, tzset_name, tzset_offset, tzset_rule,
    };
    pub use super::zoneinfo_read::{
        PlatformSourcesGuard, disable_platform_sources, goroot_zone_source,
        load_location_from_sources, load_tzinfo, read_file, reset_zoneinfo_for_testing,
        set_default_goroot, zoneinfo_for_testing,
    };
    pub use super::{UNIX_TO_INTERNAL, days_in, time_from_parts, time_parts};
}

/// A Go `time.Time`: an instant with nanosecond precision, an optional
/// monotonic clock reading, and the [`Location`] it is displayed in.
///
/// The zero value (`Time::default()`) is January 1, year 1, 00:00:00 UTC, as
/// in Go. `==` compares like Go's `==`: the wall clock, the monotonic reading
/// and the identity of the location; [`Time::equal`] compares instants.
#[derive(Clone, Default)]
pub struct Time {
    // From high to low bit: hasMonotonic (1 bit), 33-bit seconds since 1885
    // (only with hasMonotonic), 30-bit nanoseconds. As in Go.
    wall: u64,
    // Seconds since year 1 without hasMonotonic; the monotonic reading with it.
    ext: i64,
    // None is UTC, and UTC is never stored as Some (Go's nil loc), except where
    // Go stores the value of `Local` directly and `Local` is UTC.
    loc: Option<Location>,
}

const HAS_MONOTONIC: u64 = 1 << 63;
const MAX_WALL: i64 = WALL_TO_INTERNAL + ((1 << 33) - 1); // year 2157
const MIN_WALL: i64 = WALL_TO_INTERNAL; // year 1885
const NSEC_MASK: u64 = (1 << 30) - 1;
const NSEC_SHIFT: u32 = 30;

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;
const DAYS_PER_400_YEARS: u64 = 365 * 400 + 97;

// Days from March 1 through end of year.
const MARCH_THRU_DECEMBER: i64 = 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30 + 31;

// The number of years subtracted from internal time to get absolute time
// (March 1 of year -ABSOLUTE_YEARS is the absolute zero instant).
const ABSOLUTE_YEARS: u64 = 292_277_022_400;

// Go writes this as -(absoluteYears*365.2425 + marchThruDecember) * secondsPerDay
// with exact constant arithmetic; absoluteYears is a multiple of 400.
const ABSOLUTE_TO_INTERNAL: i64 =
    -(((ABSOLUTE_YEARS / 400 * DAYS_PER_400_YEARS) as i64 + MARCH_THRU_DECEMBER) * SECONDS_PER_DAY);
const INTERNAL_TO_ABSOLUTE: i64 = -ABSOLUTE_TO_INTERNAL;

/// Seconds from January 1, year 1 to the Unix epoch (Go's
/// `unixToInternal`).
pub const UNIX_TO_INTERNAL: i64 =
    (1969 * 365 + 1969 / 4 - 1969 / 100 + 1969 / 400) * SECONDS_PER_DAY;
const INTERNAL_TO_UNIX: i64 = -UNIX_TO_INTERNAL;

const ABSOLUTE_TO_UNIX: i64 = ABSOLUTE_TO_INTERNAL + INTERNAL_TO_UNIX;

const WALL_TO_INTERNAL: i64 = (1884 * 365 + 1884 / 4 - 1884 / 100 + 1884 / 400) * SECONDS_PER_DAY;

/// A month of the year (January = 1, ...). Like Go's `time.Month`, any `i64`
/// is a value; [`date`] normalizes out-of-range months.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month(pub i64);

impl Month {
    /// January.
    pub const JANUARY: Month = Month(1);
    /// February.
    pub const FEBRUARY: Month = Month(2);
    /// March.
    pub const MARCH: Month = Month(3);
    /// April.
    pub const APRIL: Month = Month(4);
    /// May.
    pub const MAY: Month = Month(5);
    /// June.
    pub const JUNE: Month = Month(6);
    /// July.
    pub const JULY: Month = Month(7);
    /// August.
    pub const AUGUST: Month = Month(8);
    /// September.
    pub const SEPTEMBER: Month = Month(9);
    /// October.
    pub const OCTOBER: Month = Month(10);
    /// November.
    pub const NOVEMBER: Month = Month(11);
    /// December.
    pub const DECEMBER: Month = Month(12);
}

/// `Month.String`: the English name, or `%!Month(n)` out of range.
impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if (1..=12).contains(&self.0) {
            f.write_str(format::LONG_MONTH_NAMES[(self.0 - 1) as usize])
        } else {
            // Go formats the value as a uint64.
            write!(f, "%!Month({})", self.0 as u64)
        }
    }
}

/// A day of the week (Sunday = 0, ...), like Go's `time.Weekday`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Weekday(pub i64);

impl Weekday {
    /// Sunday.
    pub const SUNDAY: Weekday = Weekday(0);
    /// Monday.
    pub const MONDAY: Weekday = Weekday(1);
    /// Tuesday.
    pub const TUESDAY: Weekday = Weekday(2);
    /// Wednesday.
    pub const WEDNESDAY: Weekday = Weekday(3);
    /// Thursday.
    pub const THURSDAY: Weekday = Weekday(4);
    /// Friday.
    pub const FRIDAY: Weekday = Weekday(5);
    /// Saturday.
    pub const SATURDAY: Weekday = Weekday(6);
}

/// `Weekday.String`: the English name, or `%!Weekday(n)` out of range.
impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if (0..=6).contains(&self.0) {
            f.write_str(format::LONG_DAY_NAMES[self.0 as usize])
        } else {
            write!(f, "%!Weekday({})", self.0 as u64)
        }
    }
}

// The calendar arithmetic below is Go's (Neri and Schneider, "Euclidean affine
// functions and their application to calendar algorithms"); see the
// "Computations on Times" comment in time.go. Absolute seconds and days count
// from March 1 of year -292277022400 and are unsigned.

/// Days from the absolute epoch to year-month-day; `day` may be out of range.
fn date_to_abs_days(year: i64, month: Month, day: i64) -> u64 {
    let mut amonth = month.0 as u32;
    let jan_feb = u32::from(amonth < 3);
    amonth = amonth.wrapping_add(12 * jan_feb);
    let y = (year as u64)
        .wrapping_sub(u64::from(jan_feb))
        .wrapping_add(ABSOLUTE_YEARS);
    let ayday = 979u32.wrapping_mul(amonth).wrapping_sub(2919) >> 5;
    let century = y / 100;
    let cyear = (y % 100) as u32;
    let cday = 1461 * cyear / 4;
    let centurydays = 146_097u64.wrapping_mul(century) / 4;
    centurydays.wrapping_add(
        i64::from(cday.wrapping_add(ayday))
            .wrapping_add(day)
            .wrapping_sub(1) as u64,
    )
}

/// Splits absolute days into century, year of century, and March-based day of
/// year.
fn split_days(days: u64) -> (u64, i64, i64) {
    let d = 4u64.wrapping_mul(days).wrapping_add(3);
    let century = d / 146_097;
    let cd = (d % 146_097) as u32 | 3;
    let product = 2_939_745u64 * u64::from(cd);
    let hi = (product >> 32) as u32;
    let lo = product as u32;
    (century, i64::from(hi), i64::from(lo / 2_939_745 / 4))
}

/// Splits a March-based day of year into a March-based month and a 1-based day.
fn split_yday(ayday: i64) -> (i64, i64) {
    let d = 2141u32.wrapping_mul(ayday as u32).wrapping_add(197_913);
    (i64::from(d >> 16), 1 + i64::from((d & 0xFFFF) / 2141))
}

fn jan_feb(ayday: i64) -> i64 {
    i64::from(ayday >= MARCH_THRU_DECEMBER)
}

fn abs_month(m: i64, jan_feb: i64) -> Month {
    Month(m - jan_feb * 12)
}

fn abs_leap(century: u64, cyear: i64) -> i64 {
    let y4ok = i64::from(cyear % 4 == 0);
    let y100ok = i64::from(cyear != 0);
    let y400ok = i64::from(century.is_multiple_of(4));
    y4ok & (y100ok | y400ok)
}

fn abs_year(century: u64, cyear: i64, jan_feb: i64) -> i64 {
    (century.wrapping_mul(100).wrapping_sub(ABSOLUTE_YEARS) as i64)
        .wrapping_add(cyear)
        .wrapping_add(jan_feb)
}

fn abs_yday(ayday: i64, jan_feb: i64, leap: i64) -> i64 {
    ayday + (1 + 31 + 28) + (leap & !jan_feb) - 365 * jan_feb
}

fn abs_date(days: u64) -> (i64, Month, i64) {
    let (century, cyear, ayday) = split_days(days);
    let (amonth, day) = split_yday(ayday);
    let jf = jan_feb(ayday);
    (abs_year(century, cyear, jf), abs_month(amonth, jf), day)
}

fn abs_year_yday(days: u64) -> (i64, i64) {
    let (century, cyear, ayday) = split_days(days);
    let jf = jan_feb(ayday);
    (
        abs_year(century, cyear, jf),
        abs_yday(ayday, jf, abs_leap(century, cyear)),
    )
}

fn abs_weekday(days: u64) -> Weekday {
    // March 1 of the absolute year, like March 1 of 2000, was a Wednesday.
    Weekday((days.wrapping_add(Weekday::WEDNESDAY.0 as u64) % 7) as i64)
}

fn abs_clock(abs: u64) -> (i64, i64, i64) {
    let mut sec = (abs % SECONDS_PER_DAY as u64) as i64;
    let hour = sec / SECONDS_PER_HOUR;
    sec -= hour * SECONDS_PER_HOUR;
    let min = sec / SECONDS_PER_MINUTE;
    sec -= min * SECONDS_PER_MINUTE;
    (hour, min, sec)
}

fn abs_days(abs: u64) -> u64 {
    abs / SECONDS_PER_DAY as u64
}

fn is_leap(year: i64) -> bool {
    // year%4 == 0 && (year%100 != 0 || year%400 == 0), as Go computes it.
    let mask = if year % 25 != 0 { 3 } else { 0xf };
    year & mask == 0
}

/// The number of days in a non-leap year before month m.
pub(crate) fn days_before(m: Month) -> i64 {
    let adj = if m >= Month::MARCH { -2 } else { 0 };
    (214i64.wrapping_mul(m.0).wrapping_sub(211)) / 7 + adj
}

/// The number of days in month m of year.
pub fn days_in(m: Month, year: i64) -> i64 {
    if m == Month::FEBRUARY {
        return if is_leap(year) { 29 } else { 28 };
    }
    30 + ((m.0 + (m.0 >> 3)) & 1)
}

/// Normalizes `hi*base + lo` so that `0 <= lo < base`, wrapping like Go's
/// `int`.
fn norm(mut hi: i64, mut lo: i64, base: i64) -> (i64, i64) {
    if lo < 0 {
        let n = (lo.wrapping_neg().wrapping_sub(1)) / base + 1;
        hi = hi.wrapping_sub(n);
        lo = lo.wrapping_add(n.wrapping_mul(base));
    }
    if lo >= base {
        let n = lo / base;
        hi = hi.wrapping_add(n);
        lo = lo.wrapping_sub(n.wrapping_mul(base));
    }
    (hi, lo)
}

impl Time {
    fn nsec(&self) -> i32 {
        (self.wall & NSEC_MASK) as i32
    }

    /// Seconds since January 1, year 1.
    fn sec(&self) -> i64 {
        if self.wall & HAS_MONOTONIC != 0 {
            return WALL_TO_INTERNAL + ((self.wall << 1) >> (NSEC_SHIFT + 1)) as i64;
        }
        self.ext
    }

    fn unix_sec(&self) -> i64 {
        self.sec().wrapping_add(INTERNAL_TO_UNIX)
    }

    pub(crate) fn add_sec(&mut self, d: i64) {
        if self.wall & HAS_MONOTONIC != 0 {
            let sec = ((self.wall << 1) >> (NSEC_SHIFT + 1)) as i64;
            let dsec = sec.wrapping_add(d);
            if (0..=(1 << 33) - 1).contains(&dsec) {
                self.wall = self.wall & NSEC_MASK | (dsec as u64) << NSEC_SHIFT | HAS_MONOTONIC;
                return;
            }
            // Wall second now out of range for the packed field.
            self.strip_mono();
        }
        // Saturate where the sum overflows, as Go does.
        let sum = self.ext.wrapping_add(d);
        if (sum > self.ext) == (d > 0) {
            self.ext = sum;
        } else if d > 0 {
            self.ext = i64::MAX;
        } else {
            self.ext = -i64::MAX;
        }
    }

    pub(crate) fn set_loc(&mut self, loc: &Location) {
        self.strip_mono();
        self.loc = if loc.is_utc() {
            None
        } else {
            Some(loc.clone())
        };
    }

    fn strip_mono(&mut self) {
        if self.wall & HAS_MONOTONIC != 0 {
            self.ext = self.sec();
            self.wall &= NSEC_MASK;
        }
    }

    /// Go's `setMono`: records a monotonic reading if the wall time fits the
    /// packed field.
    #[doc(hidden)]
    pub fn set_mono(&mut self, m: i64) {
        if self.wall & HAS_MONOTONIC == 0 {
            let sec = self.ext;
            if !(MIN_WALL..=MAX_WALL).contains(&sec) {
                return;
            }
            self.wall |= HAS_MONOTONIC | ((sec - MIN_WALL) as u64) << NSEC_SHIFT;
        }
        self.ext = m;
    }

    /// Go's `mono`: the monotonic reading, 0 if there is none.
    #[doc(hidden)]
    pub fn mono(&self) -> i64 {
        if self.wall & HAS_MONOTONIC == 0 {
            return 0;
        }
        self.ext
    }

    /// The location as Go's `get` sees it: nil is UTC.
    fn loc_or_utc(&self) -> Location {
        match &self.loc {
            Some(l) => l.clone(),
            None => utc(),
        }
    }

    /// Reports whether t is the zero instant, January 1, year 1, 00:00:00 UTC.
    pub fn is_zero(&self) -> bool {
        self.wall == 0 && self.ext == 0
    }

    /// Reports whether t is after u.
    pub fn after(&self, u: &Time) -> bool {
        if self.wall & u.wall & HAS_MONOTONIC != 0 {
            return self.ext > u.ext;
        }
        let ts = self.sec();
        let us = u.sec();
        ts > us || ts == us && self.nsec() > u.nsec()
    }

    /// Reports whether t is before u.
    pub fn before(&self, u: &Time) -> bool {
        if self.wall & u.wall & HAS_MONOTONIC != 0 {
            return self.ext < u.ext;
        }
        let ts = self.sec();
        let us = u.sec();
        ts < us || ts == us && self.nsec() < u.nsec()
    }

    /// Compares the instants: -1 if t is before u, +1 if after, 0 if equal.
    pub fn compare(&self, u: &Time) -> i64 {
        let (tc, uc) = if self.wall & u.wall & HAS_MONOTONIC != 0 {
            (self.ext, u.ext)
        } else if self.sec() == u.sec() {
            (i64::from(self.nsec()), i64::from(u.nsec()))
        } else {
            (self.sec(), u.sec())
        };
        match tc.cmp(&uc) {
            Ordering::Less => -1,
            Ordering::Greater => 1,
            Ordering::Equal => 0,
        }
    }

    /// Reports whether t and u are the same instant.
    pub fn equal(&self, u: &Time) -> bool {
        if self.wall & u.wall & HAS_MONOTONIC != 0 {
            return self.ext == u.ext;
        }
        self.sec() == u.sec() && self.nsec() == u.nsec()
    }

    /// Absolute seconds of t, adjusted by its zone offset.
    fn abs_sec(&self) -> u64 {
        let sec = self.unix_sec();
        let offset = match &self.loc {
            Some(l) if !l.is_utc() => l.lookup(sec).offset,
            _ => 0,
        };
        sec.wrapping_add(offset)
            .wrapping_add(UNIX_TO_INTERNAL + INTERNAL_TO_ABSOLUTE) as u64
    }

    /// The zone name and offset together with the absolute seconds.
    fn locabs(&self) -> (String, i64, u64) {
        let mut sec = self.unix_sec();
        let (name, offset) = match &self.loc {
            Some(l) if !l.is_utc() => {
                let z = l.lookup(sec);
                (z.name.to_owned(), z.offset)
            }
            _ => ("UTC".to_owned(), 0),
        };
        sec = sec.wrapping_add(offset);
        (
            name,
            offset,
            sec.wrapping_add(UNIX_TO_INTERNAL + INTERNAL_TO_ABSOLUTE) as u64,
        )
    }

    /// The year, month and day in which t occurs.
    pub fn date(&self) -> (i64, Month, i64) {
        abs_date(abs_days(self.abs_sec()))
    }

    /// The year in which t occurs.
    pub fn year(&self) -> i64 {
        let (century, cyear, ayday) = split_days(abs_days(self.abs_sec()));
        abs_year(century, cyear, jan_feb(ayday))
    }

    /// The month of the year in which t occurs.
    pub fn month(&self) -> Month {
        let (_, _, ayday) = split_days(abs_days(self.abs_sec()));
        let (amonth, _) = split_yday(ayday);
        abs_month(amonth, jan_feb(ayday))
    }

    /// The day of the month.
    pub fn day(&self) -> i64 {
        let (_, _, ayday) = split_days(abs_days(self.abs_sec()));
        split_yday(ayday).1
    }

    /// The day of the week.
    pub fn weekday(&self) -> Weekday {
        abs_weekday(abs_days(self.abs_sec()))
    }

    /// The ISO 8601 year and week number in which t occurs.
    pub fn iso_week(&self) -> (i64, i64) {
        let days = abs_days(self.abs_sec());
        // Thursday of the same Monday-based week.
        let wd = abs_weekday(days.wrapping_sub(1)).0 + 1;
        let thu = days.wrapping_add((Weekday::THURSDAY.0 - wd) as u64);
        let (year, yday) = abs_year_yday(thu);
        (year, (yday - 1) / 7 + 1)
    }

    /// The hour, minute and second within the day.
    pub fn clock(&self) -> (i64, i64, i64) {
        abs_clock(self.abs_sec())
    }

    /// The hour within the day, in [0, 23].
    pub fn hour(&self) -> i64 {
        (self.abs_sec() % SECONDS_PER_DAY as u64) as i64 / SECONDS_PER_HOUR
    }

    /// The minute within the hour, in [0, 59].
    pub fn minute(&self) -> i64 {
        (self.abs_sec() % SECONDS_PER_HOUR as u64) as i64 / SECONDS_PER_MINUTE
    }

    /// The second within the minute, in [0, 59].
    pub fn second(&self) -> i64 {
        (self.abs_sec() % SECONDS_PER_MINUTE as u64) as i64
    }

    /// The nanosecond within the second, in [0, 999999999].
    pub fn nanosecond(&self) -> i64 {
        i64::from(self.nsec())
    }

    /// The day of the year, in [1, 365] or [1, 366] in leap years.
    pub fn year_day(&self) -> i64 {
        abs_year_yday(abs_days(self.abs_sec())).1
    }

    /// Returns t+d.
    #[must_use]
    pub fn add(&self, d: Duration) -> Time {
        let mut t = self.clone();
        let mut dsec = d.0 / 1_000_000_000;
        let mut nsec = t.nsec() + (d.0 % 1_000_000_000) as i32;
        if nsec >= 1_000_000_000 {
            dsec += 1;
            nsec -= 1_000_000_000;
        } else if nsec < 0 {
            dsec -= 1;
            nsec += 1_000_000_000;
        }
        t.wall = t.wall & !NSEC_MASK | nsec as u64;
        t.add_sec(dsec);
        if t.wall & HAS_MONOTONIC != 0 {
            let te = t.ext.wrapping_add(d.0);
            if d.0 < 0 && te > t.ext || d.0 > 0 && te < t.ext {
                // Monotonic reading out of range; degrade to wall-only.
                t.strip_mono();
            } else {
                t.ext = te;
            }
        }
        t
    }

    /// Returns t-u, saturated to the Duration range.
    pub fn sub(&self, u: &Time) -> Duration {
        if self.wall & u.wall & HAS_MONOTONIC != 0 {
            return sub_mono(self.ext, u.ext);
        }
        let d = Duration(
            self.sec()
                .wrapping_sub(u.sec())
                .wrapping_mul(SECOND.0)
                .wrapping_add(i64::from(self.nsec() - u.nsec())),
        );
        if u.add(d).equal(self) {
            d
        } else if self.before(u) {
            Duration::MIN
        } else {
            Duration::MAX
        }
    }

    /// Adds years, months and days, normalizing like [`date`].
    #[must_use]
    pub fn add_date(&self, years: i64, months: i64, days: i64) -> Time {
        let (year, month, day) = self.date();
        let (hour, min, sec) = self.clock();
        date(
            year.wrapping_add(years),
            Month(month.0.wrapping_add(months)),
            day.wrapping_add(days),
            hour,
            min,
            sec,
            i64::from(self.nsec()),
            &self.location(),
        )
    }

    /// t with its location set to UTC.
    #[must_use]
    pub fn utc(&self) -> Time {
        let mut t = self.clone();
        t.set_loc(&utc());
        t
    }

    /// t with its location set to [`local`].
    #[must_use]
    pub fn local(&self) -> Time {
        let mut t = self.clone();
        t.set_loc(&local());
        t
    }

    /// Go's `Time.In`: the same instant displayed in loc.
    #[must_use]
    pub fn in_location(&self, loc: &Location) -> Time {
        let mut t = self.clone();
        t.set_loc(loc);
        t
    }

    /// The location of t (UTC for the zero location).
    pub fn location(&self) -> Location {
        self.loc_or_utc()
    }

    /// The zone abbreviation and its offset in seconds east of UTC at t.
    pub fn zone(&self) -> (String, i64) {
        let loc = self.loc_or_utc();
        let z = loc.lookup(self.unix_sec());
        (z.name.to_owned(), z.offset)
    }

    /// The start and end of the zone in effect at t, as times in t's
    /// location; zero where the zone begins at the beginning of time or goes
    /// on forever.
    pub fn zone_bounds(&self) -> (Time, Time) {
        let loc = self.loc_or_utc();
        let z = loc.lookup(self.unix_sec());
        let mut start = Time::default();
        let mut end = Time::default();
        if z.start != zoneinfo::ALPHA {
            start = unix_time(z.start, 0);
            start.set_loc_raw(self.loc.clone());
        }
        if z.end != zoneinfo::OMEGA {
            end = unix_time(z.end, 0);
            end.set_loc_raw(self.loc.clone());
        }
        (start, end)
    }

    // setLoc with a possibly-nil location, as ZoneBounds calls it.
    fn set_loc_raw(&mut self, loc: Option<Location>) {
        match loc {
            Some(l) => self.set_loc(&l),
            None => {
                self.strip_mono();
                self.loc = None;
            }
        }
    }

    /// Seconds since January 1, 1970 UTC.
    pub fn unix(&self) -> i64 {
        self.unix_sec()
    }

    /// Milliseconds since January 1, 1970 UTC.
    pub fn unix_milli(&self) -> i64 {
        self.unix_sec()
            .wrapping_mul(1000)
            .wrapping_add(i64::from(self.nsec()) / 1_000_000)
    }

    /// Microseconds since January 1, 1970 UTC.
    pub fn unix_micro(&self) -> i64 {
        self.unix_sec()
            .wrapping_mul(1_000_000)
            .wrapping_add(i64::from(self.nsec()) / 1000)
    }

    /// Nanoseconds since January 1, 1970 UTC (undefined outside 1678..2262,
    /// as in Go; here it wraps).
    pub fn unix_nano(&self) -> i64 {
        self.unix_sec()
            .wrapping_mul(1_000_000_000)
            .wrapping_add(i64::from(self.nsec()))
    }

    /// Reports whether t is in daylight saving time in its location.
    pub fn is_dst(&self) -> bool {
        self.loc_or_utc().lookup(self.unix_sec()).is_dst
    }

    /// Go's `MarshalBinary`.
    pub fn marshal_binary(&self) -> Result<Vec<u8>, TimeError> {
        let mut b = Vec::with_capacity(16);
        self.append_binary(&mut b)?;
        Ok(b)
    }

    /// Go's `AppendBinary`: appends the 15- or 16-byte binary form to b.
    pub fn append_binary(&self, b: &mut Vec<u8>) -> Result<(), TimeError> {
        let mut offset_min: i16 = -1; // minutes east of UTC; -1 is UTC
        let mut offset_sec: i8 = 0;
        let mut version = TIME_BINARY_VERSION_V1;
        if !self.location().is_utc() {
            let (_, mut offset) = self.zone();
            if offset % 60 != 0 {
                version = TIME_BINARY_VERSION_V2;
                offset_sec = (offset % 60) as i8;
            }
            offset /= 60;
            if !(-32768..=32767).contains(&offset) || offset == -1 {
                return Err(TimeError(
                    "Time.MarshalBinary: unexpected zone offset".into(),
                ));
            }
            offset_min = offset as i16;
        }
        b.push(version);
        b.extend_from_slice(&self.sec().to_be_bytes());
        b.extend_from_slice(&self.nsec().to_be_bytes());
        b.extend_from_slice(&offset_min.to_be_bytes());
        if version == TIME_BINARY_VERSION_V2 {
            b.push(offset_sec as u8);
        }
        Ok(())
    }

    /// Go's `UnmarshalBinary`.
    pub fn unmarshal_binary(&mut self, data: &[u8]) -> Result<(), TimeError> {
        let Some(&version) = data.first() else {
            return Err(TimeError("Time.UnmarshalBinary: no data".into()));
        };
        if version != TIME_BINARY_VERSION_V1 && version != TIME_BINARY_VERSION_V2 {
            return Err(TimeError(
                "Time.UnmarshalBinary: unsupported version".into(),
            ));
        }
        let want_len = if version == TIME_BINARY_VERSION_V2 {
            16
        } else {
            15
        };
        if data.len() != want_len {
            return Err(TimeError("Time.UnmarshalBinary: invalid length".into()));
        }
        let be8 = |b: &[u8]| b.iter().fold(0u64, |acc, &x| acc << 8 | u64::from(x));
        let sec = be8(&data[1..9]) as i64;
        let nsec = be8(&data[9..13]) as u32 as i32;
        let mut offset = i64::from(i16::from_be_bytes([data[13], data[14]])) * 60;
        if version == TIME_BINARY_VERSION_V2 {
            offset += i64::from(data[15] as i8);
        }
        *self = Time {
            // uint64(nsec) of an int32: sign-extends, as in Go.
            wall: i64::from(nsec) as u64,
            ext: sec,
            loc: None,
        };
        if offset == -60 {
            self.set_loc(&utc());
        } else {
            let local = local();
            if local.lookup(self.unix_sec()).offset == offset {
                self.set_loc(&local);
            } else {
                self.set_loc(&fixed_zone("", offset));
            }
        }
        Ok(())
    }

    /// Rounds t down to a multiple of d since the zero time; strips the
    /// monotonic reading.
    #[must_use]
    pub fn truncate(&self, d: Duration) -> Time {
        let mut t = self.clone();
        t.strip_mono();
        if d.0 <= 0 {
            return t;
        }
        let (_, r) = div(&t, d);
        t.add(Duration(r.0.wrapping_neg()))
    }

    /// Rounds t to the nearest multiple of d since the zero time, halfway
    /// values up; strips the monotonic reading.
    #[must_use]
    pub fn round(&self, d: Duration) -> Time {
        let mut t = self.clone();
        t.strip_mono();
        if d.0 <= 0 {
            return t;
        }
        let (_, r) = div(&t, d);
        if duration::less_than_half(r, d) {
            return t.add(Duration(r.0.wrapping_neg()));
        }
        t.add(Duration(d.0.wrapping_sub(r.0)))
    }
}

const TIME_BINARY_VERSION_V1: u8 = 1;
const TIME_BINARY_VERSION_V2: u8 = 2; // for LMT only

/// An error from [`Time::marshal_binary`], [`Time::unmarshal_binary`] and the
/// RFC 3339 marshalers, with Go's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TimeError(pub(crate) String);

/// Go's `==` on `time.Time`: same wall clock, monotonic reading and location
/// identity.
impl PartialEq for Time {
    fn eq(&self, other: &Time) -> bool {
        self.wall == other.wall
            && self.ext == other.ext
            && match (&self.loc, &other.loc) {
                (None, None) => true,
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
    }
}

impl Eq for Time {}

/// `Time.String`.
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string())
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string())
    }
}

fn sub_mono(t: i64, u: i64) -> Duration {
    let d = t.wrapping_sub(u);
    if d < 0 && t > u {
        return Duration::MAX; // positive out of range
    }
    if d > 0 && t < u {
        return Duration::MIN; // negative out of range
    }
    Duration(d)
}

/// Divides t by d, returning the quotient parity and the remainder.
fn div(t: &Time, d: Duration) -> (i64, Duration) {
    let mut neg = false;
    let mut nsec = t.nsec();
    let mut sec = t.sec();
    if sec < 0 {
        // Operate on the absolute value.
        neg = true;
        sec = sec.wrapping_neg();
        nsec = -nsec;
        if nsec < 0 {
            nsec += 1_000_000_000;
            sec -= 1; // sec >= 1 before the decrement
        }
    }

    let mut qmod2;
    let mut r;
    if d.0 < SECOND.0 && SECOND.0 % (d.0 + d.0) == 0 {
        // Special case: 2d divides 1 second.
        qmod2 = i64::from(nsec / d.0 as i32) & 1;
        r = Duration(i64::from(nsec % d.0 as i32));
    } else if d.0 % SECOND.0 == 0 {
        // Special case: d is a multiple of 1 second.
        let d1 = d.0 / SECOND.0;
        qmod2 = (sec / d1) & 1;
        r = Duration((sec % d1) * SECOND.0 + i64::from(nsec));
    } else {
        // General case: 128-bit nanoseconds, then shift-and-subtract.
        let sec = sec as u64;
        let mut tmp = (sec >> 32) * 1_000_000_000;
        let mut u1 = tmp >> 32;
        let mut u0 = tmp << 32;
        tmp = (sec & 0xFFFF_FFFF) * 1_000_000_000;
        let (mut u0x, mut u0n) = (u0, u0.wrapping_add(tmp));
        u0 = u0n;
        if u0 < u0x {
            u1 += 1;
        }
        (u0x, u0n) = (u0, u0.wrapping_add(nsec as u64));
        u0 = u0n;
        if u0 < u0x {
            u1 += 1;
        }

        let mut d1 = d.0 as u64;
        while d1 >> 63 != 1 {
            d1 <<= 1;
        }
        let mut d0 = 0u64;
        loop {
            qmod2 = 0;
            if u1 > d1 || u1 == d1 && u0 >= d0 {
                // Subtract.
                qmod2 = 1;
                (u0x, u0) = (u0, u0.wrapping_sub(d0));
                if u0 > u0x {
                    u1 = u1.wrapping_sub(1);
                }
                u1 = u1.wrapping_sub(d1);
            }
            if d1 == 0 && d0 == d.0 as u64 {
                break;
            }
            d0 >>= 1;
            d0 |= (d1 & 1) << 63;
            d1 >>= 1;
        }
        r = Duration(u0 as i64);
    }

    if neg && r.0 != 0 {
        // q*d + r = -t, so -(q-1)*d + (d-r) = t.
        qmod2 ^= 1;
        r = Duration(d.0 - r.0);
    }
    (qmod2, r)
}

// The process clock. Go measures monotonic readings from startNano, taken at
// package init as runtimeNano()-1 so that no reading is 0. Rust has no package
// init: the reference instant is taken on first use, so call
// [`init_process_clock`] early in main to start it with the process.
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Starts the monotonic clock that [`now`] readings (and so the `m=+…`
/// suffix of `Time.String`) count from. Go starts it at process start; call
/// this first thing in `main`. Later calls do nothing.
pub fn init_process_clock() {
    LazyLock::force(&START);
}

fn runtime_nano() -> i64 {
    // +1 as in Go's startNano = runtimeNano() - 1.
    i64::try_from(START.elapsed().as_nanos()).unwrap_or(i64::MAX - 1) + 1
}

/// Wall clock seconds and nanoseconds since the Unix epoch, and the monotonic
/// reading.
fn runtime_now() -> (i64, i32, i64) {
    let mono = runtime_nano();
    let (sec, nsec) = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i32),
        Err(e) => {
            let d = e.duration();
            let mut sec = -(d.as_secs() as i64);
            let mut nsec = -(d.subsec_nanos() as i32);
            if nsec < 0 {
                nsec += 1_000_000_000;
                sec -= 1;
            }
            (sec, nsec)
        }
    };
    (sec, nsec, mono)
}

/// The current local time, with a monotonic clock reading.
pub fn now() -> Time {
    let (sec, nsec, mono) = runtime_now();
    let sec = sec + UNIX_TO_INTERNAL - MIN_WALL;
    if (sec as u64) >> 33 != 0 {
        // The seconds field overflows the 33 bits a monotonic time has
        // (after March 16, 2157).
        return Time {
            wall: nsec as u64,
            ext: sec + MIN_WALL,
            loc: Some(local()),
        };
    }
    Time {
        wall: HAS_MONOTONIC | (sec as u64) << NSEC_SHIFT | nsec as u64,
        ext: mono,
        loc: Some(local()),
    }
}

/// The time elapsed since t.
pub fn since(t: &Time) -> Duration {
    if t.wall & HAS_MONOTONIC != 0 {
        return sub_mono(runtime_nano(), t.ext);
    }
    now().sub(t)
}

/// The duration until t.
pub fn until(t: &Time) -> Duration {
    if t.wall & HAS_MONOTONIC != 0 {
        return sub_mono(t.ext, runtime_nano());
    }
    t.sub(&now())
}

fn unix_time(sec: i64, nsec: i32) -> Time {
    Time {
        wall: i64::from(nsec) as u64,
        ext: sec.wrapping_add(UNIX_TO_INTERNAL),
        loc: Some(local()),
    }
}

/// The local time for `sec` seconds and `nsec` nanoseconds since January 1,
/// 1970 UTC; nsec may be outside [0, 999999999].
pub fn unix(mut sec: i64, mut nsec: i64) -> Time {
    if !(0..1_000_000_000).contains(&nsec) {
        let n = nsec / 1_000_000_000;
        sec = sec.wrapping_add(n);
        nsec -= n * 1_000_000_000;
        if nsec < 0 {
            nsec += 1_000_000_000;
            sec = sec.wrapping_sub(1);
        }
    }
    unix_time(sec, nsec as i32)
}

/// The local time for `msec` milliseconds since January 1, 1970 UTC.
pub fn unix_milli(msec: i64) -> Time {
    unix(msec / 1000, (msec % 1000) * 1_000_000)
}

/// The local time for `usec` microseconds since January 1, 1970 UTC.
pub fn unix_micro(usec: i64) -> Time {
    unix(usec / 1_000_000, (usec % 1_000_000) * 1000)
}

/// Go's `time.Date`: the time for yyyy-mm-dd hh:mm:ss + nsec in loc. Values
/// outside their usual ranges are normalized (October 32 is November 1). In a
/// skipped or repeated hour the result is correct in one of the two zones,
/// whichever Go picks.
#[expect(clippy::too_many_arguments, reason = "mirrors Go's time.Date")]
pub fn date(
    year: i64,
    month: Month,
    day: i64,
    hour: i64,
    min: i64,
    sec: i64,
    nsec: i64,
    loc: &Location,
) -> Time {
    // Normalize month, overflowing into year.
    let (year, m) = norm(year, month.0.wrapping_sub(1), 12);
    let month = Month(m + 1);

    // Normalize nsec, sec, min, hour, overflowing into day.
    let (sec, nsec) = norm(sec, nsec, 1_000_000_000);
    let (min, sec) = norm(min, sec, 60);
    let (hour, min) = norm(hour, min, 60);
    let (day, hour) = norm(day, hour, 24);

    // Convert to absolute time and then Unix time.
    let mut unix = (date_to_abs_days(year, month, day) as i64)
        .wrapping_mul(SECONDS_PER_DAY)
        .wrapping_add(
            hour.wrapping_mul(SECONDS_PER_HOUR)
                .wrapping_add(min.wrapping_mul(SECONDS_PER_MINUTE))
                .wrapping_add(sec),
        )
        .wrapping_add(ABSOLUTE_TO_UNIX);

    // Look up the offset for the expected time, then adjust if unix minus that
    // offset falls outside the zone found.
    let z = loc.lookup(unix);
    let mut offset = z.offset;
    if offset != 0 {
        let utc = unix.wrapping_sub(offset);
        if utc < z.start || utc >= z.end {
            offset = loc.lookup(utc).offset;
        }
        unix = unix.wrapping_sub(offset);
    }

    let mut t = unix_time(unix, nsec as i32);
    t.set_loc(loc);
    t
}

/// Go's `utf8.DecodeRune`: the first rune of s and its width; `(U+FFFD, 1)`
/// for an invalid byte and `(U+FFFD, 0)` for empty input.
pub(crate) fn decode_rune(s: &[u8]) -> (u32, usize) {
    if s.is_empty() {
        return (0xFFFD, 0);
    }
    // Rust and Go agree on what is valid UTF-8 (no surrogates, no overlong
    // forms, nothing past U+10FFFF); Go skips one byte of an invalid sequence.
    match s
        .utf8_chunks()
        .next()
        .and_then(|c| c.valid().chars().next())
    {
        Some(c) => (u32::from(c), c.len_utf8()),
        None => (0xFFFD, 1),
    }
}

/// The raw wall/ext words of t and whether it has a location, for the ports of
/// Go's internal tests.
pub fn time_parts(t: &Time) -> (u64, i64, Option<Location>) {
    (t.wall, t.ext, t.loc.clone())
}

/// Builds a Time from raw wall/ext words, as Go's internal tests do with
/// struct literals.
pub fn time_from_parts(wall: u64, ext: i64, loc: Option<Location>) -> Time {
    Time { wall, ext, loc }
}
