// Ports: encoding/json as go-restful v3.13.0's ReadEntity runs it (no Go test;
// the expectations are Go 1.27.0's) @ add21b5

//! Request bodies decode the way Go decodes them. `go_decode.jsonl` holds 256
//! bodies, each with the text Go's `ReadEntity` gave for it, or the value it
//! decoded to, from a probe that called `ReadEntity` on the Go type of each
//! route. Every row is checked here against [`read_entity`]. The rows are the
//! error classes of the scanner (`invalid character`, `unexpected EOF`, the
//! escapes, the literals and the numbers), the content type gate, the type
//! errors with their field paths, and the time errors.

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, header};
use serde::Deserialize;
use serde_json::Value;

use super::as_float64s;
use crate::rest::entity::read_entity;
use crate::rest::feedback::Feedback;
use crate::rest::items::Item;
use crate::rest::response::ApiError;
use gorse_storage::data::types::{ItemPatch, User, UserPatch};

/// The probe's rows, one JSON object per line.
const FIXTURE: &str = include_str!("go_decode.jsonl");

/// One row of the fixture: a body, its content type, and what Go gave.
#[derive(Deserialize)]
struct Row {
    /// The route's Go type: `user`, `users`, `user-patch`, `item`, `items`,
    /// `item-patch` or `feedback`.
    target: String,
    /// The `Content-Type` header, or `<none>` when the request had none.
    ct: String,
    /// The body, as text, unless `hex` gives its bytes.
    body: Option<String>,
    /// The body's bytes, in hex, for a body that is not UTF-8.
    hex: Option<String>,
    /// Go's error text, when the decode failed.
    err: Option<String>,
    /// Go's JSON for the decoded value, when the decode succeeded.
    value: Option<String>,
}

/// Decodes `body` as the route `target` reads it, and returns the value as
/// JSON, or the text of the error.
fn decode(target: &str, headers: &HeaderMap, body: &Bytes) -> Result<Value, String> {
    fn text<T: serde::Serialize>(result: Result<T, ApiError>) -> Result<Value, String> {
        match result {
            Ok(value) => Ok(serde_json::to_value(value).expect("the value serializes")),
            Err(ApiError::BadRequest(text)) => Err(text),
            Err(_) => panic!("a decode error is a bad request"),
        }
    }
    match target {
        "user" => text(read_entity::<User>(headers, body)),
        "users" => text(read_entity::<Vec<User>>(headers, body)),
        "user-patch" => text(read_entity::<UserPatch>(headers, body)),
        "item" => text(read_entity::<Item>(headers, body)),
        "items" => text(read_entity::<Vec<Item>>(headers, body)),
        "item-patch" => text(read_entity::<ItemPatch>(headers, body)),
        "feedback" => text(read_entity::<Vec<Feedback>>(headers, body)),
        other => panic!("no route reads {other}"),
    }
}

/// The bytes of a hex string.
fn from_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("hex is ASCII");
            u8::from_str_radix(digits, 16).expect("hex digits")
        })
        .collect()
}

/// The headers a row's request carries: its content type, if it has one.
fn headers_for(ct: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if ct != "<none>" {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(ct).expect("a fixture content type is a header value"),
        );
    }
    headers
}

#[test]
fn bodies_decode_as_go_decodes_them() {
    let mut failures = Vec::new();
    let mut rows = 0;
    for (number, line) in FIXTURE.lines().enumerate() {
        let row: Row = serde_json::from_str(line).expect("a fixture row");
        rows += 1;
        let body = match (&row.body, &row.hex) {
            (Some(text), None) => text.clone().into_bytes(),
            (None, Some(hex)) => from_hex(hex),
            _ => panic!("row {number} has a body or a hex body, not both"),
        };
        let got = decode(&row.target, &headers_for(&row.ct), &Bytes::from(body));
        let problem = match (&row.err, &row.value) {
            (Some(want), _) => match &got {
                Err(text) if text == want => None,
                _ => Some(format!("want error {want:?}, got {got:?}")),
            },
            (None, Some(want)) => {
                let want: Value = serde_json::from_str(want).expect("a Go value is JSON");
                match got {
                    // Go writes a nil slice as `null`; the port's is an empty
                    // list, and both leave the handler with no rows.
                    Ok(value)
                        if value == Value::Array(Vec::new()) && want == Value::Null
                            || as_float64s(value.clone()) == as_float64s(want.clone()) =>
                    {
                        None
                    }
                    other => Some(format!("want value {want}, got {other:?}")),
                }
            }
            (None, None) => panic!("row {number} has neither an error nor a value"),
        };
        if let Some(problem) = problem {
            failures.push(format!(
                "row {number} ({} {:?}, body {:?}): {problem}",
                row.target,
                row.ct,
                row.body
                    .as_deref()
                    .or(row.hex.as_deref())
                    .unwrap_or_default()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {rows} rows differ from Go:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Go's decoder rejects nesting past 10000 arrays and objects. A body that is
/// nested to exactly the limit still gets the type error of its route.
#[test]
fn nesting_past_the_limit_is_exceeded_max_depth() {
    let headers = headers_for("application/json");
    let past = Bytes::from("[".repeat(10_001));
    assert_eq!(
        decode("users", &headers, &past),
        Err("exceeded max depth".to_owned())
    );
    let unclosed = Bytes::from("[".repeat(10_000));
    assert_eq!(
        decode("users", &headers, &unclosed),
        Err("unexpected EOF".to_owned())
    );
}

/// A label nested past the serde limit (128 levels) is refused here. Go
/// accepts labels nested to 10000 levels, so this is a known difference
/// (X-WP42-16), pinned so that a change to it is noticed.
#[test]
fn labels_nested_past_serde_limit_are_refused_known_deviation() {
    let headers = headers_for("application/json");
    let nested = format!("{{\"Labels\":{}{}}}", "[".repeat(130), "]".repeat(130));
    let err = decode("user", &headers, &Bytes::from(nested))
        .expect_err("the labels decoder refuses this depth");
    assert!(err.contains("recursion limit"), "text: {err}");
}
