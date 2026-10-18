// Ports: $GOROOT/src/time/zoneinfo_test.go, zoneinfo_unix_test.go and
// abs_test.go @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's time package tests: Copyright 2014 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's zone loading, TZ-string and absolute-calendar tests, run against
//! `gotime`.
//!
//! Tests that set environment variables in Go (`t.Setenv`, `os.Setenv`) re-run
//! themselves in a child process with the variables set; the child does the
//! part of the Go test after the variable is set. The `tests/data/go-testdata`
//! files are Go's `time/testdata`.

mod common;

mod gotime {
    use gorse_common::gotime::internal::{
        self, ABSOLUTE_YEARS, MARCH_THRU_DECEMBER, Rule, RuleKind, abs_days_date, abs_days_split,
        abs_days_year_yday, abs_yday_split, date_to_abs_days, days_before, days_in,
    };
    use gorse_common::gotime::{
        self, LocationError, Month, Time, load_location, load_location_from_tz_data,
    };

    use crate::common::{data_dir, go_setup, goroot, is_child, run_in_child};

    /// Go's `init` check in zoneinfo_test.go: nothing reads $ZONEINFO before
    /// the first LoadLocation. Run in a fresh process.
    #[test]
    fn test_zoneinfo_init() {
        go_setup();
        if !is_child() {
            run_in_child("gotime::test_zoneinfo_init", &[]);
            return;
        }
        assert!(
            internal::zoneinfo_for_testing().is_none(),
            "zoneinfo initialized before first LoadLocation"
        );
    }

    #[test]
    fn test_env_var_usage() {
        go_setup();
        const TEST_ZONEINFO: &str = "foo.zip";
        const ENV: &str = "ZONEINFO";
        if !is_child() {
            run_in_child("gotime::test_env_var_usage", &[(ENV, Some(TEST_ZONEINFO))]);
            return;
        }
        internal::reset_zoneinfo_for_testing();

        // Result isn't important, we're testing the side effect of this
        // command.
        let _ = load_location("Asia/Jerusalem");

        let zoneinfo = internal::zoneinfo_for_testing();
        internal::reset_zoneinfo_for_testing();
        assert_eq!(
            zoneinfo.as_deref(),
            Some(TEST_ZONEINFO),
            "zoneinfo does not match env variable"
        );
    }

    #[test]
    fn test_bad_location_err_msg() {
        go_setup();
        internal::reset_zoneinfo_for_testing();
        let loc = "Asia/SomethingNotExist";
        let want = format!("unknown time zone {loc}");
        let err = load_location(loc).expect_err("LoadLocation succeeded");
        assert_eq!(err.to_string(), want, "LoadLocation({loc:?}) error");
    }

    #[test]
    fn test_load_location_validates_names() {
        go_setup();
        const ENV: &str = "ZONEINFO";
        if !is_child() {
            run_in_child(
                "gotime::test_load_location_validates_names",
                &[(ENV, Some(""))],
            );
            return;
        }
        internal::reset_zoneinfo_for_testing();

        let bad = ["/usr/foo/Foo", "\\UNC\x0coo", "..", "a.."];
        let mut failures = Vec::new();
        for v in bad {
            let err = load_location(v).err();
            if err != Some(LocationError::InvalidName) {
                failures.push(format!(
                    "LoadLocation({v:?}) error = {err:?}; want ErrLocation"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_version3() {
        go_setup();
        let _undo = internal::disable_platform_sources();
        load_location("Asia/Jerusalem").expect("LoadLocation(Asia/Jerusalem)");
    }

    /// Correct results for times before the first transition time, checked
    /// with early dates in a couple of specific timezones.
    #[test]
    fn test_first_zone() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        const FORMAT: &str = "Mon, 02 Jan 2006 15:04:05 -0700 (MST)";
        let tests = [
            (
                "PST8PDT",
                -1633269601,
                "Sun, 31 Mar 1918 01:59:59 -0800 (PST)",
                "Sun, 31 Mar 1918 03:00:00 -0700 (PDT)",
            ),
            (
                "Pacific/Fakaofo",
                1325242799,
                "Thu, 29 Dec 2011 23:59:59 -1100 (-11)",
                "Sat, 31 Dec 2011 00:00:00 +1300 (+13)",
            ),
        ];

        let mut failures = Vec::new();
        for (zone, unix, want1, want2) in tests {
            let z = load_location(zone).expect("LoadLocation");
            let s = gotime::unix(unix, 0).in_location(&z).format(FORMAT);
            if s != want1 {
                failures.push(format!("for {zone} {unix} got {s:?} want {want1:?}"));
            }
            let s = gotime::unix(unix + 1, 0).in_location(&z).format(FORMAT);
            if s != want2 {
                failures.push(format!("for {zone} {unix} got {s:?} want {want2:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_location_names() {
        go_setup();
        assert_eq!(
            gotime::local().name(),
            "Local",
            "invalid Local location name"
        );
        assert_eq!(gotime::utc().name(), "UTC", "invalid UTC location name");
    }

    #[test]
    fn test_load_location_from_tz_data() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        const LOCATION_NAME: &str = "Asia/Jerusalem";
        let reference = load_location(LOCATION_NAME).expect("LoadLocation");

        // testenv.GOROOT(t) is the test's GOROOT: the stand-in one here.
        let goroot_source = internal::goroot_zone_source(&goroot())
            .expect("Failed to locate tzinfo source in GOROOT.");
        let tzinfo = internal::load_tzinfo(LOCATION_NAME, &goroot_source).expect("LoadTzinfo");
        let sample =
            load_location_from_tz_data(LOCATION_NAME, &tzinfo).expect("LoadLocationFromTZData");

        assert!(
            reference.deep_equal(&sample),
            "return values of LoadLocationFromTZData and LoadLocation don't match"
        );
    }

    /// Issue 30099.
    #[test]
    fn test_early_location() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        const LOC_NAME: &str = "America/New_York";
        let loc = load_location(LOC_NAME).expect("LoadLocation");

        let d = gotime::date(1900, Month::JANUARY, 1, 0, 0, 0, 0, &loc);
        let (tz_name, tz_offset) = d.zone();
        assert_eq!(tz_name, "EST", "Zone name");
        assert_eq!(tz_offset, -18000, "Zone offset");
    }

    #[test]
    fn test_malformed_tz_data() {
        go_setup();
        // The goal here is just that malformed tzdata results in an error,
        // not a panic.
        let issue29437 = b"TZif\x00000000000000000\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x0000";
        assert!(
            load_location_from_tz_data("abc", issue29437).is_err(),
            "expected error, got none"
        );
    }

    struct SlimTest {
        zone_name: &'static str,
        file_name: &'static str,
        date: fn(&gotime::Location) -> Time,
        want_name: &'static str,
        want_offset: i64,
    }

    const SLIM_TESTS: &[SlimTest] = &[
        SlimTest {
            // 2020b slim tzdata for Europe/Berlin.
            zone_name: "Europe/Berlin",
            file_name: "2020b_Europe_Berlin",
            date: |loc| gotime::date(2020, Month::OCTOBER, 29, 15, 30, 0, 0, loc),
            want_name: "CET",
            want_offset: 3600,
        },
        SlimTest {
            // 2021a slim tzdata for America/Nuuk.
            zone_name: "America/Nuuk",
            file_name: "2021a_America_Nuuk",
            date: |loc| gotime::date(2020, Month::OCTOBER, 29, 15, 30, 0, 0, loc),
            want_name: "-03",
            want_offset: -10800,
        },
        SlimTest {
            // 2021a slim tzdata for Asia/Gaza.
            zone_name: "Asia/Gaza",
            file_name: "2021a_Asia_Gaza",
            date: |loc| gotime::date(2020, Month::OCTOBER, 29, 15, 30, 0, 0, loc),
            want_name: "EET",
            want_offset: 7200,
        },
        SlimTest {
            // 2021a slim tzdata for Europe/Dublin.
            zone_name: "Europe/Dublin",
            file_name: "2021a_Europe_Dublin",
            date: |loc| gotime::date(2021, Month::APRIL, 2, 11, 12, 13, 0, loc),
            want_name: "IST",
            want_offset: 3600,
        },
    ];

    #[test]
    fn test_load_location_from_tz_data_slim() {
        go_setup();
        let mut failures = Vec::new();
        for test in SLIM_TESTS {
            let path = format!("{}/go-testdata/{}", data_dir(), test.file_name);
            let tz_data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(e) => {
                    failures.push(format!("{path}: {e}"));
                    continue;
                }
            };
            let reference = match load_location_from_tz_data(test.zone_name, &tz_data) {
                Ok(l) => l,
                Err(e) => {
                    failures.push(e.to_string());
                    continue;
                }
            };

            let d = (test.date)(&reference);
            let (tz_name, tz_offset) = d.zone();
            if tz_name != test.want_name {
                failures.push(format!("Zone name == {tz_name}, want {}", test.want_name));
            }
            if tz_offset != test.want_offset {
                failures.push(format!(
                    "Zone offset == {tz_offset}, want {}",
                    test.want_offset
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_tzset() {
        go_setup();
        #[allow(clippy::type_complexity, reason = "the columns of Go's table")]
        let tests: &[(&str, i64, i64, &str, i64, i64, i64, bool, bool)] = &[
            ("", 0, 0, "", 0, 0, 0, false, false),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2159200800,
                "PDT",
                -7 * 60 * 60,
                2152173600,
                2172733200,
                true,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2152173599,
                "PST",
                -8 * 60 * 60,
                2145916800,
                2152173600,
                false,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2152173600,
                "PDT",
                -7 * 60 * 60,
                2152173600,
                2172733200,
                true,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2152173601,
                "PDT",
                -7 * 60 * 60,
                2152173600,
                2172733200,
                true,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2172733199,
                "PDT",
                -7 * 60 * 60,
                2152173600,
                2172733200,
                true,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2172733200,
                "PST",
                -8 * 60 * 60,
                2172733200,
                2177452800,
                false,
                true,
            ),
            (
                "PST8PDT,M3.2.0,M11.1.0",
                0,
                2172733201,
                "PST",
                -8 * 60 * 60,
                2172733200,
                2177452800,
                false,
                true,
            ),
            (
                "KST-9",
                592333200,
                1677246697,
                "KST",
                9 * 60 * 60,
                592333200,
                i64::MAX,
                false,
                true,
            ),
        ];
        let mut failures = Vec::new();
        for &(in_str, in_end, in_sec, name, off, start, end, is_dst, ok) in tests {
            let got = internal::tzset(in_str, in_end, in_sec);
            let want = (name, off, start, end, is_dst, ok);
            if got != want {
                failures.push(format!(
                    "tzset({in_str:?}, {in_end}, {in_sec}) = {got:?}, want {want:?}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_tzset_name() {
        go_setup();
        let tests = [
            ("", "", "", false),
            ("X", "", "", false),
            ("PST", "PST", "", true),
            ("PST8PDT", "PST", "8PDT", true),
            ("PST-08", "PST", "-08", true),
            ("<A+B>+08", "A+B", "+08", true),
        ];
        let mut failures = Vec::new();
        for (input, name, out, ok) in tests {
            let got = internal::tzset_name(input);
            if got != (name, out, ok) {
                failures.push(format!(
                    "tzsetName({input:?}) = {got:?}, want {:?}",
                    (name, out, ok)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_tzset_offset() {
        go_setup();
        let tests = [
            ("", 0, "", false),
            ("X", 0, "", false),
            ("+", 0, "", false),
            ("+08", 8 * 60 * 60, "", true),
            ("-01:02:03", -60 * 60 - 2 * 60 - 3, "", true),
            ("01", 60 * 60, "", true),
            ("100", 100 * 60 * 60, "", true),
            ("1000", 0, "", false),
            ("8PDT", 8 * 60 * 60, "PDT", true),
        ];
        let mut failures = Vec::new();
        for (input, off, out, ok) in tests {
            let got = internal::tzset_offset(input);
            if got != (off, out, ok) {
                failures.push(format!(
                    "tzsetName({input:?}) = {got:?}, want {:?}",
                    (off, out, ok)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn test_tzset_rule() {
        go_setup();
        let rule = |kind, day, week, mon, time| Rule {
            kind,
            day,
            week,
            mon,
            time,
        };
        let tests = [
            ("", Rule::default(), "", false),
            ("X", Rule::default(), "", false),
            (
                "J10",
                rule(RuleKind::Julian, 10, 0, 0, 2 * 60 * 60),
                "",
                true,
            ),
            ("20", rule(RuleKind::Doy, 20, 0, 0, 2 * 60 * 60), "", true),
            (
                "M1.2.3",
                rule(RuleKind::MonthWeekDay, 3, 2, 1, 2 * 60 * 60),
                "",
                true,
            ),
            (
                "30/03:00:00",
                rule(RuleKind::Doy, 30, 0, 0, 3 * 60 * 60),
                "",
                true,
            ),
            (
                "M4.5.6/03:00:00",
                rule(RuleKind::MonthWeekDay, 6, 5, 4, 3 * 60 * 60),
                "",
                true,
            ),
            ("M4.5.7/03:00:00", Rule::default(), "", false),
            (
                "M4.5.6/-04",
                rule(RuleKind::MonthWeekDay, 6, 5, 4, -4 * 60 * 60),
                "",
                true,
            ),
        ];
        let mut failures = Vec::new();
        for (input, r, out, ok) in tests {
            let got = internal::tzset_rule(input);
            if got != (r, out, ok) {
                failures.push(format!(
                    "tzsetName({input:?}) = {got:?}, want {:?}",
                    (r, out, ok)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    // zoneinfo_unix_test.go

    const TZ_WANT_ENV: &str = "GORSE_GOTIME_WANT_LOCAL";
    const TZ_SHANGHAI_ENV: &str = "GORSE_GOTIME_CHECK_SHANGHAI";

    /// Go's TestEnvTZUsage. Go sets $TZ and resets `Local` once per case;
    /// here each case is a child process started with that $TZ, which checks
    /// the name `Local` gets. The children read the system zone files, as Go's
    /// test does, with the stand-in GOROOT as the fallback Go has.
    #[test]
    fn test_env_tz_usage() {
        const ENV: &str = "TZ";
        if is_child() {
            internal::set_default_goroot(&goroot());
            internal::reset_local();
            let want = std::env::var(TZ_WANT_ENV).expect("want");
            let tz = std::env::var_os(ENV);
            assert_eq!(
                gotime::local().name(),
                want,
                "invalid Local location name for {tz:?}"
            );
            if std::env::var_os(TZ_SHANGHAI_ENV).is_some() {
                let time_in_utc =
                    gotime::date(2009, Month::JANUARY, 1, 12, 0, 0, 0, &gotime::utc());
                let same_time_in_shanghai =
                    gotime::date(2009, Month::JANUARY, 1, 20, 0, 0, 0, &gotime::local());
                assert!(
                    time_in_utc.equal(&same_time_in_shanghai),
                    "invalid timezone: got {time_in_utc} want {same_time_in_shanghai}"
                );
            }
            return;
        }
        go_setup();

        let not_exist = |path: &str| matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        let child = |tz: Option<&str>, want: &str, shanghai: bool| {
            let mut env = vec![(ENV, tz), (TZ_WANT_ENV, Some(want))];
            if shanghai {
                env.push((TZ_SHANGHAI_ENV, Some("1")));
            }
            run_in_child("gotime::test_env_tz_usage", &env);
        };

        let mut local_zone_name = "Local";
        // The file may not exist.
        if not_exist("/etc/localtime") {
            local_zone_name = "UTC";
        }

        let cases = [
            // no $TZ means use the system default /etc/localtime.
            (None, local_zone_name),
            // $TZ="" means use UTC.
            (Some(""), "UTC"),
            (Some(":"), "UTC"),
            (Some("Asia/Shanghai"), "Asia/Shanghai"),
            (Some(":Asia/Shanghai"), "Asia/Shanghai"),
            (Some("/etc/localtime"), local_zone_name),
            (Some(":/etc/localtime"), local_zone_name),
        ];
        for (tz, local) in cases {
            child(tz, local, false);
        }

        // The file may not exist on Solaris 2 and IRIX 6.
        let path = "/usr/share/zoneinfo/Asia/Shanghai";
        if not_exist(path) {
            // invalid path should fallback to UTC
            child(Some(path), "UTC", false);
            return;
        }
        // custom path should lead to path itself, in the right zone
        child(Some(path), path, true);

        child(Some(&format!(":{path}")), path, false);

        // invalid path should fallback to UTC
        child(Some(&path[..path.len() - 1]), "UTC", false);
    }

    // abs_test.go (Go runs these through TestInternal)

    /// Fails with the collected errors, as Go's t.Errorf plus "too many
    /// errors" after 20.
    fn check(errors: &[String]) {
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_abs_days_split() {
        go_setup();
        let is_leap = |year: u64| {
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
        };
        let mut errors = Vec::new();
        let mut want_year: u64 = 0;
        let mut want_yday: i64 = 0;
        for days in 0..1_000_000u64 {
            let (century, cyear, yday) = abs_days_split(days);
            if century != want_year / 100 || cyear != (want_year % 100) as i64 || yday != want_yday
            {
                errors.push(format!(
                    "absDays({days}).split() = {century}, {cyear}, {yday}, want {}, {}, {want_yday}",
                    want_year / 100,
                    want_year % 100
                ));
                if errors.len() >= 20 {
                    errors.push("too many errors".into());
                    check(&errors);
                }
            }
            let mut end = 365;
            if is_leap(want_year + 1) {
                end = 366;
            }
            want_yday += 1;
            if want_yday == end {
                want_year += 1;
                want_yday = 0;
            }
        }
        check(&errors);
    }

    #[test]
    fn test_abs_yday_split() {
        go_setup();
        let ends = [31, 30, 31, 30, 31, 31, 30, 31, 30, 31, 31, 29];
        let mut errors = Vec::new();
        let mut want_month: i64 = 3;
        let mut want_day: i64 = 1;
        for yday in 0..366i64 {
            let (month, day) = abs_yday_split(yday);
            if month != want_month || day != want_day {
                errors.push(format!(
                    "absYday({yday}).split() = {month}, {day}, want {want_month}, {want_day}"
                ));
                if errors.len() >= 20 {
                    errors.push("too many errors".into());
                    check(&errors);
                }
            }
            want_day += 1;
            if want_day > ends[(want_month - 3) as usize] {
                want_month += 1;
                want_day = 1;
            }
        }
        check(&errors);
    }

    #[test]
    fn test_abs_date() {
        go_setup();
        let ends = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let is_leap = |year: i64| {
            let y = (year as u64).wrapping_add(ABSOLUTE_YEARS);
            y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
        };
        let mut want_year: i64 = 0;
        let mut want_month = Month::MARCH;
        let mut want_mday: i64 = 1;
        let mut want_yday: i64 = 31 + 29 + 1;
        let mut errors = Vec::new();
        let absolute_years = ABSOLUTE_YEARS as i64;
        for days in 0..1_000_000u64 {
            let (mut year, month, mday) = abs_days_date(days);
            year += absolute_years;
            if year != want_year || month != want_month || mday != want_mday {
                errors.push(format!(
                    "days({days}).date() = {year}, {month}, {mday}, want {want_year}, {want_month}, {want_mday}"
                ));
                if errors.len() >= 20 {
                    errors.push("too many errors".into());
                    check(&errors);
                }
            }

            let yday;
            (year, yday) = abs_days_year_yday(days);
            year += absolute_years;
            if year != want_year || yday != want_yday {
                errors.push(format!(
                    "days({days}).yearYday() = {year}, {yday}, want {want_year}, {want_yday}, "
                ));
                if errors.len() >= 20 {
                    errors.push("too many errors".into());
                    check(&errors);
                }
            }

            want_mday += 1;
            if want_mday == ends[(want_month.0 - 1) as usize] + 1
                || want_month == Month::FEBRUARY && want_mday == 29 && !is_leap(year)
            {
                want_month = Month(want_month.0 + 1);
                want_mday = 1;
            }
            want_yday += 1;
            if want_month == Month(Month::DECEMBER.0 + 1) {
                want_year += 1;
                want_month = Month::JANUARY;
                want_mday = 1;
                want_yday = 1;
            }
        }
        check(&errors);
    }

    #[test]
    fn test_date_to_abs_days() {
        go_setup();
        let is_leap = |year: i64| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let mut want_days = MARCH_THRU_DECEMBER as u64;
        let mut errors = Vec::new();
        for year in 1..10000i64 {
            let days = date_to_abs_days(year - ABSOLUTE_YEARS as i64, Month::JANUARY, 1);
            if days != want_days {
                errors.push(format!(
                    "dateToAbsDays(abs {year}, Jan, 1) = {days}, want {want_days}"
                ));
                if errors.len() >= 20 {
                    errors.push("too many errors".into());
                    check(&errors);
                }
            }
            want_days += 365;
            if is_leap(year) {
                want_days += 1;
            }
        }
        check(&errors);
    }

    #[test]
    fn test_days_in() {
        go_setup();
        let is_leap = |year: i64| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let want = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut errors = Vec::new();
        for year in 0..=1600i64 {
            for m in Month::JANUARY.0..=Month::DECEMBER.0 {
                let mut w = want[m as usize];
                if m == Month::FEBRUARY.0 && is_leap(year) {
                    w += 1;
                }
                let d = days_in(Month(m), year - 800);
                if d != w {
                    errors.push(format!(
                        "daysIn({}, {}) = {d}, want {w}",
                        Month(m),
                        year - 800
                    ));
                    if errors.len() >= 20 {
                        errors.push("too many errors".into());
                        check(&errors);
                    }
                }
            }
        }
        check(&errors);
    }

    #[test]
    fn test_days_before() {
        go_setup();
        let mut errors = Vec::new();
        for (m, want) in [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365]
            .into_iter()
            .enumerate()
        {
            let d = days_before(Month(m as i64 + 1));
            if d != want {
                errors.push(format!("daysBefore({m}) = {d}, want {want}"));
            }
        }
        check(&errors);
    }
}
