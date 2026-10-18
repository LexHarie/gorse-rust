//! `dateparse` against `tests/data/dateparse_golden.json`, which
//! `contracts/tools/timegolden` wrote by running dateparse @
//! v0.0.0-20210429162001-6b43995a97de: `ParseAny`, `ParseIn`, `ParseLocal`,
//! `ParseStrict` and `ParseFormat` over dateparse's own test inputs, edge
//! cases, cache values Go nodes write and a seeded corpus of mutations, in
//! several `time.Local` zones and with each option. Each case must match
//! exactly: the time, or the error text, or the layout.

mod common;

mod dateparse {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use gorse_common::dateparse::{
        ParserOption, parse_any, parse_format, parse_in, parse_local, parse_strict,
    };
    use serde::Deserialize;

    use crate::common::{
        TimeResult, describe, go_json_string, load_golden, location, lock, set_local,
    };

    #[derive(Deserialize)]
    struct Golden {
        go_version: String,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        local: String,
        func: String,
        #[serde(rename = "in")]
        input: String,
        #[serde(default)]
        opts: Vec<String>,
        #[serde(default)]
        loc: String,
        result: Option<TimeResult>,
        #[serde(default)]
        format: String,
        #[serde(default)]
        err: String,
        #[serde(default)]
        panic: String,
    }

    /// What one call gave, in the golden's terms: the result (none for
    /// `ParseFormat` and errors), the layout, the error text.
    #[derive(Debug)]
    struct Outcome {
        result: Option<TimeResult>,
        format: String,
        err: String,
    }

    /// The golden tool's `options`.
    fn options(names: &[String]) -> Vec<ParserOption> {
        names
            .iter()
            .map(|n| match n.as_str() {
                "PreferMonthFirst(true)" => ParserOption::PreferMonthFirst(true),
                "PreferMonthFirst(false)" => ParserOption::PreferMonthFirst(false),
                "RetryAmbiguousDateWithSwap(true)" => {
                    ParserOption::RetryAmbiguousDateWithSwap(true)
                }
                "RetryAmbiguousDateWithSwap(false)" => {
                    ParserOption::RetryAmbiguousDateWithSwap(false)
                }
                _ => panic!("unknown option {n:?}"),
            })
            .collect()
    }

    /// The golden tool's `runDateparse`, after `setLocal`.
    fn run(c: &Case) -> Outcome {
        let opts = options(&c.opts);
        let result = match c.func.as_str() {
            "ParseAny" => parse_any(&c.input, &opts),
            "ParseIn" => parse_in(&c.input, &location(&c.loc), &opts),
            "ParseLocal" => parse_local(&c.input, &opts),
            "ParseStrict" => parse_strict(&c.input, &opts),
            "ParseFormat" => {
                return match parse_format(&c.input, &opts) {
                    Ok(format) => Outcome {
                        result: None,
                        format: go_json_string(&format),
                        err: String::new(),
                    },
                    Err(e) => Outcome {
                        result: None,
                        format: String::new(),
                        err: go_json_string(&e.error_bytes()),
                    },
                };
            }
            f => panic!("unknown function {f:?}"),
        };
        match result {
            Ok(t) => Outcome {
                result: Some(describe(&t)),
                format: String::new(),
                err: String::new(),
            },
            Err(e) => Outcome {
                result: None,
                format: String::new(),
                err: go_json_string(&e.error_bytes()),
            },
        }
    }

    #[test]
    fn golden() {
        let _guard = lock();
        let g: Golden = load_golden("dateparse_golden.json");
        assert_eq!(g.go_version, "go1.27.0");
        assert!(!g.cases.is_empty(), "no cases");

        let mut failures = Vec::new();
        for c in &g.cases {
            set_local(&c.local);
            let id = format!(
                "local {} {}({:?}) opts {:?} loc {:?}",
                c.local, c.func, c.input, c.opts, c.loc
            );
            let got = catch_unwind(AssertUnwindSafe(|| run(c)));
            match got {
                // Go panicked: so must this (Rust's panic text differs).
                _ if !c.panic.is_empty() => {
                    if let Ok(got) = got {
                        failures.push(format!("{id}: got {got:?}, want Go's panic {:?}", c.panic));
                    }
                }
                Err(_) => failures.push(format!("{id}: panicked, Go did not")),
                Ok(got) => {
                    if got.result.as_ref() != c.result.as_ref()
                        || got.format != c.format
                        || got.err != c.err
                    {
                        failures.push(format!(
                            "{id}: got {got:?}, want result {:?} format {:?} err {:?}",
                            c.result, c.format, c.err
                        ));
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} dateparse cases differ from Go:\n{}",
            failures.len(),
            g.cases.len(),
            failures[..failures.len().min(30)].join("\n")
        );
    }
}
