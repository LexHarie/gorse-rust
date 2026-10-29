// Ports: logics/recommend_test.go @ add21b5

//! The recommender suite of `logics/recommend_test.go`, one test per Go suite
//! method. Go's suite shares one set of stores between its methods. Each test
//! here opens its own, so no test sees another's rows. The expectations are
//! Go's. None of them reads a row that another method writes.
//!
//! The stores are the SQLite data and cache stores and the `xvec` vector store,
//! the same backends the Go suite opens.

use std::sync::Arc;

use gorse_common::expression::{ExprType, FeedbackTypeExpression};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_config::config::schema::{
    DataSourceConfig, ExternalConfig, ItemToItemConfig, RecommendConfig, UserToUserConfig,
};
use gorse_storage::cache::{self, Score};
use gorse_storage::data;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use indexmap::IndexMap;

use super::{Recommender, parse};
use crate::testing::{Reply, serve};

/// The stores one test reads and writes.
struct Stores {
    /// Keeps the directory of the SQLite files and the vector store alive.
    _dir: tempfile::TempDir,
    data: Arc<dyn data::Database>,
    cache: Arc<dyn cache::Database>,
    vectors: Arc<dyn vectors::Database>,
}

/// Opens and initialises a fresh data, cache and vector store.
async fn open_stores() -> Stores {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut data_registry = data::Registry::default();
    data::sql::register(&mut data_registry);
    let data: Arc<dyn data::Database> = Arc::from(
        data_registry
            .open(
                &format!("sqlite://{}/data.db", dir.path().display()),
                "",
                &[],
            )
            .await
            .expect("open the data store"),
    );
    let mut cache_registry = cache::Registry::default();
    cache::sql::register(&mut cache_registry);
    let cache: Arc<dyn cache::Database> = Arc::from(
        cache_registry
            .open(
                &format!("sqlite://{}/cache.db", dir.path().display()),
                "",
                &[],
            )
            .await
            .expect("open the cache store"),
    );
    let vectors: Arc<dyn vectors::Database> = Arc::from(
        vectors::open(&format!("xvec://{}/vectors", dir.path().display()), "", &[])
            .await
            .expect("open the vector store"),
    );
    data.init().await.expect("init the data store");
    cache.init().await.expect("init the cache store");
    vectors.init().await.expect("init the vector store");
    Stores {
        _dir: dir,
        data,
        cache,
        vectors,
    }
}

/// The click feedback type the suite's configs read as positive.
fn click() -> FeedbackTypeExpression {
    FeedbackTypeExpression {
        feedback_type: "click".to_owned(),
        expr_type: ExprType::None,
        value: 0.0,
    }
}

/// A click from `user_id` on each of `items`.
fn clicks(user_id: &str, items: impl IntoIterator<Item = String>) -> Vec<Feedback> {
    items
        .into_iter()
        .map(|item_id| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: user_id.to_owned(),
                item_id,
            },
            ..Default::default()
        })
        .collect()
}

/// A score with the given id and score, and no timestamp or categories.
fn score(id: &str, score: f64) -> Score {
    Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: Vec::new(),
        timestamp: Time::default(),
    }
}

/// A vector of `dim` numbers, as the Go `[]float32` labels hold them.
fn labels_embedding(values: &[f64]) -> Labels {
    Labels::Object(IndexMap::from([(
        "embedding".to_owned(),
        Labels::Array(values.iter().map(|v| Labels::Float64(*v)).collect()),
    )]))
}

#[tokio::test]
async fn item_to_item() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let now = gotime::unix(0, 0);
    stores
        .vectors
        .add_collection(
            &ctx,
            &vectors::item_to_item_collection("embedding"),
            2,
            Distance::Euclidean,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    stores
        .data
        .batch_insert_items(
            &ctx,
            &[
                Item {
                    item_id: "source".to_owned(),
                    labels: labels_embedding(&[0.0, 0.0]),
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Item {
                    item_id: "near".to_owned(),
                    labels: labels_embedding(&[0.1, 0.0]),
                    timestamp: now.clone(),
                    categories: Some(vec!["movie".to_owned()]),
                    ..Default::default()
                },
                Item {
                    item_id: "far".to_owned(),
                    labels: labels_embedding(&[10.0, 0.0]),
                    timestamp: now.clone(),
                    categories: Some(vec!["movie".to_owned()]),
                    ..Default::default()
                },
                Item {
                    item_id: "hidden".to_owned(),
                    labels: labels_embedding(&[0.05, 0.0]),
                    timestamp: now.clone(),
                    categories: Some(vec!["movie".to_owned()]),
                    is_hidden: true,
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("insert the items");
    stores
        .vectors
        .add_vectors(
            &ctx,
            &vectors::item_to_item_collection("embedding"),
            &[
                Vector {
                    id: "source".to_owned(),
                    values: vec![0.0, 0.0],
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Vector {
                    id: "near".to_owned(),
                    values: vec![0.1, 0.0],
                    categories: vec!["movie".to_owned()],
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Vector {
                    id: "far".to_owned(),
                    values: vec![10.0, 0.0],
                    categories: vec!["movie".to_owned()],
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Vector {
                    id: "hidden".to_owned(),
                    values: vec![0.05, 0.0],
                    categories: vec!["movie".to_owned()],
                    is_hidden: true,
                    timestamp: now.clone(),
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("add the vectors");
    stores
        .data
        .batch_insert_feedback(
            &ctx,
            &clicks("embedding_user", ["source".to_owned()]),
            true,
            true,
            false,
        )
        .await
        .expect("insert the feedback");

    let cfg = RecommendConfig {
        cache_size: 2,
        data_source: DataSourceConfig {
            positive_feedback_types: Some(vec![click()]),
            ..Default::default()
        },
        item_to_item: Some(vec![ItemToItemConfig {
            name: "embedding".to_owned(),
            kind: "embedding".to_owned(),
            column: "item.Labels.embedding".to_owned(),
        }]),
        ..Default::default()
    };
    let mut recommender = Recommender::new(
        cfg.clone(),
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "embedding_user".to_owned(),
        vec!["movie".to_owned()],
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_item_to_item(&ctx, "embedding")
        .await
        .expect("recommend");
    let item_to_item = &cfg.item_to_item.as_ref().expect("the config")[0];
    assert_eq!(digest, item_to_item.hash(Some(&cfg)));
    assert_eq!(cache::convert_documents_to_values(&scores), ["near", "far"]);

    let tags = vectors::item_to_item_collection("tags-vector");
    stores
        .vectors
        .add_collection(&ctx, &tags, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    stores
        .vectors
        .add_vectors(
            &ctx,
            &tags,
            &[
                Vector {
                    id: "source".to_owned(),
                    indices: vec![0],
                    values: vec![1.0],
                    ..Default::default()
                },
                Vector {
                    id: "near".to_owned(),
                    indices: vec![0],
                    values: vec![2.0],
                    ..Default::default()
                },
                Vector {
                    id: "unrelated".to_owned(),
                    indices: vec![1],
                    values: vec![1.0],
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("add the vectors");
    stores
        .data
        .batch_insert_feedback(
            &ctx,
            &clicks("tags-vector-user", ["source".to_owned()]),
            true,
            true,
            false,
        )
        .await
        .expect("insert the feedback");

    let cfg = RecommendConfig {
        cache_size: 2,
        data_source: DataSourceConfig {
            positive_feedback_types: Some(vec![click()]),
            ..Default::default()
        },
        item_to_item: Some(vec![ItemToItemConfig {
            name: "tags-vector".to_owned(),
            kind: "tags".to_owned(),
            column: "item.Labels".to_owned(),
        }]),
        ..Default::default()
    };
    let mut recommender = Recommender::new(
        cfg.clone(),
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "tags-vector-user".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_item_to_item(&ctx, "tags-vector")
        .await
        .expect("recommend");
    let item_to_item = &cfg.item_to_item.as_ref().expect("the config")[0];
    assert_eq!(digest, item_to_item.hash(Some(&cfg)));
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].id, "near");
    assert!(
        (scores[0].score - 2.0).abs() < 1e-6,
        "score {}",
        scores[0].score
    );
}

#[tokio::test]
async fn latest() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let items: Vec<Item> = (0..20)
        .map(|i| Item {
            item_id: format!("item_{i}"),
            timestamp: gotime::unix(i, 0),
            categories: (i % 2 == 0).then(|| vec!["cat_1".to_owned()]),
            ..Default::default()
        })
        .collect();
    stores
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    let feedback = clicks("user_1", (0..10).map(|i| format!("item_{i}")));
    stores
        .data
        .batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender.recommend_latest(&ctx).await.expect("recommend");
    assert_eq!(digest, "latest");
    assert_eq!(scores.len(), 10);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 19 - i));
        assert_eq!(score_.score, (19 - i) as f64);
    }

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        vec!["cat_1".to_owned()],
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender.recommend_latest(&ctx).await.expect("recommend");
    assert_eq!(digest, "latest");
    assert_eq!(scores.len(), 5);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 18 - 2 * i));
        assert_eq!(score_.score, (18 - 2 * i) as f64);
    }
}

#[tokio::test]
async fn collaborative() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let recommends: Vec<Score> = (0..20)
        .map(|i| Score {
            id: format!("item_{i}"),
            score: i as f64,
            is_hidden: false,
            categories: if i % 2 == 0 {
                vec!["cat_1".to_owned()]
            } else {
                Vec::new()
            },
            timestamp: Time::default(),
        })
        .collect();
    stores
        .cache
        .add_scores(&ctx, cache::COLLABORATIVE_FILTERING, "user_1", &recommends)
        .await
        .expect("add the scores");
    stores
        .cache
        .set(
            &ctx,
            &[cache::string(
                cache::key(&[cache::COLLABORATIVE_FILTERING_DIGEST, "user_1"]),
                "digest",
            )],
        )
        .await
        .expect("set the digest");
    let feedback = clicks("user_1", (0..10).map(|i| format!("item_{i}")));
    stores
        .data
        .batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_collaborative(&ctx)
        .await
        .expect("recommend");
    assert_eq!(digest, "digest");
    assert_eq!(scores.len(), 10);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 19 - i));
        assert_eq!(score_.score, (19 - i) as f64);
    }

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        vec!["cat_1".to_owned()],
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_collaborative(&ctx)
        .await
        .expect("recommend");
    assert_eq!(digest, "digest");
    assert_eq!(scores.len(), 5);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 18 - 2 * i));
        assert_eq!(score_.score, (18 - 2 * i) as f64);
    }
}

#[tokio::test]
async fn non_personalized() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let recommends: Vec<Score> = (0..20)
        .map(|i| Score {
            id: format!("item_{i}"),
            score: i as f64,
            is_hidden: false,
            categories: if i % 2 == 0 {
                vec![String::new(), "cat_1".to_owned()]
            } else {
                vec![String::new()]
            },
            timestamp: Time::default(),
        })
        .collect();
    stores
        .cache
        .add_scores(&ctx, cache::NON_PERSONALIZED, "a", &recommends)
        .await
        .expect("add the scores");
    stores
        .cache
        .set(
            &ctx,
            &[cache::string(
                cache::key(&[cache::NON_PERSONALIZED_DIGEST, "a"]),
                "digest",
            )],
        )
        .await
        .expect("set the digest");
    let feedback = clicks("user_1", (0..10).map(|i| format!("item_{i}")));
    stores
        .data
        .batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_non_personalized(&ctx, "a")
        .await
        .expect("recommend");
    assert_eq!(digest, "digest");
    assert_eq!(scores.len(), 10);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 19 - i));
        assert_eq!(score_.score, (19 - i) as f64);
    }

    let recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        vec!["cat_1".to_owned()],
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_non_personalized(&ctx, "a")
        .await
        .expect("recommend");
    assert_eq!(digest, "digest");
    assert_eq!(scores.len(), 5);
    for (i, score_) in scores.iter().enumerate() {
        assert_eq!(score_.id, format!("item_{}", 18 - 2 * i));
        assert_eq!(score_.score, (18 - 2 * i) as f64);
    }
}

#[tokio::test]
async fn external() {
    let (url, _requests) = serve(|request| {
        let user_id = request
            .target
            .split_once('?')
            .and_then(|(_, query)| {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("user_id="))
            })
            .unwrap_or_default();
        if user_id == "user_1" {
            Reply::ok(
                "[\"item_1\", \"item_2\", \"item_3\", \"item_100\", \"item_200\", \"item_300\"]\n",
            )
        } else {
            Reply::not_found()
        }
    });
    let stores = open_stores().await;
    let ctx = Context::background();
    let feedback = clicks("user_1", (0..10).map(|i| format!("item_{i}")));
    stores
        .data
        .batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    let cfg = RecommendConfig {
        cache_size: 10,
        external: Some(vec![ExternalConfig {
            script: format!(r#"fetch("{url}?user_id=user_1").body"#),
            name: "test".to_owned(),
        }]),
        ..Default::default()
    };
    let recommender = Recommender::new(
        cfg.clone(),
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (scores, digest) = recommender
        .recommend_external(&ctx, "test")
        .await
        .expect("recommend");
    assert_eq!(digest, cfg.external.as_ref().expect("the config")[0].hash());
    assert_eq!(
        scores,
        vec![
            score("item_100", 0.0),
            score("item_200", 0.0),
            score("item_300", 0.0),
        ]
    );
}

#[tokio::test]
async fn user_to_user() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let items = [
        Item {
            item_id: "item_20".to_owned(),
            timestamp: gotime::now(),
            ..Default::default()
        },
        Item {
            item_id: "item_21".to_owned(),
            timestamp: gotime::now(),
            ..Default::default()
        },
        Item {
            item_id: "item_22".to_owned(),
            timestamp: gotime::now(),
            is_hidden: true,
            ..Default::default()
        },
        Item {
            item_id: "item_23".to_owned(),
            timestamp: gotime::now().add_date(0, 0, -10),
            ..Default::default()
        },
    ];
    stores
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    let feedback = clicks("user_2", items.iter().map(|item| item.item_id.clone()));
    stores
        .data
        .batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");

    let user_to_user = UserToUserConfig {
        name: "test".to_owned(),
        kind: "embedding".to_owned(),
        column: String::new(),
    };
    stores
        .vectors
        .add_collection(
            &ctx,
            &vectors::user_to_user_collection("test"),
            2,
            Distance::Euclidean,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    stores
        .vectors
        .add_vectors(
            &ctx,
            &vectors::user_to_user_collection("test"),
            &[
                Vector {
                    id: "user_1".to_owned(),
                    values: vec![0.0, 0.0],
                    ..Default::default()
                },
                Vector {
                    id: "user_2".to_owned(),
                    values: vec![1.0, 0.0],
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("add the vectors");

    let cfg = RecommendConfig {
        cache_size: 10,
        user_to_user: Some(vec![user_to_user.clone()]),
        data_source: DataSourceConfig {
            positive_feedback_types: Some(vec![click()]),
            item_ttl: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let recommender = Recommender::new(
        cfg.clone(),
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let (mut scores, digest) = recommender
        .recommend_user_to_user(&ctx, "test")
        .await
        .expect("recommend");
    assert_eq!(digest, user_to_user.hash(Some(&cfg)));
    scores.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(scores, vec![score("item_20", 0.5), score("item_21", 0.5)]);
}

#[tokio::test]
async fn unknown_recommender_is_an_error() {
    let err = parse("no-such/recommender").expect_err("an unknown name");
    assert_eq!(err.to_string(), "unknown recommender: no-such/recommender");
}

#[tokio::test]
async fn missing_item_to_item_config_is_not_found() {
    let stores = open_stores().await;
    let ctx = Context::background();
    let mut recommender = Recommender::new(
        RecommendConfig {
            cache_size: 10,
            ..Default::default()
        },
        stores.cache.clone(),
        stores.data.clone(),
        stores.vectors.clone(),
        true,
        "user_1".to_owned(),
        Vec::new(),
    )
    .await
    .expect("build the recommender");
    let err = recommender
        .recommend_item_to_item(&ctx, "missing")
        .await
        .expect_err("no such recommender");
    assert_eq!(
        err.to_string(),
        "item-to-item recommender missing not found"
    );
    let err = recommender
        .recommend_user_to_user(&ctx, "missing")
        .await
        .expect_err("no such recommender");
    assert_eq!(
        err.to_string(),
        "user-to-user recommender missing not found"
    );
}

/// The recommender holds only `Send` and `Sync` parts, so a server can share it
/// between tasks. This does not run anything: it fails to compile otherwise.
#[test]
fn recommender_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Recommender>();
}
