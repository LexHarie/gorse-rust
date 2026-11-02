// Ports: encoding/json (Go standard library): Marshal and MarshalIndent, as cmd/gorse-cli prints JSON

//! JSON as Go's `encoding/json` writes it. Keys are sorted (the values are
//! `serde_json` maps, which sort them), `<`, `>` and `&` are escaped, and a
//! float is written the way Go writes a `float64`, so `1.0` prints as `1`.

use std::fmt::Write;

use serde_json::{Number, Value};

/// Go's `json.Marshal`: compact JSON.
pub fn marshal(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, None, 0);
    out
}

/// Go's `json.MarshalIndent(value, "", indent)`.
pub fn marshal_indent(value: &Value, indent: &str) -> String {
    let mut out = String::new();
    write_value(&mut out, value, Some(indent), 0);
    out
}

fn write_value(out: &mut String, value: &Value, indent: Option<&str>, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number),
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_break(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            write_break(out, indent, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            // Go sorts the keys of a map; `serde_json` may keep insertion order.
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_break(out, indent, depth + 1);
                write_string(out, key);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, indent, depth + 1);
            }
            write_break(out, indent, depth);
            out.push('}');
        }
    }
}

/// A newline and the indent of `depth`, when the output is indented.
fn write_break(out: &mut String, indent: Option<&str>, depth: usize) {
    if let Some(indent) = indent {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    }
}

fn write_number(out: &mut String, number: &Number) {
    if let Some(integer) = number.as_i64() {
        let _ = write!(out, "{integer}");
    } else if let Some(integer) = number.as_u64() {
        let _ = write!(out, "{integer}");
    } else {
        write_float(out, number.as_f64().unwrap_or(0.0));
    }
}

/// Go's float encoding: plain decimal, unless the magnitude is below 1e-6 or at
/// least 1e21, where it uses an exponent with the zero of a single-digit
/// negative exponent removed.
fn write_float(out: &mut String, value: f64) {
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-6..1e21).contains(&magnitude) {
        let text = format!("{value:e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        let exponent: i64 = exponent.parse().unwrap_or(0);
        let sign = if exponent < 0 { '-' } else { '+' };
        let _ = write!(out, "{mantissa}e{sign}{:02}", exponent.abs());
        if exponent < 0 && exponent.abs() < 10 {
            // Go cleans `e-09` to `e-9`, so a one-digit negative exponent has no zero.
            let len = out.len();
            out.replace_range(len - 2..len - 1, "");
        }
    } else {
        let _ = write!(out, "{value}");
    }
}

/// Go's string encoding, which escapes the characters HTML and JavaScript use.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn floats_print_as_go_prints_them() {
        assert_eq!(marshal(&json!(1.0)), "1");
        assert_eq!(marshal(&json!(0.1)), "0.1");
        assert_eq!(marshal(&json!(-0.0)), "-0");
        assert_eq!(marshal(&json!(1e21)), "1e+21");
        assert_eq!(marshal(&json!(1.5e-7)), "1.5e-7");
        assert_eq!(marshal(&json!(1e-6)), "0.000001");
    }

    #[test]
    fn strings_escape_html_characters() {
        assert_eq!(
            marshal(&json!("a<b>&\"c\"\n")),
            "\"a\\u003cb\\u003e\\u0026\\\"c\\\"\\n\""
        );
    }

    #[test]
    fn objects_sort_keys_and_compact() {
        assert_eq!(
            marshal(&json!({"b": [1, 2], "a": null})),
            "{\"a\":null,\"b\":[1,2]}"
        );
    }

    #[test]
    fn indent_matches_go_marshal_indent() {
        let value = json!({"a": [1], "b": {}, "c": []});
        assert_eq!(
            marshal_indent(&value, "  "),
            "{\n  \"a\": [\n    1\n  ],\n  \"b\": {},\n  \"c\": []\n}"
        );
    }
}
