// Ports: server/rest_test.go (no test; the answers are the Go server's own, probed
// on empty stores with the default configuration) @ add21b5

//! The list routes on empty stores, which settles O-76 for those routes. Go
//! answers an empty list with `[]` or `{...: []}`, never `null`, and a route
//! whose recommender is not configured with the error text Go writes.

use axum::http::StatusCode;
use serde_json::json;

use super::{Suite, get, post};

#[tokio::test]
async fn empty_stores_answer_empty_lists() {
    let suite = Suite::default_suite().await;

    suite
        .send(get("/api/items?n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Items": []}));
    suite
        .send(get("/api/users?n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Users": []}));
    suite
        .send(get("/api/feedback?n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Feedback": []}));
    suite
        .send(get("/api/latest?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/latest/a?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/latest/a?n=3&offset=50").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/recommend/0?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/recommend/0?n=3&offset=10000").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/recommend/0/a?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/non-personalized/popular?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/non-personalized/popular?n=3&offset=50").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/user/0/feedback").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/item/0/feedback").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/user/0/feedback/click").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/item/0/feedback/click").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/feedback/a/b").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));
    suite
        .send(get("/api/feedback/a/b/c").key())
        .await
        .status(StatusCode::OK)
        .json(json!({}));
}

#[tokio::test]
async fn unconfigured_recommenders_answer_go_errors() {
    let suite = Suite::default_suite().await;

    suite
        .send(get("/api/item/x/neighbors?n=3").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("item-to-item recommendation is not enabled");
    suite
        .send(get("/api/item/x/neighbors?n=3&offset=10000").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("item-to-item recommendation is not enabled");
    suite
        .send(get("/api/item-to-item/default/x").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("item-to-item recommender default not found");
    suite
        .send(get("/api/user/x/neighbors?n=3").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("user-to-user recommendation is not enabled");
    suite
        .send(get("/api/user-to-user/default/x").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("user-to-user recommender default not found");
    // The probe ran with Go's default configuration, where collaborative
    // filtering is off; the suite turns it on, so turn it off again here.
    suite.update_config(|c| c.recommend.collaborative.kind = "none".to_owned());
    suite
        .send(get("/api/collaborative-filtering/0?n=3").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("collaborative filtering recommendation is disabled");
    suite
        .send(post("/api/session/recommend?n=3").key().json(&json!([])))
        .await
        .status(StatusCode::NOT_FOUND)
        .text("item-to-item recommendation is not enabled");
    suite
        .send(get("/api/item/nope").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("item nope not found");
    suite
        .send(get("/api/user/nope").key())
        .await
        .status(StatusCode::NOT_FOUND)
        .text("user nope not found");
}
