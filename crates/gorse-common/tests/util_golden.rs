// Checks common/util/util.go, common/util/strconv.go @ add21b5 and the Go strconv functions they use @ go1.27.0

//! The Rust number parsers, `gostrconv::quote` and `is_print` (which the
//! parsers' errors use), `validate_id` and `md5`
//! against the outputs of the Go oracle, written by
//! `contracts/tools/utilgolden` to `tests/data/util_golden.json`.

use std::sync::OnceLock;

use gorse_common::gostrconv;
use gorse_common::util::{self, NumError};
use serde_json::Value;

fn golden() -> &'static Value {
    static GOLDEN: OnceLock<Value> = OnceLock::new();
    GOLDEN.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/util_golden.json");
        let text = std::fs::read_to_string(path).expect("read util_golden.json");
        serde_json::from_str(&text).expect("parse util_golden.json")
    })
}

fn cases(name: &str) -> &'static [Value] {
    golden()[name].as_array().expect(name)
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

/// A Go result `<hex bits>|<error text>`. Only the error text is compared on
/// failure: the Rust parsers return no value with an error.
fn check<T>(got: Result<T, NumError>, bits: impl Fn(T) -> u64, want: &str, what: &str) {
    let (want_bits, want_err) = want.split_once('|').expect("result");
    match got {
        Ok(v) => {
            assert_eq!(
                want_err,
                "",
                "{what}: Go fails, Rust gives {:016x}",
                bits(v)
            );
            assert_eq!(format!("{:016x}", bits(v)), want_bits, "{what}");
        }
        Err(e) => assert_eq!(e.to_string(), want_err, "{what}"),
    }
}

/// A `gostrconv` result, which keeps Go's value next to an error: both the
/// bits and the error text are compared.
fn check_go<T>(got: (T, Option<NumError>), bits: impl Fn(T) -> u64, want: &str, what: &str) {
    let (want_bits, want_err) = want.split_once('|').expect("result");
    let (value, err) = got;
    assert_eq!(format!("{:016x}", bits(value)), want_bits, "{what}");
    assert_eq!(
        err.map(|e| e.to_string()).unwrap_or_default(),
        want_err,
        "{what}"
    );
}

#[test]
fn util_parse_float_matches_go() {
    let floats = cases("floats");
    assert!(floats.len() > 2000);
    for c in floats {
        let input = s(&c["in"]);
        let short: String = input.chars().take(60).collect();
        check(
            util::parse_float::<f32>(input),
            |v| u64::from(v.to_bits()),
            s(&c["f32"]),
            &format!("ParseFloat[float32]({short:?})"),
        );
        check(
            util::parse_float::<f64>(input),
            f64::to_bits,
            s(&c["f64"]),
            &format!("ParseFloat[float64]({short:?})"),
        );
        check_go(
            gostrconv::parse_float(input, 32),
            |v| u64::from((v as f32).to_bits()),
            s(&c["f32"]),
            &format!("ParseFloat({short:?}, 32)"),
        );
        check_go(
            gostrconv::parse_float(input, 64),
            f64::to_bits,
            s(&c["f64"]),
            &format!("ParseFloat({short:?}, 64)"),
        );
    }
}

#[test]
fn util_parse_int_matches_go() {
    let ints = cases("ints");
    assert!(ints.len() > 700);
    for c in ints {
        let input = s(&c["in"]);
        let w = |key: &str| s(&c[key]).to_owned();
        let signed = |v: i64| v.cast_unsigned();
        check(
            util::parse_int::<i8>(input).map(i64::from),
            signed,
            &w("i8"),
            &format!("ParseInt[int8]({input:?})"),
        );
        check(
            util::parse_int::<i16>(input).map(i64::from),
            signed,
            &w("i16"),
            &format!("ParseInt[int16]({input:?})"),
        );
        check(
            util::parse_int::<i32>(input).map(i64::from),
            signed,
            &w("i32"),
            &format!("ParseInt[int32]({input:?})"),
        );
        check(
            util::parse_int::<i64>(input),
            signed,
            &w("i64"),
            &format!("ParseInt[int64]({input:?})"),
        );
        check(
            util::parse_uint::<u8>(input).map(u64::from),
            |v| v,
            &w("u8"),
            &format!("ParseUInt[uint8]({input:?})"),
        );
        check(
            util::parse_uint::<u16>(input).map(u64::from),
            |v| v,
            &w("u16"),
            &format!("ParseUInt[uint16]({input:?})"),
        );
        check(
            util::parse_uint::<u32>(input).map(u64::from),
            |v| v,
            &w("u32"),
            &format!("ParseUInt[uint32]({input:?})"),
        );
        check(
            util::parse_uint::<u64>(input),
            |v| v,
            &w("u64"),
            &format!("ParseUInt[uint64]({input:?})"),
        );
        check_go(
            gostrconv::atoi(input),
            signed,
            &w("atoi"),
            &format!("Atoi({input:?})"),
        );
        check_go(
            gostrconv::parse_int(input, 10, 0),
            signed,
            &w("int0"),
            &format!("ParseInt({input:?}, 10, 0)"),
        );
        check_go(
            gostrconv::parse_uint(input, 10, 0),
            |v| v,
            &w("uint0"),
            &format!("ParseUint({input:?}, 10, 0)"),
        );
    }
}

#[test]
fn util_format_int_matches_go() {
    for c in cases("format_ints") {
        let v = c["in"].as_i64().expect("in");
        assert_eq!(util::format_int(v), s(&c["out"]));
        if let Ok(v32) = i32::try_from(v) {
            assert_eq!(util::format_int(v32), s(&c["out"]));
        }
    }
}

#[test]
fn util_quote_matches_go() {
    let quotes = cases("quotes");
    let chars: usize = quotes.iter().map(|c| s(&c["in"]).chars().count()).sum();
    assert!(
        chars > 3000,
        "the corpus covers Latin-1 and every IsPrint range edge"
    );
    for c in quotes {
        let input = s(&c["in"]);
        assert_eq!(gostrconv::quote(input), s(&c["out"]), "Quote({input:?})");
    }
}

#[test]
fn util_is_print_matches_go_at_every_range_edge() {
    let edges = cases("is_print");
    assert!(edges.len() > 2900);
    for c in edges {
        let r = u32::try_from(c["rune"].as_i64().expect("rune")).expect("code point");
        let ch = char::from_u32(r).expect("not a surrogate");
        assert_eq!(
            gostrconv::is_print(ch),
            c["print"].as_bool().expect("print"),
            "IsPrint(U+{r:04X})"
        );
    }
}

#[test]
fn util_validate_id_matches_go() {
    for c in cases("validate_id") {
        let input = s(&c["in"]);
        let got = util::validate_id(input)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert_eq!(got, s(&c["err"]), "ValidateId({input:?})");
    }
}

#[test]
fn util_md5_matches_go() {
    for c in cases("md5") {
        let input: Vec<&str> = c["in"].as_array().expect("in").iter().map(s).collect();
        assert_eq!(util::md5(&input), s(&c["out"]), "MD5({input:?})");
    }
}
