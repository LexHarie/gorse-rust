// Ports: $GOROOT/src/time/zoneinfo.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package: Copyright 2011 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! [`Location`]: the zones and transitions of a time zone, their lookup, the
//! POSIX TZ ("tzset") rules that extend them, and Go's `UTC`, `Local` and
//! `FixedZone`.

use std::fmt;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use super::{
    INTERNAL_TO_ABSOLUTE, Month, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE,
    UNIX_TO_INTERNAL, abs_days, abs_year_yday, days_before, days_in, is_leap,
};

/// A Go `*time.Location`: maps instants to the zone in use at that time.
///
/// Cloning shares the location; `==` compares identity, as Go compares
/// `*Location` pointers. Display shows the name (`Location.String`).
#[derive(Clone)]
pub struct Location(Arc<LocationData>);

#[derive(Clone)]
pub(crate) struct LocationData {
    pub(crate) name: String,
    pub(crate) zone: Vec<Zone>,
    pub(crate) tx: Vec<ZoneTrans>,
    // The TZ string (without a colon) that describes transitions after the
    // last one in tx, e.g. "PST8PDT,M3.2.0,M11.1.0".
    pub(crate) extend: String,
    // A one-element cache of the zone in effect when the location was loaded:
    // if cache_start <= sec < cache_end, lookup returns cache_zone.
    pub(crate) cache_start: i64,
    pub(crate) cache_end: i64,
    pub(crate) cache_zone: Option<Zone>,
}

/// A single time zone such as CET.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Zone {
    pub(crate) name: String, // abbreviated name, "CET"
    pub(crate) offset: i64,  // seconds east of UTC
    pub(crate) is_dst: bool, // is this zone Daylight Savings Time?
}

/// A single time zone transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ZoneTrans {
    pub(crate) when: i64, // transition time, in seconds since 1970 GMT
    pub(crate) index: u8, // the index of the zone that goes into effect at that time
    pub(crate) isstd: bool,
    pub(crate) isutc: bool,
}

/// The beginning of time for zone transitions.
pub(crate) const ALPHA: i64 = i64::MIN;
/// The end of time for zone transitions.
pub(crate) const OMEGA: i64 = i64::MAX;

/// The zone in effect at an instant, as Go's `lookup` returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ZoneInfo<'a> {
    pub(crate) name: &'a str,
    pub(crate) offset: i64,
    pub(crate) start: i64,
    pub(crate) end: i64,
    pub(crate) is_dst: bool,
}

static UTC_LOC: LazyLock<Location> = LazyLock::new(|| {
    Location::from_data(LocationData {
        name: "UTC".into(),
        zone: Vec::new(),
        tx: Vec::new(),
        extend: String::new(),
        cache_start: 0,
        cache_end: 0,
        cache_zone: None,
    })
});

/// Go's `time.UTC`.
pub fn utc() -> Location {
    UTC_LOC.clone()
}

// Go's `time.Local` variable; None until first use, when initLocal fills it
// from $TZ or /etc/localtime.
static LOCAL: RwLock<Option<Location>> = RwLock::new(None);

/// Go's `time.Local`: the system's local time zone ($TZ, else
/// /etc/localtime, else UTC named "UTC"), or what [`set_local`] last set.
pub fn local() -> Location {
    if let Some(l) = LOCAL
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        return l.clone();
    }
    let mut w = LOCAL.write().unwrap_or_else(PoisonError::into_inner);
    if let Some(l) = w.as_ref() {
        return l.clone();
    }
    let l = super::zoneinfo_read::init_local();
    *w = Some(l.clone());
    l
}

/// Assigns Go's `time.Local` variable (`time.Local = loc`). Times made
/// earlier keep the location they had.
pub fn set_local(loc: Location) {
    *LOCAL.write().unwrap_or_else(PoisonError::into_inner) = Some(loc);
}

/// Forgets `time.Local`, so the next [`local`] reads $TZ again (Go's
/// `ResetLocalOnceForTest`). For tests.
#[doc(hidden)]
pub fn reset_local() {
    *LOCAL.write().unwrap_or_else(PoisonError::into_inner) = None;
}

impl Location {
    pub(crate) fn from_data(data: LocationData) -> Location {
        Location(Arc::new(data))
    }

    /// The name: the argument to [`super::load_location`] or [`fixed_zone`],
    /// "UTC", or "Local" (Go's `Location.String`).
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// A new location with the same zones and another name, as Go does with
    /// `localLoc = *z; localLoc.name = "Local"`.
    #[doc(hidden)]
    #[must_use]
    pub fn renamed(&self, name: &str) -> Location {
        let mut data = (*self.0).clone();
        data.name = name.to_owned();
        Location::from_data(data)
    }

    /// Reports whether this is [`utc`] itself (Go's `loc == &utcLoc`); a
    /// fixed zone named "UTC" is not.
    pub fn is_utc(&self) -> bool {
        Arc::ptr_eq(&self.0, &UTC_LOC.0)
    }

    /// Reports whether two locations hold the same zone data and name (Go's
    /// `reflect.DeepEqual` on two `*Location`s, which the tests use).
    #[doc(hidden)]
    pub fn deep_equal(&self, other: &Location) -> bool {
        let (a, b) = (&*self.0, &*other.0);
        a.name == b.name
            && a.zone == b.zone
            && a.tx == b.tx
            && a.extend == b.extend
            && a.cache_start == b.cache_start
            && a.cache_end == b.cache_end
            && a.cache_zone == b.cache_zone
    }

    /// The zone in use at `sec` seconds since January 1, 1970 UTC: its name,
    /// offset, the start and end of the period it is in effect, and whether
    /// it is daylight saving time.
    pub(crate) fn lookup(&self, sec: i64) -> ZoneInfo<'_> {
        let l = &*self.0;

        if l.zone.is_empty() {
            return ZoneInfo {
                name: "UTC",
                offset: 0,
                start: ALPHA,
                end: OMEGA,
                is_dst: false,
            };
        }

        if let Some(zone) = &l.cache_zone
            && l.cache_start <= sec
            && sec < l.cache_end
        {
            return ZoneInfo {
                name: &zone.name,
                offset: zone.offset,
                start: l.cache_start,
                end: l.cache_end,
                is_dst: zone.is_dst,
            };
        }

        if l.tx.is_empty() || sec < l.tx[0].when {
            let zone = &l.zone[self.lookup_first_zone()];
            return ZoneInfo {
                name: &zone.name,
                offset: zone.offset,
                start: ALPHA,
                end: l.tx.first().map_or(OMEGA, |t| t.when),
                is_dst: zone.is_dst,
            };
        }

        // Binary search for entry with largest time <= sec.
        let tx = &l.tx;
        let mut end = OMEGA;
        let mut lo = 0;
        let mut hi = tx.len();
        while hi - lo > 1 {
            let m = (lo + hi) >> 1;
            let lim = tx[m].when;
            if sec < lim {
                end = lim;
                hi = m;
            } else {
                lo = m;
            }
        }
        let zone = &l.zone[tx[lo].index as usize];
        let start = tx[lo].when;

        // If we're at the end of the known zone transitions, try the extend
        // string.
        if lo == tx.len() - 1 && !l.extend.is_empty() {
            let (ename, eoffset, estart, eend, eis_dst, ok) = tzset(&l.extend, start, sec);
            if ok {
                return ZoneInfo {
                    name: ename,
                    offset: eoffset,
                    start: estart,
                    end: eend,
                    is_dst: eis_dst,
                };
            }
        }

        ZoneInfo {
            name: &zone.name,
            offset: zone.offset,
            start,
            end,
            is_dst: zone.is_dst,
        }
    }

    /// The index of the zone to use before the first transition, or when there
    /// are none, following localtime.c from tzcode2013g:
    ///  1. If the first zone is unused by the transitions, use it.
    ///  2. Otherwise, if the first transition is to a daylight zone, the
    ///     closest earlier non-daylight zone.
    ///  3. Otherwise, the first non-daylight zone.
    ///  4. Otherwise, the first zone.
    fn lookup_first_zone(&self) -> usize {
        let l = &*self.0;
        // Case 1.
        if !self.first_zone_used() {
            return 0;
        }

        // Case 2.
        if let Some(t0) = l.tx.first()
            && l.zone[t0.index as usize].is_dst
        {
            for zi in (0..t0.index as usize).rev() {
                if !l.zone[zi].is_dst {
                    return zi;
                }
            }
        }

        // Case 3.
        if let Some(zi) = l.zone.iter().position(|z| !z.is_dst) {
            return zi;
        }

        // Case 4.
        0
    }

    /// Reports whether the first zone is used by some transition.
    fn first_zone_used(&self) -> bool {
        self.0.tx.iter().any(|tx| tx.index == 0)
    }

    /// The offset of the zone named `name` (such as "EST") at the pseudo-Unix
    /// time `unix` (what the time of day would be in UTC); preferring a zone
    /// of that name actually in effect then.
    pub(crate) fn lookup_name(&self, name: &[u8], unix: i64) -> Option<i64> {
        let l = &*self.0;

        // First try for a zone with the right name that was actually in effect
        // at the given time. (In Sydney, Australia, both standard and
        // daylight-savings time are abbreviated "EST". Using the offset helps
        // us pick the right one for the given time. It's not perfect: during
        // the backward transition we might pick either one.)
        for zone in &l.zone {
            if zone.name.as_bytes() == name {
                let z = self.lookup(unix.wrapping_sub(zone.offset));
                if z.name == zone.name {
                    return Some(z.offset);
                }
            }
        }

        // Otherwise fall back to an ordinary name match.
        l.zone
            .iter()
            .find(|zone| zone.name.as_bytes() == name)
            .map(|zone| zone.offset)
    }
}

/// Location identity, as Go compares `*Location`.
impl PartialEq for Location {
    fn eq(&self, other: &Location) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Location {}

/// `Location.String`.
impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Debug for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Location({:?})", self.name())
    }
}

const HOURS_BEFORE_UTC: i64 = 12;
const HOURS_AFTER_UTC: i64 = 14;

// Most calls to FixedZone have an unnamed zone with an offset by the hour;
// Go returns the same *Location for a given hour.
static UNNAMED_FIXED_ZONES: LazyLock<Vec<Location>> = LazyLock::new(|| {
    (-HOURS_BEFORE_UTC..=HOURS_AFTER_UTC)
        .map(|hr| new_fixed_zone("", hr * 60 * 60))
        .collect()
});

/// Go's `time.FixedZone`: a location that always uses the given zone name and
/// offset (seconds east of UTC).
pub fn fixed_zone(name: &str, offset: i64) -> Location {
    let hour = offset / 60 / 60;
    if name.is_empty()
        && (-HOURS_BEFORE_UTC..=HOURS_AFTER_UTC).contains(&hour)
        && hour * 60 * 60 == offset
    {
        return UNNAMED_FIXED_ZONES[(hour + HOURS_BEFORE_UTC) as usize].clone();
    }
    new_fixed_zone(name, offset)
}

fn new_fixed_zone(name: &str, offset: i64) -> Location {
    let zone = Zone {
        name: name.to_owned(),
        offset,
        is_dst: false,
    };
    Location::from_data(LocationData {
        name: name.to_owned(),
        zone: vec![zone.clone()],
        tx: vec![ZoneTrans {
            when: ALPHA,
            index: 0,
            isstd: false,
            isutc: false,
        }],
        extend: String::new(),
        cache_start: ALPHA,
        cache_end: OMEGA,
        cache_zone: Some(zone),
    })
}

/// Go's `tzset`: given a TZ string such as "PST8PDT,M3.2.0,M11.1.0", the time
/// of the last zone transition and an instant (both seconds since 1970 UTC),
/// the zone name, offset, start, end and DST flag at the instant, and whether
/// the string parsed.
pub fn tzset(s: &str, last_tx_sec: i64, sec: i64) -> (&str, i64, i64, i64, bool, bool) {
    const FAIL: (&str, i64, i64, i64, bool, bool) = ("", 0, 0, 0, false, false);

    let (mut std_name, s, ok) = tzset_name(s);
    if !ok {
        return FAIL;
    }
    let (mut std_offset, mut s, ok) = tzset_offset(s);
    if !ok {
        return FAIL;
    }

    // The numbers in the tzset string are added to local time to get UTC, but
    // our offsets are added to UTC to get local time, so we negate the number
    // we see here.
    std_offset = -std_offset;

    if s.is_empty() || s.as_bytes()[0] == b',' {
        // No daylight savings time.
        return (std_name, std_offset, last_tx_sec, OMEGA, false, true);
    }

    let (mut dst_name, rest, ok) = tzset_name(s);
    if !ok {
        return FAIL;
    }
    s = rest;
    let mut dst_offset;
    if s.is_empty() || s.as_bytes()[0] == b',' {
        dst_offset = std_offset + SECONDS_PER_HOUR;
    } else {
        let (off, rest, ok) = tzset_offset(s);
        if !ok {
            return FAIL;
        }
        s = rest;
        dst_offset = -off; // as with std_offset, above
    }

    if s.is_empty() {
        // Default DST rules per tzcode.
        s = ",M3.2.0,M11.1.0";
    }
    // The TZ definition does not mention ';' here but tzcode accepts it.
    if s.as_bytes()[0] != b',' && s.as_bytes()[0] != b';' {
        return FAIL;
    }
    s = &s[1..];

    let (start_rule, rest, ok) = tzset_rule(s);
    if !ok || rest.is_empty() || rest.as_bytes()[0] != b',' {
        return FAIL;
    }
    let (end_rule, rest, ok) = tzset_rule(&rest[1..]);
    if !ok || !rest.is_empty() {
        return FAIL;
    }

    // Compute start of year in seconds since Unix epoch, and seconds since
    // then to get to sec.
    let (year, yday) = abs_year_yday(abs_days(
        sec.wrapping_add(UNIX_TO_INTERNAL + INTERNAL_TO_ABSOLUTE) as u64,
    ));
    let ysec = (yday - 1) * SECONDS_PER_DAY + sec % SECONDS_PER_DAY;
    let ystart = sec - ysec;

    let mut start_sec = tzrule_time(year, &start_rule, std_offset);
    let mut end_sec = tzrule_time(year, &end_rule, dst_offset);
    let (mut dst_is_dst, mut std_is_dst) = (true, false);
    // Note: this is a flipping of "DST" and "STD" while retaining the labels.
    // This happens in southern hemispheres. The labelling here thus is a
    // little inconsistent with the goal.
    if end_sec < start_sec {
        std::mem::swap(&mut start_sec, &mut end_sec);
        std::mem::swap(&mut std_name, &mut dst_name);
        std::mem::swap(&mut std_offset, &mut dst_offset);
        std::mem::swap(&mut std_is_dst, &mut dst_is_dst);
    }

    // The start and end values that we return are accurate close to a
    // daylight savings transition, but are otherwise just the start and end
    // of the year. That suffices for the only caller that cares, which is
    // Date.
    if ysec < start_sec {
        (
            std_name,
            std_offset,
            ystart,
            start_sec + ystart,
            std_is_dst,
            true,
        )
    } else if ysec >= end_sec {
        (
            std_name,
            std_offset,
            end_sec + ystart,
            ystart + 365 * SECONDS_PER_DAY,
            std_is_dst,
            true,
        )
    } else {
        (
            dst_name,
            dst_offset,
            start_sec + ystart,
            end_sec + ystart,
            dst_is_dst,
            true,
        )
    }
}

/// Go's `tzsetName`: the zone name at the start of a TZ string, the rest, and
/// whether it parsed.
pub fn tzset_name(s: &str) -> (&str, &str, bool) {
    if s.is_empty() {
        return ("", "", false);
    }
    let b = s.as_bytes();
    if b[0] != b'<' {
        // Go ranges over runes, but every byte it matches is ASCII, so byte
        // offsets agree.
        for (i, &r) in b.iter().enumerate() {
            if matches!(r, b'0'..=b'9' | b',' | b'-' | b'+') {
                if i < 3 {
                    return ("", "", false);
                }
                return (&s[..i], &s[i..], true);
            }
        }
        if s.len() < 3 {
            return ("", "", false);
        }
        (s, "", true)
    } else {
        match b.iter().position(|&r| r == b'>') {
            Some(i) => (&s[1..i], &s[i + 1..], true),
            None => ("", "", false),
        }
    }
}

/// Go's `tzsetOffset`: the offset in seconds at the start of a TZ string
/// ("-5", "+05:30:15"), the rest, and whether it parsed.
pub fn tzset_offset(s: &str) -> (i64, &str, bool) {
    let mut s = s;
    if s.is_empty() {
        return (0, "", false);
    }
    let mut neg = false;
    if s.as_bytes()[0] == b'+' {
        s = &s[1..];
    } else if s.as_bytes()[0] == b'-' {
        s = &s[1..];
        neg = true;
    }

    // The tzdata code permits values up to 24 * 7 here, although POSIX does
    // not.
    let (hours, rest, ok) = tzset_num(s, 0, 24 * 7);
    if !ok {
        return (0, "", false);
    }
    s = rest;
    let mut off = hours * SECONDS_PER_HOUR;
    if s.is_empty() || s.as_bytes()[0] != b':' {
        if neg {
            off = -off;
        }
        return (off, s, true);
    }

    let (mins, rest, ok) = tzset_num(&s[1..], 0, 59);
    if !ok {
        return (0, "", false);
    }
    s = rest;
    off += mins * SECONDS_PER_MINUTE;
    if s.is_empty() || s.as_bytes()[0] != b':' {
        if neg {
            off = -off;
        }
        return (off, s, true);
    }

    let (secs, rest, ok) = tzset_num(&s[1..], 0, 59);
    if !ok {
        return (0, "", false);
    }
    s = rest;
    off += secs;

    if neg {
        off = -off;
    }
    (off, s, true)
}

/// The kinds of rules in a TZ string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuleKind {
    /// `Jn`: Julian day 1..365, February 29 never counted.
    #[default]
    Julian,
    /// `n`: zero-based day of year 0..365.
    Doy,
    /// `Mm.w.d`: day d of week w of month m.
    MonthWeekDay,
}

/// A transition rule read from a TZ string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    /// The kind of rule.
    pub kind: RuleKind,
    /// The day (Julian, day of year, or day of week).
    pub day: i64,
    /// The week of the month, for `MonthWeekDay`.
    pub week: i64,
    /// The month, for `MonthWeekDay`.
    pub mon: i64,
    /// The transition time, seconds after local midnight.
    pub time: i64,
}

/// Go's `tzsetRule`: a rule from a TZ string, the rest, and whether it
/// parsed.
pub fn tzset_rule(s: &str) -> (Rule, &str, bool) {
    const FAIL: (Rule, &str, bool) = (
        Rule {
            kind: RuleKind::Julian,
            day: 0,
            week: 0,
            mon: 0,
            time: 0,
        },
        "",
        false,
    );
    let mut r = Rule::default();
    let mut s = s;
    if s.is_empty() {
        return FAIL;
    }
    if s.as_bytes()[0] == b'J' {
        let (jday, rest, ok) = tzset_num(&s[1..], 1, 365);
        if !ok {
            return FAIL;
        }
        s = rest;
        r.kind = RuleKind::Julian;
        r.day = jday;
    } else if s.as_bytes()[0] == b'M' {
        let (mon, rest, ok) = tzset_num(&s[1..], 1, 12);
        if !ok || rest.is_empty() || rest.as_bytes()[0] != b'.' {
            return FAIL;
        }
        let (week, rest, ok) = tzset_num(&rest[1..], 1, 5);
        if !ok || rest.is_empty() || rest.as_bytes()[0] != b'.' {
            return FAIL;
        }
        let (day, rest, ok) = tzset_num(&rest[1..], 0, 6);
        if !ok {
            return FAIL;
        }
        s = rest;
        r.kind = RuleKind::MonthWeekDay;
        r.day = day;
        r.week = week;
        r.mon = mon;
    } else {
        let (day, rest, ok) = tzset_num(s, 0, 365);
        if !ok {
            return FAIL;
        }
        s = rest;
        r.kind = RuleKind::Doy;
        r.day = day;
    }

    if s.is_empty() || s.as_bytes()[0] != b'/' {
        r.time = 2 * SECONDS_PER_HOUR; // 2am is the default
        return (r, s, true);
    }

    let (offset, s, ok) = tzset_offset(&s[1..]);
    if !ok {
        return FAIL;
    }
    r.time = offset;

    (r, s, true)
}

/// Go's `tzsetNum`: a decimal number in [min, max] at the start of s, the
/// rest, and whether it parsed.
fn tzset_num(s: &str, min: i64, max: i64) -> (i64, &str, bool) {
    if s.is_empty() {
        return (0, "", false);
    }
    let mut num = 0;
    for (i, &r) in s.as_bytes().iter().enumerate() {
        if !r.is_ascii_digit() {
            if i == 0 || num < min {
                return (0, "", false);
            }
            return (num, &s[i..], true);
        }
        num *= 10;
        num += i64::from(r - b'0');
        if num > max {
            return (0, "", false);
        }
    }
    if num < min {
        return (0, "", false);
    }
    (num, "", true)
}

/// Seconds since the start of `year` at which rule r takes effect, in a zone
/// `off` seconds east of UTC.
fn tzrule_time(year: i64, r: &Rule, off: i64) -> i64 {
    let s = match r.kind {
        RuleKind::Julian => {
            let mut s = (r.day - 1) * SECONDS_PER_DAY;
            if is_leap(year) && r.day >= 60 {
                s += SECONDS_PER_DAY;
            }
            s
        }
        RuleKind::Doy => r.day * SECONDS_PER_DAY,
        RuleKind::MonthWeekDay => {
            // Zeller's Congruence.
            let m1 = (r.mon + 9) % 12 + 1;
            let mut yy0 = year;
            if r.mon <= 2 {
                yy0 -= 1;
            }
            let yy1 = yy0 / 100;
            let yy2 = yy0 % 100;
            let mut dow = ((26 * m1 - 2) / 10 + 1 + yy2 + yy2 / 4 + yy1 / 4 - 2 * yy1) % 7;
            if dow < 0 {
                dow += 7;
            }
            // Now dow is the day-of-week of the first day of r.mon. Get the
            // day-of-month of the first "dow" day.
            let mut d = r.day - dow;
            if d < 0 {
                d += 7;
            }
            for _ in 1..r.week {
                if d + 7 >= days_in(Month(r.mon), year) {
                    break;
                }
                d += 7;
            }
            d += days_before(Month(r.mon));
            if is_leap(year) && r.mon > 2 {
                d += 1;
            }
            d * SECONDS_PER_DAY
        }
    };
    s + r.time - off
}

/// Finds the zone in zones equal to (name, offset, is_dst).
pub(crate) fn find_zone(zones: &[Zone], name: &str, offset: i64, is_dst: bool) -> Option<usize> {
    zones
        .iter()
        .position(|z| z.name == name && z.offset == offset && z.is_dst == is_dst)
}
