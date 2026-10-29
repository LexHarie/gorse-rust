// Ports: server/rest_test.go (no Go test; the behaviour is encoding/json's, which
// Go's readEntity uses) @ add21b5

//! Go's decoder matches a JSON member name to a struct field without regard to
//! case, after trying an exact match. Several members that name one field leave
//! the value of the last one. These tests post bodies with the names in other
//! cases and check what is stored, as Go stores it. The expectations were
//! checked against a Go `json.Decoder` with `UseNumber`, which `readEntity` uses.

use axum::http::StatusCode;
use serde_json::json;

use super::{Suite, get, patch, post};

#[tokio::test]
async fn user_names_match_case_insensitively() {
    let suite = Suite::default_suite().await;

    suite
        .send(
            post("/api/user")
                .key()
                .raw(r#"{"userid":"mixed-user","labels":null,"comment":"c"}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/mixed-user").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "mixed-user", "Labels": null, "Comment": "c"}));
}

#[tokio::test]
async fn the_last_of_several_names_for_one_field_wins() {
    let suite = Suite::default_suite().await;

    // The second member folds to `UserId`, so the user is "b".
    suite
        .send(
            post("/api/user")
                .key()
                .raw(r#"{"UserId":"a","userid":"b","Labels":null}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/b").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "b", "Labels": null, "Comment": ""}));

    // An exact name beats a folded one, even when the folded name comes first.
    suite
        .send(
            post("/api/user")
                .key()
                .raw(r#"{"USERID":"u","UserId":"exact"}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/exact").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "exact", "Labels": null, "Comment": ""}));
}

#[tokio::test]
async fn item_and_feedback_names_match_case_insensitively() {
    let suite = Suite::default_suite().await;

    suite
        .send(post("/api/item").key().raw(
            r#"{"itemid":"mixed-item","ishidden":true,"categories":["c"],"timestamp":"2023-11-14T22:13:20Z","labels":null,"comment":""}"#,
        ))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/item/mixed-item").key())
        .await
        .status(StatusCode::OK)
        .json(json!({
            "ItemId": "mixed-item",
            "IsHidden": true,
            "Categories": ["c"],
            "Timestamp": "2023-11-14T22:13:20Z",
            "Labels": null,
            "Comment": "",
        }));

    suite
        .send(post("/api/feedback").key().raw(
            r#"[{"feedbacktype":"click","userid":"mixed-user","ITEMID":"mixed-item","value":1,"timestamp":"2023-11-14T22:13:20Z","labels":null,"comment":""}]"#,
        ))
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/feedback/click/mixed-user/mixed-item").key())
        .await
        .status(StatusCode::OK)
        .json(json!({
            "FeedbackType": "click",
            "UserId": "mixed-user",
            "ItemId": "mixed-item",
            "Value": 1.0,
            "Timestamp": "2023-11-14T22:13:20Z",
            // Go's BatchInsertFeedback sets Updated to the Timestamp.
            "Updated": "2023-11-14T22:13:20Z",
            "Labels": null,
            "Comment": "",
        }));
}

#[tokio::test]
async fn patch_names_match_case_insensitively() {
    let suite = Suite::default_suite().await;
    suite
        .send(
            post("/api/user")
                .key()
                .raw(r#"{"UserId":"patched-user","Labels":null,"Comment":""}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));

    suite
        .send(
            patch("/api/user/patched-user")
                .key()
                .raw(r#"{"COMMENT":"patched"}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/patched-user").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "patched-user", "Labels": null, "Comment": "patched"}));
}

#[tokio::test]
async fn label_names_are_not_matched() {
    let suite = Suite::default_suite().await;

    // The members of labels are free-form keys, so `KEY` stays `KEY`.
    suite
        .send(
            post("/api/user")
                .key()
                .raw(r#"{"UserId":"labelled","Labels":{"KEY":1}}"#),
        )
        .await
        .status(StatusCode::OK)
        .json(json!({"RowAffected": 1}));
    suite
        .send(get("/api/user/labelled").key())
        .await
        .status(StatusCode::OK)
        .json(json!({"UserId": "labelled", "Labels": {"KEY": 1}, "Comment": ""}));
}
