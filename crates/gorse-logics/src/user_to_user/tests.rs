// Ports: logics/user_to_user_test.go @ add21b5

//! As in the item-to-item tests, the labels are the processed values the
//! dataset gives them: FP16 bits for a numeric list, and label IDs for strings.

use std::sync::Arc;

use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, HOUR, MILLISECOND};
use gorse_config::config::schema::UserToUserConfig;
use gorse_dataset::dataset::{DatasetUser, Id, Value as ProcessedLabels};
use gorse_storage::data::types::Labels;
use gorse_storage::vectors::{self, Database, Distance, Vector, VectorConfig};

use super::{UserToUser, UserToUserOptions, new_user_to_user, query_user_to_user};

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
fn options(db: &Arc<dyn Database>) -> UserToUserOptions {
    UserToUserOptions {
        ctx: Some(Context::background()),
        vector_client: Some(db.clone()),
        vector_config: VectorConfig::default(),
        batch_size: 0,
        tags_idf: None,
        items_idf: None,
    }
}

fn config(name: &str, kind: &str, column: &str) -> UserToUserConfig {
    UserToUserConfig {
        name: name.to_owned(),
        kind: kind.to_owned(),
        column: column.to_owned(),
    }
}

/// A user with no labels.
fn user(user_id: &str, labels: ProcessedLabels) -> DatasetUser {
    DatasetUser {
        user_id: user_id.to_owned(),
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
    assert_send_sync::<UserToUser>();
}

#[tokio::test]
async fn embedding() {
    let (_dir, db) = open_store().await;
    let cfg = config("embedding", "embedding", "user.Labels.description");
    let user2user =
        new_user_to_user(&cfg, gotime::now(), Some(&options(&db))).expect("the recommender");

    for i in 0..100 {
        let i = i as f32;
        user2user
            .add(
                &user(
                    &(i as i32).to_string(),
                    object(vec![("description", fp16(&[0.1 * i, 0.2 * i, 0.3 * i]))]),
                ),
                &[],
            )
            .await
            .expect("add");
    }

    user2user.clean().await.expect("clean");
    let scores = query_user_to_user(&Context::background(), db.as_ref(), &cfg, "0", 10)
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
    let cfg = config("cleanup", "embedding", "user.Labels.embedding");
    let collection = vectors::user_to_user_collection(&cfg.name);
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

    let writer =
        new_user_to_user(&cfg, timestamp.clone(), Some(&options(&db))).expect("the recommender");
    writer
        .add(
            &user("new", object(vec![("embedding", fp16(&[2.0, 0.0]))])),
            &[],
        )
        .await
        .expect("add");
    writer.clean().await.expect("clean");

    let stored = db
        .get_vectors(&ctx, &collection, &["stale", "current", "new"])
        .await
        .expect("get the vectors");
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].id, "current");
    assert_eq!(stored[1].id, "new");
}

#[tokio::test]
async fn tags() {
    let (_dir, db) = open_store().await;
    let cfg = config("tags", "tags", "user.Labels");
    let mut opts = options(&db);
    opts.tags_idf = Some(ones(101));
    let user2user = new_user_to_user(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

    for i in 0..100usize {
        let labels: Vec<(String, ProcessedLabels)> = (1..=100 - i)
            .map(|j| (j.to_string(), ids(j as i32)))
            .collect();
        user2user
            .add(
                &user(
                    &i.to_string(),
                    ProcessedLabels::Object(labels.into_iter().collect()),
                ),
                &[],
            )
            .await
            .expect("add");
    }

    user2user.clean().await.expect("clean");
    let scores = query_user_to_user(&Context::background(), db.as_ref(), &cfg, "0", 10)
        .await
        .expect("query");
    assert_eq!(scores.len(), 10);
    for i in 1..=10 {
        assert_eq!(scores[i - 1].id, i.to_string());
    }
}

#[tokio::test]
async fn items() {
    let (_dir, db) = open_store().await;
    let cfg = config("items", "items", "");
    let mut opts = options(&db);
    opts.items_idf = Some(ones(101));
    let user2user = new_user_to_user(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

    for i in 0..100usize {
        let feedback: Vec<i32> = (1..=100 - i as i32).collect();
        user2user
            .add(
                &user(&i.to_string(), ProcessedLabels::Plain(Labels::Nil)),
                &feedback,
            )
            .await
            .expect("add");
    }

    user2user.clean().await.expect("clean");
    let scores = query_user_to_user(&Context::background(), db.as_ref(), &cfg, "0", 10)
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
    opts.items_idf = Some(ones(101));
    let user2user = new_user_to_user(&cfg, gotime::now(), Some(&opts)).expect("the recommender");

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
        user2user
            .add(&user(&i.to_string(), labels), &feedback)
            .await
            .expect("add");
    }

    user2user.clean().await.expect("clean");
    let scores0 = query_user_to_user(&Context::background(), db.as_ref(), &cfg, "0", 10)
        .await
        .expect("query");
    assert_eq!(scores0.len(), 10);
    let scores1 = query_user_to_user(&Context::background(), db.as_ref(), &cfg, "1", 10)
        .await
        .expect("query");
    assert_eq!(scores1.len(), 10);
}
