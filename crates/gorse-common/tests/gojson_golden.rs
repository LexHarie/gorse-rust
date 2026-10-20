//! `gojson` against `tests/data/gojson_golden.json`, which
//! `contracts/tools/storagegolden` wrote by running go1.27.0's
//! `json.Unmarshal` into a `[]any` (as Gorse's SQLite `json_contains`
//! decodes its arguments), into an `any` (as Gorse decodes labels) and into
//! a `map[string]any` (as viper's JSON codec decodes config files), over
//! hand-picked edge cases, nesting around the depth limit, numbers past 800
//! digits, seeded random documents and objects, mutations of them and byte
//! soup.
//!
//! Each decoded value is compared as Go's golden writes it: a flat token
//! list with the bits of every float. Each error is compared by its exact
//! text.

mod common;

use gorse_common::gojson::{Value, unmarshal_any, unmarshal_map_any, unmarshal_slice_any};
use gorse_common::gostrconv::quote;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use common::load_golden;

/// A Go string: a JSON string when it is valid UTF-8, else its bytes in hex.
#[derive(Deserialize)]
#[serde(untagged)]
enum GoString {
    Str(String),
    Hex { hex: String },
}

impl GoString {
    fn bytes(&self) -> Vec<u8> {
        match self {
            GoString::Str(s) => s.as_bytes().to_vec(),
            GoString::Hex { hex } => hex::decode(hex).expect("hex"),
        }
    }
}

#[derive(Deserialize)]
struct Golden {
    go_version: String,
    json_decode: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    #[serde(rename = "in")]
    input: GoString,
    err: Option<GoString>,
    nil: Option<bool>,
    value: Option<Vec<Json>>,
    any_err: Option<GoString>,
    any_value: Option<Vec<Json>>,
    map_err: Option<GoString>,
    map_nil: Option<bool>,
    map_value: Option<Vec<Json>>,
}

/// The golden tool's `canon`: a pre-order token list, objects with their
/// names sorted. Iterative, since values nest 10000 deep.
fn tokens(root: &Value) -> Vec<Json> {
    enum Item<'a> {
        Value(&'a Value),
        Name(&'a str),
    }
    let mut out = Vec::new();
    let mut pending = vec![Item::Value(root)];
    while let Some(item) = pending.pop() {
        let v = match item {
            Item::Name(name) => {
                out.push(json!(["key", name]));
                continue;
            }
            Item::Value(v) => v,
        };
        match v {
            Value::Null => out.push(json!(["null"])),
            Value::Bool(b) => out.push(json!(["bool", b])),
            Value::Number(f) => out.push(json!(["num", format!("{:016x}", f.to_bits())])),
            Value::String(s) => out.push(json!(["str", s])),
            Value::Array(items) => {
                out.push(json!(["arr", items.len()]));
                pending.extend(items.iter().rev().map(Item::Value));
            }
            Value::Object(members) => {
                out.push(json!(["obj", members.len()]));
                // Go's golden sorts the names (sort.Strings: byte order).
                let mut sorted: Vec<_> = members.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                for (name, v) in sorted.into_iter().rev() {
                    pending.push(Item::Value(v));
                    pending.push(Item::Name(name));
                }
            }
        }
    }
    out
}

fn slice_tokens(items: Vec<Value>) -> Vec<Json> {
    tokens(&Value::Array(items))
}

/// Compares token lists, reporting the first difference rather than both
/// lists, which can be 20000 tokens long.
fn check_tokens(what: &str, input: &[u8], got: &[Json], want: &[Json]) -> Result<(), String> {
    if let Some(i) = (0..got.len().max(want.len())).find(|&i| got.get(i) != want.get(i)) {
        return Err(format!(
            "{what} of {}: token {i} is {:?}, Go has {:?}",
            quote(input),
            got.get(i),
            want.get(i)
        ));
    }
    Ok(())
}

fn check_case(c: &Case) -> Result<(), String> {
    let input = c.input.bytes();
    let q = quote(&input);

    match (unmarshal_slice_any(&input), &c.err, c.nil, &c.value) {
        (Err(e), Some(want), None, None) => {
            if e.to_string().as_bytes() != want.bytes() {
                return Err(format!(
                    "[]any of {q}: error {:?}, Go has {:?}",
                    e.to_string(),
                    quote(want.bytes())
                ));
            }
        }
        (Ok(None), None, Some(true), None) => {}
        (Ok(Some(items)), None, None, Some(want)) => {
            check_tokens("[]any", &input, &slice_tokens(items), want)?;
        }
        (got, ..) => {
            return Err(format!(
                "[]any of {q}: got {}, Go has err={:?} nil={:?} value={}",
                match got {
                    Ok(None) => "nil".to_owned(),
                    Ok(Some(_)) => "a value".to_owned(),
                    Err(e) => format!("error {e:?}"),
                },
                c.err.as_ref().map(|e| quote(e.bytes())),
                c.nil,
                c.value.is_some(),
            ));
        }
    }

    match (
        unmarshal_map_any(&input),
        &c.map_err,
        c.map_nil,
        &c.map_value,
    ) {
        (Err(e), Some(want), None, None) => {
            if e.to_string().as_bytes() != want.bytes() {
                return Err(format!(
                    "map of {q}: error {:?}, Go has {:?}",
                    e.to_string(),
                    quote(want.bytes())
                ));
            }
        }
        (Ok(None), None, Some(true), None) => {}
        (Ok(Some(members)), None, None, Some(want)) => {
            check_tokens("map", &input, &tokens(&Value::Object(members)), want)?;
        }
        (got, ..) => {
            return Err(format!(
                "map of {q}: got {}, Go has err={:?} nil={:?} value={}",
                match got {
                    Ok(None) => "nil".to_owned(),
                    Ok(Some(_)) => "a value".to_owned(),
                    Err(e) => format!("error {e:?}"),
                },
                c.map_err.as_ref().map(|e| quote(e.bytes())),
                c.map_nil,
                c.map_value.is_some(),
            ));
        }
    }

    match (unmarshal_any(&input), &c.any_err, &c.any_value) {
        (Err(e), Some(want), None) => {
            if e.to_string().as_bytes() != want.bytes() {
                return Err(format!(
                    "any of {q}: error {:?}, Go has {:?}",
                    e.to_string(),
                    quote(want.bytes())
                ));
            }
        }
        (Ok(v), None, Some(want)) => check_tokens("any", &input, &tokens(&v), want)?,
        (got, ..) => {
            return Err(format!(
                "any of {q}: got {}, Go has err={:?} value={}",
                match got {
                    Ok(_) => "a value".to_owned(),
                    Err(e) => format!("error {e:?}"),
                },
                c.any_err.as_ref().map(|e| quote(e.bytes())),
                c.any_value.is_some(),
            ));
        }
    }
    Ok(())
}

#[test]
fn unmarshal_matches_go() {
    let golden: Golden = load_golden("gojson_golden.json");
    assert_eq!(golden.go_version, "go1.27.0");
    assert!(golden.json_decode.len() > 3100, "corpus too small");
    let failures: Vec<String> = golden
        .json_decode
        .iter()
        .filter_map(|c| check_case(c).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Go:\n{}",
        failures.len(),
        golden.json_decode.len(),
        failures.join("\n")
    );
}
