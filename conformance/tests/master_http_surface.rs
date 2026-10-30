//! The X-254 acceptance test: the master's HTTP surface outside the dashboard
//! API. The Go `gorse-master` and the Rust master each answer the same requests
//! for the CORS filter, the OpenAPI document and its Swagger UI page, and the
//! pprof handlers. Each answer must match in its status, its content type, its
//! redirect, its CORS headers in order, and, for the routes whose body is fixed,
//! its body byte for byte.
//!
//! The pprof bodies are not compared. A profile is sampled at the moment it is
//! asked for, and the Rust master has no sampler, so only the route and the
//! status code are checked there. The test fails, and does not skip, when the Go
//! toolchain is missing or a node does not start.

mod common;

use common::http_surface::{
    Answer, Case, DEFAULT, RESTRICTED, apidocs_cases, ask_all, compare, cors_cases, pprof_cases,
    serve,
};
use gorse_conformance::cluster::{Cluster, Edit, GoBuild};
use reqwest::Method;

/// The answers of the Go master to `cases`, under `edits`.
async fn go_answers(build: &'static GoBuild, edits: &[Edit<'_>], cases: &[Case]) -> Vec<Answer> {
    let mut go = Cluster::new(build, edits).expect("a Go run");
    go.start_master().await.expect("the Go master starts");
    let base = format!("http://127.0.0.1:{}", go.ports.master_http);
    let answers = ask_all(&base, cases).await;
    go.stop();
    answers
}

/// The answers of the `gorse-master` binary to `cases`, under `edits`. The binary
/// serves the root `/metrics` route, which the library router does not.
async fn rust_binary_answers(
    build: &'static GoBuild,
    edits: &[Edit<'_>],
    cases: &[Case],
) -> Vec<Answer> {
    let mut rust = Cluster::new(build, edits).expect("a Rust run");
    rust.start_rust_master_binary()
        .await
        .expect("the Rust master binary starts");
    let base = format!("http://127.0.0.1:{}", rust.ports.master_http);
    let answers = ask_all(&base, cases).await;
    rust.stop();
    answers
}

/// The answers of the Rust master to `cases`, under `edits`.
async fn rust_answers(build: &'static GoBuild, edits: &[Edit<'_>], cases: &[Case]) -> Vec<Answer> {
    let mut rust = Cluster::new(build, edits).expect("a Rust run");
    let master = rust
        .start_rust_master()
        .await
        .expect("the Rust master starts");
    let (port, server) = serve(master.router()).await;
    let base = format!("http://127.0.0.1:{port}");
    let answers = ask_all(&base, cases).await;
    server.abort();
    master.stop().await;
    answers
}

/// Runs `cases` against both masters under `edits`, and fails with every
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

/// The CORS filter with the configured domains: an allowed origin gets its
/// header, a refused one does not, and a preflight is answered with the methods
/// and headers the config lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_cors_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "restricted CORS", &RESTRICTED, cors_cases()).await;
}

/// The default config lists `.*`, so every origin is allowed and the origin is
/// echoed back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_cors_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "default CORS", &DEFAULT, cors_cases()).await;
}

/// The OpenAPI document and the Swagger UI page, with its files and the
/// redirect for a path without its slash.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apidocs_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "apidocs", &DEFAULT, apidocs_cases()).await;
}

/// The pprof routes answer the same status and content type as Go's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pprof_routes_match_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    assert_matches(build, "pprof", &DEFAULT, pprof_cases()).await;
}

/// The root `/metrics` route answers the status and the content type of Go's
/// promhttp exposition. The body is not compared, since the counters differ by
/// process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_matches_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    let cases = vec![Case::new("metrics", Method::GET, "/metrics").status_only()];
    let go = go_answers(build, &DEFAULT, &cases).await;
    let rust = rust_binary_answers(build, &DEFAULT, &cases).await;
    compare("metrics", &cases, &go, &rust);
}
