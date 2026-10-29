// Ports: master/rest_test.go @ add21b5

//! The tests of the master's REST API, ported from Go's `MasterAPITestSuite`.
//!
//! Each test opens its own stores in a temporary directory, logs in with the
//! dashboard credentials as the Go suite does, and sends its requests through
//! the router. The mock OpenAI server runs on a thread of its own.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Query;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt as _;
use gorse_common::expression::must_parse_feedback_type_expression;
use gorse_common::gocontext::Context;
use gorse_common::gourl::query_escape;
use gorse_common::mock::OpenAiServer;
use gorse_config::config::defaults::get_default_config;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use gorse_storage::meta;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::Master;
use crate::rest::bulk::BATCH_SIZE;
use crate::rest::config_map::config_map;
use crate::rest::router;
use crate::test_support::{close_stores, new_master, open_stores};

/// The dashboard credentials the suite logs in with (Go's `mockMasterUsername`
/// and `mockMasterPassword`).
const USER_NAME: &str = "admin";
const PASSWORD: &str = "pass";

/// One test's master, its router, and the cookie of its login.
struct Suite {
    dir: tempfile::TempDir,
    master: Arc<Master>,
    app: Router,
    cookie: String,
    openai: Arc<OpenAiServer>,
}

impl Suite {
    /// Go's `SetupTest`: the default config with the dashboard credentials, the
    /// stores in a fresh directory, the mock OpenAI server, and a login.
    async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// [`Suite::new`] with `configure` applied to the default config first.
    async fn with_config(configure: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let openai = Arc::new(OpenAiServer::new());
        let server = openai.clone();
        std::thread::spawn(move || {
            let _ = server.start();
        });
        openai.ready();
        let mut cfg = get_default_config();
        cfg.recommend.ranker.kind = "fm".to_owned();
        cfg.master.dashboard_user_name = USER_NAME.to_owned();
        cfg.master.dashboard_password = PASSWORD.to_owned();
        cfg.openai.base_url = openai.base_url();
        cfg.openai.auth_token = openai.auth_token().to_owned();
        configure(&mut cfg);
        let master = new_master(dir.path(), cfg, "").await;
        open_stores(&master, dir.path()).await;
        let app = router(master.clone());
        let mut suite = Self {
            dir,
            master,
            app,
            cookie: String::new(),
            openai,
        };
        let login = suite
            .send(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "user_name={USER_NAME}&password={PASSWORD}"
                    )))
                    .expect("a login request"),
            )
            .await;
        assert_eq!(login.status, StatusCode::FOUND);
        suite.cookie = login
            .headers
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        suite
    }

    /// Sends `request` through the router.
    async fn send(&self, request: Request<Body>) -> Reply {
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read")
            .to_vec();
        Reply {
            status,
            headers,
            body,
        }
    }

    /// A `GET` of `uri` with the login cookie.
    async fn get(&self, uri: &str) -> Reply {
        self.send(self.request("GET", uri, Body::empty())).await
    }

    /// A request of `method` to `uri` with the login cookie and `body`.
    fn request(&self, method: &str, uri: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &self.cookie)
            .body(body)
            .expect("a request")
    }

    /// Whether a task run is waiting to start (Go's `suite.scheduled`).
    fn scheduled(&self) -> bool {
        self.master.scheduled_pending()
    }

    /// Closes the stores, as Go's `TearDownTest` does.
    async fn close(self) {
        close_stores(&self.master).await;
        let _ = self.openai.close();
        drop(self.dir);
    }
}

/// The reply of a request: its status, headers and body.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    /// The body as text, as Go's `w.Body.String()` gives it.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The value of header `name`, or the empty string.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    /// The body as JSON.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }
}

/// Go's `marshalJSONLines`: each value as one JSON line.
fn json_lines<T: serde::Serialize>(values: &[T]) -> String {
    values
        .iter()
        .map(|value| format!("{}\n", serde_json::to_string(value).expect("a JSON value")))
        .collect()
}

/// A `multipart/form-data` body with one `file` field holding `content`, and
/// the content type that goes with it.
fn multipart_file(content: &str) -> (String, Body) {
    const BOUNDARY: &str = "gorse-test-boundary";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.jsonl\"\r\nContent-Type: application/octet-stream\r\n\r\n{content}\r\n--{BOUNDARY}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        Body::from(body),
    )
}

/// The time of `sec` seconds and `nsec` nanoseconds since the Unix epoch, in UTC as
/// Go's `time.Date(..., time.UTC)` gives it.
fn utc(sec: i64, nsec: i64) -> gorse_common::gotime::Time {
    gorse_common::gotime::unix(sec, nsec).utc()
}

/// The labels JSON of Go's `map[string]any` literal, as the store reads it.
fn labels(text: &str) -> gorse_storage::data::types::Labels {
    serde_json::from_str(text).expect("labels")
}

#[tokio::test]
async fn export_users() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let users = vec![
        User {
            user_id: "1".to_owned(),
            labels: labels(r#"{"gender":"male","job":"engineer"}"#),
            ..Default::default()
        },
        User {
            user_id: "2".to_owned(),
            labels: labels(r#"{"gender":"male","job":"lawyer"}"#),
            ..Default::default()
        },
        User {
            user_id: "3".to_owned(),
            labels: labels(r#"{"gender":"female","job":"teacher"}"#),
            ..Default::default()
        },
    ];
    suite
        .master
        .data()
        .batch_insert_users(&ctx, &users)
        .await
        .expect("insert users");
    let reply = suite.get("/api/bulk/users").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header("content-type"), "application/jsonl");
    assert_eq!(
        reply.header("content-disposition"),
        "attachment;filename=users.jsonl"
    );
    let expected: Vec<Value> = users
        .iter()
        .map(|user| json!({"UserId": user.user_id, "Labels": user.labels, "Comment": ""}))
        .collect();
    assert_eq!(reply.text(), json_lines(&expected));
    suite.close().await;
}

#[tokio::test]
async fn export_items() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let items = vec![
        Item {
            item_id: "1".to_owned(),
            is_hidden: false,
            categories: Some(vec!["x".to_owned()]),
            timestamp: utc(1_577_840_461, 1),
            labels: labels(r#"{"genre":["comedy","sci-fi"]}"#),
            comment: "o,n,e".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "2".to_owned(),
            is_hidden: false,
            categories: Some(vec!["x".to_owned(), "y".to_owned()]),
            timestamp: utc(1_609_462_861, 1),
            labels: labels(r#"{"genre":["documentary","sci-fi"]}"#),
            comment: "t\r\nw\r\no".to_owned(),
            ..Default::default()
        },
        Item {
            item_id: "3".to_owned(),
            is_hidden: true,
            categories: None,
            timestamp: utc(1_640_998_861, 1),
            labels: Default::default(),
            comment: "\"three\"".to_owned(),
            ..Default::default()
        },
    ];
    suite
        .master
        .data()
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert items");
    let reply = suite.get("/api/bulk/items").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header("content-type"), "application/jsonl");
    assert_eq!(
        reply.header("content-disposition"),
        "attachment;filename=items.jsonl"
    );
    let stored = suite
        .master
        .data()
        .get_items(&ctx, "", 100, None)
        .await
        .expect("read items")
        .1;
    assert_eq!(reply.text(), json_lines(&stored));
    suite.close().await;
}

#[tokio::test]
async fn export_feedback() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let feedback: Vec<Feedback> = [("click", "0", "2"), ("read", "2", "6"), ("share", "1", "4")]
        .into_iter()
        .map(|(feedback_type, user_id, item_id)| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: feedback_type.to_owned(),
                user_id: user_id.to_owned(),
                item_id: item_id.to_owned(),
            },
            ..Default::default()
        })
        .collect();
    suite
        .master
        .data()
        .batch_insert_feedback(&ctx, &feedback, true, true, true)
        .await
        .expect("insert feedback");
    let reply = suite.get("/api/bulk/feedback").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header("content-type"), "application/jsonl");
    assert_eq!(
        reply.header("content-disposition"),
        "attachment;filename=feedback.jsonl"
    );
    let stored = suite
        .master
        .data()
        .get_user_feedback(&ctx, "0", None, &[])
        .await
        .expect("read feedback");
    assert!(!stored.is_empty());
    suite.close().await;
}

#[tokio::test]
async fn import_users() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let (content_type, body) = multipart_file(
        r#"{"UserId":"1","Labels":{"性别":"男","职业":"工程师"}}
{"UserId":"2","Labels":{"性别":"男","职业":"律师"}}
{"UserId":"3","Labels":{"性别":"女","职业":"教师"}}"#,
    );
    let reply = suite
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/bulk/users")
                .header(header::COOKIE, &suite.cookie)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .expect("an import request"),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"RowAffected": 3}));
    let (_, stored) = suite
        .master
        .data()
        .get_users(&ctx, "", 100)
        .await
        .expect("read users");
    let ids: Vec<&str> = stored.iter().map(|user| user.user_id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "3"]);
    assert_eq!(stored[0].labels, labels(r#"{"性别":"男","职业":"工程师"}"#));
    assert!(stored.iter().all(|user| !user.update_at.is_zero()));
    assert!(suite.scheduled());
    suite.close().await;
}

#[tokio::test]
async fn import_items() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let (content_type, body) = multipart_file(
        r#"{"ItemId":"1","IsHidden":false,"Categories":["x"],"Timestamp":"2020-01-01 01:01:01.000000001 +0000 UTC","Labels":{"类型":["喜剧","科幻"]},"Comment":"one"}
{"ItemId":"2","IsHidden":false,"Categories":["x","y"],"Timestamp":"2021-01-01 01:01:01.000000001 +0000 UTC","Labels":{"类型":["卡通","科幻"]},"Comment":"two"}
{"ItemId":"3","IsHidden":true,"Timestamp":"2022-01-01 01:01:01.000000001 +0000 UTC","Comment":"three"}"#,
    );
    let reply = suite
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/bulk/items")
                .header(header::COOKIE, &suite.cookie)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .expect("an import request"),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"RowAffected": 3}));
    let (_, stored) = suite
        .master
        .data()
        .get_items(&ctx, "", 100, None)
        .await
        .expect("read items");
    let expected = [
        (
            "1",
            false,
            Some(vec!["x".to_owned()]),
            utc(1_577_840_461, 1),
            r#"{"类型":["喜剧","科幻"]}"#,
            "one",
        ),
        (
            "2",
            false,
            Some(vec!["x".to_owned(), "y".to_owned()]),
            utc(1_609_462_861, 1),
            r#"{"类型":["卡通","科幻"]}"#,
            "two",
        ),
        ("3", true, None, utc(1_640_998_861, 1), "null", "three"),
    ];
    assert_eq!(stored.len(), expected.len());
    for (item, (id, hidden, categories, timestamp, item_labels, comment)) in
        stored.iter().zip(expected)
    {
        assert_eq!(item.item_id, id);
        assert_eq!(item.is_hidden, hidden);
        assert_eq!(item.categories, categories);
        assert_eq!(item.timestamp, timestamp);
        assert_eq!(item.labels, labels(item_labels));
        assert_eq!(item.comment, comment);
        assert!(!item.update_at.is_zero());
    }
    assert!(suite.scheduled());
    suite.close().await;
}

#[tokio::test]
async fn get_cluster() {
    let suite = Suite::new().await;
    // The store lists the nodes that updated lately, so the nodes are stamped now.
    let now = gorse_common::gotime::now().utc();
    let server_node = gorse_storage::meta::Node {
        uuid: "alan turnin".to_owned(),
        hostname: "192.168.1.100".to_owned(),
        r#type: "Server".to_owned(),
        version: "server_version".to_owned(),
        update_time: now.clone(),
    };
    let worker_node = gorse_storage::meta::Node {
        uuid: "dennis ritchie".to_owned(),
        hostname: "192.168.1.101".to_owned(),
        r#type: "Worker".to_owned(),
        version: "worker_version".to_owned(),
        update_time: now.clone(),
    };
    let meta = suite.master.meta_store();
    meta.update_node(&server_node).await.expect("write a node");
    meta.update_node(&worker_node).await.expect("write a node");
    let reply = suite.get("/api/dashboard/cluster").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    let nodes = body.as_array().expect("an array of nodes");
    assert_eq!(nodes.len(), 2);
    let expected = [
        ("alan turnin", "192.168.1.100", "Server", "server_version"),
        (
            "dennis ritchie",
            "192.168.1.101",
            "Worker",
            "worker_version",
        ),
    ];
    for (node, (uuid, hostname, kind, version)) in nodes.iter().zip(expected) {
        assert_eq!(node["UUID"], uuid);
        assert_eq!(node["Hostname"], hostname);
        assert_eq!(node["Type"], kind);
        assert_eq!(node["Version"], version);
        let update_time = node["UpdateTime"].as_str().expect("a time");
        let parsed = gorse_common::dateparse::parse_any(update_time, &[]).expect("a time");
        assert_eq!(parsed, now);
    }
    suite.close().await;
}

#[tokio::test]
async fn get_stats() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    suite
        .master
        .cache()
        .set(
            &ctx,
            &[
                cache::integer(cache::key(&[cache::GLOBAL_META, cache::NUM_USERS]), 123),
                cache::integer(cache::key(&[cache::GLOBAL_META, cache::NUM_ITEMS]), 234),
                cache::integer(
                    cache::key(&[cache::GLOBAL_META, cache::NUM_VALID_POS_FEEDBACKS]),
                    345,
                ),
                cache::integer(
                    cache::key(&[cache::GLOBAL_META, cache::NUM_VALID_NEG_FEEDBACKS]),
                    456,
                ),
            ],
        )
        .await
        .expect("set the stats");
    let reply = suite.get("/api/dashboard/stats").await;
    assert_eq!(reply.status, StatusCode::OK);
    let zero = "0001-01-01T00:00:00Z";
    assert_eq!(
        reply.json(),
        json!({
            "BinaryVersion": "unknown-version",
            "NumServers": 0,
            "NumWorkers": 0,
            "NumUsers": 123,
            "NumItems": 234,
            "NumUserLabels": 0,
            "NumItemLabels": 0,
            "NumTotalPosFeedback": 0,
            "NumValidPosFeedback": 345,
            "NumValidNegFeedback": 456,
            "PopularItemsUpdateTime": zero,
            "LatestItemsUpdateTime": zero,
            "MatchingModelFitTime": zero,
            "MatchingModelScore": {"NDCG": 0, "Precision": 0, "Recall": 0},
            "RankingModelFitTime": zero,
            "RankingModelScore": {"RMSE": 0, "Precision": 0, "Recall": 0, "Accuracy": 0, "AUC": 0}
        })
    );
    suite.close().await;
}

/// Go's node list is nil when no node is listed, and a nil list is `null`.
#[tokio::test]
async fn get_cluster_is_null_without_nodes() {
    let suite = Suite::new().await;
    let reply = suite.get("/api/dashboard/cluster").await;
    assert_eq!(reply.text(), "null");
    suite.close().await;
}

/// Go's task list is nil when no task has run, and a nil list is `null`.
#[tokio::test]
async fn get_tasks_is_null_without_tasks() {
    let suite = Suite::new().await;
    let reply = suite.get("/api/dashboard/tasks").await;
    assert_eq!(reply.text(), "null");
    suite.close().await;
}

#[tokio::test]
async fn get_categories() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let scores =
        [("a", 3.0), ("b", 2.0), ("c", 1.0)].map(|(id, score)| gorse_storage::cache::Score {
            id: id.to_owned(),
            score,
            is_hidden: false,
            categories: Vec::new(),
            timestamp: Default::default(),
        });
    suite
        .master
        .cache()
        .add_scores(&ctx, cache::ITEM_CATEGORIES, "", &scores)
        .await
        .expect("add the categories");
    let reply = suite.get("/api/dashboard/categories").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!(["a", "b", "c"]));
    suite.close().await;
}

#[tokio::test]
async fn get_users() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let times = [
        ("0", utc(946_688_461, 1), utc(1_577_840_461, 1)),
        ("1", utc(978_310_861, 1), utc(1_609_462_861, 1)),
        ("2", utc(1_009_846_861, 1), utc(1_640_998_861, 1)),
    ];
    for (id, last_active, last_update) in &times {
        suite
            .master
            .data()
            .batch_insert_users(
                &ctx,
                &[User {
                    user_id: (*id).to_owned(),
                    ..Default::default()
                }],
            )
            .await
            .expect("insert a user");
        suite
            .master
            .cache()
            .set(
                &ctx,
                &[
                    cache::time(
                        cache::key(&[cache::LAST_MODIFY_USER_TIME, id]),
                        last_active.clone(),
                    ),
                    cache::time(
                        cache::key(&[cache::RECOMMEND_UPDATE_TIME, id]),
                        last_update.clone(),
                    ),
                ],
            )
            .await
            .expect("set the user times");
    }
    let reply = suite.get("/api/dashboard/users").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({
            "Cursor": "",
            "Users": [
                {"UserId": "0", "Labels": null, "Comment": "", "LastActiveTime": "2000-01-01T01:01:01.000000001Z", "LastUpdateTime": "2020-01-01T01:01:01.000000001Z"},
                {"UserId": "1", "Labels": null, "Comment": "", "LastActiveTime": "2001-01-01T01:01:01.000000001Z", "LastUpdateTime": "2021-01-01T01:01:01.000000001Z"},
                {"UserId": "2", "Labels": null, "Comment": "", "LastActiveTime": "2002-01-01T01:01:01.000000001Z", "LastUpdateTime": "2022-01-01T01:01:01.000000001Z"}
            ]
        })
    );
    suite.close().await;
}

#[tokio::test]
async fn import_feedback() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let (content_type, body) = multipart_file(
        r#"{"FeedbackType":"click","UserId":"0","ItemId":"2","Timestamp":"0001-01-01 00:00:00 +0000 UTC"}
{"FeedbackType":"read","UserId":"2","ItemId":"6","Timestamp":"0001-01-01 00:00:00 +0000 UTC"}
{"FeedbackType":"share","UserId":"1","ItemId":"4","Timestamp":"0001-01-01 00:00:00 +0000 UTC"}"#,
    );
    let reply = suite
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/bulk/feedback")
                .header(header::COOKIE, &suite.cookie)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .expect("an import request"),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({"RowAffected": 3}));
    let stored = suite
        .master
        .data()
        .get_feedback_stream(&ctx, 100, &[])
        .collect::<Vec<_>>()
        .await;
    let mut keys: Vec<(String, String, String)> = Vec::new();
    for batch in stored {
        for feedback in batch.expect("read feedback") {
            keys.push((
                feedback.feedback_key.feedback_type,
                feedback.feedback_key.user_id,
                feedback.feedback_key.item_id,
            ));
        }
    }
    keys.sort();
    let expected: Vec<(String, String, String)> =
        [("click", "0", "2"), ("read", "2", "6"), ("share", "1", "4")]
            .into_iter()
            .map(|(kind, user, item)| (kind.to_owned(), user.to_owned(), item.to_owned()))
            .collect();
    assert_eq!(keys, expected);
    assert!(suite.scheduled());
    suite.close().await;
}

#[tokio::test]
async fn get_latest_items() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    // Each item is stamped on January 1 of its year, at 01:01:01.000000001 UTC.
    let fixtures = [
        ("0", 2025, 1_735_693_261),
        ("1", 2024, 1_704_070_861),
        ("2", 2023, 1_672_534_861),
    ];
    let items: Vec<Item> = fixtures
        .iter()
        .map(|(id, year, _)| Item {
            item_id: (*id).to_owned(),
            timestamp: utc(year_start(*year) + 3661, 1),
            ..Default::default()
        })
        .collect();
    suite
        .master
        .data()
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert items");
    let reply = suite.get("/api/dashboard/latest").await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected: Vec<Value> = fixtures
        .iter()
        .map(|(id, year, score)| {
            json!({
                "ItemId": id,
                "IsHidden": false,
                "Categories": null,
                "Timestamp": format!("{year}-01-01T01:01:01.000000001Z"),
                "Labels": null,
                "Comment": "",
                "Score": score,
            })
        })
        .collect();
    assert_eq!(reply.json(), json!(expected));
    suite.close().await;
}

#[tokio::test]
async fn search_documents_of_items() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    // The operators: the list's collection and subset, the category, and the route.
    let operators = [
        (
            "LatestItems",
            cache::NON_PERSONALIZED,
            "latest",
            "",
            "/api/dashboard/non-personalized/latest/",
        ),
        (
            "PopularItems",
            cache::NON_PERSONALIZED,
            "popular",
            "",
            "/api/dashboard/non-personalized/popular/",
        ),
        (
            "LatestItemsCategory",
            cache::NON_PERSONALIZED,
            "latest",
            "*",
            "/api/dashboard/non-personalized/latest/",
        ),
        (
            "PopularItemsCategory",
            cache::NON_PERSONALIZED,
            "popular",
            "*",
            "/api/dashboard/non-personalized/popular/",
        ),
    ];
    for (i, (name, collection, subset, category, route)) in operators.into_iter().enumerate() {
        let scores: Vec<gorse_storage::cache::Score> = (0..5)
            .map(|j| gorse_storage::cache::Score {
                id: format!("{i}{j}"),
                score: 100.0 - j as f64,
                is_hidden: false,
                categories: vec![category.to_owned()],
                timestamp: Default::default(),
            })
            .collect();
        suite
            .master
            .cache()
            .add_scores(&ctx, collection, subset, &scores)
            .await
            .unwrap_or_else(|err| panic!("{name}: add the scores: {err}"));
        suite
            .master
            .cache()
            .set(
                &ctx,
                &[cache::time(
                    cache::key(&[&format!("{collection}_update_time"), subset]),
                    gorse_common::gotime::now(),
                )],
            )
            .await
            .unwrap_or_else(|err| panic!("{name}: set the update time: {err}"));
        for score in &scores {
            suite
                .master
                .data()
                .batch_insert_items(
                    &ctx,
                    &[Item {
                        item_id: score.id.clone(),
                        ..Default::default()
                    }],
                )
                .await
                .unwrap_or_else(|err| panic!("{name}: insert an item: {err}"));
        }
        let hide = suite
            .send(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/item/{i}3"))
                    .header(header::COOKIE, &suite.cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"IsHidden": true}"#))
                    .expect("a patch request"),
            )
            .await;
        assert_eq!(hide.status, StatusCode::OK, "{name}: hide an item");
        let reply = suite
            .send(
                Request::builder()
                    .method("GET")
                    .uri(format!("{route}?category={category}"))
                    .header(header::COOKIE, &suite.cookie)
                    .body(Body::empty())
                    .expect("a get request"),
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{name}: {}", reply.text());
        assert!(
            !reply.header("last-modified").is_empty(),
            "{name}: Last-Modified"
        );
        let expected: Vec<Value> = [0, 1, 2, 4]
            .into_iter()
            .map(|j| {
                json!({
                    "ItemId": format!("{i}{j}"),
                    "IsHidden": false,
                    "Categories": null,
                    "Timestamp": "0001-01-01T00:00:00Z",
                    "Labels": null,
                    "Comment": "",
                    "Score": 100 - j,
                })
            })
            .collect();
        assert_eq!(reply.json(), json!(expected), "{name}");
    }
    suite.close().await;
}

#[tokio::test]
async fn item_to_item() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let mut cfg = (*suite.master.config()).clone();
    cfg.recommend.item_to_item = Some(vec![gorse_config::config::schema::ItemToItemConfig {
        name: "neighbors".to_owned(),
        kind: "tags".to_owned(),
        column: "item.Labels".to_owned(),
    }]);
    suite.master.replace_config(cfg);
    let collection = gorse_storage::vectors::item_to_item_collection("neighbors");
    suite
        .master
        .vectors()
        .add_collection(
            &ctx,
            &collection,
            0,
            gorse_storage::vectors::Distance::Dot,
            Default::default(),
        )
        .await
        .expect("add the collection");
    suite
        .master
        .vectors()
        .add_vectors(
            &ctx,
            &collection,
            &sparse_vectors(&[("0", 0, 1.0), ("1", 0, 4.0), ("2", 0, 3.0)]),
        )
        .await
        .expect("add the vectors");
    let items = [("1", vec!["movie", "drama"], 4), ("2", vec!["movie"], 3)];
    for (id, categories, _) in &items {
        suite
            .master
            .data()
            .batch_insert_items(
                &ctx,
                &[Item {
                    item_id: (*id).to_owned(),
                    categories: Some(categories.iter().map(|c| (*c).to_owned()).collect()),
                    ..Default::default()
                }],
            )
            .await
            .expect("insert an item");
    }
    let all: Vec<Value> = items
        .iter()
        .map(|(id, categories, score)| scored_item(id, categories, *score))
        .collect();
    let reply = suite.get("/api/dashboard/item-to-item/neighbors/0").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!(all));
    let filtered = suite
        .send(
            Request::builder()
                .method("GET")
                .uri("/api/dashboard/item-to-item/neighbors/0?category=movie&category=drama")
                .header(header::COOKIE, &suite.cookie)
                .body(Body::empty())
                .expect("a get request"),
        )
        .await;
    assert_eq!(filtered.status, StatusCode::OK);
    assert_eq!(filtered.json(), json!(&all[..1]));
    suite.close().await;
}

#[tokio::test]
async fn user_to_user() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let mut cfg = (*suite.master.config()).clone();
    cfg.recommend.user_to_user = Some(vec![gorse_config::config::schema::UserToUserConfig {
        name: "neighbors".to_owned(),
        kind: "items".to_owned(),
        column: String::new(),
    }]);
    suite.master.replace_config(cfg);
    let collection = gorse_storage::vectors::user_to_user_collection("neighbors");
    suite
        .master
        .vectors()
        .add_collection(
            &ctx,
            &collection,
            0,
            gorse_storage::vectors::Distance::Dot,
            Default::default(),
        )
        .await
        .expect("add the collection");
    suite
        .master
        .vectors()
        .add_vectors(
            &ctx,
            &collection,
            &sparse_vectors(&[
                ("0", 0, 1.0),
                ("1", 0, 4.0),
                ("2", 0, 3.0),
                ("3", 0, 2.0),
                ("4", 0, 1.0),
            ]),
        )
        .await
        .expect("add the vectors");
    let users = [("1", 4), ("2", 3), ("3", 2), ("4", 1)];
    for (id, _) in users {
        suite
            .master
            .data()
            .batch_insert_users(
                &ctx,
                &[User {
                    user_id: id.to_owned(),
                    ..Default::default()
                }],
            )
            .await
            .expect("insert a user");
    }
    let reply = suite.get("/api/dashboard/user-to-user/neighbors/0/").await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected: Vec<Value> = users
        .iter()
        .map(|(id, score)| json!({"UserId": id, "Labels": null, "Comment": "", "Score": score}))
        .collect();
    assert_eq!(reply.json(), json!(expected));
    suite.close().await;
}

/// Sparse vectors with one index each, as the test fixtures of Go's vector tests give them.
fn sparse_vectors(rows: &[(&str, u32, f32)]) -> Vec<gorse_storage::vectors::Vector> {
    rows.iter()
        .map(|(id, index, value)| gorse_storage::vectors::Vector {
            id: (*id).to_owned(),
            indices: vec![*index],
            values: vec![*value],
            ..Default::default()
        })
        .collect()
}

/// A `ScoredItem` as the handlers write it: the item's fields, then its score.
fn scored_item(id: &str, categories: &[&str], score: i64) -> Value {
    // A nil slice in Go is `null`, and the scored items without categories have nil ones.
    let categories = if categories.is_empty() {
        Value::Null
    } else {
        json!(categories)
    };
    json!({
        "ItemId": id,
        "IsHidden": false,
        "Categories": categories,
        "Timestamp": "0001-01-01T00:00:00Z",
        "Labels": null,
        "Comment": "",
        "Score": score,
    })
}

#[tokio::test]
async fn feedback() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let item_ids = ["0", "2", "4", "6", "8"];
    let feedback: Vec<Feedback> = item_ids
        .iter()
        .map(|id| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: "0".to_owned(),
                item_id: (*id).to_owned(),
            },
            ..Default::default()
        })
        .collect();
    suite
        .master
        .data()
        .batch_insert_feedback(&ctx, &feedback, true, true, true)
        .await
        .expect("insert feedback");
    let reply = suite.get("/api/dashboard/user/0/feedback/click").await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected: Vec<Value> = item_ids
        .iter()
        .map(|id| {
            json!({
                "FeedbackType": "click",
                "UserId": "0",
                "Item": {"ItemId": id, "IsHidden": false, "Categories": null, "Timestamp": "0001-01-01T00:00:00Z", "Labels": null, "Comment": ""},
                "Value": 0,
                "Timestamp": "0001-01-01T00:00:00Z",
                "Comment": "",
            })
        })
        .collect();
    assert_eq!(reply.json(), json!(expected));
    suite.close().await;
}

#[tokio::test]
async fn get_recommends() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let recommended: Vec<gorse_storage::cache::Score> = (1..=8)
        .map(|i| gorse_storage::cache::Score {
            id: i.to_string(),
            score: 100.0 - f64::from(i),
            is_hidden: false,
            categories: vec![String::new()],
            timestamp: Default::default(),
        })
        .collect();
    suite
        .master
        .cache()
        .add_scores(&ctx, cache::RECOMMEND, "0", &recommended)
        .await
        .expect("add the recommendations");
    suite
        .master
        .data()
        .batch_insert_feedback(
            &ctx,
            &[
                Feedback {
                    feedback_key: FeedbackKey {
                        feedback_type: "a".to_owned(),
                        user_id: "0".to_owned(),
                        item_id: "2".to_owned(),
                    },
                    ..Default::default()
                },
                Feedback {
                    feedback_key: FeedbackKey {
                        feedback_type: "a".to_owned(),
                        user_id: "0".to_owned(),
                        item_id: "4".to_owned(),
                    },
                    ..Default::default()
                },
            ],
            true,
            true,
            true,
        )
        .await
        .expect("insert feedback");
    for score in &recommended {
        suite
            .master
            .data()
            .batch_insert_items(
                &ctx,
                &[Item {
                    item_id: score.id.clone(),
                    ..Default::default()
                }],
            )
            .await
            .expect("insert an item");
    }
    let reply = suite.get("/api/dashboard/recommend/0").await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected: Vec<Value> = [1, 3, 5, 6, 7, 8]
        .into_iter()
        .map(|i| scored_item(&i.to_string(), &[], 100 - i64::from(i)))
        .collect();
    assert_eq!(reply.json(), json!(expected));
    suite.close().await;
}

#[tokio::test]
async fn get_non_personalized_recommends() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let offline: Vec<gorse_storage::cache::Score> = [("1", 99.0), ("2", 98.0), ("3", 97.0)]
        .into_iter()
        .map(|(id, score)| gorse_storage::cache::Score {
            id: id.to_owned(),
            score,
            is_hidden: false,
            categories: vec![String::new()],
            timestamp: Default::default(),
        })
        .collect();
    suite
        .master
        .cache()
        .add_scores(&ctx, cache::RECOMMEND, "0", &offline)
        .await
        .expect("add the recommendations");
    let popular: Vec<gorse_storage::cache::Score> = [("10", 100.0), ("20", 99.0), ("30", 98.0)]
        .into_iter()
        .map(|(id, score)| gorse_storage::cache::Score {
            id: id.to_owned(),
            score,
            is_hidden: false,
            categories: vec![String::new()],
            timestamp: Default::default(),
        })
        .collect();
    suite
        .master
        .cache()
        .add_scores(&ctx, cache::NON_PERSONALIZED, "popular", &popular)
        .await
        .expect("add the popular items");
    let items = [
        ("1", 2020),
        ("2", 2021),
        ("3", 2022),
        ("10", 2020),
        ("20", 2021),
        ("30", 2022),
    ]
    .map(|(id, year)| Item {
        item_id: id.to_owned(),
        timestamp: utc(year_start(year) + 3661, 1),
        ..Default::default()
    });
    suite
        .master
        .data()
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert items");
    let reply = suite
        .get("/api/dashboard/recommend/0/non-personalized/popular")
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    let expected: Vec<Value> = [("10", 2020, 100), ("20", 2021, 99), ("30", 2022, 98)]
        .into_iter()
        .map(|(id, year, score)| {
            let mut item = scored_item(id, &[], score);
            item["Timestamp"] = json!(format!("{year}-01-01T01:01:01.000000001Z"));
            item
        })
        .collect();
    assert_eq!(reply.json(), json!(expected));
    suite.close().await;
}

/// The Unix time of January 1 of `year` at midnight UTC, for the years the tests use.
fn year_start(year: i64) -> i64 {
    match year {
        2020 => 1_577_836_800,
        2021 => 1_609_459_200,
        2022 => 1_640_995_200,
        2023 => 1_672_531_200,
        2024 => 1_704_067_200,
        2025 => 1_735_689_600,
        _ => panic!("no fixture for {year}"),
    }
}

#[tokio::test]
async fn get_external() {
    use base64::Engine as _;
    let suite = Suite::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let address = listener.local_addr().expect("a local address");
    let script_server = Router::new().route("/", get(external_user_items));
    tokio::spawn(async move {
        let _ = axum::serve(listener, script_server).await;
    });
    let script = format!("fetch(\"http://{address}?user_id=\" + user_id).body");
    let script_base64 = base64::engine::general_purpose::STANDARD.encode(script);
    let reply = suite
        .get(&format!(
            "/api/dashboard/external?script={}&user-id=1",
            query_escape(script_base64)
        ))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json(), json!(["x", "y"]));
    suite.close().await;
}

/// The script's server, as Go's `httptest` handler: `["x","y"]` for user `1`,
/// and not found for any other user.
async fn external_user_items(Query(query): Query<HashMap<String, String>>) -> Response {
    if query.get("user_id").map(String::as_str) == Some("1") {
        return "[\"x\",\"y\"]\n".into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

#[tokio::test]
async fn get_config_schema() {
    let suite = Suite::new().await;
    let reply = suite.get("/api/dashboard/config/schema").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    // The contract is the schema Go's reflector writes for the config struct.
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../contracts/config/config.schema.json"
    ))
    .expect("the schema contract");
    assert_eq!(reply.json(), schema);
    let properties = &schema["$defs"]["RecommendConfig"]["properties"];
    assert!(properties.get("cache_size").is_some());
    assert!(properties.get("non-personalized").is_some());
    assert!(properties.get("cacheSize").is_none());
    assert!(properties.get("nonPersonalized").is_none());
    suite.close().await;
}

#[tokio::test]
async fn get_timeseries() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let now = gorse_common::gotime::now();
    let points: Vec<cache::TimeSeriesPoint> = [(24, 1.0), (48, 2.0), (72, 3.0)]
        .into_iter()
        .map(|(hours, value)| cache::TimeSeriesPoint {
            name: "test_timeseries".to_owned(),
            timestamp: now.add(gorse_common::gotime::Duration(
                -hours * 3600 * gorse_common::gotime::SECOND.0,
            )),
            value,
        })
        .collect();
    suite
        .master
        .cache()
        .add_time_series_points(&ctx, &points)
        .await
        .expect("add the points");
    let reply = suite.get("/api/dashboard/timeseries/test_timeseries").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        reply.json().as_array().map(Vec::len),
        Some(3),
        "{}",
        reply.text()
    );
    suite.close().await;
}

#[tokio::test]
async fn chat_completions() {
    let suite = Suite::new().await;
    let content = "In my younger and more vulnerable years my father gave me some advice that I've been turning over in my mind ever since. \"Whenever you feel like criticizing any one,\" he told me, \" just remember that all the people in this world haven't had the advantages that you've had.\"";
    let body = json!({
        "model": "gpt-test",
        "messages": [{"role": "user", "content": content}],
    });
    let reply = suite
        .send(suite.request(
            "POST",
            "/api/chat/completions",
            Body::from(body.to_string()),
        ))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let chat = reply.json();
    let choices = chat["choices"].as_array().expect("the choices");
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0]["message"]["content"], json!(content));
    suite.close().await;
}

#[tokio::test]
async fn chat_completions_stream() {
    let suite = Suite::new().await;
    let content = "streaming chat completion proxy";
    let body = json!({
        "model": "gpt-test",
        "stream": true,
        "messages": [{"role": "user", "content": content}],
    });
    let reply = suite
        .send(suite.request(
            "POST",
            "/api/chat/completions",
            Body::from(body.to_string()),
        ))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header("content-type"), "text/event-stream");
    let text = reply.text();
    assert!(text.contains("data: "), "{text}");
    assert!(text.contains("data: [DONE]"), "{text}");
    let mut got = String::new();
    for event in text.split("\n\n") {
        let event = event.trim();
        if event.is_empty() || event == "data: [DONE]" {
            continue;
        }
        let payload = event.strip_prefix("data: ").expect("a data event");
        let chunk: Value = serde_json::from_str(payload).expect("a stream event");
        if let Some(delta) = chunk["choices"][0]["delta"]["content"].as_str() {
            got.push_str(delta);
        }
    }
    assert_eq!(got, content);
    suite.close().await;
}

#[tokio::test]
async fn purge() {
    let suite = Suite::new().await;
    let ctx = Context::background();
    let cache = suite.master.cache();
    let data = suite.master.data();
    cache
        .set(&ctx, &[cache::string("key", "value")])
        .await
        .expect("set the string");
    let text = cache.get(&ctx, "key").await.string().expect("the string");
    assert_eq!(text, "value");
    let scores: Vec<cache::Score> = [("a", 1.0), ("b", 2.0), ("c", 3.0)]
        .into_iter()
        .map(|(id, score)| cache::Score {
            id: id.to_owned(),
            score,
            is_hidden: false,
            categories: vec![String::new()],
            timestamp: Default::default(),
        })
        .collect();
    cache
        .add_scores(&ctx, "sorted", "", &scores)
        .await
        .expect("add the scores");
    let found = cache
        .search_scores(&ctx, "sorted", "", &[String::new()], 0, -1)
        .await
        .expect("the scores");
    let mut found: Vec<(String, f64)> = found.into_iter().map(|s| (s.id, s.score)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        found,
        vec![
            ("a".to_owned(), 1.0),
            ("b".to_owned(), 2.0),
            ("c".to_owned(), 3.0),
        ]
    );

    let feedback: Vec<Feedback> = (0..100)
        .map(|i| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: i.to_string(),
                item_id: i.to_string(),
            },
            ..Default::default()
        })
        .collect();
    data.batch_insert_feedback(&ctx, &feedback, true, true, true)
        .await
        .expect("insert the feedback");
    let (_, users) = data.get_users(&ctx, "", 100).await.expect("the users");
    assert_eq!(users.len(), 100);
    let (_, items) = data
        .get_items(&ctx, "", 100, None)
        .await
        .expect("the items");
    assert_eq!(items.len(), 100);
    let (_, found_feedback) = data
        .get_feedback(&ctx, "", 100, None, Some(gorse_common::gotime::now()), &[])
        .await
        .expect("the feedback");
    assert_eq!(found_feedback.len(), 100);

    let request = Request::builder()
        .method("POST")
        .uri("/api/purge")
        .header(header::COOKIE, &suite.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(
            "check_list=delete_users,delete_items,delete_feedback,delete_cache",
        ))
        .expect("a purge request");
    let reply = suite.send(request).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    assert!(!cache.get(&ctx, "key").await.exists());
    let remaining = cache
        .search_scores(&ctx, "sorted", "", &[String::new()], 0, -1)
        .await
        .expect("the scores");
    assert!(remaining.is_empty());
    let (_, users) = data.get_users(&ctx, "", 100).await.expect("the users");
    assert!(users.is_empty());
    let (_, items) = data
        .get_items(&ctx, "", 100, None)
        .await
        .expect("the items");
    assert!(items.is_empty());
    let (_, found_feedback) = data
        .get_feedback(&ctx, "", 100, None, Some(gorse_common::gotime::now()), &[])
        .await
        .expect("the feedback");
    assert!(found_feedback.is_empty());
    suite.close().await;
}

/// The JSON with every number as a float, so `0` and `0.0` compare equal: Go
/// writes a float64 zero as `0`, and the port's encoder keeps it a float.
fn float_numbers(value: Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64()),
        Value::Array(items) => Value::Array(items.into_iter().map(float_numbers).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, float_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}

#[tokio::test]
async fn config() {
    let suite = Suite::new().await;
    let mut cfg = suite.master.config().as_ref().clone();
    cfg.recommend.data_source.positive_feedback_types =
        Some(vec![must_parse_feedback_type_expression("a")]);
    cfg.recommend.data_source.read_feedback_types =
        Some(vec![must_parse_feedback_type_expression("b")]);
    cfg.recommend.ranker.recommenders = Some(vec!["latest".to_owned()]);
    suite.master.replace_config(cfg.clone());

    let reply = suite.get("/api/dashboard/config").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        float_numbers(reply.json()),
        float_numbers(Value::Object(config_map(&cfg)))
    );

    cfg.master.dashboard_redacted = true;
    suite.master.replace_config(cfg.clone());
    let mut redacted = config_map(&cfg);
    redacted.remove("database");
    let reply = suite.get("/api/dashboard/config").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        float_numbers(reply.json()),
        float_numbers(Value::Object(redacted))
    );
    cfg.master.dashboard_redacted = false;
    suite.master.replace_config(cfg.clone());

    // The posts send the configuration as the dashboard does: the map form of
    // the whole configuration, with the ranker type changed.
    let post = |body: &Value| {
        Request::builder()
            .method("POST")
            .uri("/api/dashboard/config")
            .header(header::COOKIE, &suite.cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("a config request")
    };
    cfg.recommend.ranker.kind = "reranker".to_owned();
    let reply = suite.send(post(&Value::Object(config_map(&cfg)))).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(suite.master.config().recommend.ranker.kind, "reranker");
    assert!(suite.scheduled());

    cfg.recommend.ranker.kind = "xxx".to_owned();
    let reply = suite.send(post(&Value::Object(config_map(&cfg)))).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(suite.master.config().recommend.ranker.kind, "reranker");

    let stored = cfg.recommend.marshal_json().expect("the recommend config");
    suite
        .master
        .meta_store()
        .put(meta::RECOMMEND_CONFIG, &stored)
        .await
        .expect("store the config");
    let reply = suite
        .send(suite.request("DELETE", "/api/dashboard/config", Body::empty()))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let value = suite
        .master
        .meta_store()
        .get(meta::RECOMMEND_CONFIG)
        .await
        .expect("read the config");
    assert!(value.is_none());
    suite.close().await;
}

#[tokio::test]
async fn get_ranker_prompt() {
    use base64::Engine as _;
    let suite = Suite::new().await;
    let ctx = Context::background();
    suite
        .master
        .data()
        .batch_insert_users(
            &ctx,
            &[User {
                user_id: "u1".to_owned(),
                ..Default::default()
            }],
        )
        .await
        .expect("insert the user");
    // The feedback items are hidden, as the Go test makes them.
    let feedback_items: Vec<Item> = (0..12)
        .map(|i| Item {
            item_id: format!("fb-{i:02}"),
            is_hidden: true,
            timestamp: utc(year_start(2020) + i, 0),
            ..Default::default()
        })
        .collect();
    let feedback: Vec<Feedback> = (0..12)
        .map(|i| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: "u1".to_owned(),
                item_id: format!("fb-{i:02}"),
            },
            timestamp: utc(year_start(2021) + i, 0),
            ..Default::default()
        })
        .collect();
    suite
        .master
        .data()
        .batch_insert_items(&ctx, &feedback_items)
        .await
        .expect("insert the feedback items");
    suite
        .master
        .data()
        .batch_insert_feedback(&ctx, &feedback, true, true, true)
        .await
        .expect("insert the feedback");
    let latest: Vec<Item> = (1..=3)
        .map(|i| Item {
            item_id: format!("lt-{i}"),
            is_hidden: false,
            timestamp: utc(year_start(2024) + i, 0),
            ..Default::default()
        })
        .collect();
    suite
        .master
        .data()
        .batch_insert_items(&ctx, &latest)
        .await
        .expect("insert the latest items");

    // The templates arrive as base64 text, as the dashboard sends them.
    let query_template = "user={{ user.UserId }}\nfeedback={% for f in feedback %}{{ f.Item.ItemId }}{% if not loop.last %}, {% endif %}{% endfor %}";
    let document_template = "item={{ item.ItemId }}";
    let encode = |text: &str| query_escape(base64::engine::general_purpose::STANDARD.encode(text));
    let reply = suite
        .get(&format!(
            "/api/dashboard/ranker/prompt?query-template={}&document-template={}&user-id=u1",
            encode(query_template),
            encode(document_template)
        ))
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    // The ten latest feedback items, newest first: fb-11 to fb-02.
    let feedback_ids: Vec<String> = (2..=11).rev().map(|i| format!("fb-{i:02}")).collect();
    assert_eq!(
        reply.json(),
        json!({
            "query": format!("user=u1\nfeedback={}", feedback_ids.join(", ")),
            "documents": ["item=lt-3", "item=lt-2", "item=lt-1"],
        })
    );
    suite.close().await;
}

/// Inserts one more record than a batch holds, so the batch boundary is crossed,
/// and returns them. Each record's id is its index, five digits wide, and its
/// labels are the odd and even numbers of that index.
async fn seeded_records(
    suite: &Suite,
    feedback_value: f64,
) -> (Vec<User>, Vec<Item>, Vec<Feedback>) {
    let ctx = Context::background();
    let users: Vec<User> = (0..BATCH_SIZE + 1)
        .map(|i| User {
            user_id: format!("{i:05}"),
            labels: labels(
                &json!({"a": (2 * i + 1).to_string(), "b": (2 * i + 2).to_string()}).to_string(),
            ),
            ..Default::default()
        })
        .collect();
    let items: Vec<Item> = (0..BATCH_SIZE + 1)
        .map(|i| Item {
            item_id: format!("{i:05}"),
            labels: labels(
                &json!({"a": (2 * i + 1).to_string(), "b": (2 * i + 2).to_string()}).to_string(),
            ),
            ..Default::default()
        })
        .collect();
    let feedback: Vec<Feedback> = (0..BATCH_SIZE + 1)
        .map(|i| Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: format!("{i:05}"),
                item_id: format!("{i:05}"),
            },
            value: feedback_value,
            ..Default::default()
        })
        .collect();
    let data = suite.master.data();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert users");
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert items");
    data.batch_insert_feedback(&ctx, &feedback, true, true, true)
        .await
        .expect("insert feedback");
    (users, items, feedback)
}

/// Checks the records the store holds against the ones the test wrote. The store
/// sets the update time of each user and item: it is checked to be set, then
/// cleared before the comparison, as the Go test does.
async fn check_records(suite: &Suite, users: Vec<User>, items: Vec<Item>, feedback: Vec<Feedback>) {
    let ctx = Context::background();
    let data = suite.master.data();
    let (_, mut returned_users) = data
        .get_users(&ctx, "", users.len() as i64)
        .await
        .expect("the users");
    assert_eq!(returned_users.len(), users.len());
    for user in &mut returned_users {
        assert_ne!(user.update_at, gorse_common::gotime::Time::default());
        user.update_at = gorse_common::gotime::Time::default();
    }
    assert_eq!(returned_users, users);

    let (_, mut returned_items) = data
        .get_items(&ctx, "", items.len() as i64, None)
        .await
        .expect("the items");
    assert_eq!(returned_items.len(), items.len());
    for item in &mut returned_items {
        assert_ne!(item.update_at, gorse_common::gotime::Time::default());
        item.update_at = gorse_common::gotime::Time::default();
    }
    assert_eq!(returned_items, items);

    let (_, returned_feedback) = data
        .get_feedback(
            &ctx,
            "",
            feedback.len() as i64,
            None,
            Some(gorse_common::gotime::now()),
            &[],
        )
        .await
        .expect("the feedback");
    assert_eq!(returned_feedback, feedback);
}

#[tokio::test]
async fn dump_and_restore() {
    let suite = Suite::new().await;
    let (users, items, feedback) = seeded_records(&suite, 0.0).await;

    let dump = suite.get("/api/dump").await;
    assert_eq!(dump.status, StatusCode::OK, "{}", dump.text());

    suite.master.data().purge().await.expect("purge the data");
    let restore = suite
        .send(
            Request::builder()
                .method("POST")
                .uri("/api/restore")
                .header(header::COOKIE, &suite.cookie)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(dump.body))
                .expect("a restore request"),
        )
        .await;
    assert_eq!(restore.status, StatusCode::OK, "{}", restore.text());

    check_records(&suite, users, items, feedback).await;
    assert!(suite.scheduled());
    suite.close().await;
}

#[tokio::test]
async fn export_and_import() {
    let suite = Suite::new().await;
    let (users, items, feedback) = seeded_records(&suite, 1.0).await;

    let exported: Vec<(&str, Reply)> = vec![
        ("/api/bulk/users", suite.get("/api/bulk/users").await),
        ("/api/bulk/items", suite.get("/api/bulk/items").await),
        ("/api/bulk/feedback", suite.get("/api/bulk/feedback").await),
    ];
    for (route, reply) in &exported {
        assert_eq!(reply.status, StatusCode::OK, "{route}: {}", reply.text());
    }

    suite.master.data().purge().await.expect("purge the data");
    for (route, reply) in exported {
        let (content_type, body) = multipart_file(&reply.text());
        let request = Request::builder()
            .method("POST")
            .uri(route)
            .header(header::COOKIE, &suite.cookie)
            .header(header::CONTENT_TYPE, content_type)
            .body(body)
            .expect("an import request");
        let imported = suite.send(request).await;
        assert_eq!(
            imported.status,
            StatusCode::OK,
            "{route}: {}",
            imported.text()
        );
    }

    check_records(&suite, users, items, feedback).await;
    suite.close().await;
}
