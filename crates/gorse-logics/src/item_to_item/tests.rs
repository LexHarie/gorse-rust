// Ports: logics/item_to_item_test.go @ add21b5

//! The labels of these items are the processed values the dataset gives them
//! (`dataset.AddItem` runs `processLabels`): a `[]float32` is FP16 bits, a
//! string or `[]string` is a label ID. The Go tests pass the raw values, and
//! the expression sees the same bits either way (`floats.FromAny`).

use std::sync::Arc;

use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, HOUR, MILLISECOND, Time};
use gorse_config::config::schema::ItemToItemConfig;
use gorse_dataset::dataset::{DatasetItem, Id, Value as ProcessedLabels};
use gorse_storage::data::types::Labels;
use gorse_storage::vectors::{self, Database, Distance, Vector, VectorConfig};

use super::{ItemToItem, ItemToItemOptions, new_item_to_item, query_item_to_item};

/// Go's `SetupTest`: a new xvec store in a temporary directory.
async fn open_store() -> (tempfile::TempDir, Arc<dyn Database>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let uri = format!("xvec://{}/vectors", dir.path().display());
    let db: Arc<dyn Database> =
        Arc::from(vectors::open(&uri, "", &[]).await.expect("open the store"));
    db.init().await.expect("init the store");
    (dir, db)
}

/// The options of a recommender that writes to `db`, with the default batch
/// size and no IDF.
fn options(db: &Arc<dyn Database>) -> ItemToItemOptions {
    ItemToItemOptions {
        ctx: Some(Context::background()),
        vector_client: Some(db.clone()),
        vector_config: VectorConfig::default(),
        batch_size: 0,
        tags_idf: None,
        users_idf: None,
    }
}

fn config(name: &str, kind: &str, column: &str) -> ItemToItemConfig {
    ItemToItemConfig {
        name: name.to_owned(),
        kind: kind.to_owned(),
        column: column.to_owned(),
    }
}

/// An item with no labels.
fn item(item_id: &str, labels: ProcessedLabels) -> DatasetItem {
    DatasetItem {
        item_id: item_id.to_owned(),
        is_hidden: false,
        categories: None,
        timestamp: Time::default(),
        labels,
        comment: String::new(),
    }
}

/// Go's `map[string]any` of processed labels.
fn object(entries: Vec<(&str, ProcessedLabels)>) -> ProcessedLabels {
    ProcessedLabels::Object(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

/// Go's `[]float32`, as the dataset keeps it: FP16 bits.
fn fp16(values: &[f32]) -> ProcessedLabels {
    ProcessedLabels::Floats(floats::from_float32(values))
}

/// Go's `[]dataset.ID{dataset.ID(id)}`.
fn ids(id: i32) -> ProcessedLabels {
    ProcessedLabels::Ids(vec![Id(id)])
}

/// Go's `[]float32{1, 1, ..., 1}` of `n` values.
fn ones(n: usize) -> Vec<f32> {
    vec![1.0; n]
}

/// The recommenders are `Send` and `Sync`, so the master can share them between
/// its workers (Q-254, D-232). This is checked when the test compiles.
#[test]
fn recommender_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ItemToItem>();
}

#[tokio::test]
async fn column_func() {
    let (_dir, db) = open_store().await;
    let ctx = Context::background();
    let cfg = config("column", "embedding", "item.Labels.description");
    let collection = vectors::item_to_item_collection(&cfg.name);
    let item2item =
        new_item_to_item(&cfg, gotime::now(), Some(&options(&db))).expect("the recommender");

    // Add success
    item2item
        .add(
            &item("1", object(vec![("description", fp16(&[0.1, 0.2, 0.3]))])),
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");
    assert_eq!(db.count_vectors(&ctx, &collection).await.expect("count"), 1);

    // Hidden
    item2item
        .add(
            &DatasetItem {
                is_hidden: true,
                ..item("2", object(vec![("description", fp16(&[0.1, 0.2, 0.3]))]))
            },
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");
    assert_eq!(db.count_vectors(&ctx, &collection).await.expect("count"), 2);

    // Dimension does not match
    item2item
        .add(
            &item("1", object(vec![("description", fp16(&[0.1, 0.2]))])),
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");
    assert_eq!(db.count_vectors(&ctx, &collection).await.expect("count"), 2);

    // Type does not match: a string is a label ID in the processed labels, and
    // Go's raw string is kept as it is here, which the expression sees the same.
    item2item
        .add(
            &item(
                "1",
                object(vec![(
                    "description",
                    ProcessedLabels::Plain(Labels::String("hello".to_owned())),
                )]),
            ),
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");
    assert_eq!(db.count_vectors(&ctx, &collection).await.expect("count"), 2);

    // Column does not exist: the labels are a list, not a map.
    item2item
        .add(&item("2", fp16(&[0.1, 0.2, 0.3])), &[])
        .await
        .expect("add");
    item2item.clean().await.expect("clean");
    assert_eq!(db.count_vectors(&ctx, &collection).await.expect("count"), 2);
}

#[tokio::test]
async fn embedding() {
    let (_dir, db) = open_store().await;
    let cfg = config("embedding", "embedding", "item.Labels.description");
    let item2item =
        new_item_to_item(&cfg, gotime::now(), Some(&options(&db))).expect("the recommender");

    for i in 0..100 {
        let i = i as f32;
        item2item
            .add(
                &item(
                    &(i as i32).to_string(),
                    object(vec![("description", fp16(&[0.1 * i, 0.2 * i, 0.3 * i]))]),
                ),
                &[],
            )
            .await
            .expect("add");
    }
    item2item.clean().await.expect("clean");

    let scores = query_item_to_item(&Context::background(), db.as_ref(), &cfg, "0", &[], 10)
        .await
        .expect("query");
    assert_eq!(scores.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores[i - 1].id, i.to_string());
    }
}

#[tokio::test]
async fn clean() {
    let (_dir, db) = open_store().await;
    let ctx = Context::background();
    let timestamp = gotime::now().utc().truncate(MILLISECOND);
    let cfg = config("cleanup", "embedding", "item.Labels.embedding");
    let collection = vectors::item_to_item_collection(&cfg.name);
    db.add_collection(
        &ctx,
        &collection,
        2,
        Distance::Euclidean,
        VectorConfig::default(),
    )
    .await
    .expect("add the collection");
    db.add_vectors(
        &ctx,
        &collection,
        &[
            Vector {
                id: "stale".to_owned(),
                values: vec![0.0, 0.0],
                timestamp: timestamp.add(Duration(-HOUR.0)),
                ..Vector::default()
            },
            Vector {
                id: "current".to_owned(),
                values: vec![1.0, 0.0],
                timestamp: timestamp.clone(),
                ..Vector::default()
            },
        ],
    )
    .await
    .expect("add the vectors");

    let item2item =
        new_item_to_item(&cfg, timestamp.clone(), Some(&options(&db))).expect("the recommender");
    item2item
        .add(
            &item("new", object(vec![("embedding", fp16(&[2.0, 0.0]))])),
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");

    let stored = db
        .get_vectors(&ctx, &collection, &["stale", "current", "new"])
        .await
        .expect("get the vectors");
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].id, "current");
    assert_eq!(stored[1].id, "new");
}

#[tokio::test]
async fn hidden() {
    let (_dir, db) = open_store().await;
    let cfg = config("hidden", "embedding", "item.Labels.description");
    let item2item =
        new_item_to_item(&cfg, gotime::now(), Some(&options(&db))).expect("the recommender");

    item2item
        .add(
            &item(
                "visible_1",
                object(vec![("description", fp16(&[0.0, 0.0, 0.0]))]),
            ),
            &[],
        )
        .await
        .expect("add");
    item2item
        .add(
            &item(
                "visible_2",
                object(vec![("description", fp16(&[0.1, 0.0, 0.0]))]),
            ),
            &[],
        )
        .await
        .expect("add");
    item2item
        .add(
            &DatasetItem {
                is_hidden: true,
                ..item(
                    "hidden_1",
                    object(vec![("description", fp16(&[0.05, 0.0, 0.0]))]),
                )
            },
            &[],
        )
        .await
        .expect("add");
    item2item.clean().await.expect("clean");

    // hidden item should have similar items generated from non-hidden index
    let hidden_scores = query_item_to_item(
        &Context::background(),
        db.as_ref(),
        &cfg,
        "hidden_1",
        &[],
        2,
    )
    .await
    .expect("query");
    assert_eq!(hidden_scores.len(), 2);
    for score in &hidden_scores {
        assert_ne!(score.id, "hidden_1");
    }

    // non-hidden item should never get hidden item in similarity results
    let visible_scores = query_item_to_item(
        &Context::background(),
        db.as_ref(),
        &cfg,
        "visible_1",
        &[],
        2,
    )
    .await
    .expect("query");
    assert_eq!(visible_scores.len(), 1);
    for score in &visible_scores {
        assert_ne!(score.id, "hidden_1");
    }
}

#[tokio::test]
async fn tags() {
    let (_dir, db) = open_store().await;
    let cfg = config("tags", "tags", "item.Labels");
    let mut opts = options(&db);
    opts.tags_idf = Some(ones(101));
    let item2item = new_item_to_item(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

    for i in 0..100usize {
        let labels: Vec<(String, ProcessedLabels)> = (1..=100 - i)
            .map(|j| (j.to_string(), ids(j as i32)))
            .collect();
        item2item
            .add(
                &item(
                    &i.to_string(),
                    ProcessedLabels::Object(labels.into_iter().collect()),
                ),
                &[],
            )
            .await
            .expect("add");
    }
    item2item.clean().await.expect("clean");

    let scores = query_item_to_item(&Context::background(), db.as_ref(), &cfg, "0", &[], 10)
        .await
        .expect("query");
    assert_eq!(scores.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores[i - 1].id, i.to_string());
    }
}

#[tokio::test]
async fn users() {
    let (_dir, db) = open_store().await;
    let cfg = config("users", "users", "");
    let mut opts = options(&db);
    opts.users_idf = Some(ones(101));
    let item2item = new_item_to_item(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

    for i in 0..100usize {
        let feedback: Vec<i32> = (1..=100 - i as i32).collect();
        item2item
            .add(
                &item(&i.to_string(), ProcessedLabels::Plain(Labels::Nil)),
                &feedback,
            )
            .await
            .expect("add");
    }
    item2item.clean().await.expect("clean");

    let scores = query_item_to_item(&Context::background(), db.as_ref(), &cfg, "0", &[], 10)
        .await
        .expect("query");
    assert_eq!(scores.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores[i - 1].id, i.to_string());
    }
}

#[tokio::test]
async fn auto() {
    let (_dir, db) = open_store().await;
    let cfg = config("auto", "auto", "");
    let mut opts = options(&db);
    opts.tags_idf = Some(ones(101));
    opts.users_idf = Some(ones(101));
    let item2item = new_item_to_item(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

    for i in 0..100usize {
        let mut feedback: Vec<i32> = Vec::new();
        let mut labels = ProcessedLabels::Plain(Labels::Nil);
        if i % 2 == 0 {
            let entries: Vec<(String, ProcessedLabels)> = (1..=100 - i)
                .map(|j| (j.to_string(), ids(j as i32)))
                .collect();
            labels = ProcessedLabels::Object(entries.into_iter().collect());
        } else {
            feedback.extend(1..=(100 - i) as i32);
        }
        item2item
            .add(&item(&i.to_string(), labels), &feedback)
            .await
            .expect("add");
    }
    item2item.clean().await.expect("clean");

    let scores0 = query_item_to_item(&Context::background(), db.as_ref(), &cfg, "0", &[], 10)
        .await
        .expect("query");
    assert_eq!(scores0.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores0[i - 1].id, (i * 2).to_string());
    }

    let scores1 = query_item_to_item(&Context::background(), db.as_ref(), &cfg, "1", &[], 10)
        .await
        .expect("query");
    assert_eq!(scores1.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores1[i - 1].id, (i * 2 + 1).to_string());
    }
}
