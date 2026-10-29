// Ports: server/rest_test.go (TestItems) @ add21b5

//! The item routes and the latest-items lists, as Go's `TestItems`. The
//! expected bodies are the JSON Go writes for `data.Item`, `ItemIterator` and
//! `cache.Score`.

use axum::http::StatusCode;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_storage::data::types::{Feedback, FeedbackKey};
use serde_json::{Value, json};

use super::{Suite, delete, get, patch, post, put};

/// Go's `data.Item` as its JSON encoding writes it.
fn data_item(
    item_id: &str,
    is_hidden: bool,
    categories: Value,
    timestamp: &str,
    labels: Value,
    comment: &str,
) -> Value {
    json!({
        "ItemId": item_id,
        "IsHidden": is_hidden,
        "Categories": categories,
        "Timestamp": timestamp,
        "Labels": labels,
        "Comment": comment,
    })
}

/// Go's `cache.Score` with only the id and the score, as its JSON encoding
/// writes it. The score is the item's timestamp in Unix seconds.
fn score(item_id: &str, unix: i64) -> Value {
    json!({"Id": item_id, "Score": unix})
}

#[tokio::test]
async fn items() {
    let suite = Suite::default_suite().await;
    let items = vec![
        data_item("0", true, Value::Null, "1996-03-15T00:00:00Z", json!(["a"]), "comment_0"),
        data_item("2", false, json!(["*"]), "1997-03-15T00:00:00Z", json!(["a"]), "comment_2"),
        data_item("4", true, Value::Null, "1998-03-15T00:00:00Z", json!(["a", "b"]), "comment_4"),
        data_item("6", false, json!(["*"]), "1999-03-15T00:00:00Z", json!(["b"]), "comment_6"),
        data_item("8", true, Value::Null, "2000-03-15T00:00:00Z", json!(["b"]), "comment_8"),
    ];

    // insert items
    suite
        .send(post("/api/item").key().json(&items[0]))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    // batch insert items
    suite
        .send(post("/api/items").key().json(&Value::Array(items[1..].to_vec())))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 4}));
    // get items
    suite
        .send(get("/api/items?cursor=&n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Items": items}));
    // get latest items
    suite
        .send(get("/api/latest?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("6", 921456000), score("2", 858384000)]));
    suite
        .send(get("/api/latest/?n=3&offset=1").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 858384000)]));
    suite
        .send(get("/api/latest/*?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("6", 921456000), score("2", 858384000)]));

    let feedback = Feedback {
        feedback_key: FeedbackKey {
            feedback_type: "read".to_owned(),
            user_id: "0".to_owned(),
            item_id: "6".to_owned(),
        },
        timestamp: gotime::now().truncate(gotime::HOUR),
        ..Default::default()
    };
    suite
        .data
        .batch_insert_feedback(&Context::background(), &[feedback], true, true, true)
        .await
        .expect("insert the feedback");
    suite
        .send(get("/api/latest?n=3&user-id=0").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 858384000)]));

    // delete item
    suite
        .send(delete("/api/item/6").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    // get item
    suite
        .send(get("/api/item/6").key())
        .await
        .status(StatusCode::NOT_FOUND);
    // get latest items
    suite
        .send(get("/api/latest?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 858384000)]));
    suite
        .send(get("/api/latest/*?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 858384000)]));

    // test modify
    let timestamp = "2010-01-01T01:01:01Z";
    suite
        .send(
            patch("/api/item/2")
                .key()
                .json(&json!({
                    "IsHidden": true,
                    "Categories": ["-"],
                    "Timestamp": timestamp,
                    "Labels": ["a", "b", "c"],
                    "Comment": "modified",
                })),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/item/2").key())
        .await
        .status(StatusCode::OK)
        .json(data_item("2", true, json!(["-"]), timestamp, json!(["a", "b", "c"]), "modified"));
    suite
        .send(
            patch("/api/item/2")
                .key()
                .json(&json!({"IsHidden": false, "Categories": null, "Timestamp": null, "Labels": null, "Comment": null})),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    // get latest items
    suite
        .send(get("/api/latest/-?n=1").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 1262307661)]));
    suite
        .send(get("/api/latest/*?n=3").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));

    // insert category
    suite
        .send(put("/api/item/2/category/@").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/item/2").key())
        .await
        .status(StatusCode::OK)
        .json(data_item("2", false, json!(["-", "@"]), timestamp, json!(["a", "b", "c"]), "modified"));
    // get latest items
    suite
        .send(get("/api/latest/@?n=1").key())
        .await
        .status(StatusCode::OK)
        .json(json!([score("2", 1262307661)]));

    // delete category
    suite
        .send(delete("/api/item/2/category/@").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/item/2").key())
        .await
        .status(StatusCode::OK)
        .json(data_item("2", false, json!(["-"]), timestamp, json!(["a", "b", "c"]), "modified"));
    // get latest items
    suite
        .send(get("/api/latest/@?n=1").key())
        .await
        .status(StatusCode::OK)
        .json(json!([]));

    // insert items without timestamp
    suite
        .send(
            post("/api/item")
                .key()
                .json(&json!({"ItemId": "256", "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": null, "Comment": ""})),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));

    // malicious labels
    suite
        .send(
            post("/api/item")
                .key()
                .json(&json!({"ItemId": "malicious", "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": ["price", 1], "Comment": ""})),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
    suite
        .send(
            post("/api/items")
                .key()
                .json(&json!([{"ItemId": "malicious", "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": ["price", 1], "Comment": ""}])),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
    suite
        .send(
            patch("/api/item/malicious")
                .key()
                .json(&json!({"IsHidden": null, "Categories": null, "Timestamp": null, "Labels": ["price", 1], "Comment": null})),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
}
