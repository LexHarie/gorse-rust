// Ports: server/rest_test.go (TestQuota) @ add21b5

//! The quota limits, as Go's `TestQuota`: each limit is set on its own, the
//! write over it is refused with 429, and the limit is cleared again.

use axum::http::StatusCode;
use serde_json::json;

use super::{Suite, patch, post};

#[tokio::test]
async fn quota() {
    let suite = Suite::default_suite().await;

    suite.update_config(|c| c.quota.max_users_count = 1);
    suite
        .send(post("/api/user").key().json(&json!({"UserId": "limited-user-1", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::OK);
    suite
        .send(post("/api/user").key().json(&json!({"UserId": "limited-user-2", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_users_count = 0);

    suite.update_config(|c| c.quota.max_items_count = 1);
    suite
        .send(post("/api/item").key().json(&item("limited-item-1")))
        .await
        .status(StatusCode::OK);
    suite
        .send(post("/api/item").key().json(&item("limited-item-2")))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_items_count = 0);

    suite.update_config(|c| c.quota.max_comment_size = 3);
    suite
        .send(post("/api/user").key().json(&json!({"UserId": "oversized-user-comment", "Labels": null, "Comment": "toolong"})))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite
        .send(
            patch("/api/user/oversized-user-comment")
                .key()
                .json(&json!({"Labels": null, "Comment": "toolong"})),
        )
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_comment_size = 0);

    suite.update_config(|c| c.quota.max_labels_size = 5);
    suite
        .send(post("/api/users").key().json(&json!([{"UserId": "oversized-user-labels", "Labels": ["abcdef"], "Comment": ""}])))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_labels_size = 0);

    suite.update_config(|c| c.quota.max_comment_size = 3);
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "oversized-item-comment", "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": null, "Comment": "toolong"})))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite
        .send(
            patch("/api/item/oversized-item-comment")
                .key()
                .json(&json!({"IsHidden": null, "Categories": null, "Timestamp": null, "Labels": null, "Comment": "toolong"})),
        )
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_comment_size = 0);

    suite.update_config(|c| c.quota.max_labels_size = 5);
    suite
        .send(post("/api/items").key().json(&json!([{"ItemId": "oversized-item-labels", "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": ["abcdef"], "Comment": ""}])))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_labels_size = 0);

    suite.update_config(|c| c.quota.max_categories_count = 1);
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "too-many-categories", "IsHidden": false, "Categories": ["a", "b"], "Timestamp": "", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_categories_count = 0);

    suite.update_config(|c| c.quota.max_categories_size = 5);
    suite
        .send(post("/api/item").key().json(&json!({"ItemId": "oversized-categories", "IsHidden": false, "Categories": ["abcdef"], "Timestamp": "", "Labels": null, "Comment": ""})))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_categories_size = 0);

    suite.update_config(|c| c.quota.max_labels_size = 5);
    suite
        .send(post("/api/feedback").key().json(&json!([{"FeedbackType": "click", "UserId": "u", "ItemId": "i", "Value": 0, "Timestamp": "", "Labels": ["abcdef"], "Comment": ""}])))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
    suite.update_config(|c| c.quota.max_labels_size = 0);

    suite.update_config(|c| c.quota.max_comment_size = 3);
    suite
        .send(post("/api/feedback").key().json(&json!([{"FeedbackType": "click", "UserId": "u", "ItemId": "i", "Value": 0, "Timestamp": "", "Labels": null, "Comment": "toolong"}])))
        .await
        .status(StatusCode::TOO_MANY_REQUESTS);
}

/// Go's `Item` with only the id set, as its JSON encoding writes every field.
fn item(item_id: &str) -> serde_json::Value {
    json!({"ItemId": item_id, "IsHidden": false, "Categories": null, "Timestamp": "", "Labels": null, "Comment": ""})
}
