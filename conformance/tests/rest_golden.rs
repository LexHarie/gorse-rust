// Ports: master/rest.go and server/rest.go @ add21b5 (WP-5-1).

//! The WP-5-1 golden test: every REST route of the two route tables, 107 pairs in
//! all. `contracts/rest/master.routes.md` lists the 65 routes of the master node,
//! and `contracts/rest/server.routes.md` the 42 routes of the server node. Each row
//! of a table is one request, built from the row's own parameter list and sent to
//! the node that the table describes.
//!
//! Two clusters are seeded the way `client/setup-test.sh` seeds the SDK suite: the
//! config of the setup (`client/config.toml`), the MovieLens 100k dump restored
//! through `/api/restore`, a restart of the master, and a wait until the master
//! has trained its collaborative filtering model. One cluster is all Go, the
//! other all Rust. The requests go to both, in the same order, and every answer is
//! compared in its status, its `Content-Type` and its body. Reads come first, so
//! the answers are settled before any write. A JSON body is compared as JSON, with
//! numbers compared as floats. A text body is compared as text.
//!
//! Normalisation. Two kinds of value differ from run to run, and both sides are
//! normalised by the same rules before they are compared:
//!
//! - N1: a timestamp (RFC 3339, with or without fractional seconds) becomes
//!   `<time>`, in any string or text body.
//! - N2: a UUID, or a 32-digit hex id without dashes, becomes `<uuid>`, in any
//!   string or text body.
//! - N3: the directory of a run (its stores, its blob store and its logs) becomes
//!   `<run>`, since each run has its own temporary directory.
//! - N4: the port numbers of a run become `<port>` in the fields that hold them in
//!   the config dump, since each run listens on its own free ports.
//!
//! Nothing else is normalised. A difference that is not explained by an entry of
//! [`EXPLAINED`] fails the test. An entry names the route, the field and the JSON
//! path of the value that differs, and carries the id of its note in
//! `porting/notes/WP-5-1.md`. A number is covered only within the entry's
//! tolerance, by the rule of D-234 in `PORTING.md`:
//! `|rust - go| <= tolerance * max(|go|, 1)`. An entry that matches no difference in a run is reported in
//! the log, not failed: a progress count or a version can match on one run and not
//! on the next.
//!
//! The test fails, and does not skip, when the Go toolchain is missing, a node does
//! not start, or the MovieLens 100k dump is not in the datasets folder.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use gorse_common::gocontext::Context;
use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};
use gorse_conformance::schema::{Result, fail, repo_root};
use gorse_storage::cache;
use regex::Regex;
use reqwest::Method;
use serde_json::Value;

/// The route tables, as `contracts/tools/openapi` writes them.
const MASTER_TABLE: &str = "contracts/rest/master.routes.md";
const SERVER_TABLE: &str = "contracts/rest/server.routes.md";

/// The rows each table must hold. The header of each table names the same count.
const MASTER_ROUTES: usize = 65;
const SERVER_ROUTES: usize = 42;

/// The request pairs of both tables. A missing row fails the count.
const ROUTE_PAIRS: usize = MASTER_ROUTES + SERVER_ROUTES;

/// The archive of the MovieLens 100k dump, as the datasets folder keeps it. The
/// environment variable `GORSE_ML_100K_DUMP` names another copy. The same archive
/// seeds the SDK suite (`tests/mixed_cluster.rs`).
const DUMP_PATH: &str = "/mnt/project-files/datasets/ml-100k.bin.gz";

/// The counts the restore must report for the MovieLens 100k dump.
const RESTORED_USERS: u64 = 943;
const RESTORED_ITEMS: u64 = 1682;
const RESTORED_FEEDBACK: u64 = 100_000;

/// The task that trains the collaborative filtering model, as the master names it.
const TRAINING_TASK: &str = "Train Collaborative Filtering Model";

/// How long the seeding of one cluster may take: the restore, the training and
/// every user's recommendation. Go's worker needs minutes for the 943 users.
// A time budget, not an assertion. The Rust nodes are debug builds, and the Rust
// worker's first pass over the 943 users runs at about 1.2 users a second, so the
// recommendations settle after about 13 minutes. The comparisons are unchanged.
const SEED_TIMEOUT: Duration = Duration::from_secs(1800);

/// How long the set of recommended users must stay the same before the requests
/// are sent: two worker intervals, so the worker has passed over every user at
/// least twice since its last change.
const SETTLE: Duration = Duration::from_secs(120);

/// How often the master's tasks and the cache are read while a cluster seeds.
const POLL: Duration = Duration::from_millis(250);

/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a worker checks for work. The setup leaves it at the worker's default.
const WORKER_INTERVAL: Duration = Duration::from_secs(60);

/// The jobs of each worker, the worker's default.
const WORKER_JOBS: usize = 1;

/// The users the worker must recommend for before a run's requests are sent. The
/// dump's users are `1` to `943`.
const USER_COUNT: u64 = 943;

/// The explanations of differences that remain after a fix. Each entry names one
/// route and one field of its answer. Every entry must match a difference, and
/// the id must have its note in `porting/notes/WP-5-1.md`. The table starts empty:
/// a difference is a port bug until a note explains it.
const EXPLAINED: &[Explained] = &[
    // Scores of the item-to-item recommender differ by up to 7.5e-7 relative. Go
    // computes them from float32 vectors with its own kernels, and the Rust port
    // with the `cc` kernels of `gorse-common`. The accepted rule is D-234 of
    // PORTING.md, which `tests/item_to_item_oracle.rs` and `tests/recommend_oracle.rs`
    // use: a score may differ from Go's by no more than 1e-6 * max(|Go's score|, 1)
    // (X-WP51-1).
    Explained {
        node: "master",
        method: "GET",
        path: "/api/dashboard/item-to-item/{name}/{item-id}",
        field: "body",
        json_path: "$[*].Score",
        tolerance: Some(1e-6),
        id: "X-WP51-1",
    },
    Explained {
        node: "master",
        method: "GET",
        path: "/api/item-to-item/{name}/{item-id}",
        field: "body",
        json_path: "$[*].Score",
        tolerance: Some(1e-6),
        id: "X-WP51-1",
    },
    Explained {
        node: "master",
        method: "GET",
        path: "/api/item/{item-id}/neighbors/",
        field: "body",
        json_path: "$[*].Score",
        tolerance: Some(1e-6),
        id: "X-WP51-1",
    },
    Explained {
        node: "master",
        method: "POST",
        path: "/api/session/recommend",
        field: "body",
        json_path: "$[*].Score",
        tolerance: Some(1e-6),
        id: "X-WP51-1",
    },
    Explained {
        node: "server",
        method: "POST",
        path: "/api/session/recommend",
        field: "body",
        json_path: "$[*].Score",
        tolerance: Some(1e-6),
        id: "X-WP51-1",
    },
    // The node's version is the build's own string: Go's build here has no version
    // set and says `unknown-version`, and the Rust crate says its package version
    // (X-WP51-2). Any value of the field is covered.
    Explained {
        node: "master",
        method: "GET",
        path: "/api/dashboard/cluster",
        field: "body",
        json_path: "$[*].Version",
        tolerance: None,
        id: "X-WP51-2",
    },
    // A task's count is its progress at the moment the list is read. The worker's
    // pass runs on its own clock, so the two clusters are at different points of
    // the pass (X-WP51-3). Any value of the field is covered.
    Explained {
        node: "master",
        method: "GET",
        path: "/api/dashboard/tasks",
        field: "body",
        json_path: "$[*].Count",
        tolerance: None,
        id: "X-WP51-3",
    },
];

/// The path placeholders that the table's parameter cell does not name, as
/// `(path, name)`. The OpenAPI document that `contracts/tools/openapi` reads has no
/// parameter for them, so the table omits them, and the Go handlers still read them
/// from the path (Q-WP51-1). The request fills each one from the same sample as a
/// listed path parameter. A placeholder not in this list fails the test.
const UNLISTED_PATH_PARAMS: &[(&str, &str)] = &[
    ("/api/dashboard/item-to-item/{name}/{item-id}", "name"),
    ("/api/dashboard/user-to-user/{name}/{user-id}", "name"),
    ("/api/dashboard/non-personalized/{name}", "name"),
    ("/api/non-personalized/{name}", "name"),
];

/// One explained difference: the node, the method and the path of the route, the
/// field that differs (`status`, `content-type` or `body`), the JSON path of the
/// value that differs with each array index written as `[*]` (empty for a status,
/// a content type or a text body), the tolerance of a number that the explanation
/// still covers under D-234 (`None` for any value), and the note's id.
#[derive(Debug, Clone, Copy)]
struct Explained {
    node: &'static str,
    method: &'static str,
    path: &'static str,
    field: &'static str,
    json_path: &'static str,
    tolerance: Option<f64>,
    id: &'static str,
}

/// The node that serves a route, as its table names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// The master node, with its dashboard routes.
    Master,
    /// The server node.
    Server,
}

impl Node {
    fn name(self) -> &'static str {
        match self {
            Node::Master => "master",
            Node::Server => "server",
        }
    }
}

/// Where a parameter goes in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Header,
    Path,
    Query,
    Body,
}

/// One parameter of a route, as the table's parameter cell writes it:
/// `path:user-id(string)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Param {
    place: Place,
    name: String,
    kind: String,
}

/// One row of a route table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    node: Node,
    method: String,
    path: String,
    params: Vec<Param>,
}

/// One request built from a route. `route` is the index of the route it answers,
/// and `node` the node it is sent to.
#[derive(Debug, Clone)]
struct Request {
    route: usize,
    node: Node,
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

/// One answer: its status, its `Content-Type` and its body.
#[derive(Debug, Clone)]
struct Reply {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

/// The answers of one cluster, with the directory of its run, aligned with the
/// requests it was sent.
struct Side {
    replies: Vec<Reply>,
    run_dir: String,
}

/// The regex of an RFC 3339 timestamp, as Go and the Rust nodes write one.
static TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})")
        .expect("the timestamp pattern is valid")
});

/// The regex of a UUID, such as the one a request id is made of.
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .expect("the UUID pattern is valid")
});

/// The regex of a 32-digit hex id without dashes, the form a node or a worker id
/// takes in the cluster and task lists.
static HEX_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-f]{32}\b").expect("the hex id pattern is valid"));

/// The regex of one array index in a JSON path, such as `[3]`.
static INDEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[0-9]+\]").expect("the index pattern is valid"));

/// The regex of Go's error for a gRPC message that is too large, with the size
/// of the message (`size`) and the limit (`limit`).
static GO_TOO_LARGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"grpc: received message larger than max \((?P<size>[0-9]+) vs\. (?P<limit>[0-9]+)\)",
    )
    .expect("the Go too-large pattern is valid")
});

/// The regex of tonic's error for a decoded gRPC message that is too large, with
/// the size of the message (`size`) and the limit (`limit`).
static RUST_TOO_LARGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"Error, decoded message length too large: found (?P<size>[0-9]+) bytes, the limit is: (?P<limit>[0-9]+) bytes",
    )
    .expect("the Rust too-large pattern is valid")
});

/// The JSON keys whose number is a port of the run (N4). They are the keys the
/// config dump uses for the ports of the master, the server and the worker.
const PORT_KEYS: [&str; 4] = ["port", "http_port", "server_port", "worker_port"];

/// Parses one of the two route tables. Each row is one route, and the header of
/// the table names how many rows it must hold.
///
/// # Errors
///
/// Fails when the table cannot be read, a row is malformed, or the rows differ in
/// number from the table's header.
fn parse_table(node: Node, file: &str, expected: usize) -> Result<Vec<Route>> {
    let text = fs::read_to_string(repo_root().join(file))
        .map_err(|e| fail(format!("read {file}: {e}")))?;
    let header = text
        .lines()
        .find(|line| line.starts_with("# "))
        .ok_or_else(|| fail(format!("{file} has no title line")))?;
    let stated = header
        .rsplit_once('(')
        .and_then(|(_, count)| count.strip_suffix(')'))
        .and_then(|count| count.parse::<usize>().ok())
        .ok_or_else(|| {
            fail(format!(
                "{file}: the title does not state a count: {header}"
            ))
        })?;
    if stated != expected {
        return Err(fail(format!(
            "{file} states {stated} routes, the test expects {expected}"
        )));
    }

    let mut routes = Vec::new();
    for line in text.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.first() == Some(&"Method") || cells.first().is_some_and(|c| c.starts_with("---")) {
            continue;
        }
        if cells.len() != 5 {
            return Err(fail(format!(
                "{file}: a row has {} cells, expected 5: {line}",
                cells.len()
            )));
        }
        let path = cells[1].trim_matches('`').to_owned();
        let params = cells[2]
            .split_whitespace()
            .map(parse_param)
            .collect::<Result<Vec<_>>>()?;
        // The table's reads column names the body type, and the body parameter
        // carries the same name. A row where they differ is malformed.
        let body_params: Vec<&Param> = params.iter().filter(|p| p.place == Place::Body).collect();
        if body_params.len() > 1 {
            return Err(fail(format!("{file}: {path} has two body parameters")));
        }
        if let Some(body) = body_params.first().filter(|body| cells[3] != body.kind) {
            return Err(fail(format!(
                "{file}: {path} reads {:?} but its body parameter is {:?}",
                cells[3], body.kind
            )));
        }
        routes.push(Route {
            node,
            method: cells[0].to_owned(),
            path,
            params,
        });
    }

    if routes.len() != expected {
        return Err(fail(format!(
            "{file} has {} rows, the test expects {expected}",
            routes.len()
        )));
    }
    Ok(routes)
}

/// Parses one parameter token: `place:name(kind)`.
///
/// # Errors
///
/// Fails when the place is unknown or the token is not `place:name(kind)`.
fn parse_param(token: &str) -> Result<Param> {
    let malformed = || fail(format!("a parameter is malformed: {token}"));
    let (place, rest) = token.split_once(':').ok_or_else(malformed)?;
    let place = match place {
        "header" => Place::Header,
        "path" => Place::Path,
        "query" => Place::Query,
        "body" => Place::Body,
        _ => return Err(fail(format!("a parameter has an unknown place: {token}"))),
    };
    let open = rest.find('(').ok_or_else(malformed)?;
    let kind = rest[open + 1..].strip_suffix(')').ok_or_else(malformed)?;
    Ok(Param {
        place,
        name: rest[..open].to_owned(),
        kind: kind.to_owned(),
    })
}

/// Both tables, master then server.
///
/// # Errors
///
/// Fails when either table is malformed or does not hold its count.
fn routes() -> Result<Vec<Route>> {
    let mut all = parse_table(Node::Master, MASTER_TABLE, MASTER_ROUTES)?;
    all.extend(parse_table(Node::Server, SERVER_TABLE, SERVER_ROUTES)?);
    Ok(all)
}

/// The sample of a path parameter. A route that has no sample for one of its
/// parameters fails the test.
///
/// # Errors
///
/// Fails when the parameter has no sample.
fn path_sample(route: &str, name: &str) -> Result<&'static str> {
    Ok(match name {
        "user-id" | "item-id" => "1",
        "category" => "golden",
        "feedback-type" => "rating",
        "recommender" => "collaborative",
        "name" => {
            if route.contains("/item-to-item/") || route.contains("/user-to-user/") {
                "neighbors"
            } else if route.contains("/non-personalized/") {
                "most_starred_weekly"
            } else if route.contains("/timeseries/") {
                "positive_feedback_ratio"
            } else if route.contains("/recommend/") {
                "neighbors"
            } else {
                return Err(fail(format!("no sample for {name} of {route}")));
            }
        }
        _ => return Err(fail(format!("no sample for the path parameter {name}"))),
    })
}

/// The sample of a query parameter. An empty sample leaves the parameter out of
/// the request, so the route's default is what is asked for.
///
/// # Errors
///
/// Fails when the parameter has no sample.
fn query_sample(name: &str) -> Result<&'static str> {
    Ok(match name {
        "n" => "5",
        "offset" => "0",
        "user-id" => "1",
        "category" | "cursor" | "q" | "index" | "script" | "write-back-type"
        | "write-back-delay" | "query-template" | "document-template" => "",
        _ => return Err(fail(format!("no sample for the query parameter {name}"))),
    })
}

/// The sample of a header parameter. An empty sample leaves the header out: no
/// node in the golden clusters has an API key, so a request without one is the
/// request the SDK suite makes.
///
/// # Errors
///
/// Fails when the header has no sample.
fn header_sample(name: &str) -> Result<&'static str> {
    match name {
        "X-API-Key" | "X-API-Version" => Ok(""),
        _ => Err(fail(format!("no sample for the header {name}"))),
    }
}

/// The JSON body of a body type, as the table names the type. A type without a
/// body fails the test.
///
/// # Errors
///
/// Fails when the type has no body.
fn body_sample(kind: &str) -> Result<String> {
    const TIME_TEXT: &str = "2024-01-01T00:00:00Z";
    let item = format!(
        r#"{{"ItemId":"golden-item","IsHidden":false,"Categories":["golden"],"Timestamp":"{TIME_TEXT}","Labels":{{}},"Comment":"golden item"}}"#
    );
    let user = r#"{"UserId":"golden-user","Labels":{},"Comment":"golden user"}"#.to_owned();
    let feedback = format!(
        r#"{{"UserId":"1","ItemId":"golden-item","FeedbackType":"rating","Value":4.0,"Timestamp":"{TIME_TEXT}","Updated":"{TIME_TEXT}","Labels":{{}},"Comment":"golden"}}"#
    );
    let session = format!(
        r#"{{"UserId":"1","ItemId":"1","FeedbackType":"rating","Value":4.0,"Timestamp":"{TIME_TEXT}","Labels":{{}},"Comment":""}}"#
    );
    Ok(match kind {
        "data.Item" => item,
        "[]data.Item" => format!("[{item}]"),
        "data.User" => user,
        "[]data.User" => format!("[{user}]"),
        "data.Feedback" => feedback,
        "[]data.Feedback" => format!("[{feedback}]"),
        "[]server.Feedback" => format!("[{session}]"),
        "data.ItemPatch" => format!(
            r#"{{"IsHidden":true,"Categories":["golden"],"Timestamp":"{TIME_TEXT}","Labels":{{}},"Comment":"patched"}}"#
        ),
        "data.UserPatch" => r#"{"Labels":{},"Comment":"patched"}"#.to_owned(),
        _ => return Err(fail(format!("no sample body for the type {kind}"))),
    })
}

/// Whether a request only reads. The reads of both nodes are sent first, so each
/// answer sees the seeded state, and then the writes are sent in table order.
fn is_read(route: &Route) -> bool {
    route.method == "GET"
        || (route.method == "POST" && route.path.starts_with("/api/session/recommend"))
}

/// Builds the request of one route from its parameter list.
///
/// # Errors
///
/// Fails when a parameter has no sample, or the path keeps a placeholder with no
/// parameter behind it.
fn request_for(index: usize, route: &Route) -> Result<Request> {
    let mut path = route.path.clone();
    let mut query: Vec<String> = Vec::new();
    let mut headers = Vec::new();
    let mut body = None;
    for param in &route.params {
        match param.place {
            Place::Path => {
                let value = path_sample(&route.path, &param.name)?;
                path = path.replace(&format!("{{{}}}", param.name), value);
            }
            Place::Query => {
                let value = query_sample(&param.name)?;
                if !value.is_empty() {
                    query.push(format!("{}={value}", param.name));
                }
            }
            Place::Header => {
                let value = header_sample(&param.name)?;
                if !value.is_empty() {
                    headers.push((param.name.clone(), value.to_owned()));
                }
            }
            Place::Body => body = Some(body_sample(&param.kind)?),
        }
    }
    // A placeholder that the parameter cell does not name is a gap in the table
    // (Q-WP51-1). Only the rows in `UNLISTED_PATH_PARAMS` may have one.
    while let Some(start) = path.find('{') {
        let end = path[start..]
            .find('}')
            .map(|offset| start + offset)
            .ok_or_else(|| fail(format!("{} has an unclosed placeholder", route.path)))?;
        let name = path[start + 1..end].to_owned();
        if !UNLISTED_PATH_PARAMS.contains(&(route.path.as_str(), name.as_str())) {
            return Err(fail(format!(
                "{} keeps the placeholder {name}, which its parameter list does not name",
                route.path
            )));
        }
        let value = path_sample(&route.path, &name)?;
        path = path.replace(&format!("{{{name}}}"), value);
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    Ok(Request {
        route: index,
        node: route.node,
        method: Method::from_bytes(route.method.as_bytes())
            .map_err(|e| fail(format!("method {}: {e}", route.method)))?,
        path,
        headers,
        body,
    })
}

/// The request list of a run: every route's request, reads first, then writes,
/// each group in table order.
///
/// # Errors
///
/// Fails when any route cannot be built into a request.
fn requests(routes: &[Route]) -> Result<Vec<Request>> {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for (index, route) in routes.iter().enumerate() {
        let request = request_for(index, route)?;
        if is_read(route) {
            reads.push(request);
        } else {
            writes.push(request);
        }
    }
    reads.extend(writes);
    Ok(reads)
}

/// Reads the MovieLens 100k dump, decompressed. A missing or unreadable archive is
/// an error, not a skip.
fn dump() -> Result<Vec<u8>> {
    let path = std::env::var("GORSE_ML_100K_DUMP").unwrap_or_else(|_| DUMP_PATH.to_owned());
    let archive = fs::read(&path).map_err(|e| fail(format!("read {path}: {e}")))?;
    let mut bytes = Vec::new();
    GzDecoder::new(&archive[..])
        .read_to_end(&mut bytes)
        .map_err(|e| fail(format!("decompress {path}: {e}")))?;
    Ok(bytes)
}

/// Which implementation a side runs: the Go binaries or the Rust ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Impl {
    Go,
    Rust,
}

/// Restores the MovieLens 100k dump through the master's `/api/restore`, as the
/// setup does with `curl --data-binary`, and checks the counts it reports.
async fn restore(master_http: u16, dump: Vec<u8>) -> Result<()> {
    let url = format!("http://127.0.0.1:{master_http}/api/restore");
    let response = reqwest::Client::new()
        .post(&url)
        .body(dump)
        .send()
        .await
        .map_err(|e| fail(format!("POST {url}: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| fail(format!("read the restore response: {e}")))?;
    if !status.is_success() {
        return Err(fail(format!("restore answered {status}: {body}")));
    }
    let stats: Value =
        serde_json::from_str(&body).map_err(|e| fail(format!("restore response {body}: {e}")))?;
    let count = |key: &str| {
        stats
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| fail(format!("restore response has no {key} count: {body}")))
    };
    let counts = (count("Users")?, count("Items")?, count("Feedback")?);
    if counts != (RESTORED_USERS, RESTORED_ITEMS, RESTORED_FEEDBACK) {
        return Err(fail(format!("restore counts {counts:?}")));
    }
    Ok(())
}

/// Starts the master of a side, and waits until its gRPC port listens.
async fn start_master(cluster: &mut Cluster, build: Impl) -> Result<()> {
    match build {
        Impl::Go => cluster.start_master().await,
        Impl::Rust => cluster.start_rust_master_binary().await,
    }
}

/// Waits until the master has trained its collaborative filtering model on the
/// restored dump and none of its own tasks is running. A training that fails is an
/// error at once.
async fn wait_for_training(master_http: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{master_http}/api/dashboard/tasks");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + SEED_TIMEOUT;
    loop {
        let tasks: Option<Vec<Value>> = match client.get(&url).send().await {
            Ok(response) => response.json().await.ok(),
            Err(_) => None,
        };
        if let Some(tasks) = tasks {
            if let Some(failed) = tasks.iter().find(|task| {
                field(task, "Name") == TRAINING_TASK && field(task, "Status") == "Failed"
            }) {
                return Err(fail(format!("training failed: {failed}")));
            }
            let trained = tasks.iter().any(|task| {
                field(task, "Name") == TRAINING_TASK && field(task, "Status") == "Complete"
            });
            let running = tasks.iter().any(|task| {
                field(task, "Tracer") == "master" && field(task, "Status") == "Running"
            });
            if trained && !running {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(fail(format!(
                "timed out waiting for {TRAINING_TASK} at {url}"
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// One string field of a task, or the empty string.
fn field<'a>(task: &'a Value, key: &str) -> &'a str {
    task.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The users of the dump that have a recommendation update time in the cache.
async fn recommended_users(cache: &dyn cache::Database) -> BTreeSet<u64> {
    let ctx = Context::background();
    let mut recommended = BTreeSet::new();
    for id in 1..=USER_COUNT {
        let text = id.to_string();
        let key = cache::key(&[cache::RECOMMEND_UPDATE_TIME, text.as_str()]);
        if cache.get(&ctx, &key).await.exists() {
            recommended.insert(id);
        }
    }
    recommended
}

/// The last lines of the logs of the nodes that run in `run_dir`, for a failure.
fn node_logs(run_dir: &str) -> String {
    let mut out = String::new();
    for name in ["gorse-master", "gorse-server", "gorse-worker"] {
        let text =
            fs::read_to_string(Path::new(run_dir).join(format!("{name}.log"))).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        out.push_str(&format!("--- {name} ---\n"));
        for line in &lines[lines.len().saturating_sub(30)..] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Seeds one cluster the way `client/setup-test.sh` does: the master, the server
/// and the worker start, the dump is restored, the master restarts, and the
/// master's training and the worker's recommendations are waited for.
async fn seed(build: &'static GoBuild, side: Impl) -> Result<Cluster> {
    let config = repo_root().join("client/config.toml");
    let mut cluster = Cluster::new_from_config(build, &config, &[])?;
    start_master(&mut cluster, side).await?;
    match side {
        Impl::Go => cluster.start_go_server().await?,
        Impl::Rust => cluster.start_rust_server_binary().await?,
    }
    match side {
        Impl::Go => {
            cluster
                .start_go_worker(WORKER_INTERVAL, WORKER_JOBS)
                .await?
        }
        Impl::Rust => {
            cluster
                .start_rust_worker_binary(WORKER_INTERVAL, WORKER_JOBS)
                .await?
        }
    }
    restore(cluster.ports.master_http, dump()?).await?;
    cluster.stop_master();
    start_master(&mut cluster, side).await?;
    wait_for_training(cluster.ports.master_http).await?;

    // The worker needs about two minutes for the 943 users of the dump (the Go
    // worker does about seven users a second). It writes no update time for a user
    // it skips: a cold-start user, which has no positive feedback, returns before
    // the write (`worker/pipeline.go`, `IsColdStart`). So the wait ends when the set
    // of recommended users has stayed the same for `SETTLE`, not when it is every
    // user. `Cluster::wait_for` gives up after two minutes, which is too early here.
    let cache = cluster.cache_store().await?;
    let deadline = Instant::now() + SEED_TIMEOUT;
    let mut previous = recommended_users(&*cache).await;
    let mut unchanged_since = Instant::now();
    loop {
        if Instant::now() >= deadline {
            return Err(fail(format!(
                "timed out after {SEED_TIMEOUT:?} waiting for the recommendations to settle: {} users have one\n{}",
                previous.len(),
                node_logs(&run_dir(&cluster))
            )));
        }
        let current = recommended_users(&*cache).await;
        if current != previous {
            previous = current;
            unchanged_since = Instant::now();
        } else if !previous.is_empty() && unchanged_since.elapsed() >= SETTLE {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(cluster)
}

/// Sends one request to `base` and reads its answer.
async fn send(client: &reqwest::Client, base: &str, request: &Request) -> Result<Reply> {
    let mut builder = client.request(request.method.clone(), format!("{base}{}", request.path));
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if let Some(body) = &request.body {
        builder = builder
            .header("Content-Type", "application/json")
            .body(body.clone());
    }
    let response = builder
        .send()
        .await
        .map_err(|e| fail(format!("{} {}: {e}", request.method, request.path)))?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<absent>")
        .to_owned();
    let body = response
        .bytes()
        .await
        .map_err(|e| fail(format!("read {}: {e}", request.path)))?
        .to_vec();
    Ok(Reply {
        status,
        content_type,
        body,
    })
}

/// The directory of a run, from its blob store's directory.
fn run_dir(cluster: &Cluster) -> String {
    cluster
        .blob_dir()
        .parent()
        .map_or_else(String::new, |dir| dir.display().to_string())
}

/// Replaces the size of a too-large gRPC message with `<N>` (D-299). The limit and
/// the rest of the text stay exact. The size must be above the limit, as a message
/// that is too large is, or the test fails. Text without such a message is returned
/// as it is, so a difference in the wording still shows.
fn mask_message_size(text: &str) -> String {
    for pattern in [&*GO_TOO_LARGE, &*RUST_TOO_LARGE] {
        let Some(caps) = pattern.captures(text) else {
            continue;
        };
        let size = caps.name("size").expect("the size group");
        let size_value: u64 = size.as_str().parse().expect("a size of digits");
        let limit: u64 = caps["limit"].parse().expect("a limit of digits");
        assert!(
            size_value > limit,
            "a too-large message of {size_value} bytes does not exceed the limit of {limit} bytes: {text}"
        );
        return format!("{}<N>{}", &text[..size.start()], &text[size.end()..]);
    }
    text.to_owned()
}

/// Normalises a string by N1, N2 and N3.
fn normalize_text(text: &str, run: &str) -> String {
    let text = TIME.replace_all(text, "<time>");
    let text = UUID.replace_all(&text, "<uuid>");
    let text = HEX_ID.replace_all(&text, "<uuid>");
    if run.is_empty() {
        text.into_owned()
    } else {
        text.replace(run, "<run>")
    }
}

/// Normalises a JSON value by N1 to N4. Strings are normalised as text, and each
/// number under a port key becomes the text `<port>`.
fn normalize_json(value: &mut Value, run: &str) {
    match value {
        Value::String(text) => *text = normalize_text(text, run),
        Value::Array(items) => items.iter_mut().for_each(|item| normalize_json(item, run)),
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if PORT_KEYS.contains(&key.as_str()) && child.is_number() {
                    *child = Value::String("<port>".to_owned());
                } else {
                    normalize_json(child, run);
                }
            }
        }
        _ => {}
    }
}

/// Whether two JSON values are equal, with every number compared as a float, since
/// Go writes a float zero as `0`.
fn json_equal(go: &Value, rust: &Value) -> bool {
    match (go, rust) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| json_equal(x, y)))
        }
        _ => go == rust,
    }
}

/// One difference between the answers of a route.
#[derive(Debug, Clone)]
struct Difference {
    /// `status`, `content-type` or `body`.
    field: &'static str,
    /// For a JSON body, the path of the value, with each array index as `[*]`
    /// (`$[*].Score`). Empty for a status, a content type or a text body.
    path: String,
    /// Go's value and Rust's value, when both are numbers, for the tolerance check.
    numbers: Option<(f64, f64)>,
    /// What differs, for a failure message.
    detail: String,
}

/// A difference at one path of two JSON values.
fn leaf(at: &str, go: Option<&Value>, rust: Option<&Value>) -> Difference {
    let numbers = match (go.and_then(Value::as_f64), rust.and_then(Value::as_f64)) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    };
    Difference {
        field: "body",
        path: INDEX.replace_all(at, "[*]").into_owned(),
        numbers,
        detail: format!("{at}: Go {}, Rust {}", short(go), short(rust)),
    }
}

/// The differences between two JSON values, each at the path where they differ.
fn json_paths(at: &str, go: &Value, rust: &Value, found: &mut Vec<Difference>) {
    if json_equal(go, rust) {
        return;
    }
    match (go, rust) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for key in keys {
                let child = format!("{at}.{key}");
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => json_paths(&child, x, y, found),
                    (x, y) => found.push(leaf(&child, x, y)),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                json_paths(&format!("{at}[{i}]"), x, y, found);
            }
        }
        _ => found.push(leaf(at, Some(go), Some(rust))),
    }
}

/// A JSON value as a short text, for a failure message.
fn short(value: Option<&Value>) -> String {
    match value {
        None => "absent".to_owned(),
        Some(value) => {
            let text = value.to_string();
            if text.chars().count() > 80 {
                format!("{}...", text.chars().take(80).collect::<String>())
            } else {
                text
            }
        }
    }
}

/// The differences between the answers of one route.
fn differences(go: &Reply, rust: &Reply, go_run: &str, rust_run: &str) -> Vec<Difference> {
    let mut found = Vec::new();
    if go.status != rust.status {
        found.push(Difference {
            field: "status",
            path: String::new(),
            numbers: None,
            detail: format!("Go {}, Rust {}", go.status, rust.status),
        });
    }
    if go.content_type != rust.content_type {
        found.push(Difference {
            field: "content-type",
            path: String::new(),
            numbers: None,
            detail: format!("Go {:?}, Rust {:?}", go.content_type, rust.content_type),
        });
    }
    if is_json(go) && is_json(rust) {
        match (
            parse_normalized(&go.body, go_run),
            parse_normalized(&rust.body, rust_run),
        ) {
            (Some(go_value), Some(rust_value)) => {
                json_paths("$", &go_value, &rust_value, &mut found);
            }
            _ => found.push(Difference {
                field: "body",
                path: String::new(),
                numbers: None,
                detail: "a JSON body does not parse".to_owned(),
            }),
        }
    } else {
        let go_text = normalize_text(
            &mask_message_size(&String::from_utf8_lossy(&go.body)),
            go_run,
        );
        let rust_text = normalize_text(
            &mask_message_size(&String::from_utf8_lossy(&rust.body)),
            rust_run,
        );
        if go_text != rust_text {
            found.push(Difference {
                field: "body",
                path: String::new(),
                numbers: None,
                detail: format!(
                    "text: Go {:?}, Rust {:?}",
                    excerpt(&go_text),
                    excerpt(&rust_text)
                ),
            });
        }
    }
    found
}

/// Whether an answer is a JSON body.
fn is_json(reply: &Reply) -> bool {
    reply.content_type.starts_with("application/json")
}

/// A body parsed as JSON, with the normalisation of the run applied.
fn parse_normalized(body: &[u8], run: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    normalize_json(&mut value, run);
    Some(value)
}

/// The first 200 characters of a text, for a failure message. The size of a
/// too-large gRPC message (`<N>`, D-299) comes after 80 characters, so it shows.
fn excerpt(text: &str) -> String {
    text.chars().take(200).collect()
}

/// Whether Rust's number is within `tolerance` of Go's, by the rule of D-234 in
/// PORTING.md: `|rust - go| <= tolerance * max(|go|, 1)`.
fn within(go: f64, rust: f64, tolerance: f64) -> bool {
    (rust - go).abs() <= tolerance * go.abs().max(1.0)
}

/// The explanation a difference has, if one matches it. A number is covered when
/// Go's and Rust's values are within the explanation's tolerance.
fn explanation(route: &Route, difference: &Difference) -> Option<&'static Explained> {
    EXPLAINED.iter().find(|entry| {
        entry.node == route.node.name()
            && entry.method == route.method
            && entry.path == route.path
            && entry.field == difference.field
            && entry.json_path == difference.path
            && match (entry.tolerance, difference.numbers) {
                (None, _) => true,
                (Some(tolerance), Some((go, rust))) => within(go, rust, tolerance),
                (Some(_), None) => false,
            }
    })
}

/// Compares the two sides route by route. Returns the explained differences, which
/// are logged for the notes, the unexplained ones, which fail the test, and the ids
/// of the explanations that matched.
fn compare(
    routes: &[Route],
    go: &Side,
    rust: &Side,
) -> (Vec<String>, Vec<String>, BTreeSet<&'static str>) {
    let mut explained_lines = Vec::new();
    let mut unexplained = Vec::new();
    let mut used = BTreeSet::new();
    let mut identical = 0_usize;
    for (index, route) in routes.iter().enumerate() {
        let found = differences(
            &go.replies[index],
            &rust.replies[index],
            &go.run_dir,
            &rust.run_dir,
        );
        if found.is_empty() {
            identical += 1;
            continue;
        }
        for difference in found {
            let label = format!(
                "{} {} {} [{}] {}",
                route.node.name(),
                route.method,
                route.path,
                difference.field,
                difference.detail
            );
            match explanation(route, &difference) {
                Some(entry) => {
                    used.insert(entry.id);
                    explained_lines.push(format!("{label} ({})", entry.id));
                }
                None => unexplained.push(label),
            }
        }
    }
    eprintln!(
        "rest_golden: {} routes compared, {identical} identical, {} explained differences, {} unexplained",
        routes.len(),
        explained_lines.len(),
        unexplained.len()
    );
    (explained_lines, unexplained, used)
}

/// Builds the request list, seeds the Go cluster and the Rust cluster, sends the
/// requests to both, and checks every answer.
#[tokio::test]
async fn rest_golden_matches_go() {
    let routes = routes().expect("the route tables parse to 107 routes");
    assert_eq!(routes.len(), ROUTE_PAIRS, "every table row is a route");
    let mut seen = BTreeSet::new();
    for route in &routes {
        assert!(
            seen.insert((route.node.name(), route.method.clone(), route.path.clone())),
            "{} {} {} is listed twice",
            route.node.name(),
            route.method,
            route.path
        );
    }
    let requests = requests(&routes).expect("every route builds a request");
    assert_eq!(requests.len(), ROUTE_PAIRS, "one request per route");

    let go_build = GoBuild::shared().expect("build the Go nodes");
    RustBuild::shared().expect("build the Rust nodes");

    let go = run_side(go_build, Impl::Go, &requests)
        .await
        .expect("the Go cluster answers every request");
    let rust = run_side(go_build, Impl::Rust, &requests)
        .await
        .expect("the Rust cluster answers every request");

    let (explained_lines, unexplained, used) = compare(&routes, &go, &rust);
    for line in &explained_lines {
        eprintln!("rest_golden explained: {line}");
    }
    for entry in EXPLAINED.iter().filter(|entry| !used.contains(entry.id)) {
        eprintln!(
            "rest_golden: explanation {} matched no difference in this run: {} {} {} [{}] {}",
            entry.id, entry.node, entry.method, entry.path, entry.field, entry.json_path
        );
    }
    assert!(
        unexplained.is_empty(),
        "the Go and Rust nodes answer differently, and no note explains it:\n{}",
        unexplained.join("\n")
    );
}

/// Seeds one cluster, sends every request in the order of `requests`, and returns
/// the answers aligned with the routes. The cluster stops when the call returns.
async fn run_side(build: &'static GoBuild, side: Impl, requests: &[Request]) -> Result<Side> {
    let cluster = seed(build, side).await?;
    let dir = run_dir(&cluster);
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| fail(format!("the HTTP client: {e}")))?;
    let mut replies: Vec<Option<Reply>> = vec![None; requests.len()];
    for request in requests {
        let port = match request.node {
            Node::Master => cluster.ports.master_http,
            Node::Server => cluster.ports.server_http,
        };
        let base = format!("http://127.0.0.1:{port}");
        replies[request.route] = Some(send(&client, &base, request).await?);
    }
    let replies = replies
        .into_iter()
        .map(|reply| reply.ok_or_else(|| fail("a route got no answer".to_owned())))
        .collect::<Result<Vec<Reply>>>()?;
    Ok(Side {
        replies,
        run_dir: dir,
    })
}

#[cfg(test)]
mod rest_golden_checks {
    use super::*;

    #[test]
    fn route_tables_hold_107_pairs() {
        let routes = routes().expect("the tables parse");
        assert_eq!(routes.len(), 107);
        assert_eq!(
            routes.iter().filter(|r| r.node == Node::Master).count(),
            MASTER_ROUTES
        );
        assert_eq!(
            routes.iter().filter(|r| r.node == Node::Server).count(),
            SERVER_ROUTES
        );
    }

    #[test]
    fn every_route_builds_a_request() {
        let routes = routes().expect("the tables parse");
        let requests = requests(&routes).expect("every route builds");
        assert_eq!(requests.len(), 107);
        let reads = requests
            .iter()
            .take_while(|request| is_read(&routes[request.route]))
            .count();
        assert!(
            reads > 0 && reads < requests.len(),
            "reads come first, then writes"
        );
    }

    #[test]
    fn a_parameter_parses_its_place_name_and_type() {
        let param = parse_param("body:body([]data.Feedback)").expect("parses");
        assert_eq!(
            param,
            Param {
                place: Place::Body,
                name: "body".to_owned(),
                kind: "[]data.Feedback".to_owned(),
            }
        );
        assert!(parse_param("cookie:session(string)").is_err());
        assert!(parse_param("query:n").is_err());
    }

    #[test]
    fn the_normalizer_masks_time_uuid_run_and_port() {
        let mut value = serde_json::json!({
            "Start": "2026-11-02T01:11:19.123456789+08:00",
            "Id": "3f1c2d4e-5a6b-4c7d-8e9f-0a1b2c3d4e5f",
            "Blob": "/tmp/run-1/blob",
            "master": {"port": 5001, "http_port": 5002, "replicas": 3},
        });
        normalize_json(&mut value, "/tmp/run-1");
        assert_eq!(value["Start"], "<time>");
        assert_eq!(value["Id"], "<uuid>");
        assert_eq!(value["Blob"], "<run>/blob");
        assert_eq!(value["master"]["port"], "<port>");
        assert_eq!(value["master"]["http_port"], "<port>");
        assert_eq!(value["master"]["replicas"], 3);
    }

    #[test]
    fn numbers_compare_as_floats() {
        assert!(json_equal(&serde_json::json!(0), &serde_json::json!(0.0)));
        assert!(!json_equal(&serde_json::json!(1), &serde_json::json!(2)));
    }

    #[test]
    fn scores_are_covered_by_the_accepted_rule() {
        // D-234: |rust - go| <= 1e-6 * max(|go|, 1). Below 1 the bound is absolute,
        // so a gap of 9e-7 on a score of 0.5 is covered.
        assert!(within(0.5, 0.5 + 9e-7, 1e-6));
        assert!(!within(0.5, 0.5 + 1.1e-6, 1e-6));
        // Above 1 the bound is relative to Go's score.
        assert!(within(100.0, 100.0 + 9e-5, 1e-6));
        assert!(!within(100.0, 100.0 + 2e-4, 1e-6));
    }

    #[test]
    fn only_the_size_of_a_too_large_message_is_masked() {
        // D-299: the size becomes <N>, and the limit and the rest of the text stay.
        let go = "rpc error: code = ResourceExhausted desc = grpc: received message larger than max (9076002 vs. 4194304)";
        assert_eq!(
            mask_message_size(go),
            "rpc error: code = ResourceExhausted desc = grpc: received message larger than max (<N> vs. 4194304)"
        );
        let rust = "rpc error: code = OutOfRange desc = Error, decoded message length too large: found 9076124 bytes, the limit is: 4194304 bytes";
        assert_eq!(
            mask_message_size(rust),
            "rpc error: code = OutOfRange desc = Error, decoded message length too large: found <N> bytes, the limit is: 4194304 bytes"
        );
        assert_eq!(mask_message_size("no such error"), "no such error");
    }

    #[test]
    #[should_panic(expected = "does not exceed the limit")]
    fn a_too_large_message_must_exceed_its_limit() {
        mask_message_size("grpc: received message larger than max (10 vs. 4194304)");
    }
}
