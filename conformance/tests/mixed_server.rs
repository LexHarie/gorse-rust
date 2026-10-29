//! The WP-4-2 mixed-server test. Two clusters run the same seeded stores: each
//! has a Go master and a Go worker. The reference cluster serves the REST API
//! from the Go server, and the mixed cluster serves it from the Rust server. The
//! same request goes to both, in the same order, and each reply must match in
//! its status, its content type, the CORS and request-id headers, and its body
//! text. Every route of `contracts/rest/server.routes.md` has at least one case.
//!
//! Each cluster has its own stores, so a write in one cluster does not change
//! what the other cluster reads. The reads come first, so the recommendations
//! are settled before any write.
//!
//! The test fails, and does not skip, when the Go toolchain is missing or a node
//! does not start.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use flate2::Compression;
use flate2::write::{GzEncoder, ZlibEncoder};
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_conformance::cluster::{Cluster, Edit, GoBuild};
use gorse_server::server::{Server, ServerError, new_server};
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use reqwest::{Client, Method, StatusCode};
use tokio::task::JoinHandle;

/// The users of a run. Each likes two items.
const USERS: usize = 10;

/// The items of a run, one minute apart, in the category `i % 3`.
const ITEMS: usize = 30;

/// How often the worker checks for work. The first check runs after one interval.
const TICK: Duration = Duration::from_millis(500);

/// How long one request may take before the comparison fails.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The config both clusters start from. No collaborative model and no ranker, so
/// the only candidates are the latest items, which the worker computes the same
/// way in both clusters.
const EDITS: [Edit<'static>; 4] = [
    ("recommend.collaborative", "type", "\"none\""),
    ("recommend.ranker", "type", "\"none\""),
    ("recommend.ranker", "recommenders", "[\"latest\"]"),
    ("recommend.fallback", "recommenders", "[\"latest\"]"),
];

/// Every route of `contracts/rest/server.routes.md`, as the contract writes it.
const ROUTES: [&str; 42] = [
    "DELETE /api/feedback/{feedback-type}/{user-id}/{item-id}",
    "DELETE /api/feedback/{user-id}/{item-id}",
    "DELETE /api/item/{item-id}/category/{category}",
    "DELETE /api/item/{item-id}",
    "DELETE /api/user/{user-id}",
    "GET /api/collaborative-filtering/{user-id}/{category}",
    "GET /api/collaborative-filtering/{user-id}",
    "GET /api/feedback/{feedback-type}/{user-id}/{item-id}",
    "GET /api/feedback/{feedback-type}",
    "GET /api/feedback/{user-id}/{item-id}",
    "GET /api/feedback",
    "GET /api/health/live",
    "GET /api/health/ready",
    "GET /api/item-to-item/{name}/{item-id}",
    "GET /api/item/{item-id}/feedback/",
    "GET /api/item/{item-id}/feedback/{feedback-type}",
    "GET /api/item/{item-id}/neighbors/",
    "GET /api/item/{item-id}/neighbors/{category}",
    "GET /api/item/{item-id}",
    "GET /api/items",
    "GET /api/latest/{category}",
    "GET /api/latest",
    "GET /api/non-personalized/{name}",
    "GET /api/recommend/{user-id}/{category}",
    "GET /api/recommend/{user-id}",
    "GET /api/user-to-user/{name}/{user-id}",
    "GET /api/user/{user-id}/feedback/{feedback-type}",
    "GET /api/user/{user-id}/feedback",
    "GET /api/user/{user-id}/neighbors/",
    "GET /api/user/{user-id}",
    "GET /api/users",
    "PATCH /api/item/{item-id}",
    "PATCH /api/user/{user-id}",
    "POST /api/feedback",
    "POST /api/item",
    "POST /api/items",
    "POST /api/session/recommend/{category}",
    "POST /api/session/recommend",
    "POST /api/user",
    "POST /api/users",
    "PUT /api/feedback",
    "PUT /api/item/{item-id}/category/{category}",
];

/// One request, sent to both servers.
struct Case {
    /// The contract route the request exercises, as [`ROUTES`] writes it.
    route: &'static str,
    method: Method,
    /// The path and the query, with the values filled in.
    path: &'static str,
    /// The body as sent, which is the text or the compressed bytes.
    body: Option<Vec<u8>>,
    /// The `Content-Type` header, when the request has one. A request with a body
    /// has the JSON type unless a case says otherwise.
    content_type: Option<&'static str>,
    /// The `Content-Encoding` header, when the body is compressed.
    content_encoding: Option<&'static str>,
}

/// The parts of a reply the comparison reads.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_type: Option<String>,
    cors: bool,
    request_id: bool,
    body: String,
}

/// The requests of the comparison, in the order they are sent. The reads come
/// first. The writes come after them, and each one changes the same stores in
/// both clusters.
fn cases() -> Vec<Case> {
    let session = r#"[{"FeedbackType":"like","UserId":"user-0","ItemId":"item-0","Value":1,"Timestamp":"2023-11-14T22:13:20Z","Labels":null,"Comment":""}]"#;
    vec![
        // Health.
        request(
            "GET /api/health/live",
            Method::GET,
            "/api/health/live",
            None,
        ),
        request(
            "GET /api/health/ready",
            Method::GET,
            "/api/health/ready",
            None,
        ),
        // Users.
        request("GET /api/users", Method::GET, "/api/users?n=100", None),
        request("GET /api/users", Method::GET, "/api/users?n=3", None),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/user-0",
            None,
        ),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/missing-user",
            None,
        ),
        request(
            "GET /api/user/{user-id}/feedback",
            Method::GET,
            "/api/user/user-0/feedback",
            None,
        ),
        request(
            "GET /api/user/{user-id}/feedback/{feedback-type}",
            Method::GET,
            "/api/user/user-0/feedback/like",
            None,
        ),
        request(
            "GET /api/user/{user-id}/neighbors/",
            Method::GET,
            "/api/user/user-0/neighbors/?n=3",
            None,
        ),
        // Items.
        request("GET /api/items", Method::GET, "/api/items?n=100", None),
        request(
            "GET /api/items",
            Method::GET,
            "/api/items?n=4&cursor=",
            None,
        ),
        request(
            "GET /api/items",
            Method::GET,
            "/api/items?q=item-1&n=5",
            None,
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/item-0",
            None,
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/missing-item",
            None,
        ),
        request(
            "GET /api/item/{item-id}/feedback/",
            Method::GET,
            "/api/item/item-0/feedback/",
            None,
        ),
        request(
            "GET /api/item/{item-id}/feedback/{feedback-type}",
            Method::GET,
            "/api/item/item-0/feedback/like",
            None,
        ),
        request(
            "GET /api/item/{item-id}/neighbors/",
            Method::GET,
            "/api/item/item-0/neighbors/?n=3",
            None,
        ),
        request(
            "GET /api/item/{item-id}/neighbors/{category}",
            Method::GET,
            "/api/item/item-0/neighbors/category-0?n=3",
            None,
        ),
        // Feedback.
        request(
            "GET /api/feedback",
            Method::GET,
            "/api/feedback?n=100",
            None,
        ),
        request(
            "GET /api/feedback/{feedback-type}",
            Method::GET,
            "/api/feedback/like?n=100",
            None,
        ),
        request(
            "GET /api/feedback/{user-id}/{item-id}",
            Method::GET,
            "/api/feedback/user-0/item-0",
            None,
        ),
        request(
            "GET /api/feedback/{feedback-type}/{user-id}/{item-id}",
            Method::GET,
            "/api/feedback/like/user-0/item-0",
            None,
        ),
        request(
            "GET /api/feedback/{feedback-type}/{user-id}/{item-id}",
            Method::GET,
            "/api/feedback/like/user-9/item-1",
            None,
        ),
        // Latest and recommendations.
        request("GET /api/latest", Method::GET, "/api/latest?n=3", None),
        request(
            "GET /api/latest",
            Method::GET,
            "/api/latest?category=category-1&n=3&user-id=user-0",
            None,
        ),
        request(
            "GET /api/latest/{category}",
            Method::GET,
            "/api/latest/category-0?n=3",
            None,
        ),
        request(
            "GET /api/latest/{category}",
            Method::GET,
            "/api/latest/category-0?n=3&offset=50",
            None,
        ),
        request(
            "GET /api/recommend/{user-id}",
            Method::GET,
            "/api/recommend/user-0?n=5",
            None,
        ),
        request(
            "GET /api/recommend/{user-id}",
            Method::GET,
            "/api/recommend/user-0?n=3&offset=10000",
            None,
        ),
        request(
            "GET /api/recommend/{user-id}",
            Method::GET,
            "/api/recommend/unknown-user?n=3",
            None,
        ),
        request(
            "GET /api/recommend/{user-id}/{category}",
            Method::GET,
            "/api/recommend/user-0/category-1?n=5",
            None,
        ),
        // Recommenders that this config does not enable.
        request(
            "GET /api/item-to-item/{name}/{item-id}",
            Method::GET,
            "/api/item-to-item/default/item-0?n=3",
            None,
        ),
        request(
            "GET /api/user-to-user/{name}/{user-id}",
            Method::GET,
            "/api/user-to-user/default/user-0?n=3",
            None,
        ),
        request(
            "GET /api/collaborative-filtering/{user-id}",
            Method::GET,
            "/api/collaborative-filtering/user-0?n=3",
            None,
        ),
        request(
            "GET /api/collaborative-filtering/{user-id}/{category}",
            Method::GET,
            "/api/collaborative-filtering/user-0/category-0?n=3",
            None,
        ),
        request(
            "GET /api/non-personalized/{name}",
            Method::GET,
            "/api/non-personalized/popular?n=3",
            None,
        ),
        request(
            "GET /api/non-personalized/{name}",
            Method::GET,
            "/api/non-personalized/popular?n=3&offset=50",
            None,
        ),
        request(
            "POST /api/session/recommend",
            Method::POST,
            "/api/session/recommend?n=3",
            Some(session),
        ),
        request(
            "POST /api/session/recommend/{category}",
            Method::POST,
            "/api/session/recommend/category-0?n=3",
            Some(session),
        ),
        // Writes. Each one changes the stores of its own cluster.
        request(
            "POST /api/user",
            Method::POST,
            "/api/user",
            Some(r#"{"UserId":"new-user","Labels":null,"Comment":""}"#),
        ),
        request(
            "POST /api/user",
            Method::POST,
            "/api/user",
            Some(r#"{"UserId":"#),
        ),
        request(
            "POST /api/users",
            Method::POST,
            "/api/users",
            Some(r#"[{"UserId":"new-user-2","Labels":["a"],"Comment":"two"}]"#),
        ),
        request(
            "PATCH /api/user/{user-id}",
            Method::PATCH,
            "/api/user/new-user",
            Some(r#"{"Comment":"patched"}"#),
        ),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/new-user",
            None,
        ),
        request(
            "DELETE /api/user/{user-id}",
            Method::DELETE,
            "/api/user/new-user-2",
            None,
        ),
        request(
            "POST /api/item",
            Method::POST,
            "/api/item",
            Some(
                r#"{"ItemId":"new-item","IsHidden":false,"Categories":["category-9"],"Timestamp":"2023-11-14T22:13:20Z","Labels":null,"Comment":""}"#,
            ),
        ),
        request(
            "POST /api/items",
            Method::POST,
            "/api/items",
            Some(
                r#"[{"ItemId":"new-item-2","IsHidden":false,"Categories":null,"Timestamp":"2023-11-14T22:13:20Z","Labels":{"rank":2},"Comment":""}]"#,
            ),
        ),
        request(
            "PATCH /api/item/{item-id}",
            Method::PATCH,
            "/api/item/new-item",
            Some(r#"{"IsHidden":true,"Comment":"hidden"}"#),
        ),
        request(
            "PUT /api/item/{item-id}/category/{category}",
            Method::PUT,
            "/api/item/new-item/category/category-8",
            None,
        ),
        request(
            "DELETE /api/item/{item-id}/category/{category}",
            Method::DELETE,
            "/api/item/new-item/category/category-9",
            None,
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/new-item",
            None,
        ),
        request(
            "DELETE /api/item/{item-id}",
            Method::DELETE,
            "/api/item/new-item-2",
            None,
        ),
        request(
            "POST /api/feedback",
            Method::POST,
            "/api/feedback",
            Some(
                r#"[{"FeedbackType":"like","UserId":"user-5","ItemId":"item-5","Value":1,"Timestamp":"2023-11-14T22:13:20Z","Labels":null,"Comment":""}]"#,
            ),
        ),
        request(
            "PUT /api/feedback",
            Method::PUT,
            "/api/feedback",
            Some(
                r#"[{"FeedbackType":"like","UserId":"user-5","ItemId":"item-5","Value":2,"Timestamp":"2023-11-14T22:13:20Z","Labels":null,"Comment":"overwrite"}]"#,
            ),
        ),
        request(
            "DELETE /api/feedback/{user-id}/{item-id}",
            Method::DELETE,
            "/api/feedback/user-0/item-0",
            None,
        ),
        request(
            "DELETE /api/feedback/{feedback-type}/{user-id}/{item-id}",
            Method::DELETE,
            "/api/feedback/like/user-1/item-3",
            None,
        ),
        // Bodies that Go refuses. The reply carries Go's text for each refusal, so
        // the Rust server must give the same text, status and headers.
        request(
            "POST /api/user",
            Method::POST,
            "/api/user",
            Some(r#"{"UserId":1}"#),
        ),
        request(
            "POST /api/user",
            Method::POST,
            "/api/user",
            Some(r#"{"UserId":"user-x",}"#),
        ),
        Case {
            content_type: None,
            ..request(
                "POST /api/user",
                Method::POST,
                "/api/user",
                Some(r#"{"UserId":"user-y"}"#),
            )
        },
        request(
            "PATCH /api/user/{user-id}",
            Method::PATCH,
            "/api/user/user-0",
            Some("  "),
        ),
        request(
            "PATCH /api/item/{item-id}",
            Method::PATCH,
            "/api/item/item-0",
            Some(r#"{"Timestamp":"2023-13-14T22:13:20Z"}"#),
        ),
        request(
            "POST /api/feedback",
            Method::POST,
            "/api/feedback",
            Some(r#"[{"FeedbackType":"like","UserId":"user-0","ItemId":"item-1","Value":1e400}]"#),
        ),
        request(
            "POST /api/items",
            Method::POST,
            "/api/items",
            Some(r#"[{"ItemId":"item-x","Categories":[1]}]"#),
        ),
        // Bodies in XML, gzip and deflate, and labels with numbers that a float64
        // cannot hold. Both clusters decode each body with the same reader, so the
        // replies match, error texts included. The reads after the writes show what
        // was stored.
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/user",
                Method::POST,
                "/api/user",
                Some(
                    "<User><UserId>xml-user</UserId><Labels><a>x</a></Labels><Comment>from xml &amp; more</Comment></User>",
                ),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/user",
                Method::POST,
                "/api/user",
                Some("<User><UserId>xml-user-2</UserId>"),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/user",
                Method::POST,
                "/api/user",
                Some("<User><UserId>a&bogus;</UserId></User>"),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/users",
                Method::POST,
                "/api/users",
                Some("<Users><UserId>xml-user-3</UserId><Comment>three</Comment></Users>"),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/items",
                Method::POST,
                "/api/items",
                Some(
                    "<Items><ItemId>item-xml</ItemId><IsHidden>true</IsHidden><Categories>1</Categories><Timestamp>2023-11-14T22:13:20Z</Timestamp><Comment>xml item</Comment></Items>",
                ),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "PATCH /api/user/{user-id}",
                Method::PATCH,
                "/api/user/xml-user",
                Some(
                    "<?xml version=\"1.0\"?><!-- c --><P><Comment><![CDATA[raw <x>]]></Comment></P>",
                ),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "PATCH /api/item/{item-id}",
                Method::PATCH,
                "/api/item/item-xml",
                Some("<P><IsHidden>maybe</IsHidden></P>"),
            )
        },
        Case {
            content_type: Some("application/xml"),
            ..request(
                "POST /api/feedback",
                Method::POST,
                "/api/feedback",
                Some(
                    "<F><FeedbackType>like</FeedbackType><UserId>user-2</UserId><ItemId>item-3</ItemId><Value>2.5</Value><Timestamp>2023-11-14T22:13:20Z</Timestamp></F>",
                ),
            )
        },
        encoded(
            "POST /api/user",
            Method::POST,
            "/api/user",
            gzip_body(r#"{"UserId":"gz-user","Labels":[1e400,2.50,1E+2],"Comment":"gz"}"#),
            "application/json",
            "gzip",
        ),
        encoded(
            "POST /api/user",
            Method::POST,
            "/api/user",
            zlib_body(r#"{"UserId":"zlib-user","Labels":["z"],"Comment":"zlib"}"#),
            "application/json",
            "deflate",
        ),
        encoded(
            "POST /api/user",
            Method::POST,
            "/api/user",
            b"not gzip at all".to_vec(),
            "application/json",
            "gzip",
        ),
        encoded(
            "POST /api/feedback",
            Method::POST,
            "/api/feedback",
            zlib_body(r#"[{"FeedbackType":"like","UserId":"user-3","ItemId":"item-4","Value":1}]"#),
            "application/json",
            "deflate",
        ),
        request(
            "POST /api/items",
            Method::POST,
            "/api/items",
            Some(r#"[{"ItemId":"item-num","Labels":[1e400,2.50,1E+2]}]"#),
        ),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/xml-user",
            None,
        ),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/gz-user",
            None,
        ),
        request(
            "GET /api/user/{user-id}",
            Method::GET,
            "/api/user/zlib-user",
            None,
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/item-xml",
            None,
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/item-num",
            None,
        ),
        request(
            "POST /api/items",
            Method::POST,
            "/api/items",
            Some(r#"[{"ItemId":"item-num2","Labels":[2.50,1E+2]}]"#),
        ),
        request(
            "GET /api/item/{item-id}",
            Method::GET,
            "/api/item/item-num2",
            None,
        ),
    ]
}

/// A case for `route`, sent with `method` to `path`, with an optional JSON body.
fn request(
    route: &'static str,
    method: Method,
    path: &'static str,
    body: Option<&'static str>,
) -> Case {
    Case {
        route,
        method,
        path,
        content_type: body.map(|_| "application/json"),
        body: body.map(|text| text.as_bytes().to_vec()),
        content_encoding: None,
    }
}

/// A case for `route` whose body is `body` as it is sent, with the content type
/// and the encoding the request carries.
fn encoded(
    route: &'static str,
    method: Method,
    path: &'static str,
    body: Vec<u8>,
    content_type: &'static str,
    content_encoding: &'static str,
) -> Case {
    Case {
        route,
        method,
        path,
        body: Some(body),
        content_type: Some(content_type),
        content_encoding: Some(content_encoding),
    }
}

/// The gzip stream of `text`, as a client sends a `Content-Encoding: gzip` body.
fn gzip_body(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(text.as_bytes())
        .expect("compress the body");
    encoder.finish().expect("finish the stream")
}

/// The zlib stream of `text`, as a client sends a `Content-Encoding: deflate` body.
fn zlib_body(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(text.as_bytes())
        .expect("compress the body");
    encoder.finish().expect("finish the stream")
}

/// The rows both clusters start from. The timestamps are fixed, so the latest
/// recommender orders the items the same way in both clusters.
fn seed() -> (Vec<User>, Vec<Item>, Vec<Feedback>) {
    const BASE: i64 = 1_700_000_000;
    let users = (0..USERS)
        .map(|u| User {
            user_id: format!("user-{u}"),
            ..Default::default()
        })
        .collect();
    let items = (0..ITEMS)
        .map(|i| Item {
            item_id: format!("item-{i}"),
            categories: Some(vec![format!("category-{}", i % 3)]),
            timestamp: gotime::unix(BASE + 60 * i as i64, 0),
            ..Default::default()
        })
        .collect();
    let feedback = (0..USERS)
        .flat_map(|u| {
            (0..2).map(move |k| {
                let item = (3 * u + 7 * k) % ITEMS;
                Feedback {
                    feedback_key: FeedbackKey {
                        feedback_type: "like".to_owned(),
                        user_id: format!("user-{u}"),
                        item_id: format!("item-{item}"),
                    },
                    value: 1.0,
                    timestamp: gotime::unix(BASE + 3_600 + 60 * (2 * u + k) as i64, 0),
                    ..Default::default()
                }
            })
        })
        .collect();
    (users, items, feedback)
}

/// Writes the rows of a cluster into its data store, then closes the store so
/// the master can open the file.
async fn seed_stores(cluster: &Cluster, users: &[User], items: &[Item], feedback: &[Feedback]) {
    let data = cluster.data_store().await.expect("open the data store");
    let ctx = Context::background();
    data.init().await.expect("init the data store");
    data.batch_insert_users(&ctx, users)
        .await
        .expect("insert the users");
    data.batch_insert_items(&ctx, items)
        .await
        .expect("insert the items");
    data.batch_insert_feedback(&ctx, feedback, false, false, false)
        .await
        .expect("insert the feedback");
    data.close().await.expect("close the data store");
}

/// Whether every user has a recommendation update time in the cache.
async fn all_recommended(cache: &dyn cache::Database, ids: &[String]) -> bool {
    let ctx = Context::background();
    for id in ids {
        let key = cache::key(&[cache::RECOMMEND_UPDATE_TIME, id.as_str()]);
        if !cache.get(&ctx, &key).await.exists() {
            return false;
        }
    }
    true
}

/// The Rust server of a cluster, serving in this process.
struct RustServer {
    server: Arc<Server>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl RustServer {
    /// Shuts the server down and waits until its serve loop returns.
    async fn stop(self) {
        self.server.shutdown();
        self.task
            .await
            .expect("join the Rust server")
            .expect("the Rust server stops cleanly");
    }
}

/// Starts the Rust server against the cluster's master, on the port the Go
/// server would take, and waits until its `/api/health/ready` answers 200.
async fn start_rust_server(cluster: &mut Cluster) -> RustServer {
    let server = new_server(
        "127.0.0.1",
        cluster.ports.master_grpc,
        "127.0.0.1",
        cluster.ports.server_http,
        None,
    )
    .expect("create the Rust server");
    let server = Arc::new(server);
    let serving = server.clone();
    let task = tokio::spawn(async move { serving.serve().await });
    let port = cluster.ports.server_http;
    cluster
        .wait_for("the Rust server to be ready", move || async move {
            http_ready(port).await
        })
        .await
        .expect("the Rust server is ready");
    RustServer { server, task }
}

/// Whether `GET /api/health/ready` on `port` answers 200.
async fn http_ready(port: u16) -> bool {
    match reqwest::get(format!("http://127.0.0.1:{port}/api/health/ready")).await {
        Ok(response) => response.status() == StatusCode::OK,
        Err(_) => false,
    }
}

/// One cluster of the comparison, with its server on `port`.
struct Side {
    cluster: Cluster,
    rust: Option<RustServer>,
    port: u16,
}

impl Side {
    /// Starts a cluster with its master, its server and its worker, and waits
    /// until the worker has recommended for every user.
    async fn start(build: &'static GoBuild, rust: bool) -> Side {
        let mut cluster = Cluster::new(build, &EDITS).expect("write the cluster config");
        let (users, items, feedback) = seed();
        seed_stores(&cluster, &users, &items, &feedback).await;
        cluster.start_master().await.expect("start the Go master");
        let rust = if rust {
            Some(start_rust_server(&mut cluster).await)
        } else {
            cluster
                .start_go_server()
                .await
                .expect("start the Go server");
            None
        };
        cluster
            .start_go_worker(TICK, 1)
            .await
            .expect("start the Go worker");

        let cache = cluster.cache_store().await.expect("open the cache store");
        let ids: Vec<String> = users.iter().map(|user| user.user_id.clone()).collect();
        let (cache_ref, ids_ref) = (&*cache, ids.as_slice());
        cluster
            .wait_for("every user to have a recommendation", move || async move {
                all_recommended(cache_ref, ids_ref).await
            })
            .await
            .expect("the worker recommends for every user");
        let port = cluster.ports.server_http;
        Side {
            cluster,
            rust,
            port,
        }
    }

    /// Stops the Rust server, if this side has one, and every Go node.
    async fn stop(mut self) {
        if let Some(rust) = self.rust.take() {
            rust.stop().await;
        }
        self.cluster.stop();
    }
}

/// Sends `case` to the server on `port` and reads the parts the comparison uses.
async fn send(client: &Client, port: u16, case: &Case) -> Reply {
    let url = format!("http://127.0.0.1:{port}{}", case.path);
    let mut request = client.request(case.method.clone(), url);
    if let Some(body) = &case.body {
        request = request.body(body.clone());
    }
    if let Some(content_type) = case.content_type {
        request = request.header("Content-Type", content_type);
    }
    if let Some(content_encoding) = case.content_encoding {
        request = request.header("Content-Encoding", content_encoding);
    }
    let response = request.send().await.expect("send the request");
    let status = response.status().as_u16();
    let headers = response.headers();
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cors = headers.contains_key("access-control-allow-origin");
    let request_id = headers.contains_key("x-request-id");
    let body = response.text().await.expect("read the body");
    Reply {
        status,
        content_type,
        cors,
        request_id,
        body,
    }
}

/// The ways a reply from the Rust server differs from the Go reply. Each entry
/// names the field and both values, so a difference can be read from the log.
fn differences(case: &Case, go: &Reply, rust: &Reply) -> Vec<String> {
    let mut found = Vec::new();
    if go.status != rust.status {
        found.push(format!("status: go {} rust {}", go.status, rust.status));
    }
    if go.content_type != rust.content_type {
        found.push(format!(
            "content type: go {:?} rust {:?}",
            go.content_type, rust.content_type
        ));
    }
    if go.cors != rust.cors {
        found.push(format!("CORS header: go {} rust {}", go.cors, rust.cors));
    }
    if go.request_id != rust.request_id {
        found.push(format!(
            "X-Request-ID header: go {} rust {}",
            go.request_id, rust.request_id
        ));
    }
    if go.body != rust.body {
        found.push(format!(
            "body: go {} rust {}",
            excerpt(&go.body),
            excerpt(&rust.body)
        ));
    }
    if found.is_empty() {
        return found;
    }
    let mut lines = vec![format!("{} {}", case.method, case.path)];
    lines.extend(found.into_iter().map(|line| format!("  {line}")));
    lines
}

/// The start of a body, for a failure message.
fn excerpt(body: &str) -> String {
    const LIMIT: usize = 240;
    if body.len() <= LIMIT {
        return format!("{body:?}");
    }
    let mut end = LIMIT;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}...", &body[..end])
}

/// The failure message: each route that differs, with its cases.
fn report(differing: &BTreeMap<&str, Vec<String>>) -> String {
    let mut out = format!("{} route(s) differ:\n", differing.len());
    for (route, lines) in differing {
        let _ = writeln!(out, "{route}");
        for line in lines {
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

#[tokio::test]
async fn mixed_server_matches_go_server() {
    let build = GoBuild::shared().expect("build the Go nodes");
    let cases = cases();

    let covered: BTreeSet<&str> = cases.iter().map(|case| case.route).collect();
    let routes: BTreeSet<&str> = ROUTES.into_iter().collect();
    assert_eq!(
        covered, routes,
        "every contract route needs a case, and every case names a contract route"
    );

    let go = Side::start(build, false).await;
    let rust = Side::start(build, true).await;
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("build the HTTP client");

    let mut differing: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for case in &cases {
        let expected = send(&client, go.port, case).await;
        let actual = send(&client, rust.port, case).await;
        let lines = differences(case, &expected, &actual);
        if !lines.is_empty() {
            differing.entry(case.route).or_default().extend(lines);
        }
    }

    go.stop().await;
    rust.stop().await;
    assert!(differing.is_empty(), "{}", report(&differing));
}
