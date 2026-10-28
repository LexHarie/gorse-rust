//! `storage::data`'s record types against `tests/data/types_golden.json`, which
//! `contracts/tools/datatypesgolden` wrote by running Go's `json.Unmarshal` and
//! `json.Marshal` on each record type, and Go's `ValidateLabels` on labels
//! given as Go values.
//!
//! A marshal case decodes the recorded input into the Rust type and must write
//! back exactly Go's output. A labels case rebuilds the Go value from its tree
//! and must give the error text Go gave, or none.

use gorse_common::jsonutil;
use gorse_storage::data::types::{
    Feedback, FeedbackKey, Item, ItemPatch, Labels, ScoredItem, User, UserPatch, validate_labels,
};
use serde::Deserialize;

const GOLDEN: &str = include_str!("data/types_golden.json");

#[derive(Deserialize)]
struct Golden {
    marshal: Vec<MarshalCase>,
    labels: Vec<LabelsCase>,
}

#[derive(Deserialize)]
struct MarshalCase {
    #[serde(rename = "type")]
    typ: String,
    name: String,
    input: String,
    output: String,
}

#[derive(Deserialize)]
struct LabelsCase {
    name: String,
    labels: serde_json::Value,
    error: Option<String>,
}

/// The Go value a tree from the golden tool stands for, as a [`Labels`].
fn labels_from_tree(node: &serde_json::Value) -> Labels {
    let kind = node["kind"].as_str().expect("kind");
    match kind {
        "nil" => Labels::Nil,
        "bool" => Labels::Bool(node["value"].as_bool().expect("bool")),
        "float64" => Labels::Float64(node["value"].as_f64().expect("float64")),
        "number" => Labels::Number(node["text"].as_str().expect("text").to_owned()),
        "string" => Labels::String(node["value"].as_str().expect("string").to_owned()),
        "array" => Labels::Array(
            node["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(labels_from_tree)
                .collect(),
        ),
        "object" => Labels::Object(
            node["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .map(|entry| {
                    let key = entry["key"].as_str().expect("key").to_owned();
                    (key, labels_from_tree(&entry["value"]))
                })
                .collect(),
        ),
        other => panic!("unknown kind {other}"),
    }
}

/// Decodes `input` into the record type `typ` and writes it back as Go does.
fn round_trip(typ: &str, input: &str) -> String {
    let bytes = match typ {
        "Item" => jsonutil::marshal(&decode::<Item>(input)),
        "ScoredItem" => jsonutil::marshal(&decode::<ScoredItem>(input)),
        "ItemPatch" => jsonutil::marshal(&decode::<ItemPatch>(input)),
        "User" => jsonutil::marshal(&decode::<User>(input)),
        "UserPatch" => jsonutil::marshal(&decode::<UserPatch>(input)),
        "FeedbackKey" => jsonutil::marshal(&decode::<FeedbackKey>(input)),
        "Feedback" => jsonutil::marshal(&decode::<Feedback>(input)),
        other => panic!("unknown type {other}"),
    }
    .expect("marshal");
    String::from_utf8(bytes).expect("utf-8")
}

fn decode<T: for<'de> Deserialize<'de>>(input: &str) -> T {
    serde_json::from_str(input).expect("decode")
}

#[test]
fn record_types_round_trip_as_go_does() {
    let golden: Golden = serde_json::from_str(GOLDEN).expect("golden");
    assert_eq!(golden.marshal.len(), 19, "the golden's marshal cases");
    for case in &golden.marshal {
        let got = round_trip(&case.typ, &case.input);
        assert_eq!(got, case.output, "{} {}", case.typ, case.name);
    }
}

#[test]
fn validate_labels_gives_go_errors() {
    let golden: Golden = serde_json::from_str(GOLDEN).expect("golden");
    assert_eq!(golden.labels.len(), 26, "the golden's labels cases");
    for case in &golden.labels {
        let labels = labels_from_tree(&case.labels);
        let got = validate_labels(&labels).err().map(|err| err.to_string());
        assert_eq!(got, case.error, "{}", case.name);
    }
}
