//! `common/expression` against what the Go package does with the same inputs
//! (`contracts/tools/exprgolden`, written to `expression_golden.json`).
//!
//! The corpus holds every feedback type expression in the Go tests and config
//! files of the pinned tree, edge cases of the pattern, and seeded random
//! strings and values. Each section is compared exactly: states field by field
//! with floats by their bits, texts and error messages byte for byte.

use gorse_common::expression::{
    self, ExprType, FeedbackTypeExpression, match_feedback_type_expressions,
    must_parse_feedback_type_expression,
};
use gorse_protocol::FeedbackTypeExpression as Pb;
use prost::Message;
use serde_json::Value as Json;

fn golden() -> Json {
    let path = format!(
        "{}/tests/data/expression_golden.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("golden file");
    serde_json::from_str(&text).expect("golden JSON")
}

fn cases<'a>(g: &'a Json, section: &str) -> &'a [Json] {
    let cases = g[section].as_array().expect("section");
    assert!(!cases.is_empty(), "section {section} is empty");
    cases
}

fn str_of(v: &Json) -> &str {
    v.as_str().expect("string")
}

fn f64_bits(v: &Json) -> f64 {
    f64::from_bits(u64::from_str_radix(str_of(v), 16).expect("hex bits"))
}

fn error_of(v: &Json) -> Option<&str> {
    v.as_str()
}

/// Go's `ExprType` constants are `iota` from `None`.
fn expr_type(n: i64) -> ExprType {
    match n {
        0 => ExprType::None,
        1 => ExprType::Less,
        2 => ExprType::LessOrEqual,
        3 => ExprType::Greater,
        4 => ExprType::GreaterOrEqual,
        _ => panic!("Go ExprType {n} outside the constants"),
    }
}

fn state(v: &Json) -> FeedbackTypeExpression {
    FeedbackTypeExpression {
        feedback_type: str_of(&v["feedback_type"]).to_owned(),
        expr_type: expr_type(v["expr_type"].as_i64().expect("expr_type")),
        value: f64_bits(&v["value"]),
    }
}

fn pb(v: &Json) -> Pb {
    Pb {
        feedback_type: str_of(&v["feedback_type"]).to_owned(),
        expression_type: i32::try_from(v["expression_type"].as_i64().expect("expression_type"))
            .expect("int32"),
        value: f64_bits(&v["value"]),
    }
}

/// Field-by-field equality with floats compared by their bits, so `-0`,
/// `NaN` payloads and the last bit of every value count.
fn same(a: &FeedbackTypeExpression, b: &FeedbackTypeExpression) -> bool {
    a.feedback_type == b.feedback_type
        && a.expr_type == b.expr_type
        && a.value.to_bits() == b.value.to_bits()
}

fn same_pb(a: &Pb, b: &Pb) -> bool {
    a.feedback_type == b.feedback_type
        && a.expression_type == b.expression_type
        && a.value.to_bits() == b.value.to_bits()
}

#[test]
fn expression_from_string_matches_go() {
    let g = golden();
    let mut sources = std::collections::BTreeMap::new();
    for c in cases(&g, "parse") {
        let input = str_of(&c["input"]);
        let mut e = state(&c["start"]);
        let result = e.from_string(input);
        let go_error = error_of(&c["error"]);
        assert_eq!(
            result.as_ref().err().map(ToString::to_string).as_deref(),
            go_error,
            "error of FromString({input:?})"
        );
        let want = state(&c["state"]);
        assert!(
            same(&e, &want),
            "FromString({input:?}) from {:?}: got {e:?}, Go {want:?}",
            c["start"]
        );
        *sources.entry(str_of(&c["source"]).to_owned()).or_insert(0) += 1;
    }
    // Every corpus source took part.
    for source in [
        "expression_test.go",
        "config.toml",
        "client/config.toml",
        "go tests",
        "server/rest_test.go",
        "edge",
        "random",
        "random value",
    ] {
        assert!(sources.contains_key(source), "no cases from {source}");
    }
}

#[test]
fn expression_string_and_marshal_json_match_go() {
    let g = golden();
    for c in cases(&g, "format") {
        let e = state(&c["state"]);
        assert_eq!(e.to_string(), str_of(&c["string"]), "String of {e:?}");
        assert_eq!(
            e.marshal_json(),
            str_of(&c["marshal_json"]),
            "MarshalJSON of {e:?}"
        );
    }
}

#[test]
fn expression_to_pb_matches_go() {
    let g = golden();
    let mut negative_zero = 0;
    for c in cases(&g, "to_pb") {
        let e = state(&c["state"]);
        let got = e.to_pb();
        let want = pb(&c["pb"]);
        assert!(
            same_pb(&got, &want),
            "ToPB of {e:?}: got {got:?}, Go {want:?}"
        );
        let go_bytes = hex::decode(str_of(&c["hex"])).expect("hex");
        let rust_bytes = got.encode_to_vec();
        if e.value.to_bits() == (-0.0_f64).to_bits() {
            // protobuf-go writes a proto3 double holding -0 (its bits are not
            // zero); prost compares with 0.0 and leaves the field out, so the
            // receiver reads +0. This is the gorse-protocol encoder's
            // behaviour, reported as an open item in porting/notes/WP-1-7.md.
            // Assert the exact difference so a change in prost is noticed.
            let field = [0x19, 0, 0, 0, 0, 0, 0, 0, 0x80];
            let at = go_bytes
                .windows(field.len())
                .position(|w| w == field)
                .expect("Go writes the -0 value field");
            let mut without = go_bytes.clone();
            without.drain(at..at + field.len());
            assert_eq!(rust_bytes, without, "ToPB bytes of {e:?}");
            negative_zero += 1;
        } else {
            assert_eq!(rust_bytes, go_bytes, "ToPB bytes of {e:?}");
        }
    }
    assert_eq!(negative_zero, 15, "the -0 cases are the only exceptions");
}

#[test]
fn expression_from_pb_matches_go() {
    let g = golden();
    for c in cases(&g, "from_pb") {
        // Decode the Go bytes, as a Rust node receiving them would.
        let bytes = hex::decode(str_of(&c["hex"])).expect("hex");
        let decoded = Pb::decode(bytes.as_slice()).expect("decode Go bytes");
        assert!(same_pb(&decoded, &pb(&c["pb"])), "decoded {decoded:?}");
        let mut e = state(&c["start"]);
        e.from_pb(&decoded);
        let want = state(&c["state"]);
        assert!(
            same(&e, &want),
            "FromPB({decoded:?}) from {:?}: got {e:?}, Go {want:?}",
            c["start"]
        );
    }
}

#[test]
fn expression_match_matches_go() {
    let g = golden();
    for c in cases(&g, "match") {
        let e = state(&c["state"]);
        let feedback_type = str_of(&c["feedback_type"]);
        let value = f64_bits(&c["value"]);
        assert_eq!(
            e.matches(feedback_type, value),
            c["match"].as_bool().expect("bool"),
            "{e:?}.Match({feedback_type:?}, {value:?})"
        );
    }
    for c in cases(&g, "match_any") {
        let exprs: Vec<_> = c["exprs"]
            .as_array()
            .map(|a| a.iter().map(state).collect())
            .unwrap_or_default();
        let feedback_type = str_of(&c["feedback_type"]);
        let value = f64_bits(&c["value"]);
        assert_eq!(
            match_feedback_type_expressions(&exprs, feedback_type, value),
            c["match"].as_bool().expect("bool"),
            "MatchFeedbackTypeExpressions({exprs:?}, {feedback_type:?}, {value:?})"
        );
    }
}

#[test]
fn expression_unmarshal_json_matches_go() {
    let g = golden();
    for c in cases(&g, "unmarshal_json") {
        let data = hex::decode(str_of(&c["json_hex"])).expect("hex");
        let mut e = state(&c["start"]);
        let result = e.unmarshal_json(&data);
        let shown = String::from_utf8_lossy(&data);
        assert_eq!(
            result.as_ref().err().map(ToString::to_string).as_deref(),
            error_of(&c["error"]),
            "error of UnmarshalJSON({shown})"
        );
        let want = state(&c["state"]);
        assert!(
            same(&e, &want),
            "UnmarshalJSON({shown}) from {:?}: got {e:?}, Go {want:?}",
            c["start"]
        );
    }
}

/// `encoding/json` of `[]FeedbackTypeExpression`, the shape of every Gorse
/// field, against serde. serde_json appends the position to an error message;
/// the rest must be Go's text.
#[test]
fn expression_slice_json_matches_go() {
    let g = golden();
    for c in cases(&g, "decode_slice") {
        let json = str_of(&c["json"]);
        let result: Result<Vec<FeedbackTypeExpression>, _> = serde_json::from_str(json);
        match (result, error_of(&c["error"])) {
            (Ok(got), None) => {
                let want: Vec<_> = c["result"]
                    .as_array()
                    .expect("result")
                    .iter()
                    .map(state)
                    .collect();
                assert_eq!(got.len(), want.len(), "decode {json}");
                for (a, b) in got.iter().zip(&want) {
                    assert!(same(a, b), "decode {json}: got {a:?}, Go {b:?}");
                }
            }
            (Err(err), Some(go)) => {
                let expected = format!("{go} at line {} column {}", err.line(), err.column());
                assert_eq!(err.to_string(), expected, "decode {json}");
            }
            (got, go) => panic!("decode {json}: got {got:?}, Go error {go:?}"),
        }
    }
    for c in cases(&g, "encode_slice") {
        let exprs: Vec<_> = c["exprs"]
            .as_array()
            .expect("exprs")
            .iter()
            .map(state)
            .collect();
        let go: Json = serde_json::from_str(str_of(&c["json"])).expect("Go JSON");
        // serde_json does not escape `<`, `>` and `&` as Go does, so compare
        // the decoded documents; the escaped bytes are MarshalJSON's test.
        let rust: Json = serde_json::to_value(&exprs).expect("serialize");
        assert_eq!(rust, go, "encode {exprs:?}");
        let joined: Vec<_> = exprs
            .iter()
            .map(FeedbackTypeExpression::marshal_json)
            .collect();
        assert_eq!(format!("[{}]", joined.join(",")), str_of(&c["json"]));
    }
}

#[test]
fn expression_must_parse_matches_go() {
    let g = golden();
    for c in cases(&g, "must_parse") {
        let input = str_of(&c["input"]).to_owned();
        let result = std::panic::catch_unwind(|| must_parse_feedback_type_expression(&input));
        match (result, error_of(&c["panic"])) {
            (Ok(got), None) => {
                let want = state(&c["state"]);
                assert!(same(&got, &want), "MustParse({input:?}): got {got:?}");
            }
            (Err(payload), Some(go)) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .expect("panic message");
                assert_eq!(message, go, "panic of MustParse({input:?})");
            }
            (Ok(got), Some(go)) => panic!("MustParse({input:?}) gave {got:?}, Go panicked: {go}"),
            (Err(_), None) => panic!("MustParse({input:?}) panicked, Go did not"),
        }
    }
}

/// Every parse error Go reports is one of the two `FromString` errors, and
/// `unmarshal_json` wraps them like Go's `%w`.
#[test]
fn expression_errors_keep_their_kind() {
    let mut e = FeedbackTypeExpression::default();
    assert!(matches!(
        e.from_string("1a"),
        Err(expression::Error::InvalidFormat)
    ));
    assert!(matches!(
        e.from_string("a<."),
        Err(expression::Error::InvalidValue(_))
    ));
    match e.unmarshal_json(br#""1a""#) {
        Err(expression::Error::Unmarshal(inner)) => {
            assert!(matches!(*inner, expression::Error::InvalidFormat));
        }
        other => panic!("unexpected {other:?}"),
    }
}
