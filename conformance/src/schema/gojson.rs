// Ports: the output of json.MarshalIndent(v, "", "  ") in contracts/tools/schemadump/main.go (Go's encoding/json, encode.go and floatEncoder).

//! Go's `json.MarshalIndent(v, "", "  ")` for the value shapes the schema dumps
//! write.
//!
//! Object keys are written sorted, as `encoding/json` writes maps. Empty arrays
//! and objects stay on one line. Strings escape `<`, `>` and `&` (Go's
//! `HTMLEscape` default), `U+2028` and `U+2029`, and control characters. Floats
//! follow `encoding/json`'s float encoder: shortest form, with an exponent only
//! below `1e-6` or from `1e21`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A JSON value, in the shapes the schema dumps write.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer, written in decimal.
    Int(i64),
    /// A float64. NaN and the infinities cannot be written, as in Go.
    Float(f64),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object, written with its keys in sorted order.
    Object(BTreeMap<String, Value>),
}

/// Go's `json.MarshalIndent(value, "", "  ")`. A float that JSON cannot carry
/// is an error, as in Go's `UnsupportedValueError`.
pub fn marshal_indent(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_value(&mut out, value, 0)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value, depth: usize) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Float(f) => out.push_str(&float(*f)?),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_value(out, item, depth + 1)?;
            }
            newline(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, depth + 1)?;
            }
            newline(out, depth);
            out.push('}');
        }
    }
    Ok(())
}

fn newline(out: &mut String, depth: usize) {
    out.push('\n');
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// `encoding/json`'s float64 format: `'f'` with the fewest digits, or `'e'` for
/// very small and very large magnitudes, with the exponent's leading zero
/// removed and a sign on a positive exponent.
fn float(f: f64) -> Result<String, String> {
    if !f.is_finite() {
        return Err(format!("json: unsupported value: {f}"));
    }
    let abs = f.abs();
    if abs == 0.0 || (1e-6..1e21).contains(&abs) {
        return Ok(format!("{f}"));
    }
    let text = format!("{f:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let exponent = if exponent.starts_with('-') {
        exponent.to_owned()
    } else {
        format!("+{exponent}")
    };
    Ok(format!("{mantissa}e{exponent}"))
}

fn write_string(out: &mut String, s: &str) {
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
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' | '\0'..='\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn schema_gojson_floats_follow_go_float64_format() {
        assert_eq!(marshal_indent(&Value::Float(1.0)).as_deref(), Ok("1"));
        assert_eq!(marshal_indent(&Value::Float(0.5)).as_deref(), Ok("0.5"));
        assert_eq!(marshal_indent(&Value::Float(-0.0)).as_deref(), Ok("-0"));
        assert_eq!(
            marshal_indent(&Value::Float(1e20)).as_deref(),
            Ok("100000000000000000000")
        );
        assert_eq!(marshal_indent(&Value::Float(1e-7)).as_deref(), Ok("1e-7"));
        assert_eq!(
            marshal_indent(&Value::Float(1.5e-10)).as_deref(),
            Ok("1.5e-10")
        );
        assert_eq!(marshal_indent(&Value::Float(1e21)).as_deref(), Ok("1e+21"));
        assert!(marshal_indent(&Value::Float(f64::NAN)).is_err());
        assert!(marshal_indent(&Value::Float(f64::INFINITY)).is_err());
    }

    #[test]
    fn schema_gojson_escapes_html_and_controls_as_go_does() {
        let text = Value::String("<a&b>\"\\\n\t\u{1}\u{2028}".to_owned());
        assert_eq!(
            marshal_indent(&text).as_deref(),
            Ok("\"\\u003ca\\u0026b\\u003e\\\"\\\\\\n\\t\\u0001\\u2028\"")
        );
    }

    #[test]
    fn schema_gojson_indents_like_marshal_indent() {
        let value = object(&[
            ("name", Value::String("documents".to_owned())),
            (
                "indexes",
                Value::Array(vec![object(&[("key", object(&[]))]), Value::Null]),
            ),
            ("empty", Value::Array(vec![])),
        ]);
        let expected = "{\n  \"empty\": [],\n  \"indexes\": [\n    {\n      \"key\": {}\n    },\n    null\n  ],\n  \"name\": \"documents\"\n}";
        assert_eq!(marshal_indent(&value).as_deref(), Ok(expected));
    }
}
