//! Ports of Go's `time_test.go` and `mono_test.go` ($GOROOT/src/time @
//! go1.27.0, the toolchain add21b5 builds with), with the same cases and
//! assertions.
//!
//! Go runs these with `time.Local` set to America/Los_Angeles
//! (`ForceUSPacificForTesting`); `common::go_setup` does the same. Where Go
//! uses `testing/quick` (random inputs seeded from the clock) these draw the
//! same kinds of values from a fixed-seed generator, so a failure reproduces.
//!
//! Not ported, as they test what gotime does not have: timers and tickers
//! (`TestConcurrentTimerReset`, `TestConcurrentTimerResetStop`, the
//! `<-After`/`<-Tick` lines of `TestHasMonotonicClock`) and allocation counts
//! (`TestCountMallocs`, `TestUnmarshalTextAllocations`). `TestInternal` runs
//! the `abs_test.go` cases, which are ported with that file. The gob tests
//! check the bytes `GobEncode` carries, which are `MarshalBinary`'s.

mod common;

mod gotime {
    use gorse_common::gotime::{
        self, Duration, HOUR, MICROSECOND, MILLISECOND, MINUTE, Month, NANOSECOND, RFC3339,
        RFC3339_NANO, SECOND, Time, UNIX_DATE, Weekday, date, fixed_zone, internal, load_location,
        local, now, parse, parse_duration, since, unix, unix_micro, unix_milli, until, utc,
    };

    use crate::common::{go_setup, is_child, run_in_child};

    /// splitmix64, standing in for `testing/quick`'s generator: full-range
    /// values, as quick's `randInt64` gives for int64 and int32 alike.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed)
        }

        fn u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn i64(&mut self) -> i64 {
            self.u64() as i64
        }

        fn i32(&mut self) -> i32 {
            self.u64() as i32
        }

        /// Go's `rand.Int31`: a non-negative int32.
        fn int31(&mut self) -> i32 {
            (self.u64() >> 33) as i32
        }
    }

    fn d(n: i64) -> Duration {
        Duration(n)
    }

    #[test]
    fn test_zero_time() {
        go_setup();
        let zero = Time::default();
        let (year, month, day) = zero.date();
        let (hour, min, sec) = zero.clock();
        let nsec = zero.nanosecond();
        let yday = zero.year_day();
        let wday = zero.weekday();
        assert!(
            year == 1
                && month == Month::JANUARY
                && day == 1
                && hour == 0
                && min == 0
                && sec == 0
                && nsec == 0
                && yday == 1
                && wday == Weekday::MONDAY,
            "zero time = {wday} {month} {day} year {year} {hour:02}:{min:02}:{sec:02}.{nsec:09} yday {yday} want Monday Jan 1 year 1 00:00:00.000000000 yday 1"
        );
    }

    // We should be in PST/PDT. The purpose of this test is to at least explain
    // why some of the subsequent tests fail.
    #[test]
    fn test_zone_data() {
        go_setup();
        let lt = now();
        // PST is 8 hours west, PDT is 7 hours west.
        let (name, off) = lt.zone();
        assert!(
            off == -8 * 60 * 60 || off == -7 * 60 * 60,
            "Unable to find US Pacific time zone data for testing; time zone is {name:?} offset {off}"
        );
    }

    /// A parsed time value.
    struct ParsedTime {
        year: i64,
        month: Month,
        day: i64,
        hour: i64,
        minute: i64,
        second: i64,
        nanosecond: i64,
        weekday: Weekday,
        zone_offset: i64,
        zone: &'static str,
    }

    struct TimeTest {
        seconds: i64,
        golden: ParsedTime,
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors Go's parsedTime literal")]
    const fn pt(
        year: i64,
        month: Month,
        day: i64,
        hour: i64,
        minute: i64,
        second: i64,
        nanosecond: i64,
        weekday: Weekday,
        zone_offset: i64,
        zone: &'static str,
    ) -> ParsedTime {
        ParsedTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond,
            weekday,
            zone_offset,
            zone,
        }
    }

    const fn tt(seconds: i64, golden: ParsedTime) -> TimeTest {
        TimeTest { seconds, golden }
    }

    use gorse_common::gotime::Month as M;
    use gorse_common::gotime::Weekday as W;

    const UTC_TESTS: &[TimeTest] = &[
        tt(
            0,
            pt(1970, M::JANUARY, 1, 0, 0, 0, 0, W::THURSDAY, 0, "UTC"),
        ),
        tt(
            1221681866,
            pt(2008, M::SEPTEMBER, 17, 20, 4, 26, 0, W::WEDNESDAY, 0, "UTC"),
        ),
        tt(
            -1221681866,
            pt(1931, M::APRIL, 16, 3, 55, 34, 0, W::THURSDAY, 0, "UTC"),
        ),
        tt(
            -11644473600,
            pt(1601, M::JANUARY, 1, 0, 0, 0, 0, W::MONDAY, 0, "UTC"),
        ),
        tt(
            599529660,
            pt(1988, M::DECEMBER, 31, 0, 1, 0, 0, W::SATURDAY, 0, "UTC"),
        ),
        tt(
            978220860,
            pt(2000, M::DECEMBER, 31, 0, 1, 0, 0, W::SUNDAY, 0, "UTC"),
        ),
    ];

    const NANO_UTC_TESTS: &[TimeTest] = &[
        tt(
            0,
            pt(
                1970,
                M::JANUARY,
                1,
                0,
                0,
                0,
                100_000_000,
                W::THURSDAY,
                0,
                "UTC",
            ),
        ),
        tt(
            1221681866,
            pt(
                2008,
                M::SEPTEMBER,
                17,
                20,
                4,
                26,
                200_000_000,
                W::WEDNESDAY,
                0,
                "UTC",
            ),
        ),
    ];

    const PST: i64 = -8 * 60 * 60;
    const PDT: i64 = -7 * 60 * 60;

    const LOCAL_TESTS: &[TimeTest] = &[
        tt(
            0,
            pt(1969, M::DECEMBER, 31, 16, 0, 0, 0, W::WEDNESDAY, PST, "PST"),
        ),
        tt(
            1221681866,
            pt(
                2008,
                M::SEPTEMBER,
                17,
                13,
                4,
                26,
                0,
                W::WEDNESDAY,
                PDT,
                "PDT",
            ),
        ),
        tt(
            2159200800,
            pt(2038, M::JUNE, 3, 11, 0, 0, 0, W::THURSDAY, PDT, "PDT"),
        ),
        tt(
            2152173599,
            pt(2038, M::MARCH, 14, 1, 59, 59, 0, W::SUNDAY, PST, "PST"),
        ),
        tt(
            2152173600,
            pt(2038, M::MARCH, 14, 3, 0, 0, 0, W::SUNDAY, PDT, "PDT"),
        ),
        tt(
            2152173601,
            pt(2038, M::MARCH, 14, 3, 0, 1, 0, W::SUNDAY, PDT, "PDT"),
        ),
        tt(
            2172733199,
            pt(2038, M::NOVEMBER, 7, 1, 59, 59, 0, W::SUNDAY, PDT, "PDT"),
        ),
        tt(
            2172733200,
            pt(2038, M::NOVEMBER, 7, 1, 0, 0, 0, W::SUNDAY, PST, "PST"),
        ),
        tt(
            2172733201,
            pt(2038, M::NOVEMBER, 7, 1, 0, 1, 0, W::SUNDAY, PST, "PST"),
        ),
    ];

    const NANO_LOCAL_TESTS: &[TimeTest] = &[
        tt(
            0,
            pt(
                1969,
                M::DECEMBER,
                31,
                16,
                0,
                0,
                100_000_000,
                W::WEDNESDAY,
                PST,
                "PST",
            ),
        ),
        tt(
            1221681866,
            pt(
                2008,
                M::SEPTEMBER,
                17,
                13,
                4,
                26,
                300_000_000,
                W::WEDNESDAY,
                PDT,
                "PDT",
            ),
        ),
    ];

    fn same(t: &Time, u: &ParsedTime) -> bool {
        // Check aggregates.
        let (year, month, day) = t.date();
        let (hour, min, sec) = t.clock();
        let (name, offset) = t.zone();
        if year != u.year
            || month != u.month
            || day != u.day
            || hour != u.hour
            || min != u.minute
            || sec != u.second
            || name != u.zone
            || offset != u.zone_offset
        {
            return false;
        }
        // Check individual entries.
        t.year() == u.year
            && t.month() == u.month
            && t.day() == u.day
            && t.hour() == u.hour
            && t.minute() == u.minute
            && t.second() == u.second
            && t.nanosecond() == u.nanosecond
            && t.weekday() == u.weekday
    }

    fn mst(t: &Time) -> String {
        t.format(&format!("{RFC3339} MST"))
    }

    #[test]
    fn test_unix_utc() {
        go_setup();
        for test in UTC_TESTS {
            let sec = test.seconds;
            let tm = unix(sec, 0).utc();
            let newsec = tm.unix();
            assert_eq!(newsec, sec, "Unix({sec}, 0).Unix() = {newsec}");
            assert!(
                same(&tm, &test.golden),
                "Unix({sec}, 0): // {}\n  have={}",
                tm.go_string(),
                mst(&tm)
            );
        }
    }

    #[test]
    fn test_unix_nano_utc() {
        go_setup();
        for test in NANO_UTC_TESTS {
            let golden = &test.golden;
            let nsec = test.seconds * 1_000_000_000 + golden.nanosecond;
            let tm = unix(0, nsec).utc();
            let newnsec = tm.unix() * 1_000_000_000 + tm.nanosecond();
            assert_eq!(newnsec, nsec, "Unix(0, {nsec}).Nanoseconds() = {newnsec}");
            assert!(same(&tm, golden), "Unix(0, {nsec}):\n  have={}", mst(&tm));
        }
    }

    #[test]
    fn test_unix() {
        go_setup();
        for test in LOCAL_TESTS {
            let sec = test.seconds;
            let tm = unix(sec, 0);
            let newsec = tm.unix();
            assert_eq!(newsec, sec, "Unix({sec}, 0).Seconds() = {newsec}");
            assert!(
                same(&tm, &test.golden),
                "Unix({sec}, 0):\n  have={}",
                mst(&tm)
            );
        }
    }

    #[test]
    fn test_unix_nano() {
        go_setup();
        for test in NANO_LOCAL_TESTS {
            let golden = &test.golden;
            let nsec = test.seconds * 1_000_000_000 + golden.nanosecond;
            let tm = unix(0, nsec);
            let newnsec = tm.unix() * 1_000_000_000 + tm.nanosecond();
            assert_eq!(newnsec, nsec, "Unix(0, {nsec}).Seconds() = {newnsec}");
            assert!(same(&tm, golden), "Unix(0, {nsec}):\n  have={}", mst(&tm));
        }
    }

    // quick.Config{MaxCount: 10000}
    const QUICK_COUNT: usize = 10000;

    #[test]
    fn test_unix_utc_and_back() {
        go_setup();
        let f = |sec: i64| unix(sec, 0).utc().unix() == sec;
        let mut rng = Rng::new(1);
        // Try a reasonable date first, then the huge ones.
        for _ in 0..QUICK_COUNT {
            let sec = rng.i32();
            assert!(f(i64::from(sec)), "failed on input {sec}");
        }
        for _ in 0..QUICK_COUNT {
            let sec = rng.i64();
            assert!(f(sec), "failed on input {sec}");
        }
    }

    #[test]
    fn test_unix_nano_utc_and_back() {
        go_setup();
        let f = |nsec: i64| {
            let t = unix(0, nsec).utc();
            let ns = t
                .unix()
                .wrapping_mul(1_000_000_000)
                .wrapping_add(t.nanosecond());
            ns == nsec
        };
        let mut rng = Rng::new(2);
        // Try a small date first, then the large ones. (The span is only a
        // few hundred years for nanoseconds in an int64.)
        for _ in 0..QUICK_COUNT {
            let nsec = rng.i32();
            assert!(f(i64::from(nsec)), "failed on input {nsec}");
        }
        for _ in 0..QUICK_COUNT {
            let nsec = rng.i64();
            assert!(f(nsec), "failed on input {nsec}");
        }
    }

    #[test]
    fn test_unix_milli() {
        go_setup();
        let mut rng = Rng::new(3);
        for _ in 0..QUICK_COUNT {
            let msec = rng.i64();
            let t = unix_milli(msec);
            assert_eq!(t.unix_milli(), msec, "failed on input {msec}");
        }
    }

    #[test]
    fn test_unix_micro() {
        go_setup();
        let mut rng = Rng::new(4);
        for _ in 0..QUICK_COUNT {
            let usec = rng.i64();
            let t = unix_micro(usec);
            assert_eq!(t.unix_micro(), usec, "failed on input {usec}");
        }
    }

    // The time routines provide no way to get absolute time (seconds since
    // zero), but we need it to compute the right answer for bizarre roundings
    // like "to the nearest 3 ns". Compute as t - year1 = (t - 1970) + (1970 -
    // 2001) + (2001 - 1). t - 1970 is returned by Unix and Nanosecond. 1970 -
    // 2001 is -(31*365+8)*86400 = -978307200 seconds. 2001 - 1 is
    // 2000*365.2425*86400 = 63113904000 seconds.
    const UNIX_TO_ZERO: i64 = -978307200 + 63113904000;

    /// The absolute time stored in t, as seconds and nanoseconds.
    fn abs(t: &Time) -> (i64, i64) {
        let unix = t.unix();
        let nano = t.nanosecond();
        (unix.wrapping_add(UNIX_TO_ZERO), nano)
    }

    /// abs as a decimal string.
    fn abs_string(t: &Time) -> String {
        let (mut sec, mut nsec) = abs(t);
        if sec < 0 {
            sec = sec.wrapping_neg();
            nsec = -nsec;
            if nsec < 0 {
                nsec += 1_000_000_000;
                sec -= 1;
            }
            return format!("-{sec}{nsec:09}");
        }
        format!("{sec}{nsec:09}")
    }

    #[test]
    fn test_truncate_round() {
        go_setup();
        let truncate_round_tests = [
            (
                date(-1, M::JANUARY, 1, 12, 15, 30, 500_000_000, &utc()),
                d(3),
            ),
            (
                date(-1, M::JANUARY, 1, 12, 15, 31, 500_000_000, &utc()),
                d(3),
            ),
            (
                date(2012, M::JANUARY, 1, 12, 15, 30, 500_000_000, &utc()),
                SECOND,
            ),
            (
                date(2012, M::JANUARY, 1, 12, 15, 31, 500_000_000, &utc()),
                SECOND,
            ),
            // 5.8*d rounds to 6*d, but .8*d+.8*d < 0 < d
            (unix(-19012425939, 649146258), d(7435029458905025217)),
        ];

        let test_one = |ti: i64, tns: i64, di: i64| -> bool {
            let t0 = unix(ti, tns).utc();
            let mut d = Duration(di);
            if d.0 < 0 {
                d = Duration(d.0.wrapping_neg());
            }
            if d.0 <= 0 {
                d = Duration(1);
            }

            // Compute bt = absolute nanoseconds.
            let (sec, nsec) = abs(&t0);
            let bt = i128::from(sec) * 1_000_000_000 + i128::from(nsec);

            // Compute quotient and remainder mod d (big.Int.DivMod is
            // Euclidean). br is < d, so it fits in an int64.
            let r = bt.rem_euclid(i128::from(d.0)) as i64;

            // To truncate, subtract remainder.
            let mut t1 = t0.add(Duration(r.wrapping_neg()));

            // Check that time.Truncate works.
            let trunc = t0.truncate(d);
            assert!(
                trunc == t1,
                "Time.Truncate({}, {d}) = {trunc}, want {}\n{} trunc {} =\n{} want\n{}",
                t0.format(RFC3339_NANO),
                t1.format(RFC3339_NANO),
                abs_string(&t0),
                d.0,
                abs_string(&trunc),
                abs_string(&t1)
            );

            // To round, add d back if remainder r > d/2 or r == exactly d/2.
            // The commented out code would round half to even instead of up,
            // but that makes it time-zone dependent, which is a bit strange.
            if r > d.0 / 2 || r.wrapping_add(r) == d.0
            /*&& bq.Bit(0) == 1*/
            {
                t1 = t1.add(d);
            }

            // Check that time.Round works.
            let rnd = t0.round(d);
            assert!(
                rnd == t1,
                "Time.Round({}, {d}) = {rnd}, want {}\n{} round {} =\n{} want\n{}",
                t0.format(RFC3339_NANO),
                t1.format(RFC3339_NANO),
                abs_string(&t0),
                d.0,
                abs_string(&rnd),
                abs_string(&t1)
            );
            true
        };

        // manual test cases
        for (t, d) in &truncate_round_tests {
            test_one(t.unix(), t.nanosecond(), d.0);
        }

        // exhaustive near 0
        for i in 0..100 {
            for j in 1..100 {
                test_one(UNIX_TO_ZERO, i, j);
                test_one(UNIX_TO_ZERO, -i, j);
            }
        }

        // randomly generated test cases: quick.Config{MaxCount: 100000}
        const COUNT: usize = 100_000;
        let mut rng = Rng::new(5);

        // divisors of Second
        let f1 = |mut ti: i64, tns: i32, logdi: i32| {
            let mut d = Duration(1);
            let (a, b) = ((logdi % 9) as u32, (logdi >> 16) % 9);
            // A Go shift by 64 or more gives 0.
            d = Duration(if a >= 64 { 0 } else { d.0 << a });
            for _ in 0..b {
                d = Duration(d.0.wrapping_mul(5));
            }

            // Make room for unix ↔ internal conversion. We don't care about
            // behavior too close to ± 2^63 Unix seconds. It is full of
            // wraparounds but will never happen in a reasonable program. (Or
            // maybe not? See go.dev/issue/20678. In any event, they're not
            // handled today.)
            ti >>= 1;

            test_one(ti, i64::from(tns), d.0)
        };
        for _ in 0..COUNT {
            let (ti, tns, logdi) = (rng.i64(), rng.i32(), rng.i32());
            f1(ti, tns, logdi);
        }

        // multiples of Second
        let f2 = |mut ti: i64, tns: i32, di: i32| {
            let mut d = Duration(i64::from(di).wrapping_mul(SECOND.0));
            if d.0 < 0 {
                d = Duration(d.0.wrapping_neg());
            }
            ti >>= 1; // see comment in f1
            test_one(ti, i64::from(tns), d.0)
        };
        for _ in 0..COUNT {
            let (ti, tns, di) = (rng.i64(), rng.i32(), rng.i32());
            f2(ti, tns, di);
        }

        // halfway cases
        let f3 = |mut tns: i64, mut di: i64| {
            di &= 0xfffffffe;
            if di == 0 {
                di = 2;
            }
            tns -= tns % di;
            if tns < 0 {
                tns += di / 2;
            } else {
                tns -= di / 2;
            }
            test_one(0, tns, di)
        };
        for _ in 0..COUNT {
            let (tns, di) = (rng.i64(), rng.i64());
            f3(tns, di);
        }

        // full generality
        let f4 = |mut ti: i64, tns: i32, di: i64| {
            ti >>= 1; // see comment in f1
            test_one(ti, i64::from(tns), di)
        };
        for _ in 0..COUNT {
            let (ti, tns, di) = (rng.i64(), rng.i32(), rng.i64());
            f4(ti, tns, di);
        }
    }

    /// (year, month, day, expected year, expected week)
    const ISO_WEEK_TESTS: &[(i64, i64, i64, i64, i64)] = &[
        (1981, 1, 1, 1981, 1),
        (1982, 1, 1, 1981, 53),
        (1983, 1, 1, 1982, 52),
        (1984, 1, 1, 1983, 52),
        (1985, 1, 1, 1985, 1),
        (1986, 1, 1, 1986, 1),
        (1987, 1, 1, 1987, 1),
        (1988, 1, 1, 1987, 53),
        (1989, 1, 1, 1988, 52),
        (1990, 1, 1, 1990, 1),
        (1991, 1, 1, 1991, 1),
        (1992, 1, 1, 1992, 1),
        (1993, 1, 1, 1992, 53),
        (1994, 1, 1, 1993, 52),
        (1995, 1, 2, 1995, 1),
        (1996, 1, 1, 1996, 1),
        (1996, 1, 7, 1996, 1),
        (1996, 1, 8, 1996, 2),
        (1997, 1, 1, 1997, 1),
        (1998, 1, 1, 1998, 1),
        (1999, 1, 1, 1998, 53),
        (2000, 1, 1, 1999, 52),
        (2001, 1, 1, 2001, 1),
        (2002, 1, 1, 2002, 1),
        (2003, 1, 1, 2003, 1),
        (2004, 1, 1, 2004, 1),
        (2005, 1, 1, 2004, 53),
        (2006, 1, 1, 2005, 52),
        (2007, 1, 1, 2007, 1),
        (2008, 1, 1, 2008, 1),
        (2009, 1, 1, 2009, 1),
        (2010, 1, 1, 2009, 53),
        (2010, 1, 1, 2009, 53),
        (2011, 1, 1, 2010, 52),
        (2011, 1, 2, 2010, 52),
        (2011, 1, 3, 2011, 1),
        (2011, 1, 4, 2011, 1),
        (2011, 1, 5, 2011, 1),
        (2011, 1, 6, 2011, 1),
        (2011, 1, 7, 2011, 1),
        (2011, 1, 8, 2011, 1),
        (2011, 1, 9, 2011, 1),
        (2011, 1, 10, 2011, 2),
        (2011, 1, 11, 2011, 2),
        (2011, 6, 12, 2011, 23),
        (2011, 6, 13, 2011, 24),
        (2011, 12, 25, 2011, 51),
        (2011, 12, 26, 2011, 52),
        (2011, 12, 27, 2011, 52),
        (2011, 12, 28, 2011, 52),
        (2011, 12, 29, 2011, 52),
        (2011, 12, 30, 2011, 52),
        (2011, 12, 31, 2011, 52),
        (1995, 1, 1, 1994, 52),
        (2012, 1, 1, 2011, 52),
        (2012, 1, 2, 2012, 1),
        (2012, 1, 8, 2012, 1),
        (2012, 1, 9, 2012, 2),
        (2012, 12, 23, 2012, 51),
        (2012, 12, 24, 2012, 52),
        (2012, 12, 30, 2012, 52),
        (2012, 12, 31, 2013, 1),
        (2013, 1, 1, 2013, 1),
        (2013, 1, 6, 2013, 1),
        (2013, 1, 7, 2013, 2),
        (2013, 12, 22, 2013, 51),
        (2013, 12, 23, 2013, 52),
        (2013, 12, 29, 2013, 52),
        (2013, 12, 30, 2014, 1),
        (2014, 1, 1, 2014, 1),
        (2014, 1, 5, 2014, 1),
        (2014, 1, 6, 2014, 2),
        (2015, 1, 1, 2015, 1),
        (2016, 1, 1, 2015, 53),
        (2017, 1, 1, 2016, 52),
        (2018, 1, 1, 2018, 1),
        (2019, 1, 1, 2019, 1),
        (2020, 1, 1, 2020, 1),
        (2021, 1, 1, 2020, 53),
        (2022, 1, 1, 2021, 52),
        (2023, 1, 1, 2022, 52),
        (2024, 1, 1, 2024, 1),
        (2025, 1, 1, 2025, 1),
        (2026, 1, 1, 2026, 1),
        (2027, 1, 1, 2026, 53),
        (2028, 1, 1, 2027, 52),
        (2029, 1, 1, 2029, 1),
        (2030, 1, 1, 2030, 1),
        (2031, 1, 1, 2031, 1),
        (2032, 1, 1, 2032, 1),
        (2033, 1, 1, 2032, 53),
        (2034, 1, 1, 2033, 52),
        (2035, 1, 1, 2035, 1),
        (2036, 1, 1, 2036, 1),
        (2037, 1, 1, 2037, 1),
        (2038, 1, 1, 2037, 53),
        (2039, 1, 1, 2038, 52),
        (2040, 1, 1, 2039, 52),
    ];

    #[test]
    fn test_iso_week() {
        go_setup();
        // Selected dates and corner cases
        for &(year, month, day, yex, wex) in ISO_WEEK_TESTS {
            let dt = date(year, Month(month), day, 0, 0, 0, 0, &utc());
            let (y, w) = dt.iso_week();
            assert!(
                w == wex && y == yex,
                "got {y}/{w}; expected {yex}/{wex} for {year}-{month:02}-{day:02}"
            );
        }

        // The only real invariant: Jan 04 is in week 1
        for year in 1950..2100 {
            let (y, w) = date(year, M::JANUARY, 4, 0, 0, 0, 0, &utc()).iso_week();
            assert!(
                y == year && w == 1,
                "got {y}/{w}; expected {year}/1 for Jan 04"
            );
        }
    }

    /// (year, month, day, yday)
    const YEAR_DAY_TESTS: &[(i64, i64, i64, i64)] = &[
        // Non-leap-year tests
        (2007, 1, 1, 1),
        (2007, 1, 15, 15),
        (2007, 2, 1, 32),
        (2007, 2, 15, 46),
        (2007, 3, 1, 60),
        (2007, 3, 15, 74),
        (2007, 4, 1, 91),
        (2007, 12, 31, 365),
        // Leap-year tests
        (2008, 1, 1, 1),
        (2008, 1, 15, 15),
        (2008, 2, 1, 32),
        (2008, 2, 15, 46),
        (2008, 3, 1, 61),
        (2008, 3, 15, 75),
        (2008, 4, 1, 92),
        (2008, 12, 31, 366),
        // Looks like leap-year (but isn't) tests
        (1900, 1, 1, 1),
        (1900, 1, 15, 15),
        (1900, 2, 1, 32),
        (1900, 2, 15, 46),
        (1900, 3, 1, 60),
        (1900, 3, 15, 74),
        (1900, 4, 1, 91),
        (1900, 12, 31, 365),
        // Year one tests (non-leap)
        (1, 1, 1, 1),
        (1, 1, 15, 15),
        (1, 2, 1, 32),
        (1, 2, 15, 46),
        (1, 3, 1, 60),
        (1, 3, 15, 74),
        (1, 4, 1, 91),
        (1, 12, 31, 365),
        // Year minus one tests (non-leap)
        (-1, 1, 1, 1),
        (-1, 1, 15, 15),
        (-1, 2, 1, 32),
        (-1, 2, 15, 46),
        (-1, 3, 1, 60),
        (-1, 3, 15, 74),
        (-1, 4, 1, 91),
        (-1, 12, 31, 365),
        // 400 BC tests (leap-year)
        (-400, 1, 1, 1),
        (-400, 1, 15, 15),
        (-400, 2, 1, 32),
        (-400, 2, 15, 46),
        (-400, 3, 1, 61),
        (-400, 3, 15, 75),
        (-400, 4, 1, 92),
        (-400, 12, 31, 366),
        // Special Cases

        // Gregorian calendar change (no effect)
        (1582, 10, 4, 277),
        (1582, 10, 15, 288),
    ];

    #[test]
    fn test_year_day() {
        go_setup();
        // Check to see if YearDay is location sensitive
        let year_day_locations = [
            fixed_zone("UTC-8", -8 * 60 * 60),
            fixed_zone("UTC-4", -4 * 60 * 60),
            utc(),
            fixed_zone("UTC+4", 4 * 60 * 60),
            fixed_zone("UTC+8", 8 * 60 * 60),
        ];
        for (i, loc) in year_day_locations.iter().enumerate() {
            for &(year, month, day, want) in YEAR_DAY_TESTS {
                let dt = date(year, Month(month), day, 0, 0, 0, 0, loc);
                let yday = dt.year_day();
                assert_eq!(
                    yday, want,
                    "Date({year}-{month:02}-{day:02} in {loc}).YearDay() = {yday}, want {want}"
                );

                if !(0..=9999).contains(&year) {
                    continue;
                }
                // fmt.Sprintf("%04d-%02d-%02d %03d %+.2d00", ...)
                let off = (i as i64 - 2) * 4;
                let sign = if off < 0 { '-' } else { '+' };
                let f = format!(
                    "{year:04}-{month:02}-{day:02} {want:03} {sign}{:02}00",
                    off.abs()
                );
                let dt1 = parse("2006-01-02 002 -0700", &f)
                    .unwrap_or_else(|e| panic!(r#"Parse("2006-01-02 002 -0700", {f:?}): {e}"#));
                assert!(
                    dt1.equal(&dt),
                    r#"Parse("2006-01-02 002 -0700", {f:?}) = {dt1}, want {dt}"#
                );
            }
        }
    }

    fn duration_tests() -> Vec<(&'static str, Duration)> {
        vec![
            ("0s", d(0)),
            ("1ns", NANOSECOND),
            ("1.1µs", d(1100 * NANOSECOND.0)),
            ("2.2ms", d(2200 * MICROSECOND.0)),
            ("3.3s", d(3300 * MILLISECOND.0)),
            ("4m5s", d(4 * MINUTE.0 + 5 * SECOND.0)),
            ("4m5.001s", d(4 * MINUTE.0 + 5001 * MILLISECOND.0)),
            (
                "5h6m7.001s",
                d(5 * HOUR.0 + 6 * MINUTE.0 + 7001 * MILLISECOND.0),
            ),
            ("8m0.000000001s", d(8 * MINUTE.0 + NANOSECOND.0)),
            ("2562047h47m16.854775807s", d(i64::MAX)),
            ("-2562047h47m16.854775808s", d(i64::MIN)),
        ]
    }

    #[test]
    fn test_duration_string() {
        go_setup();
        for (str, d) in duration_tests() {
            assert_eq!(
                d.to_string(),
                str,
                "Duration({}).String() = {d}, want {str}",
                d.0
            );
            if d.0 > 0 {
                let neg = Duration(-d.0);
                assert_eq!(
                    neg.to_string(),
                    format!("-{str}"),
                    "Duration({}).String() = {neg}, want -{str}",
                    neg.0
                );
            }
        }
    }

    /// (year, month, day, hour, min, sec, nsec, unix); all in Local.
    type DateTest = (i64, i64, i64, i64, i64, i64, i64, i64);

    const DATE_TESTS: &[DateTest] = &[
        (2011, 11, 6, 1, 0, 0, 0, 1320566400),   // 1:00:00 PDT
        (2011, 11, 6, 1, 59, 59, 0, 1320569999), // 1:59:59 PDT
        (2011, 11, 6, 2, 0, 0, 0, 1320573600),   // 2:00:00 PST
        (2011, 3, 13, 1, 0, 0, 0, 1300006800),   // 1:00:00 PST
        (2011, 3, 13, 1, 59, 59, 0, 1300010399), // 1:59:59 PST
        (2011, 3, 13, 3, 0, 0, 0, 1300010400),   // 3:00:00 PDT
        (2011, 3, 13, 2, 30, 0, 0, 1300008600),  // 2:30:00 PDT ≡ 1:30 PST
        (2012, 12, 24, 0, 0, 0, 0, 1356336000),  // Leap year
        // Many names for Fri Nov 18 7:56:35 PST 2011
        (2011, 11, 18, 7, 56, 35, 0, 1321631795), // Nov 18 7:56:35
        (2011, 11, 19, -17, 56, 35, 0, 1321631795), // Nov 19 -17:56:35
        (2011, 11, 17, 31, 56, 35, 0, 1321631795), // Nov 17 31:56:35
        (2011, 11, 18, 6, 116, 35, 0, 1321631795), // Nov 18 6:116:35
        (2011, 10, 49, 7, 56, 35, 0, 1321631795), // Oct 49 7:56:35
        (2011, 11, 18, 7, 55, 95, 0, 1321631795), // Nov 18 7:55:95
        (2011, 11, 18, 7, 56, 34, 1_000_000_000, 1321631795), // Nov 18 7:56:34 + 10⁹ns
        (2011, 12, -12, 7, 56, 35, 0, 1321631795), // Dec -21 7:56:35
        (2012, 1, -43, 7, 56, 35, 0, 1321631795), // Jan -52 7:56:35 2012
        (2012, 1 - 2, 18, 7, 56, 35, 0, 1321631795), // (Jan-2) 18 7:56:35 2012
        (2010, 12 + 11, 18, 7, 56, 35, 0, 1321631795), // (Dec+11) 18 7:56:35 2010
        (1970, 1, 15297, 7, 56, 35, 0, 1321631795), // large number of days
        (1970, 1, -25508, 0, 0, 0, 0, -2203948800), // negative Unix time
    ];

    #[test]
    fn test_date() {
        go_setup();
        for &(year, month, day, hour, min, sec, nsec, unix_sec) in DATE_TESTS {
            let time = date(year, Month(month), day, hour, min, sec, nsec, &local());
            let want = unix(unix_sec, 0);
            assert!(
                time.equal(&want),
                "Date({year}, {month}, {day}, {hour}, {min}, {sec}, {nsec}, Local) = {time}, want {want}"
            );
        }
    }

    // Several ways of getting from Fri Nov 18 7:56:35 PST 2011 to Thu Mar 19
    // 7:56:35 PST 2016
    const ADD_DATE_TESTS: &[(i64, i64, i64)] =
        &[(4, 4, 1), (3, 16, 1), (3, 15, 30), (5, -6, -18 - 30 - 12)];

    #[test]
    fn test_add_date() {
        go_setup();
        let t0 = date(2011, Month(11), 18, 7, 56, 35, 0, &utc());
        let t1 = date(2016, Month(3), 19, 7, 56, 35, 0, &utc());
        for &(years, months, days) in ADD_DATE_TESTS {
            let time = t0.add_date(years, months, days);
            assert!(
                time.equal(&t1),
                "AddDate({years}, {months}, {days}) = {time}, want {t1}"
            );
        }

        let t2 = date(1899, Month(12), 31, 0, 0, 0, 0, &utc());
        let days = t2.unix() / (24 * 60 * 60);
        let t3 = unix(0, 0).add_date(0, 0, days);
        assert!(t2.equal(&t3), "Adddate(0, 0, {days}) = {t3}, want {t2}");
    }

    #[test]
    fn test_days_in() {
        go_setup();
        // The daysIn function is not exported; Go's tests reach it through
        // internal_test.go, these through gotime::internal.
        let days_in_tests = [
            (2011, 1, 31),  // January, first month, 31 days
            (2011, 2, 28),  // February, non-leap year, 28 days
            (2012, 2, 29),  // February, leap year, 29 days
            (2011, 6, 30),  // June, 30 days
            (2011, 12, 31), // December, last month, 31 days
        ];
        for (year, month, want) in days_in_tests {
            let di = internal::days_in(Month(month), year);
            assert_eq!(di, want, "got {di}; expected {want} for {year}-{month:02}");
        }
    }

    #[test]
    fn test_add_to_exact_second() {
        go_setup();
        // Add an amount to the current time to round it up to the next exact
        // second. This test checks that the nsec field still lies within the
        // range [0, 999999999].
        let t1 = now();
        let t2 = t1.add(Duration(SECOND.0 - t1.nanosecond()));
        let sec = (t1.second() + 1) % 60;
        assert!(
            t2.second() == sec && t2.nanosecond() == 0,
            "sec = {}, nsec = {}, want sec = {sec}, nsec = 0",
            t2.second(),
            t2.nanosecond()
        );
    }

    fn equal_time_and_zone(a: &Time, b: &Time) -> bool {
        let (aname, aoffset) = a.zone();
        let (bname, boffset) = b.zone();
        a.equal(b) && aoffset == boffset && aname == bname
    }

    /// Go's `TestTimeGob` sends each time through a gob stream, whose payload
    /// is `GobEncode`, which is `MarshalBinary`; this round-trips that
    /// payload (gotime has no gob codec).
    #[test]
    fn test_time_gob() {
        go_setup();
        let gob_tests = [
            date(0, Month(1), 2, 3, 4, 5, 6, &utc()),
            date(7, Month(8), 9, 10, 11, 12, 13, &fixed_zone("", 0)),
            unix(81985467080890095, 0x76543210), // Time.sec: 0x0123456789ABCDEF
            Time::default(),                     // nil location
            date(1, Month(2), 3, 4, 5, 6, 7, &fixed_zone("", 32767 * 60)),
            date(1, Month(2), 3, 4, 5, 6, 7, &fixed_zone("", -32768 * 60)),
        ];
        for tt in &gob_tests {
            let enc = tt
                .marshal_binary()
                .unwrap_or_else(|e| panic!("{tt} gob Encode error = {e:?}, want nil"));
            let mut gobtt = Time::default();
            gobtt
                .unmarshal_binary(&enc)
                .unwrap_or_else(|e| panic!("{tt} gob Decode error = {e:?}, want nil"));
            assert!(
                equal_time_and_zone(&gobtt, tt),
                "Decoded time = {gobtt}, want {tt}"
            );
        }
    }

    #[test]
    fn test_invalid_time_gob() {
        go_setup();
        let invalid_encoding_tests: [(&[u8], &str); 3] = [
            (&[], "Time.UnmarshalBinary: no data"),
            (&[0, 2, 3], "Time.UnmarshalBinary: unsupported version"),
            (&[1, 2, 3], "Time.UnmarshalBinary: invalid length"),
        ];
        for (bytes, want) in invalid_encoding_tests {
            // GobDecode is UnmarshalBinary.
            let mut ignored = Time::default();
            let err = ignored.unmarshal_binary(bytes);
            assert!(
                matches!(&err, Err(e) if e.to_string() == want),
                "time.UnmarshalBinary({bytes:?}) error = {err:?}, want {want}"
            );
        }
    }

    #[test]
    fn test_not_gob_encodable_time() {
        go_setup();
        let want = "Time.MarshalBinary: unexpected zone offset";
        let not_encodable_times = [
            date(0, Month(1), 2, 3, 4, 5, 6, &fixed_zone("", -60)),
            date(0, Month(1), 2, 3, 4, 5, 6, &fixed_zone("", -32769 * 60)),
            date(0, Month(1), 2, 3, 4, 5, 6, &fixed_zone("", 32768 * 60)),
        ];
        for time in &not_encodable_times {
            // GobEncode is MarshalBinary.
            let err = time.marshal_binary();
            assert!(
                matches!(&err, Err(e) if e.to_string() == want),
                "{time} MarshalBinary error = {err:?}, want {want}"
            );
        }
    }

    #[test]
    fn test_time_json() {
        go_setup();
        let json_tests = [
            (
                date(9999, Month(4), 12, 23, 20, 50, 520_000_000, &utc()),
                r#""9999-04-12T23:20:50.52Z""#,
            ),
            (
                date(1996, Month(12), 19, 16, 39, 57, 0, &local()),
                r#""1996-12-19T16:39:57-08:00""#,
            ),
            (
                date(0, Month(1), 1, 0, 0, 0, 1, &fixed_zone("", 60)),
                r#""0000-01-01T00:00:00.000000001+00:01""#,
            ),
            (
                date(
                    2020,
                    Month(1),
                    1,
                    0,
                    0,
                    0,
                    0,
                    &fixed_zone("", 23 * 60 * 60 + 59 * 60),
                ),
                r#""2020-01-01T00:00:00+23:59""#,
            ),
        ];
        for (time, json) in &json_tests {
            let json_bytes = serde_json::to_string(time)
                .unwrap_or_else(|e| panic!("{time} json.Marshal error = {e}, want nil"));
            assert_eq!(&json_bytes, json, "{time} JSON = {json_bytes}, want {json}");
            let json_time: Time = serde_json::from_str(&json_bytes)
                .unwrap_or_else(|e| panic!("{time} json.Unmarshal error = {e}, want nil"));
            assert!(
                equal_time_and_zone(&json_time, time),
                "Unmarshaled time = {json_time}, want {time}"
            );
        }
    }

    #[test]
    fn test_unmarshal_invalid_times() {
        go_setup();
        let tests = [
            ("{}", "Time.UnmarshalJSON: input is not a JSON string"),
            ("[]", "Time.UnmarshalJSON: input is not a JSON string"),
            (r#""2000-01-01T1:12:34Z""#, "<nil>"),
            (r#""2000-01-01T00:00:00,000Z""#, "<nil>"),
            (r#""2000-01-01T00:00:00+24:00""#, "<nil>"),
            (r#""2000-01-01T00:00:00+00:60""#, "<nil>"),
            (
                r#""2000-01-01T00:00:00+123:45""#,
                r#"parsing time "2000-01-01T00:00:00+123:45" as "2006-01-02T15:04:05Z07:00": cannot parse "+123:45" as "Z07:00""#,
            ),
        ];

        let sprint = |err: Option<String>| err.unwrap_or_else(|| "<nil>".to_owned());
        for (input, want) in tests {
            // json.Unmarshal hands the raw value to UnmarshalJSON.
            let mut ts = Time::default();
            let err = sprint(
                ts.unmarshal_json(input.as_bytes())
                    .err()
                    .map(|e| e.to_string()),
            );
            assert_eq!(
                err, want,
                "Time.UnmarshalJSON({input}) = {err}, want {want}"
            );

            // Through serde, serde_json adds the position to the same message.
            let err = sprint(serde_json::from_str::<Time>(input).err().map(|e| {
                e.to_string()
                    .replace(&format!(" at line {} column {}", e.line(), e.column()), "")
            }));
            assert_eq!(
                err, want,
                "serde_json::from_str::<Time>({input}) = {err}, want {want}"
            );

            if input.starts_with('"') && input.ends_with('"') {
                let err = sprint(
                    ts.unmarshal_text(input.trim_matches('"').as_bytes())
                        .err()
                        .map(|e| e.to_string()),
                );
                assert_eq!(
                    err, want,
                    "Time.UnmarshalText({input}) = {err}, want {want}"
                );
            }
        }
    }

    #[test]
    fn test_marshal_invalid_times() {
        go_setup();
        let tests = [
            (
                date(10000, Month(1), 1, 0, 0, 0, 0, &utc()),
                "Time.MarshalJSON: year outside of range [0,9999]",
            ),
            (
                date(-998, Month(1), 1, 0, 0, 0, 0, &utc()).add(Duration(-SECOND.0)),
                "Time.MarshalJSON: year outside of range [0,9999]",
            ),
            (
                date(0, Month(1), 1, 0, 0, 0, 0, &utc()).add(Duration(-NANOSECOND.0)),
                "Time.MarshalJSON: year outside of range [0,9999]",
            ),
            (
                date(2020, Month(1), 1, 0, 0, 0, 0, &fixed_zone("", 24 * 60 * 60)),
                "Time.MarshalJSON: timezone hour outside of range [0,23]",
            ),
            (
                date(
                    2020,
                    Month(1),
                    1,
                    0,
                    0,
                    0,
                    0,
                    &fixed_zone("", 123 * 60 * 60),
                ),
                "Time.MarshalJSON: timezone hour outside of range [0,23]",
            ),
        ];

        for (time, want) in &tests {
            match time.marshal_json() {
                Ok(b) => panic!("({time}).MarshalJSON() = {b:?}, want nil"),
                Err(err) => assert_eq!(err.to_string(), *want, "({time}).MarshalJSON() error"),
            }

            let want_text = want.replace("JSON", "Text");
            match time.marshal_text() {
                Ok(b) => panic!("({time}).MarshalText() = {b:?}, want nil"),
                Err(err) => assert_eq!(err.to_string(), want_text, "({time}).MarshalText() error"),
            }

            // Go returns a nil slice; the Rust buffer is left as it was.
            let mut buf = Vec::with_capacity(64);
            let want_append = want.replace("MarshalJSON", "AppendText");
            match time.append_text(&mut buf) {
                Ok(()) => panic!("({time}).AppendText() = {buf:?}, want nil"),
                Err(err) => {
                    assert!(buf.is_empty(), "({time}).AppendText() = {buf:?}, want nil");
                    assert_eq!(err.to_string(), want_append, "({time}).AppendText() error");
                }
            }
        }
    }

    fn parse_duration_tests() -> Vec<(&'static str, Duration)> {
        let (ns, us, ms, s, m, h) = (
            NANOSECOND.0,
            MICROSECOND.0,
            MILLISECOND.0,
            SECOND.0,
            MINUTE.0,
            HOUR.0,
        );
        vec![
            // simple
            ("0", d(0)),
            ("5s", d(5 * s)),
            ("30s", d(30 * s)),
            ("1478s", d(1478 * s)),
            // sign
            ("-5s", d(-5 * s)),
            ("+5s", d(5 * s)),
            ("-0", d(0)),
            ("+0", d(0)),
            // decimal
            ("5.0s", d(5 * s)),
            ("5.6s", d(5 * s + 600 * ms)),
            ("5.s", d(5 * s)),
            (".5s", d(500 * ms)),
            ("1.0s", d(s)),
            ("1.00s", d(s)),
            ("1.004s", d(s + 4 * ms)),
            ("1.0040s", d(s + 4 * ms)),
            ("100.00100s", d(100 * s + ms)),
            // different units
            ("10ns", d(10 * ns)),
            ("11us", d(11 * us)),
            ("12µs", d(12 * us)), // U+00B5
            ("12μs", d(12 * us)), // U+03BC
            ("13ms", d(13 * ms)),
            ("14s", d(14 * s)),
            ("15m", d(15 * m)),
            ("16h", d(16 * h)),
            // composite durations
            ("3h30m", d(3 * h + 30 * m)),
            ("10.5s4m", d(4 * m + 10 * s + 500 * ms)),
            ("-2m3.4s", d(-(2 * m + 3 * s + 400 * ms))),
            (
                "1h2m3s4ms5us6ns",
                d(h + 2 * m + 3 * s + 4 * ms + 5 * us + 6 * ns),
            ),
            ("39h9m14.425s", d(39 * h + 9 * m + 14 * s + 425 * ms)),
            // large value
            ("52763797000ns", d(52763797000 * ns)),
            // more than 9 digits after decimal point, see https://golang.org/issue/6617
            ("0.3333333333333333333h", d(20 * m)),
            // 9007199254740993 = 1<<53+1 cannot be stored precisely in a float64
            ("9007199254740993ns", d(((1 << 53) + 1) * ns)),
            // largest duration that can be represented by int64 in nanoseconds
            ("9223372036854775807ns", d(i64::MAX)),
            ("9223372036854775.807us", d(i64::MAX)),
            ("9223372036s854ms775us807ns", d(i64::MAX)),
            ("-9223372036854775808ns", d(i64::MIN)),
            ("-9223372036854775.808us", d(i64::MIN)),
            ("-9223372036s854ms775us808ns", d(i64::MIN)),
            // largest negative value
            ("-9223372036854775808ns", d(i64::MIN)),
            // largest negative round trip value, see https://golang.org/issue/48629
            ("-2562047h47m16.854775808s", d(i64::MIN)),
            // huge string; issue 15011.
            ("0.100000000000000000000h", d(6 * m)),
            // This value tests the first overflow check in leadingFraction.
            (
                "0.830103483285477580700h",
                d(49 * m + 48 * s + 372539827 * ns),
            ),
        ]
    }

    #[test]
    fn test_parse_duration() {
        go_setup();
        for (input, want) in parse_duration_tests() {
            let got = parse_duration(input);
            assert!(
                matches!(got, Ok(d) if d == want),
                "ParseDuration({input:?}) = {got:?}, want {want}, nil"
            );
        }
    }

    #[test]
    fn test_parse_duration_errors() {
        go_setup();
        let parse_duration_error_tests: [(&[u8], &str); 21] = [
            // invalid
            (b"", r#""""#),
            (b"3", r#""3""#),
            (b"-", r#""-""#),
            (b"s", r#""s""#),
            (b".", r#"".""#),
            (b"-.", r#""-.""#),
            (b".s", r#"".s""#),
            (b"+.s", r#""+.s""#),
            (b"1d", r#""1d""#),
            (b"\x85\x85", r#""\x85\x85""#),
            (b"\xffff", r#""\xffff""#),
            (b"hello \xffff world", r#""hello \xffff world""#),
            ("\u{FFFD}".as_bytes(), r#""\xef\xbf\xbd""#), // utf8.RuneError
            (
                "\u{FFFD} hello \u{FFFD} world".as_bytes(),
                r#""\xef\xbf\xbd hello \xef\xbf\xbd world""#,
            ), // utf8.RuneError
            // overflow
            (b"9223372036854775810ns", r#""9223372036854775810ns""#),
            (b"9223372036854775808ns", r#""9223372036854775808ns""#),
            (b"-9223372036854775809ns", r#""-9223372036854775809ns""#),
            (b"9223372036854776us", r#""9223372036854776us""#),
            (b"3000000h", r#""3000000h""#),
            (b"9223372036854775.808us", r#""9223372036854775.808us""#),
            (
                b"9223372036854ms775us808ns",
                r#""9223372036854ms775us808ns""#,
            ),
        ];
        for (input, expect) in parse_duration_error_tests {
            match parse_duration(input) {
                Ok(d) => panic!("ParseDuration({input:?}) = {d}, nil, want _, non-nil"),
                Err(err) => assert!(
                    err.to_string().contains(expect),
                    "ParseDuration({input:?}) = _, {:?}, error does not contain {expect:?}",
                    err.to_string()
                ),
            }
        }
    }

    #[test]
    fn test_parse_duration_round_trip() {
        go_setup();
        // https://golang.org/issue/48629
        let max0 = Duration::MAX;
        let max1 = parse_duration(max0.to_string());
        assert!(
            matches!(max1, Ok(d) if d == max0),
            "round-trip failed: {} => {:?} => {max1:?}",
            max0.0,
            max0.to_string()
        );

        let min0 = Duration::MIN;
        let min1 = parse_duration(min0.to_string());
        assert!(
            matches!(min1, Ok(d) if d == min0),
            "round-trip failed: {} => {:?} => {min1:?}",
            min0.0,
            min0.to_string()
        );

        let mut rng = Rng::new(6);
        for _ in 0..100 {
            // Resolutions finer than milliseconds will result in imprecise
            // round-trips.
            let d0 = Duration(i64::from(rng.int31()) * MILLISECOND.0);
            let s = d0.to_string();
            let d1 = parse_duration(&s);
            assert!(
                matches!(d1, Ok(d) if d == d0),
                "round-trip failed: {} => {s:?} => {d1:?}",
                d0.0
            );
        }
    }

    /// golang.org/issue/4622: initializing Local from two threads at once.
    /// Runs in a child process: resetting Local would change it under the
    /// other tests of this binary.
    #[test]
    fn test_location_race() {
        go_setup();
        if !is_child() {
            run_in_child("gotime::test_location_race", &[]);
            return;
        }
        internal::reset_local(); // reset the Once to trigger the race

        let other = std::thread::spawn(|| now().string());
        let _ = now().string();
        other.join().expect("thread");
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Back to Los Angeles (this process runs no other test).
        let source = internal::goroot_zone_source(&crate::common::goroot()).expect("goroot");
        let la =
            internal::load_location_from_sources("America/Los_Angeles", &[&source]).expect("LA");
        gotime::set_local(la.renamed("Local"));
    }

    #[test]
    fn test_load_fixed() {
        go_setup();
        // Issue 4064: handle locations without any zone transitions.
        let loc = load_location("Etc/GMT+1").expect("LoadLocation");

        // The tzdata name Etc/GMT+1 uses "east is negative", but Go and most
        // other systems use "east is positive". So GMT+1 corresponds to -3600
        // in the Go zone, not +3600.
        let (name, offset) = now().in_location(&loc).zone();
        // The zone abbreviation is "-01" since tzdata-2016g, and "GMT+1" on
        // earlier versions; we accept both. (Issue 17276.) (Go also accepts
        // "+01" on OpenBSD.)
        let want_name = ["GMT+1", "-01"];
        assert!(
            want_name.contains(&name.as_str()) && offset == -60 * 60,
            "Now().In(loc).Zone() = {name:?}, {offset}, want {want_name:?} (one of), {}",
            -60 * 60
        );
    }

    const MIN_DURATION: Duration = Duration::MIN;
    const MAX_DURATION: Duration = Duration::MAX;

    #[test]
    fn test_sub() {
        go_setup();
        let u = utc();
        let min_mono_time = internal::time_from_parts(1 << 63, i64::MIN, Some(utc()));
        let max_mono_time = internal::time_from_parts(1 << 63, i64::MAX, Some(utc()));
        let sub_tests = [
            (Time::default(), Time::default(), d(0)),
            (
                date(2009, Month(11), 23, 0, 0, 0, 1, &u),
                date(2009, Month(11), 23, 0, 0, 0, 0, &u),
                d(1),
            ),
            (
                date(2009, Month(11), 23, 0, 0, 0, 0, &u),
                date(2009, Month(11), 24, 0, 0, 0, 0, &u),
                d(-24 * HOUR.0),
            ),
            (
                date(2009, Month(11), 24, 0, 0, 0, 0, &u),
                date(2009, Month(11), 23, 0, 0, 0, 0, &u),
                d(24 * HOUR.0),
            ),
            (
                date(-2009, Month(11), 24, 0, 0, 0, 0, &u),
                date(-2009, Month(11), 23, 0, 0, 0, 0, &u),
                d(24 * HOUR.0),
            ),
            (
                Time::default(),
                date(2109, Month(11), 23, 0, 0, 0, 0, &u),
                MIN_DURATION,
            ),
            (
                date(2109, Month(11), 23, 0, 0, 0, 0, &u),
                Time::default(),
                MAX_DURATION,
            ),
            (
                Time::default(),
                date(-2109, Month(11), 23, 0, 0, 0, 0, &u),
                MAX_DURATION,
            ),
            (
                date(-2109, Month(11), 23, 0, 0, 0, 0, &u),
                Time::default(),
                MIN_DURATION,
            ),
            (
                date(2290, Month(1), 1, 0, 0, 0, 0, &u),
                date(2000, Month(1), 1, 0, 0, 0, 0, &u),
                d(290 * 365 * 24 * HOUR.0 + 71 * 24 * HOUR.0),
            ),
            (
                date(2300, Month(1), 1, 0, 0, 0, 0, &u),
                date(2000, Month(1), 1, 0, 0, 0, 0, &u),
                MAX_DURATION,
            ),
            (
                date(2000, Month(1), 1, 0, 0, 0, 0, &u),
                date(2290, Month(1), 1, 0, 0, 0, 0, &u),
                d(-290 * 365 * 24 * HOUR.0 - 71 * 24 * HOUR.0),
            ),
            (
                date(2000, Month(1), 1, 0, 0, 0, 0, &u),
                date(2300, Month(1), 1, 0, 0, 0, 0, &u),
                MIN_DURATION,
            ),
            (
                date(2311, Month(11), 26, 2, 16, 47, 63535996, &u),
                date(2019, Month(8), 16, 2, 29, 30, 268436582, &u),
                d(9223372036795099414),
            ),
            (min_mono_time.clone(), max_mono_time.clone(), MIN_DURATION),
            (max_mono_time, min_mono_time, MAX_DURATION),
        ];
        for (i, (t, u, want)) in sub_tests.iter().enumerate() {
            let got = t.sub(u);
            assert_eq!(got, *want, "#{i}: Sub({t}, {u}): got {got}; want {want}");
        }
    }

    #[test]
    fn test_duration_nanoseconds() {
        go_setup();
        for (dur, want) in [(d(-1000), -1000), (d(-1), -1), (d(1), 1), (d(1000), 1000)] {
            let got = dur.nanoseconds();
            assert_eq!(
                got, want,
                "Duration({dur}).Nanoseconds() = {got}; want: {want}"
            );
        }
    }

    #[test]
    fn test_duration_microseconds() {
        go_setup();
        for (dur, want) in [(d(-1000), -1), (d(1000), 1)] {
            let got = dur.microseconds();
            assert_eq!(
                got, want,
                "Duration({dur}).Microseconds() = {got}; want: {want}"
            );
        }
    }

    #[test]
    fn test_duration_milliseconds() {
        go_setup();
        for (dur, want) in [(d(-1000000), -1), (d(1000000), 1)] {
            let got = dur.milliseconds();
            assert_eq!(
                got, want,
                "Duration({dur}).Milliseconds() = {got}; want: {want}"
            );
        }
    }

    #[test]
    fn test_duration_seconds() {
        go_setup();
        let sec_duration_tests = [(d(300000000), 0.3)];
        for (dur, want) in sec_duration_tests {
            let got = dur.seconds();
            assert_eq!(got, want, "Duration({dur}).Seconds() = {got}; want: {want}");
        }
    }

    #[test]
    fn test_duration_minutes() {
        go_setup();
        let min_duration_tests = [
            (d(-60000000000), -1.0),
            (d(-1), -1.0 / 60e9),
            (d(1), 1.0 / 60e9),
            (d(60000000000), 1.0),
            (d(3000), 5e-8),
        ];
        for (dur, want) in min_duration_tests {
            let got = dur.minutes();
            assert_eq!(got, want, "Duration({dur}).Minutes() = {got}; want: {want}");
        }
    }

    #[test]
    fn test_duration_hours() {
        go_setup();
        let hour_duration_tests = [
            (d(-3600000000000), -1.0),
            (d(-1), -1.0 / 3600e9),
            (d(1), 1.0 / 3600e9),
            (d(3600000000000), 1.0),
            (d(36), 1e-11),
        ];
        for (dur, want) in hour_duration_tests {
            let got = dur.hours();
            assert_eq!(got, want, "Duration({dur}).Hours() = {got}; want: {want}");
        }
    }

    #[test]
    fn test_duration_truncate() {
        go_setup();
        let (s, m, h) = (SECOND.0, MINUTE.0, HOUR.0);
        let duration_truncate_tests = [
            (0, s, 0),
            (m, -7 * s, m),
            (m, 0, m),
            (m, 1, m),
            (m + 10 * s, 10 * s, m + 10 * s),
            (2 * m + 10 * s, m, 2 * m),
            (10 * m + 10 * s, 3 * m, 9 * m),
            (m + 10 * s, m + 10 * s + 1, 0),
            (m + 10 * s, h, 0),
            (-m, s, -m),
            (-10 * m, 3 * m, -9 * m),
            (-10 * m, h, 0),
        ];
        for (dur, mul, want) in duration_truncate_tests {
            let got = d(dur).truncate(d(mul));
            assert_eq!(
                got,
                d(want),
                "Duration({}).Truncate({}) = {got}; want: {}",
                d(dur),
                d(mul),
                d(want)
            );
        }
    }

    #[test]
    fn test_duration_round() {
        go_setup();
        let (s, m) = (SECOND.0, MINUTE.0);
        let duration_round_tests = [
            (0, s, 0),
            (m, -11 * s, m),
            (m, 0, m),
            (m, 1, m),
            (2 * m, m, 2 * m),
            (2 * m + 10 * s, m, 2 * m),
            (2 * m + 30 * s, m, 3 * m),
            (2 * m + 50 * s, m, 3 * m),
            (-m, 1, -m),
            (-2 * m, m, -2 * m),
            (-2 * m - 10 * s, m, -2 * m),
            (-2 * m - 30 * s, m, -3 * m),
            (-2 * m - 50 * s, m, -3 * m),
            (
                8_000_000_000_000_000_000,
                3_000_000_000_000_000_000,
                9_000_000_000_000_000_000,
            ),
            (
                9_000_000_000_000_000_000,
                5_000_000_000_000_000_000,
                i64::MAX,
            ),
            (
                -8_000_000_000_000_000_000,
                3_000_000_000_000_000_000,
                -9_000_000_000_000_000_000,
            ),
            (
                -9_000_000_000_000_000_000,
                5_000_000_000_000_000_000,
                i64::MIN,
            ),
            ((3 << 61) - 1, 3 << 61, 3 << 61),
        ];
        for (dur, mul, want) in duration_round_tests {
            let got = d(dur).round(d(mul));
            assert_eq!(
                got,
                d(want),
                "Duration({}).Round({}) = {got}; want: {}",
                d(dur),
                d(mul),
                d(want)
            );
        }
    }

    #[test]
    fn test_duration_abs() {
        go_setup();
        let m = MINUTE.0;
        let duration_abs_tests = [
            (d(0), d(0)),
            (d(1), d(1)),
            (d(-1), d(1)),
            (d(m), d(m)),
            (d(-m), d(m)),
            (MIN_DURATION, MAX_DURATION),
            (d(MIN_DURATION.0 + 1), MAX_DURATION),
            (d(MIN_DURATION.0 + 2), d(MAX_DURATION.0 - 1)),
            (MAX_DURATION, MAX_DURATION),
            (d(MAX_DURATION.0 - 1), d(MAX_DURATION.0 - 1)),
        ];
        for (dur, want) in duration_abs_tests {
            let got = dur.abs();
            assert_eq!(got, want, "Duration({dur}).Abs() = {got}; want: {want}");
        }
    }

    type DefaultLocTest = (&'static str, fn(&Time, &Time) -> bool);

    #[test]
    fn test_default_loc() {
        go_setup();
        let default_loc_tests: &[DefaultLocTest] = &[
            ("After", |t1, t2| t1.after(t2) == t2.after(t1)),
            ("Before", |t1, t2| t1.before(t2) == t2.before(t1)),
            ("Equal", |t1, t2| t1.equal(t2) == t2.equal(t1)),
            ("Compare", |t1, t2| t1.compare(t2) == t2.compare(t1)),
            ("IsZero", |t1, t2| t1.is_zero() == t2.is_zero()),
            ("Date", |t1, t2| t1.date() == t2.date()),
            ("Year", |t1, t2| t1.year() == t2.year()),
            ("Month", |t1, t2| t1.month() == t2.month()),
            ("Day", |t1, t2| t1.day() == t2.day()),
            ("Weekday", |t1, t2| t1.weekday() == t2.weekday()),
            ("ISOWeek", |t1, t2| t1.iso_week() == t2.iso_week()),
            ("Clock", |t1, t2| t1.clock() == t2.clock()),
            ("Hour", |t1, t2| t1.hour() == t2.hour()),
            ("Minute", |t1, t2| t1.minute() == t2.minute()),
            ("Second", |t1, t2| t1.second() == t2.second()),
            // (Go's "Nanosecond" case compares Hour.)
            ("Nanosecond", |t1, t2| t1.hour() == t2.hour()),
            ("YearDay", |t1, t2| t1.year_day() == t2.year_day()),
            // Using Equal since Add don't modify loc using "==" will cause a fail
            ("Add", |t1, t2| t1.add(HOUR).equal(&t2.add(HOUR))),
            ("Sub", |t1, t2| t1.sub(t2) == t2.sub(t1)),
            // Original cause for this test case bug 15852
            ("AddDate", |t1, t2| {
                t1.add_date(1991, 9, 3) == t2.add_date(1991, 9, 3)
            }),
            ("UTC", |t1, t2| t1.utc() == t2.utc()),
            ("Local", |t1, t2| t1.local() == t2.local()),
            ("In", |t1, t2| {
                t1.in_location(&utc()) == t2.in_location(&utc())
            }),
            ("Local", |t1, t2| t1.local() == t2.local()),
            ("Zone", |t1, t2| t1.zone() == t2.zone()),
            ("Unix", |t1, t2| t1.unix() == t2.unix()),
            ("UnixNano", |t1, t2| t1.unix_nano() == t2.unix_nano()),
            ("UnixMilli", |t1, t2| t1.unix_milli() == t2.unix_milli()),
            ("UnixMicro", |t1, t2| t1.unix_micro() == t2.unix_micro()),
            ("AppendBinary", |t1, t2| {
                let mut buf1 = vec![0u8; 4];
                let mut buf2 = vec![0u8; 4];
                let b1 = t1.append_binary(&mut buf1);
                let b2 = t2.append_binary(&mut buf2);
                buf1[4..] == buf2[4..] && b1 == b2
            }),
            ("MarshalBinary", |t1, t2| {
                t1.marshal_binary() == t2.marshal_binary()
            }),
            // GobEncode is MarshalBinary.
            ("GobEncode", |t1, t2| {
                t1.marshal_binary() == t2.marshal_binary()
            }),
            ("MarshalJSON", |t1, t2| {
                t1.marshal_json() == t2.marshal_json()
            }),
            ("AppendText", |t1, t2| {
                let mut buf1 = vec![0u8; 4];
                let mut buf2 = vec![0u8; 4];
                let b1 = t1.append_text(&mut buf1);
                let b2 = t2.append_text(&mut buf2);
                buf1[4..] == buf2[4..] && b1 == b2
            }),
            ("MarshalText", |t1, t2| {
                t1.marshal_text() == t2.marshal_text()
            }),
            ("Truncate", |t1, t2| {
                t1.truncate(HOUR).equal(&t2.truncate(HOUR))
            }),
            ("Round", |t1, t2| t1.round(HOUR).equal(&t2.round(HOUR))),
            ("== Time{}", |t1, t2| {
                (*t1 == Time::default()) == (*t2 == Time::default())
            }),
        ];

        // Verify that all of Time's methods behave identically if loc is set
        // to nil or UTC.
        for (name, f) in default_loc_tests {
            let t1 = Time::default();
            let t2 = Time::default().utc();
            assert!(
                f(&t1, &t2),
                "Time{{}} and Time{{}}.UTC() behave differently for {name}"
            );
        }
    }

    #[test]
    fn test_marshal_binary_zero_time() {
        go_setup();
        let t0 = Time::default();
        let enc = t0.marshal_binary().expect("MarshalBinary");
        let mut t1 = now(); // not zero
        t1.unmarshal_binary(&enc).expect("UnmarshalBinary");
        assert!(
            t1 == t0,
            "t0={:?}\nt1={:?}\nwant identical structures",
            internal::time_parts(&t0),
            internal::time_parts(&t1)
        );
    }

    #[test]
    fn test_marshal_binary_version2() {
        go_setup();
        let t0 = parse(RFC3339, "1880-01-01T00:00:00Z").expect("Failed to parse time");
        let loc = load_location("US/Eastern").expect("Failed to load location");
        let t1 = t0.in_location(&loc);
        let b = t1.marshal_binary().expect("Failed to Marshal");

        let mut t2 = Time::default();
        t2.unmarshal_binary(&b).expect("Failed to Unmarshal");

        assert!(
            t0.equal(&t1),
            "The result t1: {t1} after Marshal is not matched original t0: {t0}"
        );
        assert!(
            t1.equal(&t2),
            "The result t2: {t2} after Unmarshal is not matched original t1: {t1}"
        );
    }

    #[test]
    fn test_marshal_binary_version2_bugfix() {
        go_setup();
        let offset_sec = -5 * 3600 - 30 * 60 - 45; // -19845
        let loc = fixed_zone("LMT", offset_sec);

        let t1 = date(2024, Month(6), 15, 12, 0, 0, 0, &loc);
        let b = t1.marshal_binary().expect("Failed to Marshal");

        let mut t2 = Time::default();
        t2.unmarshal_binary(&b).expect("Failed to Unmarshal");

        let (_, t1_origin_offset) = t1.zone();
        let (_, t2_origin_offset) = t2.zone();
        assert_eq!(
            t1_origin_offset, t2_origin_offset,
            "The result offsetSec: {t2_origin_offset} after Unmarshal is not matched original offsetSec: {t1_origin_offset}"
        );
    }

    // Issue 17720: Zero value of time.Month fails to print
    #[test]
    fn test_zero_month_string() {
        go_setup();
        let (got, want) = (Month(0).to_string(), "%!Month(0)");
        assert_eq!(got, want, "zero month = {got:?}; want {want:?}");
    }

    // Issue 24692: Out of range weekday panics
    #[test]
    fn test_weekday_string() {
        go_setup();
        let (got, want) = (Weekday::TUESDAY.to_string(), "Tuesday");
        assert_eq!(got, want, "Tuesday weekday = {got:?}; want {want:?}");
        let (got, want) = (Weekday(14).to_string(), "%!Weekday(14)");
        assert_eq!(got, want, "14th weekday = {got:?}; want {want:?}");
    }

    #[test]
    fn test_read_file_limit() {
        go_setup();
        let zero = "/dev/zero";
        if std::fs::metadata(zero).is_err() {
            eprintln!("skipping test without a /dev/zero");
            return;
        }
        let err = internal::read_file(zero);
        assert!(
            matches!(&err, Err(e) if e.to_string().contains("is too large")),
            "readFile({zero:?}) error = {err:?}; want error containing 'is too large'",
            err = err.as_ref().map(Vec::len)
        );
    }

    #[test]
    fn test_time_is_dst() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        let tz_with_dst =
            load_location("Australia/Sydney").expect("could not load tz 'Australia/Sydney'");
        let tz_without_dst =
            load_location("Australia/Brisbane").expect("could not load tz 'Australia/Brisbane'");
        let tz_fixed = fixed_zone("FIXED_TIME", 12345);

        let tests = [
            (date(2009, Month(1), 1, 12, 0, 0, 0, &utc()), false),
            (date(2009, Month(6), 1, 12, 0, 0, 0, &utc()), false),
            (date(2009, Month(1), 1, 12, 0, 0, 0, &tz_with_dst), true),
            (date(2009, Month(6), 1, 12, 0, 0, 0, &tz_with_dst), false),
            (date(2009, Month(1), 1, 12, 0, 0, 0, &tz_without_dst), false),
            (date(2009, Month(6), 1, 12, 0, 0, 0, &tz_without_dst), false),
            (date(2009, Month(1), 1, 12, 0, 0, 0, &tz_fixed), false),
            (date(2009, Month(6), 1, 12, 0, 0, 0, &tz_fixed), false),
        ];

        for (i, (time, want)) in tests.iter().enumerate() {
            let got = time.is_dst();
            assert_eq!(
                got,
                *want,
                "#{i}:: ({:?}).IsDST()={got}, want {want}",
                time.format(RFC3339)
            );
        }
    }

    #[test]
    fn test_time_add_sec_overflow() {
        go_setup();
        // Go's int64 arithmetic wraps, hence wrapping_add.
        // Test it with positive delta.
        let mut max_int64 = i64::MAX;
        let time_ext = max_int64 - internal::UNIX_TO_INTERNAL - 50;
        let mut not_mono_time = unix(time_ext, 0);
        for i in 0i64..100 {
            let sec = not_mono_time.unix();
            not_mono_time = not_mono_time.add(Duration(i * 1_000_000_000));
            let new_sec = not_mono_time.unix();
            assert!(
                !(new_sec != sec.wrapping_add(i)
                    && new_sec.wrapping_add(internal::UNIX_TO_INTERNAL) != max_int64),
                "time ext: {new_sec} overflows with positive delta, overflow threshold: {max_int64}"
            );
        }

        // Test it with negative delta.
        max_int64 = -max_int64;
        not_mono_time = internal::time_from_parts(0, i64::MIN + 50, None); // NotMonoNegativeTime
        for i in (-99i64..=0).rev() {
            let sec = not_mono_time.unix();
            not_mono_time = not_mono_time.add(Duration(i * 1_000_000_000));
            let new_sec = not_mono_time.unix();
            assert!(
                !(new_sec != sec.wrapping_add(i)
                    && new_sec.wrapping_add(internal::UNIX_TO_INTERNAL) != max_int64),
                "time ext: {new_sec} overflows with positive delta, overflow threshold: {max_int64}"
            );
        }
    }

    // Issue 49284: time: ParseInLocation incorrectly because of Daylight Saving
    // Time
    #[test]
    fn test_time_with_zone_transition() {
        go_setup();
        let _undo = internal::disable_platform_sources();

        let loc = load_location("Asia/Shanghai").expect("LoadLocation");
        let u = utc();

        let tests = [
            // 14 Apr 1991 - Daylight Saving Time Started
            // When time of "Asia/Shanghai" was about to reach
            // Sunday, 14 April 1991, 02:00:00 clocks were turned forward 1 hour to
            // Sunday, 14 April 1991, 03:00:00 local daylight time instead.
            // The UTC time was 13 April 1991, 18:00:00
            (
                date(1991, M::APRIL, 13, 17, 50, 0, 0, &loc),
                date(1991, M::APRIL, 13, 9, 50, 0, 0, &u),
            ),
            (
                date(1991, M::APRIL, 13, 18, 0, 0, 0, &loc),
                date(1991, M::APRIL, 13, 10, 0, 0, 0, &u),
            ),
            (
                date(1991, M::APRIL, 14, 1, 50, 0, 0, &loc),
                date(1991, M::APRIL, 13, 17, 50, 0, 0, &u),
            ),
            (
                date(1991, M::APRIL, 14, 3, 0, 0, 0, &loc),
                date(1991, M::APRIL, 13, 18, 0, 0, 0, &u),
            ),
            // 15 Sep 1991 - Daylight Saving Time Ended
            // When local daylight time of "Asia/Shanghai" was about to reach
            // Sunday, 15 September 1991, 02:00:00 clocks were turned backward 1 hour to
            // Sunday, 15 September 1991, 01:00:00 local standard time instead.
            // The UTC time was 14 September 1991, 17:00:00
            (
                date(1991, M::SEPTEMBER, 14, 16, 50, 0, 0, &loc),
                date(1991, M::SEPTEMBER, 14, 7, 50, 0, 0, &u),
            ),
            (
                date(1991, M::SEPTEMBER, 14, 17, 0, 0, 0, &loc),
                date(1991, M::SEPTEMBER, 14, 8, 0, 0, 0, &u),
            ),
            (
                date(1991, M::SEPTEMBER, 15, 0, 50, 0, 0, &loc),
                date(1991, M::SEPTEMBER, 14, 15, 50, 0, 0, &u),
            ),
            (
                date(1991, M::SEPTEMBER, 15, 2, 0, 0, 0, &loc),
                date(1991, M::SEPTEMBER, 14, 18, 0, 0, 0, &u),
            ),
        ];

        for (i, (give, want)) in tests.iter().enumerate() {
            assert!(
                give.equal(want),
                "#{i}:: {:?} is not equal to {:?}",
                give.format(RFC3339),
                want.format(RFC3339)
            );
        }
    }

    #[test]
    fn test_zone_bounds() {
        go_setup();
        let _undo = internal::disable_platform_sources();
        let loc = load_location("Asia/Shanghai").expect("LoadLocation");

        // The ZoneBounds of a UTC location would just return two zero Time.
        for test in UTC_TESTS {
            let sec = test.seconds;
            let tm = unix(sec, 0).utc();
            let (start, end) = tm.zone_bounds();
            assert!(
                start.is_zero() && end.is_zero(),
                "ZoneBounds of {sec} expects two zero Time, got:\n  start={start}\n  end={end}"
            );
        }

        // If the zone begins at the beginning of time, start will be returned
        // as a zero Time. Use math.MinInt32 to avoid overflow of int arguments
        // on 32-bit systems.
        let begin_time = date(i64::from(i32::MIN), M::JANUARY, 1, 0, 0, 0, 0, &loc);
        let (start, end) = begin_time.zone_bounds();
        assert!(
            start.is_zero() && !end.is_zero(),
            "ZoneBounds of {begin_time} expects start is zero Time, got:\n  start={start}\n  end={end}"
        );

        // If the zone goes on forever, end will be returned as a zero Time.
        // Use math.MaxInt32 to avoid overflow of int arguments on 32-bit
        // systems.
        let forever_time = date(i64::from(i32::MAX), M::JANUARY, 1, 0, 0, 0, 0, &loc);
        let (start, end) = forever_time.zone_bounds();
        assert!(
            !start.is_zero() && end.is_zero(),
            "ZoneBounds of {forever_time} expects end is zero Time, got:\n  start={start}\n  end={end}"
        );

        // Check some real-world cases to make sure we're getting the right
        // bounds.
        let bound_one = date(1990, M::SEPTEMBER, 16, 1, 0, 0, 0, &loc);
        let bound_two = date(1991, M::APRIL, 14, 3, 0, 0, 0, &loc);
        let bound_three = date(1991, M::SEPTEMBER, 15, 1, 0, 0, 0, &loc);
        let make_local_time = |sec: i64| unix(sec, 0);
        let zero = Time::default();
        let real_tests = [
            // The ZoneBounds of "Asia/Shanghai" Daylight Saving Time
            (
                date(1991, M::APRIL, 13, 17, 50, 0, 0, &loc),
                bound_one.clone(),
                bound_two.clone(),
            ),
            (
                date(1991, M::APRIL, 13, 18, 0, 0, 0, &loc),
                bound_one.clone(),
                bound_two.clone(),
            ),
            (
                date(1991, M::APRIL, 14, 1, 50, 0, 0, &loc),
                bound_one.clone(),
                bound_two.clone(),
            ),
            (bound_two.clone(), bound_two.clone(), bound_three.clone()),
            (
                date(1991, M::SEPTEMBER, 14, 16, 50, 0, 0, &loc),
                bound_two.clone(),
                bound_three.clone(),
            ),
            (
                date(1991, M::SEPTEMBER, 14, 17, 0, 0, 0, &loc),
                bound_two.clone(),
                bound_three.clone(),
            ),
            (
                date(1991, M::SEPTEMBER, 15, 0, 50, 0, 0, &loc),
                bound_two.clone(),
                bound_three.clone(),
            ),
            // The ZoneBounds of a "Asia/Shanghai" after the last transition
            // (Standard Time)
            (bound_three.clone(), bound_three.clone(), zero.clone()),
            (
                date(1991, M::DECEMBER, 15, 1, 50, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::APRIL, 13, 17, 50, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::APRIL, 13, 18, 0, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::APRIL, 14, 1, 50, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::SEPTEMBER, 14, 16, 50, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::SEPTEMBER, 14, 17, 0, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            (
                date(1992, M::SEPTEMBER, 15, 0, 50, 0, 0, &loc),
                bound_three.clone(),
                zero.clone(),
            ),
            // The ZoneBounds of a local time would return two local Time.
            // Note: We preloaded "America/Los_Angeles" as time.Local for
            // testing
            (
                make_local_time(0),
                make_local_time(-5756400),
                make_local_time(9972000),
            ),
            (
                make_local_time(1221681866),
                make_local_time(1205056800),
                make_local_time(1225616400),
            ),
            (
                make_local_time(2152173599),
                make_local_time(2145916800),
                make_local_time(2152173600),
            ),
            (
                make_local_time(2152173600),
                make_local_time(2152173600),
                make_local_time(2172733200),
            ),
            (
                make_local_time(2152173601),
                make_local_time(2152173600),
                make_local_time(2172733200),
            ),
            (
                make_local_time(2159200800),
                make_local_time(2152173600),
                make_local_time(2172733200),
            ),
            (
                make_local_time(2172733199),
                make_local_time(2152173600),
                make_local_time(2172733200),
            ),
            (
                make_local_time(2172733200),
                make_local_time(2172733200),
                make_local_time(2177452800),
            ),
        ];
        for (i, (give_time, want_start, want_end)) in real_tests.iter().enumerate() {
            let (start, end) = give_time.zone_bounds();
            assert!(
                start.equal(want_start) && end.equal(want_end),
                "#{i}:: ZoneBounds of {give_time} expects right bounds:\n  got start={start}\n  want start={want_start}\n  got end={end}\n  want end={want_end}"
            );
        }
    }

    // mono_test.go

    #[test]
    fn test_has_monotonic_clock() {
        go_setup();
        let yes = |expr: &str, tt: &Time| {
            assert!(tt.mono() != 0, "{expr}: missing monotonic clock reading")
        };
        let no = |expr: &str, tt: &Time| {
            assert!(tt.mono() == 0, "{expr}: unexpected monotonic clock reading")
        };

        // (Go's <-After(1) and <-Tick(1) cases: gotime has no timers.)
        no(
            "Date(2009, 11, 23, 0, 0, 0, 0, UTC)",
            &date(2009, Month(11), 23, 0, 0, 0, 0, &utc()),
        );
        let tp = parse(UNIX_DATE, "Sat Mar  7 11:06:39 PST 2015").unwrap_or_default();
        no(r#"Parse(UnixDate, "Sat Mar  7 11:06:39 PST 2015")"#, &tp);
        no("Unix(1486057371, 0)", &unix(1486057371, 0));

        yes("Now()", &now());

        let tu = unix(1486057371, 0);
        let mut tm = tu.clone();
        tm.set_mono(123456);
        no("tu", &tu);
        yes("tm", &tm);

        no("tu.Add(1)", &tu.add(d(1)));
        no("tu.In(UTC)", &tu.in_location(&utc()));
        no("tu.AddDate(1, 1, 1)", &tu.add_date(1, 1, 1));
        no("tu.AddDate(0, 0, 0)", &tu.add_date(0, 0, 0));
        no("tu.Local()", &tu.local());
        no("tu.UTC()", &tu.utc());
        no("tu.Round(2)", &tu.round(d(2)));
        no("tu.Truncate(2)", &tu.truncate(d(2)));

        yes("tm.Add(1)", &tm.add(d(1)));
        no("tm.AddDate(1, 1, 1)", &tm.add_date(1, 1, 1));
        no("tm.AddDate(0, 0, 0)", &tm.add_date(0, 0, 0));
        no("tm.In(UTC)", &tm.in_location(&utc()));
        no("tm.Local()", &tm.local());
        no("tm.UTC()", &tm.utc());
        no("tm.Round(2)", &tm.round(d(2)));
        no("tm.Truncate(2)", &tm.truncate(d(2)));
    }

    #[test]
    fn test_monotonic_add() {
        go_setup();
        let mut tm = unix(1486057371, 123456);
        tm.set_mono(123456789012345);

        let t2 = tm.add(d(100_000_000));
        assert_eq!(
            t2.nanosecond(),
            100123456,
            "t2.Nanosecond() = {}, want 100123456",
            t2.nanosecond()
        );
        assert_eq!(
            t2.mono(),
            123456889012345,
            "t2.mono = {}, want 123456889012345",
            t2.mono()
        );

        let t3 = tm.add(d(-9_000_000_000_000_000_000)); // wall now out of range
        assert_eq!(
            t3.nanosecond(),
            123456,
            "t3.Nanosecond() = {}, want 123456",
            t3.nanosecond()
        );
        assert_eq!(
            t3.mono(),
            0,
            "t3.mono = {}, want 0 (wall time out of range for monotonic reading)",
            t3.mono()
        );

        let t4 = tm.add(d(9_000_000_000_000_000_000)); // wall now out of range
        assert_eq!(
            t4.nanosecond(),
            123456,
            "t4.Nanosecond() = {}, want 123456",
            t4.nanosecond()
        );
        assert_eq!(
            t4.mono(),
            0,
            "t4.mono = {}, want 0 (wall time out of range for monotonic reading)",
            t4.mono()
        );

        let tn = now();
        let tn1 = tn.add(HOUR);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let dur = until(&tn1);
        assert!(
            dur.0 >= 59 * MINUTE.0,
            "Until(Now().Add(1*Hour)) = {dur}, wanted at least 59m"
        );
        let now = now();
        assert!(
            !now.after(&tn1),
            "Now().After(Now().Add(1*Hour)) = true, want false"
        );
        assert!(
            tn1.after(&now),
            "Now().Add(1*Hour).After(now) = false, want true"
        );
        assert!(
            !tn1.before(&now),
            "Now().Add(1*Hour).Before(Now()) = true, want false"
        );
        assert!(
            now.before(&tn1),
            "Now().Before(Now().Add(1*Hour)) = false, want true"
        );
        assert_eq!(now.compare(&tn1), -1, "Now().Compare(Now().Add(1*Hour))");
        assert_eq!(tn1.compare(&now), 1, "Now().Add(1*Hour).Compare(Now())");
    }

    #[test]
    fn test_monotonic_sub() {
        go_setup();
        let mut t1 = unix(1483228799, 995_000_000);
        t1.set_mono(123456789012345);

        let mut t2 = unix(1483228799, 5_000_000);
        t2.set_mono(123456789012345 + 10_000_000);

        let mut t3 = unix(1483228799, 995_000_000);
        t3.set_mono(123456789012345 + 1_000_000_000);

        let t1w = t1.add_date(0, 0, 0);
        assert_eq!(
            t1w.mono(),
            0,
            "AddDate didn't strip monotonic clock reading"
        );
        let t2w = t2.add_date(0, 0, 0);
        assert_eq!(
            t2w.mono(),
            0,
            "AddDate didn't strip monotonic clock reading"
        );
        let t3w = t3.add_date(0, 0, 0);
        assert_eq!(
            t3w.mono(),
            0,
            "AddDate didn't strip monotonic clock reading"
        );

        let ms = MILLISECOND.0;
        #[expect(clippy::too_many_arguments, reason = "mirrors Go's closure")]
        fn sub(
            txs: &str,
            tys: &str,
            tx: &Time,
            txw: &Time,
            ty: &Time,
            tyw: &Time,
            dur: Duration,
            dw: Duration,
        ) {
            let check = |expr: String, d: Duration, want: Duration| {
                assert_eq!(d, want, "{expr} = {d}, want {want}")
            };
            check(format!("{txs}.Sub({tys})"), tx.sub(ty), dur);
            check(format!("{txs}w.Sub({tys})"), txw.sub(ty), dw);
            check(format!("{txs}.Sub({tys}w)"), tx.sub(tyw), dw);
            check(format!("{txs}w.Sub({tys}w)"), txw.sub(tyw), dw);
        }
        sub("t1", "t1", &t1, &t1w, &t1, &t1w, d(0), d(0));
        sub("t1", "t2", &t1, &t1w, &t2, &t2w, d(-10 * ms), d(990 * ms));
        sub("t1", "t3", &t1, &t1w, &t3, &t3w, d(-1000 * ms), d(0));

        sub("t2", "t1", &t2, &t2w, &t1, &t1w, d(10 * ms), d(-990 * ms));
        sub("t2", "t2", &t2, &t2w, &t2, &t2w, d(0), d(0));
        sub("t2", "t3", &t2, &t2w, &t3, &t3w, d(-990 * ms), d(-990 * ms));

        sub("t3", "t1", &t3, &t3w, &t1, &t1w, d(1000 * ms), d(0));
        sub("t3", "t2", &t3, &t3w, &t2, &t2w, d(990 * ms), d(990 * ms));
        sub("t3", "t3", &t3, &t3w, &t3, &t3w, d(0), d(0));

        #[expect(clippy::too_many_arguments, reason = "mirrors Go's closure")]
        fn cmp(
            txs: &str,
            tys: &str,
            tx: &Time,
            txw: &Time,
            ty: &Time,
            tyw: &Time,
            c: i64,
            cw: i64,
        ) {
            fn check<T: PartialEq + std::fmt::Debug>(expr: String, b: T, want: T) {
                assert_eq!(b, want, "{expr} = {b:?}, want {want:?}");
            }
            check(format!("{txs}.After({tys})"), tx.after(ty), c > 0);
            check(format!("{txs}w.After({tys})"), txw.after(ty), cw > 0);
            check(format!("{txs}.After({tys}w)"), tx.after(tyw), cw > 0);
            check(format!("{txs}w.After({tys}w)"), txw.after(tyw), cw > 0);

            check(format!("{txs}.Before({tys})"), tx.before(ty), c < 0);
            check(format!("{txs}w.Before({tys})"), txw.before(ty), cw < 0);
            check(format!("{txs}.Before({tys}w)"), tx.before(tyw), cw < 0);
            check(format!("{txs}w.Before({tys}w)"), txw.before(tyw), cw < 0);

            check(format!("{txs}.Equal({tys})"), tx.equal(ty), c == 0);
            check(format!("{txs}w.Equal({tys})"), txw.equal(ty), cw == 0);
            check(format!("{txs}.Equal({tys}w)"), tx.equal(tyw), cw == 0);
            check(format!("{txs}w.Equal({tys}w)"), txw.equal(tyw), cw == 0);

            check(format!("{txs}.Compare({tys})"), tx.compare(ty), c);
            check(format!("{txs}w.Compare({tys})"), txw.compare(ty), cw);
            check(format!("{txs}.Compare({tys}w)"), tx.compare(tyw), cw);
            check(format!("{txs}w.Compare({tys}w)"), txw.compare(tyw), cw);
        }

        cmp("t1", "t1", &t1, &t1w, &t1, &t1w, 0, 0);
        cmp("t1", "t2", &t1, &t1w, &t2, &t2w, -1, 1);
        cmp("t1", "t3", &t1, &t1w, &t3, &t3w, -1, 0);

        cmp("t2", "t1", &t2, &t2w, &t1, &t1w, 1, -1);
        cmp("t2", "t2", &t2, &t2w, &t2, &t2w, 0, 0);
        cmp("t2", "t3", &t2, &t2w, &t3, &t3w, -1, -1);

        cmp("t3", "t1", &t3, &t3w, &t1, &t1w, 1, 0);
        cmp("t3", "t2", &t3, &t3w, &t2, &t2w, 1, 1);
        cmp("t3", "t3", &t3, &t3w, &t3, &t3w, 0, 0);
    }

    #[test]
    fn test_monotonic_overflow() {
        go_setup();
        let s = SECOND.0;
        let mut t1 = now().add(d(-30 * s));
        let mut dur = until(&t1);
        assert!(
            !(dur.0 < -35 * s || -30 * s < dur.0),
            "Until(Now().Add(-30s)) = {dur}, want roughly -30s (-35s to -30s)"
        );

        t1 = now().add(d(30 * s));
        dur = until(&t1);
        assert!(
            !(dur.0 < 25 * s || 30 * s < dur.0),
            "Until(Now().Add(-30s)) = {dur}, want roughly 30s (25s to 30s)"
        );

        let mut t0 = now();
        t1 = t0.add(Duration::MAX);
        assert_eq!(
            t1.mono(),
            0,
            "Now().Add(maxDuration) has monotonic clock reading ({} => {} {} {})",
            t0.string(),
            t1.string(),
            t0.unix(),
            t1.unix()
        );
        let mut t2 = t1.add(Duration(-Duration::MAX.0));
        dur = since(&t2);
        assert!(
            !(dur.0 < -10 * s || 10 * s < dur.0),
            "Since(Now().Add(max).Add(-max)) = {dur}, want [-10s, 10s]"
        );

        t0 = now();
        t1 = t0.add(HOUR);
        std::thread::sleep(std::time::Duration::from_millis(100));
        t2 = now().add(d(-5 * s));
        assert!(
            t1.after(&t2),
            "Now().Add(1*Hour).After(Now().Add(-5*Second)) = false, want true\nt1={t1}\nt2={t2}"
        );
        assert!(
            !t2.after(&t1),
            "Now().Add(-5*Second).After(Now().Add(1*Hour)) = true, want false\nt1={t1}\nt2={t2}"
        );
        assert!(
            !t1.before(&t2),
            "Now().Add(1*Hour).Before(Now().Add(-5*Second)) = true, want false\nt1={t1}\nt2={t2}"
        );
        assert!(
            t2.before(&t1),
            "Now().Add(-5*Second).Before(Now().Add(1*Hour)) = false, want true\nt1={t1}\nt2={t2}"
        );
        assert_eq!(
            t1.compare(&t2),
            1,
            "Now().Add(1*Hour).Compare(Now().Add(-5*Second))\nt1={t1}\nt2={t2}"
        );
        assert_eq!(
            t2.compare(&t1),
            -1,
            "Now().Add(-5*Second).Before(Now().Add(1*Hour))\nt1={t1}\nt2={t2}"
        );
    }

    #[test]
    fn test_monotonic_string() {
        go_setup();
        let monotonic_string_tests = [
            (0, "m=+0.000000000"),
            (123456789, "m=+0.123456789"),
            (-123456789, "m=-0.123456789"),
            (123456789000, "m=+123.456789000"),
            (-123456789000, "m=-123.456789000"),
            (9_000_000_000_000_000_000, "m=+9000000000.000000000"),
            (-9_000_000_000_000_000_000, "m=-9000000000.000000000"),
            (i64::MIN, "m=-9223372036.854775808"),
        ];

        let t1 = now();
        eprintln!("Now() = {t1}");

        for (mono, want) in monotonic_string_tests {
            let mut t1 = now();
            t1.set_mono(mono);
            let s = t1.string();
            let got = &s[s.rfind(' ').map_or(0, |i| i + 1)..];
            assert_eq!(got, want, "with mono={mono}: got {got:?}; want {want:?}");
        }
    }
}
