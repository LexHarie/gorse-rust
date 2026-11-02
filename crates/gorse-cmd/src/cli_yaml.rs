// Ports: gopkg.in/yaml.v3 v3.0.1, the encoder cmd/gorse-cli uses for `pipeline get` and `pipeline schema`: encode.go (the style a string starts with), emitterc.go (indentation, scalar analysis, the literal and double-quoted writers) and yamlprivateh.go (is_printable, is_break)

//! The YAML `gorse-cli` writes for a config and a schema, laid out as yaml.v3
//! lays it out: keys sorted, four spaces of indent with sequences indented under
//! their key, numbers in Go's `%g` form, and strings in the style yaml.v3 picks.
//!
//! A string with a newline is written as a literal block (`|`, `|-`, `|+`, with
//! an indentation hint where the text starts with a space or a break) when
//! yaml.v3 allows one, and double quoted otherwise. Indentation follows the
//! emitter: a value under a key at column `m` starts at the next multiple of
//! four past `m + 4`, and an item under a dash at column `s` starts at `s + 2`.
//! A key with a newline is written as a `?` complex key.

use serde_json::Value;

/// yaml.v3's `best_indent`.
const BEST_INDENT: usize = 4;

/// The YAML document for `value`, as yaml.v3's `Encoder` writes it.
pub fn encode(value: &Value) -> String {
    let lines = match value {
        Value::Object(object) if !object.is_empty() => map_lines(object, 0),
        Value::Array(items) if !items.is_empty() => seq_lines(items, 0),
        Value::String(text) if is_literal(text) => literal("", text, BEST_INDENT),
        scalar_or_empty => return format!("{}\n", inline(scalar_or_empty)),
    };
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The column a value under a key at column `column` is indented to: the next
/// multiple of `BEST_INDENT` past `column + BEST_INDENT`, as yaml.v3's
/// `increase_indent` rounds it.
fn child_column(column: usize) -> usize {
    BEST_INDENT * ((column + BEST_INDENT) / BEST_INDENT)
}

/// The lines of a non-empty mapping whose keys sit at `column`, sorted by key.
fn map_lines(object: &serde_json::Map<String, Value>, column: usize) -> Vec<String> {
    let pad = " ".repeat(column);
    let mut entries: Vec<(&String, &Value)> = object.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut lines = Vec::new();
    for (key, value) in entries {
        if key.contains('\n') {
            lines.extend(complex_entry(key, value, column));
            continue;
        }
        let key = scalar_text(key);
        match value {
            Value::String(text) if is_literal(text) => {
                lines.extend(literal(
                    &format!("{pad}{key}: "),
                    text,
                    child_column(column),
                ));
            }
            _ if is_block(value) => {
                lines.push(format!("{pad}{key}:"));
                lines.extend(block_lines(value, child_column(column)));
            }
            _ => lines.push(format!("{pad}{key}: {}", inline(value))),
        }
    }
    lines
}

/// A mapping entry whose key holds a newline. yaml.v3 writes the key as a
/// `?` complex key and the value after a `:` on its own line.
fn complex_entry(key: &str, value: &Value, column: usize) -> Vec<String> {
    let pad = " ".repeat(column);
    let child = child_column(column);
    let mut lines = if block_allowed(key) {
        literal(&format!("{pad}? "), key, child)
    } else {
        vec![format!("{pad}? {}", double_quote(key))]
    };
    match value {
        Value::String(text) if is_literal(text) => {
            lines.extend(literal(&format!("{pad}: "), text, child));
        }
        _ if is_block(value) => {
            let mut body = block_lines(value, child);
            // The first entry of the value follows the colon, padded to its column.
            let first = body.remove(0);
            let gap = " ".repeat(child - column - 1);
            lines.push(format!("{pad}:{gap}{}", &first[child..]));
            lines.extend(body);
        }
        _ => lines.push(format!("{pad}: {}", inline(value))),
    }
    lines
}

/// The lines of a non-empty sequence whose dashes sit at `column`. An item that
/// is a mapping or sequence starts on the dash line, and its later lines keep
/// the column they were written at.
fn seq_lines(items: &[Value], column: usize) -> Vec<String> {
    let pad = " ".repeat(column);
    let item_column = column + 2;
    let mut lines = Vec::new();
    for item in items {
        match item {
            Value::String(text) if is_literal(text) => {
                lines.extend(literal(&format!("{pad}- "), text, item_column));
            }
            _ if is_block(item) => {
                let mut body = block_lines(item, item_column);
                let first = body.remove(0);
                lines.push(format!("{pad}- {}", &first[item_column..]));
                lines.extend(body);
            }
            _ => lines.push(format!("{pad}- {}", inline(item))),
        }
    }
    lines
}

/// The lines of a mapping or sequence that is not empty, at `column`.
fn block_lines(value: &Value, column: usize) -> Vec<String> {
    match value {
        Value::Object(object) => map_lines(object, column),
        Value::Array(items) => seq_lines(items, column),
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
        Value::String(text) => scalar_text(text),
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
    }
}

/// Whether yaml.v3 writes a string as a literal block: it holds a newline and
/// the literal style is allowed for it.
fn is_literal(text: &str) -> bool {
    text.contains('\n') && block_allowed(text)
}

/// yaml.v3's `literal` writer. The first line is `prefix`, the indicator and
/// the hints. Each line of the text is then written at `indent`, and a break in
/// the text ends a line. The lines come back as they are written, with the
/// last break of the text ending the final line instead of adding one.
fn literal(prefix: &str, text: &str, indent: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut header = format!("{prefix}|");
    if chars.first().is_some_and(|&ch| ch == ' ' || is_break(ch)) {
        header.push_str(&BEST_INDENT.to_string());
    }
    let chomp = match chars.len() {
        0 => "-",
        n if !is_break(chars[n - 1]) => "-",
        1 => "+",
        n if is_break(chars[n - 2]) => "+",
        _ => "",
    };
    header.push_str(chomp);

    let mut out = header;
    let mut column = out.chars().count();
    let mut at_line_start = true;
    for &ch in &chars {
        if is_break(ch) {
            out.push(ch);
            column = 0;
            at_line_start = true;
        } else {
            if at_line_start {
                // The header line is still open: end it before the first line.
                if column > 0 {
                    out.push('\n');
                    column = 0;
                }
                while column < indent {
                    out.push(' ');
                    column += 1;
                }
            }
            out.push(ch);
            column += 1;
            at_line_start = false;
        }
    }

    let mut lines: Vec<String> = out.split('\n').map(str::to_owned).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Whether a literal block may hold this text: yaml.v3 refuses one when a
/// space ends the text or ends a line, or when a character is not printable.
fn block_allowed(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let special = chars.iter().any(|&ch| ch != '\t' && !is_printable(ch));
    let trailing_space = chars.last() == Some(&' ');
    let space_break = chars
        .windows(2)
        .any(|pair| pair[0] == ' ' && is_break(pair[1]));
    !(special || trailing_space || space_break)
}

/// yaml.v3's `is_printable`: the characters a scalar can hold without an
/// escape. Tab is handled apart from this, and any character above U+FFFF is not
/// printable in yaml.v3, so an emoji is escaped.
fn is_printable(ch: char) -> bool {
    matches!(ch as u32, 0x0A | 0x20..=0x7E | 0xA0..=0xD7FF | 0xE000..=0xFFFD) && ch != '\u{FEFF}'
}

/// The line breaks yaml.v3 recognises.
fn is_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
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

/// A string written on one line, as yaml.v3 writes it: double quoted when it
/// holds a newline, a tab or a character that is not printable, or when it
/// would read back as something other than a string; plain when block style
/// allows; single quoted otherwise.
fn scalar_text(text: &str) -> String {
    if text.contains('\n')
        || text.chars().any(|ch| ch == '\t' || !is_printable(ch))
        || reads_as_non_string(text)
    {
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
    // A document marker at the start of the text is an indicator.
    if text.starts_with("---") || text.starts_with("...") {
        return false;
    }
    !(text.starts_with(' ')
        || text.ends_with(' ')
        || text.ends_with(':')
        || text.contains(": ")
        || text.contains(" #"))
}

/// A double-quoted scalar. yaml.v3 writes the named escapes for the characters
/// that have one, and `\x`, `\u` or `\U` with upper-case digits for the rest of
/// the characters that are not printable.
fn double_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        let escape = ch == '"' || ch == '\\' || ch == '\t' || is_break(ch) || !is_printable(ch);
        if !escape {
            out.push(ch);
            continue;
        }
        let named = match ch {
            '\0' => Some("\\0"),
            '\u{7}' => Some("\\a"),
            '\u{8}' => Some("\\b"),
            '\t' => Some("\\t"),
            '\n' => Some("\\n"),
            '\u{B}' => Some("\\v"),
            '\u{C}' => Some("\\f"),
            '\r' => Some("\\r"),
            '\u{1B}' => Some("\\e"),
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\u{85}' => Some("\\N"),
            '\u{2028}' => Some("\\L"),
            '\u{2029}' => Some("\\P"),
            _ => None,
        };
        match named {
            Some(escape) => out.push_str(escape),
            None => {
                let code = ch as u32;
                if code <= 0xFF {
                    out.push_str(&format!("\\x{code:02X}"));
                } else if code <= 0xFFFF {
                    out.push_str(&format!("\\u{code:04X}"));
                } else {
                    out.push_str(&format!("\\U{code:08X}"));
                }
            }
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
        assert_eq!(scalar_text("reranker"), "reranker");
        assert_eq!(scalar_text("$schema"), "$schema");
        assert_eq!(scalar_text("true"), "\"true\"");
        assert_eq!(scalar_text("123"), "\"123\"");
        assert_eq!(scalar_text(""), "\"\"");
        assert_eq!(scalar_text("a: b"), "'a: b'");
        assert_eq!(scalar_text("it's"), "it's");
        assert_eq!(scalar_text("y"), "\"y\"");
        assert_eq!(scalar_text("-x"), "-x");
        assert_eq!(scalar_text("- x"), "'- x'");
        assert_eq!(scalar_text("*star"), "'*star'");
        assert_eq!(scalar_text("---x"), "'---x'");
        assert_eq!(scalar_text("a\tb"), "\"a\\tb\"");
        assert_eq!(scalar_text("\u{1F600}"), "\"\\U0001F600\"");
    }

    #[test]
    fn literal_blocks_pick_their_chomping_hint() {
        assert_eq!(encode(&json!({"k": "a\nb"})), "k: |-\n    a\n    b\n");
        assert_eq!(encode(&json!({"k": "a\nb\n"})), "k: |\n    a\n    b\n");
        assert_eq!(encode(&json!({"k": "a\nb\n\n"})), "k: |+\n    a\n    b\n\n");
        assert_eq!(encode(&json!({"k": "\na"})), "k: |4-\n    a\n");
        assert_eq!(encode(&json!({"k": "\n"})), "k: |4+\n");
        assert_eq!(encode(&json!({"k": " a\nb"})), "k: |4-\n     a\n    b\n");
    }

    #[test]
    fn literal_blocks_follow_the_emitter_columns() {
        assert_eq!(encode(&json!([{"k": {"j": "v"}}])), "- k:\n    j: v\n");
        assert_eq!(
            encode(&json!({"k": ["x\ny\n", {"m": {"n": "p\nq"}}]})),
            "k:\n    - |\n      x\n      y\n    - m:\n        \"n\": |-\n            p\n            q\n"
        );
        assert_eq!(encode(&json!([["a\nb"]])), "- - |-\n    a\n    b\n");
        assert_eq!(
            encode(&json!({"k\nx": ["q"]})),
            "? |-\n    k\n    x\n:   - q\n"
        );
    }

    #[test]
    fn strings_with_breaks_that_cannot_be_literal_are_double_quoted() {
        assert_eq!(encode(&json!({"k": "a \nb"})), "k: \"a \\nb\"\n");
        assert_eq!(encode(&json!({"k": "x\ty\nz "})), "k: \"x\\ty\\nz \"\n");
        assert_eq!(encode(&json!({"k": "x\u{1}y\nz"})), "k: \"x\\x01y\\nz\"\n");
    }

    #[test]
    fn empty_collections_are_flow_style() {
        assert_eq!(encode(&json!({"a": {}, "b": []})), "a: {}\nb: []\n");
    }
}
