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

use std::collections::BTreeMap;

use gorse_conformance::cluster::{Cluster, Edit, GoBuild};
use reqwest::Method;

/// The origin the configured CORS domains allow.
const ALLOWED: &str = "https://allowed.example";

/// An origin no configured CORS domain allows.
const DENIED: &str = "https://denied.example";

/// The CORS domains and methods of the first run, which the default config does
/// not match: only `ALLOWED` is allowed.
const RESTRICTED: [Edit<'static>; 2] = [
    (
        "master",
        "http_cors_domains",
        "[\"https://allowed.example\"]",
    ),
    (
        "master",
        "http_cors_methods",
        "[\"GET\",\"POST\",\"PUT\",\"DELETE\",\"PATCH\"]",
    ),
];

/// The default config, which allows every origin.
const DEFAULT: [Edit<'static>; 0] = [];

/// How a case compares the bodies of the two masters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyCheck {
    /// Byte for byte.
    Exact,
    /// The OpenAPI document. Go's `required` arrays come out in a random order
    /// (`PORTING_NOTES.md` Q-8), so each one is compared as a set, and the rest
    /// of the document byte for byte as JSON.
    Document,
    /// Not compared: a pprof profile, which is sampled when it is asked for.
    Skip,
}

/// One request both masters receive.
struct Case {
    name: &'static str,
    method: Method,
    path: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: BodyCheck,
}

impl Case {
    fn new(name: &'static str, method: Method, path: &'static str) -> Case {
        Case {
            name,
            method,
            path,
            headers: Vec::new(),
            body: BodyCheck::Exact,
        }
    }

    /// Compares the body as the OpenAPI document.
    fn document(mut self) -> Case {
        self.body = BodyCheck::Document;
        self
    }

    fn origin(mut self, origin: &'static str) -> Case {
        self.headers.push(("origin", origin));
        self
    }

    fn header(mut self, name: &'static str, value: &'static str) -> Case {
        self.headers.push((name, value));
        self
    }

    /// Compares the status, the content type and the headers, not the body.
    fn status_only(mut self) -> Case {
        self.body = BodyCheck::Skip;
        self
    }
}

/// The CORS cases, the preflight cases and the routes the filter does not touch.
fn cors_cases() -> Vec<Case> {
    vec![
        Case::new("items without an origin", Method::GET, "/api/items?n=5"),
        Case::new(
            "items from an allowed origin",
            Method::GET,
            "/api/items?n=5",
        )
        .origin(ALLOWED),
        Case::new("items from a refused origin", Method::GET, "/api/items?n=5").origin(DENIED),
        Case::new(
            "items from an upper-case origin",
            Method::GET,
            "/api/items?n=5",
        )
        .origin("HTTPS://ALLOWED.EXAMPLE"),
        Case::new(
            "error from an allowed origin",
            Method::GET,
            "/api/user/nobody",
        )
        .origin(ALLOWED),
        Case::new(
            "error from a refused origin",
            Method::GET,
            "/api/user/nobody",
        )
        .origin(DENIED),
        Case::new(
            "unknown route from an allowed origin",
            Method::GET,
            "/api/nothing",
        )
        .origin(ALLOWED),
        Case::new(
            "preflight from an allowed origin",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "DELETE")
        .header("access-control-request-headers", "Content-Type, x-api-key"),
        Case::new(
            "preflight without requested headers",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "GET"),
        Case::new(
            "preflight for an unlisted method",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "TRACE"),
        Case::new(
            "preflight for an unlisted header",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "X-Other"),
        Case::new(
            "preflight from a refused origin",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(DENIED)
        .header("access-control-request-method", "GET"),
        Case::new(
            "options without a requested method",
            Method::OPTIONS,
            "/api/items",
        )
        .origin(ALLOWED),
        Case::new(
            "preflight for a path with a parameter",
            Method::OPTIONS,
            "/api/user/nobody",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "PATCH"),
        Case::new(
            "preflight for a path no route takes",
            Method::OPTIONS,
            "/api/nothing",
        )
        .origin(ALLOWED)
        .header("access-control-request-method", "GET"),
    ]
}

/// The OpenAPI document, the Swagger UI page and its files, and the redirects
/// `ServeMux` answers for a path without its trailing slash.
fn apidocs_cases() -> Vec<Case> {
    vec![
        Case::new("openapi document", Method::GET, "/apidocs.json").document(),
        Case::new(
            "openapi document from an allowed origin",
            Method::GET,
            "/apidocs.json",
        )
        .origin(ALLOWED)
        .document(),
        Case::new("swagger ui without the slash", Method::GET, "/apidocs"),
        Case::new("swagger ui page", Method::GET, "/apidocs/"),
        Case::new(
            "swagger ui page with a query",
            Method::GET,
            "/apidocs/?proxy=x",
        ),
        Case::new(
            "swagger ui bundle",
            Method::GET,
            "/apidocs/swagger-ui-bundle.js",
        ),
        Case::new(
            "swagger ui stylesheet",
            Method::GET,
            "/apidocs/swagger-ui.css",
        ),
        Case::new(
            "swagger ui redirect page",
            Method::GET,
            "/apidocs/oauth2-redirect.html",
        ),
        Case::new(
            "swagger ui favicon",
            Method::GET,
            "/apidocs/favicon-32x32.png",
        ),
        Case::new(
            "missing swagger ui file",
            Method::GET,
            "/apidocs/missing.js",
        ),
    ]
}

/// The pprof handlers Go registers, and the status each answers.
fn pprof_cases() -> Vec<Case> {
    let paths = [
        "/debug/pprof",
        "/debug/pprof/",
        "/debug/pprof/cmdline",
        "/debug/pprof/symbol",
        "/debug/pprof/allocs",
        "/debug/pprof/block",
        "/debug/pprof/goroutine",
        "/debug/pprof/goroutineleak",
        "/debug/pprof/heap",
        "/debug/pprof/mutex",
        "/debug/pprof/threadcreate",
        "/debug/pprof/profile?seconds=1",
        "/debug/pprof/trace?seconds=1",
        "/debug/pprof/nothing",
    ];
    paths
        .into_iter()
        .map(|path| Case::new(path, Method::GET, path).status_only())
        .collect()
}

/// What one master answers for a request: the parts the test compares.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    content_type: String,
    location: String,
    /// Every `Access-Control-Allow-Origin` value, in the order sent.
    allow_origin: Vec<String>,
    allow_methods: String,
    allow_headers: String,
    body: Vec<u8>,
}

impl Answer {
    /// Reads a response. A header that is absent reads as `<absent>`.
    async fn read(response: reqwest::Response) -> Answer {
        let headers = response.headers();
        let text = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("<absent>")
                .to_owned()
        };
        let allow_origin = headers
            .get_all("access-control-allow-origin")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_owned)
            .collect();
        let answer = Answer {
            status: response.status().as_u16(),
            content_type: text("content-type"),
            location: text("location"),
            allow_origin,
            allow_methods: text("access-control-allow-methods"),
            allow_headers: text("access-control-allow-headers"),
            body: Vec::new(),
        };
        let body = response.bytes().await.expect("the body is read").to_vec();
        Answer { body, ..answer }
    }
}

/// Sends one case to `base`, with no redirect followed.
async fn ask(client: &reqwest::Client, base: &str, case: &Case) -> Answer {
    let mut request = client.request(case.method.clone(), format!("{base}{}", case.path));
    for (name, value) in &case.headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.expect("the master answers");
    Answer::read(response).await
}

/// Serves `router` on a free port, and returns the port and the server task.
async fn serve(router: axum::Router) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("the bound address").port();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve the router");
    });
    (port, task)
}

/// The answers of the Go master to `cases`, under `edits`.
async fn go_answers(build: &'static GoBuild, edits: &[Edit<'_>], cases: &[Case]) -> Vec<Answer> {
    let mut go = Cluster::new(build, edits).expect("a Go run");
    go.start_master().await.expect("the Go master starts");
    let base = format!("http://127.0.0.1:{}", go.ports.master_http);
    let client = no_redirect_client();
    let mut answers = Vec::new();
    for case in cases {
        answers.push(ask(&client, &base, case).await);
    }
    go.stop();
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
    let client = no_redirect_client();
    let mut answers = Vec::new();
    for case in cases {
        answers.push(ask(&client, &base, case).await);
    }
    server.abort();
    master.stop().await;
    answers
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("the HTTP client")
}

/// The fields of the two answers that differ, one line each. A pprof body is
/// not compared, since the case says so.
fn differences(case: &Case, go: &Answer, rust: &Answer) -> Vec<String> {
    let mut found = Vec::new();
    let mut check = |field: &str, go: String, rust: String| {
        if go != rust {
            found.push(format!("  {field}: Go {go:?}, Rust {rust:?}"));
        }
    };
    check("status", go.status.to_string(), rust.status.to_string());
    check(
        "content-type",
        go.content_type.clone(),
        rust.content_type.clone(),
    );
    check("location", go.location.clone(), rust.location.clone());
    check(
        "access-control-allow-origin",
        format!("{:?}", go.allow_origin),
        format!("{:?}", rust.allow_origin),
    );
    check(
        "access-control-allow-methods",
        go.allow_methods.clone(),
        rust.allow_methods.clone(),
    );
    check(
        "access-control-allow-headers",
        go.allow_headers.clone(),
        rust.allow_headers.clone(),
    );
    match case.body {
        BodyCheck::Exact => check(
            "body",
            String::from_utf8_lossy(&go.body).into_owned(),
            String::from_utf8_lossy(&rust.body).into_owned(),
        ),
        BodyCheck::Document => {
            if normalized_document(&go.body) != normalized_document(&rust.body) {
                found.push(
                    "  body: the documents differ, as JSON with `required` sorted".to_owned(),
                );
            }
        }
        BodyCheck::Skip => {}
    }
    found
}

/// The OpenAPI document as JSON, with every `required` array sorted: Go writes
/// those arrays in a random order (`PORTING_NOTES.md` Q-8).
fn normalized_document(body: &[u8]) -> serde_json::Value {
    let mut document: serde_json::Value =
        serde_json::from_slice(body).expect("the document is JSON");
    sort_required(&mut document);
    document
}

/// Sorts every `required` array under `value`.
fn sort_required(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if let (true, serde_json::Value::Array(items)) = (key == "required", &mut *child) {
                    items.sort_by_key(|item| item.to_string());
                }
                sort_required(child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(sort_required),
        _ => {}
    }
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
    let mut failures: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for ((case, go), rust) in cases.iter().zip(&go).zip(&rust) {
        let found = differences(case, go, rust);
        if !found.is_empty() {
            failures.insert(case.name, found);
        }
    }
    assert!(
        failures.is_empty(),
        "{label}: the Go and Rust masters answer differently:\n{}",
        failures
            .iter()
            .map(|(name, found)| format!("{name}:\n{}", found.join("\n")))
            .collect::<Vec<_>>()
            .join("\n")
    );
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
