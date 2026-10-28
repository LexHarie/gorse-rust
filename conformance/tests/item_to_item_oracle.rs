// Ports: logics/item_to_item.go and logics/user_to_user.go @ add21b5 (WP-3-5).

//! WP-3-5, the item-to-item and user-to-user recommenders, against the Go ones.
//!
//! `contracts/tools/item2itemgolden` builds 40 cases from seeded inputs: the
//! rows with their labels in the form the dataset gives them, the feedback, the
//! column expressions and the IDF. It runs the Go recommenders on each case, on
//! an xvec store, and writes the inputs with what Go's store holds afterwards
//! and the neighbours of each row. This test runs the same inputs through the
//! Rust recommenders on their own stores and compares: the vectors bit for bit,
//! and the neighbours of each row as a set. The store decides the order and the
//! last bits of a score, not the recommender: a sparse dot product sums in an
//! order that can round one FP32 ulp differently (`PORTING_NOTES.md` X-75), and
//! ties are broken by the smaller ID, which Go's `xvec` leaves unspecified. So a
//! neighbour's score is compared within `SCORE_TOLERANCE`, and the order of the
//! neighbours is not compared. A neighbour that only one store has is a
//! difference; the Go side's HNSW may miss one (O-30).
//!
//! The test fails when the Go toolchain is missing; it is never skipped
//! (`conformance/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_config::config::schema::{ItemToItemConfig, UserToUserConfig};
use gorse_dataset::dataset::{DatasetItem, DatasetUser, Id, Value as ProcessedLabels};
use gorse_logics::item_to_item::{ItemToItemOptions, new_item_to_item, query_item_to_item};
use gorse_logics::user_to_user::{UserToUserOptions, new_user_to_user, query_user_to_user};
use gorse_storage::cache::Score;
use gorse_storage::data::types::Labels;
use gorse_storage::vectors::{self, Database, Vector, VectorConfig};
use gorse_storage_core::errors::Error as StorageError;
use serde_json::{Value as Json, json};

/// The neighbours asked for each row, as the Go generator asks.
const NEIGHBOURS: usize = 5;

fn repo_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .expect("the repository root")
}

/// Runs the Go generator, and returns its cases.
fn go_cases() -> Vec<Json> {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("cases.json");
    let output = Command::new("go")
        .current_dir(repo_root())
        .args(["run", "./contracts/tools/item2itemgolden", "-out"])
        .arg(&out)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "starting go: {e}; the item-to-item oracle needs the Go toolchain \
                 (conformance/README.md)"
            )
        });
    assert!(
        output.status.success(),
        "go run ./contracts/tools/item2itemgolden failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("the Go generator did not write {}: {e}", out.display()));
    serde_json::from_str(&text).expect("the Go cases are JSON")
}

/// A JSON list of signed numbers. Go writes a nil slice as `null`, which is the
/// empty list.
fn i32s(v: &Json) -> Vec<i32> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| i32::try_from(x.as_i64().expect("int")).expect("int32"))
        .collect()
}

/// A JSON list of unsigned numbers, with `null` as the empty list.
fn u32s(v: &Json) -> Vec<u32> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| u32::try_from(x.as_u64().expect("uint")).expect("uint32"))
        .collect()
}

/// A JSON list with Go's nil slice, `null`, read as the empty list.
fn list(v: &Json) -> Json {
    if v.is_null() { json!([]) } else { v.clone() }
}

/// A JSON list of FP32 bits, as the IDF is written. `null` is Go's nil slice.
fn idf(v: &Json) -> Option<Vec<f32>> {
    v.as_array().map(|bits| {
        bits.iter()
            .map(|x| f32::from_bits(x.as_u64().expect("bits") as u32))
            .collect()
    })
}

/// A plain label, as Go's `any` holds a list element the dataset did not
/// process.
fn plain(v: &Json) -> Labels {
    let value = &v["v"];
    match v["t"].as_str().expect("the label's type") {
        "nil" => Labels::Nil,
        "bool" => Labels::Bool(value.as_bool().expect("bool")),
        "f64" => Labels::Float64(value.as_f64().expect("float64")),
        "string" => Labels::String(value.as_str().expect("string").to_owned()),
        other => panic!("a list element of type {other} is not generated"),
    }
}

/// A processed label, as the dataset gives it: `dataset.ID`s, FP16 bits and
/// maps are processed, and the rest are kept as they are.
fn label(v: &Json) -> ProcessedLabels {
    let value = &v["v"];
    match v["t"].as_str().expect("the label's type") {
        "nil" => ProcessedLabels::Plain(Labels::Nil),
        "bool" => ProcessedLabels::Plain(Labels::Bool(value.as_bool().expect("bool"))),
        "f64" => ProcessedLabels::Plain(Labels::Float64(value.as_f64().expect("float64"))),
        "string" => ProcessedLabels::Plain(plain(v)),
        "id" => ProcessedLabels::Id(Id(
            i32::try_from(value.as_i64().expect("int")).expect("int32")
        )),
        "ids" => ProcessedLabels::Ids(i32s(value).into_iter().map(Id).collect()),
        "fp16" => ProcessedLabels::Floats(
            u32s(value)
                .into_iter()
                .map(|bits| u16::try_from(bits).expect("uint16"))
                .collect(),
        ),
        "any" => ProcessedLabels::Plain(Labels::Array(
            value
                .as_array()
                .expect("a list")
                .iter()
                .map(plain)
                .collect(),
        )),
        "map" => ProcessedLabels::Object(
            value
                .as_object()
                .expect("a map")
                .iter()
                .map(|(name, entry)| (name.clone(), label(entry)))
                .collect(),
        ),
        other => panic!("unknown label type {other}"),
    }
}

/// The item a row of the case is, with the feedback it came with.
fn item(row: &Json) -> (DatasetItem, Vec<i32>) {
    let item = DatasetItem {
        item_id: row["id"].as_str().expect("id").to_owned(),
        is_hidden: row["hidden"].as_bool().expect("hidden"),
        categories: row["categories"].as_array().map(|names| {
            names
                .iter()
                .map(|name| name.as_str().expect("category").to_owned())
                .collect()
        }),
        timestamp: Time::default(),
        labels: label(&row["labels"]),
        comment: String::new(),
    };
    (item, i32s(&row["feedback"]))
}

/// The user a row of the case is, with the feedback it came with.
fn user(row: &Json) -> (DatasetUser, Vec<i32>) {
    let user = DatasetUser {
        user_id: row["id"].as_str().expect("id").to_owned(),
        labels: label(&row["labels"]),
        comment: String::new(),
    };
    (user, i32s(&row["feedback"]))
}

/// The bits of each stored vector's FP32 values, its indices and categories,
/// in the store's order: what the Go side writes for each case.
fn stored_vectors(stored: &[Vector]) -> Json {
    Json::Array(
        stored
            .iter()
            .map(|vector| {
                json!({
                    "id": vector.id,
                    "values": vector.float32_values().iter().map(|v| v.to_bits()).collect::<Vec<u32>>(),
                    "indices": vector.indices,
                    "categories": vector.categories,
                })
            })
            .collect(),
    )
}

/// Runs one case through the Rust recommender and compares it with the Go
/// output in the case. Panics on the first difference, naming the case.
async fn check_case(case: &Json) {
    let name = case["name"].as_str().expect("name");
    let kind = case["kind"].as_str().expect("kind");
    let column = case["column"].as_str().expect("column");
    let rows = case["rows"].as_array().expect("rows");
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["id"].as_str().expect("id"))
        .collect();

    let dir = tempfile::tempdir().expect("temp dir");
    let uri = format!("xvec://{}/vectors", dir.path().display());
    let db: Arc<dyn Database> =
        Arc::from(vectors::open(&uri, "", &[]).await.expect("open the store"));
    db.init().await.expect("init the store");
    let ctx = Context::background();
    let timestamp = gotime::now();

    let (actual_vectors, actual_neighbours, missing) = if let Some(kind) =
        kind.strip_prefix("item-")
    {
        let cfg = ItemToItemConfig {
            name: "oracle".to_owned(),
            kind: kind.to_owned(),
            column: column.to_owned(),
        };
        let opts = ItemToItemOptions {
            ctx: Some(ctx.clone()),
            vector_client: Some(db.clone()),
            vector_config: VectorConfig::default(),
            batch_size: 0,
            tags_idf: idf(&case["tags_idf"]),
            users_idf: idf(&case["users_idf"]),
        };
        let recommender = new_item_to_item(&cfg, timestamp, Some(&opts))
            .unwrap_or_else(|e| panic!("{name}: the recommender: {e}"));
        for row in rows {
            let (item, feedback) = item(row);
            recommender
                .add(&item, &feedback)
                .await
                .unwrap_or_else(|e| panic!("{name}: add {}: {e}", item.item_id));
        }
        recommender
            .clean()
            .await
            .unwrap_or_else(|e| panic!("{name}: clean: {e}"));
        let collection = vectors::item_to_item_collection("oracle");
        match db.get_vectors(&ctx, &collection, &ids).await {
            Ok(stored) => {
                let mut neighbours = Vec::new();
                for id in &ids {
                    let scores = query_item_to_item(&ctx, db.as_ref(), &cfg, id, &[], NEIGHBOURS)
                        .await
                        .unwrap_or_else(|e| panic!("{name}: query {id}: {e}"));
                    neighbours.push(neighbours_json(id, &scores));
                }
                (stored_vectors(&stored), neighbours, false)
            }
            Err(err) if err.kind() == Some(StorageError::NotFound) => {
                (Json::Array(Vec::new()), Vec::new(), true)
            }
            Err(err) => panic!("{name}: get the vectors: {err}"),
        }
    } else {
        let kind = kind.strip_prefix("user-").expect("item- or user- kind");
        let cfg = UserToUserConfig {
            name: "oracle".to_owned(),
            kind: kind.to_owned(),
            column: column.to_owned(),
        };
        let opts = UserToUserOptions {
            ctx: Some(ctx.clone()),
            vector_client: Some(db.clone()),
            vector_config: VectorConfig::default(),
            batch_size: 0,
            tags_idf: idf(&case["tags_idf"]),
            items_idf: idf(&case["items_idf"]),
        };
        let recommender = new_user_to_user(&cfg, timestamp, Some(&opts))
            .unwrap_or_else(|e| panic!("{name}: the recommender: {e}"));
        for row in rows {
            let (user, feedback) = user(row);
            recommender
                .add(&user, &feedback)
                .await
                .unwrap_or_else(|e| panic!("{name}: add {}: {e}", user.user_id));
        }
        recommender
            .clean()
            .await
            .unwrap_or_else(|e| panic!("{name}: clean: {e}"));
        let collection = vectors::user_to_user_collection("oracle");
        match db.get_vectors(&ctx, &collection, &ids).await {
            Ok(stored) => {
                let mut neighbours = Vec::new();
                for id in &ids {
                    let scores = query_user_to_user(&ctx, db.as_ref(), &cfg, id, NEIGHBOURS)
                        .await
                        .unwrap_or_else(|e| panic!("{name}: query {id}: {e}"));
                    neighbours.push(neighbours_json(id, &scores));
                }
                (stored_vectors(&stored), neighbours, false)
            }
            Err(err) if err.kind() == Some(StorageError::NotFound) => {
                (Json::Array(Vec::new()), Vec::new(), true)
            }
            Err(err) => panic!("{name}: get the vectors: {err}"),
        }
    };

    assert_eq!(
        missing,
        case["missing"].as_bool().unwrap_or(false),
        "{name}: whether the collection exists"
    );
    if missing {
        return;
    }
    let expected_vectors = case["vectors"].as_array().cloned().unwrap_or_default();
    let actual_vectors = actual_vectors.as_array().cloned().unwrap_or_default();
    assert_eq!(
        actual_vectors.len(),
        expected_vectors.len(),
        "{name}: the number of stored vectors"
    );
    for (i, (actual, expected)) in actual_vectors.iter().zip(&expected_vectors).enumerate() {
        assert_eq!(actual["id"], expected["id"], "{name}: vector {i} id");
        assert_eq!(
            actual["indices"],
            list(&expected["indices"]),
            "{name}: vector {i} indices"
        );
        assert_eq!(
            actual["values"],
            list(&expected["values"]),
            "{name}: vector {i} values"
        );
        assert_eq!(
            actual["categories"],
            list(&expected["categories"]),
            "{name}: vector {i} categories"
        );
    }

    let expected_neighbours = case["neighbors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        actual_neighbours.len(),
        expected_neighbours.len(),
        "{name}: the number of rows with neighbours"
    );
    for (actual, expected) in actual_neighbours.iter().zip(&expected_neighbours) {
        assert_eq!(actual["id"], expected["id"], "{name}: neighbours row");
        assert_neighbours(name, expected["id"].as_str().expect("id"), actual, expected);
    }
}

/// The relative difference a neighbour's score may have from Go's: one FP32
/// rounding step per term of a sparse dot product is far below this.
const SCORE_TOLERANCE: f64 = 1e-6;

/// The neighbours of a row, by ID, with their scores. Go's `null` is the empty
/// list.
fn neighbours_by_id(scores: &Json) -> Vec<(String, f64)> {
    list(scores)
        .as_array()
        .expect("a list of neighbours")
        .iter()
        .map(|score| {
            (
                score["id"].as_str().expect("id").to_owned(),
                f64::from_bits(score["score"].as_u64().expect("score bits")),
            )
        })
        .collect()
}

/// Asserts that the neighbours of row `id` are the expected ones. The lists have
/// the same length, and their lowest scores agree within [`SCORE_TOLERANCE`].
/// The neighbours that score above that lowest score are the same in both, with
/// their scores within tolerance. The ones tied at the lowest score may differ:
/// the store breaks ties by the smaller ID (X-75), and Go's HNSW may miss one
/// of them (O-30).
fn assert_neighbours(name: &str, id: &str, actual: &Json, expected: &Json) {
    let actual = neighbours_by_id(&actual["scores"]);
    let expected = neighbours_by_id(&expected["scores"]);
    assert_eq!(
        actual.len(),
        expected.len(),
        "{name}: the neighbours of {id}"
    );
    let lowest = |list: &[(String, f64)]| {
        list.iter()
            .map(|(_, score)| *score)
            .fold(f64::INFINITY, f64::min)
    };
    let (actual_lowest, expected_lowest) = (lowest(&actual), lowest(&expected));
    if expected.is_empty() {
        return;
    }
    assert!(
        (actual_lowest - expected_lowest).abs() <= SCORE_TOLERANCE * expected_lowest.abs().max(1.0),
        "{name}: the lowest score among the neighbours of {id}: {actual_lowest} vs Go's {expected_lowest}"
    );
    let above = |list: &[(String, f64)]| {
        list.iter()
            .filter(|(_, score)| {
                *score > expected_lowest + SCORE_TOLERANCE * expected_lowest.abs().max(1.0)
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let (actual_above, expected_above) = (above(&actual), above(&expected));
    assert_eq!(
        actual_above
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        expected_above
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        "{name}: the neighbours of {id} above the lowest score: {actual:?} vs Go's {expected:?}"
    );
    for (neighbour, score) in &expected_above {
        let (_, actual_score) = actual_above
            .iter()
            .find(|(actual_id, _)| actual_id == neighbour)
            .expect("the IDs are equal");
        assert!(
            (actual_score - score).abs() <= SCORE_TOLERANCE * score.abs().max(1.0),
            "{name}: the score of {neighbour} among the neighbours of {id}: {actual_score} vs Go's {score}"
        );
    }
}

/// The neighbours of `id` as the Go side writes them: each neighbour's ID and
/// its score as FP64 bits.
fn neighbours_json(id: &str, scores: &[Score]) -> Json {
    json!({
        "id": id,
        "scores": scores
            .iter()
            .map(|score| json!({"id": score.id, "score": score.score.to_bits()}))
            .collect::<Vec<Json>>(),
    })
}

/// The item-to-item and user-to-user recommenders produce what the Go ones do
/// on 40 generated cases: the same stored vectors bit for bit, and the same
/// neighbours with the same scores bit for bit.
#[tokio::test]
async fn recommenders_match_go() {
    let cases = go_cases();
    assert_eq!(cases.len(), 40, "cases");
    for case in &cases {
        check_case(case).await;
    }
}
