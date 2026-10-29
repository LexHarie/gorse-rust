//! The WP-4-4 acceptance test: the master's REST routes. The Go `gorse-master`
//! and the Rust master each serve the same routes over their own copy of one
//! seeded SQLite store, and every route must answer the same: its status, the
//! headers that matter, and its body.
//!
//! A JSON body is compared as JSON, with every number read as a float, because
//! Go writes a float zero as `0`. Any other body is compared byte for byte. The
//! dump is split at the statistics that follow its records: the records are
//! compared byte for byte, and the statistics as JSON without their `Duration`,
//! which is a wall-clock time. `Last-Modified` and `Set-Cookie` are compared by
//! presence only, since their values carry a time and a random key.
//!
//! The test fails, and does not skip, when the Go toolchain is missing or a node
//! does not start.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_common::gourl::query_escape;
use gorse_conformance::cluster::{Cluster, Edit, GoBuild};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use gorse_storage::meta::Node;
use reqwest::Method;
use serde_json::{Value, json};

/// The dashboard login every request after the first one uses.
const USER_NAME: &str = "admin";
const PASSWORD: &str = "pass";

/// The config both nodes start from: the dashboard login.
const EDITS: [Edit<'static>; 2] = [
    ("master", "dashboard_user_name", "\"admin\""),
    ("master", "dashboard_password", "\"pass\""),
];

/// The first timestamp of the seeded rows, in seconds. The rows are a minute apart,
/// so the latest items come out in one order in both runs.
const BASE: i64 = 1_700_000_000;

/// The boundary of the multipart body the import cases send.
const BOUNDARY: &str = "gorse-conformance-boundary";

/// The statistics that follow a dump's records, which the dump writes as JSON.
const STATS_MARKER: &[u8] = b"{\n \"Users\":";

/// The headers whose values both nodes must send alike.
const VALUE_HEADERS: [&str; 6] = [
    "content-type",
    "content-disposition",
    "access-control-allow-origin",
    "location",
    "cache-control",
    "expires",
];

/// The headers that must be present on both nodes, or on neither.
const PRESENCE_HEADERS: [&str; 2] = ["last-modified", "set-cookie"];

/// The answer to one request: its status, its headers, its body, and the cookies it sets.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// Each compared header's value, or `<absent>`. A presence header reads as
    /// `<present>` or `<absent>`.
    headers: BTreeMap<String, String>,
    /// The values of the presence headers as sent, for a failure message only.
    raw: BTreeMap<String, String>,
    body: Vec<u8>,
    /// The cookies the answer sets, as the `name=value` pairs a request sends back.
    cookie: String,
}

impl Reply {
    /// Reads a response into a reply.
    async fn read(response: reqwest::Response) -> Reply {
        let mut headers = BTreeMap::new();
        for name in VALUE_HEADERS {
            let value = response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<absent>");
            headers.insert(name.to_owned(), value.to_owned());
        }
        let mut raw = BTreeMap::new();
        for name in PRESENCE_HEADERS {
            let value = response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok());
            let marker = if value.is_some() {
                "<present>"
            } else {
                "<absent>"
            };
            headers.insert(name.to_owned(), marker.to_owned());
            raw.insert(name.to_owned(), value.unwrap_or("<absent>").to_owned());
        }
        let cookie = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let status = response.status().as_u16();
        let body = response.bytes().await.expect("the body is read").to_vec();
        Reply {
            status,
            headers,
            raw,
            body,
            cookie,
        }
    }

    /// The body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as JSON, when the answer says it is JSON.
    fn json(&self) -> Option<Value> {
        if !self.headers["content-type"].starts_with("application/json") {
            return None;
        }
        serde_json::from_slice(&self.body).ok()
    }
}

/// The JSON with every number as a float, so `0` and `0.0` compare equal.
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

/// The keys whose values are set per run: the stores' and the blob's paths, which
/// sit in a temporary directory, and the ports, which the harness picks free. The
/// config carries them, so both sides must have them, and their values are not
/// compared.
const RUN_KEYS: [&str; 12] = [
    "data_store",
    "cache_store",
    "vector_store",
    "uri",
    "port",
    "http_port",
    "DataStore",
    "CacheStore",
    "VectorStore",
    "URI",
    "Port",
    "HttpPort",
];

/// The times a run stamps with its own clock: a node's update time, a task's start
/// and finish, a model's fit, and the cache's update times. Each is replaced when
/// it is set, since the runs stamp it at different moments. A zero time is kept,
/// so a time that one run never set still differs from the other.
const CLOCK_KEYS: [&str; 7] = [
    "UpdateTime",
    "StartTime",
    "FinishTime",
    "MatchingModelFitTime",
    "RankingModelFitTime",
    "PopularItemsUpdateTime",
    "LatestItemsUpdateTime",
];

/// Go's zero time, as its JSON form writes it.
const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

/// Replaces the value of every per-run key with `<run>`.
fn mask_run_values(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                let clock = CLOCK_KEYS.contains(&key.as_str()) && field.as_str() != Some(ZERO_TIME);
                if RUN_KEYS.contains(&key.as_str()) || clock {
                    *field = json!("<run>");
                } else {
                    mask_run_values(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_run_values),
        _ => {}
    }
}

/// Collects the JSON pointers at which `go` and `rust` differ, in order. A value
/// that one side lacks is a difference at its pointer.
fn json_paths(at: &str, go: &Value, rust: &Value, found: &mut Vec<String>) {
    match (go, rust) {
        (Value::Object(go_fields), Value::Object(rust_fields)) => {
            for key in go_fields.keys().chain(rust_fields.keys()) {
                let pointer = format!("{at}/{key}");
                match (go_fields.get(key), rust_fields.get(key)) {
                    (Some(a), Some(b)) => json_paths(&pointer, a, b, found),
                    (a, b) => found.push(format!("{pointer}: {a:?} from Go, {b:?} from Rust")),
                }
            }
        }
        (Value::Array(go_items), Value::Array(rust_items))
            if go_items.len() == rust_items.len() =>
        {
            for (i, (a, b)) in go_items.iter().zip(rust_items).enumerate() {
                json_paths(&format!("{at}/{i}"), a, b, found);
            }
        }
        _ if go != rust => found.push(format!("{at}: {go} from Go, {rust} from Rust")),
        _ => {}
    }
}

/// The last bytes of a body, for a failure message.
fn tail(body: &[u8]) -> String {
    let start = body.len().saturating_sub(160);
    String::from_utf8_lossy(&body[start..]).into_owned()
}

/// The text of a body, cut short, for a failure message.
fn preview(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    text.chars().take(200).collect()
}

/// Splits a dump at its statistics. The records come first, and the statistics
/// are the JSON after the last [`STATS_MARKER`].
fn split_dump(body: &[u8]) -> Option<(&[u8], Value)> {
    let at = body
        .windows(STATS_MARKER.len())
        .rposition(|window| window == STATS_MARKER)?;
    let stats = serde_json::from_slice(&body[at..]).ok()?;
    Some((&body[..at], stats))
}

/// The differences between the two nodes' answers to the request `name`.
fn differences(name: &str, go: &Reply, rust: &Reply) -> Vec<String> {
    let mut found = Vec::new();
    if go.status != rust.status {
        found.push(format!(
            "{name}: status {} from Go, {} from Rust",
            go.status, rust.status
        ));
    }
    for (header, value) in &go.headers {
        let other = rust
            .headers
            .get(header)
            .map(String::as_str)
            .unwrap_or("<missing>");
        if value != other {
            let sent = match (go.raw.get(header), rust.raw.get(header)) {
                (Some(go_sent), Some(rust_sent)) => {
                    format!(" (Go sent {go_sent:?}, Rust sent {rust_sent:?})")
                }
                _ => String::new(),
            };
            found.push(format!(
                "{name}: header {header} is {value:?} from Go, {other:?} from Rust{sent}"
            ));
        }
    }
    if name == "dump" {
        match (split_dump(&go.body), split_dump(&rust.body)) {
            (Some((go_records, mut go_stats)), Some((rust_records, mut rust_stats))) => {
                if go_records != rust_records {
                    let at = go_records
                        .iter()
                        .zip(rust_records)
                        .position(|(a, b)| a != b)
                        .unwrap_or(go_records.len().min(rust_records.len()));
                    found.push(format!(
                        "{name}: the records differ at byte {at} (lengths {} and {}): {:?} from Go, {:?} from Rust",
                        go_records.len(),
                        rust_records.len(),
                        preview(&go_records[at..]),
                        preview(&rust_records[at.min(rust_records.len())..]),
                    ));
                }
                // The duration is a wall-clock time, which the contracts mark as run-dependent.
                for stats in [&mut go_stats, &mut rust_stats] {
                    stats
                        .as_object_mut()
                        .map(|fields| fields.remove("Duration"));
                }
                if go_stats != rust_stats {
                    found.push(format!(
                        "{name}: the statistics differ: {go_stats} from Go, {rust_stats} from Rust"
                    ));
                }
            }
            _ => found.push(format!(
                "{name}: a dump has no statistics: {:?} from Go, {:?} from Rust",
                tail(&go.body),
                tail(&rust.body)
            )),
        }
        return found;
    }
    match (go.json(), rust.json()) {
        (Some(go_json), Some(rust_json)) => {
            let mut go_json = float_numbers(go_json);
            let mut rust_json = float_numbers(rust_json);
            mask_run_values(&mut go_json);
            mask_run_values(&mut rust_json);
            let mut paths = Vec::new();
            json_paths("", &go_json, &rust_json, &mut paths);
            for path in paths.iter().take(40) {
                found.push(format!("{name}: JSON differs at {path}"));
            }
        }
        _ => {
            if go.body != rust.body {
                found.push(format!(
                    "{name}: body differs: {:?} from Go, {:?} from Rust",
                    preview(&go.body),
                    preview(&rust.body)
                ));
            }
        }
    }
    found
}

/// Writes the rows both runs start from: three users, three items in two
/// categories, and two likes for each user.
async fn seed(cluster: &Cluster) {
    let ctx = Context::background();
    let store = cluster.data_store().await.expect("open the data store");
    store.init().await.expect("init the data store");
    let users: Vec<User> = (0..3)
        .map(|u| User {
            user_id: format!("user-{u}"),
            labels: serde_json::from_value(json!({"gender": "male"})).expect("labels"),
            comment: format!("user {u}"),
            ..Default::default()
        })
        .collect();
    let items: Vec<Item> = (0..3)
        .map(|i| Item {
            item_id: format!("item-{i}"),
            categories: Some(vec![format!("category-{}", i % 2)]),
            timestamp: gotime::unix(BASE + 60 * i, 0),
            comment: format!("item {i}"),
            ..Default::default()
        })
        .collect();
    let feedback: Vec<Feedback> = (0..3)
        .flat_map(|u| {
            [u, (u + 1) % 3].into_iter().map(move |i| Feedback {
                feedback_key: FeedbackKey {
                    feedback_type: "like".to_owned(),
                    user_id: format!("user-{u}"),
                    item_id: format!("item-{i}"),
                },
                value: 1.0,
                timestamp: gotime::unix(BASE + 3600 + 60 * i, 0),
                ..Default::default()
            })
        })
        .collect();
    store
        .batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");
    store
        .batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");
    store
        .batch_insert_feedback(&ctx, &feedback, false, false, false)
        .await
        .expect("insert the feedback");
    store.close().await.expect("close the data store");

    // One server and one worker, so the cluster, the stats and the tasks routes
    // count and list nodes. Both are stamped now, since the store lists the nodes
    // that updated lately.
    let meta = cluster.meta_store().await.expect("open the meta store");
    let now = gotime::now().utc();
    for (uuid, node_type) in [("alan turnin", "Server"), ("dennis ritchie", "Worker")] {
        meta.update_node(&Node {
            uuid: uuid.to_owned(),
            hostname: "192.168.1.100".to_owned(),
            r#type: node_type.to_owned(),
            version: "interop".to_owned(),
            update_time: now.clone(),
        })
        .await
        .expect("register the node");
    }
    meta.close().await.expect("close the meta store");
}

/// Sends one request to `base`, with `cookie` when the request is logged in.
async fn send(
    client: &reqwest::Client,
    base: &str,
    cookie: Option<&str>,
    method: Method,
    path: &str,
    body: Option<(String, Vec<u8>)>,
) -> Reply {
    let mut request = client.request(method, format!("{base}{path}"));
    if let Some(cookie) = cookie {
        request = request.header(reqwest::header::COOKIE, cookie);
    }
    if let Some((content_type, bytes)) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes);
    }
    let response = request.send().await.expect("the node answers");
    Reply::read(response).await
}

/// A `multipart/form-data` body with one `file` field holding `content`, and
/// the content type that goes with it.
fn multipart(content: &str) -> (String, Vec<u8>) {
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"file.jsonl\"\r\nContent-Type: application/octet-stream\r\n\r\n{content}\r\n--{BOUNDARY}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        body.into_bytes(),
    )
}

/// A form body of `text`.
fn form(text: &str) -> Option<(String, Vec<u8>)> {
    Some((
        "application/x-www-form-urlencoded".to_owned(),
        text.as_bytes().to_vec(),
    ))
}

/// Waits until the master's background tasks have all completed. The click-through
/// optimisation is the last task of a run, so the wait ends once it is listed and
/// every listed task is complete. The stats and the tasks are read after that, so
/// both runs show the same finished work.
async fn wait_for_tasks(client: &reqwest::Client, base: &str, cookie: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let reply = send(
            client,
            base,
            Some(cookie),
            Method::GET,
            "/api/dashboard/tasks",
            None,
        )
        .await;
        let settled = reply
            .json()
            .and_then(|tasks| {
                tasks.as_array().map(|items| {
                    items
                        .iter()
                        .any(|task| task["Name"] == "Optimize Click-Through Rate Prediction Model")
                        && items.iter().all(|task| task["Status"] == "Complete")
                })
            })
            .unwrap_or(false);
        if settled {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{base} never completed its tasks: {}",
            reply.text()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits until the master has written the item categories to its cache. Both
/// masters start a dataset load when they start, and that load writes the
/// categories before anything else the test reads, so the cases wait for them.
async fn wait_for_categories(client: &reqwest::Client, base: &str, cookie: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let reply = send(
            client,
            base,
            Some(cookie),
            Method::GET,
            "/api/dashboard/categories",
            None,
        )
        .await;
        let loaded = reply
            .json()
            .and_then(|categories| categories.as_array().map(|items| !items.is_empty()))
            .unwrap_or(false);
        if loaded {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{base} never wrote the item categories: {}",
            reply.text()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Sends the routes of the test to the master at `base`, in order, and returns
/// each request's name with its answer. The first request logs in, and the rest
/// use its cookie. The config is read, changed and posted back, so the post
/// carries the node's own configuration.
async fn exercise(base: &str) -> Vec<(&'static str, Reply)> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the HTTP client");
    let mut replies = Vec::new();

    let login = send(
        &client,
        base,
        None,
        Method::POST,
        "/login",
        form(&format!("user_name={USER_NAME}&password={PASSWORD}")),
    )
    .await;
    let cookie = login.cookie.clone();
    wait_for_categories(&client, base, &cookie).await;
    wait_for_tasks(&client, base, &cookie).await;
    replies.push(("login", login));
    replies.push((
        "login with a wrong password",
        send(
            &client,
            base,
            None,
            Method::POST,
            "/login",
            form(&format!("user_name={USER_NAME}&password=wrong")),
        )
        .await,
    ));
    replies.push((
        "dashboard page without login",
        send(&client, base, None, Method::GET, "/", None).await,
    ));
    replies.push((
        "dashboard page",
        send(&client, base, Some(&cookie), Method::GET, "/", None).await,
    ));
    replies.push((
        "dashboard index.html redirect",
        send(
            &client,
            base,
            Some(&cookie),
            Method::GET,
            "/index.html",
            None,
        )
        .await,
    ));
    replies.push((
        "dashboard stylesheet",
        send(
            &client,
            base,
            Some(&cookie),
            Method::GET,
            "/css/app.4a0f7911.css",
            None,
        )
        .await,
    ));
    replies.push((
        "dashboard unknown file",
        send(
            &client,
            base,
            Some(&cookie),
            Method::GET,
            "/no-such-file.js",
            None,
        )
        .await,
    ));
    replies.push((
        "unknown dashboard route",
        send(
            &client,
            base,
            Some(&cookie),
            Method::GET,
            "/api/dashboard/no-such-route",
            None,
        )
        .await,
    ));
    replies.push((
        "dashboard route without login",
        send(
            &client,
            base,
            None,
            Method::GET,
            "/api/dashboard/latest",
            None,
        )
        .await,
    ));
    replies.push((
        "unknown dashboard route without login",
        send(
            &client,
            base,
            None,
            Method::GET,
            "/api/dashboard/no-such-route",
            None,
        )
        .await,
    ));

    for (name, path) in [
        ("userinfo", "/api/dashboard/userinfo"),
        ("categories", "/api/dashboard/categories"),
        ("users", "/api/dashboard/users?n=10"),
        ("user", "/api/dashboard/user/user-1"),
        ("user feedback", "/api/dashboard/user/user-1/feedback"),
        ("missing user", "/api/dashboard/user/no-such-user"),
        ("latest items", "/api/dashboard/latest?n=2"),
        (
            "empty non-personalized",
            "/api/dashboard/non-personalized/popular/",
        ),
        (
            "empty timeseries",
            "/api/dashboard/timeseries/no-such-series",
        ),
        ("config schema", "/api/dashboard/config/schema"),
        ("cluster", "/api/dashboard/cluster"),
        ("stats", "/api/dashboard/stats"),
        ("tasks", "/api/dashboard/tasks"),
        // The default item-to-item recommender is by embeddings, which this run
        // does not have, so its collection is missing and Go answers that error.
        (
            "item-to-item without its collection",
            "/api/dashboard/item-to-item/neighbors/item-0?n=2&offset=10",
        ),
        // The user-to-user recommender is by items and has its collection. An
        // offset past its last neighbour leaves no result, which Go's nil list
        // answers as an empty list, because the handler builds its answer with make.
        (
            "user-to-user past the end",
            "/api/dashboard/user-to-user/neighbors/user-0?n=2&offset=10",
        ),
    ] {
        replies.push((
            name,
            send(&client, base, Some(&cookie), Method::GET, path, None).await,
        ));
    }

    let query_template = "user={{ user.UserId }}\nfeedback={% for f in feedback %}{{ f.Item.ItemId }}{% if not loop.last %}, {% endif %}{% endfor %}";
    let document_template = "item={{ item.ItemId }}";
    let encode = |text: &str| query_escape(base64::engine::general_purpose::STANDARD.encode(text));
    let prompt = format!(
        "/api/dashboard/ranker/prompt?query-template={}&document-template={}&user-id=user-0",
        encode(query_template),
        encode(document_template)
    );
    replies.push((
        "ranker prompt",
        send(&client, base, Some(&cookie), Method::GET, &prompt, None).await,
    ));

    let config = send(
        &client,
        base,
        Some(&cookie),
        Method::GET,
        "/api/dashboard/config",
        None,
    )
    .await;
    let mut updated: Value = serde_json::from_slice(&config.body).unwrap_or(Value::Null);
    updated["recommend"]["cache_size"] = json!(50);
    replies.push(("config", config));
    replies.push((
        "update config",
        send(
            &client,
            base,
            Some(&cookie),
            Method::POST,
            "/api/dashboard/config",
            Some((
                "application/json".to_owned(),
                updated.to_string().into_bytes(),
            )),
        )
        .await,
    ));

    for (name, body) in [
        ("update config with bad json", "{\"recommend\": x}"),
        ("update config without a value", ""),
    ] {
        replies.push((
            name,
            send(
                &client,
                base,
                Some(&cookie),
                Method::POST,
                "/api/dashboard/config",
                Some(("application/json".to_owned(), body.as_bytes().to_vec())),
            )
            .await,
        ));
    }

    for (name, path) in [
        ("export users", "/api/bulk/users"),
        ("export items", "/api/bulk/items"),
        ("export feedback", "/api/bulk/feedback"),
    ] {
        replies.push((
            name,
            send(&client, base, Some(&cookie), Method::GET, path, None).await,
        ));
    }
    replies.push((
        "dump",
        send(&client, base, Some(&cookie), Method::GET, "/api/dump", None).await,
    ));
    replies.push((
        "purge with a partial check list",
        send(
            &client,
            base,
            Some(&cookie),
            Method::POST,
            "/api/purge",
            form("check_list=delete_users"),
        )
        .await,
    ));
    replies.push((
        "purge with GET",
        send(
            &client,
            base,
            Some(&cookie),
            Method::GET,
            "/api/purge",
            None,
        )
        .await,
    ));
    replies.push((
        "purge without login",
        send(
            &client,
            base,
            None,
            Method::POST,
            "/api/purge",
            form("check_list=delete_users,delete_items,delete_feedback,delete_cache"),
        )
        .await,
    ));
    replies.push((
        "import users",
        send(
            &client,
            base,
            Some(&cookie),
            Method::POST,
            "/api/bulk/users",
            Some(multipart(
                "{\"UserId\":\"import-1\",\"Labels\":{\"gender\":\"female\"},\"Comment\":\"\"}\n",
            )),
        )
        .await,
    ));
    replies.push((
        "logout",
        send(&client, base, Some(&cookie), Method::GET, "/logout", None).await,
    ));
    replies
}

/// Serves `router` on a free local port, and returns the port and the task that serves it.
async fn serve(router: axum::Router) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port");
    let port = listener.local_addr().expect("the local address").port();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("the server runs");
    });
    (port, task)
}

#[tokio::test]
async fn master_rest_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");

    let mut go = Cluster::new(build, &EDITS).expect("a Go run");
    seed(&go).await;
    go.start_master().await.expect("the Go master starts");
    let go_base = format!("http://127.0.0.1:{}", go.ports.master_http);
    let go_replies = exercise(&go_base).await;
    go.stop();

    let mut rust = Cluster::new(build, &EDITS).expect("a Rust run");
    seed(&rust).await;
    let master = rust
        .start_rust_master()
        .await
        .expect("the Rust master starts");
    let (port, server) = serve(master.router()).await;
    let rust_replies = exercise(&format!("http://127.0.0.1:{port}")).await;
    server.abort();
    master.stop().await;

    assert_eq!(
        go_replies.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        rust_replies
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        "the two runs ask the same requests"
    );
    // Two cases must be the answers they are named for, not an error that both
    // masters happen to share: a missing collection, and Go's empty list.
    for (name, reply) in &go_replies {
        let (status, body) = match *name {
            "item-to-item without its collection" => {
                (500, "collection item_to_item_neighbors: not found")
            }
            "user-to-user past the end" => (200, "[]"),
            _ => continue,
        };
        assert_eq!(
            (reply.status, reply.text()),
            (status, body.to_owned()),
            "{name}: Go's answer"
        );
    }
    let found: Vec<String> = go_replies
        .iter()
        .zip(&rust_replies)
        .flat_map(|((name, go), (_, rust))| differences(name, go, rust))
        .collect();
    assert!(
        found.is_empty(),
        "the Go and Rust masters answer differently:\n{}",
        found.join("\n")
    );
}

/// A master with no node registered lists no node. Go's list is nil there, and a
/// nil list is `null`, so the cluster route must answer `null` in both masters.
#[tokio::test]
async fn empty_cluster_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");

    let mut go = Cluster::new(build, &EDITS).expect("a Go run");
    go.start_master().await.expect("the Go master starts");
    let go_replies = empty_cluster(&format!("http://127.0.0.1:{}", go.ports.master_http)).await;
    go.stop();

    let mut rust = Cluster::new(build, &EDITS).expect("a Rust run");
    let master = rust
        .start_rust_master()
        .await
        .expect("the Rust master starts");
    let (port, server) = serve(master.router()).await;
    let rust_replies = empty_cluster(&format!("http://127.0.0.1:{port}")).await;
    server.abort();
    master.stop().await;

    let found: Vec<String> = go_replies
        .iter()
        .zip(&rust_replies)
        .flat_map(|((name, go), (_, rust))| differences(name, go, rust))
        .collect();
    assert!(
        found.is_empty(),
        "the Go and Rust masters answer differently:\n{}",
        found.join("\n")
    );
}

/// Logs in to the master at `base`, and asks it for `path` with the cookie.
async fn ask_logged_in(base: &str, path: &str) -> Reply {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the HTTP client");
    let login = send(
        &client,
        base,
        None,
        Method::POST,
        "/login",
        form(&format!("user_name={USER_NAME}&password={PASSWORD}")),
    )
    .await;
    let cookie = login.cookie.clone();
    send(&client, base, Some(&cookie), Method::GET, path, None).await
}

/// Asks a master with no node registered for its cluster.
async fn empty_cluster(base: &str) -> Vec<(&'static str, Reply)> {
    let cluster = ask_logged_in(base, "/api/dashboard/cluster").await;
    assert_eq!(cluster.status, 200, "{base} answers the cluster route");
    assert_eq!(cluster.text(), "null", "{base} lists no node");
    vec![("empty cluster", cluster)]
}

/// A script that returns the string `null` gives an external recommender with no
/// items. The preview route answers Go's nil list as `null`.
#[tokio::test]
async fn null_external_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    let script = base64::engine::general_purpose::STANDARD.encode("'null'");
    let path = format!(
        "/api/dashboard/external?script={}&user-id=user-0",
        query_escape(&script)
    );

    let mut go = Cluster::new(build, &EDITS).expect("a Go run");
    go.start_master().await.expect("the Go master starts");
    let go_reply =
        ask_logged_in(&format!("http://127.0.0.1:{}", go.ports.master_http), &path).await;
    go.stop();

    let mut rust = Cluster::new(build, &EDITS).expect("a Rust run");
    let master = rust
        .start_rust_master()
        .await
        .expect("the Rust master starts");
    let (port, server) = serve(master.router()).await;
    let rust_reply = ask_logged_in(&format!("http://127.0.0.1:{port}"), &path).await;
    server.abort();
    master.stop().await;

    // The answer must be Go's `null`, not an error that both masters share.
    assert_eq!(
        (go_reply.status, go_reply.text()),
        (200, "null".to_owned()),
        "Go answers the null list"
    );
    let found = differences("external", &go_reply, &rust_reply);
    assert!(
        found.is_empty(),
        "the Go and Rust masters answer differently:\n{}",
        found.join("\n")
    );
}
