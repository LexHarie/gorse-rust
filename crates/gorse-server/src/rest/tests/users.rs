// Ports: server/rest_test.go (TestUsers) @ add21b5

//! The user routes, as Go's `TestUsers`. The expected bodies are the JSON Go
//! writes for `data.User`, `UserIterator` and `Success`.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{Suite, delete, get, patch, post};

/// A user as Go's `data.User` marshals it when only the id is set.
fn plain_user(user_id: &str) -> Value {
    json!({"UserId": user_id, "Labels": null, "Comment": ""})
}

#[tokio::test]
async fn users() {
    let suite = Suite::default_suite().await;
    let users: Vec<Value> = (0..5).map(|id| plain_user(&id.to_string())).collect();

    suite
        .send(post("/api/user").key().json(&users[0]))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/0").key())
        .await
        .status(StatusCode::OK)
        .json(users[0].clone());
    suite
        .send(
            post("/api/users")
                .key()
                .json(&Value::Array(users[1..].to_vec())),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 4}));
    suite
        .send(get("/api/users?cursor=&n=100").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"Cursor": "", "Users": users}));
    suite
        .send(delete("/api/user/0").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/0").key())
        .await
        .status(StatusCode::NOT_FOUND);

    // test modify
    suite
        .send(
            patch("/api/user/1")
                .key()
                .json(&json!({"Labels": ["a", "b", "c"], "Comment": "modified"})),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/1").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "1", "Labels": ["a", "b", "c"], "Comment": "modified"}));

    // malicious labels
    suite
        .send(
            post("/api/user")
                .key()
                .json(&json!({"UserId": "malicious", "Labels": ["price", 100], "Comment": ""})),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
    suite
        .send(
            post("/api/users")
                .key()
                .json(&json!([{"UserId": "malicious", "Labels": ["price", 100], "Comment": ""}])),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
    suite
        .send(
            patch("/api/user/malicious")
                .key()
                .json(&json!({"Labels": ["price", 100], "Comment": null})),
        )
        .await
        .status(StatusCode::BAD_REQUEST);
}
