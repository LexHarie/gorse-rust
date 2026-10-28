// Ports: the golden cases of contracts/tools/yamlgolden (viper v1.21.0's YAML codec over go.yaml.in/yaml/v3 v3.0.4) @ add21b5

//! Replays every case of `tests/data/yaml_golden.json` against
//! `formats::yaml::decode`. A case either matches Go exactly or is listed in
//! one of the deviation tables below, which assert what the port does
//! instead (see `porting/notes/WP-1-15-yaml.md`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Mutex;

use gorse_common::gotime;
use gorse_config::formats::yaml::decode;
use gorse_config::value::Value;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

/// `time.Local` is process-wide; the tests that set it take this lock.
static LOCAL: Mutex<()> = Mutex::new(());

fn golden() -> Json {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/yaml_golden.json"))
        .expect("read yaml_golden.json");
    serde_json::from_str(&text).expect("parse yaml_golden.json")
}

/// The input of a case: `input`, `input_hex` or the parts of `gen`.
fn input_of(case: &Json) -> Vec<u8> {
    if let Some(s) = case.get("input").and_then(Json::as_str) {
        return s.as_bytes().to_vec();
    }
    if let Some(h) = case.get("input_hex").and_then(Json::as_str) {
        return hex::decode(h).expect("input_hex");
    }
    let mut out = Vec::new();
    for part in case["gen"].as_array().expect("gen") {
        let s = part["s"].as_str().expect("part s");
        let n = part["n"].as_u64().expect("part n");
        for _ in 0..n {
            out.extend_from_slice(s.as_bytes());
        }
    }
    out
}

/// Decodes `input` with `time.Local` set as the case says.
fn run(input: &[u8], local: Option<&str>) -> Result<gorse_config::value::Map, String> {
    let _guard = LOCAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match local {
        Some("CET") => gotime::set_local(gotime::fixed_zone("CET", 3600)),
        _ => gotime::set_local(gotime::utc()),
    }
    let out = decode(input);
    gotime::set_local(gotime::utc());
    out
}

/// The golden tool's `writeString`.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Go's `json.Marshal` of a string (HTML escaping on), for the order of
/// `map[interface {}]interface {}` entries.
fn go_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The golden tool's `Value`, built from a Rust value.
#[derive(Clone)]
enum Enc {
    Scalar(&'static str, String),
    Nil,
    List(Vec<Enc>),
    StringMap(Vec<(String, Enc)>),
    AnyMap(Vec<(Enc, Enc)>),
}

fn enc(v: &Value) -> Enc {
    match v {
        Value::Nil => Enc::Nil,
        Value::Bool(b) => Enc::Scalar("bool", b.to_string()),
        Value::Int(i) => Enc::Scalar("int", i.to_string()),
        Value::Int64(i) => Enc::Scalar("int64", i.to_string()),
        Value::Uint64(u) => Enc::Scalar("uint64", u.to_string()),
        Value::Float64(f) => Enc::Scalar("float64", format!("{:016x}", f.to_bits())),
        Value::String(s) => Enc::Scalar("string", s.clone()),
        Value::Time(t) => Enc::Scalar("time.Time", t.string()),
        Value::Slice(items) => Enc::List(items.iter().map(enc).collect()),
        Value::Map(m) => {
            let mut entries: Vec<(String, Enc)> = m.iter().map(|(k, v)| (k.to_owned(), enc(v))).collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            Enc::StringMap(entries)
        }
        Value::AnyMap(entries) => {
            let mut list: Vec<(String, String, Enc, Enc)> = entries
                .iter()
                .map(|(k, v)| {
                    let (k, v) = (enc(k), enc(v));
                    (go_json(&k), go_json(&v), k, v)
                })
                .collect();
            list.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            Enc::AnyMap(list.into_iter().map(|(_, _, k, v)| (k, v)).collect())
        }
        other => panic!("the YAML decoder does not make {other:?}"),
    }
}

fn type_of(e: &Enc) -> &'static str {
    match e {
        Enc::Scalar(t, _) => t,
        Enc::Nil => "nil",
        Enc::List(_) => "[]interface {}",
        Enc::StringMap(_) => "map[string]interface {}",
        Enc::AnyMap(_) => "map[interface {}]interface {}",
    }
}

/// `json.Marshal` of the golden tool's `Value`.
fn go_json(e: &Enc) -> String {
    let mut out = String::from("{\"t\":");
    go_json_string(&mut out, type_of(e));
    match e {
        Enc::Nil => {}
        Enc::Scalar(_, v) => {
            out.push_str(",\"v\":");
            go_json_string(&mut out, v);
        }
        Enc::List(items) => {
            out.push_str(",\"v\":[");
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&go_json(x));
            }
            out.push(']');
        }
        Enc::StringMap(entries) => {
            out.push_str(",\"v\":[");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("{\"k\":");
                go_json_string(&mut out, k);
                out.push_str(",\"v\":");
                out.push_str(&go_json(v));
                out.push('}');
            }
            out.push(']');
        }
        Enc::AnyMap(entries) => {
            out.push_str(",\"v\":[");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("{\"k\":");
                out.push_str(&go_json(k));
                out.push_str(",\"v\":");
                out.push_str(&go_json(v));
                out.push('}');
            }
            out.push(']');
        }
    }
    out.push('}');
    out
}

/// The golden tool's `writeCompact` of a Rust value.
fn compact(e: &Enc, out: &mut String) {
    out.push_str("{\"t\":");
    write_string(out, type_of(e));
    match e {
        Enc::Nil => {}
        Enc::Scalar(_, v) => {
            out.push_str(",\"v\":");
            write_string(out, v);
        }
        Enc::List(items) => {
            out.push_str(",\"v\":[");
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                compact(x, out);
            }
            out.push(']');
        }
        Enc::StringMap(entries) => {
            out.push_str(",\"v\":[");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("{\"k\":");
                write_string(out, k);
                out.push_str(",\"v\":");
                compact(v, out);
                out.push('}');
            }
            out.push(']');
        }
        Enc::AnyMap(entries) => {
            out.push_str(",\"v\":[");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("{\"k\":");
                compact(k, out);
                out.push_str(",\"v\":");
                compact(v, out);
                out.push('}');
            }
            out.push(']');
        }
    }
    out.push('}');
}

/// The golden tool's `writeCompact` of a golden `Value` (as parsed JSON).
fn compact_json(v: &Json, out: &mut String) {
    out.push_str("{\"t\":");
    write_string(out, v["t"].as_str().expect("t"));
    match v.get("v") {
        None => {}
        Some(Json::String(s)) => {
            out.push_str(",\"v\":");
            write_string(out, s);
        }
        Some(Json::Array(items)) => {
            out.push_str(",\"v\":[");
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if let Some(k) = x.get("k") {
                    out.push_str("{\"k\":");
                    match k {
                        Json::String(k) => {
                            write_string(out, k);
                            if let Some(h) = x.get("k_hex").and_then(Json::as_str) {
                                out.push_str(",\"k_hex\":");
                                write_string(out, h);
                            }
                        }
                        k => compact_json(k, out),
                    }
                    out.push_str(",\"v\":");
                    compact_json(&x["v"], out);
                    out.push('}');
                } else {
                    compact_json(x, out);
                }
            }
            out.push(']');
        }
        Some(other) => panic!("unexpected golden value {other}"),
    }
    if let Some(h) = v.get("hex").and_then(Json::as_str) {
        out.push_str(",\"hex\":");
        write_string(out, h);
    }
    out.push('}');
}

/// What a decode did, in the golden's terms: `error: <text>`, `result:
/// <compact text>` or, for long results, `result-sha256: <hash> <len>`.
fn outcome_of(result: &Result<gorse_config::value::Map, String>) -> String {
    match result {
        Err(e) => format!("error: {e}"),
        Ok(m) => {
            let mut text = String::new();
            compact(&enc(&Value::Map(m.clone())), &mut text);
            describe_result(text)
        }
    }
}

const MAX_TEXT: usize = 16384;

fn describe_result(text: String) -> String {
    if text.len() > MAX_TEXT {
        let sum = Sha256::digest(text.as_bytes());
        format!("result-sha256: {} {}", hex::encode(sum), text.len())
    } else {
        format!("result: {text}")
    }
}

/// The golden outcome of a case, in the terms of [`outcome_of`].
fn golden_outcome(case: &Json) -> String {
    if let Some(e) = case.get("error").and_then(Json::as_str) {
        return format!("error: {e}");
    }
    if let Some(p) = case.get("panic").and_then(Json::as_str) {
        return format!("panic: {p}");
    }
    if let Some(sum) = case.get("result_sha256").and_then(Json::as_str) {
        return format!("result-sha256: {sum} {}", case["result_len"]);
    }
    if let Some(text) = case.get("result_text").and_then(Json::as_str) {
        return describe_result(text.to_owned());
    }
    let mut text = String::new();
    compact_json(&case["result"], &mut text);
    describe_result(text)
}

/// Case ids: the name, with `#n` for its n-th repeat.
fn case_ids(cases: &[Json]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    cases
        .iter()
        .map(|c| {
            let name = c["name"].as_str().expect("name").to_owned();
            let n = seen.entry(name.clone()).or_default();
            *n += 1;
            if *n == 1 { name } else { format!("{name}#{}", *n - 1) }
        })
        .collect()
}

#[test]
fn golden_cases() {
    let g = golden();
    let cases = g["cases"].as_array().expect("cases");
    let ids = case_ids(cases);
    let mut mismatches = Vec::new();
    for (case, id) in cases.iter().zip(&ids) {
        let input = input_of(case);
        let local = case.get("local").and_then(Json::as_str);
        let got = outcome_of(&run(&input, local));
        let want = golden_outcome(case);
        if got != want {
            mismatches.push((id.clone(), want, got));
        }
    }
    if let Ok(path) = std::env::var("YAML_GOLDEN_REPORT") {
        let mut report = String::new();
        for (id, want, got) in &mismatches {
            let _ = writeln!(report, "{}", serde_json::json!({"id": id, "want": want, "got": got}));
        }
        std::fs::write(path, report).expect("write the report");
    }
    assert!(mismatches.is_empty(), "{} of {} cases differ", mismatches.len(), cases.len());
}
