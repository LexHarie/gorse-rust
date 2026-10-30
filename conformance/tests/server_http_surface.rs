//! The X-214 acceptance test: the server node's HTTP surface. The Go
//! `gorse-server` and the Rust server each answer the same requests for the CORS
//! filter, the OpenAPI document and its Swagger UI page, the pprof handlers and
//! the metrics route. Each answer must match in its status, its content type, its
//! redirect, its CORS headers in order, and, for the routes whose body is fixed,
//! its body byte for byte. The checks are shared with `master_http_surface`.
//!
//! The server node takes no dashboard route and no master plain handler, so the
//! cases that the master registers, and the server does not, are checked here as
//! well. The test fails, and does not skip, when the Go toolchain is missing or a
//! node does not start.

mod common;

use std::sync::Arc;

use common::http_surface::{
    ALLOWED, Answer, Case, DEFAULT, RESTRICTED, apidocs_cases, ask_all, compare, cors_cases,
    pprof_cases,
};
use gorse_conformance::cluster::{Cluster, Edit, GoBuild};
use gorse_server::server::{Server, ServerError, new_server};
use reqwest::{Method, StatusCode};
use tokio::task::JoinHandle;

/// The cases only a server node can answer the way the test expects: the master's
/// dashboard routes and its plain handlers are not on the server, and the metrics
/// route is.
fn server_cases() -> Vec<Case> {
    vec![
        Case::new(
            "dashboard route from an allowed origin",
            Method::GET,
            "/api/dashboard/cluster",
        )
        .origin(ALLOWED),
        Case::new(
            "dashboard preflight on the server node",
            Method::OPTIONS,
            "/api/dashboard/cluster",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "GET"),
        Case::new("master purge route", Method::GET, "/api/purge").origin(ALLOWED),
        Case::new("metrics", Method::GET, "/metrics").status_only(),
    ]
}

/// The answers of the Go server node to `cases`, under `edits`, with the Go
/// master it joins.
async fn go_answers(build: &'static GoBuild, edits: &[Edit<'_>], cases: &[Case]) -> Vec<Answer> {
    let mut cluster = Cluster::new(build, edits).expect("a Go run");
    cluster.start_master().await.expect("the Go master starts");
    cluster
        .start_go_server()
        .await
        .expect("the Go server starts");
    let base = format!("http://127.0.0.1:{}", cluster.ports.server_http);
    let answers = ask_all(&base, cases).await;
    cluster.stop();
    answers
}

/// The Rust server node, serving in this process, with the Go master it joins.
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

/// Starts the Rust server against the cluster's master, on the port the Go server
/// takes, and waits until its `/api/health/ready` answers 200.
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

/// The answers of the Rust server node to `cases`, under `edits`, with the Go
/// master it joins.
async fn rust_answers(build: &'static GoBuild, edits: &[Edit<'_>], cases: &[Case]) -> Vec<Answer> {
    let mut cluster = Cluster::new(build, edits).expect("a Rust run");
    cluster.start_master().await.expect("the Go master starts");
    let server = start_rust_server(&mut cluster).await;
    let base = format!("http://127.0.0.1:{}", cluster.ports.server_http);
    let answers = ask_all(&base, cases).await;
    server.stop().await;
    cluster.stop();
    answers
}

/// Runs `cases` against both server nodes under `edits`, and fails with every
/// difference, with the case name and both answers' fields.
async fn assert_matches(
    build: &'static GoBuild,
    label: &str,
    edits: &[Edit<'_>],
    cases: Vec<Case>,
) {
    let go = go_answers(build, edits, &cases).await;
    let rust = rust_answers(build, edits, &cases).await;
    compare(label, &cases, &go, &rust);
}

/// The CORS filter with the configured domains, on the server node.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_cors_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "restricted CORS", &RESTRICTED, cors_cases()).await;
}

/// The default config, which allows every origin, on the server node.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_cors_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "default CORS", &DEFAULT, cors_cases()).await;
}

/// The OpenAPI document of the server, and the Swagger UI page with its files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apidocs_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "apidocs", &DEFAULT, apidocs_cases()).await;
}

/// The pprof routes, with the same status and content type as Go's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pprof_routes_match_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "pprof", &DEFAULT, pprof_cases()).await;
}

/// The routes only the server node has or lacks, and the metrics route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_only_routes_match_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "server routes", &DEFAULT, server_cases()).await;
}
