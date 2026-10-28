// Checks the go-toml v2.2.4 port (viper v1.21.0's TOML codec) against Go's results from contracts/tools/tomlgolden

//! The TOML decoder against what Go's go-toml v2.2.4 (viper's codec) does
//! with every document of `toml_golden.json`, written by
//! `contracts/tools/tomlgolden`: the decoded map with the Go type of every
//! value, or the error's text, with the line, column and context of a
//! `*toml.DecodeError`.

use std::fmt::Write as _;

use gorse_config::formats::toml::{self, Error};
use gorse_config::value::{Map, Value};
use serde_json::{Value as Json, json};

fn load() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/toml_golden.json");
    let text = std::fs::read_to_string(path).expect("read the golden file");
    serde_json::from_str(&text).expect("parse the golden file")
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// A golden text field, or its `_hex` twin when Go's bytes are not UTF-8.
fn bytes_of(case: &Json, field: &str) -> Vec<u8> {
    match case.get(format!("{field}_hex")).and_then(Json::as_str) {
        Some(h) if !h.is_empty() => unhex(h),
        _ => case[field].as_str().unwrap_or_default().as_bytes().to_vec(),
    }
}

/// The value tree in the golden's form: pre-order nodes, map entries
/// sorted by key. Iterative, as values can nest deeply.
fn flatten(m: &Map) -> Vec<Json> {
    let mut out = Vec::new();
    let mut stack: Vec<(Option<&str>, &Value)> = Vec::new();
    // The root, a map[string]interface {}.
    out.push(json!({"t": "map[string]interface {}", "n": m.len()}));
    let mut entries: Vec<(&str, &Value)> = m.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in entries.into_iter().rev() {
        stack.push((Some(k), v));
    }
    while let Some((k, v)) = stack.pop() {
        let mut node = serde_json::Map::new();
        if let Some(k) = k {
            node.insert("k".into(), json!(k));
        }
        node.insert("t".into(), json!(v.type_name()));
        match v {
            Value::Int64(i) => {
                node.insert("v".into(), json!(i.to_string()));
            }
            Value::Float64(f) => {
                node.insert("v".into(), json!(format!("{:016x}", f.to_bits())));
            }
            Value::Bool(b) => {
                node.insert("v".into(), json!(b));
            }
            Value::String(s) => {
                node.insert("v".into(), json!(s));
            }
            Value::Time(t) => {
                let (zone, offset) = t.zone();
                node.insert("v".into(), json!(t.to_string()));
                node.insert(
                    "time".into(),
                    json!({"zone": zone, "offset": offset, "location": t.location().name()}),
                );
            }
            Value::LocalDate(d) => {
                node.insert(
                    "v".into(),
                    json!({"year": d.year, "month": d.month, "day": d.day}),
                );
            }
            Value::LocalTime(t) => {
                node.insert(
                    "v".into(),
                    json!({"hour": t.hour, "minute": t.minute, "second": t.second,
                        "nanosecond": t.nanosecond, "precision": t.precision}),
                );
            }
            Value::LocalDateTime(d, t) => {
                node.insert(
                    "v".into(),
                    json!({"year": d.year, "month": d.month, "day": d.day,
                        "hour": t.hour, "minute": t.minute, "second": t.second,
                        "nanosecond": t.nanosecond, "precision": t.precision}),
                );
            }
            Value::Slice(elems) => {
                node.insert("n".into(), json!(elems.len()));
                for e in elems.iter().rev() {
                    stack.push((None, e));
                }
            }
            Value::Map(m) => {
                node.insert("n".into(), json!(m.len()));
                let mut entries: Vec<(&str, &Value)> = m.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                for (k, v) in entries.into_iter().rev() {
                    stack.push((Some(k), v));
                }
            }
            other => panic!("go-toml never makes a {}", other.type_name()),
        }
        out.push(Json::Object(node));
    }
    out
}

/// What differs between the golden case and the Rust result, if anything.
fn check(case: &Json) -> Option<String> {
    let input = bytes_of(case, "input");
    let got = toml::unmarshal(&input);
    if let Some(p) = case.get("panic") {
        return Some(format!("Go panicked ({p}), Rust returned {got:?}"));
    }
    match (case.get("value"), got) {
        (Some(want), Ok(map)) => {
            let want = want.as_array().expect("nodes");
            let have = flatten(&map);
            if have.len() != want.len() {
                return Some(format!("{} nodes, want {}", have.len(), want.len()));
            }
            for (i, (h, w)) in have.iter().zip(want).enumerate() {
                if h != w {
                    return Some(format!("node {i}: got {h}, want {w}"));
                }
            }
            drop_deep(map);
            None
        }
        (Some(_), Err(err)) => Some(format!("got error {err:?}, want a value")),
        (None, Ok(_)) => Some(format!("got a value, want error {}", case["error"])),
        (None, Err(err)) => {
            let want = case["error"].as_str().expect("error");
            if err.to_string() != want {
                return Some(format!("error {:?}, want {want:?}", err.to_string()));
            }
            match (case.get("decode_error"), err) {
                (Some(de), Error::Decode(got)) => {
                    let want_pos = (
                        de["line"].as_i64().expect("line"),
                        de["column"].as_i64().expect("column"),
                    );
                    if got.position() != want_pos {
                        return Some(format!("position {:?}, want {want_pos:?}", got.position()));
                    }
                    let want_human = bytes_of(de, "human");
                    if got.human() != want_human.as_slice() {
                        return Some(format!(
                            "human\n{}\nwant\n{}",
                            String::from_utf8_lossy(got.human()),
                            String::from_utf8_lossy(&want_human)
                        ));
                    }
                    None
                }
                (None, Error::Other(_)) => None,
                (want, got) => Some(format!(
                    "error kind {got:?}, want decode error {}",
                    want.is_some()
                )),
            }
        }
    }
}

/// Drops a decoded map without recursing (some documents nest ten thousand
/// levels deep, and the compiler's drop of a `Value` recurses).
fn drop_deep(m: Map) {
    let mut stack: Vec<Value> = m.into_iter().map(|(_, v)| v).collect();
    while let Some(v) = stack.pop() {
        match v {
            Value::Slice(items) => stack.extend(items),
            Value::Map(m) => stack.extend(m.into_iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

#[test]
fn toml_golden() {
    // This runs on the test thread's default stack: the decoder grows its
    // stack where Go's would grow.
    let golden = load();
    let cases = golden["cases"].as_array().expect("cases");
    let mut failures = 0;
    let mut report = String::new();
    for case in cases {
        if let Some(diff) = check(case) {
            failures += 1;
            if failures <= 30 {
                let _ = writeln!(
                    report,
                    "{}: {:?}\n  {diff}",
                    case["name"],
                    String::from_utf8_lossy(&bytes_of(case, "input"))
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
            }
        }
    }
    let total = cases.len();
    assert!(total > 10_000, "only {total} cases");
    assert_eq!(failures, 0, "{failures} of {total} cases differ:\n{report}");
}
