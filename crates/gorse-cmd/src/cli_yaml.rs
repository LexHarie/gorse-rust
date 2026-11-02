// Ports: gopkg.in/yaml.v3 (the encoder cmd/gorse-cli uses for `pipeline get` and `pipeline schema`)

//! The YAML `gorse-cli` writes for a config and a schema, laid out as yaml.v3
//! lays it out: four spaces of indent, sequences indented under their key, keys
//! sorted, numbers in Go's `%g` form, and strings quoted where yaml.v3 quotes
//! them. Literal blocks for strings with newlines are not written; such strings
//! are double quoted instead.

use serde_json::Value;

/// The YAML document for `value`, as yaml.v3's `Encoder` writes it.
pub fn encode(value: &Value) -> String {
    let lines = match value {
        Value::Object(object) if !object.is_empty() => map_lines(object),
        Value::Array(items) if !items.is_empty() => seq_lines(items),
        scalar_or_empty => return format!("{}\n", inline(scalar_or_empty)),
    };
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The lines of a non-empty mapping, with the keys sorted.
fn map_lines(object: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut entries: Vec<(&String, &Value)> = object.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut lines = Vec::new();
    for (key, value) in entries {
        let key = quote_string(key);
        if is_block(value) {
            lines.push(format!("{key}:"));
            lines.extend(indent(&block_lines(value), 4));
        } else {
            lines.push(format!("{key}: {}", inline(value)));
        }
    }
    lines
}

/// The lines of a non-empty sequence. A mapping or sequence item starts on the
/// `- ` line, and its later lines are indented to line up with its first.
fn seq_lines(items: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for item in items {
        if is_block(item) {
            let child = block_lines(item);
            for (i, line) in child.iter().enumerate() {
                if i == 0 {
                    lines.push(format!("- {line}"));
                } else {
                    lines.push(format!("  {line}"));
                }
            }
        } else {
            lines.push(format!("- {}", inline(item)));
        }
    }
    lines
}

/// The lines of a mapping or sequence that is not empty.
fn block_lines(value: &Value) -> Vec<String> {
    match value {
        Value::Object(object) => map_lines(object),
        Value::Array(items) => seq_lines(items),
        _ => Vec::new(),
    }
}

/// Whether `value` is written on lines of its own: a mapping or sequence with
/// entries. Empty ones are written in flow style, as `{}` and `[]`.
fn is_block(value: &Value) -> bool {
    match value {
        Value::Object(object) => !object.is_empty(),
        Value::Array(items) => !items.is_empty(),
        _ => false,
    }
}

/// The text of a value written on one line.
fn inline(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => format_go_float(number.as_f64().unwrap_or(0.0)),
        Value::String(text) => quote_string(text),
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
    }
}

fn indent(lines: &[String], by: usize) -> Vec<String> {
    let pad = " ".repeat(by);
    lines.iter().map(|line| format!("{pad}{line}")).collect()
}

/// Go's `strconv.FormatFloat(f, 'g', -1, 64)`, which yaml.v3 uses for numbers:
/// the shortest digits, in fixed notation from 1e-4 up to 1e+5 and exponent
/// notation from 1e+6 on.
fn format_go_float(value: f64) -> String {
    if value.is_nan() {
        return ".nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            ".inf".to_owned()
        } else {
            "-.inf".to_owned()
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0".to_owned()
        } else {
            "0".to_owned()
        };
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if !(-4..6).contains(&exponent) {
        let (head, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        return format!("{sign}{head}{fraction}e{exp_sign}{:02}", exponent.abs());
    }
    if exponent < 0 {
        let zeros = "0".repeat((-exponent - 1) as usize);
        return format!("{sign}0.{zeros}{digits}");
    }
    let point = exponent as usize + 1;
    if digits.len() <= point {
        let zeros = "0".repeat(point - digits.len());
        format!("{sign}{digits}{zeros}")
    } else {
        let (whole, fraction) = digits.split_at(point);
        format!("{sign}{whole}.{fraction}")
    }
}

/// How yaml.v3 writes a string: plain when it reads back as the same string,
/// single quoted when only the layout needs it, and double quoted otherwise.
fn quote_string(text: &str) -> String {
    if text.chars().any(|ch| ch.is_control() && ch != '\t') {
        return double_quote(text);
    }
    if reads_as_non_string(text) {
        return double_quote(text);
    }
    if plain_is_allowed(text) {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', "''"))
}

/// Whether a plain scalar with this text would read as something other than a
/// string: a null, a boolean (including YAML 1.1's `yes` and `on`), or a number.
fn reads_as_non_string(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if text.is_empty()
        || matches!(
            lower.as_str(),
            "~" | "null"
                | "true"
                | "false"
                | "y"
                | "yes"
                | "n"
                | "no"
                | "on"
                | "off"
                | ".inf"
                | "-.inf"
                | ".nan"
        )
    {
        return true;
    }
    let starts_numeric = text
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || ch == '+' || ch == '-' || ch == '.');
    starts_numeric && text.parse::<f64>().is_ok()
}

/// Whether the text can be written without quotes in block style.
fn plain_is_allowed(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    if "[]{},#&*!|>'\"%@`".contains(first) {
        return false;
    }
    // A leading `-`, `?` or `:` is an indicator only when a space follows it.
    if matches!(first, '-' | '?' | ':') && (text.len() == 1 || text.as_bytes()[1] == b' ') {
        return false;
    }
    !(text.starts_with(' ')
        || text.ends_with(' ')
        || text.ends_with(':')
        || text.contains(": ")
        || text.contains(" #"))
}

/// A double-quoted scalar with the escapes YAML needs.
fn double_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            ch if ch.is_control() => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mappings_are_sorted_and_indented_by_four() {
        let value = json!({"b": {"z": 1, "x": "two"}, "a": [1, 2]});
        assert_eq!(
            encode(&value),
            "a:\n    - 1\n    - 2\nb:\n    x: two\n    z: 1\n"
        );
    }

    #[test]
    fn sequences_of_mappings_line_up_after_the_dash() {
        let value = json!({"items": [{"name": "a", "size": 2}]});
        assert_eq!(encode(&value), "items:\n    - name: a\n      size: 2\n");
    }

    #[test]
    fn numbers_use_go_g_format() {
        assert_eq!(format_go_float(100.0), "100");
        assert_eq!(format_go_float(1_000_000.0), "1e+06");
        assert_eq!(format_go_float(0.5), "0.5");
        assert_eq!(format_go_float(0.00001), "1e-05");
        assert_eq!(format_go_float(0.0001), "0.0001");
        assert_eq!(format_go_float(-2.5), "-2.5");
    }

    #[test]
    fn strings_quote_where_yaml_needs_it() {
        assert_eq!(quote_string("reranker"), "reranker");
        assert_eq!(quote_string("$schema"), "$schema");
        assert_eq!(quote_string("true"), "\"true\"");
        assert_eq!(quote_string("123"), "\"123\"");
        assert_eq!(quote_string(""), "\"\"");
        assert_eq!(quote_string("a: b"), "'a: b'");
        assert_eq!(quote_string("it's"), "it's");
        assert_eq!(quote_string("y"), "\"y\"");
        assert_eq!(quote_string("-x"), "-x");
        assert_eq!(quote_string("- x"), "'- x'");
        assert_eq!(quote_string("*star"), "'*star'");
    }

    #[test]
    fn empty_collections_are_flow_style() {
        assert_eq!(encode(&json!({"a": {}, "b": []})), "a: {}\nb: []\n");
    }
}
