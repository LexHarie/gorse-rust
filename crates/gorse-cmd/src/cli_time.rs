// Ports: time (Go standard library): the RFC 3339 parse and format that cmd/gorse-cli uses

//! RFC 3339 times as `gorse-cli` shows them. The master encodes times as
//! RFC 3339 with nanoseconds, and the CLI prints them in RFC 3339 without them.
//! A zero offset prints as `Z`, as Go's `time.RFC3339` does.

/// A parsed time: the instant, and the offset it was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rfc3339 {
    /// Nanoseconds since the Unix epoch.
    pub unix_nanos: i128,
    /// The offset from UTC, in seconds, as the text wrote it.
    pub offset_secs: i64,
}

/// Parses `text` as Go's `time.RFC3339Nano` does: a date and time with `T` between
/// them, optional fractional seconds, and a `Z` or `±hh:mm` zone. Returns `None`
/// for anything else, including out-of-range fields.
pub fn parse_rfc3339(text: &str) -> Option<Rfc3339> {
    let bytes = text.as_bytes();
    let digits = |from: usize, count: usize| -> Option<i64> {
        let slice = bytes.get(from..from + count)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    let hour = digits(11, 2)?;
    let minute = digits(14, 2)?;
    let second = digits(17, 2)?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut pos = 19;
    let mut nanos: i64 = 0;
    if bytes.get(pos) == Some(&b'.') {
        pos += 1;
        let start = pos;
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            if pos - start < 9 {
                nanos = nanos * 10 + i64::from(bytes[pos] - b'0');
            }
            pos += 1;
        }
        if pos == start {
            return None;
        }
        for _ in (pos - start).min(9)..9 {
            nanos *= 10;
        }
    }

    let offset_secs = match bytes.get(pos)? {
        b'Z' if pos + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') => {
            if bytes.len() != pos + 6 || bytes[pos + 3] != b':' {
                return None;
            }
            let hours = digits(pos + 1, 2)?;
            let minutes = digits(pos + 4, 2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let secs = hours * 3600 + minutes * 60;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let local_secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    let unix_secs = local_secs - offset_secs;
    Some(Rfc3339 {
        unix_nanos: i128::from(unix_secs) * 1_000_000_000 + i128::from(nanos),
        offset_secs,
    })
}

/// Formats `time` as Go's `time.RFC3339`: seconds only, with the offset it was
/// parsed with, and `Z` for a zero offset.
pub fn format_rfc3339(time: Rfc3339) -> String {
    let local = time.unix_nanos.div_euclid(1_000_000_000) as i64 + time.offset_secs;
    let days = local.div_euclid(86_400);
    let seconds_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let zone = if time.offset_secs == 0 {
        "Z".to_owned()
    } else {
        let sign = if time.offset_secs < 0 { '-' } else { '+' };
        let abs = time.offset_secs.abs();
        format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60)
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{zone}",
        seconds_of_day / 3600,
        seconds_of_day % 3600 / 60,
        seconds_of_day % 60
    )
}

/// Formats text that parses as RFC 3339 in RFC 3339 without fractions, and
/// returns `None` for any other text.
pub fn format_time_text(text: &str) -> Option<String> {
    parse_rfc3339(text).map(format_rfc3339)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian calendar).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_times_print_with_z_and_no_fraction() {
        let time = parse_rfc3339("2026-01-01T01:00:00.123456789Z").expect("a valid time");
        assert_eq!(format_rfc3339(time), "2026-01-01T01:00:00Z");
    }

    #[test]
    fn zero_offset_prints_as_z() {
        assert_eq!(
            format_time_text("2026-01-01T01:00:00+00:00").as_deref(),
            Some("2026-01-01T01:00:00Z")
        );
    }

    #[test]
    fn offsets_are_kept() {
        assert_eq!(
            format_time_text("2026-01-01T09:30:00+08:00").as_deref(),
            Some("2026-01-01T09:30:00+08:00")
        );
        assert_eq!(
            format_time_text("2026-01-01T01:00:00-05:30").as_deref(),
            Some("2026-01-01T01:00:00-05:30")
        );
    }

    #[test]
    fn zero_time_round_trips() {
        assert_eq!(
            format_time_text("0001-01-01T00:00:00Z").as_deref(),
            Some("0001-01-01T00:00:00Z")
        );
    }

    #[test]
    fn invalid_times_are_text() {
        assert_eq!(format_time_text("alice"), None);
        assert_eq!(format_time_text("2026-02-30T00:00:00Z"), None);
        assert_eq!(format_time_text("2026-01-01T24:00:00Z"), None);
        assert_eq!(format_time_text("2026-01-01 00:00:00Z"), None);
        assert_eq!(format_time_text("2026-01-01T00:00:00"), None);
    }

    #[test]
    fn parsed_instants_order_by_time_not_text() {
        let earlier = parse_rfc3339("2026-01-01T09:00:00+08:00").expect("a valid time");
        let later = parse_rfc3339("2026-01-01T02:00:00Z").expect("a valid time");
        assert!(later.unix_nanos > earlier.unix_nanos);
    }
}
