// Ports: master/rest_test.go @ add21b5

//! The tests of the master's REST API, ported from Go's `MasterAPITestSuite`.
//!
//! Each test opens its own stores in a temporary directory, logs in with the
//! dashboard credentials as the Go suite does, and sends its requests through
//! the router. The mock OpenAI server runs on a thread of its own.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use futures_util::StreamExt as _;
use gorse_common::gocontext::Context;
use gorse_common::mock::OpenAiServer;
use gorse_config::config::defaults::get_default_config;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::Master;
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
