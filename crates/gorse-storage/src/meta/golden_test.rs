// Ports: storage/meta/database.go and sqlite.go @ add21b5, checked against the
// golden data of contracts/tools/metagolden (recorded with Go 1.27.0).

//! The meta store checked against what the Go implementation produces: the
//! text a node's update time is stored as and read back from, `ToJSON`, and
//! `FromJSON` with each parameter's Go type.
//!
//! One difference from Go is recorded in `porting/notes/WP-2-11.md` and is
//! checked here, not left out: a nil `Params` encodes as `{}` where Go writes
//! `null`. Arrays and objects decode and encode again byte for byte, as Go's
//! do.

use gorse_common::gojson::Value;
use gorse_common::gotime::{self, Duration, Month, SECOND, Time};
use gorse_common::jsonutil;
use gorse_model_core::params::{ParamValue, Params};
use serde::Deserialize;

use super::database_test::params;
use super::sqlite::{format_time, parse_time};
use super::{Model, Node, open};

/// The golden data, written by `contracts/tools/metagolden`.
const GOLDEN: &str = include_str!("../../tests/data/meta_golden.json");

/// The cases whose Go model has a nil `Params`. Go writes them as `null`; this
/// port writes an empty map, `{}`.
const NIL_PARAMS: [&str; 2] = ["int_score", "nil_params"];

#[derive(Deserialize)]
struct Golden {
    go_version: String,
    times: Vec<TimeCase>,
    to_json: Vec<JsonCase>,
    from_json: Vec<FromCase>,
    round_trip: Vec<RoundTripCase>,
}

#[derive(Deserialize)]
struct TimeCase {
    name: String,
    date: [i64; 7],
    written: String,
    stored: String,
    read: String,
}

#[derive(Deserialize)]
struct JsonCase {
    name: String,
    json: String,
}

#[derive(Deserialize)]
struct RoundTripCase {
    name: String,
    input: String,
    json: String,
}

#[derive(Deserialize)]
struct ParamResult {
    name: String,
    go_type: String,
    value: String,
}

#[derive(Deserialize)]
struct FromCase {
    name: String,
    input: String,
    id: i64,
    r#type: String,
    params_nil: bool,
    params: Option<Vec<ParamResult>>,
    score: i64,
    err: bool,
}

fn golden() -> Golden {
    serde_json::from_str(GOLDEN).expect("meta_golden.json parses")
}

/// The UTC time with the given year, month, day, hour, minute, second and
/// nanosecond.
fn time_at(date: [i64; 7]) -> Time {
    let [year, month, day, hour, min, sec, nsec] = date;
    gotime::date(
        year,
        Month(month),
        day,
        hour,
        min,
        sec,
        nsec,
        &gotime::utc(),
    )
}

/// The stored text of the golden time called `name`.
fn stored_text(golden: &Golden, name: &str) -> String {
    golden
        .times
        .iter()
        .find(|case| case.name == name)
        .expect("the golden time exists")
        .stored
        .clone()
}

#[test]
fn update_time_text_matches_go() {
    let golden = golden();
    assert_eq!(golden.times.len(), 5, "{}", golden.go_version);
    for case in &golden.times {
        let time = time_at(case.date);
        assert_eq!(format_time(&time), case.stored, "{}", case.name);
        assert_eq!(time.utc().string(), case.written, "{}", case.name);
        let read = parse_time(&case.stored).expect("the stored text parses");
        assert_eq!(read.string(), case.read, "{}", case.name);
    }
}

/// The Rust model for each `to_json` case, built the way the Go generator
/// builds it.
fn rust_json(name: &str) -> String {
    let json = match name {
        "int_score" => Model {
            id: 7,
            r#type: "als".to_owned(),
            params: Params::default(),
            score: 42_i64,
        }
        .to_json(),
        "params" => Model {
            id: 7,
            r#type: "als".to_owned(),
            params: params(&[
                ("Lr", ParamValue::Float64(0.007)),
                ("NFactors", ParamValue::Int(80)),
                ("HiddenLayers", ParamValue::IntSlice(vec![64, 32])),
                ("Optimizer", ParamValue::String("adam".to_owned())),
                ("AutoScale", ParamValue::Bool(true)),
                ("Missing", ParamValue::Nil),
                ("Alpha", ParamValue::Float32(0.007)),
                ("BatchSize", ParamValue::Int64(256)),
                ("InitStdDev", ParamValue::Float32(0.1)),
                ("RandomState", ParamValue::Int32(-3)),
            ]),
            score: 42_i64,
        }
        .to_json(),
        "number_forms" => Model {
            id: 1,
            r#type: "forms".to_owned(),
            params: params(&[
                ("big", ParamValue::Float64(1e21)),
                ("small", ParamValue::Float64(1e-7)),
                ("plain", ParamValue::Float64(123_456_789.0)),
                // Go folds 0.1 + 0.2 as a constant, so it is 0.3.
                ("sum", ParamValue::Float64(0.3)),
                ("negzero", ParamValue::Float64(-0.0)),
                ("tiny32", ParamValue::Float32(1e-7)),
                ("big32", ParamValue::Float32(1e21)),
            ]),
            score: 0.5_f64,
        }
        .to_json(),
        "string_escapes" => Model {
            id: 2,
            r#type: "<a&b>\u{2028}\u{2029}\"\\\n\t\u{1}é".to_owned(),
            params: params(&[(
                "s",
                ParamValue::String("<a&b>\u{2028}\u{2029}\"\\\n\t\u{1}é\u{8}\u{c}\r".to_owned()),
            )]),
            score: 0_i64,
        }
        .to_json(),
        "empty_params" => Model {
            id: 3,
            r#type: "none".to_owned(),
            params: Params::default(),
            score: 0_i64,
        }
        .to_json(),
        "nil_params" => Model {
            id: 4,
            r#type: "nil".to_owned(),
            params: Params::default(),
            score: 0_i64,
        }
        .to_json(),
        other => panic!("no Rust model for the golden case {other}"),
    };
    json.expect("the model encodes")
}

#[test]
fn to_json_matches_go() {
    let golden = golden();
    assert_eq!(golden.to_json.len(), 6, "{}", golden.go_version);
    for case in &golden.to_json {
        let expected = if NIL_PARAMS.contains(&case.name.as_str()) {
            case.json.replace("\"Params\":null", "\"Params\":{}")
        } else {
            case.json.clone()
        };
        assert_eq!(rust_json(&case.name), expected, "{}", case.name);
    }
}

/// The model each `from_json` case starts from. Go's zero model has a nil
/// `Params`; this port's zero model has an empty one.
fn start(name: &str) -> Model<i64> {
    match name {
        "absent_fields_keep_values"
        | "null_params_clears"
        | "null_fields_are_noops"
        | "top_level_null_is_noop" => Model {
            id: 5,
            r#type: "old".to_owned(),
            params: params(&[
                ("keep", ParamValue::Int(1)),
                ("Lr", ParamValue::Float64(0.1)),
            ]),
            score: 9,
        },
        _ => Model::default(),
    }
}

/// The name of a parameter's Go type, as `%T` prints it.
fn go_type(value: &ParamValue) -> &str {
    match value {
        ParamValue::Nil => "<nil>",
        ParamValue::Int(_) => "int",
        ParamValue::Int32(_) => "int32",
        ParamValue::Int64(_) => "int64",
        ParamValue::Float32(_) => "float32",
        ParamValue::Float64(_) => "float64",
        ParamValue::Bool(_) => "bool",
        ParamValue::String(_) => "string",
        ParamValue::IntSlice(_) => "[]int",
        ParamValue::Json(Value::Object(_)) => "map[string]interface {}",
        ParamValue::Json(_) => "[]interface {}",
        ParamValue::Other(name) => name,
    }
}

/// The parameter's value as `json.Marshal` writes it, or `None` for a value
/// that has no Rust form ([`ParamValue::Other`], which Gorse never stores).
fn go_value_json(value: &ParamValue) -> Option<String> {
    let json = match value {
        ParamValue::Nil => return Some("null".to_owned()),
        ParamValue::Int(v) | ParamValue::Int64(v) => jsonutil::must_marshal(v),
        ParamValue::Int32(v) => jsonutil::must_marshal(v),
        ParamValue::Float32(v) => jsonutil::must_marshal(v),
        ParamValue::Float64(v) => jsonutil::must_marshal(v),
        ParamValue::Bool(v) => jsonutil::must_marshal(v),
        ParamValue::String(v) => jsonutil::must_marshal(v),
        ParamValue::IntSlice(v) => jsonutil::must_marshal(v),
        ParamValue::Json(_) => jsonutil::must_marshal(&super::ParamValueJson(value)),
        ParamValue::Other(_) => return None,
    };
    Some(json)
}

/// Checks the parameters against Go's, name by name, in Go's order.
fn assert_params(actual: &Params, expected: &[ParamResult], case: &str) {
    let names: Vec<&str> = actual.0.keys().map(String::as_str).collect();
    let want: Vec<&str> = expected.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, want, "{case}: parameter names");
    for ((name, value), want) in actual.0.iter().zip(expected) {
        assert_eq!(go_type(value), want.go_type, "{case}: {name} type");
        if let Some(json) = go_value_json(value) {
            assert_eq!(json, want.value, "{case}: {name} value");
        }
    }
}

#[test]
fn from_json_matches_go() {
    let golden = golden();
    assert_eq!(golden.from_json.len(), 10, "{}", golden.go_version);
    for case in &golden.from_json {
        assert_eq!(case.params_nil, case.params.is_none(), "{}", case.name);
        let mut model = start(&case.name);
        let result = model.from_json(&case.input);
        assert_eq!(result.is_err(), case.err, "{}", case.name);
        if case.err {
            continue;
        }
        assert_eq!(model.id, case.id, "{}", case.name);
        assert_eq!(model.r#type, case.r#type, "{}", case.name);
        assert_eq!(model.score, case.score, "{}", case.name);
        match &case.params {
            // Go's nil map, which this port holds as an empty one.
            None => assert!(model.params.0.is_empty(), "{}", case.name),
            Some(expected) => assert_params(&model.params, expected, &case.name),
        }
    }
}

#[test]
fn round_trip_matches_go() {
    let golden = golden();
    assert_eq!(golden.round_trip.len(), 8, "{}", golden.go_version);
    for case in &golden.round_trip {
        let mut model: Model<i64> = Model::default();
        model
            .from_json(&case.input)
            .unwrap_or_else(|err| panic!("{}: {err}", case.name));
        assert_eq!(
            model.to_json().expect("the model encodes"),
            case.json,
            "{}",
            case.name
        );
    }
}

#[tokio::test]
async fn update_time_is_stored_as_go_stores_it() {
    let golden = golden();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("meta.sqlite3");
    let db = open(&format!("sqlite://{}", file.display()), SECOND)
        .await
        .unwrap();
    db.init().await.unwrap();
    for case in &golden.times {
        let node = Node {
            uuid: case.name.clone(),
            hostname: "localhost".to_owned(),
            r#type: "master".to_owned(),
            version: "v0.1.0".to_owned(),
            update_time: time_at(case.date),
        };
        db.update_node(&node).await.unwrap();
    }
    let raw = rusqlite::Connection::open(&file).unwrap();
    for case in &golden.times {
        let stored: String = raw
            .query_row(
                "SELECT CAST(update_time AS TEXT) FROM nodes WHERE uuid = ?",
                [&case.name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, case.stored, "{}", case.name);
    }
    db.close().await.unwrap();
}

#[tokio::test]
async fn go_written_rows_are_listed_and_pruned() {
    let golden = golden();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("meta.sqlite3");
    // Ten years: "fraction" (2026) is within it and "whole_seconds" (1996) is
    // not, whatever day the test runs.
    let ttl = Duration(10 * 365 * 24 * 3600 * SECOND.0);
    let db = open(&format!("sqlite://{}", file.display()), ttl)
        .await
        .unwrap();
    db.init().await.unwrap();
    // Rows as Go wrote them, with the update times Go stored.
    let raw = rusqlite::Connection::open(&file).unwrap();
    for (uuid, time) in [("old", "whole_seconds"), ("new", "fraction")] {
        raw.execute(
            "INSERT INTO nodes (uuid, hostname, type, version, update_time) VALUES (?, 'localhost', 'master', 'v0.1.0', ?)",
            rusqlite::params![uuid, stored_text(&golden, time)],
        )
        .unwrap();
    }
    let nodes = db.list_nodes().await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].uuid, "new");
    let fraction = golden
        .times
        .iter()
        .find(|case| case.name == "fraction")
        .unwrap();
    assert_eq!(nodes[0].update_time.string(), fraction.read);
    // The outdated row was deleted by the same call.
    let rows: i64 = raw
        .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    db.close().await.unwrap();
}
