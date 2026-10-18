// Ports: $GOROOT/src/time/format_test.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package tests: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `format_test.go`, case for case. Go runs it with `time.Local` set to
//! America/Los_Angeles named "Local" (`ForceUSPacificForTesting`), which
//! [`common::go_setup`] reproduces. The two fuzz targets run their seed
//! corpora, as `go test` does without `-fuzz`.

mod common;

mod gotime {
    use gorse_common::gotime::{
        self, ANSIC, DATE_ONLY, DATE_TIME, KITCHEN, Location, Month, RFC850, RFC1123, RFC1123Z,
        RFC3339, RFC3339_NANO, RUBY_DATE, STAMP, STAMP_MICRO, STAMP_MILLI, STAMP_NANO, TIME_ONLY,
        Time, UNIX_DATE, Weekday, date, fixed_zone, internal, load_location, local, parse,
        parse_in_location, unix, utc,
    };

    use crate::common::go_setup;

    const NEXT_STD_CHUNK_TESTS: &[&str] = &[
        "(2006)-(01)-(02)T(15):(04):(05)(Z07:00)",
        "(2006)-(01)-(02) (002) (15):(04):(05)",
        "(2006)-(01) (002) (15):(04):(05)",
        "(2006)-(002) (15):(04):(05)",
        "(2006)(002)(01) (15):(04):(05)",
        "(2006)(002)(04) (15):(04):(05)",
    ];

    #[test]
    fn test_next_std_chunk() {
        go_setup();
        // Most bugs in Parse or Format boil down to problems with the exact
        // detection of format chunk boundaries in the helper function
        // nextStdChunk. This test checks nextStdChunk's behavior directly.

        // markChunks returns format with each detected 'format chunk'
        // parenthesized. For example showChunks("2006-01-02") ==
        // "(2006)-(01)-(02)".
        let mark_chunks = |format: &str| -> String {
            let mut out = Vec::new();
            let mut s = format.as_bytes();
            while !s.is_empty() {
                let (prefix, std, suffix) = internal::next_std_chunk(s);
                out.extend_from_slice(prefix);
                if std > 0 {
                    out.push(b'(');
                    out.extend_from_slice(
                        internal::std_chunk_name(std).unwrap_or_default().as_bytes(),
                    );
                    out.push(b')');
                }
                s = suffix;
            }
            String::from_utf8(out).expect("UTF-8")
        };

        let mut errors = Vec::new();
        for marked in NEXT_STD_CHUNK_TESTS {
            // marked is an expected output from markChunks. If we delete the
            // parens and pass it through markChunks, we should get the
            // original back.
            let format: String = marked.chars().filter(|&r| r != '(' && r != ')').collect();
            let out = mark_chunks(&format);
            if out != *marked {
                errors.push(format!(
                    "nextStdChunk parses {format:?} as {out:?}, want {marked:?}"
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    struct TimeFormatTest {
        time: Time,
        formatted_value: &'static str,
    }

    fn rfc3339_formats() -> Vec<TimeFormatTest> {
        vec![
            TimeFormatTest {
                time: date(2008, Month(9), 17, 20, 4, 26, 0, &utc()),
                formatted_value: "2008-09-17T20:04:26Z",
            },
            TimeFormatTest {
                time: date(1994, Month(9), 17, 20, 4, 26, 0, &fixed_zone("EST", -18000)),
                formatted_value: "1994-09-17T20:04:26-05:00",
            },
            TimeFormatTest {
                time: date(2000, Month(12), 26, 1, 15, 6, 0, &fixed_zone("OTO", 15600)),
                formatted_value: "2000-12-26T01:15:06+04:20",
            },
        ]
    }

    #[test]
    fn test_rfc3339_conversion() {
        go_setup();
        for f in rfc3339_formats() {
            assert_eq!(f.time.format(RFC3339), f.formatted_value, "RFC3339");
        }
    }

    #[test]
    fn test_append_int() {
        go_setup();
        let tests: &[(i64, i64, &str)] = &[
            (0, 0, "0"),
            (0, 1, "0"),
            (0, 2, "00"),
            (0, 3, "000"),
            (1, 0, "1"),
            (1, 1, "1"),
            (1, 2, "01"),
            (1, 3, "001"),
            (-1, 0, "-1"),
            (-1, 1, "-1"),
            (-1, 2, "-01"),
            (-1, 3, "-001"),
            (99, 2, "99"),
            (100, 2, "100"),
            (1, 4, "0001"),
            (12, 4, "0012"),
            (123, 4, "0123"),
            (1234, 4, "1234"),
            (12345, 4, "12345"),
            (1, 5, "00001"),
            (12, 5, "00012"),
            (123, 5, "00123"),
            (1234, 5, "01234"),
            (12345, 5, "12345"),
            (123456, 5, "123456"),
            (0, 9, "000000000"),
            (123, 9, "000000123"),
            (123456, 9, "000123456"),
            (123456789, 9, "123456789"),
        ];
        let mut got = Vec::new();
        for &(input, width, want) in tests {
            got.clear();
            internal::append_int(&mut got, input, width);
            assert_eq!(
                String::from_utf8_lossy(&got),
                want,
                "appendInt({input}, {width})"
            );
        }
    }

    struct FormatTest {
        name: &'static str,
        format: &'static str,
        result: &'static str,
    }

    const fn ft(name: &'static str, format: &'static str, result: &'static str) -> FormatTest {
        FormatTest {
            name,
            format,
            result,
        }
    }

    const FORMAT_TESTS: &[FormatTest] = &[
        ft("ANSIC", ANSIC, "Wed Feb  4 21:00:57 2009"),
        ft("UnixDate", UNIX_DATE, "Wed Feb  4 21:00:57 PST 2009"),
        ft("RubyDate", RUBY_DATE, "Wed Feb 04 21:00:57 -0800 2009"),
        ft("RFC822", gotime::RFC822, "04 Feb 09 21:00 PST"),
        ft("RFC850", RFC850, "Wednesday, 04-Feb-09 21:00:57 PST"),
        ft("RFC1123", RFC1123, "Wed, 04 Feb 2009 21:00:57 PST"),
        ft("RFC1123Z", RFC1123Z, "Wed, 04 Feb 2009 21:00:57 -0800"),
        ft("RFC3339", RFC3339, "2009-02-04T21:00:57-08:00"),
        ft(
            "RFC3339Nano",
            RFC3339_NANO,
            "2009-02-04T21:00:57.0123456-08:00",
        ),
        ft("Kitchen", KITCHEN, "9:00PM"),
        ft("am/pm", "3pm", "9pm"),
        ft("AM/PM", "3PM", "9PM"),
        ft("two-digit year", "06 01 02", "09 02 04"),
        // Three-letter months and days must not be followed by lower-case
        // letter.
        ft(
            "Janet",
            "Hi Janet, the Month is January",
            "Hi Janet, the Month is February",
        ),
        // Time stamps, Fractional seconds.
        ft("Stamp", STAMP, "Feb  4 21:00:57"),
        ft("StampMilli", STAMP_MILLI, "Feb  4 21:00:57.012"),
        ft("StampMicro", STAMP_MICRO, "Feb  4 21:00:57.012345"),
        ft("StampNano", STAMP_NANO, "Feb  4 21:00:57.012345600"),
        ft("DateTime", DATE_TIME, "2009-02-04 21:00:57"),
        ft("DateOnly", DATE_ONLY, "2009-02-04"),
        ft("TimeOnly", TIME_ONLY, "21:00:57"),
        ft("YearDay", "Jan  2 002 __2 2", "Feb  4 035  35 4"),
        ft("Year", "2006 6 06 _6 __6 ___6", "2009 6 09 _6 __6 ___6"),
        ft("Month", "Jan January 1 01 _1", "Feb February 2 02 _2"),
        ft("DayOfMonth", "2 02 _2 __2", "4 04  4  35"),
        ft("DayOfWeek", "Mon Monday", "Wed Wednesday"),
        ft("Hour", "15 3 03 _3", "21 9 09 _9"),
        ft("Minute", "4 04 _4", "0 00 _0"),
        ft("Second", "5 05 _5", "57 57 _57"),
    ];

    #[test]
    fn test_format() {
        go_setup();
        // The numeric time represents Thu Feb  4 21:00:57.012345600 PST 2009
        let time = unix(0, 1233810057012345600);
        let mut errors = Vec::new();
        for test in FORMAT_TESTS {
            let result = time.format(test.format);
            if result != test.result {
                errors.push(format!(
                    "{} expected {:?} got {result:?}",
                    test.name, test.result
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_go_string() {
        go_setup();
        let tests = [
            (
                date(2009, Month::FEBRUARY, 5, 5, 0, 57, 12345600, &utc()),
                "time.Date(2009, time.February, 5, 5, 0, 57, 12345600, time.UTC)",
            ),
            (
                date(2009, Month::FEBRUARY, 5, 5, 0, 57, 12345600, &local()),
                "time.Date(2009, time.February, 5, 5, 0, 57, 12345600, time.Local)",
            ),
            (
                date(
                    2009,
                    Month::FEBRUARY,
                    5,
                    5,
                    0,
                    57,
                    12345600,
                    &fixed_zone("Europe/Berlin", 3 * 60 * 60),
                ),
                r#"time.Date(2009, time.February, 5, 5, 0, 57, 12345600, time.Location("Europe/Berlin"))"#,
            ),
            (
                date(
                    2009,
                    Month::FEBRUARY,
                    5,
                    5,
                    0,
                    57,
                    12345600,
                    &fixed_zone("Non-ASCII character ⏰", 3 * 60 * 60),
                ),
                r#"time.Date(2009, time.February, 5, 5, 0, 57, 12345600, time.Location("Non-ASCII character \xe2\x8f\xb0"))"#,
            ),
        ];
        // The numeric time represents Thu Feb  4 21:00:57.012345600 PST 2009
        for (input, want) in tests {
            assert_eq!(input.go_string(), want, "GoString ({input})");
        }
    }

    // issue 12440.
    #[test]
    fn test_format_single_digits() {
        go_setup();
        let time = date(2001, Month(2), 3, 4, 5, 6, 700000000, &utc());
        let test = ft("single digit format", "3:4:5", "4:5:6");
        let result = time.format(test.format);
        assert_eq!(result, test.result, "{}", test.name);
    }

    #[test]
    fn test_format_short_year() {
        go_setup();
        let years: &[i64] = &[
            -100001, -100000, -99999, -10001, -10000, -9999, -1001, -1000, -999, -101, -100, -99,
            -11, -10, -9, -1, 0, 1, 9, 10, 11, 99, 100, 101, 999, 1000, 1001, 9999, 10000, 10001,
            99999, 100000, 100001,
        ];

        for &y in years {
            let time = date(y, Month::JANUARY, 1, 0, 0, 0, 0, &utc());
            let result = time.format("2006.01.02");
            let want = if y < 0 {
                // The 4 in %04d counts the - sign, so print -y instead and
                // introduce our own - sign.
                format!("-{:04}.{:02}.{:02}", -y, 1, 1)
            } else {
                format!("{y:04}.{:02}.{:02}", 1, 1)
            };
            assert_eq!(result, want, "(jan 1 {y}).Format(\"2006.01.02\")");
        }
    }

    struct ParseTest {
        name: &'static str,
        format: &'static str,
        value: &'static str,
        has_tz: bool,       // contains a time zone
        has_wd: bool,       // contains a weekday
        year_sign: i64,     // sign of year, -1 indicates the year is not present in the format
        frac_digits: usize, // number of digits of fractional second
    }

    const fn pt(
        name: &'static str,
        format: &'static str,
        value: &'static str,
        has_tz: bool,
        has_wd: bool,
        year_sign: i64,
        frac_digits: usize,
    ) -> ParseTest {
        ParseTest {
            name,
            format,
            value,
            has_tz,
            has_wd,
            year_sign,
            frac_digits,
        }
    }

    const PARSE_TESTS: &[ParseTest] = &[
        pt(
            "ANSIC",
            ANSIC,
            "Thu Feb  4 21:00:57 2010",
            false,
            true,
            1,
            0,
        ),
        pt(
            "UnixDate",
            UNIX_DATE,
            "Thu Feb  4 21:00:57 PST 2010",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57 -0800 2010",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RFC850",
            RFC850,
            "Thursday, 04-Feb-10 21:00:57 PST",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RFC1123",
            RFC1123,
            "Thu, 04 Feb 2010 21:00:57 PST",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RFC1123",
            RFC1123,
            "Thu, 04 Feb 2010 22:00:57 PDT",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RFC1123Z",
            RFC1123Z,
            "Thu, 04 Feb 2010 21:00:57 -0800",
            true,
            true,
            1,
            0,
        ),
        pt(
            "RFC3339",
            RFC3339,
            "2010-02-04T21:00:57-08:00",
            true,
            false,
            1,
            0,
        ),
        pt(
            "custom: \"2006-01-02 15:04:05-07\"",
            "2006-01-02 15:04:05-07",
            "2010-02-04 21:00:57-08",
            true,
            false,
            1,
            0,
        ),
        // Optional fractional seconds.
        pt(
            "ANSIC",
            ANSIC,
            "Thu Feb  4 21:00:57.0 2010",
            false,
            true,
            1,
            1,
        ),
        pt(
            "UnixDate",
            UNIX_DATE,
            "Thu Feb  4 21:00:57.01 PST 2010",
            true,
            true,
            1,
            2,
        ),
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57.012 -0800 2010",
            true,
            true,
            1,
            3,
        ),
        pt(
            "RFC850",
            RFC850,
            "Thursday, 04-Feb-10 21:00:57.0123 PST",
            true,
            true,
            1,
            4,
        ),
        pt(
            "RFC1123",
            RFC1123,
            "Thu, 04 Feb 2010 21:00:57.01234 PST",
            true,
            true,
            1,
            5,
        ),
        pt(
            "RFC1123Z",
            RFC1123Z,
            "Thu, 04 Feb 2010 21:00:57.01234 -0800",
            true,
            true,
            1,
            5,
        ),
        pt(
            "RFC3339",
            RFC3339,
            "2010-02-04T21:00:57.012345678-08:00",
            true,
            false,
            1,
            9,
        ),
        pt(
            "custom: \"2006-01-02 15:04:05\"",
            "2006-01-02 15:04:05",
            "2010-02-04 21:00:57.0",
            false,
            false,
            1,
            0,
        ),
        // Amount of white space should not matter.
        pt("ANSIC", ANSIC, "Thu Feb 4 21:00:57 2010", false, true, 1, 0),
        pt(
            "ANSIC",
            ANSIC,
            "Thu      Feb     4     21:00:57     2010",
            false,
            true,
            1,
            0,
        ),
        // Case should not matter
        pt("ANSIC", ANSIC, "THU FEB 4 21:00:57 2010", false, true, 1, 0),
        pt("ANSIC", ANSIC, "thu feb 4 21:00:57 2010", false, true, 1, 0),
        // Fractional seconds.
        pt(
            "millisecond:: dot separator",
            "Mon Jan _2 15:04:05.000 2006",
            "Thu Feb  4 21:00:57.012 2010",
            false,
            true,
            1,
            3,
        ),
        pt(
            "microsecond:: dot separator",
            "Mon Jan _2 15:04:05.000000 2006",
            "Thu Feb  4 21:00:57.012345 2010",
            false,
            true,
            1,
            6,
        ),
        pt(
            "nanosecond:: dot separator",
            "Mon Jan _2 15:04:05.000000000 2006",
            "Thu Feb  4 21:00:57.012345678 2010",
            false,
            true,
            1,
            9,
        ),
        pt(
            "millisecond:: comma separator",
            "Mon Jan _2 15:04:05,000 2006",
            "Thu Feb  4 21:00:57.012 2010",
            false,
            true,
            1,
            3,
        ),
        pt(
            "microsecond:: comma separator",
            "Mon Jan _2 15:04:05,000000 2006",
            "Thu Feb  4 21:00:57.012345 2010",
            false,
            true,
            1,
            6,
        ),
        pt(
            "nanosecond:: comma separator",
            "Mon Jan _2 15:04:05,000000000 2006",
            "Thu Feb  4 21:00:57.012345678 2010",
            false,
            true,
            1,
            9,
        ),
        // Leading zeros in other places should not be taken as fractional
        // seconds.
        pt(
            "zero1",
            "2006.01.02.15.04.05.0",
            "2010.02.04.21.00.57.0",
            false,
            false,
            1,
            1,
        ),
        pt(
            "zero2",
            "2006.01.02.15.04.05.00",
            "2010.02.04.21.00.57.01",
            false,
            false,
            1,
            2,
        ),
        // Month and day names only match when not followed by a lower-case
        // letter.
        pt(
            "Janet",
            "Hi Janet, the Month is January: Jan _2 15:04:05 2006",
            "Hi Janet, the Month is February: Feb  4 21:00:57 2010",
            false,
            true,
            1,
            0,
        ),
        // GMT with offset.
        pt(
            "GMT-8",
            UNIX_DATE,
            "Fri Feb  5 05:00:57 GMT-8 2010",
            true,
            true,
            1,
            0,
        ),
        // Accept any number of fractional second digits (including none) for
        // .999... In Go 1, .999... was completely ignored in the format,
        // meaning the first two cases would succeed, but the next four would
        // not. Go 1.1 accepts all six. decimal "." separator.
        pt(
            "",
            "2006-01-02 15:04:05.9999 -0700 MST",
            "2010-02-04 21:00:57 -0800 PST",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02 15:04:05.999999999 -0700 MST",
            "2010-02-04 21:00:57 -0800 PST",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02 15:04:05.9999 -0700 MST",
            "2010-02-04 21:00:57.0123 -0800 PST",
            true,
            false,
            1,
            4,
        ),
        pt(
            "",
            "2006-01-02 15:04:05.999999999 -0700 MST",
            "2010-02-04 21:00:57.0123 -0800 PST",
            true,
            false,
            1,
            4,
        ),
        pt(
            "",
            "2006-01-02 15:04:05.9999 -0700 MST",
            "2010-02-04 21:00:57.012345678 -0800 PST",
            true,
            false,
            1,
            9,
        ),
        pt(
            "",
            "2006-01-02 15:04:05.999999999 -0700 MST",
            "2010-02-04 21:00:57.012345678 -0800 PST",
            true,
            false,
            1,
            9,
        ),
        // comma "," separator.
        pt(
            "",
            "2006-01-02 15:04:05,9999 -0700 MST",
            "2010-02-04 21:00:57 -0800 PST",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02 15:04:05,999999999 -0700 MST",
            "2010-02-04 21:00:57 -0800 PST",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02 15:04:05,9999 -0700 MST",
            "2010-02-04 21:00:57.0123 -0800 PST",
            true,
            false,
            1,
            4,
        ),
        pt(
            "",
            "2006-01-02 15:04:05,999999999 -0700 MST",
            "2010-02-04 21:00:57.0123 -0800 PST",
            true,
            false,
            1,
            4,
        ),
        pt(
            "",
            "2006-01-02 15:04:05,9999 -0700 MST",
            "2010-02-04 21:00:57.012345678 -0800 PST",
            true,
            false,
            1,
            9,
        ),
        pt(
            "",
            "2006-01-02 15:04:05,999999999 -0700 MST",
            "2010-02-04 21:00:57.012345678 -0800 PST",
            true,
            false,
            1,
            9,
        ),
        // issue 4502.
        pt(
            "",
            STAMP_NANO,
            "Feb  4 21:00:57.012345678",
            false,
            false,
            -1,
            9,
        ),
        pt(
            "",
            "Jan _2 15:04:05.999",
            "Feb  4 21:00:57.012300000",
            false,
            false,
            -1,
            4,
        ),
        pt(
            "",
            "Jan _2 15:04:05.999",
            "Feb  4 21:00:57.012345678",
            false,
            false,
            -1,
            9,
        ),
        pt(
            "",
            "Jan _2 15:04:05.999999999",
            "Feb  4 21:00:57.0123",
            false,
            false,
            -1,
            4,
        ),
        pt(
            "",
            "Jan _2 15:04:05.999999999",
            "Feb  4 21:00:57.012345678",
            false,
            false,
            -1,
            9,
        ),
        // Day of year.
        pt(
            "",
            "2006-01-02 002 15:04:05",
            "2010-02-04 035 21:00:57",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01 002 15:04:05",
            "2010-02 035 21:00:57",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-002 15:04:05",
            "2010-035 21:00:57",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "200600201 15:04:05",
            "201003502 21:00:57",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "200600204 15:04:05",
            "201003504 21:00:57",
            false,
            false,
            1,
            0,
        ),
        // Time zone offsets
        pt(
            "",
            "2006-01-02T15:04:05Z07",
            "2010-02-04T21:00:57Z",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07",
            "2010-02-04T21:00:57+08",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07",
            "2010-02-04T21:00:57-08",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z0700",
            "2010-02-04T21:00:57Z",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z0700",
            "2010-02-04T21:00:57+0800",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z0700",
            "2010-02-04T21:00:57-0800",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00",
            "2010-02-04T21:00:57Z",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00",
            "2010-02-04T21:00:57+08:00",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00",
            "2010-02-04T21:00:57-08:00",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z070000",
            "2010-02-04T21:00:57Z",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z070000",
            "2010-02-04T21:00:57+080000",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z070000",
            "2010-02-04T21:00:57-080000",
            true,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00:00",
            "2010-02-04T21:00:57Z",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00:00",
            "2010-02-04T21:00:57+08:00:00",
            false,
            false,
            1,
            0,
        ),
        pt(
            "",
            "2006-01-02T15:04:05Z07:00:00",
            "2010-02-04T21:00:57-08:00:00",
            true,
            false,
            1,
            0,
        ),
    ];

    #[test]
    fn test_parse() {
        go_setup();
        let mut errors = Vec::new();
        for test in PARSE_TESTS {
            match parse(test.format, test.value) {
                Err(err) => errors.push(format!("{} error: {err}", test.name)),
                Ok(time) => check_time(&time, test, &mut errors),
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    // All parsed with ANSIC.
    const DAY_OUT_OF_RANGE_TESTS: &[(&str, bool)] = &[
        ("Thu Jan 99 21:00:57 2010", false),
        ("Thu Jan 31 21:00:57 2010", true),
        ("Thu Jan 32 21:00:57 2010", false),
        ("Thu Feb 28 21:00:57 2012", true),
        ("Thu Feb 29 21:00:57 2012", true),
        ("Thu Feb 29 21:00:57 2010", false),
        ("Thu Mar 31 21:00:57 2010", true),
        ("Thu Mar 32 21:00:57 2010", false),
        ("Thu Apr 30 21:00:57 2010", true),
        ("Thu Apr 31 21:00:57 2010", false),
        ("Thu May 31 21:00:57 2010", true),
        ("Thu May 32 21:00:57 2010", false),
        ("Thu Jun 30 21:00:57 2010", true),
        ("Thu Jun 31 21:00:57 2010", false),
        ("Thu Jul 31 21:00:57 2010", true),
        ("Thu Jul 32 21:00:57 2010", false),
        ("Thu Aug 31 21:00:57 2010", true),
        ("Thu Aug 32 21:00:57 2010", false),
        ("Thu Sep 30 21:00:57 2010", true),
        ("Thu Sep 31 21:00:57 2010", false),
        ("Thu Oct 31 21:00:57 2010", true),
        ("Thu Oct 32 21:00:57 2010", false),
        ("Thu Nov 30 21:00:57 2010", true),
        ("Thu Nov 31 21:00:57 2010", false),
        ("Thu Dec 31 21:00:57 2010", true),
        ("Thu Dec 32 21:00:57 2010", false),
        ("Thu Dec 00 21:00:57 2010", false),
    ];

    #[test]
    fn test_parse_day_out_of_range() {
        go_setup();
        let mut errors = Vec::new();
        for &(d, ok) in DAY_OUT_OF_RANGE_TESTS {
            match (ok, parse(ANSIC, d)) {
                (true, Ok(_)) => {}
                (false, Err(err)) => {
                    if !err.to_string().contains("day out of range") {
                        errors.push(format!("{d:?}: expected 'day' error, got {err}"));
                    }
                }
                (true, Err(err)) => errors.push(format!("{d:?}: unexpected error: {err}")),
                (false, Ok(_)) => errors.push(format!("{d:?}: expected 'day' error, got none")),
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    // TestParseInLocation checks that the Parse and ParseInLocation functions
    // do not get confused by the fact that AST (Arabia Standard Time) and AST
    // (Atlantic Standard Time) are different time zones, even though they
    // have the same abbreviation.
    //
    // ICANN has been slowly phasing out invented abbreviation in favor of
    // numeric time zones (for example, the Asia/Baghdad time zone abbreviation
    // got changed from AST to +03 in the 2017a tzdata release); but we still
    // want to make sure that the time package does not get confused on systems
    // with slightly older tzdata packages.
    #[test]
    fn test_parse_in_location() {
        go_setup();
        let baghdad = load_location("Asia/Baghdad").expect("Asia/Baghdad");

        let t1 = parse_in_location("Jan 02 2006 MST", "Feb 01 2013 AST", &baghdad).expect("parse");

        let (_, offset) = t1.zone();

        // A zero offset means that ParseInLocation did not recognize the 'AST'
        // abbreviation as matching the current location (Baghdad, where we'd
        // expect a +03 hrs offset); likely because we're using a recent tzdata
        // release (2017a or newer). If it happens, skip the Baghdad test.
        if offset != 0 {
            let t2 = date(2013, Month::FEBRUARY, 1, 0, 0, 0, 0, &baghdad);
            assert!(
                t1 == t2,
                "ParseInLocation(Feb 01 2013 AST, Baghdad) = {t1}, want {t2}"
            );
            assert_eq!(
                offset,
                3 * 60 * 60,
                "ParseInLocation(Feb 01 2013 AST, Baghdad).Zone"
            );
        }

        let blanc_sablon = load_location("America/Blanc-Sablon").expect("America/Blanc-Sablon");

        // In this case 'AST' means 'Atlantic Standard Time', and we expect the
        // abbreviation to correctly match the american location.
        let t1 =
            parse_in_location("Jan 02 2006 MST", "Feb 01 2013 AST", &blanc_sablon).expect("parse");
        let t2 = date(2013, Month::FEBRUARY, 1, 0, 0, 0, 0, &blanc_sablon);
        assert!(
            t1 == t2,
            "ParseInLocation(Feb 01 2013 AST, Blanc-Sablon) = {t1}, want {t2}"
        );
        let (_, offset) = t1.zone();
        assert_eq!(
            offset,
            -4 * 60 * 60,
            "ParseInLocation(Feb 01 2013 AST, Blanc-Sablon).Zone"
        );
    }

    #[test]
    fn test_load_location_zip_file() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        load_location("Australia/Sydney").expect("Australia/Sydney");
    }

    const RUBY_TESTS: &[ParseTest] = &[
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57 -0800 2010",
            true,
            true,
            1,
            0,
        ),
        // Ignore the time zone in the test. If it parses, it'll be OK.
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57 -0000 2010",
            false,
            true,
            1,
            0,
        ),
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57 +0000 2010",
            false,
            true,
            1,
            0,
        ),
        pt(
            "RubyDate",
            RUBY_DATE,
            "Thu Feb 04 21:00:57 +1130 2010",
            false,
            true,
            1,
            0,
        ),
    ];

    // Problematic time zone format needs special tests.
    #[test]
    fn test_ruby_parse() {
        go_setup();
        let mut errors = Vec::new();
        for test in RUBY_TESTS {
            match parse(test.format, test.value) {
                Err(err) => errors.push(format!("{} error: {err}", test.name)),
                Ok(time) => check_time(&time, test, &mut errors),
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    fn check_time(time: &Time, test: &ParseTest, errors: &mut Vec<String>) {
        // The time should be Thu Feb  4 21:00:57 PST 2010
        if test.year_sign >= 0 && test.year_sign * time.year() != 2010 {
            errors.push(format!(
                "{}: bad year: {} not {}",
                test.name,
                time.year(),
                2010
            ));
        }
        if time.month() != Month::FEBRUARY {
            errors.push(format!(
                "{}: bad month: {} not {}",
                test.name,
                time.month(),
                Month::FEBRUARY
            ));
        }
        if time.day() != 4 {
            errors.push(format!("{}: bad day: {} not {}", test.name, time.day(), 4));
        }
        if time.hour() != 21 {
            errors.push(format!(
                "{}: bad hour: {} not {}",
                test.name,
                time.hour(),
                21
            ));
        }
        if time.minute() != 0 {
            errors.push(format!(
                "{}: bad minute: {} not {}",
                test.name,
                time.minute(),
                0
            ));
        }
        if time.second() != 57 {
            errors.push(format!(
                "{}: bad second: {} not {}",
                test.name,
                time.second(),
                57
            ));
        }
        // Nanoseconds must be checked against the precision of the input.
        let digits = format!(
            "{}{}",
            &"012345678"[..test.frac_digits],
            &"000000000"[..9 - test.frac_digits]
        );
        let nanosec: i64 = digits.parse().expect("digits");
        if time.nanosecond() != nanosec {
            errors.push(format!(
                "{}: bad nanosecond: {} not {nanosec}",
                test.name,
                time.nanosecond()
            ));
        }
        let (name, offset) = time.zone();
        if test.has_tz && offset != -28800 {
            errors.push(format!(
                "{}: bad tz offset: {name} {offset} not {}",
                test.name, -28800
            ));
        }
        if test.has_wd && time.weekday() != Weekday::THURSDAY {
            errors.push(format!(
                "{}: bad weekday: {} not {}",
                test.name,
                time.weekday(),
                Weekday::THURSDAY
            ));
        }
    }

    /// The generator for the `testing/quick` property tests: Go draws from a
    /// time-seeded `math/rand` source, so its sequence is not reproducible
    /// anyway; this is a fixed-seed splitmix64 stream of the same type
    /// (`randInt64` is a raw `Uint64` reinterpreted, and an `int32` argument is
    /// its truncation).
    struct QuickRand(u64);

    impl QuickRand {
        fn uint64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
    }

    #[test]
    fn test_format_and_parse() {
        go_setup();
        const FMT: &str = "Mon MST 2006-01-02T15:04:05Z07:00"; // "Mon MST " + RFC3339: all fields
        let f = |sec: i64| -> Result<(), String> {
            let t1 = unix(sec / 2, 0);
            // (As in Go, t1.Unix() is sec/2, so only sec == 0 gets past this.)
            if t1.year() < 1000 || t1.year() > 9999 || t1.unix() != sec {
                // not required to work
                return Ok(());
            }
            let t2 = parse(FMT, t1.format(FMT)).map_err(|err| format!("error: {err}"))?;
            if t1.unix() != t2.unix() || t1.nanosecond() != t2.nanosecond() {
                return Err(format!(
                    "FormatAndParse {sec}: {t1:?}({}) {t2:?}({})",
                    t1.unix(),
                    t2.unix()
                ));
            }
            Ok(())
        };
        const MAX_COUNT: usize = 10000;
        let mut rng = QuickRand(1);

        // Try a reasonable date first, then the huge ones.
        for _ in 0..MAX_COUNT {
            let sec = rng.uint64() as i64 as i32;
            f(i64::from(sec)).unwrap_or_else(|e| panic!("#{sec}: {e}"));
        }
        for _ in 0..MAX_COUNT {
            let sec = rng.uint64() as i64;
            f(sec).unwrap_or_else(|e| panic!("#{sec}: {e}"));
        }
        // The one value the property reaches.
        f(0).expect("sec 0");
    }

    const PARSE_TIME_ZONE_TESTS: &[(&str, usize, bool)] = &[
        ("gmt hi there", 0, false),
        ("GMT hi there", 3, true),
        ("GMT+12 hi there", 6, true),
        ("GMT+00 hi there", 6, true),
        ("GMT+", 3, true),
        ("GMT+3", 5, true),
        ("GMT+a", 3, true),
        ("GMT+3a", 5, true),
        ("GMT-5 hi there", 5, true),
        ("GMT-51 hi there", 3, true),
        ("ChST hi there", 4, true),
        ("MeST hi there", 4, true),
        ("MSDx", 3, true),
        ("MSDY", 0, false), // four letters must end in T.
        ("ESAST hi", 5, true),
        ("ESASTT hi", 0, false), // run of upper-case letters too long.
        ("ESATY hi", 0, false),  // five letters must end in T.
        ("WITA hi", 4, true),    // Issue #18251
        // Issue #24071
        ("+03 hi", 3, true),
        ("-04 hi", 3, true),
        // Issue #26032
        ("+00", 3, true),
        ("-11", 3, true),
        ("-12", 3, true),
        ("-23", 3, true),
        ("-24", 0, false),
        ("+13", 3, true),
        ("+14", 3, true),
        ("+23", 3, true),
        ("+24", 0, false),
    ];

    #[test]
    fn test_parse_time_zone() {
        go_setup();
        for &(value, want_length, want_ok) in PARSE_TIME_ZONE_TESTS {
            let got = internal::parse_time_zone(value.as_bytes());
            let (length, ok) = (got.unwrap_or(0), got.is_some());
            assert_eq!(ok, want_ok, "expected {want_ok} for {value:?} got {ok}");
            assert_eq!(
                length, want_length,
                "expected {want_length} for {value:?} got {length}"
            );
        }
    }

    // (format, value, a string that must appear within the error)
    const PARSE_ERROR_TESTS: &[(&str, &str, &str)] = &[
        (
            ANSIC,
            "Feb  4 21:00:60 2010",
            r#"cannot parse "Feb  4 21:00:60 2010" as "Mon""#,
        ),
        (
            ANSIC,
            "Thu Feb  4 21:00:57 @2010",
            r#"cannot parse "@2010" as "2006""#,
        ),
        (ANSIC, "Thu Feb  4 21:00:60 2010", "second out of range"),
        (ANSIC, "Thu Feb  4 21:61:57 2010", "minute out of range"),
        (ANSIC, "Thu Feb  4 24:00:60 2010", "hour out of range"),
        (
            "Mon Jan _2 15:04:05.000 2006",
            "Thu Feb  4 23:00:59x01 2010",
            r#"cannot parse "x01 2010" as ".000""#,
        ),
        (
            "Mon Jan _2 15:04:05.000 2006",
            "Thu Feb  4 23:00:59.xxx 2010",
            r#"cannot parse ".xxx 2010" as ".000""#,
        ),
        (
            "Mon Jan _2 15:04:05.000 2006",
            "Thu Feb  4 23:00:59.-123 2010",
            "fractional second out of range",
        ),
        // issue 4502. StampNano requires exactly 9 digits of precision.
        (
            STAMP_NANO,
            "Dec  7 11:22:01.000000",
            r#"cannot parse ".000000" as ".000000000""#,
        ),
        (
            STAMP_NANO,
            "Dec  7 11:22:01.0000000000",
            r#"extra text: "0""#,
        ),
        // issue 4493. Helpful errors.
        (
            RFC3339,
            "2006-01-02T15:04:05Z07:00",
            r#"parsing time "2006-01-02T15:04:05Z07:00": extra text: "07:00""#,
        ),
        (
            RFC3339,
            "2006-01-02T15:04_abc",
            r#"parsing time "2006-01-02T15:04_abc" as "2006-01-02T15:04:05Z07:00": cannot parse "_abc" as ":""#,
        ),
        (
            RFC3339,
            "2006-01-02T15:04:05_abc",
            r#"parsing time "2006-01-02T15:04:05_abc" as "2006-01-02T15:04:05Z07:00": cannot parse "_abc" as "Z07:00""#,
        ),
        (
            RFC3339,
            "2006-01-02T15:04:05Z_abc",
            r#"parsing time "2006-01-02T15:04:05Z_abc": extra text: "_abc""#,
        ),
        // invalid second followed by optional fractional seconds
        (
            RFC3339,
            "2010-02-04T21:00:67.012345678-08:00",
            "second out of range",
        ),
        // issue 54569
        (
            RFC3339,
            "0000-01-01T00:00:.0+00:00",
            r#"parsing time "0000-01-01T00:00:.0+00:00" as "2006-01-02T15:04:05Z07:00": cannot parse ".0+00:00" as "05""#,
        ),
        // issue 21113
        (
            "_2 Jan 06 15:04 MST",
            "4 --- 00 00:00 GMT",
            r#"cannot parse "--- 00 00:00 GMT" as "Jan""#,
        ),
        (
            "_2 January 06 15:04 MST",
            "4 --- 00 00:00 GMT",
            r#"cannot parse "--- 00 00:00 GMT" as "January""#,
        ),
        // invalid or mismatched day-of-year
        (
            "Jan _2 002 2006",
            "Feb  4 034 2006",
            "day-of-year does not match day",
        ),
        (
            "Jan _2 002 2006",
            "Feb  4 004 2006",
            "day-of-year does not match month",
        ),
        // issue 45391.
        (
            r#""2006-01-02T15:04:05Z07:00""#,
            "0",
            r#"parsing time "0" as "\"2006-01-02T15:04:05Z07:00\"": cannot parse "0" as "\"""#,
        ),
        (
            RFC3339,
            "\"",
            r#"parsing time "\"" as "2006-01-02T15:04:05Z07:00": cannot parse "\"" as "2006""#,
        ),
        // issue 54570
        (
            RFC3339,
            "0000-01-01T00:00:00+00:+0",
            r#"parsing time "0000-01-01T00:00:00+00:+0" as "2006-01-02T15:04:05Z07:00": cannot parse "+00:+0" as "Z07:00""#,
        ),
        (
            RFC3339,
            "0000-01-01T00:00:00+-0:00",
            r#"parsing time "0000-01-01T00:00:00+-0:00" as "2006-01-02T15:04:05Z07:00": cannot parse "+-0:00" as "Z07:00""#,
        ),
        // issue 56730
        (
            "2006-01-02",
            "22-10-25",
            r#"parsing time "22-10-25" as "2006-01-02": cannot parse "22-10-25" as "2006""#,
        ),
        (
            "06-01-02",
            "a2-10-25",
            r#"parsing time "a2-10-25" as "06-01-02": cannot parse "a2-10-25" as "06""#,
        ),
        (
            "03:04PM",
            "12:03pM",
            r#"parsing time "12:03pM" as "03:04PM": cannot parse "pM" as "PM""#,
        ),
        (
            "03:04pm",
            "12:03pM",
            r#"parsing time "12:03pM" as "03:04pm": cannot parse "pM" as "pm""#,
        ),
        // issue 67470
        ("-07", "-25", "time zone offset hour out of range"),
        ("-07:00", "+25:00", "time zone offset hour out of range"),
        ("-07:00", "-23:61", "time zone offset minute out of range"),
        (
            "-07:00:00",
            "+23:59:61",
            "time zone offset second out of range",
        ),
        ("Z07", "-25", "time zone offset hour out of range"),
        ("Z07:00", "+25:00", "time zone offset hour out of range"),
        ("Z07:00", "-23:61", "time zone offset minute out of range"),
        (
            "Z07:00:00",
            "+23:59:61",
            "time zone offset second out of range",
        ),
    ];

    #[test]
    fn test_parse_errors() {
        go_setup();
        let mut errors = Vec::new();
        for &(format, value, expect) in PARSE_ERROR_TESTS {
            match parse(format, value) {
                Ok(_) => errors.push(format!("expected error for {format:?} {value:?}")),
                Err(err) => {
                    if !err.to_string().contains(expect) {
                        errors.push(format!(
                            "expected error with {expect:?} for {format:?} {value:?}; got {err}"
                        ));
                    }
                }
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_noon_is_12pm() {
        go_setup();
        let noon = date(0, Month::JANUARY, 1, 12, 0, 0, 0, &utc());
        const EXPECT: &str = "12:00PM";
        assert_eq!(noon.format("3:04PM"), EXPECT);
        assert_eq!(noon.format("03:04PM"), EXPECT);
    }

    #[test]
    fn test_midnight_is_12am() {
        go_setup();
        let midnight = date(0, Month::JANUARY, 1, 0, 0, 0, 0, &utc());
        let expect = "12:00AM";
        assert_eq!(midnight.format("3:04PM"), expect);
        assert_eq!(midnight.format("03:04PM"), expect);
    }

    #[test]
    fn test_12pm_is_noon() {
        go_setup();
        let noon = parse("3:04PM", "12:00PM").expect("error parsing date");
        assert_eq!(noon.hour(), 12);
        let noon = parse("03:04PM", "12:00PM").expect("error parsing date");
        assert_eq!(noon.hour(), 12);
    }

    #[test]
    fn test_12am_is_midnight() {
        go_setup();
        let midnight = parse("3:04PM", "12:00AM").expect("error parsing date");
        assert_eq!(midnight.hour(), 0);
        let midnight = parse("03:04PM", "12:00AM").expect("error parsing date");
        assert_eq!(midnight.hour(), 0);
    }

    // Check that a time without a Zone still produces a (numeric) time zone
    // when formatted with MST as a requested zone.
    #[test]
    fn test_missing_zone() {
        go_setup();
        let time = parse(RUBY_DATE, "Thu Feb 02 16:10:03 -0500 2006").expect("error parsing date");
        let expect = "Thu Feb  2 16:10:03 -0500 2006"; // -0500 not EST
        let s = time.format(UNIX_DATE); // uses MST as its time zone
        assert_eq!(s, expect);
    }

    #[test]
    fn test_minutes_in_time_zone() {
        go_setup();
        let time = parse(RUBY_DATE, "Mon Jan 02 15:04:05 +0123 2006").expect("error parsing date");
        let expected = (60 + 23) * 60;
        let (_, offset) = time.zone();
        assert_eq!(offset, expected, "ZoneOffset");
    }

    // (format, value, expected offset)
    const SECONDS_TIME_ZONE_OFFSET_TESTS: &[(&str, &str, i64)] = &[
        (
            "2006-01-02T15:04:05-070000",
            "1871-01-01T05:33:02-003408",
            -(34 * 60 + 8),
        ),
        (
            "2006-01-02T15:04:05-07:00:00",
            "1871-01-01T05:33:02-00:34:08",
            -(34 * 60 + 8),
        ),
        (
            "2006-01-02T15:04:05-070000",
            "1871-01-01T05:33:02+003408",
            34 * 60 + 8,
        ),
        (
            "2006-01-02T15:04:05-07:00:00",
            "1871-01-01T05:33:02+00:34:08",
            34 * 60 + 8,
        ),
        (
            "2006-01-02T15:04:05Z070000",
            "1871-01-01T05:33:02-003408",
            -(34 * 60 + 8),
        ),
        (
            "2006-01-02T15:04:05Z07:00:00",
            "1871-01-01T05:33:02+00:34:08",
            34 * 60 + 8,
        ),
        ("2006-01-02T15:04:05-07", "1871-01-01T05:33:02+01", 60 * 60),
        (
            "2006-01-02T15:04:05-07",
            "1871-01-01T05:33:02-02",
            -2 * 60 * 60,
        ),
        (
            "2006-01-02T15:04:05Z07",
            "1871-01-01T05:33:02-02",
            -2 * 60 * 60,
        ),
    ];

    #[test]
    fn test_parse_seconds_in_time_zone() {
        go_setup();
        // should accept timezone offsets with seconds like: Zone
        // America/New_York   -4:56:02 -      LMT     1883 Nov 18 12:03:58
        for &(format, value, expected_offset) in SECONDS_TIME_ZONE_OFFSET_TESTS {
            let time = parse(format, value).expect("error parsing date");
            let (_, offset) = time.zone();
            assert_eq!(offset, expected_offset, "ZoneOffset of {value:?}");
        }
    }

    #[test]
    fn test_format_seconds_in_time_zone() {
        go_setup();
        for &(format, value, expected_offset) in SECONDS_TIME_ZONE_OFFSET_TESTS {
            let d = date(
                1871,
                Month(1),
                1,
                5,
                33,
                2,
                0,
                &fixed_zone("LMT", expected_offset),
            );
            let timestr = d.format(format);
            assert_eq!(timestr, value, "Format");
        }
    }

    // Issue 11334.
    #[test]
    fn test_underscore_two_thousand() {
        go_setup();
        let format = "15:04_20060102";
        let input = "14:38_20150618";
        let time = parse(format, input).expect("parse");
        let (y, m, d) = time.date();
        assert!(
            y == 2015 && m.0 == 6 && d == 18,
            "Incorrect y/m/d, got {y}/{}/{d}",
            m.0
        );
        assert_eq!(time.hour(), 14, "Incorrect hour");
        assert_eq!(time.minute(), 38, "Incorrect minute");
    }

    // Issue 29918, 29916
    #[test]
    fn test_std0x_parse_error() {
        go_setup();
        let tests = [
            ("01 MST", "0 MST", "0"),
            ("01 MST", "1 MST", "1"),
            (RFC850, "Thursday, 04-Feb-1 21:00:57 PST", "1"),
        ];
        for (format, value, value_elem_prefix) in tests {
            // (Parse returns only ParseError, so Go's type check always holds.)
            match parse(format, value) {
                Ok(_) => panic!("Parse({format:?}, {value:?}) did not fail as expected"),
                Err(perr) => assert!(
                    perr.to_string().contains("cannot parse")
                        && perr.value_elem().starts_with(value_elem_prefix.as_bytes()),
                    "Parse({format:?}, {value:?}) returned wrong parsing error message: {perr}"
                ),
            }
        }
    }

    const MONTH_OUT_OF_RANGE_TESTS: &[(&str, bool)] =
        &[("00-01", false), ("13-01", false), ("01-01", true)];

    #[test]
    fn test_parse_month_out_of_range() {
        go_setup();
        for &(value, ok) in MONTH_OUT_OF_RANGE_TESTS {
            match (ok, parse("01-02", value)) {
                (false, Err(err)) => assert!(
                    err.to_string().contains("month out of range"),
                    "{value:?}: expected 'month' error, got {err}"
                ),
                (true, Err(err)) => panic!("{value:?}: unexpected error: {err}"),
                (false, Ok(_)) => panic!("{value:?}: expected 'month' error, got none"),
                (true, Ok(_)) => {}
            }
        }
    }

    // Issue 37387.
    #[test]
    fn test_parse_yday() {
        go_setup();
        for i in 1..=365 {
            let d = format!("2020-{i:03}");
            let tm = parse("2006-002", &d)
                .unwrap_or_else(|err| panic!("unexpected error for {d}: {err}"));
            assert!(
                tm.year() == 2020 && tm.year_day() == i,
                "got year {} yearday {}, want {} {i}",
                tm.year(),
                tm.year_day(),
                2020
            );
        }
    }

    // Issue 45391.
    #[test]
    fn test_quote() {
        go_setup();
        let tests = [
            (r#"""#, r#""\"""#),
            (r#"abc"xyz""#, r#""abc\"xyz\"""#),
            ("", r#""""#),
            ("abc", r#""abc""#),
            ("☺", r#""\xe2\x98\xba""#),
            (
                "☺ hello ☺ hello",
                r#""\xe2\x98\xba hello \xe2\x98\xba hello""#,
            ),
            ("\x04", r#""\x04""#),
        ];
        for (s, want) in tests {
            assert_eq!(internal::quote(s.as_bytes()), want, "Quote({s:?})");
        }
    }

    // Issue 48037
    #[test]
    fn test_format_fractional_second_separators() {
        go_setup();
        let tests = [
            ("15:04:05.000", "21:00:57.012"),
            ("15:04:05.999", "21:00:57.012"),
            ("15:04:05,000", "21:00:57,012"),
            ("15:04:05,999", "21:00:57,012"),
        ];

        // The numeric time represents Thu Feb  4 21:00:57.012345600 PST 2009
        let time = unix(0, 1233810057012345600);
        for (s, want) in tests {
            assert_eq!(time.format(s), want, "Format({s:?})");
        }
    }

    const LONG_FRACTIONAL_DIGITS_TESTS: &[(&str, i64)] = &[
        // 9 digits
        ("2021-09-29T16:04:33.000000000Z", 0),
        ("2021-09-29T16:04:33.000000001Z", 1),
        ("2021-09-29T16:04:33.100000000Z", 100_000_000),
        ("2021-09-29T16:04:33.100000001Z", 100_000_001),
        ("2021-09-29T16:04:33.999999999Z", 999_999_999),
        ("2021-09-29T16:04:33.012345678Z", 12_345_678),
        // 10 digits, truncates
        ("2021-09-29T16:04:33.0000000000Z", 0),
        ("2021-09-29T16:04:33.0000000001Z", 0),
        ("2021-09-29T16:04:33.1000000000Z", 100_000_000),
        ("2021-09-29T16:04:33.1000000009Z", 100_000_000),
        ("2021-09-29T16:04:33.9999999999Z", 999_999_999),
        ("2021-09-29T16:04:33.0123456789Z", 12_345_678),
        // 11 digits, truncates
        ("2021-09-29T16:04:33.10000000000Z", 100_000_000),
        ("2021-09-29T16:04:33.00123456789Z", 1_234_567),
        // 12 digits, truncates
        ("2021-09-29T16:04:33.000123456789Z", 123_456),
        // 15 digits, truncates
        ("2021-09-29T16:04:33.9999999999999999Z", 999_999_999),
    ];

    // Issue 48685 and 54567.
    #[test]
    fn test_parse_fractional_seconds_longer_than_nine_digits() {
        go_setup();
        for &(value, want) in LONG_FRACTIONAL_DIGITS_TESTS {
            for format in [RFC3339, RFC3339_NANO] {
                let tm = parse(format, value)
                    .unwrap_or_else(|err| panic!("Parse({format:?}, {value:?}) error: {err}"));
                assert_eq!(tm.nanosecond(), want, "Parse({format:?}, {value:?})");
            }
        }
    }

    /// The seed corpus of Go's `FuzzFormatRFC3339`.
    #[test]
    fn fuzz_format_rfc3339() {
        go_setup();
        let mut seeds: Vec<(i64, i64, bool, bool, i64)> = Vec::new();
        for ts in [
            (i64::MIN, i64::MIN),      // 292277026304-08-26T15:42:51Z
            (-62167219200, 0),         // 0000-01-01T00:00:00Z
            (1661201140, 676836973),   // 2022-08-22T20:45:40.676836973Z
            (253402300799, 999999999), // 9999-12-31T23:59:59.999999999Z
            (i64::MAX, i64::MAX),      // -292277022365-05-08T08:17:07Z
        ] {
            seeds.push((ts.0, ts.1, true, false, 0));
            seeds.push((ts.0, ts.1, false, true, 0));
            for offset in [0, 60, 60 * 60, 99 * 60 * 60 + 99 * 60, 123456789] {
                seeds.push((ts.0, ts.1, false, false, -offset));
                seeds.push((ts.0, ts.1, false, false, offset));
            }
        }

        for (sec, nsec, use_utc, use_local, tz_offset) in seeds {
            let loc = if use_utc {
                utc()
            } else if use_local {
                local()
            } else {
                fixed_zone("", tz_offset)
            };
            let ts = unix(sec, nsec).in_location(&loc);

            let mut got = Vec::new();
            ts.append_format_rfc3339(&mut got, false);
            let mut want = Vec::new();
            ts.append_format_any(&mut want, RFC3339.as_bytes());
            assert_eq!(
                got,
                want,
                "Format({ts}, RFC3339) mismatch:\n\tgot:  {}\n\twant: {}",
                String::from_utf8_lossy(&got),
                String::from_utf8_lossy(&want)
            );

            let mut got_nanos = Vec::new();
            ts.append_format_rfc3339(&mut got_nanos, true);
            let mut want_nanos = Vec::new();
            ts.append_format_any(&mut want_nanos, RFC3339_NANO.as_bytes());
            assert_eq!(
                got_nanos,
                want_nanos,
                "Format({ts}, RFC3339Nano) mismatch:\n\tgot:  {}\n\twant: {}",
                String::from_utf8_lossy(&got_nanos),
                String::from_utf8_lossy(&want_nanos)
            );
        }
    }

    /// The seed corpus of Go's `FuzzParseRFC3339`.
    #[test]
    fn fuzz_parse_rfc3339() {
        go_setup();
        let mut seeds: Vec<&str> = Vec::new();
        seeds.extend(FORMAT_TESTS.iter().map(|tt| tt.result));
        seeds.extend(PARSE_TESTS.iter().map(|tt| tt.value));
        seeds.extend(PARSE_ERROR_TESTS.iter().map(|tt| tt.1));
        seeds.extend(LONG_FRACTIONAL_DIGITS_TESTS.iter().map(|tt| tt.0));

        // equalTime is like time.Time.Equal, but also compares the time zone.
        let equal_time = |t1: &Time, t2: &Time| {
            let (name1, offset1) = t1.zone();
            let (name2, offset2) = t2.zone();
            t1.equal(t2) && name1 == name2 && offset1 == offset2
        };

        let mut skipped = Vec::new();
        'seeds: for s in seeds {
            let tzs: [Location; 2] = [utc(), local()];
            for tz in &tzs {
                // Parsing as RFC3339 or RFC3339Nano should be identical.
                let r1 = internal::parse_any(RFC3339.as_bytes(), s.as_bytes(), &utc(), tz);
                let r2 = internal::parse_any(RFC3339_NANO.as_bytes(), s.as_bytes(), &utc(), tz);
                let (t1, t2) = match (&r1, &r2) {
                    (Ok(t1), Ok(t2)) => (t1.clone(), t2.clone()),
                    (Err(_), Err(_)) => (Time::default(), Time::default()),
                    _ => panic!("ParseAny({s:?}) error mismatch:\n\tgot:  {r1:?}\n\twant: {r2:?}"),
                };
                assert!(
                    equal_time(&t1, &t2),
                    "ParseAny({s:?}) value mismatch:\n\tgot:  {t1}\n\twant: {t2}"
                );

                // TODO(https://go.dev/issue/54580): Remove these checks after
                // ParseAny rejects all invalid RFC 3339.
                if r1.is_ok() {
                    let b = s.as_bytes();
                    let num2 = |s: &[u8]| {
                        10u8.wrapping_mul(s[0].wrapping_sub(b'0'))
                            .wrapping_add(s[1].wrapping_sub(b'0'))
                    };
                    let skip = if b.len() > 12 && b[12] == b':' {
                        Some("incorrectly allows single-digit hour fields")
                    } else if b.len() > 19 && b[19] == b',' {
                        Some("incorrectly allows comma as sub-second separator")
                    } else if !s.ends_with('Z')
                        && b.len() > 4
                        && (num2(&b[b.len() - 5..]) >= 24 || num2(&b[b.len() - 2..]) >= 60)
                    {
                        Some("incorrectly allows out-of-range zone offset")
                    } else {
                        None
                    };
                    if let Some(why) = skip {
                        // Go's t.Skipf ends this seed's run.
                        skipped.push(format!("ParseAny({s:?}) {why}"));
                        continue 'seeds;
                    }
                }

                // Customized parser should be identical to general parser.
                let got = internal::parse_rfc3339(s.as_bytes(), tz);
                assert_eq!(
                    got.is_some(),
                    r1.is_ok(),
                    "ParseRFC3339({s:?}) error mismatch:\n\tgot:  {}\n\twant: {}",
                    got.is_some(),
                    r1.is_ok()
                );
                let got = got.unwrap_or_default();
                assert!(
                    equal_time(&got, &t1),
                    "ParseRFC3339({s:?}) value mismatch:\n\tgot:  {got}\n\twant: {t2}"
                );
            }
        }
        // Go reports these seeds as skipped; print them the same way.
        for s in skipped {
            println!("skip: {s}");
        }
    }
}
