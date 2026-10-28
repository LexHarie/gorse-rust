// Ports: storage/data/sql.go (normalizeTimestampForWrite, convertTimeZone and the
// time columns the three drivers scan) and the time handling of the Go drivers
// they use: go-sql-driver/mysql, lib/pq and modernc.org/sqlite @ add21b5

//! How the SQL data store writes and reads a time, per dialect.
//!
//! The Go drivers convert a `time.Time` in three different ways, and the rows
//! a Rust node writes must read back the same in a Go node:
//!
//! - MySQL: go-sql-driver writes the time in UTC, with microseconds, and
//!   `normalizeTimestampForWrite` has already cut `time_stamp` to whole seconds.
//! - PostgreSQL: lib/pq writes the nanoseconds and the server rounds them to
//!   microseconds.
//! - SQLite: modernc writes `t.String()` of the UTC time, which is
//!   `2006-01-02 15:04:05.999999999 -0700 MST`, and reads that form back.
//!
//! Reads are text on every dialect: MySQL and PostgreSQL select their times
//! through [`select`], so [`parse`] reads all three.

use std::sync::Arc;

use gorse_common::gotime::{self, Duration, MICROSECOND, Month, SECOND, Time};

use super::super::Error;
use super::query::Driver;

/// The SQL that reads the time column `column` as text on `driver`, in UTC,
/// with its fraction. SQLite stores the text itself.
pub fn select(driver: Driver, column: &str) -> String {
    match driver {
        Driver::MySql => format!("DATE_FORMAT({column}, '%Y-%m-%d %H:%i:%s.%f')"),
        Driver::Postgres => {
            format!("to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.US')")
        }
        Driver::Sqlite => column.to_owned(),
    }
}

/// Go's `normalizeTimestampForWrite`: the time a store keeps for a `time_stamp`
/// it is given. MySQL keeps whole seconds, SQLite keeps UTC, PostgreSQL keeps
/// the time as it is.
pub fn normalize_for_write(driver: Driver, t: &Time) -> Time {
    match driver {
        Driver::MySql => t.truncate(SECOND),
        Driver::Postgres => t.clone(),
        Driver::Sqlite => t.utc(),
    }
}

/// The text a store binds for `t`, which the driver converts to the column's
/// type. MySQL cuts to microseconds, PostgreSQL rounds to them, and SQLite
/// writes Go's `t.String()` in UTC.
pub fn format(driver: Driver, t: &Time) -> String {
    match driver {
        Driver::MySql => {
            let t = t.utc().truncate(MICROSECOND);
            with_micros(&t, t.nanosecond() / 1000, "")
        }
        Driver::Postgres => {
            let t = t.utc().round(MICROSECOND);
            with_micros(&t, t.nanosecond() / 1000, "+00")
        }
        Driver::Sqlite => {
            let t = t.utc();
            let mut text = clock(&t);
            let nanos = t.nanosecond();
            if nanos != 0 {
                let digits = format!("{nanos:09}");
                text.push('.');
                text.push_str(digits.trim_end_matches('0'));
            }
            text.push_str(" +0000 UTC");
            text
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS` of a UTC time.
fn clock(t: &Time) -> String {
    let (year, month, day) = t.date();
    let (hour, min, sec) = t.clock();
    format!(
        "{year:04}-{:02}-{day:02} {hour:02}:{min:02}:{sec:02}",
        month.0
    )
}

/// `clock` with the microseconds, when there are any, and a zone suffix.
fn with_micros(t: &Time, micros: i64, zone: &str) -> String {
    let mut text = clock(t);
    if micros != 0 {
        text.push_str(&format!(".{micros:06}"));
    }
    text.push_str(zone);
    text
}

/// The text of a time a store could not read.
#[derive(Debug, thiserror::Error)]
#[error("unrecognised time {0:?}")]
struct ParseTimeError(String);

/// Reads a time as a store returns it, in any form the Go drivers accept:
/// Go's `t.String()` (with an optional monotonic reading, which is dropped),
/// the RFC 3339 forms with `T` or a space, a bare date, and the forms without
/// a zone, which are UTC. Go's `time.Parse` decides the zone-less forms, and
/// so does this.
pub fn parse(text: &str) -> Result<Time, Error> {
    parse_text(text).ok_or_else(|| Error::Backend(Arc::new(ParseTimeError(text.to_owned()))))
}

/// The byte cursor [`parse_text`] reads a time with.
struct Scan<'a> {
    text: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    /// Exactly `n` digits, as a number.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let digits = self.text.get(self.at..self.at + n)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at += n;
        std::str::from_utf8(digits).ok()?.parse().ok()
    }

    /// Consumes `byte` when it is next.
    fn eat(&mut self, byte: u8) -> bool {
        if self.text.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// Whether the text is used up.
    fn done(&self) -> bool {
        self.at == self.text.len()
    }

    /// The next byte, if any.
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }
}

/// [`parse`] without the error: `None` when the text has no time form.
fn parse_text(text: &str) -> Option<Time> {
    let text = text.split(" m=").next()?;
    let mut scan = Scan {
        text: text.as_bytes(),
        at: 0,
    };
    let year = scan.digits(4)?;
    if !scan.eat(b'-') {
        return None;
    }
    let month = scan.digits(2)?;
    if !scan.eat(b'-') {
        return None;
    }
    let day = scan.digits(2)?;
    let (mut hour, mut min, mut sec, mut nanos) = (0, 0, 0, 0);
    let mut offset_minutes = 0;
    if !scan.done() {
        if !(scan.eat(b' ') || scan.eat(b'T')) {
            return None;
        }
        hour = scan.digits(2)?;
        if !scan.eat(b':') {
            return None;
        }
        min = scan.digits(2)?;
        if scan.eat(b':') {
            sec = scan.digits(2)?;
        }
        if scan.eat(b'.') {
            let start = scan.at;
            while scan.peek().is_some_and(|b| b.is_ascii_digit()) && scan.at - start < 9 {
                scan.at += 1;
            }
            let digits = &text[start..scan.at];
            if digits.is_empty() {
                return None;
            }
            nanos = format!("{digits:0<9}").parse().ok()?;
        }
        if scan.eat(b'Z') {
            // UTC.
        } else if matches!(scan.peek(), Some(b' ' | b'+' | b'-')) {
            scan.eat(b' ');
            let negative = match scan.peek()? {
                b'+' => false,
                b'-' => true,
                _ => return None,
            };
            scan.at += 1;
            let zone_hours = scan.digits(2)?;
            scan.eat(b':');
            let zone_minutes = scan.digits(2)?;
            offset_minutes = zone_hours * 60 + zone_minutes;
            if negative {
                offset_minutes = -offset_minutes;
            }
            if scan.eat(b' ') {
                // The zone name, such as `UTC`.
                let start = scan.at;
                while scan.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                    scan.at += 1;
                }
                if scan.at == start {
                    return None;
                }
            }
        }
    }
    if !scan.done() {
        return None;
    }
    if !(1..=12).contains(&month)
        || !(1..=gotime::days_in(Month(month), year)).contains(&day)
        || hour > 23
        || min > 59
        || sec > 59
    {
        return None;
    }
    let t = gotime::date(
        year,
        Month(month),
        day,
        hour,
        min,
        sec,
        nanos,
        &gotime::utc(),
    );
    Some(t.add(Duration(-offset_minutes * 60 * SECOND.0)))
}
