// Ports: master/tasks_test.go @ add21b5 (the ten suite methods), and the
// O-75 and X-199 pins the WP-4-3 brief asks for.

//! The tests of the master's tasks: the dataset load, the recommenders the load
//! updates, the snapshot it emits, the garbage collection and the removal of
//! out-of-date models. Go's suite runs its methods one after another, and the
//! event handler is process-wide, so every test here takes [`SERIAL`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use async_trait::async_trait;
use gorse_common::event;
use gorse_common::expression::{FeedbackTypeExpression, must_parse_feedback_type_expression};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, HOUR, Month, Time};
use gorse_config::config::get_default_config;
use gorse_config::config::schema::{
    Config, ItemToItemConfig, NonPersonalizedConfig, UserToUserConfig,
};
use gorse_dataset::dataset::{Dataset, DatasetItem, Value};
use gorse_logics::item_to_item::{ItemToItemOptions, new_item_to_item, query_item_to_item};
use gorse_logics::recommend::Recommender;
use gorse_logics::user_to_user::query_user_to_user;
use gorse_storage::blob::{self, BlobError, BlobWriter};
use gorse_storage::cache::{self, Score};
use gorse_storage_core::errors::Error as StorageError;

use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels, User};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use tempfile::TempDir;
use tokio::io::AsyncRead;
use tokio::sync::oneshot;

use super::load::Sources;
use super::{BATCH_SIZE, deep_size_of, item_size, user_size};
use crate::master::Master;
use crate::metrics;
use crate::test_support::{close_stores, new_master, open_stores};

/// Held by every test in this module, so that no test sees another test's
/// events. The event handler is process-wide.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A master over sqlite stores in a temporary directory, with `cfg` as its
/// configuration. Go's `SetupTest`.
async fn setup(cfg: Config) -> (TempDir, Arc<Master>) {
    let dir = TempDir::new().expect("a temporary directory");
    let master = new_master(dir.path(), cfg, "").await;
    open_stores(&master, dir.path()).await;
    (dir, master)
}

/// Replaces the master's configuration with the one `change` makes of it, as
/// Go's tests assign `s.Config`.
fn set_config(master: &Master, change: impl FnOnce(&mut Config)) {
    let mut cfg = (*master.config()).clone();
    change(&mut cfg);
    *master
        .config
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Arc::new(cfg);
}

fn expression(text: &str) -> FeedbackTypeExpression {
    must_parse_feedback_type_expression(text)
}

/// Labels of the given strings, as `[]string` or `[]any` of strings marshal.
fn strings(values: &[&str]) -> Labels {
    Labels::Array(
        values
            .iter()
            .map(|value| Labels::String((*value).to_owned()))
            .collect(),
    )
}

fn feedback(kind: &str, user: &str, item: &str, timestamp: Time) -> Feedback {
    Feedback {
        feedback_key: FeedbackKey {
            feedback_type: kind.to_owned(),
            user_id: user.to_owned(),
            item_id: item.to_owned(),
        },
        timestamp,
        ..Default::default()
    }
}

fn ids(documents: &[Score]) -> Vec<String> {
    cache::convert_documents_to_values(documents)
}

/// Go's `s.LoadDataFromDatabase` with the given positive feedback types and no
/// negative or read ones. It returns the ranking dataset.
async fn load_positive(master: &Master, positive: &str) -> Dataset {
    let cfg = master.config();
    let data = master.data();
    let positive = [expression(positive)];
    let sources = Sources {
        positive: &positive,
        negative: &[],
        read: &[],
        item_ttl: 0,
        positive_feedback_ttl: 0,
    };
    let evaluator = Mutex::new(metrics::new_online_evaluator(Vec::new(), Vec::new()));
    let (_, dataset, _) = master
        .load_data_from_database(
            &Context::background(),
            &*data,
            &cfg,
            &sources,
            &evaluator,
            &[],
        )
        .await
        .expect("load the dataset");
    dataset
}

/// Go's `FailOnceBlobStore`: its `remove` of `name` fails the first time.
struct FailOnceBlobStore {
    inner: blob::posix::Posix,
    name: String,
    failed: AtomicBool,
}

#[async_trait]
impl blob::Store for FailOnceBlobStore {
    async fn open(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>, BlobError> {
        self.inner.open(ctx, name).await
    }

    async fn create(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Box<dyn BlobWriter>, oneshot::Receiver<()>), BlobError> {
        self.inner.create(ctx, name).await
    }

    async fn list(&self, ctx: &Context) -> Result<Vec<String>, BlobError> {
        self.inner.list(ctx).await
    }

    async fn remove(&self, ctx: &Context, name: &str) -> Result<(), BlobError> {
        if name == self.name && !self.failed.swap(true, Ordering::SeqCst) {
            return Err(BlobError::Config(format!("failed to remove {name}")));
        }
        self.inner.remove(ctx, name).await
    }
}

/// Go's `failOnceVectorDatabase`: its `DeleteCollection` of `name` fails the
/// first time. Every other call goes to the database it wraps.
struct FailOnceVectorDatabase {
    inner: Arc<dyn vectors::Database>,
    name: String,
    failed: AtomicBool,
}

#[async_trait]
impl vectors::Database for FailOnceVectorDatabase {
    async fn init(&self) -> Result<(), vectors::Error> {
        self.inner.init().await
    }

    async fn optimize(&self, ctx: &Context, name: &str) -> Result<(), vectors::Error> {
        self.inner.optimize(ctx, name).await
    }

    async fn close(&self) -> Result<(), vectors::Error> {
        self.inner.close().await
    }

    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, vectors::Error> {
        self.inner.list_collections(ctx).await
    }

    async fn describe_collection(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<vectors::CollectionInfo, vectors::Error> {
        self.inner.describe_collection(ctx, name).await
    }

    async fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), vectors::Error> {
        self.inner
            .add_collection(ctx, name, dimensions, distance, config)
            .await
    }

    async fn delete_collection(&self, ctx: &Context, name: &str) -> Result<(), vectors::Error> {
        if name == self.name && !self.failed.swap(true, Ordering::SeqCst) {
            return Err(vectors::Error::Other(format!("failed to delete {name}")));
        }
        self.inner.delete_collection(ctx, name).await
    }

    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, vectors::Error> {
        self.inner.count_vectors(ctx, collection).await
    }

    async fn add_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        vectors: &[Vector],
    ) -> Result<(), vectors::Error> {
        self.inner.add_vectors(ctx, collection, vectors).await
    }

    async fn get_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<Vector>, vectors::Error> {
        self.inner.get_vectors(ctx, collection, ids).await
    }

    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), vectors::Error> {
        self.inner.delete_vectors(ctx, collection, timestamp).await
    }

    async fn query_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        q: &Vector,
        categories: &[&str],
        top_k: i64,
    ) -> Result<Vec<vectors::ScoredVector>, vectors::Error> {
        self.inner
            .query_vectors(ctx, collection, q, categories, top_k)
            .await
    }
}

/// Writes a blob with the content `model`, as Go's tests do.
async fn put_blob(store: &dyn blob::Store, ctx: &Context, name: &str) {
    let (mut writer, done) = store.create(ctx, name).await.expect("create the blob");
    writer.write(ctx, b"model").await.expect("write the blob");
    writer.close(ctx).await.expect("close the blob");
    // The receiver fails once the blob is finished, as Go's closed channel reads.
    let _ = done.await;
}

/// Go's `TestRemoveOutOfDateModels`. The first removal of blob `50` fails, so
/// it is left for the second run.
#[tokio::test]
async fn test_remove_out_of_date_models() {
    let _serial = SERIAL.lock().await;
    let (dir, master) = setup(get_default_config()).await;
    let ctx = Context::background();
    *master.blob.write().unwrap_or_else(PoisonError::into_inner) =
        Some(Arc::new(FailOnceBlobStore {
            inner: blob::posix::Posix::new(dir.path().join("models")),
            name: "50".to_owned(),
            failed: AtomicBool::new(false),
        }));
    master
        .collaborative_filtering
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 100;
    master
        .click_through_rate
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 150;
    let store = master.blob_store();
    for id in [50, 100, 150, 200, 300, 400] {
        put_blob(store.as_ref(), &ctx, &id.to_string()).await;
    }
    put_blob(store.as_ref(), &ctx, "invalid").await;
    let vector_store = master.vectors();
    for id in [100, 200, 300, 400, 500] {
        vector_store
            .add_collection(
                &ctx,
                &vectors::collaborative_filtering_collection(id),
                2,
                Distance::Dot,
                VectorConfig::default(),
            )
            .await
            .expect("add the collection");
    }
    for name in ["collaborative_filtering_invalid", "unrelated"] {
        vector_store
            .add_collection(&ctx, name, 2, Distance::Dot, VectorConfig::default())
            .await
            .expect("add the collection");
    }

    master.remove_out_of_date_models(&ctx).await;
    master.remove_out_of_date_models(&ctx).await;

    let mut collections = master
        .vectors()
        .list_collections(&ctx)
        .await
        .expect("list the collections");
    collections.sort();
    let mut want = vec![
        vectors::collaborative_filtering_collection(100),
        vectors::collaborative_filtering_collection(300),
        vectors::collaborative_filtering_collection(400),
        "collaborative_filtering_invalid".to_owned(),
        "unrelated".to_owned(),
    ];
    want.sort();
    assert_eq!(collections, want);
    let mut files = master
        .blob_store()
        .list(&ctx)
        .await
        .expect("list the blobs");
    files.sort();
    assert_eq!(files, ["100", "150", "300", "400", "invalid"]);
    close_stores(&master).await;
}

/// Go's `TestRemoveOutOfDateModelsRetry`. Both the blob and the collection of
/// model `200` fail once, so three runs are needed to remove it.
#[tokio::test]
async fn test_remove_out_of_date_models_retry() {
    let _serial = SERIAL.lock().await;
    let (dir, master) = setup(get_default_config()).await;
    let ctx = Context::background();
    *master.blob.write().unwrap_or_else(PoisonError::into_inner) =
        Some(Arc::new(FailOnceBlobStore {
            inner: blob::posix::Posix::new(dir.path().join("models")),
            name: "200".to_owned(),
            failed: AtomicBool::new(false),
        }));
    let inner = master.vectors();
    *master
        .vectors
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Arc::new(FailOnceVectorDatabase {
        inner,
        name: vectors::collaborative_filtering_collection(200),
        failed: AtomicBool::new(false),
    });
    master
        .collaborative_filtering
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 100;
    master
        .click_through_rate
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 150;
    let store = master.blob_store();
    for id in [100, 150, 200, 300, 400] {
        put_blob(store.as_ref(), &ctx, &id.to_string()).await;
    }
    let vector_store = master.vectors();
    for id in [100, 200, 300, 400] {
        vector_store
            .add_collection(
                &ctx,
                &vectors::collaborative_filtering_collection(id),
                2,
                Distance::Dot,
                VectorConfig::default(),
            )
            .await
            .expect("add the collection");
    }

    master.remove_out_of_date_models(&ctx).await;
    master.remove_out_of_date_models(&ctx).await;
    master.remove_out_of_date_models(&ctx).await;

    let collections = master
        .vectors()
        .list_collections(&ctx)
        .await
        .expect("list the collections");
    assert!(!collections.contains(&vectors::collaborative_filtering_collection(200)));
    let files = master
        .blob_store()
        .list(&ctx)
        .await
        .expect("list the blobs");
    assert!(!files.contains(&"200".to_owned()));
    close_stores(&master).await;
}

/// Go's `TestFindItemToItem`: the item-to-item recommenders over common users
/// and common labels, and the automatic one, which uses both.
#[tokio::test]
async fn test_find_item_to_item() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.master.n_jobs = 4;
    });
    let now = gotime::now();
    let items = [
        ("0", vec!["*"], strings(&["a", "b", "c", "d"])),
        ("1", vec!["*"], strings(&[])),
        ("2", vec!["*"], strings(&["b", "c", "d"])),
        ("3", vec![], strings(&[])),
        ("4", vec![], strings(&["b", "c"])),
        ("5", vec!["*"], strings(&[])),
        ("6", vec!["*"], strings(&["c"])),
        ("7", vec!["*"], strings(&[])),
        ("8", vec!["*"], strings(&["a", "b", "c", "d", "e"])),
        ("9", vec![], strings(&[])),
    ];
    let items: Vec<Item> = items
        .into_iter()
        .map(|(id, categories, labels)| Item {
            item_id: id.to_owned(),
            categories: (!categories.is_empty())
                .then(|| categories.into_iter().map(str::to_owned).collect()),
            timestamp: now.clone(),
            labels,
            ..Default::default()
        })
        .collect();
    let mut feedbacks = Vec::new();
    for i in 0..10 {
        for j in 0..=i {
            if i % 2 == 1 {
                feedbacks.push(feedback(
                    "FeedbackType",
                    &j.to_string(),
                    &i.to_string(),
                    now.clone(),
                ));
            }
        }
    }
    let data = master.data();
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    data.batch_insert_feedback(&ctx, &feedbacks, true, true, true)
        .await
        .expect("insert the feedback");

    // insert hidden item
    data.batch_insert_items(
        &ctx,
        &[Item {
            item_id: "10".to_owned(),
            labels: strings(&["a", "b", "c", "d", "e"]),
            is_hidden: true,
            ..Default::default()
        }],
    )
    .await
    .expect("insert the hidden item");
    for i in 0..=10 {
        data.batch_insert_feedback(
            &ctx,
            &[feedback("FeedbackType", &i.to_string(), "10", now.clone())],
            true,
            true,
            true,
        )
        .await
        .expect("insert the hidden feedback");
    }

    // load mock dataset
    let dataset = load_positive(&master, "FeedbackType").await;

    // similar items (common users)
    let users = ItemToItemConfig {
        name: "users".to_owned(),
        kind: "users".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.recommend.item_to_item = Some(vec![users.clone()])
    });
    master
        .update_item_to_item(&ctx, &dataset)
        .await
        .expect("update the item-to-item recommender");
    let vectors = master.vectors();
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &users, "9", &[], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["7", "5", "3"]);
    // similar items in category (common users)
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &users, "9", &["*"], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["7", "5", "1"]);

    // similar items (common labels)
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_ITEM_TIME, "8"])).await;
    let tags = ItemToItemConfig {
        name: "tags".to_owned(),
        kind: "tags".to_owned(),
        column: "item.Labels".to_owned(),
    };
    set_config(&master, |cfg| {
        cfg.recommend.item_to_item = Some(vec![tags.clone()])
    });
    master
        .update_item_to_item(&ctx, &dataset)
        .await
        .expect("update the item-to-item recommender");
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &tags, "8", &[], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["0", "2", "4"]);
    // similar items in category (common labels)
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &tags, "8", &["*"], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["0", "2", "6"]);

    // similar items (auto)
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_ITEM_TIME, "8"])).await;
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_ITEM_TIME, "9"])).await;
    let auto = ItemToItemConfig {
        name: "auto".to_owned(),
        kind: "auto".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.recommend.item_to_item = Some(vec![auto.clone()])
    });
    master
        .update_item_to_item(&ctx, &dataset)
        .await
        .expect("update the item-to-item recommender");
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &auto, "8", &[], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["0", "2", "4"]);
    let similar = query_item_to_item(&ctx, vectors.as_ref(), &auto, "9", &[], 3)
        .await
        .expect("query the item-to-item recommender");
    assert_eq!(ids(&similar), ["7", "5", "3"]);
    close_stores(&master).await;
}

/// Sets the cache value `name` to the current time.
async fn cache_set_time(master: &Master, name: &str) {
    master
        .cache()
        .set(
            &Context::background(),
            &[cache::time(name.to_owned(), gotime::now())],
        )
        .await
        .expect("set the time");
}

/// Go's `TestUserToUser`: the user-to-user recommenders over common items and
/// common labels, and the automatic one. The vector of a user the update does
/// not hold is removed.
#[tokio::test]
async fn test_user_to_user() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.master.n_jobs = 4;
    });
    let now = gotime::now();
    let users = [
        ("0", strings(&["a", "b", "c", "d"])),
        ("1", strings(&[])),
        ("2", strings(&["b", "c", "d"])),
        ("3", strings(&[])),
        ("4", strings(&["b", "c"])),
        ("5", strings(&[])),
        ("6", strings(&["c"])),
        ("7", strings(&[])),
        ("8", strings(&["a", "b", "c", "d", "e"])),
        ("9", strings(&[])),
    ]
    .into_iter()
    .map(|(id, labels)| User {
        user_id: id.to_owned(),
        labels,
        ..Default::default()
    })
    .collect::<Vec<_>>();
    let mut feedbacks = Vec::new();
    for i in 0..10 {
        for j in 0..=i {
            if i % 2 == 1 {
                feedbacks.push(feedback(
                    "FeedbackType",
                    &j.to_string(),
                    &i.to_string(),
                    now.clone(),
                ));
            }
        }
    }
    let data = master.data();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");
    // The feedback is written from the user's side: the user is `i` and the item `j`.
    let feedbacks: Vec<Feedback> = feedbacks
        .into_iter()
        .map(|f| Feedback {
            feedback_key: FeedbackKey {
                user_id: f.feedback_key.item_id.clone(),
                item_id: f.feedback_key.user_id.clone(),
                ..f.feedback_key
            },
            ..f
        })
        .collect();
    data.batch_insert_feedback(&ctx, &feedbacks, true, true, true)
        .await
        .expect("insert the feedback");
    let dataset = load_positive(&master, "FeedbackType").await;

    // similar users (common items)
    let items = UserToUserConfig {
        name: "items".to_owned(),
        kind: "items".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.recommend.user_to_user = Some(vec![items.clone()])
    });
    let collection = vectors::user_to_user_collection("items");
    let vector_store = master.vectors();
    vector_store
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    vector_store
        .add_vectors(
            &ctx,
            &collection,
            &[Vector {
                id: "stale".to_owned(),
                h_values: Vec::new(),
                indices: vec![0],
                values: vec![1.0],
                is_hidden: false,
                categories: Vec::new(),
                timestamp: dataset.get_timestamp().add(Duration(-HOUR.0)),
            }],
        )
        .await
        .expect("add the stale vector");
    master
        .update_user_to_user(&ctx, &dataset)
        .await
        .expect("update the user-to-user recommender");
    let stale = vector_store
        .get_vectors(&ctx, &collection, &["stale"])
        .await
        .expect("get the stale vector");
    assert!(stale.is_empty());
    let similar = query_user_to_user(&ctx, vector_store.as_ref(), &items, "9", 3)
        .await
        .expect("query the user-to-user recommender");
    assert_eq!(ids(&similar), ["7", "5", "3"]);

    // similar users (common labels)
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_USER_TIME, "8"])).await;
    let tags = UserToUserConfig {
        name: "tags".to_owned(),
        kind: "tags".to_owned(),
        column: "user.Labels".to_owned(),
    };
    set_config(&master, |cfg| {
        cfg.recommend.user_to_user = Some(vec![tags.clone()])
    });
    master
        .update_user_to_user(&ctx, &dataset)
        .await
        .expect("update the user-to-user recommender");
    let similar = query_user_to_user(&ctx, vector_store.as_ref(), &tags, "8", 3)
        .await
        .expect("query the user-to-user recommender");
    assert_eq!(ids(&similar), ["0", "2", "4"]);

    // similar users (auto)
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_USER_TIME, "8"])).await;
    cache_set_time(&master, &cache::key(&[cache::LAST_MODIFY_USER_TIME, "9"])).await;
    let auto = UserToUserConfig {
        name: "auto".to_owned(),
        kind: "auto".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.recommend.user_to_user = Some(vec![auto.clone()])
    });
    master
        .update_user_to_user(&ctx, &dataset)
        .await
        .expect("update the user-to-user recommender");
    let similar = query_user_to_user(&ctx, vector_store.as_ref(), &auto, "8", 3)
        .await
        .expect("query the user-to-user recommender");
    assert_eq!(ids(&similar), ["0", "2", "4"]);
    let similar = query_user_to_user(&ctx, vector_store.as_ref(), &auto, "9", 3)
        .await
        .expect("query the user-to-user recommender");
    assert_eq!(ids(&similar), ["7", "5", "3"]);
    close_stores(&master).await;
}

/// Records the snapshots the load emits, as Go's `snapshotHandler` does.
struct SnapshotHandler {
    snapshots: mpsc::Sender<event::Snapshot>,
}

impl event::Handler for SnapshotHandler {
    fn emit_request(&self, _ctx: &Context, _event: &event::Request) {}

    fn emit_snapshot(&self, _ctx: &Context, event: &event::Snapshot) {
        let _ = self.snapshots.send(event.clone());
    }
}

/// Go's `TestEmitSnapshot`: a load emits the sizes of the users, items and
/// feedback it read.
#[tokio::test]
async fn test_emit_snapshot() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.master.n_jobs = 1;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.negative_feedback_types = Some(vec![expression("negative")]);
    });
    let mut users = vec![
        User {
            user_id: "0".to_owned(),
            ..Default::default()
        },
        User {
            user_id: "1".to_owned(),
            ..Default::default()
        },
    ];
    let mut items = vec![
        Item {
            item_id: "0".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "1".to_owned(),
            ..Default::default()
        },
    ];
    let feedbacks = vec![
        feedback("positive", "0", "0", gotime::unix(0, 0)),
        feedback("negative", "0", "0", gotime::unix(0, 0)),
    ];
    let data = master.data();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    data.batch_insert_feedback(&ctx, &feedbacks, false, false, false)
        .await
        .expect("insert the feedback");
    for user in &mut users {
        *user = data
            .get_user(&ctx, &user.user_id)
            .await
            .expect("get the user");
    }
    for item in &mut items {
        *item = data
            .get_item(&ctx, &item.item_id)
            .await
            .expect("get the item");
    }

    let (sender, snapshots) = mpsc::channel();
    event::set_event_handler(Arc::new(SnapshotHandler { snapshots: sender }));
    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");
    event::set_event_handler(Arc::new(event::NopHandler));

    assert_eq!(
        1,
        datasets.click_train_set.count() + datasets.click_test_set.count()
    );
    let snapshot = snapshots
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the snapshot is emitted");
    assert_eq!(users.len() as i64, snapshot.user_count);
    assert_eq!(deep_size_of(&users, user_size), snapshot.user_bytes);
    assert_eq!(items.len() as i64, snapshot.item_count);
    assert_eq!(deep_size_of(&items, item_size), snapshot.item_bytes);
    assert_eq!(feedbacks.len() as i64, snapshot.feedback_count);
    assert_eq!(
        deep_size_of(&feedbacks, super::feedback_size),
        snapshot.feedback_bytes
    );
    assert!(!snapshot.timestamp.is_zero());
    close_stores(&master).await;
}

/// Go's `TestLoadDataFromDatabase`: the counts of the ranking and click-through
/// rate datasets, the latest items, and the categories.
#[tokio::test]
async fn test_load_data_from_database() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.read_feedback_types = Some(vec![expression("negative")]);
        cfg.master.n_jobs = num_cpus();
    });
    let data = master.data();

    // insert items
    let mut items = Vec::new();
    for i in 0..9 {
        items.push(Item {
            item_id: i.to_string(),
            timestamp: gotime::date(2000 + i, Month::JANUARY, 1, 1, 1, 0, 0, &gotime::utc()),
            labels: strings(&[&(i % 3).to_string(), &(i * 10 + 10).to_string()]),
            categories: Some(vec![(i % 3).to_string()]),
            ..Default::default()
        });
    }
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    data.batch_insert_items(
        &ctx,
        &[Item {
            item_id: "9".to_owned(),
            timestamp: gotime::date(2020, Month::JANUARY, 1, 1, 1, 0, 0, &gotime::utc()),
            is_hidden: true,
            ..Default::default()
        }],
    )
    .await
    .expect("insert the hidden item");

    // insert users
    let mut users = Vec::new();
    for i in 0..=10 {
        users.push(User {
            user_id: i.to_string(),
            labels: strings(&[&(i % 5).to_string(), &(i * 10 + 10).to_string()]),
            ..Default::default()
        });
    }
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");

    // insert feedback
    let now = gotime::now();
    let mut feedbacks = Vec::new();
    for i in 0..10 {
        // positive feedback: item i is liked by users 0..=i
        for j in 0..=i {
            feedbacks.push(feedback(
                "positive",
                &j.to_string(),
                &i.to_string(),
                now.clone(),
            ));
        }
        // negative feedback: item i is read by users i+1..=10
        for j in (i + 1)..11 {
            feedbacks.push(feedback(
                "negative",
                &j.to_string(),
                &i.to_string(),
                now.clone(),
            ));
        }
    }
    data.batch_insert_feedback(&ctx, &feedbacks, false, false, true)
        .await
        .expect("insert the feedback");

    // load dataset
    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");
    assert_eq!(11, datasets.ranking_train_set.count_users());
    assert_eq!(10, datasets.ranking_train_set.count_items());
    assert_eq!(11, datasets.ranking_test_set.count_users());
    assert_eq!(10, datasets.ranking_test_set.count_items());
    assert_eq!(
        55,
        datasets.ranking_train_set.count_feedback() + datasets.ranking_test_set.count_feedback()
    );
    let click_train = datasets.click_train_set.index.as_ref().expect("an index");
    let click_test = datasets.click_test_set.index.as_ref().expect("an index");
    assert_eq!(11, click_train.count_users());
    assert_eq!(10, click_train.count_items());
    assert_eq!(11, click_test.count_users());
    assert_eq!(10, click_test.count_items());
    assert_eq!(3, click_train.count_item_labels());
    assert_eq!(5, click_train.count_user_labels());
    assert_eq!(3, click_test.count_item_labels());
    assert_eq!(5, click_test.count_user_labels());
    assert_eq!(
        110,
        datasets.click_train_set.count() + datasets.click_test_set.count()
    );
    assert_eq!(
        55,
        datasets.click_train_set.positive_count + datasets.click_test_set.positive_count
    );
    assert_eq!(
        55,
        datasets.click_train_set.negative_count + datasets.click_test_set.negative_count
    );

    // check latest items
    let mut latest = data
        .get_latest_items(&ctx, 3, &[], None)
        .await
        .expect("get the latest items");
    for item in &mut latest {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(
        latest,
        vec![items[8].clone(), items[7].clone(), items[6].clone()]
    );
    let mut latest = data
        .get_latest_items(&ctx, 3, &["2".to_owned()], None)
        .await
        .expect("get the latest items");
    for item in &mut latest {
        assert!(!item.update_at.is_zero());
        item.update_at = Time::default();
    }
    assert_eq!(
        latest,
        vec![items[8].clone(), items[5].clone(), items[2].clone()]
    );

    // check categories
    let categories = master
        .cache()
        .search_scores(&ctx, cache::ITEM_CATEGORIES, "", &[], 0, -1)
        .await
        .expect("search the categories");
    let categories: Vec<String> = categories.into_iter().map(|score| score.id).collect();
    assert_eq!(categories, ["0", "1", "2"]);
    close_stores(&master).await;
}

/// Go's `TestNegativeFeedbackPriority`: a user who gave an item both positive
/// and negative feedback has it as negative, and the recommender excludes it.
#[tokio::test]
async fn test_negative_feedback_priority() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.negative_feedback_types = Some(vec![expression("dislike")]);
        cfg.recommend.data_source.read_feedback_types = Some(vec![expression("read")]);
        cfg.master.n_jobs = num_cpus();
    });
    let data = master.data();

    // insert items
    let items: Vec<Item> = (0..5)
        .map(|i| Item {
            item_id: i.to_string(),
            timestamp: gotime::date(2000 + i, Month::JANUARY, 1, 1, 1, 0, 0, &gotime::utc()),
            ..Default::default()
        })
        .collect();
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");

    // insert users
    let users: Vec<User> = (0..3)
        .map(|i| User {
            user_id: i.to_string(),
            ..Default::default()
        })
        .collect();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");

    // insert feedback
    let now = gotime::now();
    let feedbacks = vec![
        // User 0: positive on item 0, 1; negative on item 2; read on item 3, 4
        feedback("positive", "0", "0", now.clone()),
        feedback("positive", "0", "1", now.clone()),
        feedback("dislike", "0", "2", now.clone()),
        feedback("read", "0", "3", now.clone()),
        feedback("read", "0", "4", now.clone()),
        // User 1: positive AND negative on item 0 (should be negative due to priority)
        feedback("positive", "1", "0", now.clone()),
        feedback("dislike", "1", "0", now.clone()),
        feedback("positive", "1", "1", now.clone()),
        // User 2: positive, then negative on item 2 (should be negative due to priority)
        feedback("positive", "2", "2", now.clone().add(Duration(-HOUR.0))),
        feedback("dislike", "2", "2", now.clone()),
        feedback("positive", "2", "3", now.clone()),
    ];
    data.batch_insert_feedback(&ctx, &feedbacks, false, false, true)
        .await
        .expect("insert the feedback");

    // load dataset
    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");

    // 9 samples: 4 positive and 5 negative, across the two splits
    assert_eq!(
        9,
        datasets.click_train_set.count() + datasets.click_test_set.count()
    );
    assert_eq!(
        4,
        datasets.click_train_set.positive_count + datasets.click_test_set.positive_count
    );
    assert_eq!(
        5,
        datasets.click_train_set.negative_count + datasets.click_test_set.negative_count
    );

    // Negative feedback items are excluded from recommendations.
    let recommender = Recommender::new(
        master.config().recommend.clone(),
        master.cache(),
        master.data(),
        master.vectors(),
        true,
        "1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    assert!(
        recommender.exclude_set().contains("0"),
        "item 0 should be excluded due to dislike feedback"
    );
    close_stores(&master).await;
}

/// Go's `TestNonPersonalizedRecommend`: the scores of the non-personalized
/// recommender, and its digest.
#[tokio::test]
async fn test_non_personalized_recommend() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    let latest = NonPersonalizedConfig {
        name: "latest".to_owned(),
        score: "item.Timestamp.Unix()".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.read_feedback_types = Some(vec![expression("negative")]);
        cfg.recommend.non_personalized = Some(vec![latest.clone()]);
        cfg.master.n_jobs = num_cpus();
    });
    let data = master.data();

    // insert items
    let items: Vec<Item> = (0..10)
        .map(|i| Item {
            item_id: i.to_string(),
            timestamp: gotime::date(2000 + i % 2, Month::JANUARY, 1, i, 1, 0, 0, &gotime::utc()),
            ..Default::default()
        })
        .collect();
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");

    // insert users
    let users: Vec<User> = (0..10)
        .map(|i| User {
            user_id: i.to_string(),
            ..Default::default()
        })
        .collect();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");

    // insert feedback: even items are liked by users 0..=i
    let now = gotime::now();
    let mut feedbacks = Vec::new();
    for i in (0..10).filter(|i| i % 2 == 0) {
        for j in 0..=i {
            feedbacks.push(feedback(
                "positive",
                &j.to_string(),
                &i.to_string(),
                now.clone(),
            ));
        }
    }
    data.batch_insert_feedback(&ctx, &feedbacks, false, false, true)
        .await
        .expect("insert the feedback");

    // load dataset
    master.load_dataset(&ctx).await.expect("load the dataset");

    // check latest items
    let latest_scores = master
        .cache()
        .search_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "latest",
            &[String::new()],
            0,
            3,
        )
        .await
        .expect("search the scores");
    let got: Vec<(String, f64)> = latest_scores
        .into_iter()
        .map(|score| (score.id, score.score))
        .collect();
    let want: Vec<(String, f64)> = [9, 7, 5]
        .into_iter()
        .map(|i| (items[i].item_id.clone(), items[i].timestamp.unix() as f64))
        .collect();
    assert_eq!(got, want);

    // check digest
    let digest = master
        .cache()
        .get(
            &ctx,
            &cache::key(&[cache::NON_PERSONALIZED_DIGEST, "latest"]),
        )
        .await
        .string()
        .expect("read the digest");
    assert_eq!(latest.hash(), digest);
    close_stores(&master).await;
}

/// Go's `TestGarbageCollection`: the scores of a configuration that is gone,
/// and the collaborative filtering scores of a user who is gone, are deleted.
#[tokio::test]
async fn test_garbage_collection() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.master.n_jobs = 1;
        cfg.recommend.non_personalized = Some(vec![NonPersonalizedConfig {
            name: "custom".to_owned(),
            score: "1".to_owned(),
            ..Default::default()
        }]);
    });

    // insert items
    let data = master.data();
    let now = gotime::now();
    data.batch_insert_items(
        &ctx,
        &[
            Item {
                item_id: "1".to_owned(),
                timestamp: now.clone(),
                categories: Some(vec!["*".to_owned()]),
                labels: strings(&["a", "b", "c", "d"]),
                ..Default::default()
            },
            Item {
                item_id: "2".to_owned(),
                timestamp: now.clone(),
                categories: Some(vec!["*".to_owned()]),
                labels: strings(&[]),
                ..Default::default()
            },
        ],
    )
    .await
    .expect("insert the items");

    // insert users
    data.batch_insert_users(
        &ctx,
        &[
            User {
                user_id: "1".to_owned(),
                labels: strings(&["a", "b", "c", "d"]),
                ..Default::default()
            },
            User {
                user_id: "2".to_owned(),
                labels: strings(&[]),
                ..Default::default()
            },
        ],
    )
    .await
    .expect("insert the users");

    // insert non-personalized cache
    let cache_client = master.cache();
    let timestamp = now.clone().add(HOUR);
    let scored = |id: &str, score: f64, timestamp: Option<Time>| Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: vec![String::new()],
        timestamp: timestamp.unwrap_or_default(),
    };
    cache_client
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "custom",
            &[
                scored("1", 1.0, Some(timestamp.clone())),
                scored("2", 2.0, Some(timestamp.clone())),
            ],
        )
        .await
        .expect("add the scores");
    cache_client
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "unknown",
            &[
                scored("1", 1.0, Some(timestamp.clone())),
                scored("2", 2.0, Some(timestamp)),
            ],
        )
        .await
        .expect("add the scores");

    // insert collaborative filtering cache
    let collaborative = |id: &str, score: f64| Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: vec![String::new()],
        timestamp: Time::default(),
    };
    cache_client
        .add_scores(
            &ctx,
            cache::COLLABORATIVE_FILTERING,
            "1",
            &[collaborative("1", 1.0), collaborative("2", 2.0)],
        )
        .await
        .expect("add the scores");
    cache_client
        .add_scores(
            &ctx,
            cache::COLLABORATIVE_FILTERING,
            "3",
            &[collaborative("1", 1.0), collaborative("2", 2.0)],
        )
        .await
        .expect("add the scores");

    // load dataset and run garbage collection
    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");
    master
        .collect_garbage(&ctx, &datasets.ranking_dataset)
        .await
        .expect("collect the garbage");

    // check non-personalized cache
    let np = cache_client
        .search_scores(&ctx, cache::NON_PERSONALIZED, "custom", &[], 0, 100)
        .await
        .expect("search the scores");
    assert_eq!(ids(&np), ["2", "1"]);
    let np = cache_client
        .search_scores(&ctx, cache::NON_PERSONALIZED, "unknown", &[], 0, 100)
        .await
        .expect("search the scores");
    assert!(np.is_empty());

    // check collaborative filtering cache
    let cf = cache_client
        .search_scores(&ctx, cache::COLLABORATIVE_FILTERING, "1", &[], 0, 100)
        .await
        .expect("search the scores");
    assert_eq!(ids(&cf), ["2", "1"]);
    let cf = cache_client
        .search_scores(&ctx, cache::COLLABORATIVE_FILTERING, "3", &[], 0, 100)
        .await
        .expect("search the scores");
    assert!(cf.is_empty());
    close_stores(&master).await;
}

/// Go's `TestLoadDataFromDatabaseInParallel`: a single user with 12,000
/// positive feedback records is loaded in full across the parallel batches.
#[tokio::test]
async fn test_load_data_from_database_in_parallel() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(get_default_config()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.master.n_jobs = 16;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.negative_feedback_types = None;
        cfg.recommend.data_source.read_feedback_types = None;
    });
    const NUM_ITEMS: usize = 12000;
    let items: Vec<Item> = (0..NUM_ITEMS)
        .map(|i| Item {
            item_id: format!("item-{i:05}"),
            timestamp: gotime::unix(i as i64, 0),
            ..Default::default()
        })
        .collect();
    let feedbacks: Vec<Feedback> = (0..NUM_ITEMS)
        .map(|i| {
            feedback(
                "positive",
                "hot-user",
                &format!("item-{i:05}"),
                gotime::unix(i as i64, 0),
            )
        })
        .collect();

    let data = master.data();
    data.batch_insert_users(
        &ctx,
        &[User {
            user_id: "hot-user".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .expect("insert the user");
    // The SQLite backend binds at most 32,766 values per statement, which 12,000
    // rows in one statement exceed, so the batch goes in parts. The rows and the
    // load under test are the same as Go's.
    for part in items.chunks(1000) {
        data.batch_insert_items(&ctx, part)
            .await
            .expect("insert the items");
    }
    for part in feedbacks.chunks(1000) {
        data.batch_insert_feedback(&ctx, part, false, false, true)
            .await
            .expect("insert the feedback");
    }

    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");
    assert_eq!(1, datasets.ranking_train_set.count_users());
    assert_eq!(1, datasets.ranking_test_set.count_users());
    assert_eq!(
        NUM_ITEMS,
        datasets.ranking_train_set.count_feedback() + datasets.ranking_test_set.count_feedback()
    );
    assert_eq!(
        NUM_ITEMS,
        datasets.click_train_set.count() + datasets.click_test_set.count()
    );
    assert_eq!(
        NUM_ITEMS,
        datasets.click_train_set.positive_count + datasets.click_test_set.positive_count
    );
    assert_eq!(
        0,
        datasets.click_train_set.negative_count + datasets.click_test_set.negative_count
    );
    close_stores(&master).await;
}

/// O-75 and Q-259. The tags recommender reads label IDs. The master gives it
/// the processed items, which carry the IDs, so their tags are found. The same
/// labels as raw values carry no IDs, so the recommender finds no tags.
#[tokio::test]
async fn tags_recommender_reads_processed_labels_only() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.master.n_jobs = 1;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
    });
    let now = gotime::now();
    let data = master.data();
    data.batch_insert_items(
        &ctx,
        &[
            Item {
                item_id: "0".to_owned(),
                timestamp: now.clone(),
                labels: strings(&["a", "b"]),
                ..Default::default()
            },
            Item {
                item_id: "1".to_owned(),
                timestamp: now.clone(),
                labels: strings(&["b"]),
                ..Default::default()
            },
            Item {
                item_id: "2".to_owned(),
                timestamp: now.clone(),
                labels: strings(&["a"]),
                ..Default::default()
            },
        ],
    )
    .await
    .expect("insert the items");
    data.batch_insert_users(
        &ctx,
        &[User {
            user_id: "0".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .expect("insert the user");
    data.batch_insert_feedback(
        &ctx,
        &[feedback("positive", "0", "0", now.clone())],
        false,
        false,
        true,
    )
    .await
    .expect("insert the feedback");
    let dataset = master
        .load_dataset(&ctx)
        .await
        .expect("load the dataset")
        .ranking_dataset;

    // The processed items: the master's tags recommender finds the tags of item 0.
    let tags = ItemToItemConfig {
        name: "tags".to_owned(),
        kind: "tags".to_owned(),
        column: "item.Labels".to_owned(),
    };
    set_config(&master, |cfg| {
        cfg.recommend.item_to_item = Some(vec![tags.clone()])
    });
    master
        .update_item_to_item(&ctx, &dataset)
        .await
        .expect("update the item-to-item recommender");
    let vector_store = master.vectors();
    let processed = vector_store
        .get_vectors(&ctx, &vectors::item_to_item_collection("tags"), &["0"])
        .await
        .expect("get the vectors");
    assert_eq!(processed.len(), 1);
    assert_eq!(
        processed[0].indices.len(),
        2,
        "the tags of item 0 are a and b"
    );

    // The raw values of the same items: the recommender finds no tags.
    let raw_tags = ItemToItemConfig {
        name: "raw_tags".to_owned(),
        ..tags.clone()
    };
    let options = ItemToItemOptions {
        ctx: Some(ctx.clone()),
        vector_client: Some(vector_store.clone()),
        vector_config: VectorConfig::default(),
        batch_size: BATCH_SIZE,
        tags_idf: Some(dataset.get_item_column_values_idf()),
        users_idf: Some(dataset.get_user_idf()),
    };
    let recommender = new_item_to_item(&raw_tags, dataset.get_timestamp(), Some(&options))
        .expect("build the raw recommender");
    let raw_labels = [("0", vec!["a", "b"]), ("1", vec!["b"]), ("2", vec!["a"])];
    for (i, item) in dataset.get_items().iter().enumerate() {
        let raw = DatasetItem {
            item_id: item.item_id.clone(),
            is_hidden: item.is_hidden,
            categories: item.categories.clone(),
            timestamp: item.timestamp.clone(),
            labels: Value::Plain(strings(
                &raw_labels
                    .iter()
                    .find(|(id, _)| *id == item.item_id)
                    .map(|(_, labels)| labels.clone())
                    .unwrap_or_default(),
            )),
            comment: item.comment.clone(),
        };
        recommender
            .add(&raw, &dataset.get_item_feedback()[i])
            .await
            .expect("add the raw item");
    }
    recommender
        .clean()
        .await
        .expect("clean the raw recommender");
    // Go's writer creates its collection when it flushes a vector, so a
    // recommender that finds no tags leaves no collection behind.
    let raw = match vector_store
        .get_vectors(
            &ctx,
            &vectors::item_to_item_collection("raw_tags"),
            &["0", "1", "2"],
        )
        .await
    {
        Ok(raw) => raw,
        Err(err) if err.kind() == Some(StorageError::NotFound) => Vec::new(),
        Err(err) => panic!("get the vectors: {err}"),
    };
    let raw_tag_count: usize = raw.iter().map(|vector| vector.indices.len()).sum();
    assert_eq!(0, raw_tag_count, "raw labels carry no tag IDs");
    close_stores(&master).await;
}

/// X-199. The master reads each item's feedback before it hands the items to
/// the recommenders, and the recommenders no longer sort those lists in place.
/// The dataset the update reads is the same after the update.
#[tokio::test]
async fn item_feedback_is_read_in_order_and_not_reordered() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    let users = ItemToItemConfig {
        name: "users".to_owned(),
        kind: "users".to_owned(),
        ..Default::default()
    };
    set_config(&master, |cfg| {
        cfg.master.n_jobs = 1;
        cfg.recommend.item_to_item = Some(vec![users.clone()]);
    });
    let now = gotime::now();
    let mut dataset = Dataset::new(now.clone(), 4, 3);
    for user in ["0", "1", "2", "3"] {
        dataset.add_user(User {
            user_id: user.to_owned(),
            ..Default::default()
        });
    }
    for item in ["0", "1", "2"] {
        dataset.add_item(Item {
            item_id: item.to_owned(),
            timestamp: now.clone(),
            ..Default::default()
        });
    }
    // Item 2 is liked by users 3, 0 and 2 in that order, which is not sorted.
    for (user, item) in [("3", "2"), ("0", "2"), ("2", "2"), ("1", "0"), ("0", "1")] {
        dataset.add_feedback(user, item, now.clone());
    }
    let before: Vec<Vec<i32>> = dataset.get_item_feedback().to_vec();
    assert!(
        before
            .iter()
            .any(|list| list.windows(2).any(|pair| pair[0] > pair[1])),
        "the feedback must start unsorted for the test to mean anything"
    );

    master
        .update_item_to_item(&ctx, &dataset)
        .await
        .expect("update the item-to-item recommender");

    assert_eq!(dataset.get_item_feedback(), before.as_slice());
    let collections = master
        .vectors()
        .list_collections(&ctx)
        .await
        .expect("list the collections");
    assert!(collections.contains(&vectors::item_to_item_collection("users")));
    close_stores(&master).await;
}

/// The number of CPUs, as Go's `runtime.NumCPU` gives it.
fn num_cpus() -> i64 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .unwrap_or(1)
}

/// Go's `loadDataset` sets the dataset gauges from the datasets it has just
/// loaded (master/tasks.go 159 to 214). The gauges are process-wide, so each
/// one starts at a sentinel that no load writes, and the load must overwrite it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_load_dataset_sets_gauges() {
    let _serial = SERIAL.lock().await;
    let (_dir, master) = setup(Config::default()).await;
    let ctx = Context::background();
    set_config(&master, |cfg| {
        cfg.recommend.cache_size = 3;
        cfg.recommend.data_source.positive_feedback_types = Some(vec![expression("positive")]);
        cfg.recommend.data_source.negative_feedback_types = Some(vec![expression("dislike")]);
        cfg.recommend.data_source.read_feedback_types = Some(vec![expression("read")]);
        cfg.master.n_jobs = num_cpus();
    });
    let data = master.data();

    // five items with labels 0, 1, 2, 0, 1 and three users with labels 0, 1, 0
    let items: Vec<Item> = (0..5)
        .map(|i| Item {
            item_id: i.to_string(),
            timestamp: gotime::date(2000 + i, Month::JANUARY, 1, 1, 1, 0, 0, &gotime::utc()),
            labels: strings(&[&(i % 3).to_string()]),
            ..Default::default()
        })
        .collect();
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    let users: Vec<User> = (0..3)
        .map(|i| User {
            user_id: i.to_string(),
            labels: strings(&[&(i % 2).to_string()]),
            ..Default::default()
        })
        .collect();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");

    // the feedback of `test_negative_feedback_priority`: 4 positive, 5 negative
    let now = gotime::now();
    let feedbacks = vec![
        feedback("positive", "0", "0", now.clone()),
        feedback("positive", "0", "1", now.clone()),
        feedback("dislike", "0", "2", now.clone()),
        feedback("read", "0", "3", now.clone()),
        feedback("read", "0", "4", now.clone()),
        feedback("positive", "1", "0", now.clone()),
        feedback("dislike", "1", "0", now.clone()),
        feedback("positive", "1", "1", now.clone()),
        feedback("positive", "2", "2", now.clone().add(Duration(-HOUR.0))),
        feedback("dislike", "2", "2", now.clone()),
        feedback("positive", "2", "3", now.clone()),
    ];
    data.batch_insert_feedback(&ctx, &feedbacks, false, false, true)
        .await
        .expect("insert the feedback");

    let gauges: [&prometheus::Gauge; 11] = [
        &metrics::USERS_TOTAL,
        &metrics::ITEMS_TOTAL,
        &metrics::IMPLICIT_FEEDBACKS_TOTAL,
        &metrics::USER_LABELS_TOTAL,
        &metrics::ITEM_LABELS_TOTAL,
        &metrics::POSITIVE_FEEDBACKS_TOTAL,
        &metrics::NEGATIVE_FEEDBACK_TOTAL,
        &metrics::ACTIVE_USERS_TOTAL,
        &metrics::INACTIVE_USERS_TOTAL,
        &metrics::ACTIVE_ITEMS_TOTAL,
        &metrics::INACTIVE_ITEMS_TOTAL,
    ];
    for gauge in gauges {
        gauge.set(-1.0);
    }

    let datasets = master.load_dataset(&ctx).await.expect("load the dataset");

    // the splits partition the ranking dataset, and both hold every user and item
    let ranking_feedback =
        datasets.ranking_train_set.count_feedback() + datasets.ranking_test_set.count_feedback();
    let positive = datasets.click_train_set.positive_count + datasets.click_test_set.positive_count;
    let negative = datasets.click_train_set.negative_count + datasets.click_test_set.negative_count;
    assert_eq!(3.0, metrics::USERS_TOTAL.get(), "users");
    assert_eq!(5.0, metrics::ITEMS_TOTAL.get(), "items");
    assert_eq!(
        ranking_feedback as f64,
        metrics::IMPLICIT_FEEDBACKS_TOTAL.get(),
        "implicit feedbacks"
    );
    // Go adds a label to its index at the second occurrence, so a label seen
    // once is not counted (master/tasks.go, the user and item loaders): the user
    // labels 0, 1, 0 count one, and the item labels 0, 1, 2, 0, 1 count two.
    assert_eq!(1.0, metrics::USER_LABELS_TOTAL.get(), "user labels");
    assert_eq!(2.0, metrics::ITEM_LABELS_TOTAL.get(), "item labels");
    assert_eq!(4.0, positive as f64, "the fixture's positive samples");
    assert_eq!(
        positive as f64,
        metrics::POSITIVE_FEEDBACKS_TOTAL.get(),
        "positive feedbacks"
    );
    assert_eq!(5.0, negative as f64, "the fixture's negative samples");
    assert_eq!(
        negative as f64,
        metrics::NEGATIVE_FEEDBACK_TOTAL.get(),
        "negative feedbacks"
    );
    assert!(
        metrics::ACTIVE_USERS_TOTAL.get() > 0.0,
        "users with feedback: {}",
        metrics::ACTIVE_USERS_TOTAL.get()
    );
    assert_eq!(
        metrics::USERS_TOTAL.get(),
        metrics::ACTIVE_USERS_TOTAL.get() + metrics::INACTIVE_USERS_TOTAL.get(),
        "active and inactive users"
    );
    assert_eq!(
        metrics::ITEMS_TOTAL.get(),
        metrics::ACTIVE_ITEMS_TOTAL.get() + metrics::INACTIVE_ITEMS_TOTAL.get(),
        "active and inactive items"
    );
}
