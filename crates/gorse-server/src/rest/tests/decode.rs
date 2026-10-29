// Ports: encoding/json as go-restful v3.13.0's ReadEntity runs it (no Go test;
// the expectations are Go 1.27.0's) @ add21b5

//! Request bodies decode the way Go decodes them. The conformance crate's
//! `go_decode` test is the oracle: it runs Go's `ReadEntity` on every body and
//! compares each with [`read_entity`]. The tests here pin the cases that the
//! oracle checks by its own corpus but that need the decoder's internals to
//! state: the nesting limit, and labels whose numbers and depth outlast what
//! serde keeps.

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, header};
use serde_json::Value;

use crate::rest::entity::read_entity;
use crate::rest::feedback::Feedback;
use crate::rest::items::Item;
use crate::rest::response::ApiError;
use gorse_storage::data::types::{ItemPatch, Labels, User, UserPatch};

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

/// The headers a request carries: its content type.
fn headers_for(ct: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(ct).expect("a content type is a header value"),
    );
    headers
}

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

/// Go keeps each number of a label as its text, so a number past the
/// `float64` range, or with a fraction or an exponent, is read back as it was
/// written.
#[test]
fn label_numbers_keep_their_text() {
    let headers = headers_for("application/json");
    let body = Bytes::from(r#"{"Labels":[1e400,2.50,1E+2]}"#);
    let Ok(user) = read_entity::<User>(&headers, &body) else {
        panic!("Go accepts these labels");
    };
    assert_eq!(
        user.labels,
        Labels::Array(vec![
            Labels::Number("1e400".to_owned()),
            Labels::Number("2.50".to_owned()),
            Labels::Number("1E+2".to_owned()),
        ])
    );
}

/// Go decodes labels nested to the scanner's limit in full. The validation
/// that follows refuses a nested list with its text, so the decoder must
/// accept the depth.
#[test]
fn labels_nested_past_the_serde_limit_are_decoded() {
    let headers = headers_for("application/json");
    let nested = format!("{{\"Labels\":{}{}}}", "[".repeat(130), "]".repeat(130));
    let Ok(user) = read_entity::<User>(&headers, &Bytes::from(nested)) else {
        panic!("Go accepts labels nested this deep");
    };
    let mut depth = 0;
    let mut value = &user.labels;
    while let Labels::Array(items) = value {
        depth += 1;
        match items.first() {
            Some(first) => value = first,
            None => break,
        }
    }
    assert_eq!(depth, 130);
}
