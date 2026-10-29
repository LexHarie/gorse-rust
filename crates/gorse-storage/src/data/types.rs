// Ports: storage/data/database.go (ValidateLabels, Item, ScoredItem, ItemPatch, User, UserPatch, FeedbackKey, Feedback, SortFeedbacks) @ add21b5

//! The record types of the data store and the helpers that work on them:
//! [`User`], [`Item`], [`Feedback`] and their patches, with [`Labels`] for
//! Go's `any` labels. The `Database` interface and its backends come in later
//! work packages.
//!
//! JSON follows Go's `encoding/json` on these structs. Go gives them no JSON
//! tags, so each field's JSON name is its Go name, embedded structs are
//! flattened, and `UpdateAt` is skipped (`json:"-"`). The gorm and mapstructure
//! tags belong to the stores and to Go's config code, and are not ported here.

use gorse_common::gosort;
use gorse_common::gotime::Time;
use gorse_common::jsonutil;
use indexmap::IndexMap;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

/// Go's `any` as Gorse's labels hold it: the values a JSON document decodes to
/// (`nil`, `bool`, `float64`, `json.Number`, `string`, `[]any` and
/// `map[string]any`).
///
/// Go's `json.Unmarshal` into an `any` gives a `float64` for every number, and
/// that is what [`Deserialize`] gives too. [`Labels::Number`] is the
/// `json.Number` that a decoder with `UseNumber` gives, which keeps the text.
/// [`validate_labels`] rejects `Bool` and `Float64`, as Go's `ValidateLabels`
/// rejects `bool` and `float64`.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Labels {
    /// Go's `nil`, which marshals as `null`. It is the zero value, and the
    /// value of a patch field that does not change anything.
    #[default]
    Nil,
    /// A `bool`.
    Bool(bool),
    /// A `float64`.
    Float64(f64),
    /// A `json.Number`: a JSON number, kept as its text.
    Number(String),
    /// A `string`.
    String(String),
    /// A `[]any`.
    Array(Vec<Labels>),
    /// A `map[string]any`. The names keep the order they were inserted in,
    /// since Go's maps have no order. They marshal sorted by name, as
    /// `encoding/json` writes maps.
    Object(IndexMap<String, Labels>),
}

/// Go's `jsontext` `maxNestingDepth`: the deepest nesting that `json.Marshal`
/// and `json.Unmarshal` accept. Labels come from JSON that Go decodes with the
/// same limit, so a list or object opened past it is refused here too.
pub const MAX_NESTING_DEPTH: usize = 10_000;

/// Checks the nesting of a JSON text as Go's scanner does. A list or an object
/// opened while [`MAX_NESTING_DEPTH`] are already open is refused with Go's
/// text, `exceeded max depth`. The error has no position, as Go's has none.
pub fn check_nesting(json: &[u8]) -> Result<(), serde_json::Error> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in json {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' if depth == MAX_NESTING_DEPTH => {
                return Err(<serde_json::Error as de::Error>::custom(
                    "exceeded max depth",
                ));
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// Go's `jsonutil.Unmarshal` of labels, with Go's text for nesting past
/// [`MAX_NESTING_DEPTH`]. The nesting is checked before the value is decoded,
/// since a deserializer's error carries the position it failed at.
pub fn unmarshal_labels(json: &[u8]) -> Result<Labels, serde_json::Error> {
    check_nesting(json)?;
    let mut labels = Labels::Nil;
    jsonutil::unmarshal(json, &mut labels)?;
    Ok(labels)
}

impl Serialize for Labels {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The labels are written as text by a loop and handed over as raw JSON,
        // so a value nested deeper than the call stack still serializes.
        let text = labels_json(self).map_err(<S::Error as serde::ser::Error>::custom)?;
        check_nesting(text.as_bytes()).map_err(<S::Error as serde::ser::Error>::custom)?;
        let raw = RawValue::from_string(text).map_err(<S::Error as serde::ser::Error>::custom)?;
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Labels {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // The value is taken as its raw text, which a JSON deserializer reads
        // without recursing, and the labels are built from that text by a loop.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        check_nesting(raw.get().as_bytes()).map_err(<D::Error as de::Error>::custom)?;
        labels_from_json(raw.get()).map_err(<D::Error as de::Error>::custom)
    }
}

impl Drop for Labels {
    fn drop(&mut self) {
        // Dropping nested labels would recurse, so the children are taken out
        // and dropped in a loop, as `gojson::Value` does.
        let mut pending = match self {
            Labels::Array(items) => std::mem::take(items),
            Labels::Object(entries) => std::mem::take(entries).into_values().collect(),
            _ => return,
        };
        while let Some(mut value) = pending.pop() {
            match &mut value {
                Labels::Array(items) => pending.append(items),
                Labels::Object(entries) => {
                    pending.extend(std::mem::take(entries).into_values());
                }
                _ => {}
            }
        }
    }
}

/// Reads the labels that a JSON text holds, numbers as `float64`, as
/// [`Deserialize`] does. The text has already been checked as JSON by the
/// deserializer that captured it. The lists and objects being read are kept on
/// a stack, so a value nested at any depth is read in full.
fn labels_from_json(text: &str) -> Result<Labels, String> {
    /// A list or an object that is still being read.
    enum Open {
        List(Vec<Labels>),
        /// The entries read so far, and the name of the value being read.
        Object(IndexMap<String, Labels>, String),
    }
    let mut reader = Reader { text, pos: 0 };
    let mut open: Vec<Open> = Vec::new();
    loop {
        // Read the next value. A list or an object with entries opens a frame,
        // and its first value is read next.
        let value = match reader.peek() {
            Some(b'[') => {
                reader.pos += 1;
                if reader.eat(b']') {
                    Labels::Array(Vec::new())
                } else {
                    open.push(Open::List(Vec::new()));
                    continue;
                }
            }
            Some(b'{') => {
                reader.pos += 1;
                if reader.eat(b'}') {
                    Labels::Object(IndexMap::new())
                } else {
                    let name = reader.name()?;
                    open.push(Open::Object(IndexMap::new(), name));
                    continue;
                }
            }
            Some(b'"') => Labels::String(reader.string()?),
            Some(b't') => {
                reader.word("true")?;
                Labels::Bool(true)
            }
            Some(b'f') => {
                reader.word("false")?;
                Labels::Bool(false)
            }
            Some(b'n') => {
                reader.word("null")?;
                Labels::Nil
            }
            Some(_) => Labels::Float64(reader.number()?),
            None => return Err("unexpected end of JSON input".to_owned()),
        };
        // Give the value to the innermost frame. A separator then either moves
        // on to the next value, or closes the frame, which is a value in turn.
        let mut value = value;
        loop {
            match open.last_mut() {
                None => return Ok(value),
                Some(Open::List(items)) => items.push(value),
                Some(Open::Object(entries, name)) => {
                    entries.insert(std::mem::take(name), value);
                }
            }
            if reader.eat(b',') {
                if let Some(Open::Object(_, name)) = open.last_mut() {
                    *name = reader.name()?;
                }
                break;
            }
            value = match open.pop() {
                Some(Open::List(items)) if reader.eat(b']') => Labels::Array(items),
                Some(Open::Object(entries, _)) if reader.eat(b'}') => Labels::Object(entries),
                _ => return Err(format!("expected a separator at byte {}", reader.pos)),
            };
        }
    }
}

/// A cursor over a JSON text that has been checked as JSON.
struct Reader<'a> {
    text: &'a str,
    pos: usize,
}

impl Reader<'_> {
    /// The next byte that is not white space. It is not read.
    fn peek(&mut self) -> Option<u8> {
        self.skip_space();
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while matches!(
            self.text.as_bytes().get(self.pos),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.pos += 1;
        }
    }

    /// Reads `byte` when it is the next byte that is not white space.
    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// A JSON string. serde_json decodes the escapes of the quoted text.
    fn string(&mut self) -> Result<String, String> {
        self.skip_space();
        let bytes = self.text.as_bytes();
        let start = self.pos;
        let mut end = start + 1;
        while bytes.get(end).is_some_and(|&byte| byte != b'"') {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        self.pos = end + 1;
        let token = self
            .text
            .get(start..=end)
            .ok_or_else(|| format!("unterminated string at byte {start}"))?;
        serde_json::from_str(token).map_err(|err| err.to_string())
    }

    /// The name of an object's entry, and the colon after it.
    fn name(&mut self) -> Result<String, String> {
        let name = self.string()?;
        if self.eat(b':') {
            Ok(name)
        } else {
            Err(format!("expected ':' at byte {}", self.pos))
        }
    }

    /// A JSON number, as `float64`. Go's decoder rejects a number that
    /// overflows, and so does this.
    fn number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while matches!(
            self.text.as_bytes().get(self.pos),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        let value: f64 = self.text[start..self.pos]
            .parse()
            .map_err(|_| format!("invalid number at byte {start}"))?;
        if value.is_infinite() {
            return Err(format!("number out of range at byte {start}"));
        }
        Ok(value)
    }

    /// One of the words `true`, `false` and `null`.
    fn word(&mut self, word: &str) -> Result<(), String> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(format!("invalid literal at byte {}", self.pos))
        }
    }
}

/// A label that [`validate_labels`] rejects.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LabelsError {
    /// A list whose elements are not all strings or all numbers, or whose
    /// first element is neither. The text is the list as JSON.
    #[error("unsupported labels: {0}")]
    Unsupported(String),
    /// A value of a type Gorse's labels cannot hold; the text names the type.
    #[error("unsupported type in labels: {0}")]
    UnsupportedType(&'static str),
}

/// Go's `ValidateLabels`: checks that labels consist of
/// - a list of strings, or a list of numbers (`json.Number`),
/// - a map of names to valid labels,
/// - a string or a number,
/// - or nothing (`nil`).
///
/// An empty list is valid. Booleans and `float64`s are not.
///
/// The check walks a stack instead of recursing, so a value nested to the
/// decoder's limit is checked in full, as Go's check does.
pub fn validate_labels(labels: &Labels) -> Result<(), LabelsError> {
    let mut pending = vec![labels];
    while let Some(labels) = pending.pop() {
        match labels {
            Labels::Nil | Labels::String(_) | Labels::Number(_) => {}
            Labels::Array(items) => {
                let Some(first) = items.first() else {
                    continue;
                };
                let all = match first {
                    Labels::String(_) => items.iter().all(|v| matches!(v, Labels::String(_))),
                    Labels::Number(_) => items.iter().all(|v| matches!(v, Labels::Number(_))),
                    _ => false,
                };
                if !all {
                    return Err(unsupported_labels(items));
                }
            }
            // The values are pushed in reverse, so the first name is checked first.
            Labels::Object(map) => pending.extend(map.values().rev()),
            Labels::Bool(_) => return Err(LabelsError::UnsupportedType("bool")),
            Labels::Float64(_) => return Err(LabelsError::UnsupportedType("float64")),
        }
    }
    Ok(())
}

/// The error for a list of labels that do not validate, with the list as JSON.
fn unsupported_labels(items: &[Labels]) -> LabelsError {
    // A number that cannot be written is an error Go's `MustMarshal` panics on
    // too, and the labels here are always writable.
    LabelsError::Unsupported(list_json(items).expect("labels marshal as JSON"))
}

/// One piece of the JSON text of labels. The pieces are pushed in reverse, so
/// that they pop in the order they are written.
enum Step<'a> {
    Value(&'a Labels),
    Key(&'a str),
    Text(&'static str),
}

/// The labels as `json.Marshal` writes them, through `jsonutil`. The lists and
/// maps are opened and closed on a stack, not by recursion, so labels nested
/// at any depth are written in full.
pub(crate) fn labels_json(labels: &Labels) -> Result<String, serde_json::Error> {
    write_steps(vec![Step::Value(labels)])
}

/// The labels of a list, as the text that [`LabelsError::Unsupported`] holds.
fn list_json(items: &[Labels]) -> Result<String, serde_json::Error> {
    let mut steps = Vec::new();
    push_list(&mut steps, items);
    steps.push(Step::Text("["));
    write_steps(steps)
}

/// Pushes the pieces of a list, closing bracket included, in reverse order.
fn push_list<'a>(steps: &mut Vec<Step<'a>>, items: &'a [Labels]) {
    steps.push(Step::Text("]"));
    for (index, item) in items.iter().enumerate().rev() {
        steps.push(Step::Value(item));
        if index > 0 {
            steps.push(Step::Text(","));
        }
    }
}

/// Writes the pieces, popping them in order.
fn write_steps(mut steps: Vec<Step<'_>>) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    while let Some(step) = steps.pop() {
        match step {
            Step::Text(text) => out.push_str(text),
            Step::Key(name) => {
                out.push_str(utf8(&jsonutil::marshal(name)?));
                out.push(':');
            }
            Step::Value(Labels::Nil) => out.push_str("null"),
            Step::Value(Labels::Bool(value)) => {
                out.push_str(if *value { "true" } else { "false" });
            }
            Step::Value(Labels::Float64(value)) => {
                out.push_str(utf8(&jsonutil::marshal(value)?));
            }
            Step::Value(Labels::Number(text)) => {
                // The text is written as it is, as `json.Marshal` writes a
                // json.Number. A text that is not a JSON number is an error.
                out.push_str(RawValue::from_string(text.clone())?.get());
            }
            Step::Value(Labels::String(value)) => {
                out.push_str(utf8(&jsonutil::marshal(value)?));
            }
            Step::Value(Labels::Array(items)) => {
                out.push('[');
                push_list(&mut steps, items);
            }
            Step::Value(Labels::Object(entries)) => {
                out.push('{');
                steps.push(Step::Text("}"));
                // encoding/json writes a map's names in sorted order.
                let mut sorted: Vec<(&String, &Labels)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                for (index, &(name, item)) in sorted.iter().enumerate().rev() {
                    steps.push(Step::Value(item));
                    steps.push(Step::Key(name));
                    if index > 0 {
                        steps.push(Step::Text(","));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The bytes that `jsonutil` wrote, as text. serde_json writes UTF-8.
fn utf8(json: &[u8]) -> &str {
    std::str::from_utf8(json).expect("serde_json writes UTF-8")
}

/// Labels nested `depth` levels, alternating lists and maps from the outside
/// in, with the text that Go writes for them. The text is built as the labels
/// are, so the test compares two values that were made apart.
#[cfg(test)]
pub(crate) fn nested_labels(depth: usize) -> (Labels, String) {
    let mut value = Labels::String("leaf".to_owned());
    let mut opened = Vec::with_capacity(depth);
    let mut closed = String::new();
    for level in 0..depth {
        if level % 2 == 0 {
            value = Labels::Array(vec![value]);
            opened.push("[");
            closed.push(']');
        } else {
            value = Labels::Object(IndexMap::from([("k".to_owned(), value)]));
            opened.push("{\"k\":");
            closed.push('}');
        }
    }
    let text = format!(
        "{}\"leaf\"{closed}",
        opened.iter().rev().copied().collect::<String>()
    );
    (value, text)
}

/// An item: `storage/data`'s `Item`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Item {
    /// The item id (`ItemId`), the primary key.
    pub item_id: String,
    /// Whether the item is hidden from recommendations (`IsHidden`).
    pub is_hidden: bool,
    /// The item's categories (`Categories`). `None` is Go's nil slice, which
    /// marshals as `null`; an empty list is `Some` of an empty vector.
    pub categories: Option<Vec<String>>,
    /// The item's time stamp (`Timestamp`).
    pub timestamp: Time,
    /// The latest database write time, assigned by the store and skipped in
    /// JSON (`UpdateAt`).
    #[serde(skip)]
    pub update_at: Time,
    /// The item's labels (`Labels`).
    pub labels: Labels,
    /// The item's comment (`Comment`).
    pub comment: String,
}

/// An item with its search relevance score: `storage/data`'s `ScoredItem`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ScoredItem {
    /// The item, whose fields are flattened into this object as in Go.
    #[serde(flatten)]
    pub item: Item,
    /// The relevance score (`Score`).
    pub score: f64,
}

/// A modification of an item: `storage/data`'s `ItemPatch`. A `None` field,
/// or [`Labels::Nil`], leaves the item's value as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ItemPatch {
    /// Sets `IsHidden` when `Some`.
    pub is_hidden: Option<bool>,
    /// Sets `Categories` when `Some`, even to an empty list.
    pub categories: Option<Vec<String>>,
    /// Sets `Timestamp` when `Some`.
    pub timestamp: Option<Time>,
    /// Sets `Labels` unless it is [`Labels::Nil`], as Go's `patch.Labels != nil`
    /// does.
    pub labels: Labels,
    /// Sets `Comment` when `Some`.
    pub comment: Option<String>,
}

/// A user: `storage/data`'s `User`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct User {
    /// The user id (`UserId`), the primary key.
    pub user_id: String,
    /// The user's labels (`Labels`).
    pub labels: Labels,
    /// The user's comment (`Comment`).
    pub comment: String,
    /// The latest database write time, assigned by the store and skipped in
    /// JSON (`UpdateAt`).
    #[serde(skip)]
    pub update_at: Time,
}

/// A modification of a user: `storage/data`'s `UserPatch`. Each `None` or
/// [`Labels::Nil`] field leaves the user's value as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserPatch {
    /// Sets `Labels` unless it is [`Labels::Nil`].
    pub labels: Labels,
    /// Sets `Comment` when `Some`.
    pub comment: Option<String>,
}

/// The key that identifies a feedback: `storage/data`'s `FeedbackKey`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FeedbackKey {
    /// The feedback type (`FeedbackType`).
    pub feedback_type: String,
    /// The user (`UserId`).
    pub user_id: String,
    /// The item (`ItemId`).
    pub item_id: String,
}

/// A feedback: `storage/data`'s `Feedback`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Feedback {
    /// The key, whose fields are flattened into this object as in Go.
    #[serde(flatten)]
    pub feedback_key: FeedbackKey,
    /// The feedback's value (`Value`).
    pub value: f64,
    /// When the feedback happened (`Timestamp`).
    pub timestamp: Time,
    /// When the feedback was last written (`Updated`).
    pub updated: Time,
    /// The feedback's labels (`Labels`).
    pub labels: Labels,
    /// The feedback's comment (`Comment`).
    pub comment: String,
}

/// Go's `SortFeedbacks`: sorts feedback from the latest to the oldest, with
/// Go's `sort.Sort`. The sort is not stable, so feedback with equal time
/// stamps comes out in the order Go puts it.
pub fn sort_feedbacks(feedback: &mut [Feedback]) {
    gosort::sort(&mut FeedbackSorter(feedback));
}

/// Go's `feedbackSorter`: the feedback, latest first.
struct FeedbackSorter<'a>(&'a mut [Feedback]);

impl gosort::Interface for FeedbackSorter<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.0[i].timestamp.after(&self.0[j].timestamp)
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use gorse_common::gotime::{self, Month};

    use super::*;

    #[test]
    fn validate_labels_writes_a_deep_list_in_full() {
        // A list nested past the serde limit is refused with its whole text,
        // as Go refuses it, and the walk does not recurse on the way down.
        let mut deep = Labels::Nil;
        for _ in 0..300 {
            deep = array(vec![deep]);
        }
        let text = format!("{}null{}", "[".repeat(300), "]".repeat(300));
        assert_eq!(validate_labels(&deep), Err(LabelsError::Unsupported(text)));
    }

    #[test]
    fn validate_labels_accepts_a_deep_map() {
        let mut deep = string("leaf");
        for _ in 0..300 {
            deep = object(vec![("next", deep)]);
        }
        assert_eq!(validate_labels(&deep), Ok(()));
    }

    #[test]
    fn labels_json_writes_names_sorted_and_escaped() {
        let value = array(vec![object(vec![
            ("b", number("2.50")),
            ("a", string("<&>")),
        ])]);
        assert_eq!(
            validate_labels(&value),
            // Go's json.Marshal escapes <, > and & (checked with Go 1.27.0).
            Err(LabelsError::Unsupported(
                r#"[{"a":"\u003c\u0026\u003e","b":2.50}]"#.to_owned()
            ))
        );
    }

    fn number(text: &str) -> Labels {
        Labels::Number(text.to_owned())
    }

    fn string(text: &str) -> Labels {
        Labels::String(text.to_owned())
    }

    fn array(items: Vec<Labels>) -> Labels {
        Labels::Array(items)
    }

    fn object(entries: Vec<(&str, Labels)>) -> Labels {
        Labels::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    fn key(feedback_type: &str, user_id: &str, item_id: &str) -> FeedbackKey {
        FeedbackKey {
            feedback_type: feedback_type.to_owned(),
            user_id: user_id.to_owned(),
            item_id: item_id.to_owned(),
        }
    }

    #[test]
    fn test_sort_feedbacks() {
        let utc = gotime::utc();
        let mut feedback = vec![
            Feedback {
                feedback_key: key("star", "1", "1"),
                timestamp: gotime::date(2000, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                ..Default::default()
            },
            Feedback {
                feedback_key: key("like", "1", "1"),
                timestamp: gotime::date(2001, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                ..Default::default()
            },
            Feedback {
                feedback_key: key("read", "1", "1"),
                timestamp: gotime::date(2002, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                ..Default::default()
            },
        ];
        sort_feedbacks(&mut feedback);
        assert_eq!(
            feedback,
            vec![
                Feedback {
                    feedback_key: key("read", "1", "1"),
                    timestamp: gotime::date(2002, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                    ..Default::default()
                },
                Feedback {
                    feedback_key: key("like", "1", "1"),
                    timestamp: gotime::date(2001, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                    ..Default::default()
                },
                Feedback {
                    feedback_key: key("star", "1", "1"),
                    timestamp: gotime::date(2000, Month::OCTOBER, 1, 0, 0, 0, 0, &utc),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn test_validate_labels() {
        assert_eq!(validate_labels(&Labels::Nil), Ok(()));
        assert_eq!(validate_labels(&number("1")), Ok(()));
        assert_eq!(validate_labels(&string("label")), Ok(()));
        assert_eq!(
            validate_labels(&array(vec![number("1"), number("2"), number("3")])),
            Ok(())
        );
        assert_eq!(
            validate_labels(&array(vec![string("1"), string("2"), string("3")])),
            Ok(())
        );
        assert_eq!(
            validate_labels(&object(vec![
                ("city", number("1")),
                ("tags", array(vec![number("1"), number("2"), number("3")])),
            ])),
            Ok(())
        );
        assert_eq!(
            validate_labels(&object(vec![
                ("city", string("wenzhou")),
                ("tags", array(vec![string("1"), string("2"), string("3")])),
            ])),
            Ok(())
        );
        assert_eq!(
            validate_labels(&object(vec![(
                "address",
                object(vec![("province", number("1")), ("city", number("2"))]),
            )])),
            Ok(())
        );
        assert_eq!(
            validate_labels(&object(vec![(
                "address",
                object(vec![
                    ("province", string("zhejiang")),
                    ("city", string("wenzhou"))
                ]),
            )])),
            Ok(())
        );

        assert!(
            validate_labels(&object(vec![
                ("price", Labels::Float64(100.0)),
                ("tags", array(vec![number("1"), string("2"), string("3")])),
            ]))
            .is_err()
        );
        assert!(
            validate_labels(&object(vec![
                ("city", string("wenzhou")),
                ("tags", array(vec![string("1"), number("2"), string("3")])),
            ]))
            .is_err()
        );
        assert!(
            validate_labels(&object(vec![
                ("city", string("wenzhou")),
                ("tags", array(vec![string("1"), string("2"), number("3")])),
            ]))
            .is_err()
        );
    }

    #[test]
    fn validate_labels_reports_go_text() {
        // The texts are Go's: the list as `json.Marshal` writes it, or the
        // type name.
        assert_eq!(
            validate_labels(&array(vec![number("1"), string("2"), number("3")])),
            Err(LabelsError::Unsupported("[1,\"2\",3]".to_owned()))
        );
        assert_eq!(
            validate_labels(&array(vec![Labels::Nil])),
            Err(LabelsError::Unsupported("[null]".to_owned()))
        );
        assert_eq!(
            validate_labels(&object(vec![("ok", Labels::Bool(true))])),
            Err(LabelsError::UnsupportedType("bool"))
        );
        assert_eq!(
            validate_labels(&array(vec![Labels::Float64(1.5), Labels::Float64(2.0)])),
            Err(LabelsError::Unsupported("[1.5,2]".to_owned()))
        );
        assert_eq!(
            validate_labels(&Labels::Float64(100.0))
                .unwrap_err()
                .to_string(),
            "unsupported type in labels: float64"
        );
    }

    #[test]
    fn labels_marshal_as_go_does() {
        let labels = object(vec![
            ("b", array(vec![number("1.50"), Labels::Float64(2.0)])),
            ("a", Labels::Nil),
            ("c<&>", string("x")),
        ]);
        assert_eq!(
            String::from_utf8(jsonutil::marshal(&labels).expect("marshal")).expect("utf-8"),
            r#"{"a":null,"b":[1.50,2],"c\u003c\u0026\u003e":"x"}"#
        );
    }

    #[test]
    fn labels_decode_numbers_as_float64() {
        let labels: Labels =
            serde_json::from_str(r#"{"n":1.50,"list":[true,"x"]}"#).expect("decode");
        assert_eq!(
            labels,
            object(vec![
                ("n", Labels::Float64(1.5)),
                ("list", array(vec![Labels::Bool(true), string("x")])),
            ])
        );
    }

    #[test]
    fn labels_nested_ten_thousand_levels_round_trip_as_json() {
        labels_round_trip_as_json(MAX_NESTING_DEPTH);
    }

    #[test]
    fn labels_nested_past_the_go_limit_are_refused_as_go_refuses() {
        labels_refused_as_json(MAX_NESTING_DEPTH + 1);
    }

    #[test]
    fn labels_nested_one_hundred_thousand_levels_are_refused_as_go_refuses() {
        labels_refused_as_json(100_000);
    }

    #[test]
    fn check_nesting_counts_only_brackets_outside_strings() {
        // Brackets inside a string, and an escaped quote, are not levels.
        let text = format!("[\"{}\\\"]\", []]", "[".repeat(20_000));
        assert!(check_nesting(text.as_bytes()).is_ok());
        let deep = format!("{}{}", "[".repeat(10_001), "]".repeat(10_001));
        assert_eq!(
            check_nesting(deep.as_bytes())
                .expect_err("too deep")
                .to_string(),
            "exceeded max depth"
        );
    }

    /// Writes labels nested `depth` levels, which Go writes in full, and reads
    /// them back, as the SQL backends do. Both walks must finish without the
    /// call stack limiting them, and the text must not change on the way.
    fn labels_round_trip_as_json(depth: usize) {
        let (labels, text) = nested_labels(depth);
        let written =
            String::from_utf8(jsonutil::marshal(&labels).expect("marshal")).expect("utf-8");
        assert!(written == text, "labels were written differently");
        let read = unmarshal_labels(text.as_bytes()).expect("unmarshal");
        let again = String::from_utf8(jsonutil::marshal(&read).expect("marshal")).expect("utf-8");
        assert!(again == text, "labels changed on the way back");
    }

    /// Refuses labels nested past Go's limit, on writing and on reading, with
    /// the text Go gives: `json.Marshal` and `json.Unmarshal` both return
    /// `exceeded max depth` for them.
    fn labels_refused_as_json(depth: usize) {
        let (labels, text) = nested_labels(depth);
        let written = jsonutil::marshal(&labels).expect_err("marshal past the limit");
        assert_eq!(written.to_string(), "exceeded max depth");
        let read = unmarshal_labels(text.as_bytes()).expect_err("unmarshal past the limit");
        assert_eq!(read.to_string(), "exceeded max depth");
    }

    #[test]
    fn labels_with_a_number_that_is_not_json_are_an_error() {
        // The writer reports the error rather than panicking, as Go's
        // json.Marshal reports it.
        let labels = array(vec![array(vec![number("1.2.3")])]);
        assert!(jsonutil::marshal(&labels).is_err());
    }

    #[test]
    fn labels_read_duplicate_names_last_value_first_position() {
        let labels: Labels = serde_json::from_str(r#"{"a":1,"b":[],"a":"x"}"#).expect("decode");
        assert_eq!(
            String::from_utf8(jsonutil::marshal(&labels).expect("marshal")).expect("utf-8"),
            r#"{"a":"x","b":[]}"#
        );
    }

    #[test]
    fn item_json_uses_go_field_names() {
        let item = Item {
            item_id: "1".to_owned(),
            categories: Some(vec!["a".to_owned()]),
            labels: number("3"),
            comment: "c".to_owned(),
            update_at: gotime::unix(5, 0),
            ..Default::default()
        };
        assert_eq!(
            String::from_utf8(jsonutil::marshal(&item).expect("marshal")).expect("utf-8"),
            r#"{"ItemId":"1","IsHidden":false,"Categories":["a"],"Timestamp":"0001-01-01T00:00:00Z","Labels":3,"Comment":"c"}"#
        );
    }

    #[test]
    fn feedback_flattens_its_key() {
        let feedback = Feedback {
            feedback_key: key("star", "u", "i"),
            value: 1.0,
            ..Default::default()
        };
        let text =
            String::from_utf8(jsonutil::marshal(&feedback).expect("marshal")).expect("utf-8");
        assert!(
            text.starts_with(r#"{"FeedbackType":"star","UserId":"u","ItemId":"i","Value":1,"#),
            "{text}"
        );
        let back: Feedback = serde_json::from_str(&text).expect("decode");
        assert_eq!(back, feedback);
    }
}
