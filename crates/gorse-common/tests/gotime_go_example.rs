// Ports: $GOROOT/src/time/example_test.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package tests: Copyright 2011 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `example_test.go`. `go test` runs every example that ends in an
//! `// Output:` comment and compares what it printed with that comment; each
//! such example is a test here that builds the same text and checks it the
//! way `go test` does ([`check_output`]), against the comment copied verbatim.
//!
//! Go runs the examples in the time package's test binary, where `time.Local`
//! is America/Los_Angeles named "Local" (`ForceUSPacificForTesting`), which
//! [`common::go_setup`] reproduces.
//!
//! Not ported, because `go test` does not run them (they have no `// Output:`
//! comment): `ExampleDuration`, `ExampleSince`, `ExampleAfter`,
//! `ExampleSleep`, `ExampleTick`, `ExampleMonth` and `ExampleNewTicker`.

mod common;

mod gotime {
    use gorse_common::gostrconv::quote;
    use gorse_common::gotime::{
        self, Duration, HOUR, KITCHEN, MICROSECOND, MILLISECOND, MINUTE, Month, NANOSECOND, RFC822,
        RFC3339, SECOND, Time, UNIX_DATE, date, fixed_zone, load_location, local, now, parse,
        parse_duration, parse_in_location, unix, unix_micro, unix_milli, until, utc,
    };

    use crate::common::go_setup;

    /// Go's `stripTrailingWhitespace` (go/ast).
    fn strip_trailing_whitespace(s: &str) -> &str {
        s.trim_end_matches([' ', '\t', '\n', '\r'])
    }

    /// The expected output `go test` takes from an example's last comment:
    /// `CommentGroup.Text` (markers and one leading space removed, trailing
    /// white space stripped, leading blank lines dropped, runs of blank lines
    /// folded), then the text after `Output:` (go/doc `exampleOutput`).
    fn example_output(comment: &[&str]) -> String {
        let mut lines: Vec<&str> = Vec::new();
        for c in comment {
            let mut c = c.strip_prefix("//").expect("a // comment");
            if let Some(rest) = c.strip_prefix(' ') {
                c = rest;
            }
            lines.push(strip_trailing_whitespace(c));
        }
        let mut kept: Vec<&str> = Vec::new();
        for line in lines {
            if !line.is_empty() || kept.last().is_some_and(|l| !l.is_empty()) {
                kept.push(line);
            }
        }
        if kept.last().is_some_and(|l| !l.is_empty()) {
            kept.push("");
        }
        let text = kept.join("\n");

        // outputPrefix is `(?i)^[[:space:]]*(unordered )?output:`; these
        // examples all use "Output:".
        let text = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
        let text = text
            .strip_prefix("Output:")
            .expect("the comment starts with Output:");
        // Strip zero or more spaces followed by \n or a single space.
        let text = text.trim_start_matches(' ');
        text.strip_prefix('\n').unwrap_or(text).to_owned()
    }

    /// `go test`'s check of an example (testing `processRunResult`): the
    /// printed text and the expected output must be equal after
    /// `strings.TrimSpace`.
    fn check_output(stdout: &str, comment: &[&str]) {
        let want = example_output(comment);
        let got = stdout.trim();
        let want = want.trim();
        assert_eq!(got, want, "got:\n{got}\nwant:\n{want}\n");
    }

    /// Go's `%<width>s`: right-justified, padded with spaces to width runes.
    fn pad_left(s: &str, width: usize) -> String {
        format!("{s:>width$}")
    }

    /// Go's `%<width>.<prec>e` for a finite float: d.dddde±dd, right-justified.
    fn go_e(f: f64, width: usize, prec: usize) -> String {
        let s = format!("{f:.prec$e}");
        let (mantissa, exp) = s.split_once('e').expect("Rust writes an exponent");
        let exp: i32 = exp.parse().expect("exponent");
        let sign = if exp < 0 { '-' } else { '+' };
        pad_left(&format!("{mantissa}e{sign}{:02}", exp.abs()), width)
    }

    #[test]
    fn example_duration_round() {
        go_setup();
        let mut out = String::new();
        let d = parse_duration("1h15m30.918273645s").unwrap_or_else(|e| panic!("{e}"));

        let round = [
            NANOSECOND,
            MICROSECOND,
            MILLISECOND,
            SECOND,
            Duration(2 * SECOND.0),
            MINUTE,
            Duration(10 * MINUTE.0),
            HOUR,
        ];

        for r in round {
            out += &format!(
                "d.Round({}) = {}\n",
                pad_left(&r.to_string(), 6),
                d.round(r)
            );
        }
        check_output(
            &out,
            &[
                "// Output:",
                "// d.Round(   1ns) = 1h15m30.918273645s",
                "// d.Round(   1µs) = 1h15m30.918274s",
                "// d.Round(   1ms) = 1h15m30.918s",
                "// d.Round(    1s) = 1h15m31s",
                "// d.Round(    2s) = 1h15m30s",
                "// d.Round(  1m0s) = 1h16m0s",
                "// d.Round( 10m0s) = 1h20m0s",
                "// d.Round(1h0m0s) = 1h0m0s",
            ],
        );
    }

    #[test]
    fn example_duration_string() {
        go_setup();
        let mut out = String::new();
        out += &format!(
            "{}\n",
            Duration(HOUR.0 + 2 * MINUTE.0 + 300 * MILLISECOND.0)
        );
        out += &format!("{}\n", Duration(300 * MILLISECOND.0));
        check_output(&out, &["// Output:", "// 1h2m0.3s", "// 300ms"]);
    }

    #[test]
    fn example_duration_truncate() {
        go_setup();
        let mut out = String::new();
        let d = parse_duration("1h15m30.918273645s").unwrap_or_else(|e| panic!("{e}"));

        let trunc = [
            NANOSECOND,
            MICROSECOND,
            MILLISECOND,
            SECOND,
            Duration(2 * SECOND.0),
            MINUTE,
            Duration(10 * MINUTE.0),
            HOUR,
        ];

        for t in trunc {
            out += &format!(
                "d.Truncate({}) = {}\n",
                pad_left(&t.to_string(), 6),
                d.truncate(t)
            );
        }
        check_output(
            &out,
            &[
                "// Output:",
                "// d.Truncate(   1ns) = 1h15m30.918273645s",
                "// d.Truncate(   1µs) = 1h15m30.918273s",
                "// d.Truncate(   1ms) = 1h15m30.918s",
                "// d.Truncate(    1s) = 1h15m30s",
                "// d.Truncate(    2s) = 1h15m30s",
                "// d.Truncate(  1m0s) = 1h15m0s",
                "// d.Truncate( 10m0s) = 1h10m0s",
                "// d.Truncate(1h0m0s) = 1h0m0s",
            ],
        );
    }

    #[test]
    fn example_parse_duration() {
        go_setup();
        let mut out = String::new();
        // Go ignores these errors; a failed parse gives 0.
        let hours = parse_duration("10h").unwrap_or_default();
        let complex = parse_duration("1h10m10s").unwrap_or_default();
        let micro = parse_duration("1µs").unwrap_or_default();
        // The package also accepts the incorrect but common prefix u for micro.
        let micro2 = parse_duration("1us").unwrap_or_default();

        out += &format!("{hours}\n");
        out += &format!("{complex}\n");
        out += &format!("There are {:.0} seconds in {complex}.\n", complex.seconds());
        out += &format!(
            "There are {} nanoseconds in {micro}.\n",
            micro.nanoseconds()
        );
        out += &format!(
            "There are {} seconds in {micro2}.\n",
            go_e(micro2.seconds(), 6, 2)
        );
        check_output(
            &out,
            &[
                "// Output:",
                "// 10h0m0s",
                "// 1h10m10s",
                "// There are 4210 seconds in 1h10m10s.",
                "// There are 1000 nanoseconds in 1µs.",
                "// There are 1.00e-06 seconds in 1µs.",
            ],
        );
    }

    #[test]
    fn example_until() {
        go_setup();
        let future_time = now().add(Duration(5 * SECOND.0));
        let duration_until = until(&future_time);
        let out = format!(
            "Duration until future time: {:.0} seconds",
            duration_until.seconds().ceil()
        );
        check_output(&out, &["// Output: Duration until future time: 5 seconds"]);
    }

    #[test]
    fn example_duration_abs() {
        go_setup();
        let mut out = String::new();
        let positive_duration = Duration(5 * SECOND.0);
        let negative_duration = Duration(-3 * SECOND.0);
        let min_int64_case_duration = Duration(i64::MIN);

        let abs_positive = positive_duration.abs();
        let abs_negative = negative_duration.abs();
        let abs_special = min_int64_case_duration.abs() == Duration(i64::MAX);

        out += &format!("Absolute value of positive duration: {abs_positive}\n");
        out += &format!("Absolute value of negative duration: {abs_negative}\n");
        out += &format!("Absolute value of MinInt64 equal to MaxInt64: {abs_special}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// Absolute value of positive duration: 5s",
                "// Absolute value of negative duration: 3s",
                "// Absolute value of MinInt64 equal to MaxInt64: true",
            ],
        );
    }

    #[test]
    fn example_duration_hours() {
        go_setup();
        let h = parse_duration("4h30m").unwrap_or_default();
        let out = format!("I've got {:.1} hours of work left.", h.hours());
        check_output(&out, &["// Output: I've got 4.5 hours of work left."]);
    }

    #[test]
    fn example_duration_microseconds() {
        go_setup();
        let u = parse_duration("1s").unwrap_or_default();
        let out = format!("One second is {} microseconds.\n", u.microseconds());
        check_output(
            &out,
            &["// Output:", "// One second is 1000000 microseconds."],
        );
    }

    #[test]
    fn example_duration_milliseconds() {
        go_setup();
        let u = parse_duration("1s").unwrap_or_default();
        let out = format!("One second is {} milliseconds.\n", u.milliseconds());
        check_output(&out, &["// Output:", "// One second is 1000 milliseconds."]);
    }

    #[test]
    fn example_duration_minutes() {
        go_setup();
        let m = parse_duration("1h30m").unwrap_or_default();
        let out = format!("The movie is {:.0} minutes long.", m.minutes());
        check_output(&out, &["// Output: The movie is 90 minutes long."]);
    }

    #[test]
    fn example_duration_nanoseconds() {
        go_setup();
        let u = parse_duration("1µs").unwrap_or_default();
        let out = format!("One microsecond is {} nanoseconds.\n", u.nanoseconds());
        check_output(
            &out,
            &["// Output:", "// One microsecond is 1000 nanoseconds."],
        );
    }

    #[test]
    fn example_duration_seconds() {
        go_setup();
        let m = parse_duration("1m30s").unwrap_or_default();
        let out = format!("Take off in t-{:.0} seconds.", m.seconds());
        check_output(&out, &["// Output: Take off in t-90 seconds."]);
    }

    #[test]
    fn example_date() {
        go_setup();
        let t = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &utc());
        let out = format!("Go launched at {}\n", t.local());
        check_output(
            &out,
            &["// Output: Go launched at 2009-11-10 15:00:00 -0800 PST"],
        );
    }

    /// The `do` helper of `ExampleTime_Format` and `ExampleTime_Format_pad`.
    fn format_do(out: &mut String, t: &Time, name: &str, layout: &str, want: &str) {
        let got = t.format(layout);
        if want != got {
            *out += &format!(
                "error: for {} got {}; expected {}\n",
                quote(layout),
                quote(&got),
                quote(want)
            );
            return;
        }
        *out += &format!("{name:<16} {} gives {}\n", quote(layout), quote(&got));
    }

    #[test]
    fn example_time_format() {
        go_setup();
        let mut out = String::new();
        // Parse a time value from a string in the standard Unix format.
        let t = parse(UNIX_DATE, "Wed Feb 25 11:06:39 PST 2015").unwrap_or_else(|e| panic!("{e}"));

        let tz = load_location("Asia/Shanghai").unwrap_or_else(|e| panic!("{e}"));

        // time.Time's Stringer method is useful without any format.
        out += &format!("default format: {t}\n");

        // Predefined constants in the package implement common layouts.
        out += &format!("Unix format: {}\n", t.format(UNIX_DATE));

        // The time zone attached to the time value affects its output.
        out += &format!("Same, in UTC: {}\n", t.utc().format(UNIX_DATE));

        out += &format!(
            "in Shanghai with seconds: {}\n",
            t.in_location(&tz).format("2006-01-02T15:04:05 -070000")
        );

        out += &format!(
            "in Shanghai with colon seconds: {}\n",
            t.in_location(&tz).format("2006-01-02T15:04:05 -07:00:00")
        );

        // Print a header in our output.
        out += "\nFormats:\n\n";

        // Simple starter examples.
        format_do(
            &mut out,
            &t,
            "Basic full date",
            "Mon Jan 2 15:04:05 MST 2006",
            "Wed Feb 25 11:06:39 PST 2015",
        );
        format_do(&mut out, &t, "Basic short date", "2006/01/02", "2015/02/25");

        // The hour of the reference time is 15, or 3PM. The layout can express
        // it either way, and since our value is the morning we should see it as
        // an AM time. We show both in one format string. Lower case too.
        format_do(&mut out, &t, "AM/PM", "3PM==3pm==15h", "11AM==11am==11h");

        // When parsing, if the seconds value is followed by a decimal point
        // and some digits, that is taken as a fraction of a second even if
        // the layout string does not represent the fractional second.
        // Here we add a fractional second to our time value used above.
        let t =
            parse(UNIX_DATE, "Wed Feb 25 11:06:39.1234 PST 2015").unwrap_or_else(|e| panic!("{e}"));
        // It does not appear in the output if the layout string does not contain
        // a representation of the fractional second.
        format_do(
            &mut out,
            &t,
            "No fraction",
            UNIX_DATE,
            "Wed Feb 25 11:06:39 PST 2015",
        );

        // Fractional seconds can be printed by adding a run of 0s or 9s after
        // a decimal point in the seconds value in the layout string.
        // If the layout digits are 0s, the fractional second is of the specified
        // width. Note that the output has a trailing zero.
        format_do(
            &mut out,
            &t,
            "0s for fraction",
            "15:04:05.00000",
            "11:06:39.12340",
        );

        // If the fraction in the layout is 9s, trailing zeros are dropped.
        format_do(
            &mut out,
            &t,
            "9s for fraction",
            "15:04:05.99999999",
            "11:06:39.1234",
        );

        check_output(
            &out,
            &[
                "// Output:",
                "// default format: 2015-02-25 11:06:39 -0800 PST",
                "// Unix format: Wed Feb 25 11:06:39 PST 2015",
                "// Same, in UTC: Wed Feb 25 19:06:39 UTC 2015",
                "// in Shanghai with seconds: 2015-02-26T03:06:39 +080000",
                "// in Shanghai with colon seconds: 2015-02-26T03:06:39 +08:00:00",
                "//",
                "// Formats:",
                "//",
                r#"// Basic full date  "Mon Jan 2 15:04:05 MST 2006" gives "Wed Feb 25 11:06:39 PST 2015""#,
                r#"// Basic short date "2006/01/02" gives "2015/02/25""#,
                r#"// AM/PM            "3PM==3pm==15h" gives "11AM==11am==11h""#,
                r#"// No fraction      "Mon Jan _2 15:04:05 MST 2006" gives "Wed Feb 25 11:06:39 PST 2015""#,
                r#"// 0s for fraction  "15:04:05.00000" gives "11:06:39.12340""#,
                r#"// 9s for fraction  "15:04:05.99999999" gives "11:06:39.1234""#,
            ],
        );
    }

    #[test]
    fn example_time_format_pad() {
        go_setup();
        let mut out = String::new();
        // Parse a time value from a string in the standard Unix format.
        let t = parse(UNIX_DATE, "Sat Mar 7 11:06:39 PST 2015").unwrap_or_else(|e| panic!("{e}"));

        // The predefined constant Unix uses an underscore to pad the day.
        format_do(
            &mut out,
            &t,
            "Unix",
            UNIX_DATE,
            "Sat Mar  7 11:06:39 PST 2015",
        );

        // For fixed-width printing of values, such as the date, that may be one or
        // two characters (7 vs. 07), use an _ instead of a space in the layout string.
        // Here we print just the day, which is 2 in our layout string and 7 in our
        // value.
        format_do(&mut out, &t, "No pad", "<2>", "<7>");

        // An underscore represents a space pad, if the date only has one digit.
        format_do(&mut out, &t, "Spaces", "<_2>", "< 7>");

        // A "0" indicates zero padding for single-digit values.
        format_do(&mut out, &t, "Zeros", "<02>", "<07>");

        // If the value is already the right width, padding is not used.
        // For instance, the second (05 in the reference time) in our value is 39,
        // so it doesn't need padding, but the minutes (04, 06) does.
        format_do(&mut out, &t, "Suppressed pad", "04:05", "06:39");

        check_output(
            &out,
            &[
                "// Output:",
                r#"// Unix             "Mon Jan _2 15:04:05 MST 2006" gives "Sat Mar  7 11:06:39 PST 2015""#,
                r#"// No pad           "<2>" gives "<7>""#,
                r#"// Spaces           "<_2>" gives "< 7>""#,
                r#"// Zeros            "<02>" gives "<07>""#,
                r#"// Suppressed pad   "04:05" gives "06:39""#,
            ],
        );
    }

    #[test]
    fn example_time_go_string() {
        go_setup();
        let mut out = String::new();
        let mut t = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &utc());
        out += &format!("{}\n", t.go_string());
        t = t.add(MINUTE);
        out += &format!("{}\n", t.go_string());
        t = t.add_date(0, 1, 0);
        out += &format!("{}\n", t.go_string());
        // Go ignores the error; a failed parse gives the zero Time.
        t = parse("Jan 2, 2006 at 3:04pm (MST)", "Feb 3, 2013 at 7:54pm (UTC)").unwrap_or_default();
        out += &format!("{}\n", t.go_string());

        check_output(
            &out,
            &[
                "// Output:",
                "// time.Date(2009, time.November, 10, 23, 0, 0, 0, time.UTC)",
                "// time.Date(2009, time.November, 10, 23, 1, 0, 0, time.UTC)",
                "// time.Date(2009, time.December, 10, 23, 1, 0, 0, time.UTC)",
                "// time.Date(2013, time.February, 3, 19, 54, 0, 0, time.UTC)",
            ],
        );
    }

    #[test]
    fn example_parse() {
        go_setup();
        let mut out = String::new();
        // longForm shows by example how the reference time would be represented in
        // the desired layout.
        const LONG_FORM: &str = "Jan 2, 2006 at 3:04pm (MST)";
        // Go ignores these errors; a failed parse gives the zero Time.
        let t = parse(LONG_FORM, "Feb 3, 2013 at 7:54pm (PST)").unwrap_or_default();
        out += &format!("{t}\n");

        // shortForm is another way the reference time would be represented
        // in the desired layout; it has no time zone present.
        // Note: without explicit zone, returns time in UTC.
        const SHORT_FORM: &str = "2006-Jan-02";
        let t = parse(SHORT_FORM, "2013-Feb-03").unwrap_or_default();
        out += &format!("{t}\n");

        // Some valid layouts are invalid time values, due to format specifiers
        // such as _ for space padding and Z for zone information.
        // For example the RFC3339 layout 2006-01-02T15:04:05Z07:00
        // contains both Z and a time zone offset in order to handle both valid options:
        // 2006-01-02T15:04:05Z
        // 2006-01-02T15:04:05+07:00
        let t = parse(RFC3339, "2006-01-02T15:04:05Z").unwrap_or_default();
        out += &format!("{t}\n");
        let t = parse(RFC3339, "2006-01-02T15:04:05+07:00").unwrap_or_default();
        out += &format!("{t}\n");
        // Returns an error as the layout is not a valid time value. (Go prints
        // a nil error as "<nil>".)
        let err = match parse(RFC3339, RFC3339) {
            Ok(_) => "<nil>".to_owned(),
            Err(e) => e.to_string(),
        };
        out += &format!("error {err}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// 2013-02-03 19:54:00 -0800 PST",
                "// 2013-02-03 00:00:00 +0000 UTC",
                "// 2006-01-02 15:04:05 +0000 UTC",
                "// 2006-01-02 15:04:05 +0700 +0700",
                r#"// error parsing time "2006-01-02T15:04:05Z07:00": extra text: "07:00""#,
            ],
        );
    }

    #[test]
    fn example_parse_in_location() {
        go_setup();
        let mut out = String::new();
        // Go ignores these errors.
        let loc = load_location("Europe/Berlin").unwrap_or_else(|_| utc());

        // This will look for the name CEST in the Europe/Berlin time zone.
        const LONG_FORM: &str = "Jan 2, 2006 at 3:04pm (MST)";
        let t =
            parse_in_location(LONG_FORM, "Jul 9, 2012 at 5:02am (CEST)", &loc).unwrap_or_default();
        out += &format!("{t}\n");

        // Note: without explicit zone, returns time in given location.
        const SHORT_FORM: &str = "2006-Jan-02";
        let t = parse_in_location(SHORT_FORM, "2012-Jul-09", &loc).unwrap_or_default();
        out += &format!("{t}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// 2012-07-09 05:02:00 +0200 CEST",
                "// 2012-07-09 00:00:00 +0200 CEST",
            ],
        );
    }

    #[test]
    fn example_unix() {
        go_setup();
        let mut out = String::new();
        let unix_time = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &utc());
        out += &format!("{}\n", unix_time.unix());
        let t = unix(unix_time.unix(), 0).utc();
        out += &format!("{t}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// 1257894000",
                "// 2009-11-10 23:00:00 +0000 UTC",
            ],
        );
    }

    #[test]
    fn example_unix_micro() {
        go_setup();
        let mut out = String::new();
        let umt = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &utc());
        out += &format!("{}\n", umt.unix_micro());
        let t = unix_micro(umt.unix_micro()).utc();
        out += &format!("{t}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// 1257894000000000",
                "// 2009-11-10 23:00:00 +0000 UTC",
            ],
        );
    }

    #[test]
    fn example_unix_milli() {
        go_setup();
        let mut out = String::new();
        let umt = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &utc());
        out += &format!("{}\n", umt.unix_milli());
        let t = unix_milli(umt.unix_milli()).utc();
        out += &format!("{t}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// 1257894000000",
                "// 2009-11-10 23:00:00 +0000 UTC",
            ],
        );
    }

    #[test]
    fn example_time_unix() {
        go_setup();
        let mut out = String::new();
        // 1 billion seconds of Unix, three ways.
        out += &format!("{}\n", unix(1_000_000_000, 0).utc()); // 1e9 seconds
        out += &format!("{}\n", unix(0, 1_000_000_000_000_000_000).utc()); // 1e18 nanoseconds
        out += &format!(
            "{}\n",
            unix(2_000_000_000, -1_000_000_000_000_000_000).utc()
        ); // 2e9 seconds - 1e18 nanoseconds

        let t = date(2001, Month::SEPTEMBER, 9, 1, 46, 40, 0, &utc());
        out += &format!("{}\n", t.unix()); // seconds since 1970
        out += &format!("{}\n", t.unix_nano()); // nanoseconds since 1970

        check_output(
            &out,
            &[
                "// Output:",
                "// 2001-09-09 01:46:40 +0000 UTC",
                "// 2001-09-09 01:46:40 +0000 UTC",
                "// 2001-09-09 01:46:40 +0000 UTC",
                "// 1000000000",
                "// 1000000000000000000",
            ],
        );
    }

    #[test]
    fn example_time_round() {
        go_setup();
        let mut out = String::new();
        let t = date(0, Month(0), 0, 12, 15, 30, 918273645, &utc());
        let round = [
            NANOSECOND,
            MICROSECOND,
            MILLISECOND,
            SECOND,
            Duration(2 * SECOND.0),
            MINUTE,
            Duration(10 * MINUTE.0),
            HOUR,
        ];

        for d in round {
            out += &format!(
                "t.Round({}) = {}\n",
                pad_left(&d.to_string(), 6),
                t.round(d).format("15:04:05.999999999")
            );
        }
        check_output(
            &out,
            &[
                "// Output:",
                "// t.Round(   1ns) = 12:15:30.918273645",
                "// t.Round(   1µs) = 12:15:30.918274",
                "// t.Round(   1ms) = 12:15:30.918",
                "// t.Round(    1s) = 12:15:31",
                "// t.Round(    2s) = 12:15:30",
                "// t.Round(  1m0s) = 12:16:00",
                "// t.Round( 10m0s) = 12:20:00",
                "// t.Round(1h0m0s) = 12:00:00",
            ],
        );
    }

    #[test]
    fn example_time_truncate() {
        go_setup();
        let mut out = String::new();
        // Go ignores the error; a failed parse gives the zero Time.
        let t = parse("2006 Jan 02 15:04:05", "2012 Dec 07 12:15:30.918273645").unwrap_or_default();
        let trunc = [
            NANOSECOND,
            MICROSECOND,
            MILLISECOND,
            SECOND,
            Duration(2 * SECOND.0),
            MINUTE,
            Duration(10 * MINUTE.0),
        ];

        for d in trunc {
            out += &format!(
                "t.Truncate({}) = {}\n",
                pad_left(&d.to_string(), 5),
                t.truncate(d).format("15:04:05.999999999")
            );
        }
        // To round to the last midnight in the local timezone, create a new Date.
        let midnight = date(t.year(), t.month(), t.day(), 0, 0, 0, 0, &local());
        let _ = midnight;

        check_output(
            &out,
            &[
                "// Output:",
                "// t.Truncate(  1ns) = 12:15:30.918273645",
                "// t.Truncate(  1µs) = 12:15:30.918273",
                "// t.Truncate(  1ms) = 12:15:30.918",
                "// t.Truncate(   1s) = 12:15:30",
                "// t.Truncate(   2s) = 12:15:30",
                "// t.Truncate( 1m0s) = 12:15:00",
                "// t.Truncate(10m0s) = 12:10:00",
            ],
        );
    }

    #[test]
    fn example_load_location() {
        go_setup();
        let location = load_location("America/Los_Angeles").unwrap_or_else(|e| panic!("{e}"));

        let time_in_utc = date(2018, Month(8), 30, 12, 0, 0, 0, &utc());
        let out = format!("{}\n", time_in_utc.in_location(&location));
        check_output(&out, &["// Output: 2018-08-30 05:00:00 -0700 PDT"]);
    }

    #[test]
    fn example_location() {
        go_setup();
        // China doesn't have daylight saving. It uses a fixed 8 hour offset from UTC.
        // (Go's int(float64) conversion truncates.)
        let seconds_east_of_utc = Duration(8 * HOUR.0).seconds() as i64;
        let beijing = fixed_zone("Beijing Time", seconds_east_of_utc);

        // Creating a time requires a location. Common locations are time.Local and time.UTC.
        let time_in_utc = date(2009, Month(1), 1, 12, 0, 0, 0, &utc());
        let same_time_in_beijing = date(2009, Month(1), 1, 20, 0, 0, 0, &beijing);

        // Although the UTC clock time is 1200 and the Beijing clock time is 2000, Beijing is
        // 8 hours ahead so the two dates actually represent the same instant.
        let times_are_equal = time_in_utc.equal(&same_time_in_beijing);
        let out = format!("{times_are_equal}\n");

        check_output(&out, &["// Output:", "// true"]);
    }

    #[test]
    fn example_time_add() {
        go_setup();
        let mut out = String::new();
        let start = date(2009, Month(1), 1, 12, 0, 0, 0, &utc());
        let after_ten_seconds = start.add(Duration(SECOND.0 * 10));
        let after_ten_minutes = start.add(Duration(MINUTE.0 * 10));
        let after_ten_hours = start.add(Duration(HOUR.0 * 10));
        let after_ten_days = start.add(Duration(HOUR.0 * 24 * 10));

        out += &format!("start = {start}\n");
        out += &format!("start.Add(time.Second * 10) = {after_ten_seconds}\n");
        out += &format!("start.Add(time.Minute * 10) = {after_ten_minutes}\n");
        out += &format!("start.Add(time.Hour * 10) = {after_ten_hours}\n");
        out += &format!("start.Add(time.Hour * 24 * 10) = {after_ten_days}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// start = 2009-01-01 12:00:00 +0000 UTC",
                "// start.Add(time.Second * 10) = 2009-01-01 12:00:10 +0000 UTC",
                "// start.Add(time.Minute * 10) = 2009-01-01 12:10:00 +0000 UTC",
                "// start.Add(time.Hour * 10) = 2009-01-01 22:00:00 +0000 UTC",
                "// start.Add(time.Hour * 24 * 10) = 2009-01-11 12:00:00 +0000 UTC",
            ],
        );
    }

    #[test]
    fn example_time_add_date() {
        go_setup();
        let mut out = String::new();
        let start = date(2023, Month(3), 25, 12, 0, 0, 0, &utc());
        let one_day_later = start.add_date(0, 0, 1);
        let day_duration = one_day_later.sub(&start);
        let one_month_later = start.add_date(0, 1, 0);
        let one_year_later = start.add_date(1, 0, 0);

        let zurich = load_location("Europe/Zurich").unwrap_or_else(|e| panic!("{e}"));
        // This was the day before a daylight saving time transition in Zürich.
        let start_zurich = date(2023, Month(3), 25, 12, 0, 0, 0, &zurich);
        let one_day_later_zurich = start_zurich.add_date(0, 0, 1);
        let day_duration_zurich = one_day_later_zurich.sub(&start_zurich);

        out += &format!("oneDayLater: start.AddDate(0, 0, 1) = {one_day_later}\n");
        out += &format!("oneMonthLater: start.AddDate(0, 1, 0) = {one_month_later}\n");
        out += &format!("oneYearLater: start.AddDate(1, 0, 0) = {one_year_later}\n");
        out +=
            &format!("oneDayLaterZurich: startZurich.AddDate(0, 0, 1) = {one_day_later_zurich}\n");
        out += &format!(
            "Day duration in UTC: {day_duration} | Day duration in Zürich: {day_duration_zurich}\n"
        );

        check_output(
            &out,
            &[
                "// Output:",
                "// oneDayLater: start.AddDate(0, 0, 1) = 2023-03-26 12:00:00 +0000 UTC",
                "// oneMonthLater: start.AddDate(0, 1, 0) = 2023-04-25 12:00:00 +0000 UTC",
                "// oneYearLater: start.AddDate(1, 0, 0) = 2024-03-25 12:00:00 +0000 UTC",
                "// oneDayLaterZurich: startZurich.AddDate(0, 0, 1) = 2023-03-26 12:00:00 +0200 CEST",
                "// Day duration in UTC: 24h0m0s | Day duration in Zürich: 23h0m0s",
            ],
        );
    }

    #[test]
    fn example_time_after() {
        go_setup();
        let mut out = String::new();
        let year2000 = date(2000, Month(1), 1, 0, 0, 0, 0, &utc());
        let year3000 = date(3000, Month(1), 1, 0, 0, 0, 0, &utc());

        let is_year3000_after_year2000 = year3000.after(&year2000); // True
        let is_year2000_after_year3000 = year2000.after(&year3000); // False

        out += &format!("year3000.After(year2000) = {is_year3000_after_year2000}\n");
        out += &format!("year2000.After(year3000) = {is_year2000_after_year3000}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// year3000.After(year2000) = true",
                "// year2000.After(year3000) = false",
            ],
        );
    }

    #[test]
    fn example_time_before() {
        go_setup();
        let mut out = String::new();
        let year2000 = date(2000, Month(1), 1, 0, 0, 0, 0, &utc());
        let year3000 = date(3000, Month(1), 1, 0, 0, 0, 0, &utc());

        let is_year2000_before_year3000 = year2000.before(&year3000); // True
        let is_year3000_before_year2000 = year3000.before(&year2000); // False

        out += &format!("year2000.Before(year3000) = {is_year2000_before_year3000}\n");
        out += &format!("year3000.Before(year2000) = {is_year3000_before_year2000}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// year2000.Before(year3000) = true",
                "// year3000.Before(year2000) = false",
            ],
        );
    }

    #[test]
    fn example_time_date() {
        go_setup();
        let mut out = String::new();
        let d = date(2000, Month(2), 1, 12, 30, 0, 0, &utc());
        let (year, month, day) = d.date();

        out += &format!("year = {year}\n");
        out += &format!("month = {month}\n");
        out += &format!("day = {day}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// year = 2000",
                "// month = February",
                "// day = 1",
            ],
        );
    }

    #[test]
    fn example_time_day() {
        go_setup();
        let d = date(2000, Month(2), 1, 12, 30, 0, 0, &utc());
        let day = d.day();

        let out = format!("day = {day}\n");

        check_output(&out, &["// Output:", "// day = 1"]);
    }

    #[test]
    fn example_time_equal() {
        go_setup();
        let mut out = String::new();
        let seconds_east_of_utc = Duration(8 * HOUR.0).seconds() as i64;
        let beijing = fixed_zone("Beijing Time", seconds_east_of_utc);

        // Unlike the equal operator, Equal is aware that d1 and d2 are the
        // same instant but in different time zones.
        let d1 = date(2000, Month(2), 1, 12, 30, 0, 0, &utc());
        let d2 = date(2000, Month(2), 1, 20, 30, 0, 0, &beijing);

        let dates_equal_using_equal_operator = d1 == d2;
        let dates_equal_using_function = d1.equal(&d2);

        out += &format!("datesEqualUsingEqualOperator = {dates_equal_using_equal_operator}\n");
        out += &format!("datesEqualUsingFunction = {dates_equal_using_function}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// datesEqualUsingEqualOperator = false",
                "// datesEqualUsingFunction = true",
            ],
        );
    }

    #[test]
    fn example_time_string() {
        go_setup();
        let mut out = String::new();
        let time_with_nanoseconds = date(2000, Month(2), 1, 12, 13, 14, 15, &utc());
        let with_nanoseconds = time_with_nanoseconds.string();

        let time_without_nanoseconds = date(2000, Month(2), 1, 12, 13, 14, 0, &utc());
        let without_nanoseconds = time_without_nanoseconds.string();

        out += &format!("withNanoseconds = {with_nanoseconds}\n");
        out += &format!("withoutNanoseconds = {without_nanoseconds}\n");

        check_output(
            &out,
            &[
                "// Output:",
                "// withNanoseconds = 2000-02-01 12:13:14.000000015 +0000 UTC",
                "// withoutNanoseconds = 2000-02-01 12:13:14 +0000 UTC",
            ],
        );
    }

    #[test]
    fn example_time_sub() {
        go_setup();
        let start = date(2000, Month(1), 1, 0, 0, 0, 0, &utc());
        let end = date(2000, Month(1), 1, 12, 0, 0, 0, &utc());

        let difference = end.sub(&start);
        let out = format!("difference = {difference}\n");

        check_output(&out, &["// Output:", "// difference = 12h0m0s"]);
    }

    #[test]
    fn example_time_append_binary() {
        go_setup();
        let mut out = String::new();
        let t = date(2025, Month(4), 1, 15, 30, 45, 123456789, &utc());

        let mut buffer = Vec::new();
        t.append_binary(&mut buffer)
            .unwrap_or_else(|e| panic!("{e}"));

        let mut parse_time = Time::default();
        parse_time
            .unmarshal_binary(&buffer[..])
            .unwrap_or_else(|e| panic!("{e}"));

        out += &format!("t: {t}\n");
        out += &format!("parseTime: {parse_time}\n");
        out += &format!("equal: {}\n", parse_time.equal(&t));

        check_output(
            &out,
            &[
                "// Output:",
                "// t: 2025-04-01 15:30:45.123456789 +0000 UTC",
                "// parseTime: 2025-04-01 15:30:45.123456789 +0000 UTC",
                "// equal: true",
            ],
        );
    }

    #[test]
    fn example_time_append_format() {
        go_setup();
        let t = date(2017, Month::NOVEMBER, 4, 11, 0, 0, 0, &utc());
        let mut text = b"Time: ".to_vec();

        t.append_format(&mut text, KITCHEN.as_bytes());
        let out = format!("{}\n", String::from_utf8(text).expect("UTF-8"));

        check_output(&out, &["// Output:", "// Time: 11:00AM"]);
    }

    #[test]
    fn example_time_append_text() {
        go_setup();
        let t = date(2025, Month(4), 1, 15, 30, 45, 123456789, &utc());

        let mut buffer = b"t: ".to_vec();

        t.append_text(&mut buffer).unwrap_or_else(|e| panic!("{e}"));

        let out = format!("{}\n", String::from_utf8(buffer).expect("UTF-8"));

        check_output(
            &out,
            &["// Output:", "// t: 2025-04-01T15:30:45.123456789Z"],
        );
    }

    #[test]
    fn example_fixed_zone() {
        go_setup();
        let loc = fixed_zone("UTC-8", -8 * 60 * 60);
        let t = date(2009, Month::NOVEMBER, 10, 23, 0, 0, 0, &loc);
        let out = format!("The time is: {}\n", t.format(RFC822));
        check_output(&out, &["// Output: The time is: 10 Nov 09 23:00 UTC-8"]);
    }

    /// The comparison itself: Go's rules, checked on the shapes the examples
    /// above use, so a lax helper cannot hide a wrong output.
    #[test]
    fn check_output_follows_go() {
        assert_eq!(example_output(&["// Output: a"]), "a\n");
        assert_eq!(example_output(&["// Output:", "// a", "// b  "]), "a\nb\n");
        assert_eq!(
            example_output(&["// Output:", "// a", "//", "//", "// b"]),
            "a\n\nb\n"
        );
        // Inner spaces and leading spaces of a line are kept.
        assert_eq!(example_output(&["// Output:", "//  a  b"]), " a  b\n");
        let caught = std::panic::catch_unwind(|| check_output("a\n", &["// Output: b"]));
        assert!(caught.is_err(), "different output must fail");
        let caught = std::panic::catch_unwind(|| check_output("a b\n", &["// Output: a  b"]));
        assert!(caught.is_err(), "inner spaces must match");
        check_output("\n a\n\n", &["// Output: a"]);
        // `gotime` is used above only through the example tests.
        let _ = gotime::utc();
    }
}
