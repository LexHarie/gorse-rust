// Checked against: contracts/tools/tpegolden (the Go oracle of this module), whose
// output is committed as crates/gorse-model/testdata/tpe/golden.json.
//
// Derived from Go's math package and from goptuna@v0.9.0 (MIT): see the
// headers of gomath.rs and sampler.rs.

//! The Go oracle checks of the TPE module. The golden file holds Go's
//! `math.Erf` on a grid, Go's `math.Max` and `math.Min` on special values, and
//! a goptuna study of 60 trials. Every comparison is bit for bit, except that
//! a NaN matches any NaN, because Go's NaN carries another payload.

use serde::Deserialize;

use super::gomath::{erf, go_max, go_min};
use super::sampler::TpeSampler;
use super::{Direction, Failure, Study};

const GOLDEN: &str = include_str!("../../testdata/tpe/golden.json");

#[derive(Deserialize)]
struct Golden {
    erf: Vec<ErfCase>,
    maxmin: Vec<MaxMinCase>,
    trials: Vec<TrialCase>,
}

#[derive(Deserialize)]
struct ErfCase {
    x: String,
    erf: String,
}

#[derive(Deserialize)]
struct MaxMinCase {
    a: String,
    b: String,
    max: String,
    min: String,
}

#[derive(Deserialize)]
struct TrialCase {
    number: usize,
    x: String,
    lr: String,
    q: String,
    n: String,
    s: String,
    c: String,
    value: String,
}

fn golden() -> Golden {
    serde_json::from_str(GOLDEN).expect("the golden file parses")
}

/// Reads a bit pattern written as `0x` and 16 hex digits.
fn bits(hex: &str) -> u64 {
    u64::from_str_radix(hex.trim_start_matches("0x"), 16).expect("the golden holds hex bits")
}

/// Asserts that `got` is the golden value `want`, bit for bit, or NaN for a NaN.
fn assert_go_value(got: f64, want: &str, what: &str) {
    let want = f64::from_bits(bits(want));
    if want.is_nan() {
        assert!(got.is_nan(), "{what}: want NaN, got {got}");
    } else {
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "{what}: got {got}, want {want}"
        );
    }
}

#[test]
fn erf_matches_go_golden() {
    let golden = golden();
    assert_eq!(
        golden.erf.len(),
        769,
        "the grid is -6 to 6 in steps of 1/64"
    );
    for case in golden.erf {
        let x = f64::from_bits(bits(&case.x));
        assert_go_value(erf(x), &case.erf, &format!("erf({x})"));
    }
}

#[test]
fn max_and_min_match_go_golden() {
    let golden = golden();
    assert_eq!(
        golden.maxmin.len(),
        64,
        "every pair of the eight special values"
    );
    for case in golden.maxmin {
        let a = f64::from_bits(bits(&case.a));
        let b = f64::from_bits(bits(&case.b));
        assert_go_value(go_max(a, b), &case.max, &format!("max({a}, {b})"));
        assert_go_value(go_min(a, b), &case.min, &format!("min({a}, {b})"));
    }
}

#[test]
fn seeded_study_matches_go_golden() {
    const CHOICES: [&str; 3] = ["a", "b", "c"];
    let mut study = Study::new(
        Direction::Minimize,
        Box::new(TpeSampler::with_go_golden_seeds(1)),
    );
    let mut got = Vec::new();
    study
        .optimize(
            |trial| {
                let number = trial.number();
                let x = trial
                    .suggest_float("x", -10.0, 10.0)
                    .map_err(Failure::error)?;
                let lr = trial
                    .suggest_log_float("lr", 1e-3, 10.0)
                    .map_err(Failure::error)?;
                let q = trial
                    .suggest_discrete_float("q", 0.0, 1.0, 0.25)
                    .map_err(Failure::error)?;
                let n = trial.suggest_int("n", 1, 10).map_err(Failure::error)?;
                let s = trial
                    .suggest_step_int("s", 0, 10, 2)
                    .map_err(Failure::error)?;
                let c = trial
                    .suggest_categorical("c", &CHOICES)
                    .map_err(Failure::error)?;
                let idx = CHOICES
                    .iter()
                    .position(|choice| *choice == c)
                    .expect("the choice is one of CHOICES");
                // The same sums, in the same order, as the Go objective.
                let value = (x - 1.0).abs()
                    + (lr - 0.5).abs()
                    + (q - 0.5).abs()
                    + (n - 4).abs() as f64
                    + s as f64
                    + idx as f64;
                got.push((
                    number,
                    [x, lr, q, n as f64, s as f64, idx as f64].map(f64::to_bits),
                    value.to_bits(),
                ));
                Ok(value)
            },
            60,
        )
        .expect("the study runs");

    let want = golden().trials;
    assert_eq!(got.len(), want.len(), "the number of trials");
    for ((number, params, value), want) in got.into_iter().zip(want) {
        assert_eq!(number, want.number, "trial number");
        let want_params =
            [&want.x, &want.lr, &want.q, &want.n, &want.s, &want.c].map(|hex| bits(hex));
        assert_eq!(params, want_params, "suggestions of trial {number}");
        assert_eq!(value, bits(&want.value), "value of trial {number}");
    }
}
