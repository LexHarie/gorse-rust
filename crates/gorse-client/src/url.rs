// Ports: net/url (Go standard library, the escaping the client relies on)

//! Go's `net/url` escaping, byte for byte, so the paths and queries this client
//! sends are the ones the Go client sends.

use std::collections::BTreeMap;
use std::fmt::Write;

/// Whether `byte` is kept as it is in a path segment (Go's `url.PathEscape`).
fn keeps_in_path_segment(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b':' | b'=' | b'@'
        )
}

/// Whether `byte` is kept as it is in a query component (Go's `url.QueryEscape`).
fn keeps_in_query_component(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn escape(input: &str, keeps: fn(u8) -> bool, space: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte == b' ' {
            out.push_str(space);
        } else if keeps(byte) {
            out.push(byte as char);
        } else {
            write!(out, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    out
}

/// Escapes one path segment, as Go's `url.PathEscape` does.
pub fn path_escape(input: &str) -> String {
    escape(input, keeps_in_path_segment, "%20")
}

/// Escapes a query component, as Go's `url.QueryEscape` does.
pub fn query_escape(input: &str) -> String {
    escape(input, keeps_in_query_component, "+")
}

/// Go's `url.Values`: the query parameters of one request. Keys are encoded in
/// sorted order, as `Values.Encode` encodes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    values: BTreeMap<String, Vec<String>>,
}

impl Query {
    /// Creates an empty query.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key` to `value`, replacing its values, as `Values.Set` does.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_owned(), vec![value.into()]);
    }

    /// Appends `value` to the values of `key`, as `Values.Add` does.
    pub fn add(&mut self, key: &str, value: &str) {
        self.values
            .entry(key.to_owned())
            .or_default()
            .push(value.to_owned());
    }

    /// Whether the query has no parameters.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The encoded query, without the leading `?`.
    pub fn encode(&self) -> String {
        let mut parts = Vec::new();
        for (key, values) in &self.values {
            for value in values {
                parts.push(format!("{}={}", query_escape(key), query_escape(value)));
            }
        }
        parts.join("&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_escape_matches_go() {
        assert_eq!(path_escape("alice"), "alice");
        assert_eq!(path_escape("a b"), "a%20b");
        assert_eq!(path_escape("a/b;c,d?e"), "a%2Fb%3Bc%2Cd%3Fe");
        assert_eq!(path_escape("x$&+:=@-_.~"), "x$&+:=@-_.~");
        assert_eq!(path_escape("é"), "%C3%A9");
    }

    #[test]
    fn query_escape_matches_go() {
        assert_eq!(query_escape("a b"), "a+b");
        assert_eq!(query_escape("a+b&c=d"), "a%2Bb%26c%3Dd");
        assert_eq!(query_escape("$:/"), "%24%3A%2F");
    }

    #[test]
    fn query_encodes_keys_sorted_and_repeats_values() {
        let mut query = Query::new();
        query.set("n", "10");
        query.add("category", "news");
        query.add("category", "tech code");
        query.set("begin", "2026-01-01T00:00:00Z");
        assert_eq!(
            query.encode(),
            "begin=2026-01-01T00%3A00%3A00Z&category=news&category=tech+code&n=10"
        );
        assert!(!query.is_empty());
        assert!(Query::new().is_empty());
    }
}
