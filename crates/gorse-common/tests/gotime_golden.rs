//! `gotime` against `tests/data/gotime_golden.json`, which
//! `contracts/tools/timegolden` wrote by running Go's time package: every
//! layout over instants in many locations, `Time.String`, the calendar
//! accessors and `MarshalJSON`; `Parse` and `ParseInLocation` results and
//! error texts; `Date` normalization and zone gaps; `Duration.String`,
//! `ParseDuration` and `UnmarshalJSON`. Each case must match exactly.

mod common;

mod gotime {
    use gorse_common::gotime::{
        self, Duration, Month, Time, parse, parse_duration, parse_in_location,
    };
    use serde::Deserialize;

    use crate::common::{
        TimeResult, describe, go_json_string, load_golden, location, lock, set_local,
    };

    #[derive(Deserialize)]
    struct Golden {
        go_version: String,
        local: String,
        layouts: Vec<String>,
        format: Vec<FormatCase>,
        parse: Vec<ParseCase>,
        date: Vec<DateCase>,
        duration_string: Vec<DurationCase>,
        parse_duration: Vec<ParseDurationCase>,
        unmarshal: Vec<UnmarshalCase>,
    }

    #[derive(Deserialize)]
    struct FormatCase {
        unix: String,
        nsec: i64,
        location: String,
        outputs: Vec<String>,
        string: String,
        date: [i64; 3],
        clock: [i64; 3],
        weekday: i64,
        yday: i64,
        json: String,
        json_err: String,
    }

    #[derive(Deserialize)]
    struct ParseCase {
        local: String,
        layout: String,
        value: String,
        #[serde(rename = "in")]
        in_loc: String,
        result: Option<TimeResult>,
        err: String,
        err_elems: Option<Vec<String>>,
    }

    #[derive(Deserialize)]
    struct DateCase {
        args: [i64; 7],
        location: String,
        result: TimeResult,
    }

    #[derive(Deserialize)]
    struct DurationCase {
        d: String,
        string: String,
    }

    #[derive(Deserialize)]
    struct ParseDurationCase {
        #[serde(rename = "in")]
        input: String,
        d: String,
        err: String,
    }

    #[derive(Deserialize)]
    struct UnmarshalCase {
        local: String,
        #[serde(rename = "in")]
        input: String,
        result: Option<TimeResult>,
        err: String,
    }

    fn golden() -> Golden {
        let g: Golden = load_golden("gotime_golden.json");
        assert_eq!(g.go_version, "go1.27.0");
        g
    }

    /// Fails with the first mismatches and their count.
    fn report(what: &str, total: usize, failures: &[String]) {
        assert!(total > 0, "no {what} cases");
        assert!(
            failures.is_empty(),
            "{} of {total} {what} cases differ from Go:\n{}",
            failures.len(),
            failures[..failures.len().min(30)].join("\n")
        );
    }

    #[test]
    fn golden_format() {
        let _guard = lock();
        let g = golden();
        set_local(&g.local);
        let mut failures = Vec::new();
        for c in &g.format {
            let sec: i64 = c.unix.parse().expect("unix");
            let t = gotime::unix(sec, c.nsec).in_location(&location(&c.location));
            let id = format!("{}.{:09} in {}", c.unix, c.nsec, c.location);
            for (layout, want) in g.layouts.iter().zip(&c.outputs) {
                let got = t.format(layout);
                if &got != want {
                    failures.push(format!("{id}: Format({layout:?}) = {got:?}, want {want:?}"));
                }
            }
            if t.string() != c.string {
                failures.push(format!(
                    "{id}: String() = {:?}, want {:?}",
                    t.string(),
                    c.string
                ));
            }
            let (y, m, d) = t.date();
            if [y, m.0, d] != c.date {
                failures.push(format!(
                    "{id}: Date() = {:?}, want {:?}",
                    [y, m.0, d],
                    c.date
                ));
            }
            let (h, mi, s) = t.clock();
            if [h, mi, s] != c.clock {
                failures.push(format!(
                    "{id}: Clock() = {:?}, want {:?}",
                    [h, mi, s],
                    c.clock
                ));
            }
            if t.weekday().0 != c.weekday {
                failures.push(format!(
                    "{id}: Weekday() = {}, want {}",
                    t.weekday().0,
                    c.weekday
                ));
            }
            if t.year_day() != c.yday {
                failures.push(format!(
                    "{id}: YearDay() = {}, want {}",
                    t.year_day(),
                    c.yday
                ));
            }
            let (json, json_err) = match t.marshal_json() {
                Ok(b) => (String::from_utf8(b).expect("ASCII"), String::new()),
                Err(e) => (String::new(), e.to_string()),
            };
            if json != c.json || json_err != c.json_err {
                failures.push(format!(
                    "{id}: MarshalJSON() = ({json:?}, {json_err:?}), want ({:?}, {:?})",
                    c.json, c.json_err
                ));
            }
        }
        report("format", g.format.len(), &failures);
    }

    #[test]
    fn golden_parse() {
        let _guard = lock();
        let g = golden();
        let mut failures = Vec::new();
        for c in &g.parse {
            set_local(&c.local);
            let got = if c.in_loc.is_empty() {
                parse(&c.layout, &c.value)
            } else {
                parse_in_location(&c.layout, &c.value, &location(&c.in_loc))
            };
            let id = format!(
                "local {} Parse({:?}, {:?}) in {:?}",
                c.local, c.layout, c.value, c.in_loc
            );
            match got {
                Ok(t) => {
                    let r = describe(&t);
                    if c.result.as_ref() != Some(&r) || !c.err.is_empty() {
                        failures.push(format!(
                            "{id}: got {r:?}, want {:?} / err {:?}",
                            c.result, c.err
                        ));
                    }
                }
                Err(e) => {
                    let elems = vec![
                        go_json_string(e.layout()),
                        go_json_string(e.value()),
                        go_json_string(e.layout_elem()),
                        go_json_string(e.value_elem()),
                        e.message().to_owned(),
                    ];
                    if e.to_string() != c.err || c.err_elems.as_ref() != Some(&elems) {
                        failures.push(format!(
                            "{id}: err {:?} {elems:?}, want {:?} {:?} (result {:?})",
                            e.to_string(),
                            c.err,
                            c.err_elems,
                            c.result
                        ));
                    }
                }
            }
        }
        report("parse", g.parse.len(), &failures);
    }

    #[test]
    fn golden_date() {
        let _guard = lock();
        let g = golden();
        set_local(&g.local);
        let mut failures = Vec::new();
        for c in &g.date {
            let a = c.args;
            let t = gotime::date(
                a[0],
                Month(a[1]),
                a[2],
                a[3],
                a[4],
                a[5],
                a[6],
                &location(&c.location),
            );
            let r = describe(&t);
            if r != c.result {
                failures.push(format!(
                    "Date({a:?}, {}) = {r:?}, want {:?}",
                    c.location, c.result
                ));
            }
        }
        report("date", g.date.len(), &failures);
    }

    #[test]
    fn golden_duration_string() {
        let g = golden();
        let mut failures = Vec::new();
        for c in &g.duration_string {
            let d = Duration(c.d.parse().expect("i64"));
            if d.to_string() != c.string {
                failures.push(format!(
                    "Duration({}).String() = {:?}, want {:?}",
                    c.d,
                    d.to_string(),
                    c.string
                ));
            }
        }
        report("Duration.String", g.duration_string.len(), &failures);
    }

    #[test]
    fn golden_parse_duration() {
        let g = golden();
        let mut failures = Vec::new();
        for c in &g.parse_duration {
            let (d, err) = match parse_duration(&c.input) {
                Ok(d) => (d.0.to_string(), String::new()),
                Err(e) => (String::new(), e.to_string()),
            };
            if d != c.d || err != c.err {
                failures.push(format!(
                    "ParseDuration({:?}) = ({d:?}, {err:?}), want ({:?}, {:?})",
                    c.input, c.d, c.err
                ));
            }
        }
        report("ParseDuration", g.parse_duration.len(), &failures);
    }

    #[test]
    fn golden_unmarshal_json() {
        let _guard = lock();
        let g = golden();
        let mut failures = Vec::new();
        for c in &g.unmarshal {
            set_local(&c.local);
            let mut t = Time::default();
            let (r, err) = match t.unmarshal_json(c.input.as_bytes()) {
                Ok(()) => (Some(describe(&t)), String::new()),
                Err(e) => (None, e.to_string()),
            };
            if r != c.result || err != c.err {
                failures.push(format!(
                    "local {} UnmarshalJSON({:?}) = ({r:?}, {err:?}), want ({:?}, {:?})",
                    c.local, c.input, c.result, c.err
                ));
            }
        }
        report("UnmarshalJSON", g.unmarshal.len(), &failures);
    }
}
