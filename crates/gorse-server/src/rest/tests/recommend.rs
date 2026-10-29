// Ports: server/rest_test.go (TestItemToItem, TestNonPersonalizedRecommend,
// TestUserToUser, TestGetRecommends and its variants, TestSessionRecommend) @
// add21b5

//! The recommendation routes, as Go's recommend tests. The expected bodies are
//! the JSON Go writes for each `cache.Score` list, and the `Last-Modified`
//! header is checked where Go checks it.

use axum::http::StatusCode;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use serde_json::{Value, json};

use super::{Suite, get, patch};

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
