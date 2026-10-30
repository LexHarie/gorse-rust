// Ports: server/rest.go (Ok, BadRequest, TooManyRequests, InternalServerError,
// PageNotFound, Error, Text, ParseInt, ParseDuration, ReadCategories,
// withWildCard) @ add21b5

//! The response writers and request readers that `server/rest.go` shares
//! between its handlers, and the error type the handlers return.

use std::collections::HashMap;
use std::io;

use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gorse_common::gotime;
use gorse_common::gourl;
use gorse_storage::data::types::{Labels, validate_labels};
use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter};

/// The parsed query string of a request. Go's `QueryParameter` and
/// `QueryParameters` read it through `url.Values`, which keeps every value of
/// a name in order.
#[derive(Debug, Default)]
pub struct Query {
    pairs: Vec<(String, String)>,
}

impl Query {
    /// Parses a raw query string the way Go's `url.ParseQuery` does: pairs are
    /// split on `&`, a pair with a malformed escape is skipped, and so is a
    /// pair with a `;` in its key (Go 1.17 onwards).
    pub fn parse(raw: Option<&str>) -> Self {
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
    pub fn get(&self, name: &str) -> &str {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map_or("", |(_, value)| value.as_str())
    }

    /// Go's `QueryParameters`: every value of `name`, in order.
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

/// The path parameters of a matched route, by name, as go-restful's
/// `PathParameter` reads them. A name the route does not have reads as `""`.
pub type PathParams = HashMap<String, String>;

/// Go's `PathParameter`.
pub fn path_param<'a>(params: &'a PathParams, name: &str) -> &'a str {
    params.get(name).map_or("", String::as_str)
}

/// An error a handler returns. It is written the way Go's error writers write
/// one: the status code, the error message as the whole body, and
/// `Access-Control-Allow-Origin: *`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
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
    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::Internal(err.to_string())
    }

    /// The status code the error is written with.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PageNotFound(_) => StatusCode::NOT_FOUND,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Status(status, _) => *status,
        }
    }

    /// The message the error is written with.
    pub fn message(&self) -> &str {
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
        tracing::error!(
            status = self.status().as_u16(),
            error = self.message(),
            "request failed"
        );
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
pub fn plain(status: StatusCode, body: String) -> Response {
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
pub fn ok<T: Serialize + ?Sized>(value: &T) -> Result<Response, ApiError> {
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
pub fn text(content: &str) -> Response {
    let mut response = (StatusCode::OK, content.to_owned()).into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

/// Go's `http.Redirect` with the temporary status, as `ServeMux` sends it for a
/// path that lacks the slash its pattern ends with. The body is the link Go
/// writes for GET and HEAD, and empty for any other method.
pub fn temporary_redirect(method: &Method, to: &str) -> Response {
    let body = if *method == Method::GET || *method == Method::HEAD {
        format!("<a href=\"{to}\">Temporary Redirect</a>.\n\n")
    } else {
        String::new()
    };
    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::LOCATION, to),
        ],
        body,
    )
        .into_response()
}

/// Encodes `value` as go-restful writes JSON: `json.MarshalIndent` with a
/// one-space indent. Go's encoder also escapes `<`, `>`, `&` and the two line
/// separators inside strings, so this does the same. Those bytes occur only in
/// strings in JSON, so the escape is a plain byte substitution.
pub fn go_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    let formatter = GoFormatter(PrettyFormatter::with_indent(b" "), 0);
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value.serialize(&mut serializer)?;
    Ok(escape_html(&out))
}

/// Encodes `value` as Go's `json.Encoder.Encode` writes a line of JSON: compact,
/// with the floats written as `encoding/json` writes them and the `<`, `>` and
/// `&` escapes. The newline after the value is the caller's.
pub fn go_json_compact<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, GoCompactFormatter);
    value.serialize(&mut serializer)?;
    Ok(escape_html(&out))
}

/// serde_json's compact formatter, with the floats written as `encoding/json`
/// writes them. A raw fragment is already compact, so it is written as it is.
struct GoCompactFormatter;

impl Formatter for GoCompactFormatter {
    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_f32(writer, value)
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_f64(writer, value)
    }

    fn write_raw_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        CompactFormatter.write_raw_fragment(writer, fragment)
    }
}

/// serde_json's pretty printer, with the floats written as `encoding/json`
/// writes them. serde_json writes a whole float as `1.0`, and Go writes `1`.
/// The second field counts the arrays and objects that are open, which is the
/// indentation a raw fragment is written at.
struct GoFormatter<'a>(PrettyFormatter<'a>, usize);

impl Formatter for GoFormatter<'_> {
    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.1 += 1;
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.1 -= 1;
        self.0.end_array(writer)
    }

    /// A raw fragment is a complete compact JSON value, such as labels that the
    /// storage layer writes as text. It is indented as the values built here
    /// are, so the response reads the same whichever way its value was built.
    fn write_raw_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        write_indented(writer, fragment, self.1)
    }

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.1 += 1;
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.1 -= 1;
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        write_go_f32(writer, value)
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        write_go_f64(writer, value)
    }
}

/// Writes a compact JSON value indented as serde's pretty printer indents the
/// values around it, with one space per level and `depth` levels already open.
/// An empty array or object stays `[]` or `{}`, as the pretty printer writes
/// it, and `": "` follows each key.
fn write_indented<W: ?Sized + io::Write>(
    writer: &mut W,
    json: &str,
    depth: usize,
) -> io::Result<()> {
    fn newline<W: ?Sized + io::Write>(writer: &mut W, depth: usize) -> io::Result<()> {
        writer.write_all(b"\n")?;
        writer.write_all(" ".repeat(depth).as_bytes())
    }
    let bytes = json.as_bytes();
    let mut depth = depth;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            writer.write_all(&[byte])?;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                writer.write_all(&[byte])?;
            }
            b'[' | b'{' => {
                writer.write_all(&[byte])?;
                let close = if byte == b'[' { b']' } else { b'}' };
                if bytes.get(index + 1) != Some(&close) {
                    depth += 1;
                    newline(writer, depth)?;
                }
            }
            b']' | b'}' => {
                let open = if byte == b']' { b'[' } else { b'{' };
                if index == 0 || bytes[index - 1] != open {
                    depth -= 1;
                    newline(writer, depth)?;
                }
                writer.write_all(&[byte])?;
            }
            b',' => {
                writer.write_all(b",")?;
                newline(writer, depth)?;
            }
            b':' => writer.write_all(b": ")?,
            other => writer.write_all(&[other])?,
        }
    }
    Ok(())
}

/// Writes a float as `encoding/json` does: the shortest text that reads back to
/// the same value, in plain notation, or in `e` notation outside `[1e-6, 1e21)`.
/// The `e` exponent has its sign, and a negative exponent drops the zero that
/// pads it, as Go does. The range is checked in the float's own precision, as Go
/// checks it. `serde_json` calls these only for finite values.
macro_rules! go_float {
    ($name:ident, $float:ty) => {
        fn $name<W: ?Sized + io::Write>(writer: &mut W, value: $float) -> io::Result<()> {
            let abs = value.abs();
            if abs != 0.0 && (abs < 1e-6 || abs >= 1e21) {
                let text = format!("{value:e}");
                let (mantissa, exponent) = text.split_once('e').unwrap_or((text.as_str(), "0"));
                let sign = if exponent.starts_with('-') { "" } else { "+" };
                return write!(writer, "{mantissa}e{sign}{exponent}");
            }
            write!(writer, "{value}")
        }
    };
}

go_float!(write_go_f32, f32);
go_float!(write_go_f64, f64);

/// Go's `json.HTMLEscape`: replaces `<`, `>`, `&` and the U+2028 and U+2029
/// separators with their `\u` escapes, as `json.Encoder` does by default.
pub fn escape_html(json: &[u8]) -> Vec<u8> {
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

/// Go's `readEntity` decodes with `UseNumber`, so every number in a request's
/// labels is a `json.Number`. The storage decoder reads numbers as `float64`,
/// so the labels of a request are converted here, then validated as Go's
/// `ValidateLabels` validates them. A number keeps the shortest text that reads
/// back to its `float64`, which is the text of a whole number as Go writes it.
pub fn validate_request_labels(labels: &mut Labels) -> Result<(), ApiError> {
    use_numbers(labels);
    validate_labels(labels).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Turns every `float64` in `labels` into a `json.Number` with the same value.
/// The walk keeps the lists and maps still to be visited on a stack, so labels
/// nested at any depth are converted in full.
fn use_numbers(labels: &mut Labels) {
    let mut pending = vec![labels];
    while let Some(labels) = pending.pop() {
        if let Labels::Float64(value) = *labels {
            *labels = Labels::Number(value.to_string());
            continue;
        }
        match labels {
            Labels::Array(items) => pending.extend(items.iter_mut()),
            Labels::Object(fields) => pending.extend(fields.values_mut()),
            Labels::Nil
            | Labels::Bool(_)
            | Labels::Float64(_)
            | Labels::Number(_)
            | Labels::String(_) => {}
        }
    }
}

/// Go's `ParseInt`: the query parameter as an integer, or `fallback` when it is
/// absent or empty.
///
/// # Errors
///
/// Fails with `strconv.Atoi`'s message when the value is not an integer.
pub fn parse_int(query: &Query, name: &str, fallback: i64) -> Result<i64, ApiError> {
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
pub fn parse_duration(query: &Query, name: &str) -> Result<gotime::Duration, ApiError> {
    let value = query.get(name);
    if value.is_empty() {
        return Ok(gotime::Duration(0));
    }
    gotime::parse_duration(value).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go's `ReadCategories`: the `category` path parameter, else every non-empty
/// `category` query value, else `defaults`.
pub fn read_categories(params: &PathParams, query: &Query, defaults: &[&str]) -> Vec<String> {
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
pub fn with_wild_card(categories: &[String]) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use serde_json::value::RawValue;

    use super::*;

    #[test]
    fn raw_fragments_are_indented_as_the_values_around_them() {
        // The same object, once with its labels as a raw fragment and once with
        // the labels built as values. Both must be indented alike.
        #[derive(Serialize)]
        struct Raw {
            labels: Box<RawValue>,
            comment: &'static str,
        }
        #[derive(Serialize)]
        struct Built {
            labels: serde_json::Value,
            comment: &'static str,
        }
        let raw = Raw {
            labels: RawValue::from_string(r#"[1,[],{},{"b":[true,null],"c":"<"}]"#.to_owned())
                .expect("valid JSON"),
            comment: "x",
        };
        let built = Built {
            labels: serde_json::json!([1, [], {}, {"b": [true, null], "c": "<"}]),
            comment: "x",
        };
        assert_eq!(go_json(&raw).expect("raw"), go_json(&built).expect("built"));
    }
}
