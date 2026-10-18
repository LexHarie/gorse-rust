//! Ports of dateparse's own tests (`parseany_test.go` @
//! v0.0.0-20210429162001-6b43995a97de, the version add21b5's go.mod pins),
//! with the same cases and assertions, and of the cache `Time` round trip in
//! `storage/cache/database_test.go` @ add21b5.
//!
//! Go runs these tests one after another in one process, so `time.Local`
//! carries over from one test to the next. Each port sets the `time.Local`
//! its Go test sees, holding the shared lock. Zones come from the stand-in
//! GOROOT zip (`common::go_setup`), where Go's tests read the machine's.

mod common;

mod dateparse {
    use std::sync::MutexGuard;

    use gorse_common::dateparse::{
        Error, ParserOption, internal, must_parse, parse_any, parse_format, parse_in, parse_local,
        parse_strict,
    };
    use gorse_common::gotime::{self, Location, Month, RFC3339, Time};

    use crate::common::{go_setup, lock};

    /// Holds the lock on the process-wide state and sets up Go's test
    /// environment (zones from the GOROOT zip).
    fn setup() -> MutexGuard<'static, ()> {
        let guard = lock();
        go_setup();
        guard
    }

    /// Go's `time.LoadLocation`.
    fn load_location(name: &str) -> Location {
        gotime::load_location(name)
            .unwrap_or_else(|e| panic!("Expected to load location {name:?} but got {e}"))
    }

    /// `fmt.Sprintf("%v", ts.In(time.UTC))`.
    fn utc_string(ts: &Time) -> String {
        ts.in_location(&gotime::utc()).string()
    }

    /// The `(time.Time, error)` pair Go returns: the zero Time with an error.
    fn go_result(r: Result<Time, Error>) -> (Time, Option<Error>) {
        match r {
            Ok(t) => (t, None),
            Err(e) => (Time::default(), Some(e)),
        }
    }

    /// Go's `testDidPanic`: whether `MustParse` panics.
    fn test_did_panic(datestr: &str) -> bool {
        std::panic::catch_unwind(|| must_parse(datestr, &[])).is_err()
    }

    #[derive(Clone, Copy)]
    struct DateTest {
        input: &'static str,
        out: &'static str,
        loc: &'static str,
        err: bool,
    }

    /// The zero `dateTest`, for `..T` in the tables.
    const T: DateTest = DateTest {
        input: "",
        out: "",
        loc: "",
        err: false,
    };

    #[test]
    fn test_one() {
        let _guard = setup();
        gotime::set_local(gotime::utc());
        let ts = must_parse("2020-07-20+08:00", &[]);
        assert_eq!("2020-07-19 16:00:00 +0000 UTC", utc_string(&ts));
    }

    #[rustfmt::skip]
    static TEST_INPUTS: &[DateTest] = &[
        DateTest { input: "oct 7, 1970", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "oct 7, '70", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "Oct 7, '70", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "Oct. 7, '70", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "oct. 7, '70", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "oct. 7, 1970", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "Sept. 7, '70", out: "1970-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "sept. 7, 1970", out: "1970-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "Feb 8, 2009 5:57:51 AM", out: "2009-02-08 05:57:51 +0000 UTC", ..T },
        DateTest { input: "May 8, 2009 5:57:51 PM", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "May 8, 2009 5:57:1 PM", out: "2009-05-08 17:57:01 +0000 UTC", ..T },
        DateTest { input: "May 8, 2009 5:7:51 PM", out: "2009-05-08 17:07:51 +0000 UTC", ..T },
        DateTest { input: "May 8, 2009, 5:7:51 PM", out: "2009-05-08 17:07:51 +0000 UTC", ..T },
        DateTest { input: "7 oct 70", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "7 oct 1970", out: "1970-10-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "7 May 1970", out: "1970-05-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "7 Sep 1970", out: "1970-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "7 June 1970", out: "1970-06-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "7 September 1970", out: "1970-09-07 00:00:00 +0000 UTC", ..T },
        //   ANSIC       = "Mon Jan _2 15:04:05 2006"
        DateTest { input: "Mon Jan  2 15:04:05 2006", out: "2006-01-02 15:04:05 +0000 UTC", ..T },
        DateTest { input: "Thu May 8 17:57:51 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "Thu May  8 17:57:51 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        //   ANSIC_GLIBC = "Mon 02 Jan 2006 03:04:05 PM UTC"
        DateTest { input: "Mon 02 Jan 2006 03:04:05 PM UTC", out: "2006-01-02 15:04:05 +0000 UTC", ..T },
        DateTest { input: "Mon 30 Sep 2018 09:09:09 PM UTC", out: "2018-09-30 21:09:09 +0000 UTC", ..T },
        // RubyDate    = "Mon Jan 02 15:04:05 -0700 2006"
        DateTest { input: "Mon Jan 02 15:04:05 -0700 2006", out: "2006-01-02 22:04:05 +0000 UTC", ..T },
        DateTest { input: "Thu May 08 11:57:51 -0700 2009", out: "2009-05-08 18:57:51 +0000 UTC", ..T },
        //   UnixDate    = "Mon Jan _2 15:04:05 MST 2006"
        DateTest { input: "Mon Jan  2 15:04:05 MST 2006", out: "2006-01-02 15:04:05 +0000 UTC", ..T },
        DateTest { input: "Thu May  8 17:57:51 MST 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "Thu May  8 17:57:51 PST 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "Thu May 08 17:57:51 PST 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "Thu May 08 17:57:51 CEST 2009", out: "2009-05-08 17:57:51 +0000 UTC", ..T },
        DateTest { input: "Thu May 08 05:05:07 PST 2009", out: "2009-05-08 05:05:07 +0000 UTC", ..T },
        DateTest { input: "Thu May 08 5:5:7 PST 2009", out: "2009-05-08 05:05:07 +0000 UTC", ..T },
        // Day Month dd time
        DateTest { input: "Mon Aug 10 15:44:11 UTC+0000 2015", out: "2015-08-10 15:44:11 +0000 UTC", ..T },
        DateTest { input: "Mon Aug 10 15:44:11 PST-0700 2015", out: "2015-08-10 22:44:11 +0000 UTC", ..T },
        DateTest { input: "Mon Aug 10 15:44:11 CEST+0200 2015", out: "2015-08-10 13:44:11 +0000 UTC", ..T },
        DateTest { input: "Mon Aug 1 15:44:11 CEST+0200 2015", out: "2015-08-01 13:44:11 +0000 UTC", ..T },
        DateTest { input: "Mon Aug 1 5:44:11 CEST+0200 2015", out: "2015-08-01 03:44:11 +0000 UTC", ..T },
        // ??
        DateTest { input: "Fri Jul 03 2015 18:04:07 GMT+0100 (GMT Daylight Time)", out: "2015-07-03 17:04:07 +0000 UTC", ..T },
        DateTest { input: "Fri Jul 3 2015 06:04:07 GMT+0100 (GMT Daylight Time)", out: "2015-07-03 05:04:07 +0000 UTC", ..T },
        DateTest { input: "Fri Jul 3 2015 06:04:07 PST-0700 (Pacific Daylight Time)", out: "2015-07-03 13:04:07 +0000 UTC", ..T },
        // Month dd, yyyy at time
        DateTest { input: "September 17, 2012 at 5:00pm UTC-05", out: "2012-09-17 17:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17, 2012 at 10:09am PST-08", out: "2012-09-17 18:09:00 +0000 UTC", ..T },
        DateTest { input: "September 17, 2012, 10:10:09", out: "2012-09-17 10:10:09 +0000 UTC", ..T },
        DateTest { input: "May 17, 2012 at 10:09am PST-08", out: "2012-05-17 18:09:00 +0000 UTC", ..T },
        DateTest { input: "May 17, 2012 AT 10:09am PST-08", out: "2012-05-17 18:09:00 +0000 UTC", ..T },
        // Month dd, yyyy time
        DateTest { input: "September 17, 2012 5:00pm UTC-05", out: "2012-09-17 17:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17, 2012 10:09am PST-08", out: "2012-09-17 18:09:00 +0000 UTC", ..T },
        DateTest { input: "September 17, 2012 09:01:00", out: "2012-09-17 09:01:00 +0000 UTC", ..T },
        // Month dd yyyy time
        DateTest { input: "September 17 2012 5:00pm UTC-05", out: "2012-09-17 17:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17 2012 5:00pm UTC-0500", out: "2012-09-17 17:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17 2012 10:09am PST-08", out: "2012-09-17 18:09:00 +0000 UTC", ..T },
        DateTest { input: "September 17 2012 5:00PM UTC-05", out: "2012-09-17 17:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17 2012 10:09AM PST-08", out: "2012-09-17 18:09:00 +0000 UTC", ..T },
        DateTest { input: "September 17 2012 09:01:00", out: "2012-09-17 09:01:00 +0000 UTC", ..T },
        DateTest { input: "May 17, 2012 10:10:09", out: "2012-05-17 10:10:09 +0000 UTC", ..T },
        // Month dd, yyyy
        DateTest { input: "September 17, 2012", out: "2012-09-17 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 7, 2012", out: "2012-05-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 7, 2012", out: "2012-06-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 7 2012", out: "2012-06-07 00:00:00 +0000 UTC", ..T },
        // Month dd[th,nd,st,rd] yyyy
        DateTest { input: "September 17th, 2012", out: "2012-09-17 00:00:00 +0000 UTC", ..T },
        DateTest { input: "September 17th 2012", out: "2012-09-17 00:00:00 +0000 UTC", ..T },
        DateTest { input: "September 7th, 2012", out: "2012-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "September 7th 2012", out: "2012-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "September 7tH 2012", out: "2012-09-07 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 1st 2012", out: "2012-05-01 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 1st, 2012", out: "2012-05-01 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 21st 2012", out: "2012-05-21 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 21st, 2012", out: "2012-05-21 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 23rd 2012", out: "2012-05-23 00:00:00 +0000 UTC", ..T },
        DateTest { input: "May 23rd, 2012", out: "2012-05-23 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 2nd, 2012", out: "2012-06-02 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 2nd 2012", out: "2012-06-02 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 22nd, 2012", out: "2012-06-22 00:00:00 +0000 UTC", ..T },
        DateTest { input: "June 22nd 2012", out: "2012-06-22 00:00:00 +0000 UTC", ..T },
        // RFC1123     = "Mon, 02 Jan 2006 15:04:05 MST"
        DateTest { input: "Fri, 03 Jul 2015 08:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        //{in: "Fri, 03 Jul 2015 08:08:08 CET", out: "2015-07-03 08:08:08 +0000 UTC"},
        DateTest { input: "Fri, 03 Jul 2015 08:08:08 PST", out: "2015-07-03 16:08:08 +0000 UTC", loc: "America/Los_Angeles", ..T },
        DateTest { input: "Fri, 03 Jul 2015 08:08:08 PST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 3 Jul 2015 08:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 03 Jul 2015 8:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 03 Jul 2015 8:8:8 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        // ?
        DateTest { input: "Thu, 03 Jul 2017 08:08:04 +0100", out: "2017-07-03 07:08:04 +0000 UTC", ..T },
        DateTest { input: "Thu, 03 Jul 2017 08:08:04 -0100", out: "2017-07-03 09:08:04 +0000 UTC", ..T },
        DateTest { input: "Thu, 3 Jul 2017 08:08:04 +0100", out: "2017-07-03 07:08:04 +0000 UTC", ..T },
        DateTest { input: "Thu, 03 Jul 2017 8:08:04 +0100", out: "2017-07-03 07:08:04 +0000 UTC", ..T },
        DateTest { input: "Thu, 03 Jul 2017 8:8:4 +0100", out: "2017-07-03 07:08:04 +0000 UTC", ..T },
        //
        DateTest { input: "Tue, 11 Jul 2017 04:08:03 +0200 (CEST)", out: "2017-07-11 02:08:03 +0000 UTC", ..T },
        DateTest { input: "Tue, 5 Jul 2017 04:08:03 -0700 (CEST)", out: "2017-07-05 11:08:03 +0000 UTC", ..T },
        DateTest { input: "Tue, 11 Jul 2017 04:08:03 +0200 (CEST)", out: "2017-07-11 02:08:03 +0000 UTC", loc: "Europe/Berlin", ..T },
        // day, dd-Mon-yy hh:mm:zz TZ
        DateTest { input: "Fri, 03-Jul-15 08:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 03-Jul-15 08:08:08 PST", out: "2015-07-03 16:08:08 +0000 UTC", loc: "America/Los_Angeles", ..T },
        DateTest { input: "Fri, 03-Jul 2015 08:08:08 PST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 3-Jul-15 08:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 03-Jul-15 8:08:08 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        DateTest { input: "Fri, 03-Jul-15 8:8:8 MST", out: "2015-07-03 08:08:08 +0000 UTC", ..T },
        // day, dd-Mon-yy hh:mm:zz TZ (text) https://github.com/araddon/dateparse/issues/116
        DateTest { input: "Sun, 3 Jan 2021 00:12:23 +0800 (GMT+08:00)", out: "2021-01-02 16:12:23 +0000 UTC", ..T },
        // RFC850    = "Monday, 02-Jan-06 15:04:05 MST"
        DateTest { input: "Wednesday, 07-May-09 08:00:43 MST", out: "2009-05-07 08:00:43 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 28-Feb-18 09:01:00 MST", out: "2018-02-28 09:01:00 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 28-Feb-18 09:01:00 MST", out: "2018-02-28 16:01:00 +0000 UTC", loc: "America/Denver", ..T },
        // with offset then with variations on non-zero filled stuff
        DateTest { input: "Monday, 02 Jan 2006 15:04:05 +0100", out: "2006-01-02 14:04:05 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 28 Feb 2018 09:01:00 -0300", out: "2018-02-28 12:01:00 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 2 Feb 2018 09:01:00 -0300", out: "2018-02-02 12:01:00 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 2 Feb 2018 9:01:00 -0300", out: "2018-02-02 12:01:00 +0000 UTC", ..T },
        DateTest { input: "Wednesday, 2 Feb 2018 09:1:00 -0300", out: "2018-02-02 12:01:00 +0000 UTC", ..T },
        //  dd mon yyyy  12 Feb 2006, 19:17:08
        DateTest { input: "07 Feb 2004, 09:07", out: "2004-02-07 09:07:00 +0000 UTC", ..T },
        DateTest { input: "07 Feb 2004, 09:07:07", out: "2004-02-07 09:07:07 +0000 UTC", ..T },
        DateTest { input: "7 Feb 2004, 09:07:07", out: "2004-02-07 09:07:07 +0000 UTC", ..T },
        DateTest { input: "07 Feb 2004, 9:7:7", out: "2004-02-07 09:07:07 +0000 UTC", ..T },
        // dd Mon yyyy hh:mm:ss
        DateTest { input: "07 Feb 2004 09:07:08", out: "2004-02-07 09:07:08 +0000 UTC", ..T },
        DateTest { input: "07 Feb 2004 09:07", out: "2004-02-07 09:07:00 +0000 UTC", ..T },
        DateTest { input: "7 Feb 2004 9:7:8", out: "2004-02-07 09:07:08 +0000 UTC", ..T },
        DateTest { input: "07 Feb 2004 09:07:08.123", out: "2004-02-07 09:07:08.123 +0000 UTC", ..T },
        //  dd-mon-yyyy  12 Feb 2006, 19:17:08 GMT
        DateTest { input: "07 Feb 2004, 09:07:07 GMT", out: "2004-02-07 09:07:07 +0000 UTC", ..T },
        //  dd-mon-yyyy  12 Feb 2006, 19:17:08 +0100
        DateTest { input: "07 Feb 2004, 09:07:07 +0100", out: "2004-02-07 08:07:07 +0000 UTC", ..T },
        //  dd-mon-yyyy   12-Feb-2006 19:17:08
        DateTest { input: "07-Feb-2004 09:07:07 +0100", out: "2004-02-07 08:07:07 +0000 UTC", ..T },
        //  dd-mon-yy   12-Feb-2006 19:17:08
        DateTest { input: "07-Feb-04 09:07:07 +0100", out: "2004-02-07 08:07:07 +0000 UTC", ..T },
        // yyyy-mon-dd    2013-Feb-03
        DateTest { input: "2013-Feb-03", out: "2013-02-03 00:00:00 +0000 UTC", ..T },
        // 03 February 2013
        DateTest { input: "03 February 2013", out: "2013-02-03 00:00:00 +0000 UTC", ..T },
        DateTest { input: "3 February 2013", out: "2013-02-03 00:00:00 +0000 UTC", ..T },
        // Chinese 2014年04月18日
        DateTest { input: "2014年04月08日", out: "2014-04-08 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2014年04月08日 19:17:22", out: "2014-04-08 19:17:22 +0000 UTC", ..T },
        //  mm/dd/yyyy
        DateTest { input: "03/31/2014", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        DateTest { input: "3/31/2014", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        DateTest { input: "3/5/2014", out: "2014-03-05 00:00:00 +0000 UTC", ..T },
        //  mm/dd/yy
        DateTest { input: "08/08/71", out: "1971-08-08 00:00:00 +0000 UTC", ..T },
        DateTest { input: "8/8/71", out: "1971-08-08 00:00:00 +0000 UTC", ..T },
        //  mm/dd/yy hh:mm:ss
        DateTest { input: "04/02/2014 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "4/2/2014 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 4:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 4:8:9", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 4:8", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09.123", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09.12312", out: "2014-04-02 04:08:09.12312 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09.123123", out: "2014-04-02 04:08:09.123123 +0000 UTC", ..T },
        //  mm:dd:yy hh:mm:ss
        DateTest { input: "04:02:2014 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "4:2:2014 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 4:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 4:8:9", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 4:8", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 04:08:09.123", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 04:08:09.12312", out: "2014-04-02 04:08:09.12312 +0000 UTC", ..T },
        DateTest { input: "04:02:2014 04:08:09.123123", out: "2014-04-02 04:08:09.123123 +0000 UTC", ..T },
        //  mm/dd/yy hh:mm:ss AM
        DateTest { input: "04/02/2014 04:08:09 AM", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09 PM", out: "2014-04-02 16:08:09 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08 AM", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08 PM", out: "2014-04-02 16:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 4:8 AM", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 4:8 PM", out: "2014-04-02 16:08:00 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09.123 AM", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "04/02/2014 04:08:09.123 PM", out: "2014-04-02 16:08:09.123 +0000 UTC", ..T },
        //   yyyy/mm/dd
        DateTest { input: "2014/04/02", out: "2014-04-02 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2014/03/31", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2014/4/2", out: "2014-04-02 00:00:00 +0000 UTC", ..T },
        //   yyyy/mm/dd hh:mm:ss AM
        DateTest { input: "2014/04/02 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014/03/31 04:08", out: "2014-03-31 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014/4/2 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 4:8", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/03/31 04:08:09", out: "2014-03-31 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/4/2 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09.123", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09.123123", out: "2014-04-02 04:08:09.123123 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09 AM", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/03/31 04:08:09 AM", out: "2014-03-31 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/4/2 04:08:09 AM", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09.123 AM", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "2014/04/02 04:08:09.123 PM", out: "2014-04-02 16:08:09.123 +0000 UTC", ..T },
        // dd/mon/yyyy:hh:mm:ss tz  nginx-log?    https://github.com/araddon/dateparse/issues/118
        // 112.195.209.90 - - [20/Feb/2018:12:12:14 +0800] "GET / HTTP/1.1" 200 190 "-" "Mozilla/5.0 (Linux; Android 6.0; Nexus 5 Build/MRA58N) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Mobile Safari/537.36" "-"
        DateTest { input: "06/May/2008:08:11:17 -0700", out: "2008-05-06 15:11:17 +0000 UTC", ..T },
        DateTest { input: "30/May/2008:08:11:17 -0700", out: "2008-05-30 15:11:17 +0000 UTC", ..T },
        // dd/mon/yyy hh:mm:ss tz
        DateTest { input: "06/May/2008:08:11:17 -0700", out: "2008-05-06 15:11:17 +0000 UTC", ..T },
        DateTest { input: "30/May/2008:08:11:17 -0700", out: "2008-05-30 15:11:17 +0000 UTC", ..T },
        //   yyyy-mm-dd
        DateTest { input: "2014-04-02", out: "2014-04-02 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2014-03-31", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2014-4-2", out: "2014-04-02 00:00:00 +0000 UTC", ..T },
        //   yyyy-mm-dd-07:00
        DateTest { input: "2020-07-20+08:00", out: "2020-07-19 16:00:00 +0000 UTC", ..T },
        DateTest { input: "2020-07-20+0800", out: "2020-07-19 16:00:00 +0000 UTC", ..T },
        //   dd-mmm-yy
        DateTest { input: "28-Feb-02", out: "2002-02-28 00:00:00 +0000 UTC", ..T },
        DateTest { input: "15-Jan-18", out: "2018-01-15 00:00:00 +0000 UTC", ..T },
        DateTest { input: "15-Jan-2017", out: "2017-01-15 00:00:00 +0000 UTC", ..T },
        // yyyy-mm
        DateTest { input: "2014-04", out: "2014-04-01 00:00:00 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss AM
        DateTest { input: "2014-04-02 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014-03-31 04:08", out: "2014-03-31 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014-4-2 04:08", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 4:8", out: "2014-04-02 04:08:00 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-03-31 04:08:09", out: "2014-03-31 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-4-2 04:08:09", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09.123", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09.123123", out: "2014-04-02 04:08:09.123123 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09.12312312", out: "2014-04-02 04:08:09.12312312 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09 AM", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-03-31 04:08:09 AM", out: "2014-03-31 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 05:24:37 PM", out: "2014-04-26 17:24:37 +0000 UTC", ..T },
        DateTest { input: "2014-4-2 04:08:09 AM", out: "2014-04-02 04:08:09 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09.123 AM", out: "2014-04-02 04:08:09.123 +0000 UTC", ..T },
        DateTest { input: "2014-04-02 04:08:09.123 PM", out: "2014-04-02 16:08:09.123 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss,000
        DateTest { input: "2014-05-11 08:20:13,787", out: "2014-05-11 08:20:13.787 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss +0000
        DateTest { input: "2012-08-03 18:31:59 +0000", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:59 -0600", out: "2012-08-03 19:31:59 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 18:31:59.257000000 +0000", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 8:1:59.257000000 +0000", out: "2012-08-03 08:01:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-03 18:31:59.257000000 +0000", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-3 18:31:59.257000000 +0000", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.123456 +0000", out: "2014-04-26 17:24:37.123456 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.12 +0000", out: "2014-04-26 17:24:37.12 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.1 +0000", out: "2014-04-26 17:24:37.1 +0000 UTC", ..T },
        DateTest { input: "2014-05-11 08:20:13 +0000", out: "2014-05-11 08:20:13 +0000 UTC", ..T },
        DateTest { input: "2014-05-11 08:20:13 +0530", out: "2014-05-11 02:50:13 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss +0300 +03  ?? issue author said this is from golang?
        DateTest { input: "2018-06-29 19:09:57.77297118 +0300 +03", out: "2018-06-29 16:09:57.77297118 +0000 UTC", ..T },
        DateTest { input: "2018-06-29 19:09:57.77297118 +0300 +0300", out: "2018-06-29 16:09:57.77297118 +0000 UTC", ..T },
        DateTest { input: "2018-06-29 19:09:57 +0300 +03", out: "2018-06-29 16:09:57 +0000 UTC", ..T },
        DateTest { input: "2018-06-29 19:09:57 +0300 +0300", out: "2018-06-29 16:09:57 +0000 UTC", ..T },

        // 13:31:51.999 -07:00 MST
        //   yyyy-mm-dd hh:mm:ss +00:00
        DateTest { input: "2012-08-03 18:31:59 +00:00", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2014-05-01 08:02:13 +00:00", out: "2014-05-01 08:02:13 +0000 UTC", ..T },
        DateTest { input: "2014-5-01 08:02:13 +00:00", out: "2014-05-01 08:02:13 +0000 UTC", ..T },
        DateTest { input: "2014-05-1 08:02:13 +00:00", out: "2014-05-01 08:02:13 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:59 -06:00", out: "2012-08-03 19:31:59 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 18:31:59.257000000 +00:00", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 8:1:59.257000000 +00:00", out: "2012-08-03 08:01:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-03 18:31:59.257000000 +00:00", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-3 18:31:59.257000000 +00:00", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.123456 +00:00", out: "2014-04-26 17:24:37.123456 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.12 +00:00", out: "2014-04-26 17:24:37.12 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.1 +00:00", out: "2014-04-26 17:24:37.1 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss +0000 TZ
        // Golang Native Format
        DateTest { input: "2012-08-03 18:31:59 +0000 UTC", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:59 -0600 MST", out: "2012-08-03 19:31:59 +0000 UTC", loc: "America/Denver", ..T },
        DateTest { input: "2015-02-18 00:12:00 +0000 UTC", out: "2015-02-18 00:12:00 +0000 UTC", ..T },
        DateTest { input: "2015-02-18 00:12:00 +0000 GMT", out: "2015-02-18 00:12:00 +0000 UTC", ..T },
        DateTest { input: "2015-02-08 03:02:00 +0200 CEST", out: "2015-02-08 01:02:00 +0000 UTC", loc: "Europe/Berlin", ..T },
        DateTest { input: "2015-02-08 03:02:00 +0300 MSK", out: "2015-02-08 00:02:00 +0000 UTC", ..T },
        DateTest { input: "2015-2-08 03:02:00 +0300 MSK", out: "2015-02-08 00:02:00 +0000 UTC", ..T },
        DateTest { input: "2015-02-8 03:02:00 +0300 MSK", out: "2015-02-08 00:02:00 +0000 UTC", ..T },
        DateTest { input: "2015-2-8 03:02:00 +0300 MSK", out: "2015-02-08 00:02:00 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 18:31:59.257000000 +0000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 8:1:59.257000000 +0000 UTC", out: "2012-08-03 08:01:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-03 18:31:59.257000000 +0000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-3 18:31:59.257000000 +0000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.123456 +0000 UTC", out: "2014-04-26 17:24:37.123456 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.12 +0000 UTC", out: "2014-04-26 17:24:37.12 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.1 +0000 UTC", out: "2014-04-26 17:24:37.1 +0000 UTC", ..T },
        DateTest { input: "2015-02-08 03:02:00 +0200 CEST m=+0.000000001", out: "2015-02-08 01:02:00 +0000 UTC", loc: "Europe/Berlin", ..T },
        DateTest { input: "2015-02-08 03:02:00 +0300 MSK m=+0.000000001", out: "2015-02-08 00:02:00 +0000 UTC", ..T },
        DateTest { input: "2015-02-08 03:02:00.001 +0300 MSK m=+0.000000001", out: "2015-02-08 00:02:00.001 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss TZ
        DateTest { input: "2012-08-03 18:31:59 UTC", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2014-12-16 06:20:00 GMT", out: "2014-12-16 06:20:00 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:59 MST", out: "2012-08-03 20:31:59 +0000 UTC", loc: "America/Denver", ..T },
        DateTest { input: "2012-08-03 18:31:59.257000000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 8:1:59.257000000 UTC", out: "2012-08-03 08:01:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-03 18:31:59.257000000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-3 18:31:59.257000000 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.123456 UTC", out: "2014-04-26 17:24:37.123456 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.12 UTC", out: "2014-04-26 17:24:37.12 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.1 UTC", out: "2014-04-26 17:24:37.1 +0000 UTC", ..T },
        // This one is pretty special, it is TIMEZONE based but starts with P to emulate collions with PM
        DateTest { input: "2014-04-26 05:24:37 PST", out: "2014-04-26 05:24:37 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 05:24:37 PST", out: "2014-04-26 13:24:37 +0000 UTC", loc: "America/Los_Angeles", ..T },
        //   yyyy-mm-dd hh:mm:ss+00:00
        DateTest { input: "2012-08-03 18:31:59+00:00", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2017-07-19 03:21:51+00:00", out: "2017-07-19 03:21:51 +0000 UTC", ..T },
        //   yyyy:mm:dd hh:mm:ss+00:00
        DateTest { input: "2012:08:03 18:31:59+00:00", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        //   dd:mm:yyyy hh:mm:ss+00:00
        DateTest { input: "08:03:2012 18:31:59+00:00", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        //   yyyy-mm-dd hh:mm:ss.000+00:00 PST
        DateTest { input: "2012-08-03 18:31:59.000+00:00 PST", out: "2012-08-03 18:31:59 +0000 UTC", loc: "America/Los_Angeles", ..T },
        //   yyyy-mm-dd hh:mm:ss +00:00 TZ
        DateTest { input: "2012-08-03 18:31:59 +00:00 UTC", out: "2012-08-03 18:31:59 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:51 -07:00 MST", out: "2012-08-03 20:31:51 +0000 UTC", loc: "America/Denver", ..T },
        DateTest { input: "2012-08-03 18:31:59.257000000 +00:00 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-08-03 13:31:51.123 -08:00 PST", out: "2012-08-03 21:31:51.123 +0000 UTC", loc: "America/Los_Angeles", ..T },
        DateTest { input: "2012-08-03 13:31:51.123 +02:00 CEST", out: "2012-08-03 11:31:51.123 +0000 UTC", loc: "Europe/Berlin", ..T },
        DateTest { input: "2012-08-03 8:1:59.257000000 +00:00 UTC", out: "2012-08-03 08:01:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-03 18:31:59.257000000 +00:00 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2012-8-3 18:31:59.257000000 +00:00 UTC", out: "2012-08-03 18:31:59.257 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.123456 +00:00 UTC", out: "2014-04-26 17:24:37.123456 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.12 +00:00 UTC", out: "2014-04-26 17:24:37.12 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.1 +00:00 UTC", out: "2014-04-26 17:24:37.1 +0000 UTC", ..T },
        //   yyyy-mm-ddThh:mm:ss
        DateTest { input: "2009-08-12T22:15:09", out: "2009-08-12 22:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-08T02:08:08", out: "2009-08-08 02:08:08 +0000 UTC", ..T },
        DateTest { input: "2009-08-08T2:8:8", out: "2009-08-08 02:08:08 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.123", out: "2009-08-12 22:15:09.123 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.123456", out: "2009-08-12 22:15:09.123456 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.12", out: "2009-08-12 22:15:09.12 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.1", out: "2009-08-12 22:15:09.1 +0000 UTC", ..T },
        DateTest { input: "2014-04-26 17:24:37.3186369", out: "2014-04-26 17:24:37.3186369 +0000 UTC", ..T },
        //   yyyy-mm-ddThh:mm:ss-07:00
        DateTest { input: "2009-08-12T22:15:09-07:00", out: "2009-08-13 05:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09-03:00", out: "2009-08-13 01:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:9-07:00", out: "2009-08-13 05:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.123-07:00", out: "2009-08-13 05:15:09.123 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00+01:00", out: "2016-06-21 18:55:00 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00.799+01:00", out: "2016-06-21 18:55:00.799 +0000 UTC", ..T },
        //   yyyy-mm-ddThh:mm:ss-07   TZ truncated to 2 digits instead of 4
        DateTest { input: "2019-05-29T08:41-04", out: "2019-05-29 12:41:00 +0000 UTC", ..T },
        //   yyyy-mm-ddThh:mm:ss-0700
        DateTest { input: "2009-08-12T22:15:09-0700", out: "2009-08-13 05:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09-0300", out: "2009-08-13 01:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:9-0700", out: "2009-08-13 05:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.123-0700", out: "2009-08-13 05:15:09.123 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00+0100", out: "2016-06-21 18:55:00 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00.799+0100", out: "2016-06-21 18:55:00.799 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00+0100", out: "2016-06-21 18:55:00 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00-0700", out: "2016-06-22 02:55:00 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55:00.799+0100", out: "2016-06-21 18:55:00.799 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55+0100", out: "2016-06-21 18:55:00 +0000 UTC", ..T },
        DateTest { input: "2016-06-21T19:55+0130", out: "2016-06-21 18:25:00 +0000 UTC", ..T },
        //   yyyy-mm-ddThh:mm:ss:000+0000    - weird format with additional colon in front of milliseconds
        DateTest { input: "2012-08-17T18:31:59:257+0100", out: "2012-08-17 17:31:59.257 +0000 UTC", ..T },  // https://github.com/araddon/dateparse/issues/117

        //   yyyy-mm-ddThh:mm:ssZ
        DateTest { input: "2009-08-12T22:15Z", out: "2009-08-12 22:15:00 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09Z", out: "2009-08-12 22:15:09 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.99Z", out: "2009-08-12 22:15:09.99 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.9999Z", out: "2009-08-12 22:15:09.9999 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:09.99999999Z", out: "2009-08-12 22:15:09.99999999 +0000 UTC", ..T },
        DateTest { input: "2009-08-12T22:15:9.99999999Z", out: "2009-08-12 22:15:09.99999999 +0000 UTC", ..T },
        // yyyy.mm
        DateTest { input: "2014.05", out: "2014-05-01 00:00:00 +0000 UTC", ..T },
        DateTest { input: "2018.09.30", out: "2018-09-30 00:00:00 +0000 UTC", ..T },

        //   mm.dd.yyyy
        DateTest { input: "3.31.2014", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        DateTest { input: "3.3.2014", out: "2014-03-03 00:00:00 +0000 UTC", ..T },
        DateTest { input: "03.31.2014", out: "2014-03-31 00:00:00 +0000 UTC", ..T },
        //   mm.dd.yy
        DateTest { input: "08.21.71", out: "1971-08-21 00:00:00 +0000 UTC", ..T },
        //  yyyymmdd and similar
        DateTest { input: "2014", out: "2014-01-01 00:00:00 +0000 UTC", ..T },
        DateTest { input: "20140601", out: "2014-06-01 00:00:00 +0000 UTC", ..T },
        DateTest { input: "20140722105203", out: "2014-07-22 10:52:03 +0000 UTC", ..T },
        // yymmdd hh:mm:yy  mysql log  https://github.com/araddon/dateparse/issues/119
        // 080313 05:21:55 mysqld started
        // 080313 5:21:55 InnoDB: Started; log sequence number 0 43655
        DateTest { input: "171113 14:14:20", out: "2017-11-13 14:14:20 +0000 UTC", ..T },

        // all digits:  unix secs, ms etc
        DateTest { input: "1332151919", out: "2012-03-19 10:11:59 +0000 UTC", ..T },
        DateTest { input: "1332151919", out: "2012-03-19 10:11:59 +0000 UTC", loc: "America/Denver", ..T },
        DateTest { input: "1384216367111", out: "2013-11-12 00:32:47.111 +0000 UTC", ..T },
        DateTest { input: "1384216367111222", out: "2013-11-12 00:32:47.111222 +0000 UTC", ..T },
        DateTest { input: "1384216367111222333", out: "2013-11-12 00:32:47.111222333 +0000 UTC", ..T },
    ];

    #[test]
    fn test_parse() {
        let _guard = setup();

        // Lets ensure we are operating on UTC
        gotime::set_local(gotime::utc());

        let zero_time = Time::default().unix();
        let (ts, err) = go_result(parse_any("INVALID", &[]));
        assert_eq!(zero_time, ts.unix());
        assert!(err.is_some());

        assert!(test_did_panic("NOT GONNA HAPPEN"));
        // https://github.com/golang/go/issues/5294
        let (_, err) = go_result(parse_any(RFC3339, &[]));
        assert!(err.is_some());

        for th in TEST_INPUTS {
            if !th.loc.is_empty() {
                let loc = load_location(th.loc);
                let ts = parse_in(th.input, &loc, &[])
                    .unwrap_or_else(|e| panic!("expected to parse {:?} but got {e}", th.input));
                let got = utc_string(&ts);
                assert_eq!(
                    th.out, got,
                    "Expected {:?} but got {:?} from {:?}",
                    th.out, got, th.input
                );
            } else {
                let ts = must_parse(th.input, &[]);
                let got = utc_string(&ts);
                assert_eq!(
                    th.out, got,
                    "Expected {:?} but got {:?} from {:?}",
                    th.out, got, th.input
                );
            }
        }

        // some errors

        assert!(test_did_panic(r#"{"ts":"now"}"#));

        // too many digits
        let (_, err) = go_result(parse_any("138421636711122233311111", &[]));
        assert!(err.is_some());

        let (_, err) = go_result(parse_any("-1314", &[]));
        assert!(err.is_some());

        // month 13 doesn't exist so error
        let (_, err) = go_result(parse_any("2014-13-13 08:20:13,787", &[]));
        assert!(err.is_some());
    }

    #[test]
    fn test_p_struct() {
        let _guard = setup();
        // time.Local as TestParse leaves it.
        gotime::set_local(gotime::utc());

        let denver_loc = load_location("America/Denver");

        let mut p = internal::new_parser("08.21.71", Some(&denver_loc), &[]);

        p.set_month();
        assert_eq!(0, p.moi());
        p.set_day();
        assert_eq!(0, p.dayi());
        p.set(-1, "not");
        p.set(15, "not");
        assert_eq!(b"08.21.71", p.datestr());
        assert_eq!(b"08.21.71", p.format());
        assert!(!p.ds().is_empty());
        assert!(!p.ts().is_empty());
    }

    #[rustfmt::skip]
    static TEST_PARSE_ERRORS: &[DateTest] = &[
        DateTest { input: "3", err: true, ..T },
        DateTest { input: "{\"hello\"}", err: true, ..T },
        DateTest { input: "2009-15-12T22:15Z", err: true, ..T },
        DateTest { input: "5,000-9,999", err: true, ..T },
        DateTest { input: "xyzq-baad", ..T },
        DateTest { input: "oct.-7-1970", err: true, ..T },
        DateTest { input: "septe. 7, 1970", err: true, ..T },
        DateTest { input: "SeptemberRR 7th, 1970", err: true, ..T },
        DateTest { input: "29-06-2016", err: true, ..T },
        // this is just testing the empty space up front
        DateTest { input: " 2018-01-02 17:08:09 -07:00", err: true, ..T },
    ];

    #[test]
    fn test_parse_errors() {
        let _guard = setup();
        // time.Local as TestParse leaves it.
        gotime::set_local(gotime::utc());

        for th in TEST_PARSE_ERRORS {
            let (v, err) = go_result(parse_any(th.input, &[]));
            assert!(err.is_some(), "{v} for {}", th.input);

            let (v, err) = go_result(parse_any(
                th.input,
                &[ParserOption::RetryAmbiguousDateWithSwap(true)],
            ));
            assert!(err.is_some(), "{v} for {}", th.input);
        }
    }

    #[test]
    fn test_parse_layout() {
        let _guard = setup();

        gotime::set_local(gotime::utc());
        // These tests are verifying that the layout returned by ParseFormat
        // are correct.   Above tests correct parsing, this tests correct
        // re-usable formatting string
        #[rustfmt::skip]
        let test_parse_format: &[DateTest] = &[
            // errors
            DateTest { input: "3", err: true, ..T },
            DateTest { input: "{\"hello\"}", err: true, ..T },
            DateTest { input: "2009-15-12T22:15Z", err: true, ..T },
            DateTest { input: "5,000-9,999", err: true, ..T },
            // This 3 digit TZ offset (should be 2 or 4?  is 3 a thing?)
            DateTest { input: "2019-05-29T08:41-047", err: true, ..T },
            //
            DateTest { input: "06/May/2008 15:04:05 -0700", out: "02/Jan/2006 15:04:05 -0700", ..T },
            DateTest { input: "06/May/2008:15:04:05 -0700", out: "02/Jan/2006:15:04:05 -0700", ..T },
            DateTest { input: "14 May 2019 19:11:40.164", out: "02 Jan 2006 15:04:05.000", ..T },
            DateTest { input: "171113 14:14:20", out: "060102 15:04:05", ..T },

            DateTest { input: "oct 7, 1970", out: "Jan 2, 2006", ..T },
            DateTest { input: "sept. 7, 1970", out: "Jan. 2, 2006", ..T },
            DateTest { input: "May 05, 2015, 05:05:07", out: "Jan 02, 2006, 15:04:05", ..T },
            // 03 February 2013
            DateTest { input: "03 February 2013", out: "02 January 2006", ..T },
            // 13:31:51.999 -07:00 MST
            //   yyyy-mm-dd hh:mm:ss +00:00
            DateTest { input: "2012-08-03 18:31:59 +00:00", out: "2006-01-02 15:04:05 -07:00", ..T },
            //   yyyy-mm-dd hh:mm:ss +0000 TZ
            // Golang Native Format = "2006-01-02 15:04:05.999999999 -0700 MST"
            DateTest { input: "2012-08-03 18:31:59 +0000 UTC", out: "2006-01-02 15:04:05 -0700 MST", ..T },
            //   yyyy-mm-dd hh:mm:ss TZ
            DateTest { input: "2012-08-03 18:31:59 UTC", out: "2006-01-02 15:04:05 MST", ..T },
            DateTest { input: "2012-08-03 18:31:59 CEST", out: "2006-01-02 15:04:05 MST", ..T },
            //   yyyy-mm-ddThh:mm:ss-07:00
            DateTest { input: "2009-08-12T22:15:09-07:00", out: "2006-01-02T15:04:05-07:00", ..T },
            //   yyyy-mm-ddThh:mm:ss-0700
            DateTest { input: "2009-08-12T22:15:09-0700", out: "2006-01-02T15:04:05-0700", ..T },
            //   yyyy-mm-ddThh:mm:ssZ
            DateTest { input: "2009-08-12T22:15Z", out: "2006-01-02T15:04Z", ..T },
        ];

        for th in test_parse_format {
            let l = parse_format(th.input, &[]);
            if th.err {
                assert!(l.is_err());
            } else {
                let l = l.unwrap_or_else(|e| panic!("{e} for in={}", th.input));
                assert_eq!(
                    th.out.as_bytes(),
                    &l[..],
                    "for in={}: got {:?}",
                    th.input,
                    String::from_utf8_lossy(&l)
                );
            }
        }
    }

    #[rustfmt::skip]
    static TEST_PARSE_STRICT: &[DateTest] = &[
        //   dd-mon-yy  13-Feb-03
        DateTest { input: "03-03-14", ..T },
        //   mm.dd.yyyy
        DateTest { input: "3.3.2014", ..T },
        //   mm.dd.yy
        DateTest { input: "08.09.71", ..T },
        //  mm/dd/yyyy
        DateTest { input: "3/5/2014", ..T },
        //  mm/dd/yy
        DateTest { input: "08/08/71", ..T },
        DateTest { input: "8/8/71", ..T },
        //  mm/dd/yy hh:mm:ss
        DateTest { input: "04/02/2014 04:08:09", ..T },
        DateTest { input: "4/2/2014 04:08:09", ..T },
    ];

    #[test]
    fn test_parse_strict() {
        let _guard = setup();
        // time.Local as TestParseLayout leaves it.
        gotime::set_local(gotime::utc());

        for th in TEST_PARSE_STRICT {
            let (_, err) = go_result(parse_strict(th.input, &[]));
            assert!(err.is_some());
        }

        let (_, err) = go_result(parse_strict(r#"{"hello"}"#, &[]));
        assert!(err.is_some());

        let (_, err) = go_result(parse_strict("2009-08-12T22:15Z", &[]));
        assert!(err.is_none());
    }

    // Lets test to see how this performs using different Timezones/Locations
    // Also of note, try changing your server/machine timezones and repeat
    //
    // !!!!! The time-zone of local machine effects the results!
    // https://play.golang.org/p/IDHRalIyXh
    // https://github.com/golang/go/issues/18012
    #[test]
    fn test_in_location() {
        let _guard = setup();

        let denver_loc = load_location("America/Denver");

        // Start out with time.UTC
        gotime::set_local(gotime::utc());

        // Just normal parse to test out zone/offset
        let ts = must_parse("2013-02-01 00:00:00", &[]);
        let (zone, offset) = ts.zone();
        assert_eq!(0, offset, "Should have found offset = 0 {offset}");
        assert_eq!("UTC", zone, "Should have found zone = UTC {zone}");
        assert_eq!("2013-02-01 00:00:00 +0000 UTC", utc_string(&ts));

        // Now lets set to denver (MST/MDT) and re-parse the same time string
        // and since no timezone info in string, we expect same result
        gotime::set_local(denver_loc.clone());
        let ts = must_parse("2013-02-01 00:00:00", &[]);
        let (zone, offset) = ts.zone();
        assert_eq!(0, offset, "Should have found offset = 0 {offset}");
        assert_eq!("UTC", zone, "Should have found zone = UTC {zone}");
        assert_eq!("2013-02-01 00:00:00 +0000 UTC", utc_string(&ts));

        let ts = must_parse("Tue, 5 Jul 2017 16:28:13 -0700 (MST)", &[]);
        assert_eq!("2017-07-05 23:28:13 +0000 UTC", utc_string(&ts));

        // Now we are going to use ParseIn() and see that it gives different answer
        // with different zone, offset
        // (Go sets time.Local = nil, which reads as UTC; a Location cannot be
        // nil here, and neither ParseIn call below reads time.Local.)
        gotime::set_local(gotime::utc());
        let (ts, err) = go_result(parse_in("2013-02-01 00:00:00", &denver_loc, &[]));
        assert!(err.is_none());
        let (zone, offset) = ts.zone();
        assert_eq!(
            -25200, offset,
            "Should have found offset = -25200 {offset}  {denver_loc}"
        );
        assert_eq!("MST", zone, "Should have found zone = MST {zone}");
        assert_eq!("2013-02-01 07:00:00 +0000 UTC", utc_string(&ts));

        let (ts, err) = go_result(parse_in("18 January 2018", &denver_loc, &[]));
        assert!(err.is_none());
        let (zone, offset) = ts.zone();
        assert_eq!(-25200, offset, "Should have found offset = 0 {offset}");
        assert_eq!("MST", zone, "Should have found zone = UTC {zone}");
        assert_eq!("2018-01-18 07:00:00 +0000 UTC", utc_string(&ts));

        // Now we are going to use ParseLocal() and see that it gives same
        // answer as ParseIn when we have time.Local set to a location
        gotime::set_local(denver_loc.clone());
        let (ts, err) = go_result(parse_local("2013-02-01 00:00:00", &[]));
        assert!(err.is_none());
        let (zone, offset) = ts.zone();
        assert_eq!(
            -25200, offset,
            "Should have found offset = -25200 {offset}  {denver_loc}"
        );
        assert_eq!("MST", zone, "Should have found zone = MST {zone}");
        assert_eq!("2013-02-01 07:00:00 +0000 UTC", utc_string(&ts));

        // Lets advance past daylight savings time start
        // use parseIn and see offset/zone has changed to Daylight Savings Equivalents
        let (ts, err) = go_result(parse_in("2013-04-01 00:00:00", &denver_loc, &[]));
        assert!(err.is_none());
        let (zone, offset) = ts.zone();
        assert_eq!(
            -21600, offset,
            "Should have found offset = -21600 {offset}  {denver_loc}"
        );
        assert_eq!("MDT", zone, "Should have found zone = MDT {zone}");
        assert_eq!("2013-04-01 06:00:00 +0000 UTC", utc_string(&ts));

        // reset to UTC
        gotime::set_local(gotime::utc());

        //   UnixDate    = "Mon Jan _2 15:04:05 MST 2006"
        let ts = must_parse("Mon Jan  2 15:04:05 MST 2006", &[]);

        let (_, offset) = ts.zone();
        assert_eq!(0, offset, "Should have found offset = 0 {offset}");
        assert_eq!("2006-01-02 15:04:05 +0000 UTC", utc_string(&ts));

        // Now lets set to denver(mst/mdt)
        gotime::set_local(denver_loc.clone());
        let ts = must_parse("Mon Jan  2 15:04:05 MST 2006", &[]);

        // this time is different from one above parsed with time.Local set to UTC
        let (_, offset) = ts.zone();
        assert_eq!(-25200, offset, "Should have found offset = -25200 {offset}");
        assert_eq!("2006-01-02 22:04:05 +0000 UTC", utc_string(&ts));

        // Now Reset To UTC
        gotime::set_local(gotime::utc());

        // RFC850    = "Monday, 02-Jan-06 15:04:05 MST"
        let ts = must_parse("Monday, 02-Jan-06 15:04:05 MST", &[]);
        let (_, offset) = ts.zone();
        assert_eq!(0, offset, "Should have found offset = 0 {offset}");
        assert_eq!("2006-01-02 15:04:05 +0000 UTC", utc_string(&ts));

        // Now lets set to denver
        gotime::set_local(denver_loc.clone());
        let ts = must_parse("Monday, 02-Jan-06 15:04:05 MST", &[]);
        let (_, offset) = ts.zone();
        assert_ne!(0, offset, "Should have found offset {offset}");
        assert_eq!("2006-01-02 22:04:05 +0000 UTC", utc_string(&ts));

        // Now some errors
        let zero_time = Time::default().unix();
        let (ts, err) = go_result(parse_in("INVALID", &denver_loc, &[]));
        assert_eq!(zero_time, ts.unix());
        assert!(err.is_some());

        let (ts, err) = go_result(parse_local("INVALID", &[]));
        assert_eq!(zero_time, ts.unix());
        assert!(err.is_some());
    }

    #[test]
    fn test_prefer_month_first() {
        let _guard = setup();
        // time.Local as TestInLocation leaves it.
        gotime::set_local(load_location("America/Denver"));

        // default case is true
        let (ts, err) = go_result(parse_any("04/02/2014 04:08:09 +0000 UTC", &[]));
        assert!(err.is_none());
        assert_eq!("2014-04-02 04:08:09 +0000 UTC", utc_string(&ts));

        let prefer_month_first_true = ParserOption::PreferMonthFirst(true);
        let (ts, err) = go_result(parse_any(
            "04/02/2014 04:08:09 +0000 UTC",
            &[prefer_month_first_true],
        ));
        assert!(err.is_none());
        assert_eq!("2014-04-02 04:08:09 +0000 UTC", utc_string(&ts));

        // allows the day to be preferred before the month, when completely ambiguous
        let prefer_month_first_false = ParserOption::PreferMonthFirst(false);
        let (ts, err) = go_result(parse_any(
            "04/02/2014 04:08:09 +0000 UTC",
            &[prefer_month_first_false],
        ));
        assert!(err.is_none());
        assert_eq!("2014-02-04 04:08:09 +0000 UTC", utc_string(&ts));
    }

    #[test]
    fn test_retry_ambiguous_date_with_swap() {
        let _guard = setup();
        // time.Local as TestInLocation leaves it.
        gotime::set_local(load_location("America/Denver"));

        // default is false
        let (_, err) = go_result(parse_any("13/02/2014 04:08:09 +0000 UTC", &[]));
        assert!(err.is_some());

        // will fail error if the month preference cannot work due to the value being larger than 12
        let retry_ambiguous_date_with_swap_false = ParserOption::RetryAmbiguousDateWithSwap(false);
        let (_, err) = go_result(parse_any(
            "13/02/2014 04:08:09 +0000 UTC",
            &[retry_ambiguous_date_with_swap_false],
        ));
        assert!(err.is_some());

        // will retry with the other month preference if this error is detected
        let retry_ambiguous_date_with_swap_true = ParserOption::RetryAmbiguousDateWithSwap(true);
        let (ts, err) = go_result(parse_any(
            "13/02/2014 04:08:09 +0000 UTC",
            &[retry_ambiguous_date_with_swap_true],
        ));
        assert!(err.is_none());
        assert_eq!("2014-02-13 04:08:09 +0000 UTC", utc_string(&ts));
    }

    /// `storage/cache/database_test.go` lines 92-99 (`TestMeta`): the cache
    /// stores a time as `Time.String()` (`cache.Time`) and reads it back with
    /// `dateparse.ParseAny` and `.In(time.UTC)` (`ReturnValue.Time`).
    ///
    /// Go's cache tests run in the machine's `time.Local`; this runs the case
    /// in the Go test environment's `Local` and in a few other zones a node
    /// may run in.
    #[test]
    fn cache_time_round_trip() {
        let _guard = setup();
        let zones = [
            load_location("America/Los_Angeles").renamed("Local"),
            gotime::utc(),
            load_location("Asia/Shanghai"),
            load_location("Asia/Kathmandu"),
            load_location("Pacific/Chatham"),
        ];
        for zone in zones {
            gotime::set_local(zone.clone());

            // set meta time (cache.Time)
            let value = gotime::date(1996, Month(4), 8, 0, 0, 0, 0, &gotime::utc()).string();
            // get meta time (ReturnValue.Time)
            let val_time = parse_any(&value, &[])
                .unwrap_or_else(|e| panic!("Local {zone}: {e}"))
                .in_location(&gotime::utc());
            assert_eq!(1996, val_time.year(), "Local {zone}");
            assert_eq!(Month(4), val_time.month(), "Local {zone}");
            assert_eq!(8, val_time.day(), "Local {zone}");
        }
    }
}
