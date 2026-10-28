// Ports: logics/recommend.go @ add21b5 (WP-3-7).

//! WP-3-7, the recommendation pipeline, against the Go one.
//!
//! `contracts/tools/recommendgolden` seeds 40 cases from one seed. Each case has
//! the rows of one user's run (items, feedback, the item-to-item and user-to-user
//! vectors, the cached lists and their digests), a seeded configuration, and
//! three runs on one recommender: `Recommend`, `RecommendSequential` over a
//! seeded list of names (sometimes with an unknown one), and `Recommend` again.
//! The generator runs the Go recommender on SQLite and xvec stores and writes
//! what each run gave. This test builds the same rows in the Rust stores, runs
//! the Rust recommender through the same runs, and compares after each run: the
//! error text, the items in order, their scores within `SCORE_TOLERANCE`, their
//! categories, the digest, the exclude set, the cold-start flag and the feedback
//! order.
//!
//! Go can return a nil list where this port returns an empty `Vec` (O-76). A
//! `Vec` cannot tell the two apart, so nil-ness is not compared. The test counts
//! the runs whose Go result is nil and reports the count.
//!
//! The test fails when the Go toolchain is missing; it is never skipped
//! (`conformance/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gorse_common::expression::{ExprType, FeedbackTypeExpression};
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_config::config::schema::{
    DataSourceConfig, FallbackConfig, ItemToItemConfig, RankerConfig, RecommendConfig,
    ReplacementConfig, UserToUserConfig,
};
use gorse_logics::recommend::Recommender;
use gorse_storage::cache::{self, Score};
use gorse_storage::data;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use serde_json::Value as Json;

/// The number of cases the Go generator writes.
const EXPECTED_CASES: usize = 40;
/// The number of runs in each case.
const RUNS_PER_CASE: usize = 3;
/// A score must agree within this, relative to its size (at least 1).
const SCORE_TOLERANCE: f64 = 1e-6;

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
        .args(["run", "./contracts/tools/recommendgolden", "-out"])
        .arg(&out)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "starting go: {e}; the recommend oracle needs the Go toolchain \
                 (conformance/README.md)"
            )
        });
    assert!(
        output.status.success(),
        "go run ./contracts/tools/recommendgolden failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("the Go generator did not write {}: {e}", out.display()));
    let file: Json = serde_json::from_str(&text).expect("the Go cases are JSON");
    file["cases"].as_array().expect("the cases").clone()
}

/// A JSON string.
fn text(v: &Json) -> String {
    v.as_str().expect("a string").to_owned()
}

/// A JSON list of strings. Go writes a nil slice as `null`, the empty list.
fn texts(v: &Json) -> Vec<String> {
    v.as_array().into_iter().flatten().map(text).collect()
}

/// A JSON integer.
fn int(v: &Json) -> i64 {
    v.as_i64().expect("an integer")
}

/// Go's feedback type expressions of plain type names.
fn feedback_types(v: &Json) -> Vec<FeedbackTypeExpression> {
    texts(v)
        .into_iter()
        .map(|feedback_type| FeedbackTypeExpression {
            feedback_type,
            expr_type: ExprType::None,
            value: 0.0,
        })
        .collect()
}

/// The configuration of a case: the fields the runs read.
fn recommend_config(case: &Json) -> RecommendConfig {
    let c = &case["config"];
    let item_to_item: Vec<ItemToItemConfig> = c["item_to_item"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| ItemToItemConfig {
            name: text(&r["name"]),
            kind: text(&r["type"]),
            column: String::new(),
        })
        .collect();
    let user_to_user: Vec<UserToUserConfig> = c["user_to_user"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| UserToUserConfig {
            name: text(&r["name"]),
            kind: text(&r["type"]),
            column: String::new(),
        })
        .collect();
    RecommendConfig {
        cache_size: int(&c["cache_size"]),
        context_size: int(&c["context_size"]),
        data_source: DataSourceConfig {
            positive_feedback_types: Some(feedback_types(&c["positive"])),
            negative_feedback_types: (!c["negative"].is_null())
                .then(|| feedback_types(&c["negative"])),
            item_ttl: c["item_ttl"].as_u64().expect("the item TTL"),
            ..Default::default()
        },
        item_to_item: (!item_to_item.is_empty()).then_some(item_to_item),
        user_to_user: (!user_to_user.is_empty()).then_some(user_to_user),
        replacement: ReplacementConfig {
            enable_replacement: c["enable_replacement"].as_bool().expect("the flag"),
            ..Default::default()
        },
        ranker: RankerConfig {
            kind: text(&c["ranker_type"]),
            recommenders: Some(texts(&c["ranker_recommenders"])),
            ..Default::default()
        },
        fallback: FallbackConfig {
            recommenders: Some(texts(&c["fallback_recommenders"])),
        },
        ..Default::default()
    }
}

/// The data, cache and vector stores of one case, opened in `dir`.
struct Stores {
    data: Arc<dyn data::Database>,
    cache: Arc<dyn cache::Database>,
    vectors: Arc<dyn vectors::Database>,
}

/// Opens and initialises a fresh set of stores, as the Go generator does.
async fn open_stores(dir: &Path) -> Stores {
    let mut data_registry = data::Registry::default();
    data::sql::register(&mut data_registry);
    let data: Arc<dyn data::Database> = Arc::from(
        data_registry
            .open(&format!("sqlite://{}/data.db", dir.display()), "", &[])
            .await
            .expect("open the data store"),
    );
    let mut cache_registry = cache::Registry::default();
    cache::sql::register(&mut cache_registry);
    let cache: Arc<dyn cache::Database> = Arc::from(
        cache_registry
            .open(&format!("sqlite://{}/cache.db", dir.display()), "", &[])
            .await
            .expect("open the cache store"),
    );
    let vectors: Arc<dyn vectors::Database> = Arc::from(
        vectors::open(&format!("xvec://{}/vectors", dir.display()), "", &[])
            .await
            .expect("open the vector store"),
    );
    data.init().await.expect("init the data store");
    cache.init().await.expect("init the cache store");
    vectors.init().await.expect("init the vector store");
    Stores {
        data,
        cache,
        vectors,
    }
}

/// Writes the rows of a case into the stores: the items, the feedback, the
/// vector collections, the cached lists and the digests.
async fn write_rows(ctx: &Context, case: &Json, stores: &Stores) {
    let items: Vec<Item> = case["items"]
        .as_array()
        .expect("the items")
        .iter()
        .map(|i| Item {
            item_id: text(&i["id"]),
            is_hidden: i["hidden"].as_bool().expect("the hidden flag"),
            categories: (!i["categories"].is_null()).then(|| texts(&i["categories"])),
            timestamp: gotime::unix(int(&i["timestamp"]), 0),
            ..Default::default()
        })
        .collect();
    stores
        .data
        .batch_insert_items(ctx, &items)
        .await
        .expect("insert the items");

    let feedback: Vec<Feedback> = case["feedback"]
        .as_array()
        .expect("the feedback")
        .iter()
        .map(|f| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: text(&f["type"]),
                user_id: text(&f["user"]),
                item_id: text(&f["item"]),
            },
            value: f["value"].as_f64().expect("the value"),
            timestamp: gotime::unix(int(&f["timestamp"]), 0),
            ..Default::default()
        })
        .collect();
    stores
        .data
        .batch_insert_feedback(ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    for collection in case["vectors"].as_array().expect("the vectors") {
        let name = text(&collection["name"]);
        let distance = if collection["distance"] == "euclidean" {
            Distance::Euclidean
        } else {
            Distance::Dot
        };
        let dim = int(&collection["dim"]);
        stores
            .vectors
            .add_collection(ctx, &name, dim, distance, VectorConfig::default())
            .await
            .expect("add the collection");
        let rows: Vec<Vector> = collection["vectors"]
            .as_array()
            .expect("the rows")
            .iter()
            .map(|row| Vector {
                id: text(&row["id"]),
                values: row["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_f64().expect("a value") as f32)
                    .collect(),
                indices: row["indices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_u64().expect("an index") as u32)
                    .collect(),
                categories: texts(&row["categories"]),
                is_hidden: row["hidden"].as_bool().expect("the hidden flag"),
                ..Default::default()
            })
            .collect();
        stores
            .vectors
            .add_vectors(ctx, &name, &rows)
            .await
            .expect("add the vectors");
    }

    for list in case["scores"].as_array().expect("the score lists") {
        let scores: Vec<Score> = list["items"]
            .as_array()
            .expect("the scores")
            .iter()
            .map(|s| Score {
                id: text(&s["id"]),
                score: s["score"].as_f64().expect("a score"),
                is_hidden: false,
                categories: texts(&s["categories"]),
                timestamp: Default::default(),
            })
            .collect();
        if scores.is_empty() {
            continue;
        }
        stores
            .cache
            .add_scores(
                ctx,
                &text(&list["collection"]),
                &text(&list["subset"]),
                &scores,
            )
            .await
            .expect("add the scores");
    }
    for digest in case["digests"].as_array().expect("the digests") {
        let parts = texts(&digest["parts"]);
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        stores
            .cache
            .set(
                ctx,
                &[cache::string(cache::key(&parts), text(&digest["value"]))],
            )
            .await
            .expect("set the digest");
    }
}

/// What one run gave, in the shape the Go generator writes.
#[derive(Debug)]
struct Outcome {
    /// The error text, empty when the run succeeded.
    error: String,
    /// The recommended items, in order: the ID, the score and the categories.
    results: Vec<(String, f64, Vec<String>)>,
    /// The digest of the run; empty for `Recommend`.
    digest: String,
    /// The exclude set after the run, sorted.
    exclude: Vec<String>,
    /// Whether the user is cold-started after the run.
    cold_start: bool,
    /// The feedback item IDs after the run, in the order the recommender keeps.
    feedback_order: Vec<String>,
}

/// Builds the case's recommender on fresh stores, runs it through the case's
/// runs, and returns what each run gave.
async fn replay(case: &Json) -> Vec<Outcome> {
    let dir = tempfile::tempdir().expect("temp dir");
    let ctx = Context::background();
    let stores = open_stores(dir.path()).await;
    write_rows(&ctx, case, &stores).await;
    let mut recommender = Recommender::new(
        recommend_config(case),
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        case["online"].as_bool().expect("the online flag"),
        text(&case["user_id"]),
        texts(&case["categories"]),
    )
    .await
    .expect("build the recommender");

    let mut outcomes = Vec::new();
    for run in case["runs"].as_array().expect("the runs") {
        let limit = int(&run["limit"]);
        let outcome = match run["op"].as_str() {
            Some("recommend") => recommender
                .recommend(&ctx, limit)
                .await
                .map(|scores| (scores, String::new())),
            Some("sequential") => {
                recommender
                    .recommend_sequential(&ctx, Vec::new(), limit, &texts(&run["names"]))
                    .await
            }
            other => panic!("an unknown run op: {other:?}"),
        };
        let (error, results, digest) = match outcome {
            Ok((scores, digest)) => (
                String::new(),
                scores
                    .into_iter()
                    .map(|s| (s.id, s.score, s.categories))
                    .collect(),
                digest,
            ),
            Err(e) => (e.to_string(), Vec::new(), String::new()),
        };
        let mut exclude: Vec<String> = recommender.exclude_set().iter().cloned().collect();
        exclude.sort();
        outcomes.push(Outcome {
            error,
            results,
            digest,
            exclude,
            cold_start: recommender.is_cold_start(),
            feedback_order: recommender
                .user_feedback()
                .iter()
                .map(|f| f.feedback_key.item_id.clone())
                .collect(),
        });
    }
    outcomes
}

/// Whether two scores agree within the tolerance, relative to their size.
fn same_score(a: f64, b: f64) -> bool {
    (a - b).abs() <= SCORE_TOLERANCE * a.abs().max(b.abs()).max(1.0)
}

/// Records each way the Rust run differs from the Go run.
fn compare(
    case: &str,
    run: usize,
    go: &Json,
    rust: &Outcome,
    differences: &mut Vec<String>,
    largest_gap: &mut f64,
) {
    let mut diff = |what: &str, detail: String| {
        differences.push(format!("{case} run {run}: {what}: {detail}"));
    };
    let go_error = text(&go["error"]);
    if go_error != rust.error {
        diff("error", format!("go {go_error:?}, rust {:?}", rust.error));
    }
    let go_results = go["results"].as_array().expect("the results");
    if go_results.len() != rust.results.len() {
        diff(
            "result count",
            format!("go {}, rust {}", go_results.len(), rust.results.len()),
        );
    } else {
        for (i, (g, r)) in go_results.iter().zip(&rust.results).enumerate() {
            let go_score = g["score"].as_f64().expect("a score");
            if go_score.is_finite() && r.1.is_finite() {
                *largest_gap = largest_gap.max((go_score - r.1).abs());
            }
            if text(&g["id"]) != r.0 {
                diff(
                    "item",
                    format!("at {i}: go {:?}, rust {:?}", text(&g["id"]), r.0),
                );
            } else if !same_score(go_score, r.1) {
                diff(
                    "score",
                    format!("{} at {i}: go {go_score}, rust {}", r.0, r.1),
                );
            }
            if texts(&g["categories"]) != r.2 {
                diff(
                    "categories",
                    format!(
                        "{} at {i}: go {:?}, rust {:?}",
                        r.0,
                        texts(&g["categories"]),
                        r.2
                    ),
                );
            }
        }
    }
    if text(&go["digest"]) != rust.digest {
        diff(
            "digest",
            format!("go {:?}, rust {:?}", text(&go["digest"]), rust.digest),
        );
    }
    if texts(&go["exclude"]) != rust.exclude {
        diff(
            "exclude set",
            format!("go {:?}, rust {:?}", texts(&go["exclude"]), rust.exclude),
        );
    }
    if go["cold_start"].as_bool() != Some(rust.cold_start) {
        diff(
            "cold start",
            format!("go {}, rust {}", go["cold_start"], rust.cold_start),
        );
    }
    if texts(&go["feedback_order"]) != rust.feedback_order {
        diff(
            "feedback order",
            format!(
                "go {:?}, rust {:?}",
                texts(&go["feedback_order"]),
                rust.feedback_order
            ),
        );
    }
}

#[tokio::test]
async fn recommend_matches_go() {
    let cases = go_cases();
    assert_eq!(
        cases.len(),
        EXPECTED_CASES,
        "the Go generator writes {EXPECTED_CASES} cases"
    );
    let mut differences = Vec::new();
    let mut compared = 0;
    let mut nil_runs = 0;
    let mut largest_gap = 0.0_f64;
    for case in &cases {
        let name = text(&case["name"]);
        let expected = case["expected"].as_array().expect("the expected outcomes");
        let outcomes = replay(case).await;
        assert_eq!(
            outcomes.len(),
            expected.len(),
            "{name}: the run count differs"
        );
        for (i, (go, rust)) in expected.iter().zip(&outcomes).enumerate() {
            compared += 1;
            if go["nil"].as_bool() == Some(true) {
                nil_runs += 1;
            }
            compare(&name, i, go, rust, &mut differences, &mut largest_gap);
        }
    }
    assert_eq!(compared, EXPECTED_CASES * RUNS_PER_CASE);
    eprintln!(
        "recommend oracle: {compared} runs compared, largest score difference {largest_gap:e}, \
         {nil_runs} of them give a nil result in Go (O-76)"
    );
    assert!(
        differences.is_empty(),
        "{} differences from Go:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
