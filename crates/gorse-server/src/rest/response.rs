// Ports: server/rest.go (Ok, BadRequest, TooManyRequests, InternalServerError,
// PageNotFound, Error, Text, ParseInt, ParseDuration, ReadCategories,
// withWildCard) @ add21b5

//! The response writers and request readers that `server/rest.go` shares
//! between its handlers, and the error type the handlers return.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gorse_common::gourl;
use gorse_common::gotime;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The parsed query string of a request. Go's `QueryParameter` and
/// `QueryParameters` read it through `url.Values`, which keeps every value of
/// a name in order.
#[derive(Debug, Default)]
pub(crate) struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    /// Parses a raw query string the way Go's `url.ParseQuery` does: pairs are
    /// split on `&`, a pair with a malformed escape is skipped, and so is a
    /// pair with a `;` in its key (Go 1.17 onwards).
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        let mut pairs = Vec::new();
        for segment in raw.unwrap_or("").split('&') {
            if segment.is_empty() {
                continue;
            }
            let (key, value) = match segment.split_once('=') {
                Some((key, value)) => (key, value),
                None => (segment, ""),
            };
            if key.contains(';') {
                continue;
            }
            let (Ok(key), Ok(value)) = (gourl::query_unescape(key), gourl::query_unescape(value))
            else {
                continue;
            };
            pairs.push((
                String::from_utf8_lossy(&key).into_owned(),
                String::from_utf8_lossy(&value).into_owned(),
            ));
        }
        Self { pairs }
    }

    /// Go's `QueryParameter`: the first value of `name`, or `""`.
    pub(crate) fn get(&self, name: &str) -> &str {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map_or("", |(_, value)| value.as_str())
    }

    /// Go's `QueryParameters`: every value of `name`, in order.
    pub(crate) fn get_all(&self, name: &str) -> Vec<&str> {
        self.pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

/// The path parameters of a matched route, by name, as go-restful's
/// `PathParameter` reads them. A name the route does not have reads as `""`.
pub(crate) type PathParams = HashMap<String, String>;

/// Go's `PathParameter`.
pub(crate) fn path_param<'a>(params: &'a PathParams, name: &str) -> &'a str {
    params.get(name).map_or("", String::as_str)
}

/// An error a handler returns. It is written the way Go's error writers write
/// one: the status code, the error message as the whole body, and
/// `Access-Control-Allow-Origin: *`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApiError {
    /// `BadRequest`: 400.
    BadRequest(String),
    /// `PageNotFound`: 404.
    PageNotFound(String),
    /// `TooManyRequests`: 429, also used for quota errors.
    TooManyRequests(String),
    /// `InternalServerError`: 500.
    Internal(String),
    /// `Error` with any other status.
    Status(StatusCode, String),
}

impl ApiError {
    /// Go's `InternalServerError` for a store or logic error, which is written
    /// with the error's own text.
    pub(crate) fn internal(err: impl std::fmt::Display) -> Self {
        Self::Internal(err.to_string())
    }

    /// The status code the error is written with.
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PageNotFound(_) => StatusCode::NOT_FOUND,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Status(status, _) => *status,
        }
    }

    /// The message the error is written with.
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::BadRequest(message)
            | Self::PageNotFound(message)
            | Self::TooManyRequests(message)
            | Self::Internal(message)
            | Self::Status(_, message) => message,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(status = self.status().as_u16(), error = self.message(), "request failed");
        let mut response = plain(self.status(), self.message().to_owned());
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        response
    }
}

/// A plain-text response, as go-restful's `WriteErrorString` and Go's
/// `net/http` write one. The body is served as `text/plain`, which Go's content
/// sniffing gives it.
pub(crate) fn plain(status: StatusCode, body: String) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Go's `Ok`: writes `value` as JSON, with CORS on, and the status 200.
///
/// # Errors
///
/// Fails when `value` cannot be encoded as JSON, which Go reports only in its
/// log.
pub(crate) fn ok<T: Serialize + ?Sized>(value: &T) -> Result<Response, ApiError> {
    let body = go_json(value).map_err(ApiError::internal)?;
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    Ok(response)
}

/// Go's `Text`: writes `content` as it is, with CORS on, and the status 200.
pub(crate) fn text(content: &str) -> Response {
    let mut response = (StatusCode::OK, content.to_owned()).into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

/// Encodes `value` as go-restful writes JSON: `json.MarshalIndent` with a
/// one-space indent. Go's encoder also escapes `<`, `>`, `&` and the two line
/// separators inside strings, so this does the same. Those bytes occur only in
/// strings in JSON, so the escape is a plain byte substitution.
pub(crate) fn go_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut serializer)?;
    Ok(escape_html(&out))
}

fn escape_html(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len());
    let mut i = 0;
    while i < json.len() {
        match json[i] {
            b'<' => out.extend_from_slice(b"\\u003c"),
            b'>' => out.extend_from_slice(b"\\u003e"),
            b'&' => out.extend_from_slice(b"\\u0026"),
            // U+2028 and U+2029 are E2 80 A8 and E2 80 A9.
            0xE2 if json[i..].starts_with(&[0xE2, 0x80, 0xA8]) => {
                out.extend_from_slice(b"\\u2028");
                i += 2;
            }
            0xE2 if json[i..].starts_with(&[0xE2, 0x80, 0xA9]) => {
                out.extend_from_slice(b"\\u2029");
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    out
}

/// Go's `ReadEntity` for a JSON body. Go's `json.Decoder` reads the first JSON
/// value and ignores what follows it, so this does the same.
///
/// # Errors
///
/// Fails with the decoder's message when the body is not the JSON the handler
/// expects. An empty body is `EOF`, as Go's decoder reports it.
pub(crate) fn read_entity<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(ApiError::BadRequest("EOF".to_owned()));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut deserializer).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go's `ParseInt`: the query parameter as an integer, or `fallback` when it is
/// absent or empty.
///
/// # Errors
///
/// Fails with `strconv.Atoi`'s message when the value is not an integer.
pub(crate) fn parse_int(query: &Query, name: &str, fallback: i64) -> Result<i64, ApiError> {
    let value = query.get(name);
    if value.is_empty() {
        return Ok(fallback);
    }
    value.parse::<i64>().map_err(|err| {
        let reason = match err.kind() {
            std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                "value out of range"
            }
            _ => "invalid syntax",
        };
        ApiError::BadRequest(format!(
            "strconv.Atoi: parsing {}: {reason}",
            go_quote(value)
        ))
    })
}

/// Go's `ParseDuration`: the query parameter as a duration, or zero when it is
/// absent or empty.
///
/// # Errors
///
/// Fails with `time.ParseDuration`'s message when the value is malformed.
pub(crate) fn parse_duration(query: &Query, name: &str) -> Result<gotime::Duration, ApiError> {
    let value = query.get(name);
    if value.is_empty() {
        return Ok(gotime::Duration(0));
    }
    gotime::parse_duration(value).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go's `ReadCategories`: the `category` path parameter, else every non-empty
/// `category` query value, else `defaults`.
pub(crate) fn read_categories(
    params: &PathParams,
    query: &Query,
    defaults: &[&str],
) -> Vec<String> {
    let path_value = path_param(params, "category");
    if !path_value.is_empty() {
        return vec![path_value.to_owned()];
    }
    let query_values = query.get_all("category");
    if !query_values.is_empty() {
        return query_values
            .into_iter()
            .filter(|category| !category.is_empty())
            .map(str::to_owned)
            .collect();
    }
    defaults.iter().map(|value| (*value).to_owned()).collect()
}

/// Go's `withWildCard`: the categories with the empty category appended, which
/// matches every item in the cache's categories.
pub(crate) fn with_wild_card(categories: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(categories.len() + 1);
    result.extend_from_slice(categories);
    result.push(String::new());
    result
}

/// Go's `strconv.Quote` for the text `ParseInt` reports. Printable characters
/// are kept, and the quote and backslash are escaped.
fn go_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            ch if ch.is_control() => quoted.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}
