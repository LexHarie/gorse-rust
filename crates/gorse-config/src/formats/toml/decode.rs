// Ports: decode.go @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! The values of scalar nodes: integers in four bases, floats, and the four
//! kinds of date and time. The parser only scanned these; this is where they
//! are checked, so their errors come when the decoder reaches the value.

use std::borrow::Cow;

use gorse_common::gostrconv;
use gorse_common::gotime::{self, Month, Time};

use super::errors::{ParserError, new_parser_error};
use crate::value::{LocalDate, LocalTime};

/// `parseInteger`.
pub(crate) fn parse_integer(b: &[u8]) -> Result<i64, ParserError<'_>> {
    if b.len() > 2 && b[0] == b'0' {
        match b[1] {
            b'x' => return parse_int_hex(b),
            b'b' => return parse_int_bin(b),
            b'o' => return parse_int_oct(b),
            c => panic!(
                "invalid base '{}', should have been checked by scanIntOrFloat",
                char::from(c)
            ),
        }
    }

    parse_int_dec(b)
}

/// `parseLocalDate`: `YYYY-MM-DD`, a date that exists.
pub(crate) fn parse_local_date(b: &[u8]) -> Result<LocalDate, ParserError<'_>> {
    // full-date      = date-fullyear "-" date-month "-" date-mday
    // date-fullyear  = 4DIGIT
    // date-month     = 2DIGIT  ; 01-12
    // date-mday      = 2DIGIT  ; 01-28, 01-29, 01-30, 01-31 based on month/year
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return Err(new_parser_error(
            b,
            "dates are expected to have the format YYYY-MM-DD",
        ));
    }

    let date = LocalDate {
        year: parse_decimal_digits(&b[0..4])?,
        month: parse_decimal_digits(&b[5..7])?,
        day: parse_decimal_digits(&b[8..10])?,
    };

    if !is_valid_date(date.year, date.month, date.day) {
        return Err(new_parser_error(b, "impossible date"));
    }

    Ok(date)
}

fn parse_decimal_digits(b: &[u8]) -> Result<i64, ParserError<'_>> {
    let mut v = 0;

    for (i, &c) in b.iter().enumerate() {
        if !c.is_ascii_digit() {
            return Err(new_parser_error(&b[i..i + 1], "expected digit (0-9)"));
        }
        v *= 10;
        v += i64::from(c - b'0');
    }

    Ok(v)
}

/// `parseDateTime`: an offset date-time, as `time.Date` in UTC (for `Z` and
/// a zero offset) or in `time.FixedZone("", offset)`.
pub(crate) fn parse_date_time(b: &[u8]) -> Result<Time, ParserError<'_>> {
    // offset-date-time = full-date time-delim full-time
    // full-time      = partial-time time-offset
    // time-offset    = "Z" / time-numoffset
    // time-numoffset = ( "+" / "-" ) time-hour ":" time-minute

    let (date, time, mut b) = parse_local_date_time(b)?;

    if b.is_empty() {
        // parser should have checked that when assigning the date time node
        panic!("date time should have a timezone");
    }

    let zone = if b[0] == b'Z' || b[0] == b'z' {
        b = &b[1..];
        gotime::utc()
    } else {
        const DATE_TIME_BYTE_LEN: usize = 6;
        if b.len() != DATE_TIME_BYTE_LEN {
            return Err(new_parser_error(b, "invalid date-time timezone"));
        }
        let direction = match b[0] {
            b'-' => -1,
            b'+' => 1,
            _ => {
                return Err(new_parser_error(
                    &b[..1],
                    "invalid timezone offset character",
                ));
            }
        };

        if b[3] != b':' {
            return Err(new_parser_error(&b[3..4], "expected a : separator"));
        }

        let hours = parse_decimal_digits(&b[1..3])?;
        if hours > 23 {
            return Err(new_parser_error(&b[..1], "invalid timezone offset hours"));
        }

        let minutes = parse_decimal_digits(&b[4..6])?;
        if minutes > 59 {
            return Err(new_parser_error(&b[..1], "invalid timezone offset minutes"));
        }

        let seconds = direction * (hours * 3600 + minutes * 60);
        b = &b[DATE_TIME_BYTE_LEN..];
        if seconds == 0 {
            gotime::utc()
        } else {
            gotime::fixed_zone("", seconds)
        }
    };

    if !b.is_empty() {
        return Err(new_parser_error(
            b,
            "extra bytes at the end of the timezone",
        ));
    }

    Ok(gotime::date(
        date.year,
        Month(date.month),
        date.day,
        time.hour,
        time.minute,
        time.second,
        time.nanosecond,
        &zone,
    ))
}

/// `parseLocalDateTime`: the date, the time and the bytes after them.
pub(crate) fn parse_local_date_time(
    b: &[u8],
) -> Result<(LocalDate, LocalTime, &[u8]), ParserError<'_>> {
    const LOCAL_DATE_TIME_BYTE_MIN_LEN: usize = 11;
    if b.len() < LOCAL_DATE_TIME_BYTE_MIN_LEN {
        return Err(new_parser_error(
            b,
            "local datetimes are expected to have the format YYYY-MM-DDTHH:MM:SS[.NNNNNNNNN]",
        ));
    }

    let date = parse_local_date(&b[..10])?;

    let sep = b[10];
    if sep != b'T' && sep != b' ' && sep != b't' {
        return Err(new_parser_error(
            &b[10..11],
            "datetime separator is expected to be T or a space",
        ));
    }

    let (time, rest) = parse_local_time(&b[11..])?;

    Ok((date, time, rest))
}

/// `parseLocalTime`: `HH:MM:SS[.NNNNNN]` and the bytes after it, which
/// `parseDateTime` reads as the timezone. Digits past nanoseconds are
/// dropped.
pub(crate) fn parse_local_time(b: &[u8]) -> Result<(LocalTime, &[u8]), ParserError<'_>> {
    const NSPOW: [i64; 10] = [
        0,
        100_000_000,
        10_000_000,
        1_000_000,
        100_000,
        10_000,
        1_000,
        100,
        10,
        1,
    ];
    let mut t = LocalTime {
        hour: 0,
        minute: 0,
        second: 0,
        nanosecond: 0,
        precision: 0,
    };

    // check if b matches to have expected format HH:MM:SS[.NNNNNN]
    const LOCAL_TIME_BYTE_LEN: usize = 8;
    if b.len() < LOCAL_TIME_BYTE_LEN {
        return Err(new_parser_error(
            b,
            "times are expected to have the format HH:MM:SS[.NNNNNN]",
        ));
    }

    t.hour = parse_decimal_digits(&b[0..2])?;

    if t.hour > 23 {
        return Err(new_parser_error(&b[0..2], "hour cannot be greater 23"));
    }
    if b[2] != b':' {
        return Err(new_parser_error(
            &b[2..3],
            "expecting colon between hours and minutes",
        ));
    }

    t.minute = parse_decimal_digits(&b[3..5])?;
    if t.minute > 59 {
        return Err(new_parser_error(&b[3..5], "minutes cannot be greater 59"));
    }
    if b[5] != b':' {
        return Err(new_parser_error(
            &b[5..6],
            "expecting colon between minutes and seconds",
        ));
    }

    t.second = parse_decimal_digits(&b[6..8])?;

    if t.second > 60 {
        return Err(new_parser_error(&b[6..8], "seconds cannot be greater 60"));
    }

    let b = &b[8..];

    if !b.is_empty() && b[0] == b'.' {
        let mut frac = 0;
        let mut precision = 0;
        let mut digits = 0;

        for (i, &c) in b[1..].iter().enumerate() {
            if !c.is_ascii_digit() {
                if i == 0 {
                    return Err(new_parser_error(
                        &b[0..1],
                        "need at least one digit after fraction point",
                    ));
                }
                break;
            }
            digits += 1;

            const MAX_FRAC_PRECISION: usize = 9;
            if i >= MAX_FRAC_PRECISION {
                // go-toml allows decoding fractional seconds beyond the
                // supported precision of 9 digits. It truncates the
                // fractional component to the supported precision and
                // ignores the remaining digits.
                //
                // https://github.com/pelletier/go-toml/discussions/707
                continue;
            }

            frac *= 10;
            frac += i64::from(c - b'0');
            precision += 1;
        }

        if precision == 0 {
            return Err(new_parser_error(
                &b[..1],
                "nanoseconds need at least one digit",
            ));
        }

        t.nanosecond = frac * NSPOW[precision];
        t.precision = precision as i64;

        return Ok((t, &b[1 + digits..]));
    }
    Ok((t, b))
}

/// `parseFloat`: a float, `inf` or `nan` with an optional sign, checked
/// against TOML's rules and parsed by Go's `strconv.ParseFloat`.
pub(crate) fn parse_float(b: &[u8]) -> Result<f64, ParserError<'_>> {
    if b.len() == 4 && (b[0] == b'+' || b[0] == b'-') && &b[1..] == b"nan" {
        return Ok(go_nan());
    }

    let cleaned = check_and_remove_underscores_floats(b)?;

    if cleaned[0] == b'.' {
        return Err(new_parser_error(b, "float cannot start with a dot"));
    }

    if cleaned[cleaned.len() - 1] == b'.' {
        return Err(new_parser_error(b, "float cannot end with a dot"));
    }

    // The highlights index b with positions in cleaned, as Go does.
    let mut dot_already_seen = false;
    for (i, &c) in cleaned.iter().enumerate() {
        if c == b'.' {
            if dot_already_seen {
                return Err(new_parser_error(
                    &b[i..i + 1],
                    "float can have at most one decimal point",
                ));
            }
            if !cleaned[i - 1].is_ascii_digit() {
                return Err(new_parser_error(
                    &b[i - 1..i + 1],
                    "float decimal point must be preceded by a digit",
                ));
            }
            if !cleaned[i + 1].is_ascii_digit() {
                return Err(new_parser_error(
                    &b[i..i + 2],
                    "float decimal point must be followed by a digit",
                ));
            }
            dot_already_seen = true;
        }
    }

    let mut start = 0;
    if cleaned[0] == b'+' || cleaned[0] == b'-' {
        start = 1;
    }
    if cleaned[start] == b'0' && cleaned.len() > start + 1 && cleaned[start + 1].is_ascii_digit() {
        return Err(new_parser_error(
            b,
            "float integer part cannot have leading zeroes",
        ));
    }

    match gostrconv::parse_float(&*cleaned, 64) {
        (f, None) => Ok(f),
        (_, Some(err)) => Err(new_parser_error(b, format!("unable to parse float: {err}"))),
    }
}

/// Go's `math.NaN()`.
fn go_nan() -> f64 {
    f64::from_bits(0x7FF8_0000_0000_0001)
}

fn parse_int_hex(b: &[u8]) -> Result<i64, ParserError<'_>> {
    let cleaned = check_and_remove_underscores_integers(&b[2..])?;

    match gostrconv::parse_int(&*cleaned, 16, 64) {
        (i, None) => Ok(i),
        (_, Some(err)) => Err(new_parser_error(
            b,
            format!("couldn't parse hexadecimal number: {err}"),
        )),
    }
}

fn parse_int_oct(b: &[u8]) -> Result<i64, ParserError<'_>> {
    let cleaned = check_and_remove_underscores_integers(&b[2..])?;

    match gostrconv::parse_int(&*cleaned, 8, 64) {
        (i, None) => Ok(i),
        (_, Some(err)) => Err(new_parser_error(
            b,
            format!("couldn't parse octal number: {err}"),
        )),
    }
}

fn parse_int_bin(b: &[u8]) -> Result<i64, ParserError<'_>> {
    let cleaned = check_and_remove_underscores_integers(&b[2..])?;

    match gostrconv::parse_int(&*cleaned, 2, 64) {
        (i, None) => Ok(i),
        (_, Some(err)) => Err(new_parser_error(
            b,
            format!("couldn't parse binary number: {err}"),
        )),
    }
}

fn is_sign(b: u8) -> bool {
    b == b'+' || b == b'-'
}

fn parse_int_dec(b: &[u8]) -> Result<i64, ParserError<'_>> {
    let cleaned = check_and_remove_underscores_integers(b)?;

    let mut start_idx = 0;

    if is_sign(cleaned[0]) {
        start_idx += 1;
    }

    if cleaned.len() > start_idx + 1 && cleaned[start_idx] == b'0' {
        return Err(new_parser_error(
            b,
            "leading zero not allowed on decimal number",
        ));
    }

    match gostrconv::parse_int(&*cleaned, 10, 64) {
        (i, None) => Ok(i),
        (_, Some(err)) => Err(new_parser_error(
            b,
            format!("couldn't parse decimal number: {err}"),
        )),
    }
}

fn check_and_remove_underscores_integers(b: &[u8]) -> Result<Cow<'_, [u8]>, ParserError<'_>> {
    let mut start = 0;
    if b[start] == b'+' || b[start] == b'-' {
        start += 1;
    }

    if b.len() == start {
        return Ok(Cow::Borrowed(b));
    }

    if b[start] == b'_' {
        return Err(new_parser_error(
            &b[start..start + 1],
            "number cannot start with underscore",
        ));
    }

    if b[b.len() - 1] == b'_' {
        return Err(new_parser_error(
            &b[b.len() - 1..],
            "number cannot end with underscore",
        ));
    }

    // fast path
    let Some(mut i) = b.iter().position(|&c| c == b'_') else {
        return Ok(Cow::Borrowed(b));
    };

    let mut before = false;
    let mut cleaned = Vec::with_capacity(b.len());
    cleaned.extend_from_slice(&b[..i]);

    i += 1;
    while i < b.len() {
        let c = b[i];
        if c == b'_' {
            if !before {
                return Err(new_parser_error(
                    &b[i - 1..i + 1],
                    "number must have at least one digit between underscores",
                ));
            }
            before = false;
        } else {
            before = true;
            cleaned.push(c);
        }
        i += 1;
    }

    Ok(Cow::Owned(cleaned))
}

fn check_and_remove_underscores_floats(b: &[u8]) -> Result<Cow<'_, [u8]>, ParserError<'_>> {
    if b[0] == b'_' {
        return Err(new_parser_error(
            &b[0..1],
            "number cannot start with underscore",
        ));
    }

    if b[b.len() - 1] == b'_' {
        return Err(new_parser_error(
            &b[b.len() - 1..],
            "number cannot end with underscore",
        ));
    }

    // fast path
    if !b.contains(&b'_') {
        return Ok(Cow::Borrowed(b));
    }

    let mut before = false;
    let mut cleaned = Vec::with_capacity(b.len());

    for i in 0..b.len() {
        let c = b[i];

        match c {
            b'_' => {
                if !before {
                    return Err(new_parser_error(
                        &b[i - 1..i + 1],
                        "number must have at least one digit between underscores",
                    ));
                }
                if i < b.len() - 1 && (b[i + 1] == b'e' || b[i + 1] == b'E') {
                    return Err(new_parser_error(
                        &b[i + 1..i + 2],
                        "cannot have underscore before exponent",
                    ));
                }
                before = false;
            }
            b'+' | b'-' => {
                // signed exponents
                cleaned.push(c);
                before = false;
            }
            b'e' | b'E' => {
                if i < b.len() - 1 && b[i + 1] == b'_' {
                    return Err(new_parser_error(
                        &b[i + 1..i + 2],
                        "cannot have underscore after exponent",
                    ));
                }
                cleaned.push(c);
            }
            b'.' => {
                if i < b.len() - 1 && b[i + 1] == b'_' {
                    return Err(new_parser_error(
                        &b[i + 1..i + 2],
                        "cannot have underscore after decimal point",
                    ));
                }
                if i > 0 && b[i - 1] == b'_' {
                    return Err(new_parser_error(
                        &b[i - 1..i],
                        "cannot have underscore before decimal point",
                    ));
                }
                cleaned.push(c);
            }
            _ => {
                before = true;
                cleaned.push(c);
            }
        }
    }

    Ok(Cow::Owned(cleaned))
}

/// `isValidDate`: whether the date exists.
fn is_valid_date(year: i64, month: i64, day: i64) -> bool {
    month > 0 && month < 13 && day > 0 && day <= days_in(month, year)
}

/// `daysBefore[m]`: the days in a non-leap year before month m begins.
const DAYS_BEFORE: [i64; 13] = [
    0,
    31,
    31 + 28,
    31 + 28 + 31,
    31 + 28 + 31 + 30,
    31 + 28 + 31 + 30 + 31,
    31 + 28 + 31 + 30 + 31 + 30,
    31 + 28 + 31 + 30 + 31 + 30 + 31,
    31 + 28 + 31 + 30 + 31 + 30 + 31 + 31,
    31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 30,
    31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31,
    31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30,
    31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 30 + 31 + 30 + 31,
];

fn days_in(m: i64, year: i64) -> i64 {
    if m == 2 && is_leap(year) {
        return 29;
    }
    let m = m as usize;
    DAYS_BEFORE[m] - DAYS_BEFORE[m - 1]
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_in_months() {
        assert_eq!(days_in(1, 2001), 31);
        assert_eq!(days_in(2, 2001), 28);
        assert_eq!(days_in(2, 2000), 29);
        assert_eq!(days_in(2, 1900), 28);
        assert_eq!(days_in(2, 2004), 29);
        assert_eq!(days_in(4, 2004), 30);
        assert_eq!(days_in(12, 2004), 31);
    }

    #[test]
    fn nan_is_gos() {
        assert_eq!(go_nan().to_bits(), 0x7FF8_0000_0000_0001);
        assert_eq!(
            parse_float(b"-nan").map(f64::to_bits),
            Ok(go_nan().to_bits())
        );
    }
}
