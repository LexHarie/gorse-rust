// Ports: server/rest_test.go (TestFeedback) @ add21b5

//! The feedback routes, as Go's `TestFeedback`. The expected bodies are the
//! JSON Go writes for `data.Feedback`, `FeedbackIterator`, `data.Item` and
//! `data.User`.

use axum::http::StatusCode;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use serde_json::{Value, json};

use super::{Suite, delete, get, post, put};

/// The zero time, as Go writes an unset `time.Time`.
const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// Go's `data.Feedback` as its JSON encoding writes it, with the zero time for
/// the timestamp and the update time.
fn stored_feedback(
    feedback_type: &str,
    user_id: &str,
    item_id: &str,
    value: f64,
    labels: Value,
    comment: &str,
) -> Value {
    json!({
        "FeedbackType": feedback_type,
        "UserId": user_id,
        "ItemId": item_id,
        "Value": value,
        "Timestamp": ZERO_TIME,
        "Updated": ZERO_TIME,
        "Labels": labels,
        "Comment": comment,
    })
}

#[tokio::test]
async fn feedback() {
    let suite = Suite::default_suite().await;
    let feedback = vec![
        stored_feedback("click", "0", "0", 1.0, Value::Null, ""),
        stored_feedback("click", "1", "2", 1.0, json!(["positive", "mobile"]), ""),
        stored_feedback(
            "click",
            "2",
            "4",
            1.0,
            json!({"source": "rest", "rank": 2}),
            "",
        ),
        stored_feedback("click", "3", "6", 1.0, Value::Null, ""),
        stored_feedback("click", "4", "8", 1.0, Value::Null, ""),
    ];

    // BatchInsertFeedback
    suite
        .send(post("/api/feedback").key().json(&Value::Array(feedback.clone())))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 5}));
    // Get Feedback
    suite
        .send(get("/api/feedback?cursor=&n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Feedback": feedback}));
    // get feedback by user
    suite
        .send(get("/api/user/1/feedback").key())
        .await
        .status(StatusCode::OK)
        .json(json!([feedback[1]]));
    // get feedback by item
    suite
        .send(get("/api/item/2/feedback").key())
        .await
        .status(StatusCode::OK)
        .json(json!([feedback[1]]));
    // Get Items
    let items: Vec<Value> = ["0", "2", "4", "6", "8"]
        .into_iter()
        .map(|item_id| {
            json!({
                "ItemId": item_id,
                "IsHidden": false,
                "Categories": null,
                "Timestamp": ZERO_TIME,
                "Labels": null,
                "Comment": "",
            })
        })
        .collect();
    suite
        .send(get("/api/items").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Items": items}));
    let users: Vec<Value> = ["0", "1", "2", "3", "4"]
        .into_iter()
        .map(|user_id| json!({"UserId": user_id, "Labels": null, "Comment": ""}))
        .collect();
    suite
        .send(get("/api/users").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Users": users}));
    suite
        .send(get("/api/user/2/feedback/click").key())
        .await
        .status(StatusCode::OK)
        .json(json!([feedback[2]]));
    suite
        .send(get("/api/item/4/feedback/click").key())
        .await
        .status(StatusCode::OK)
        .json(json!([feedback[2]]));

    // test overwrite
    suite
        .send(
            put("/api/feedback").key().json(&json!([
                stored_feedback("click", "0", "0", 0.0, Value::Null, "override")
            ])),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    let ctx = Context::background();
    let now = suite.rest.config().now();
    let mut expression = FeedbackTypeExpression::default();
    expression.from_string("click").expect("a feedback type");
    let ret = suite
        .data
        .get_user_feedback(&ctx, "0", Some(now.clone()), std::slice::from_ref(&expression))
        .await
        .expect("read the feedback");
    assert_eq!(ret.len(), 1);
    assert_eq!(ret[0].comment, "override");

    // test not overwrite
    suite
        .send(
            post("/api/feedback").key().json(&Value::Array(vec![
                stored_feedback("click", "0", "0", 0.0, Value::Null, "not_override"),
            ])),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    let ret = suite
        .data
        .get_user_feedback(&ctx, "0", Some(now), std::slice::from_ref(&expression))
        .await
        .expect("read the feedback");
    assert_eq!(ret.len(), 1);
    assert_eq!(ret[0].comment, "not_override");

    // insert feedback without timestamp
    suite
        .send(
            post("/api/feedback").key().json(&json!([
                {"FeedbackType": "Type", "UserId": "100", "ItemId": "100", "Value": 0, "Timestamp": "", "Labels": null, "Comment": ""}
            ])),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
}

#[tokio::test]
async fn delete_feedback() {
    let suite = Suite::default_suite().await;

    // Get nonexistent typed feedback.
    suite
        .send(get("/api/feedback/missing/missing/missing").key())
        .await
        .status(StatusCode::OK)
        .json(json!({}));
    // Insert feedback
    let feedback = vec![
        stored_feedback("type1", "2", "3", 0.0, Value::Null, ""),
        stored_feedback("type2", "2", "3", 0.0, Value::Null, ""),
        stored_feedback("type3", "2", "3", 0.0, Value::Null, ""),
        stored_feedback("type1", "1", "6", 0.0, Value::Null, ""),
        stored_feedback("type1", "4", "8", 0.0, Value::Null, ""),
    ];
    suite
        .send(post("/api/feedback").key().json(&Value::Array(feedback.clone())))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 5}));
    // Get Feedback
    suite
        .send(get("/api/feedback/2/3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([feedback[0], feedback[1], feedback[2]]));
    // Get typed feedback
    suite
        .send(get("/api/feedback/type2/2/3").key())
        .await
        .status(StatusCode::OK)
        .json(feedback[1].clone());
    // delete feedback
    suite
        .send(delete("/api/feedback/2/3").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 3}));
    // delete typed feedback
    suite
        .send(delete("/api/feedback/type1/4/8").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
}
