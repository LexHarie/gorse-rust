// Ports: server/rest_test.go (TestItemToItem, TestNonPersonalizedRecommend,
// TestUserToUser, TestGetRecommends and its variants, TestSessionRecommend) @
// add21b5

//! The recommendation routes, as Go's recommend tests. The expected bodies are
//! the JSON Go writes for each `cache.Score` list, and the `Last-Modified`
//! header is checked where Go checks it.

use axum::http::StatusCode;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use serde_json::{Value, json};

use super::{Suite, delete, get, patch, post, put};

/// Go's `cache.Score` with only the id and the score, as its JSON encoding
/// writes it.
fn score_json(item_id: &str, score: f64) -> Value {
    json!({"Id": item_id, "Score": score})
}

/// A `Score` to store, with the categories Go's test gives it.
fn document(item_id: &str, score: f64, categories: &[&str]) -> Score {
    Score {
        id: item_id.to_owned(),
        score,
        is_hidden: false,
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        timestamp: gotime::Time::default(),
    }
}

#[tokio::test]
async fn item_to_item() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| {
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            kind: "embedding".to_owned(),
            column: "item.Labels.embedding".to_owned(),
            ..Default::default()
        }]);
    });
    let now = gotime::unix(0, 0);
    let embedding = |values: &[f64]| -> Labels {
        serde_json::from_value(json!({ "embedding": values })).expect("labels")
    };
    suite
        .data
        .batch_insert_items(
            &ctx,
            &[
                Item {
                    item_id: "source".to_owned(),
                    labels: embedding(&[0.0, 0.0]),
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Item {
                    item_id: "near".to_owned(),
                    labels: embedding(&[0.1, 0.0]),
                    categories: Some(vec!["movie".to_owned(), "drama".to_owned()]),
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Item {
                    item_id: "far".to_owned(),
                    labels: embedding(&[10.0, 0.0]),
                    categories: Some(vec!["movie".to_owned()]),
                    timestamp: now.clone(),
                    ..Default::default()
                },
                Item {
                    item_id: "hidden".to_owned(),
                    labels: embedding(&[0.05, 0.0]),
                    categories: Some(vec!["movie".to_owned()]),
                    is_hidden: true,
                    timestamp: now.clone(),
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("insert the items");
    let collection = vectors::item_to_item_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 2, Distance::Euclidean, VectorConfig::default())
        .await
        .expect("add the collection");
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
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
                    categories: vec!["movie".to_owned(), "drama".to_owned()],
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

    let expected = json!([
        score_json("near", 0.990103795941627),
        score_json("far", 0.009900990099009901),
    ]);
    let expected_all_categories = json!([score_json("near", 0.990103795941627)]);
    suite
        .send(get("/api/item/source/neighbors?category=movie").key())
        .await
        .status(StatusCode::OK)
        .json(expected.clone());
    suite
        .send(get("/api/item-to-item/default/source?category=movie").key())
        .await
        .status(StatusCode::OK)
        .json(expected.clone());
    suite
        .send(get("/api/item/source/neighbors?category=movie&category=drama").key())
        .await
        .status(StatusCode::OK)
        .json(expected_all_categories.clone());
    suite
        .send(get("/api/item-to-item/default/source?category=movie&category=drama").key())
        .await
        .status(StatusCode::OK)
        .json(expected_all_categories);
}

#[tokio::test]
async fn non_personalized_recommend() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| {
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            ..Default::default()
        }]);
    });
    // (name, collection, subset, category, url): Go's `ListOperator`. The user
    // neighbours are left out, as in Go, which does not hide users yet.
    let operators = [
        ("NonPersonalized", cache::NON_PERSONALIZED, "trending", "", "/api/non-personalized/trending"),
        ("NonPersonalizedCategory", cache::NON_PERSONALIZED, "trending", "0", "/api/non-personalized/trending"),
        ("CollaborativeFiltering", cache::RECOMMEND, "0", "", "/api/collaborative-filtering/0"),
        ("CollaborativeFilteringCategory", cache::RECOMMEND, "0", "0", "/api/collaborative-filtering/0/0"),
    ];
    let last_modified = gotime::now();

    for (i, (_name, collection, subset, category, url)) in operators.into_iter().enumerate() {
        let prefix = i.to_string();
        // insert documents
        let documents = vec![
            document(&format!("{prefix}0"), 100.0, &[category]),
            document(&format!("{prefix}1"), 99.0, &[category]),
            document(&format!("{prefix}2"), 98.0, &[category]),
            document(&format!("{prefix}3"), 97.0, &[category]),
            document(&format!("{prefix}4"), 96.0, &[category]),
        ];
        suite
            .cache
            .add_scores(&ctx, collection, subset, &documents)
            .await
            .expect("add the scores");
        suite
            .cache
            .set(
                &ctx,
                &[cache::time(
                    cache::key(&[&format!("{collection}_update_time"), subset]),
                    last_modified.clone(),
                )],
            )
            .await
            .expect("set the update time");
        // hidden item
        suite
            .send(
                patch(&format!("/api/item/{prefix}3"))
                    .key()
                    .json(&json!({"IsHidden": true, "Categories": null, "Timestamp": null, "Labels": null, "Comment": null})),
            )
            .await
            .status(StatusCode::OK);
        // insert read feedback
        suite
            .data
            .batch_insert_feedback(
                &ctx,
                &[Feedback {
                    feedback_key: FeedbackKey {
                        feedback_type: "read".to_owned(),
                        user_id: "0".to_owned(),
                        item_id: format!("{prefix}1"),
                    },
                    timestamp: gotime::now().add(gotime::Duration(-3_600_000_000_000)),
                    ..Default::default()
                }],
                true,
                true,
                true,
            )
            .await
            .expect("insert the feedback");

        let all = json!([
            score_json(&documents[0].id, 100.0),
            score_json(&documents[1].id, 99.0),
            score_json(&documents[2].id, 98.0),
            score_json(&documents[4].id, 96.0),
        ]);
        let base = format!("{url}?category={category}");
        suite
            .send(get(&base).key())
            .await
            .status(StatusCode::OK)
            .header_present("Last-Modified")
            .json(all.clone());
        suite
            .send(get(&format!("{base}&offset=0&n=3")).key())
            .await
            .status(StatusCode::OK)
            .header_present("Last-Modified")
            .json(json!([
                score_json(&documents[0].id, 100.0),
                score_json(&documents[1].id, 99.0),
                score_json(&documents[2].id, 98.0),
            ]));
        suite
            .send(get(&format!("{base}&offset=1&n=3")).key())
            .await
            .status(StatusCode::OK)
            .header_present("Last-Modified")
            .json(json!([
                score_json(&documents[1].id, 99.0),
                score_json(&documents[2].id, 98.0),
                score_json(&documents[4].id, 96.0),
            ]));
        suite
            .send(get(&format!("{base}&offset=0")).key())
            .await
            .status(StatusCode::OK)
            .header_present("Last-Modified")
            .json(all);
        suite
            .send(get(&format!("{base}&user-id=0&offset=0")).key())
            .await
            .status(StatusCode::OK)
            .header_present("Last-Modified")
            .json(json!([
                score_json(&documents[0].id, 100.0),
                score_json(&documents[2].id, 98.0),
                score_json(&documents[4].id, 96.0),
            ]));
    }
}

#[tokio::test]
async fn user_to_user() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| {
        c.recommend.user_to_user = Some(vec![gorse_config::config::schema::UserToUserConfig {
            name: "default".to_owned(),
            kind: "items".to_owned(),
            ..Default::default()
        }]);
    });
    let collection = vectors::user_to_user_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    let sparse = |id: &str, value: f32| Vector {
        id: id.to_owned(),
        indices: vec![0],
        values: vec![value],
        ..Default::default()
    };
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse("0", 1.0),
                sparse("1", 5.0),
                sparse("2", 4.0),
                sparse("3", 3.0),
                sparse("4", 2.0),
                sparse("5", 1.0),
            ],
        )
        .await
        .expect("add the vectors");

    let expected = json!([
        score_json("1", 5.0),
        score_json("2", 4.0),
        score_json("3", 3.0),
        score_json("4", 2.0),
        score_json("5", 1.0),
    ]);
    suite
        .send(get("/api/user-to-user/default/0").key())
        .await
        .status(StatusCode::OK)
        .json(expected.clone());
    suite
        .send(get("/api/user/0/neighbors").key())
        .await
        .status(StatusCode::OK)
        .json(expected);
}

/// Go's `data.ItemPatch{IsHidden: new(true)}`, as its JSON encoding writes it.
fn hide_item_patch() -> Value {
    json!({"IsHidden": true, "Categories": null, "Timestamp": null, "Labels": null, "Comment": null})
}

/// The feedback Go's recommend tests post: two reads in the past and one in
/// the future, which is not yet a read.
fn recommend_feedback() -> Value {
    json!([
        {"FeedbackType": "a", "UserId": "0", "ItemId": "2", "Value": 0, "Timestamp": ZERO_TIME, "Updated": ZERO_TIME, "Labels": null, "Comment": ""},
        {"FeedbackType": "a", "UserId": "0", "ItemId": "4", "Value": 0, "Timestamp": ZERO_TIME, "Updated": ZERO_TIME, "Labels": null, "Comment": ""},
        {"FeedbackType": "a", "UserId": "0", "ItemId": "1", "Value": 0, "Timestamp": gotime::now().add(gotime::HOUR).format(gotime::RFC3339_NANO), "Updated": ZERO_TIME, "Labels": null, "Comment": ""},
    ])
}

/// The zero time, as Go writes an unset `time.Time`.
const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// Go's `data.Item` with only the id set, inserted by the tests directly.
fn plain_item(item_id: &str) -> Item {
    Item {
        item_id: item_id.to_owned(),
        ..Default::default()
    }
}

#[tokio::test]
async fn get_recommends() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    // insert hidden items
    suite
        .cache
        .add_scores(&ctx, cache::RECOMMEND, "0", &[document("0", 100.0, &[""])])
        .await
        .expect("add the scores");
    // hide item
    suite
        .send(patch("/api/item/0").key().json(&hide_item_patch()))
        .await
        .status(StatusCode::OK);
    // insert items
    let items: Vec<Item> = ["1", "2", "3", "4", "5", "6", "7", "8"]
        .into_iter()
        .map(plain_item)
        .collect();
    suite
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    // insert recommendation
    let recommendation: Vec<Score> = [("1", 99.0), ("2", 98.0), ("3", 97.0), ("4", 96.0), ("5", 95.0), ("6", 94.0), ("7", 93.0), ("8", 92.0)]
        .into_iter()
        .map(|(item_id, score)| document(item_id, score, &[""]))
        .collect();
    suite
        .cache
        .add_scores(&ctx, cache::RECOMMEND, "0", &recommendation)
        .await
        .expect("add the recommendation");
    // insert feedback
    suite
        .send(post("/api/feedback").key().json(&recommend_feedback()))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 3}));

    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "3", "5"]));
    suite
        .send(get("/api/recommend/0?n=3").key().header("X-API-Version", "2"))
        .await
        .status(StatusCode::OK)
        .json(json!([score_json("1", 99.0), score_json("3", 97.0), score_json("5", 95.0)]));
    suite
        .send(get("/api/recommend/0?n=3&offset=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["6", "7", "8"]));
    suite
        .send(get("/api/recommend/0?n=3&offset=10000").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/recommend/0?n=3&write-back-type=read").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "3", "5"]));
    suite
        .send(get("/api/recommend/0?n=3&write-back-type=read&write-back-delay=10m").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["6", "7", "8"]));
    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["6", "7", "8"]));
}

#[tokio::test]
async fn get_recommends_multi_categories() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    // insert recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 1.0, &[""]),
                document("2", 2.0, &["", "2"]),
                document("3", 3.0, &["", "3"]),
                document("4", 4.0, &["", "2"]),
                document("5", 5.0, &["", "5"]),
                document("6", 6.0, &["", "2", "3"]),
                document("7", 7.0, &["", "7"]),
                document("8", 8.0, &["", "2"]),
                document("9", 9.0, &["", "3"]),
            ],
        )
        .await
        .expect("add the recommendation");
    suite
        .send(get("/api/recommend/0?n=3&category=2&category=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["6"]));
}

#[tokio::test]
async fn get_recommends_replacement() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| c.recommend.replacement.enable_replacement = true);
    // insert recommendation
    let recommendation: Vec<Score> = [("0", 100.0), ("1", 99.0), ("2", 98.0), ("3", 97.0), ("4", 96.0), ("5", 95.0), ("6", 94.0), ("7", 93.0), ("8", 92.0)]
        .into_iter()
        .map(|(item_id, score)| document(item_id, score, &[""]))
        .collect();
    suite
        .cache
        .add_scores(&ctx, cache::RECOMMEND, "0", &recommendation)
        .await
        .expect("add the recommendation");
    // hide item
    suite
        .send(patch("/api/item/0").key().json(&hide_item_patch()))
        .await
        .status(StatusCode::OK);
    // insert feedback
    suite
        .send(post("/api/feedback").key().json(&recommend_feedback()))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 3}));
    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3"]));
}

/// Go's `data.Feedback` of type `a` with the given time, as a request body.
fn feedback_at(item_id: &str, timestamp: &str) -> Value {
    json!({"FeedbackType": "a", "UserId": "0", "ItemId": item_id, "Value": 0, "Timestamp": timestamp, "Updated": ZERO_TIME, "Labels": null, "Comment": ""})
}

/// Go's `data.Feedback` of type `a` from `user_id` to `item_id`, written to the
/// store directly as the Go tests do.
async fn store_feedback(suite: &Suite, user_id: &str, item_id: &str) {
    suite
        .data
        .batch_insert_feedback(
            &Context::background(),
            &[Feedback {
                feedback_key: FeedbackKey {
                    feedback_type: "a".to_owned(),
                    user_id: user_id.to_owned(),
                    item_id: item_id.to_owned(),
                },
                ..Default::default()
            }],
            true,
            true,
            true,
        )
        .await
        .expect("insert the feedback");
}

/// A sparse vector with the given indices and values, as the Go tests write
/// them.
fn sparse_vector(id: &str, indices: &[u32], values: &[f32], categories: &[&str]) -> Vector {
    Vector {
        id: id.to_owned(),
        indices: indices.to_vec(),
        values: values.to_vec(),
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn get_recommends_fallback_item_to_item() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    let mut positive = FeedbackTypeExpression::default();
    positive.from_string("a").expect("a feedback type");
    suite.update_config(|c| {
        c.recommend.context_size = 4;
        c.recommend.data_source.positive_feedback_types = Some(vec![positive]);
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            ..Default::default()
        }]);
    });
    // insert recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 99.0, &[]),
                document("2", 98.0, &[]),
                document("3", 97.0, &[]),
                document("4", 96.0, &[]),
            ],
        )
        .await
        .expect("add the recommendation");
    // insert feedback
    suite
        .send(
            post("/api/feedback").key().json(&json!([
                feedback_at("1", "2010-01-01T01:01:01.000000001Z"),
                feedback_at("2", "2009-01-01T01:01:01.000000001Z"),
                feedback_at("3", "2008-01-01T01:01:01.000000001Z"),
                feedback_at("4", "2007-01-01T01:01:01.000000001Z"),
                feedback_at("5", "2006-01-01T01:01:01.000000001Z"),
            ])),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 5}));

    // insert item-to-item vectors
    suite.update_config(|c| {
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            kind: "tags".to_owned(),
            column: "item.Labels".to_owned(),
            ..Default::default()
        }]);
    });
    let collection = vectors::item_to_item_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse_vector("1", &[0], &[1.0], &[]),
                sparse_vector("2", &[1], &[1.0], &[]),
                sparse_vector("3", &[2], &[1.0], &[]),
                sparse_vector("4", &[3], &[1.0], &[]),
                sparse_vector("5", &[4], &[1.0], &[]),
                sparse_vector("6", &[3], &[1.0], &[]),
                sparse_vector("7", &[2, 3], &[1.0, 1.0], &["*"]),
                sparse_vector("8", &[1, 2, 3], &[1.0, 1.0, 1.0], &[]),
                sparse_vector("9", &[0, 1, 2, 3], &[1.0, 1.0, 1.0, 1.0], &["*"]),
            ],
        )
        .await
        .expect("add the vectors");

    // test fallback
    suite.update_config(|c| {
        c.recommend.fallback.recommenders = Some(vec!["item-to-item/default".to_owned()]);
    });
    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["9", "8", "7"]));
    suite
        .send(get("/api/recommend/0/*?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["9", "7"]));
}

#[tokio::test]
async fn get_recommends_fallback_user_to_user() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| {
        c.recommend.user_to_user = Some(vec![gorse_config::config::schema::UserToUserConfig {
            name: "default".to_owned(),
            kind: "items".to_owned(),
            ..Default::default()
        }]);
    });
    // insert recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 99.0, &[]),
                document("2", 98.0, &[]),
                document("3", 97.0, &[]),
                document("4", 96.0, &[]),
            ],
        )
        .await
        .expect("add the recommendation");
    // insert feedback
    suite
        .send(
            post("/api/feedback").key().json(&json!([
                feedback_at_zero("1"),
                feedback_at_zero("2"),
                feedback_at_zero("3"),
                feedback_at_zero("4"),
            ])),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 4}));
    // insert similar users
    let collection = vectors::user_to_user_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse_vector("0", &[0], &[1.0], &[]),
                sparse_vector("1", &[0], &[2.0], &[]),
                sparse_vector("2", &[0], &[1.5], &[]),
                sparse_vector("3", &[0], &[1.0], &[]),
            ],
        )
        .await
        .expect("add the vectors");
    store_feedback(&suite, "1", "11").await;
    store_feedback(&suite, "2", "12").await;
    store_feedback(&suite, "2", "48").await;
    store_feedback(&suite, "3", "13").await;
    store_feedback(&suite, "3", "48").await;
    // insert categorized items
    suite
        .data
        .batch_insert_items(
            &ctx,
            &[
                Item {
                    item_id: "12".to_owned(),
                    categories: Some(vec!["*".to_owned()]),
                    ..Default::default()
                },
                Item {
                    item_id: "48".to_owned(),
                    categories: Some(vec!["*".to_owned()]),
                    ..Default::default()
                },
            ],
        )
        .await
        .expect("insert the items");
    // test fallback
    suite.update_config(|c| {
        c.recommend.fallback.recommenders = Some(vec!["user-to-user/default".to_owned()]);
    });
    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["48", "11", "12"]));
    suite
        .send(get("/api/recommend/0/*?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["48", "12"]));
}

/// Go's feedback of type `a` from user `0` to `item_id` with no timestamp.
fn feedback_at_zero(item_id: &str) -> Value {
    feedback_at(item_id, ZERO_TIME)
}

#[tokio::test]
async fn get_recommends_fallback_latest() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    // insert offline recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 99.0, &["*"]),
                document("2", 98.0, &["*"]),
                document("3", 97.0, &["*"]),
                document("4", 96.0, &["*"]),
            ],
        )
        .await
        .expect("add the recommendation");
    // insert latest items
    suite
        .data
        .batch_insert_items(&ctx, &latest_items())
        .await
        .expect("insert the items");
    // test latest fallback
    suite.update_config(|c| c.recommend.fallback.recommenders = Some(vec!["latest".to_owned()]));
    suite
        .send(get("/api/recommend/0?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "5", "6", "7", "8"]));
    suite
        .send(get("/api/recommend/0/*?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "105", "106", "107", "108"]));
}

/// The items Go's latest tests insert: four plain items and four in `*`.
fn latest_items() -> Vec<Item> {
    let item = |item_id: &str, categories: Option<&[&str]>, unix: i64| Item {
        item_id: item_id.to_owned(),
        categories: categories.map(|c| c.iter().map(|c| (*c).to_owned()).collect()),
        timestamp: gotime::unix(unix, 0),
        ..Default::default()
    };
    vec![
        item("5", None, 95),
        item("6", None, 94),
        item("7", None, 93),
        item("8", None, 92),
        item("105", Some(&["*"]), 85),
        item("106", Some(&["*"]), 84),
        item("107", Some(&["*"]), 83),
        item("108", Some(&["*"]), 82),
    ]
}

#[tokio::test]
async fn get_recommends_fallback_collaborative_filtering() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    // insert offline recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 99.0, &["*"]),
                document("2", 98.0, &["*"]),
                document("3", 97.0, &["*"]),
                document("4", 96.0, &["*"]),
            ],
        )
        .await
        .expect("add the recommendation");
    // insert collaborative filtering recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::COLLABORATIVE_FILTERING,
            "0",
            &[
                document("13", 79.0, &["*"]),
                document("14", 78.0, &["*"]),
                document("15", 77.0, &["*"]),
                document("16", 76.0, &["*"]),
            ],
        )
        .await
        .expect("add the collaborative filtering recommendation");
    // test collaborative filtering
    suite.update_config(|c| c.recommend.fallback.recommenders = Some(vec!["collaborative".to_owned()]));
    suite
        .send(get("/api/recommend/0?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "13", "14", "15", "16"]));
    suite
        .send(get("/api/recommend/0/*?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "13", "14", "15", "16"]));
}

#[tokio::test]
async fn get_recommends_fallback_non_personalized() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    // insert offline recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::RECOMMEND,
            "0",
            &[
                document("1", 99.0, &["*"]),
                document("2", 98.0, &["*"]),
                document("3", 97.0, &["*"]),
                document("4", 96.0, &["*"]),
            ],
        )
        .await
        .expect("add the recommendation");
    // insert non-personalized recommendation
    suite
        .cache
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "popular",
            &[
                document("5", 95.0, &[""]),
                document("6", 94.0, &[""]),
                document("7", 93.0, &[""]),
                document("8", 92.0, &[""]),
                document("105", 91.0, &["*"]),
                document("106", 90.0, &["*"]),
                document("107", 89.0, &["*"]),
                document("108", 88.0, &["*"]),
            ],
        )
        .await
        .expect("add the non-personalized recommendation");
    // test non-personalized fallback
    suite.update_config(|c| c.recommend.fallback.recommenders = Some(vec!["non-personalized/popular".to_owned()]));
    suite
        .send(get("/api/recommend/0?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "5", "6", "7", "8"]));
    suite
        .send(get("/api/recommend/0/*?n=8").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["1", "2", "3", "4", "105", "106", "107", "108"]));
}

#[tokio::test]
async fn get_recommends_latest() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite
        .data
        .batch_insert_items(&ctx, &latest_items()[..4])
        .await
        .expect("insert the items");
    suite.update_config(|c| {
        c.recommend.ranker.kind = "none".to_owned();
        c.recommend.ranker.recommenders = Some(vec!["latest".to_owned()]);
        c.recommend.fallback.recommenders = Some(vec![]);
    });
    suite
        .send(get("/api/recommend/0?n=4").key())
        .await
        .status(StatusCode::OK)
        .json(json!(["5", "6", "7", "8"]));
}

/// Go's `data.Feedback` of type `a` at `timestamp`, with the user `0`.
fn session_feedback(item_id: &str, timestamp: &str) -> Value {
    feedback_at(item_id, timestamp)
}

#[tokio::test]
async fn session_recommend() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    let mut positive = FeedbackTypeExpression::default();
    positive.from_string("a").expect("a feedback type");
    suite.update_config(|c| {
        c.recommend.context_size = 4;
        c.recommend.data_source.positive_feedback_types = Some(vec![positive]);
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            kind: "tags".to_owned(),
            column: "item.Labels".to_owned(),
            ..Default::default()
        }]);
    });

    // insert item-to-item vectors
    let collection = vectors::item_to_item_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    suite
        .vectors
        .add_vectors(
            &ctx,
            &collection,
            &[
                sparse_vector("1", &[0], &[1.0], &[]),
                sparse_vector("2", &[1], &[1.0], &[]),
                sparse_vector("3", &[2], &[1.0], &[]),
                sparse_vector("4", &[3], &[1.0], &[]),
                sparse_vector("5", &[4], &[1.0], &[]),
                sparse_vector("6", &[3], &[1.0], &[]),
                sparse_vector("7", &[2, 3], &[1.0, 1.0], &["*"]),
                sparse_vector("8", &[1, 2, 3], &[1.0, 1.0, 1.0], &[]),
                sparse_vector("9", &[0, 1, 2, 3], &[1.0, 1.0, 1.0, 1.0], &["*"]),
                sparse_vector("100", &[0], &[100.0], &[]),
            ],
        )
        .await
        .expect("add the vectors");
    let items = vec![
        Item {
            item_id: "6".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "7".to_owned(),
            categories: Some(vec!["*".to_owned()]),
            ..Default::default()
        },
        Item {
            item_id: "8".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "9".to_owned(),
            categories: Some(vec!["*".to_owned()]),
            ..Default::default()
        },
    ];
    suite
        .data
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");

    // hide items
    suite
        .send(
            post("/api/item")
                .key()
                .json(&json!({"ItemId": "100", "IsHidden": true, "Categories": null, "Timestamp": "", "Labels": null, "Comment": ""})),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));

    // test fallback
    let feedback = json!([
        session_feedback("1", "2010-01-01T01:01:01.000000001Z"),
        session_feedback("2", "2009-01-01T01:01:01.000000001Z"),
        session_feedback("3", "2008-01-01T01:01:01.000000001Z"),
        session_feedback("4", "2007-01-01T01:01:01.000000001Z"),
        session_feedback("5", "2006-01-01T01:01:01.000000001Z"),
    ]);
    suite
        .send(post("/api/session/recommend?n=3").key().json(&feedback))
        .await
        .status(StatusCode::OK)
        .json(json!([score_json("9", 4.0), score_json("8", 3.0), score_json("7", 2.0)]));
    suite
        .send(post("/api/session/recommend?offset=100").key().json(&feedback))
        .await
        .status(StatusCode::OK)
        .json(Value::Null);
    suite.update_config(|c| {
        c.recommend.fallback.recommenders = Some(vec!["item_based".to_owned()]);
    });
    suite
        .send(post("/api/session/recommend/*?n=3").key().json(&feedback))
        .await
        .status(StatusCode::OK)
        .json(json!([score_json("9", 4.0), score_json("7", 2.0)]));
}

/// Go's `time.Date(1989, 6, day, 1, 1, 1, 1, time.UTC).Unix()`.
fn day_unix(day: usize) -> i64 {
    [612666061, 612752461, 612838861, 612925261, 613011661][day - 1]
}

/// Go's `time.Date(1989, 6, day, 1, 1, 1, 1, time.UTC).String()`, the
/// timestamp the visibility test sends.
fn day_string(day: usize) -> String {
    format!("1989-06-{day:02} 01:01:01.000000001 +0000 UTC")
}

/// Go's `cache.Score` list as the route writes it: the id and the score.
fn scores_json(documents: &[Score]) -> Value {
    Value::Array(
        documents
            .iter()
            .map(|document| score_json(&document.id, document.score))
            .collect(),
    )
}

/// Go's `cache.ConvertDocumentsToValues`: the ids of the documents.
fn ids_json(documents: &[Score]) -> Value {
    Value::Array(
        documents
            .iter()
            .map(|document| Value::String(document.id.clone()))
            .collect(),
    )
}

#[tokio::test]
async fn visibility() {
    let suite = Suite::default_suite().await;
    let ctx = Context::background();
    suite.update_config(|c| {
        c.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
            name: "default".to_owned(),
            kind: "tags".to_owned(),
            column: "item.Labels".to_owned(),
            ..Default::default()
        }]);
    });
    // insert items: 0, 1, 2, 3, 4
    let items: Vec<Value> = (0..5)
        .map(|i| {
            json!({"ItemId": i.to_string(), "IsHidden": false, "Categories": ["a"], "Timestamp": day_string(i + 1), "Labels": null, "Comment": ""})
        })
        .collect();
    suite
        .send(post("/api/items?n=3").key().json(&Value::Array(items)))
        .await
        .status(StatusCode::OK);

    // insert cache: the documents, newest first, as Go reverses them
    let mut documents: Vec<Score> = (0..5)
        .map(|i| document(&i.to_string(), day_unix(i + 1) as f64, &["", "a"]))
        .collect();
    documents.reverse();
    let collection = vectors::item_to_item_collection("default");
    suite
        .vectors
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("add the collection");
    let mut values = vec![sparse_vector("100", &[0], &[1.0], &[])];
    let mut neighbor_documents = Vec::new();
    for document in &documents[..documents.len() - 1] {
        values.push(sparse_vector(
            &document.id,
            &[0],
            &[document.score as f32],
            &document.categories.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
        neighbor_documents.push(Score {
            score: f64::from(document.score as f32),
            ..document.clone()
        });
    }
    suite
        .vectors
        .add_vectors(&ctx, &collection, &values)
        .await
        .expect("add the vectors");
    suite
        .cache
        .add_scores(&ctx, cache::RECOMMEND, "100", &documents)
        .await
        .expect("add the recommendation");

    // delete item
    suite.send(delete("/api/item/0").key()).await.status(StatusCode::OK);
    // modify item
    suite
        .send(patch("/api/item/1").key().json(&hide_patch(Some(true))))
        .await
        .status(StatusCode::OK);
    // overwrite item
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "2", "IsHidden": true, "Categories": null, "Timestamp": "", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);

    // recommend
    suite
        .send(get("/api/latest").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&documents[..2]));
    suite
        .send(get("/api/item/100/neighbors/").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&neighbor_documents[..2]));
    suite
        .send(get("/api/recommend/100/").key())
        .await
        .status(StatusCode::OK)
        .json(ids_json(&documents[..2]));

    // insert item
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "0", "IsHidden": false, "Categories": null, "Timestamp": day_string(1), "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);
    // modify item
    suite
        .send(patch("/api/item/1").key().json(&hide_patch(Some(false))))
        .await
        .status(StatusCode::OK);
    // overwrite item
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "2", "IsHidden": false, "Categories": null, "Timestamp": day_string(3), "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);

    // recommend
    suite
        .send(get("/api/latest").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&documents));
    suite
        .send(get("/api/item/100/neighbors/").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&neighbor_documents));
    suite
        .send(get("/api/recommend/100/").key())
        .await
        .status(StatusCode::OK)
        .json(ids_json(&documents));

    // delete category
    suite
        .send(delete("/api/item/0/category/a").key())
        .await
        .status(StatusCode::OK);
    // modify category
    suite
        .send(patch("/api/item/1").key().json(&json!({"IsHidden": null, "Categories": [], "Timestamp": null, "Labels": null, "Comment": null})))
        .await
        .status(StatusCode::OK);
    // overwrite category
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "2", "IsHidden": false, "Categories": [], "Timestamp": "", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);

    // recommend
    suite
        .send(get("/api/latest/a").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&documents[..2]));
    suite
        .send(get("/api/item/100/neighbors/a").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&neighbor_documents[..2]));
    suite
        .send(get("/api/recommend/100/a").key())
        .await
        .status(StatusCode::OK)
        .json(ids_json(&documents[..2]));

    // insert category
    suite
        .send(put("/api/item/0/category/a").key())
        .await
        .status(StatusCode::OK);
    // modify category
    suite
        .send(patch("/api/item/1").key().json(&json!({"IsHidden": null, "Categories": ["a"], "Timestamp": null, "Labels": null, "Comment": null})))
        .await
        .status(StatusCode::OK);
    // overwrite category
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "2", "IsHidden": false, "Categories": ["a"], "Timestamp": day_string(3), "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);

    // recommend
    suite
        .send(get("/api/latest/a").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&documents));
    suite
        .send(get("/api/item/100/neighbors/a").key())
        .await
        .status(StatusCode::OK)
        .json(scores_json(&neighbor_documents));
    suite
        .send(get("/api/recommend/100/a").key())
        .await
        .status(StatusCode::OK)
        .json(ids_json(&documents));
}

/// Go's `data.ItemPatch` that sets only `IsHidden`, when `is_hidden` is given.
fn hide_patch(is_hidden: Option<bool>) -> Value {
    json!({"IsHidden": is_hidden, "Categories": null, "Timestamp": null, "Labels": null, "Comment": null})
}
