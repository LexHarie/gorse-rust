// Ports: worker/worker_test.go @ add21b5

//! The worker suite of `worker/worker_test.go`, one test per Go suite method,
//! and the top-level `TestWorker_Sync`. The Go suite runs against shared
//! SQLite and `xvec` stores; each test here opens its own (`testing::fixture`).
//!
//! Go's `Worker` embeds its `Pipeline`, so the tests run the pipeline's
//! methods on the pipeline the worker owns.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::State;
use axum::http::StatusCode;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration as GoDuration, HOUR, Time};
use gorse_common::reranker::MockServer;
use gorse_config::config::schema::{
    DecisionAPIConfig, ItemToItemConfig, NonPersonalizedConfig, RerankerAPIConfig, UserToUserConfig,
};
use gorse_logics::cf::MatrixFactorizationUsers;
use gorse_logics::reranker::{Decision, Reranker};
use gorse_model::ctr::model::FactorizationMachines;
use gorse_protocol::master_client::MasterClient;
use gorse_protocol::master_server::{Master, MasterServer};
use gorse_protocol::{Meta, NodeInfo, PushProgressRequest, PushProgressResponse};
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Item, User};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Endpoint, Server};
use tonic::{Request, Response, Status};

use crate::pipeline::{ItemCache, Pipeline};
use crate::testing::{
    ScriptedServer, default_config, expressions, feedback, feedback_at, fixture, hours_from_now,
    item, item_with, new_pipeline, no_database_pipeline, on_worker_thread, score, user,
};
use crate::worker::{HealthStatus, Worker, WorkerOptions, check_live, check_ready, pull_users};

/// Go's `pipeline.Recommend(ctx, users, nil)` on the pipeline.
async fn run_recommend(pipeline: &Arc<Pipeline>, users: Vec<User>) {
    Arc::clone(pipeline)
        .recommend(Context::background(), users, None)
        .await;
}

/// Go's `CacheClient.Get(ctx, cache.Key(name, id)).Time()`.
async fn read_time(pipeline: &Pipeline, name: &str, id: &str) -> Time {
    pipeline
        .cache()
        .get(&Context::background(), &cache::key(&[name, id]))
        .await
        .time()
        .expect("read the time")
}

/// Go's `CacheClient.SearchScores(ctx, cache.Recommend, id, categories, begin, end)`.
async fn search_recommend(
    pipeline: &Pipeline,
    id: &str,
    categories: &[&str],
    begin: i64,
    end: i64,
) -> Vec<Score> {
    let categories: Vec<String> = categories.iter().map(|c| (*c).to_owned()).collect();
    pipeline
        .cache()
        .search_scores(
            &Context::background(),
            cache::RECOMMEND,
            id,
            &categories,
            begin,
            end,
        )
        .await
        .expect("search the recommendations")
}

/// Go's `CacheClient.SearchScores` with `cache.Recommend`, categories, and the
/// whole range.
async fn all_recommends(pipeline: &Pipeline, id: &str, categories: &[&str]) -> Vec<Score> {
    search_recommend(pipeline, id, categories, 0, -1).await
}

/// The recommend-digest key of a user.
fn digest_key(id: &str) -> String {
    cache::key(&[cache::RECOMMEND_DIGEST, id])
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_users_keeps_the_users_of_this_worker() {
    let fx = fixture().await;
    let ctx = Context::background();
    // create user index
    fx.data
        .batch_insert_users(
            &ctx,
            &(1..=8).map(|i| user(&i.to_string())).collect::<Vec<_>>(),
        )
        .await
        .expect("insert the users");
    // create nodes
    let nodes = ["a", "b", "c"].map(str::to_owned);

    let mut users = pull_users(fx.data.as_ref(), &nodes, "b")
        .await
        .expect("pull the users");
    for user in &mut users {
        assert!(!user.update_at.is_zero());
        user.update_at = Time::default();
    }
    assert_eq!(users, vec![user("1"), user("3"), user("6")]);

    assert!(pull_users(fx.data.as_ref(), &nodes, "d").await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn check_recommend_cache_timeout() {
    let fx = fixture().await;
    let config = default_config();
    let pipeline = Arc::new(new_pipeline(&fx, config.clone()));
    let ctx = Context::background();

    // empty cache
    assert!(pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
    fx.cache
        .add_scores(&ctx, cache::RECOMMEND, "0", &[score("0", 0.0, &[""])])
        .await
        .expect("add the scores");

    // digest mismatch
    assert!(pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
    fx.cache
        .set(
            &ctx,
            &[cache::string(digest_key("0"), config.recommend.hash())],
        )
        .await
        .expect("set the digest");

    fx.cache
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, "0"]),
                hours_from_now(-1),
            )],
        )
        .await
        .expect("set the active time");
    assert!(pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
    fx.cache
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::RECOMMEND_UPDATE_TIME, "0"]),
                hours_from_now(-100),
            )],
        )
        .await
        .expect("set the update time");
    assert!(pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
    fx.cache
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::RECOMMEND_UPDATE_TIME, "0"]),
                hours_from_now(100),
            )],
        )
        .await
        .expect("set the update time");
    assert!(!pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
    fx.cache
        .delete_scores(
            &ctx,
            &[cache::RECOMMEND.to_owned()],
            &cache::ScoreCondition {
                subset: Some("0".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("delete the scores");
    assert!(pipeline.check_recommend_cache_out_of_date(&ctx, "0").await);
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_collaborative() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.recommenders = Some(vec!["collaborative".to_owned()]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    // insert feedbacks
    let now = gotime::now();
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[
                feedback_at("click", "0", "9", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "8", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "7", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "6", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "5", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "4", now.add(GoDuration(-HOUR.0))),
                feedback_at("click", "0", "3", now.add(HOUR)),
                feedback_at("click", "0", "2", now.add(HOUR)),
                feedback_at("click", "0", "1", now.add(HOUR)),
                feedback_at("click", "0", "0", now.add(HOUR)),
            ],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");

    // insert hidden items and categorized items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                Item {
                    is_hidden: true,
                    ..item("10")
                },
                Item {
                    is_hidden: true,
                    ..item("11")
                },
                item_with("3", &["*"], Time::default()),
                item_with("1", &["*"], Time::default()),
            ],
        )
        .await
        .expect("insert the items");

    // create mock model
    let mut mf_vectors = Vec::new();
    for i in 0..10 {
        mf_vectors.push(Vector {
            id: i.to_string(),
            values: vec![i as f32, 1.0],
            categories: if i == 1 || i == 3 {
                vec!["*".to_owned()]
            } else {
                Vec::new()
            },
            ..Default::default()
        });
    }
    fx.vectors
        .add_vectors(
            &ctx,
            &vectors::collaborative_filtering_collection(1),
            &mf_vectors,
        )
        .await
        .expect("add the vectors");
    pipeline.set_matrix_factorization(1, Some(mf_users(&[("0", vec![1.0, 0.0])])));
    run_recommend(&pipeline, vec![user("0")]).await;

    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;

    let recommends = all_recommends(&pipeline, "0", &[]).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("3", 3.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("2", 2.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("1", 1.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("0", 0.0, &[])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_cold_start() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["click"]));
    let mut pipeline = new_pipeline(&fx, config);
    pipeline.dont_skip_cold_start_users = false;
    pipeline.set_matrix_factorization(0, None);
    let pipeline = Arc::new(pipeline);

    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("0", &[], gotime::unix(1, 0)),
                item_with("1", &[], gotime::unix(2, 0)),
            ],
        )
        .await
        .expect("insert the items");
    fx.data
        .batch_insert_feedback(&ctx, &[feedback("click", "0", "0")], true, true, true)
        .await
        .expect("insert the feedback");

    run_recommend(&pipeline, vec![user("0")]).await;
    let recommends = all_recommends(&pipeline, "0", &[]).await;
    assert!(!recommends.is_empty());
    assert_eq!(recommends[0].id, "1");
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_item_to_item() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.recommenders = Some(vec!["item-to-item/default".to_owned()]);
    config.recommend.item_to_item = Some(vec![ItemToItemConfig {
        name: "default".to_owned(),
        ..Default::default()
    }]);
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["a"]));
    // insert feedback
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[
                feedback("a", "0", "21"),
                feedback("a", "0", "22"),
                feedback("a", "0", "23"),
                feedback("a", "0", "24"),
            ],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");

    // insert item-to-item vectors
    config.recommend.item_to_item = Some(vec![ItemToItemConfig {
        name: "default".to_owned(),
        kind: "tags".to_owned(),
        column: "item.Labels".to_owned(),
    }]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    let collection = vectors::item_to_item_collection("default");
    fx.vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse("21", &[0], &[1.0]),
                sparse("22", &[1], &[1.0]),
                sparse("23", &[2], &[1.0]),
                sparse("24", &[3], &[1.0]),
                Vector {
                    is_hidden: true,
                    ..sparse("25", &[0, 1, 2, 3], &[100.0, 100.0, 100.0, 100.0])
                },
                Vector {
                    categories: vec!["*".to_owned()],
                    ..sparse("26", &[3], &[1.0])
                },
                sparse("27", &[2, 3], &[1.0, 1.0]),
                Vector {
                    categories: vec!["*".to_owned()],
                    ..sparse("28", &[1, 2, 3], &[1.0, 1.0, 1.0])
                },
                sparse("29", &[0, 1, 2, 3], &[1.0, 1.0, 1.0, 1.0]),
            ],
        )
        .await
        .expect("add the vectors");

    // insert items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item("21"),
                item("22"),
                item("23"),
                item("24"),
                item("25"),
                item("26"),
                item("27"),
                item("28"),
                item("29"),
            ],
        )
        .await
        .expect("insert the items");
    // insert hidden items
    fx.data
        .batch_insert_items(
            &ctx,
            &[Item {
                is_hidden: true,
                ..item("25")
            }],
        )
        .await
        .expect("insert the hidden item");
    // insert categorized items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("26", &["*"], Time::default()),
                item_with("28", &["*"], Time::default()),
            ],
        )
        .await
        .expect("insert the categorized items");
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("29", 4.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("28", 3.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("27", 2.0, &[])
            },
        ]
    );
    let recommends = search_recommend(&pipeline, "0", &["*"], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("28", 3.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("26", 1.0, &["*"])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_user_to_user() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.recommenders = Some(vec!["user-to-user/default".to_owned()]);
    config.recommend.user_to_user = Some(vec![UserToUserConfig {
        name: "default".to_owned(),
        kind: "items".to_owned(),
        ..Default::default()
    }]);
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["a"]));
    let pipeline = Arc::new(new_pipeline(&fx, config));
    // insert similar users
    let collection = vectors::user_to_user_collection("default");
    fx.vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse("0", &[0], &[1.0]),
                sparse("1", &[0], &[2.0]),
                sparse("2", &[0], &[1.5]),
                sparse("3", &[0], &[1.0]),
            ],
        )
        .await
        .expect("add the vectors");
    // insert feedback
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[feedback("a", "1", "10"), feedback("a", "1", "11")],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[
                feedback("a", "2", "10"),
                feedback("a", "2", "12"),
                feedback("a", "2", "48"),
            ],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[
                feedback("a", "3", "10"),
                feedback("a", "3", "13"),
                feedback("a", "3", "48"),
            ],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");
    // insert hidden items
    fx.data
        .batch_insert_items(
            &ctx,
            &[Item {
                is_hidden: true,
                ..item("10")
            }],
        )
        .await
        .expect("insert the hidden item");
    // insert categorized items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("12", &["*"], Time::default()),
                item_with("48", &["*"], Time::default()),
            ],
        )
        .await
        .expect("insert the categorized items");
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("48", 2.5, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("11", 2.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("12", 1.5, &["*"])
            },
        ]
    );
    let recommends = search_recommend(&pipeline, "0", &["*"], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("48", 2.5, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("12", 1.5, &["*"])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_latest() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.recommenders = Some(vec!["latest".to_owned()]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    // insert items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("21", &[], gotime::unix(21, 0)),
                item_with("20", &[], gotime::unix(20, 0)),
                item_with("19", &[], gotime::unix(19, 0)),
                item_with("18", &[], gotime::unix(18, 0)),
                item_with("10", &["*"], gotime::unix(10, 0)),
                item_with("9", &["*"], gotime::unix(9, 0)),
                item_with("8", &["*"], gotime::unix(8, 0)),
            ],
        )
        .await
        .expect("insert the items");
    // insert hidden items
    fx.data
        .batch_insert_items(
            &ctx,
            &[Item {
                is_hidden: true,
                ..item_with("21", &[], gotime::unix(21, 0))
            }],
        )
        .await
        .expect("insert the hidden item");
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("20", 20.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("19", 19.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("18", 18.0, &[])
            },
        ]
    );
    let recommends = all_recommends(&pipeline, "0", &["*"]).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("10", 10.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("9", 9.0, &["*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("8", 8.0, &["*"])
            },
        ]
    );
    // read recommend digest
    let digest = fx
        .cache
        .get(&ctx, &digest_key("0"))
        .await
        .string()
        .expect("read the digest");
    assert_eq!(digest, gorse_common::util::md5(&["latest"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_non_personalized() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.recommenders = Some(vec!["non-personalized/popular".to_owned()]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    // insert items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item("11"),
                item("10"),
                item("9"),
                item("8"),
                item_with("20", &["*"], Time::default()),
                item_with("19", &["*"], Time::default()),
                item_with("18", &["*"], Time::default()),
            ],
        )
        .await
        .expect("insert the items");
    // insert non-personalized recommendation
    fx.cache
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "popular",
            &[
                score("11", 31.0, &[""]),
                score("10", 30.0, &[""]),
                score("9", 29.0, &[""]),
                score("8", 28.0, &[""]),
                score("20", 20.0, &["", "*"]),
                score("19", 19.0, &["", "*"]),
                score("18", 18.0, &["", "*"]),
            ],
        )
        .await
        .expect("add the scores");
    // insert hidden items
    fx.data
        .batch_insert_items(
            &ctx,
            &[Item {
                is_hidden: true,
                ..item("11")
            }],
        )
        .await
        .expect("insert the hidden item");
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("10", 30.0, &[""])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("9", 29.0, &[""])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("8", 28.0, &[""])
            },
        ]
    );
    let recommends = all_recommends(&pipeline, "0", &["*"]).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("20", 20.0, &["", "*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("19", 19.0, &["", "*"])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("18", 18.0, &["", "*"])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.kind = "fm".to_owned();
    config.recommend.ranker.recommenders = None;
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["a"]));
    config.recommend.cache_size = 1;
    config.recommend.non_personalized = Some(vec![NonPersonalizedConfig {
        name: "popular".to_owned(),
        ..Default::default()
    }]);
    config.recommend.item_to_item = Some(vec![ItemToItemConfig {
        name: "default".to_owned(),
        ..Default::default()
    }]);
    config.recommend.user_to_user = Some(vec![UserToUserConfig {
        name: "default".to_owned(),
        kind: "items".to_owned(),
        ..Default::default()
    }]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    fx.vectors
        .add_vectors(
            &ctx,
            &vectors::collaborative_filtering_collection(1),
            &[Vector {
                id: "4".to_owned(),
                values: vec![4.0, 1.0],
                ..Default::default()
            }],
        )
        .await
        .expect("add the vectors");
    pipeline.set_matrix_factorization(1, Some(mf_users(&[("0", vec![1.0, 0.0])])));
    pipeline.set_click_through_rate_model(Some(Arc::new(MockFactorizationMachine)));

    // insert items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("0", &[], gotime::unix(0, 0)),
                item_with("1", &[], gotime::unix(1, 0)),
                item_with("2", &[], gotime::unix(2, 0)),
                item_with("3", &[], gotime::unix(3, 0)),
                item_with("4", &[], gotime::unix(4, 0)),
                item_with("5", &["a"], gotime::unix(5, 0)),
            ],
        )
        .await
        .expect("insert the items");

    // insert feedback
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[feedback("a", "0", "0"), feedback("a", "1", "1")],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");

    // insert stale recommendation
    fx.cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[Score {
                id: "999".to_owned(),
                score: 999.0,
                ..score("", 0.0, &[])
            }],
        )
        .await
        .expect("add the stale recommendation");

    // insert non-personalized recommendation
    fx.cache
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "popular",
            &[score("3", 0.0, &[""])],
        )
        .await
        .expect("add the non-personalized recommendation");

    // insert item-to-item vectors
    let item_collection = vectors::item_to_item_collection("default");
    fx.vectors
        .add_collection(
            &ctx,
            &item_collection,
            0,
            Distance::Dot,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &item_collection,
            &[sparse("0", &[0], &[1.0]), sparse("2", &[0], &[1.0])],
        )
        .await
        .expect("add the vectors");

    // insert user-to-user vectors
    let user_collection = vectors::user_to_user_collection("default");
    fx.vectors
        .add_collection(
            &ctx,
            &user_collection,
            0,
            Distance::Dot,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &user_collection,
            &[sparse("0", &[0], &[1.0]), sparse("1", &[0], &[1.0])],
        )
        .await
        .expect("add the vectors");

    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 5).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                categories: vec!["a".to_owned()],
                ..score("5", 5.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("4", 4.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("3", 3.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("2", 2.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("1", 1.0, &[])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recommend_ranker_none() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.ranker.kind = "none".to_owned();
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["a"]));
    config.recommend.cache_size = 1;
    config.recommend.non_personalized = Some(vec![NonPersonalizedConfig {
        name: "popular".to_owned(),
        ..Default::default()
    }]);
    config.recommend.item_to_item = Some(vec![ItemToItemConfig {
        name: "default".to_owned(),
        ..Default::default()
    }]);
    config.recommend.user_to_user = Some(vec![UserToUserConfig {
        name: "default".to_owned(),
        kind: "items".to_owned(),
        ..Default::default()
    }]);
    let pipeline = Arc::new(new_pipeline(&fx, config));
    fx.vectors
        .add_vectors(
            &ctx,
            &vectors::collaborative_filtering_collection(1),
            &[Vector {
                id: "4".to_owned(),
                values: vec![4.0, 1.0],
                ..Default::default()
            }],
        )
        .await
        .expect("add the vectors");
    pipeline.set_matrix_factorization(1, Some(mf_users(&[("0", vec![1.0, 0.0])])));
    pipeline.set_click_through_rate_model(Some(Arc::new(MockFactorizationMachine)));

    // insert items
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("0", &[], gotime::unix(0, 0)),
                item_with("1", &[], gotime::unix(1, 0)),
                item_with("2", &[], gotime::unix(2, 0)),
                item_with("3", &[], gotime::unix(3, 0)),
                item_with("4", &[], gotime::unix(4, 0)),
                item_with("5", &["a"], gotime::unix(5, 0)),
            ],
        )
        .await
        .expect("insert the items");

    // insert feedback
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[feedback("a", "0", "0"), feedback("a", "1", "1")],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");

    // insert non-personalized recommendation
    fx.cache
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "popular",
            &[score("3", 0.0, &[""])],
        )
        .await
        .expect("add the non-personalized recommendation");

    // insert item-to-item vectors
    let item_collection = vectors::item_to_item_collection("default");
    fx.vectors
        .add_collection(
            &ctx,
            &item_collection,
            0,
            Distance::Dot,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &item_collection,
            &[sparse("0", &[0], &[1.0]), sparse("2", &[0], &[1.0])],
        )
        .await
        .expect("add the vectors");

    // insert user-to-user vectors
    let user_collection = vectors::user_to_user_collection("default");
    fx.vectors
        .add_collection(
            &ctx,
            &user_collection,
            0,
            Distance::Dot,
            VectorConfig::default(),
        )
        .await
        .expect("add the collection");
    fx.vectors
        .add_vectors(
            &ctx,
            &user_collection,
            &[sparse("0", &[0], &[1.0]), sparse("1", &[0], &[1.0])],
        )
        .await
        .expect("add the vectors");

    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 5).await;
    let unranked = [
        Score {
            timestamp: recommend_time.clone(),
            categories: vec!["a".to_owned()],
            ..score("5", 5.0, &[])
        },
        Score {
            timestamp: recommend_time.clone(),
            ..score("4", 4.0, &[])
        },
        Score {
            timestamp: recommend_time.clone(),
            ..score("3", 3.0, &[])
        },
        Score {
            timestamp: recommend_time.clone(),
            ..score("2", 2.0, &[])
        },
        Score {
            timestamp: recommend_time.clone(),
            ..score("1", 1.0, &[])
        },
    ];
    assert_ne!(recommends, unranked.to_vec());
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_sync() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut cfg = gorse_config::config::get_default_config();
    cfg.database.data_store = format!("sqlite://{}/data.db", dir.path().display());
    cfg.database.cache_store = format!("sqlite://{}/cache.db", dir.path().display());
    let meta = Meta {
        config: cfg.marshal_json().expect("marshal the config"),
        click_through_rate_model_id: 1,
        collaborative_filtering_model_id: 2,
        ..Default::default()
    };
    let master = start_mock_master(meta).await;
    let channel = Endpoint::from_shared(format!("http://{}", master.addr))
        .expect("the master address")
        .connect()
        .await
        .expect("connect to the mock master");
    let worker = Worker::new(sync_options()).expect("create the worker");
    {
        let mut state = worker.state.lock().expect("the worker state");
        state.master = Some(MasterClient::new(channel.clone()));
        state.conn = Some(channel);
    }

    worker.sync_once().await;
    {
        let state = worker.state.lock().expect("the worker state");
        assert_eq!(state.data_path, cfg.database.data_store);
        assert_eq!(state.cache_path, cfg.database.cache_store);
        assert_eq!(state.latest_click_through_rate_model_id, 1);
        assert_eq!(state.latest_collaborative_filtering_model_id, 2);
        assert_eq!(state.click_through_rate_model_id, 0);
    }
    assert_eq!(worker.pipeline().get_matrix_factorization_id(), 0);
    worker
        .pipeline()
        .data()
        .close()
        .await
        .expect("close the data store");
    worker
        .pipeline()
        .cache()
        .close()
        .await
        .expect("close the cache store");
    master.stop();
}

/// A feedback-less pipeline's click-through rate model that predicts an item
/// by its id, as Go's `mockFactorizationMachine` does.
#[derive(Debug)]
struct MockFactorizationMachine;

impl gorse_model_core::model::Model for MockFactorizationMachine {
    fn set_params(&mut self, _params: gorse_model_core::params::Params) {}

    fn get_params(&self) -> &gorse_model_core::params::Params {
        panic!("implement me")
    }

    fn suggest_params(
        &self,
        _trial: &mut dyn gorse_model_core::model::SuggestTrial,
    ) -> gorse_model_core::params::Params {
        panic!("implement me")
    }

    fn clear(&mut self) {
        panic!("implement me")
    }

    fn invalid(&self) -> bool {
        false
    }
}

impl FactorizationMachines for MockFactorizationMachine {
    fn header(&self) -> &'static str {
        panic!("implement me")
    }

    fn predict(
        &self,
        _user_id: &str,
        item_id: &str,
        _user_features: &[gorse_model::ctr::data::Label],
        _item_features: &[gorse_model::ctr::data::Label],
    ) -> f32 {
        let score: i64 = item_id.parse().expect("an item id that is a number");
        score as f32
    }

    fn internal_predict(&self, _x: &[i32], _values: &[f32]) -> f32 {
        panic!("implement me")
    }

    fn fit(
        &mut self,
        _ctx: &Context,
        _train_set: &dyn gorse_dataset::dataset::CtrSplit,
        _test_set: &dyn gorse_dataset::dataset::CtrSplit,
        _config: &gorse_model::ctr::model::FitConfig,
    ) -> gorse_model::ctr::model::Score {
        panic!("implement me")
    }

    fn marshal<W: std::io::Write>(&self, _w: &mut W) -> Result<(), gorse_model::ctr::model::Error>
    where
        Self: Sized,
    {
        panic!("implement me")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rank_by_click_through_rate() {
    let fx = fixture().await;
    let ctx = Context::background();
    let pipeline = Arc::new(new_pipeline(&fx, default_config()));
    // insert a user
    fx.data
        .batch_insert_users(&ctx, &[user("1")])
        .await
        .expect("insert the user");
    // insert items
    fx.data
        .batch_insert_items(&ctx, &["1", "2", "3", "4", "5"].map(item))
        .await
        .expect("insert the items");
    // rank items
    let item_cache = ItemCache::new(fx.data.clone());
    let result = pipeline
        .rank_by_click_through_rate(
            &ctx,
            &MockFactorizationMachine,
            &user("1"),
            &["1", "2", "3", "4", "5"].map(|id| score(id, 0.0, &[])),
            &item_cache,
            gotime::now(),
        )
        .await
        .expect("rank the items");
    let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["5", "4", "3", "2", "1"]);
    assert!(result.windows(2).all(|pair| pair[0].score >= pair[1].score));
}

#[tokio::test(flavor = "multi_thread")]
async fn rank_by_reranker() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mock_ai = Arc::new(MockServer::new());
    let server = Arc::clone(&mock_ai);
    std::thread::spawn(move || {
        let _ = server.start();
    });
    mock_ai.ready();

    // insert a user
    fx.data
        .batch_insert_users(&ctx, &[user("u1")])
        .await
        .expect("insert the user");
    // insert items used by candidates and feedback
    fx.data
        .batch_insert_items(&ctx, &["1", "2", "3", "4", "5"].map(item))
        .await
        .expect("insert the items");

    let reranker_config = RerankerAPIConfig {
        url: mock_ai.url(),
        auth_token: mock_ai.auth_token().to_owned(),
        model: "v1".to_owned(),
        query_template: "{{user.UserId}}".to_owned(),
        document_template: "{{item.ItemId}}".to_owned(),
    };
    let ranker = Reranker::new(&reranker_config).expect("create the reranker");

    let mut config = default_config();
    config.recommend.ranker.reranker_api = reranker_config;
    let pipeline = Arc::new(new_pipeline(&fx, config));
    let item_cache = ItemCache::new(fx.data.clone());
    let recommend_time = gotime::now();
    let candidates = ["1", "2", "3"].map(|id| score(id, 0.0, &[]));
    let feedbacks = vec![feedback("like", "u1", "4"), feedback("like", "u1", "5")];
    let rank_time = recommend_time.clone();
    let result = on_worker_thread(async move {
        pipeline
            .rank_by_reranker(
                &ctx,
                None,
                &ranker,
                &user("u1"),
                &feedbacks,
                &candidates,
                &item_cache,
                rank_time,
            )
            .await
    })
    .await
    .expect("rank the items");
    let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "3"]);
    let scores: Vec<f64> = result.iter().map(|s| s.score).collect();
    assert_eq!(scores, [1.0, 0.5, 1.0 / 3.0]);
    for scored in &result {
        assert_eq!(scored.timestamp, recommend_time);
    }
    let _ = mock_ai.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn rank_by_decision() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.context_size = 1;
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["like"]));
    let server = ScriptedServer::start(
        r#"{"answers":{"1":{"type":"noul","noul":0.15},"2":{"type":"noul","noul":0.85}}}"#,
    );
    fx.data
        .batch_insert_items(
            &ctx,
            &[
                item_with("1", &["a"], Time::default()),
                item_with("2", &["b"], Time::default()),
                Item {
                    is_hidden: true,
                    ..item("3")
                },
                item("4"),
                item("5"),
            ],
        )
        .await
        .expect("insert the items");

    let ranker = Decision::new(&DecisionAPIConfig {
        url: server.url.clone(),
        state_template: "{{user.UserId}}: {% for f in feedback %}{{f.ItemId}}{% endfor %}"
            .to_owned(),
        instructions_template: "{{item.ItemId}}".to_owned(),
        ..Default::default()
    })
    .expect("create the decision ranker");
    let now = gotime::now();
    let pipeline = Arc::new(new_pipeline(&fx, config));
    let item_cache = ItemCache::new(fx.data.clone());
    let feedbacks = vec![
        feedback_at("like", "u1", "4", now.add(GoDuration(-HOUR.0))),
        feedback_at("like", "u1", "5", now.clone()),
        feedback_at("read", "u1", "1", now.add(HOUR)),
    ];
    let candidates = ["1", "2", "3", "missing"].map(|id| score(id, 0.0, &[]));
    let rank_time = now.clone();
    let result = on_worker_thread(async move {
        pipeline
            .rank_by_decision(
                &ctx,
                None,
                &ranker,
                &user("u1"),
                &feedbacks,
                &candidates,
                &item_cache,
                rank_time,
            )
            .await
    })
    .await
    .expect("rank the items");
    assert_eq!(
        result,
        vec![
            Score {
                timestamp: now.clone(),
                ..score("2", 0.85, &["b"])
            },
            Score {
                timestamp: now.clone(),
                ..score("1", 0.15, &["a"])
            },
        ]
    );
    let request = server
        .received
        .recv()
        .expect("the decision API received a request");
    let request: serde_json::Value =
        serde_json::from_str(&request.body).expect("the request is JSON");
    assert_eq!(request["state"], "u1: 5");
    assert_eq!(
        request["questions"].as_object().expect("questions").len(),
        2
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    config.recommend.data_source.positive_feedback_types = Some(expressions(&["p"]));
    config.recommend.data_source.read_feedback_types = Some(expressions(&["n"]));
    config.recommend.ranker.kind = "fm".to_owned();
    config.recommend.ranker.recommenders = Some(vec!["collaborative".to_owned()]);
    config.recommend.replacement.enable_replacement = true;
    config.recommend.replacement.positive_replacement_decay = 0.8;
    config.recommend.replacement.read_replacement_decay = 0.7;
    let pipeline = Arc::new(new_pipeline(&fx, config.clone()));
    pipeline.set_click_through_rate_model(Some(Arc::new(MockFactorizationMachine)));

    // 1. Insert historical items into empty recommendation.
    // insert items
    fx.data
        .batch_insert_items(&ctx, &["10", "9", "8", "7", "6", "5"].map(item))
        .await
        .expect("insert the items");
    // insert feedback
    fx.data
        .batch_insert_feedback(
            &ctx,
            &[
                feedback("p", "0", "10"),
                feedback("n", "0", "9"),
                feedback("i", "0", "8"),
            ],
            true,
            false,
            true,
        )
        .await
        .expect("insert the feedback");
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("10", 8.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("9", 6.3, &[])
            },
        ]
    );

    // 2. Insert historical items into non-empty recommendation.
    let mut next = config;
    next.recommend.cache_expire = GoDuration(0);
    next.recommend.ranker.recommenders = Some(vec!["latest".to_owned()]);
    pipeline.set_config(next);
    run_recommend(&pipeline, vec![user("0")]).await;
    // read recommend time
    let recommend_time = read_time(&pipeline, cache::RECOMMEND_UPDATE_TIME, "0").await;
    // read recommend result
    let recommends = search_recommend(&pipeline, "0", &[], 0, 3).await;
    assert_eq!(
        recommends,
        vec![
            Score {
                timestamp: recommend_time.clone(),
                ..score("10", 8.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("7", 7.0, &[])
            },
            Score {
                timestamp: recommend_time.clone(),
                ..score("9", 6.3, &[])
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn user_activity() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut config = default_config();
    let pipeline = Arc::new(new_pipeline(&fx, config.clone()));
    fx.cache
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, "0"]),
                gotime::now().add(GoDuration(-24 * HOUR.0)),
            )],
        )
        .await
        .expect("set the active time");
    fx.cache
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, "1"]),
                gotime::now().add(GoDuration(-240 * HOUR.0)),
            )],
        )
        .await
        .expect("set the active time");
    for (id, item_id) in [("0", "0"), ("1", "1"), ("2", "2")] {
        fx.cache
            .add_scores(&ctx, cache::RECOMMEND, id, &[score(item_id, 1.0, &[""])])
            .await
            .expect("add the scores");
    }

    assert!(pipeline.check_user_active_time(&ctx, "0").await);
    assert!(pipeline.check_user_active_time(&ctx, "1").await);
    assert!(pipeline.check_user_active_time(&ctx, "2").await);
    for id in ["0", "1", "2"] {
        assert!(!all_recommends_in(&fx, id).await.is_empty());
    }

    config.recommend.active_user_ttl = 5;
    pipeline.set_config(config);
    assert!(pipeline.check_user_active_time(&ctx, "0").await);
    assert!(!pipeline.check_user_active_time(&ctx, "1").await);
    assert!(pipeline.check_user_active_time(&ctx, "2").await);
    assert!(!all_recommends_in(&fx, "0").await.is_empty());
    assert!(all_recommends_in(&fx, "1").await.is_empty());
    assert!(!all_recommends_in(&fx, "2").await.is_empty());
}

/// The recommend cache rows of a user, read straight from the fixture's cache.
async fn all_recommends_in(fx: &crate::testing::Fixture, id: &str) -> Vec<Score> {
    fx.cache
        .search_scores(
            &Context::background(),
            cache::RECOMMEND,
            id,
            &[String::new()],
            0,
            1,
        )
        .await
        .expect("search the recommendations")
}

#[tokio::test(flavor = "multi_thread")]
async fn health() {
    let fx = fixture().await;
    let pipeline = Arc::new(new_pipeline(&fx, default_config()));
    let response = check_live(State(Arc::clone(&pipeline))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_text(response).await,
        r#"{"Ready":true,"DataStoreError":null,"CacheStoreError":null,"DataStoreConnected":true,"CacheStoreConnected":true}"#
    );

    let response = check_ready(State(Arc::clone(&pipeline))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_text(response).await,
        r#"{"Ready":true,"DataStoreError":null,"CacheStoreError":null,"DataStoreConnected":true,"CacheStoreConnected":true}"#
    );

    let no_database = no_database_pipeline();
    let response = check_live(State(Arc::clone(&no_database))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_text(response).await,
        r#"{"Ready":false,"DataStoreError":{},"CacheStoreError":{},"DataStoreConnected":false,"CacheStoreConnected":false}"#
    );

    let response = check_ready(State(no_database)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    // The status struct keeps Go's field names and layout.
    let status = HealthStatus {
        ready: false,
        data_store_error: Some(Default::default()),
        cache_store_error: Some(Default::default()),
        data_store_connected: false,
        cache_store_connected: false,
    };
    assert_eq!(
        serde_json::to_string(&status).expect("marshal"),
        r#"{"Ready":false,"DataStoreError":{},"CacheStoreError":{},"DataStoreConnected":false,"CacheStoreConnected":false}"#
    );
}

/// The body of a response as text.
async fn body_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read the body");
    String::from_utf8(bytes.to_vec()).expect("the body is UTF-8")
}

/// A sparse vector with the given indices and values.
fn sparse(id: &str, indices: &[u32], values: &[f32]) -> Vector {
    Vector {
        id: id.to_owned(),
        indices: indices.to_vec(),
        values: values.to_vec(),
        ..Default::default()
    }
}

/// The matrix factorisation users of the given ids and embeddings.
fn mf_users(users: &[(&str, Vec<f32>)]) -> MatrixFactorizationUsers {
    let mut out = MatrixFactorizationUsers::new();
    for (id, embedding) in users {
        out.add(id, embedding.clone());
    }
    out
}

/// The options of a worker that talks to a mock master on a local port.
fn sync_options() -> WorkerOptions {
    WorkerOptions {
        master_host: "127.0.0.1".to_owned(),
        master_port: 0,
        http_host: "127.0.0.1".to_owned(),
        http_port: 0,
        jobs: 1,
        tls: None,
        tick_interval: std::time::Duration::from_secs(60),
    }
}

/// A mock master that answers `GetMeta` with a fixed meta and accepts progress.
#[derive(Clone)]
struct MockMaster {
    meta: Meta,
}

#[tonic::async_trait]
impl Master for MockMaster {
    async fn get_meta(&self, _request: Request<NodeInfo>) -> Result<Response<Meta>, Status> {
        Ok(Response::new(self.meta.clone()))
    }

    async fn push_progress(
        &self,
        _request: Request<PushProgressRequest>,
    ) -> Result<Response<PushProgressResponse>, Status> {
        Ok(Response::new(PushProgressResponse::default()))
    }
}

/// A running mock master.
struct RunningMaster {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl RunningMaster {
    /// Stops the server.
    fn stop(self) {
        self.task.abort();
    }
}

/// Starts a mock master on a free local port.
async fn start_mock_master(meta: Meta) -> RunningMaster {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let addr = listener.local_addr().expect("local address");
    let service = MasterServer::new(MockMaster { meta });
    let task = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(service)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await;
    });
    RunningMaster { addr, task }
}
