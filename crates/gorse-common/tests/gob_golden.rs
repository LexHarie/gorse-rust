//! The encoding primitives and the gob subset against data produced by Go
//! (`contracts/tools/gobgolden`):
//!
//! - `gob_golden.json`: `encoding.WriteGob`, `WriteSlice`, `WriteString` and
//!   `WriteBytes` of every Go type Gorse gob-encodes. Rust must decode each one
//!   to the same value and, where Go's output is deterministic, encode the
//!   value to the same bytes.
//! - `gob_decode_golden.json`: hand-made gob streams, valid and corrupt, with
//!   what Go's decoder returned. Rust must return the same value, or an error
//!   with the same text. The cases named `rust encoder: ...` are streams the
//!   Rust encoder wrote; Go decoding them is the evidence that Go reads what
//!   Rust writes when the bytes cannot be compared with Go's own output.

use std::collections::BTreeMap;

use gorse_common::encoding::gob::{self, Params, Value};
use gorse_common::encoding::{self, read_gob, write_gob};
use serde_json::Value as Json;

fn load(name: &str) -> Vec<Json> {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("golden file");
    serde_json::from_str(&text).expect("golden JSON")
}

fn str_of(v: &Json) -> &str {
    v.as_str().expect("string")
}

fn int(v: &Json) -> i64 {
    str_of(v).parse().expect("decimal")
}

fn f32_bits(v: &Json) -> f32 {
    f32::from_bits(u32::from_str_radix(str_of(v), 16).expect("hex bits"))
}

fn f64_bits(v: &Json) -> f64 {
    f64::from_bits(u64::from_str_radix(str_of(v), 16).expect("hex bits"))
}

fn ints(v: &Json) -> Vec<i64> {
    match v {
        Json::Null => Vec::new(),
        Json::Array(items) => items.iter().map(int).collect(),
        _ => panic!("not a list: {v}"),
    }
}

fn params(v: &Json) -> Params {
    let mut map = BTreeMap::new();
    for entry in v.as_array().into_iter().flatten() {
        let raw = &entry["value"];
        let value = match str_of(&entry["type"]) {
            "nil" => Value::Nil,
            "int" => Value::Int(int(raw)),
            "int64" => Value::Int64(int(raw)),
            "int32" => Value::Int32(i32::try_from(int(raw)).expect("int32")),
            "float32" => Value::Float32(f32_bits(raw)),
            "float64" => Value::Float64(f64_bits(raw)),
            "bool" => Value::Bool(raw.as_bool().expect("bool")),
            "string" => Value::String(str_of(raw).to_owned()),
            "[]int" => Value::IntSlice(ints(raw)),
            other => panic!("unexpected Go type {other}"),
        };
        map.insert(str_of(&entry["key"]).to_owned(), value);
    }
    Params(map)
}

fn unhex(v: &Json) -> Vec<u8> {
    hex::decode(str_of(v)).expect("hex")
}

/// Decodes `bytes` with `read_gob` and checks the value; for deterministic
/// cases also checks that `write_gob` reproduces `bytes`.
fn check_gob<T>(name: &str, bytes: &[u8], expected: &T, deterministic: bool)
where
    T: gob::GobValue + PartialEq + std::fmt::Debug,
{
    let decoded: T = read_gob(&mut &bytes[..]).unwrap_or_else(|e| panic!("{name}: decode: {e}"));
    assert_eq!(&decoded, expected, "{name}: decoded value");
    let mut encoded = Vec::new();
    write_gob(&mut encoded, expected).unwrap_or_else(|e| panic!("{name}: encode: {e}"));
    if deterministic {
        assert_eq!(
            hex::encode(&encoded),
            hex::encode(bytes),
            "{name}: encoded bytes"
        );
    }
    // Whatever the order of the entries, the Rust encoding reads back.
    let again: T =
        read_gob(&mut encoded.as_slice()).unwrap_or_else(|e| panic!("{name}: re-decode: {e}"));
    assert_eq!(&again, expected, "{name}: re-decoded value");
}

#[test]
fn encodings_match_go() {
    let cases = load("gob_golden.json");
    assert_eq!(cases.len(), 65);
    for case in &cases {
        let name = str_of(&case["name"]);
        let value = &case["value"];
        let bytes = unhex(&case["hex"]);
        let deterministic = case["deterministic"].as_bool().expect("bool");
        match str_of(&case["kind"]) {
            "int64" | "int" => check_gob(name, &bytes, &int(value), deterministic),
            "int32" => check_gob(
                name,
                &bytes,
                &i32::try_from(int(value)).expect("int32"),
                deterministic,
            ),
            "bool" => check_gob(name, &bytes, &value.as_bool().expect("bool"), deterministic),
            "float32" => {
                let decoded: f32 = read_gob(&mut bytes.as_slice()).expect("decode");
                assert_eq!(decoded.to_bits(), f32_bits(value).to_bits(), "{name}");
                check_gob(name, &bytes, &f32_bits(value), deterministic);
            }
            "float64" => check_gob(name, &bytes, &f64_bits(value), deterministic),
            "string" => check_gob(name, &bytes, &str_of(value).to_owned(), deterministic),
            "[]int" => check_gob(name, &bytes, &ints(value), deterministic),
            "params" => check_gob(name, &bytes, &params(value), deterministic),
            "slice-f32" => {
                let expected: Vec<f32> = value
                    .as_array()
                    .expect("list")
                    .iter()
                    .map(f32_bits)
                    .collect();
                let decoded: Vec<f32> =
                    encoding::read_slice(&mut bytes.as_slice()).expect("decode");
                assert_eq!(decoded, expected, "{name}");
                let mut encoded = Vec::new();
                encoding::write_slice(&mut encoded, &expected).expect("encode");
                assert_eq!(encoded, bytes, "{name}");
            }
            "raw-string" => {
                assert_eq!(
                    encoding::read_string(&mut bytes.as_slice()).expect("decode"),
                    str_of(value)
                );
                let mut encoded = Vec::new();
                encoding::write_string(&mut encoded, str_of(value)).expect("encode");
                assert_eq!(encoded, bytes, "{name}");
            }
            "raw-bytes" => {
                let expected = unhex(value);
                assert_eq!(
                    encoding::read_bytes(&mut bytes.as_slice()).expect("decode"),
                    expected
                );
                let mut encoded = Vec::new();
                encoding::write_bytes(&mut encoded, &expected).expect("encode");
                assert_eq!(encoded, bytes, "{name}");
            }
            other => panic!("{name}: unknown kind {other}"),
        }
    }
}

/// Streams Go decodes that this port rejects on purpose, because they hold
/// values outside the subset Gorse writes (`PORTING_NOTES.md` X-8).
const UNSUPPORTED: &[&str] = &["params with uint", "invalid UTF-8 string"];

fn decode_as(target: &str, data: &[u8]) -> Result<Json, gob::Error> {
    Ok(match target {
        "int64" => Json::String(gob::decode::<i64>(data)?.to_string()),
        "int32" => Json::String(gob::decode::<i32>(data)?.to_string()),
        "float32" => Json::String(format!("{:08x}", gob::decode::<f32>(data)?.to_bits())),
        "float64" => Json::String(format!("{:016x}", gob::decode::<f64>(data)?.to_bits())),
        "bool" => Json::Bool(gob::decode::<bool>(data)?),
        "string" => Json::String(hex::encode(gob::decode::<String>(data)?)),
        "[]int" => Json::Array(
            gob::decode::<Vec<i64>>(data)?
                .iter()
                .map(|x| Json::String(x.to_string()))
                .collect(),
        ),
        "params" => {
            return Err(gob::Error::Invalid(
                "params are compared separately".to_owned(),
            ));
        }
        other => panic!("unknown target {other}"),
    })
}

#[test]
fn decoder_verdicts_match_go() {
    let cases = load("gob_decode_golden.json");
    assert!(cases.len() >= 37);
    for case in &cases {
        let name = str_of(&case["name"]);
        let target = str_of(&case["target"]);
        let data = unhex(&case["hex"]);
        let go_ok = case["ok"].as_bool().expect("bool");
        let rust = if target == "params" {
            gob::decode::<Params>(&data).map(|p| (Json::Null, Some(p)))
        } else {
            decode_as(target, &data).map(|v| (v, None))
        };
        if UNSUPPORTED.contains(&name) {
            assert!(go_ok, "{name}: Go accepts it");
            assert!(
                matches!(rust, Err(gob::Error::Unsupported(_))),
                "{name}: {rust:?}"
            );
            continue;
        }
        match (go_ok, rust) {
            (true, Ok((value, None))) => assert_eq!(value, case["value"], "{name}"),
            (true, Ok((_, Some(p)))) => {
                assert_eq!(p, params(&case["value"]), "{name}");
                // Streams the Rust encoder wrote, which Go decoded to this value.
                if name.starts_with("rust encoder:") {
                    assert_eq!(
                        hex::encode(gob::encode(&p).expect("encode")),
                        str_of(&case["hex"]),
                        "{name}"
                    );
                }
            }
            (false, Err(err)) => assert_eq!(err.to_string(), str_of(&case["error"]), "{name}"),
            (go_ok, rust) => panic!(
                "{name}: Go ok={go_ok} error={}, Rust {rust:?}",
                case["error"]
            ),
        }
    }
}
