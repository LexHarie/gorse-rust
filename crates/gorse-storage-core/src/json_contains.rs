// Ports: storage/scheme.go (the json_contains SQLite function registered in init) @ add21b5

//! `json_contains(a, b)`, the SQLite function Gorse registers so that its
//! SQLite stores can filter on JSON arrays (an item's categories, a user's
//! labels): whether every element of the array `b` is an element of the
//! array `a`.
//!
//! Go registers it with modernc's SQLite driver, and the driver decides how
//! the arguments arrive and how an error leaves: [`modernc_text`] and
//! [`modernc_error_text`] give those rules, for the Rust SQLite glue to
//! apply.
//!
//! Deviation: Go hashes the elements in a `map[any]struct{}`, and an element
//! that is itself an array or an object panics there. The panic leaves the
//! Go connection locked. Here it is [`Error::Unhashable`] with Go's panic
//! text.

use std::collections::HashSet;

use gorse_common::gojson::{UnmarshalError, Value, unmarshal_slice_any};
use thiserror::Error;

/// One argument, as SQLite types it and modernc hands it to Go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'a> {
    /// `NULL` (Go's `nil`).
    Null,
    /// An `INTEGER` (Go's `int64`).
    Integer(i64),
    /// A `REAL` (Go's `float64`).
    Real(f64),
    /// A `TEXT` (Go's `string`), as [`modernc_text`] cuts it.
    Text(&'a [u8]),
    /// A `BLOB` (Go's `[]byte`), all of its bytes.
    Blob(&'a [u8]),
}

/// Why [`json_contains`] failed. The text is Go's `err.Error()`, or for
/// [`Error::Unhashable`] what Go's panic prints.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// An `INTEGER` or `REAL` argument (Go's
    /// `errors.Errorf("unsupported type %T", arg)`).
    #[error("unsupported type {0}")]
    UnsupportedType(&'static str),
    /// An argument that `json.Unmarshal` cannot decode into a `[]any`.
    #[error(transparent)]
    Json(#[from] UnmarshalError),
    /// An element that is an array or an object, which Go cannot hash: Go
    /// panics with this text (see the module docs).
    #[error("{0}")]
    Unhashable(String),
}

/// A hashable element, as Go's map compares `any` keys: by dynamic type,
/// then by value, with -0 equal to 0 (JSON has no NaN).
#[derive(PartialEq, Eq, Hash)]
enum Key<'a> {
    Null,
    Bool(bool),
    Number(u64),
    String(&'a str),
}

/// What hashing an array or an object panics with. Go's `typehash`, which
/// inserting and looking up in a non-empty map run, panics with a
/// `runtime.errorString`; a lookup in an empty map checks the key with
/// `mapKeyError` instead, whose `unhashableTypeError` reads differently.
enum Hashing {
    Typehash,
    MapKeyError,
}

fn key(v: &Value, hashing: Hashing) -> Result<Key<'_>, Error> {
    let go_type = match v {
        Value::Null => return Ok(Key::Null),
        Value::Bool(b) => return Ok(Key::Bool(*b)),
        Value::Number(f) => return Ok(Key::Number(if *f == 0.0 { 0 } else { f.to_bits() })),
        Value::String(s) => return Ok(Key::String(s)),
        Value::Array(_) => "[]interface {}",
        Value::Object(_) => "map[string]interface {}",
    };
    Err(Error::Unhashable(match hashing {
        Hashing::Typehash => format!("runtime error: hash of unhashable type {go_type}"),
        Hashing::MapKeyError => format!("hash of unhashable type: {go_type}"),
    }))
}

/// The function's `parse`: a text or a blob decoded as a `[]any`, `None`
/// for JSON `null` (Go's nil slice).
fn parse(arg: Arg<'_>) -> Result<Option<Vec<Value>>, Error> {
    let data = match arg {
        Arg::Text(data) | Arg::Blob(data) => data,
        Arg::Integer(_) => return Err(Error::UnsupportedType("int64")),
        Arg::Real(_) => return Err(Error::UnsupportedType("float64")),
        Arg::Null => unreachable!("json_contains returns NULL before parsing a NULL"),
    };
    Ok(unmarshal_slice_any(data)?)
}

/// Gorse's `json_contains(a, b)`: `None` (SQL `NULL`) when either argument
/// is `NULL`, else whether each element of b is an element of a.
///
/// Both arguments are decoded before anything is compared, a first and the
/// first error returned; then every element of a is hashed, then the
/// elements of b are looked up in order until one is missing.
pub fn json_contains(a: Arg<'_>, b: Arg<'_>) -> Result<Option<bool>, Error> {
    if a == Arg::Null || b == Arg::Null {
        return Ok(None);
    }
    let j1 = parse(a)?.unwrap_or_default();
    let j2 = parse(b)?.unwrap_or_default();
    let mut elements = HashSet::with_capacity(j1.len());
    for e in &j1 {
        elements.insert(key(e, Hashing::Typehash)?);
    }
    let hashing = || {
        if elements.is_empty() {
            Hashing::MapKeyError
        } else {
            Hashing::Typehash
        }
    };
    for e in &j2 {
        if !elements.contains(&key(e, hashing())?) {
            return Ok(Some(false));
        }
    }
    Ok(Some(true))
}

/// A `TEXT` argument as modernc hands it to Go: copied with `libc.GoString`,
/// so it ends at its first NUL. A `BLOB` keeps every byte.
pub fn modernc_text(text: &[u8]) -> &[u8] {
    text.iter()
        .position(|&c| c == 0)
        .map_or(text, |end| &text[..end])
}

/// The text of the error a Go query gets when [`json_contains`] fails.
/// modernc passes the error text to `sqlite3_result_error` as a C string,
/// so it ends at its first NUL, and reports it as `SQLITE_ERROR` in the
/// form of its `errstrForDB`: `SQL logic error: <text> (1)`, or `SQL logic
/// error (1)` when the text is SQLite's own.
pub fn modernc_error_text(err: &Error) -> String {
    const ERRSTR: &str = "SQL logic error";
    let msg = err.to_string();
    let msg = msg.split('\0').next().unwrap_or_default();
    if msg == ERRSTR {
        format!("{ERRSTR} (1)")
    } else {
        format!("{ERRSTR}: {msg} (1)")
    }
}
