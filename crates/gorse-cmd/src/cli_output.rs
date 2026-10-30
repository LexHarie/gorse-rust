// Ports: cmd/gorse-cli/table.go @ add21b5 and cmd/gorse-cli/format.go @ add21b5

//! The output of `gorse-cli`: an object as `Field: value` lines, an array as a
//! table, and everything else as JSON. Go works out the columns and the field
//! names from the types with reflection; here the values carry their Go field
//! names, and an empty array takes its columns from the caller.

use std::io::{self, Write};

use serde_json::{Map, Value};

use crate::cli_json::{marshal, marshal_indent};
use crate::cli_time::format_time_text;

/// The most scalar values an array shows inline before it is summarised.
pub const MAX_INLINE_ARRAY_VALUES: usize = 8;

/// The most values of a `Labels` array a table cell shows.
pub const MAX_TABLE_LABEL_ARRAY_VALUES: usize = 3;

/// The fields of an object that come first, in this order, when they are present.
const OBJECT_PRIORITY: [&str; 6] = [
    "UserId",
    "ItemId",
    "Comment",
    "Categories",
    "IsHidden",
    "Timestamp",
];

/// Go's `printStruct`: an object as its fields, anything else as JSON.
pub fn print_struct(out: &mut dyn Write, value: &Value) -> io::Result<()> {
    match value {
        Value::Object(object) => print_object_tables(out, object),
        _ => print_json_value(out, value),
    }
}

/// Go's `printObjectTables`: a user or an item as one object, and any other
/// object as its scalar fields, followed by one table for each array field.
fn print_object_tables(out: &mut dyn Write, object: &Map<String, Value>) -> io::Result<()> {
    if object.contains_key("UserId") || object.contains_key("ItemId") {
        return print_object_table(out, object);
    }
    let mut array_keys = Vec::new();
    let mut metadata = Map::new();
    for (key, child) in object {
        if child.is_array() {
            array_keys.push(key.clone());
        } else {
            metadata.insert(key.clone(), child.clone());
        }
    }
    if array_keys.is_empty() {
        return print_object_table(out, object);
    }
    array_keys.sort();
    if !metadata.is_empty() {
        print_object_table(out, &metadata)?;
        writeln!(out)?;
    }
    for (i, key) in array_keys.iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        print_array_table(out, &object[key], &[])?;
    }
    Ok(())
}

/// Go's `printObjectTable`: one `Field: value` line per field.
fn print_object_table(out: &mut dyn Write, object: &Map<String, Value>) -> io::Result<()> {
    if object.is_empty() {
        return writeln!(out, "{{}}");
    }
    let keys: Vec<String> = object.keys().cloned().collect();
    for key in ordered_keys(&keys, &OBJECT_PRIORITY) {
        print_object_field(out, &key, &object[&key])?;
    }
    Ok(())
}

/// Go's `printObjectField`. A long scalar array is summarised, an object is
/// printed as indented JSON on the lines after its name, and anything else is
/// its table form.
fn print_object_field(out: &mut dyn Write, key: &str, value: &Value) -> io::Result<()> {
    if let Some(summary) = format_array_summary(value, MAX_INLINE_ARRAY_VALUES) {
        return writeln!(out, "{key}: {summary}");
    }
    if value.is_object() {
        return writeln!(out, "{key}:\n{}", format_pretty_json(value));
    }
    writeln!(out, "{key}: {}", format_table_value(value))
}

/// Go's `printArrayTable`. An array of objects is a table with one column for
/// each field; an array of scalars is a table of index and value; anything that
/// is not an array is JSON. `element_columns` names the columns of an empty
/// array of objects, since there are no elements to read them from.
pub fn print_array_table(
    out: &mut dyn Write,
    value: &Value,
    element_columns: &[&str],
) -> io::Result<()> {
    let Value::Array(items) = value else {
        return print_json_value(out, value);
    };
    if items.is_empty() {
        let columns = if element_columns.is_empty() {
            vec!["Index".to_owned(), "Value".to_owned()]
        } else {
            let names: Vec<String> = element_columns
                .iter()
                .map(|name| (*name).to_owned())
                .collect();
            ordered_array_columns(&names)
        };
        return print_table(out, &columns, &[]);
    }
    if !items.iter().all(Value::is_object) {
        let rows: Vec<Vec<String>> = items
            .iter()
            .enumerate()
            .map(|(index, item)| vec![index.to_string(), format_table_value(item)])
            .collect();
        return print_table(out, &["Index".to_owned(), "Value".to_owned()], &rows);
    }
    let mut columns: Vec<String> = Vec::new();
    for item in items {
        if let Value::Object(object) = item {
            for column in object.keys() {
                if !columns.contains(column) {
                    columns.push(column.clone());
                }
            }
        }
    }
    let columns = ordered_array_columns(&columns);
    let empty = Value::Null;
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let object = item.as_object();
            columns
                .iter()
                .map(|column| {
                    let cell = object
                        .and_then(|object| object.get(column))
                        .unwrap_or(&empty);
                    format_table_cell_value(column, cell)
                })
                .collect()
        })
        .collect();
    print_table(out, &columns, &rows)
}

/// The column order of an array of objects: the columns that matter most for
/// the type come first, then the rest in name order.
fn ordered_array_columns(columns: &[String]) -> Vec<String> {
    if columns.iter().any(|column| column == "FeedbackType") {
        return ordered_keys(
            columns,
            &[
                "FeedbackType",
                "UserId",
                "ItemId",
                "Value",
                "Timestamp",
                "Comment",
                "Updated",
            ],
        );
    }
    if columns.iter().any(|column| column == "Progress") {
        return ordered_keys(
            columns,
            &[
                "Tracer",
                "Name",
                "Status",
                "Progress",
                "Error",
                "StartTime",
                "FinishTime",
            ],
        );
    }
    ordered_keys(columns, &OBJECT_PRIORITY)
}

/// Go's `orderedKeys`: the keys in `priority` that are present, in that order,
/// then the others sorted by name.
fn ordered_keys(keys: &[String], priority: &[&str]) -> Vec<String> {
    let mut ordered = Vec::with_capacity(keys.len());
    for name in priority {
        if keys.iter().any(|key| key == name) {
            ordered.push((*name).to_owned());
        }
    }
    let mut remaining: Vec<String> = keys
        .iter()
        .filter(|key| !priority.contains(&key.as_str()))
        .cloned()
        .collect();
    remaining.sort();
    ordered.extend(remaining);
    ordered
}

/// Go's `formatTableCellValue`: the `Labels` column is shown as a short JSON
/// summary, and other columns as their table form.
fn format_table_cell_value(column: &str, value: &Value) -> String {
    if column != "Labels" {
        return format_table_value(value);
    }
    if value.is_array() || value.is_object() {
        return marshal(&summarize_long_arrays(value, MAX_TABLE_LABEL_ARRAY_VALUES));
    }
    format_table_value(value)
}

/// Go's `formatTableValue`: the text of one value in a table or a field line.
pub fn format_table_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => format_time_text(text).unwrap_or_else(|| text.clone()),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(integer), _) => integer.to_string(),
            (_, Some(integer)) => integer.to_string(),
            _ => format!("{}", number.as_f64().unwrap_or(0.0)),
        },
        Value::Array(_) => format_array_summary(value, MAX_INLINE_ARRAY_VALUES)
            .unwrap_or_else(|| marshal(&summarize_long_arrays(value, MAX_INLINE_ARRAY_VALUES))),
        Value::Object(_) => marshal(&summarize_long_arrays(value, MAX_INLINE_ARRAY_VALUES)),
    }
}

/// Go's `formatPrettyJSON`: indented JSON with the long arrays summarised.
fn format_pretty_json(value: &Value) -> String {
    marshal_indent(&summarize_long_arrays(value, MAX_INLINE_ARRAY_VALUES), "  ")
}

/// Go's `summarizeLongArraysWithLimit`: each array of more than `max` scalars
/// becomes its summary text, and the rest of the value is kept as it is.
fn summarize_long_arrays(value: &Value, max: usize) -> Value {
    if let Some(summary) = format_array_summary(value, max) {
        return Value::String(summary);
    }
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| summarize_long_arrays(item, max))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), summarize_long_arrays(item, max)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Go's `formatArraySummaryWithLimit`: `[a, b, ...] (n values)` for an array of
/// more than `max` scalars, and `None` for anything else.
fn format_array_summary(value: &Value, max: usize) -> Option<String> {
    let Value::Array(items) = value else {
        return None;
    };
    let max = max.max(1);
    if items.len() <= max || !items.iter().all(is_scalar) {
        return None;
    }
    let shown: Vec<String> = items[..max].iter().map(marshal).collect();
    Some(format!(
        "[{}, ...] ({} values)",
        shown.join(", "),
        items.len()
    ))
}

fn is_scalar(value: &Value) -> bool {
    !value.is_array() && !value.is_object()
}

/// Go's `formatProgressBar`: a 20-cell bar and the percentage.
pub fn format_progress_bar(count: i64, total: i64) -> String {
    const WIDTH: i64 = 20;
    let count = count.max(0);
    let total = total.max(0);
    let count = if total > 0 && count > total {
        total
    } else {
        count
    };
    let ratio = if total > 0 {
        count as f64 / total as f64
    } else {
        0.0
    };
    let filled = ((ratio * WIDTH as f64 + 0.5) as i64).min(WIDTH);
    let percent = (ratio * 100.0 + 0.5) as i64;
    format!(
        "[{}{}] {percent}%",
        "#".repeat(filled as usize),
        "-".repeat((WIDTH - filled) as usize)
    )
}

/// Go's `printTable`: the headers and the rows, with each column padded to its
/// widest cell plus two spaces, as `text/tabwriter` pads them with its settings.
/// The last column is not padded.
pub fn print_table(
    out: &mut dyn Write,
    headers: &[String],
    rows: &[Vec<String>],
) -> io::Result<()> {
    let headers: Vec<String> = headers
        .iter()
        .map(|header| format_table_header(header))
        .collect();
    let mut lines = vec![headers];
    lines.extend(rows.iter().cloned());
    let columns = lines[0].len();
    let mut widths = vec![0usize; columns];
    for line in &lines {
        for (column, cell) in line.iter().enumerate().take(columns.saturating_sub(1)) {
            widths[column] = widths[column].max(cell.chars().count() + 2);
        }
    }
    for line in &lines {
        let mut text = String::new();
        for (column, cell) in line.iter().enumerate() {
            text.push_str(cell);
            if column + 1 < columns {
                let padding = widths[column].saturating_sub(cell.chars().count());
                text.push_str(&" ".repeat(padding));
            }
        }
        writeln!(out, "{text}")?;
    }
    Ok(())
}

/// Go's `formatTableHeader`: a field name in upper case words joined by `-`,
/// so `UpdateTime` is `UPDATE-TIME`.
pub fn format_table_header(header: &str) -> String {
    let runes: Vec<char> = header.chars().collect();
    let mut builder = String::new();
    for (i, &ch) in runes.iter().enumerate() {
        if ch == '.' || ch == '_' || ch == '-' || ch.is_whitespace() {
            if !builder.is_empty() {
                builder.push(' ');
            }
            continue;
        }
        if i > 0 && should_split_table_header(&runes, i) {
            builder.push(' ');
        }
        builder.extend(ch.to_uppercase());
    }
    builder.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Whether a word starts at `runes[i]`: an upper case letter after a lower case
/// letter or digit, or the last letter of an acronym before a lower case one.
fn should_split_table_header(runes: &[char], i: usize) -> bool {
    let current = runes[i];
    let previous = runes[i - 1];
    if !current.is_uppercase() {
        return false;
    }
    if previous.is_lowercase() || previous.is_ascii_digit() {
        return true;
    }
    previous.is_uppercase() && i + 1 < runes.len() && runes[i + 1].is_lowercase()
}

/// Go's `printJSONValue`: the value as one line of compact JSON.
pub fn print_json_value(out: &mut dyn Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{}", marshal(value))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn header(text: &str) -> String {
        format_table_header(text)
    }

    #[test]
    fn headers_split_camel_case_into_upper_words() {
        assert_eq!(header("ItemId"), "ITEM-ID");
        assert_eq!(header("UpdateTime"), "UPDATE-TIME");
        assert_eq!(header("UUID"), "UUID");
        assert_eq!(header("FeedbackType"), "FEEDBACK-TYPE");
        assert_eq!(header("Tracer"), "TRACER");
    }

    #[test]
    fn progress_bar_matches_go() {
        assert_eq!(format_progress_bar(0, 0), "[--------------------] 0%");
        assert_eq!(format_progress_bar(5, 10), "[##########----------] 50%");
        assert_eq!(format_progress_bar(12, 10), "[####################] 100%");
    }

    #[test]
    fn long_scalar_arrays_are_summarised_in_cells() {
        let value = json!([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]);
        assert_eq!(
            format_table_value(&value),
            "[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, ...] (10 values)"
        );
    }

    #[test]
    fn object_fields_print_as_lines() {
        let mut out = Vec::new();
        print_struct(
            &mut out,
            &json!({"UserId": "alice", "Labels": {"role": "tester"}, "Comment": ""}),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "UserId: alice\nComment: \nLabels:\n{\n  \"role\": \"tester\"\n}\n"
        );
    }

    #[test]
    fn empty_arrays_use_the_given_columns() {
        let mut out = Vec::new();
        print_array_table(&mut out, &json!([]), &["UserId", "Comment", "Labels"]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "USER-ID  COMMENT  LABELS\n"
        );
        let mut out = Vec::new();
        print_array_table(&mut out, &json!([]), &[]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "INDEX  VALUE\n");
    }

    #[test]
    fn table_columns_are_aligned_and_the_last_is_not_padded() {
        let mut out = Vec::new();
        print_table(
            &mut out,
            &["Name".to_owned(), "Count".to_owned()],
            &[
                vec!["alice".to_owned(), "1".to_owned()],
                vec!["bo".to_owned(), "22".to_owned()],
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "NAME   COUNT\nalice  1\nbo     22\n"
        );
    }
}
