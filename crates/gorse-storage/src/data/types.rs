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

use std::fmt;

use gorse_common::gosort;
use gorse_common::gotime::Time;
use gorse_common::jsonutil;
use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
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

impl Serialize for Labels {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Labels::Nil => serializer.serialize_none(),
            Labels::Bool(value) => serializer.serialize_bool(*value),
            Labels::Float64(value) => serializer.serialize_f64(*value),
            Labels::Number(text) => {
                // The text is written as it is, as `json.Marshal` writes a
                // json.Number. A text that is not a JSON number is an error.
                let raw = RawValue::from_string(text.clone())
                    .map_err(<S::Error as serde::ser::Error>::custom)?;
                raw.serialize(serializer)
            }
            Labels::String(value) => serializer.serialize_str(value),
            Labels::Array(items) => items.serialize(serializer),
            Labels::Object(map) => {
                // encoding/json writes a map's names in sorted order.
                let mut entries: Vec<(&String, &Labels)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                serializer.collect_map(entries)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Labels {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(LabelsVisitor)
    }
}

/// Decodes any JSON value into [`Labels`], numbers as `float64`.
struct LabelsVisitor;

impl<'de> Visitor<'de> for LabelsVisitor {
    type Value = Labels;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Labels, E> {
        Ok(Labels::Nil)
    }

    fn visit_none<E: de::Error>(self) -> Result<Labels, E> {
        Ok(Labels::Nil)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Labels, E> {
        Ok(Labels::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Labels, E> {
        Ok(Labels::Float64(value as f64))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Labels, E> {
        Ok(Labels::Float64(value as f64))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Labels, E> {
        Ok(Labels::Float64(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Labels, E> {
        Ok(Labels::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Labels, E> {
        Ok(Labels::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Labels, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Labels::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Labels, A::Error> {
        // A repeated name keeps its first position and takes its last value,
        // as Go's map does.
        let mut object = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<String, Labels>()? {
            object.insert(key, value);
        }
        Ok(Labels::Object(object))
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
pub fn validate_labels(labels: &Labels) -> Result<(), LabelsError> {
    match labels {
        Labels::Nil => Ok(()),
        Labels::Array(items) => {
            let Some(first) = items.first() else {
                return Ok(());
            };
            let all = match first {
                Labels::String(_) => items.iter().all(|v| matches!(v, Labels::String(_))),
                Labels::Number(_) => items.iter().all(|v| matches!(v, Labels::Number(_))),
                _ => false,
            };
            if all {
                Ok(())
            } else {
                Err(unsupported_labels(items))
            }
        }
        Labels::Object(map) => map.values().try_for_each(validate_labels),
        Labels::String(_) | Labels::Number(_) => Ok(()),
        Labels::Bool(_) => Err(LabelsError::UnsupportedType("bool")),
        Labels::Float64(_) => Err(LabelsError::UnsupportedType("float64")),
    }
}

/// The error for a list of labels that do not validate, with the list as JSON.
fn unsupported_labels(items: &[Labels]) -> LabelsError {
    // Go's jsonutil.MustMarshal panics here too. Labels hold no value JSON
    // cannot write, so this does not happen for decoded labels.
    let json = jsonutil::marshal(items).expect("labels marshal as JSON");
    LabelsError::Unsupported(String::from_utf8_lossy(&json).into_owned())
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
