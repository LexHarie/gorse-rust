// Ports: server/rest.go (`StartHttpServer`, the `/debug/pprof/` routes) and Go's `net/http/pprof` (Go 1.27) @ add21b5

//! The pprof handlers (X-254).
//!
//! The routes are Go's, and so are the status codes and the content types of
//! their answers. The bodies are not. A Go profile is sampled when it is asked
//! for, and the Rust master has no sampler, so a profile route answers an empty
//! body, and `profile` and `trace` answer at once rather than after `seconds`.
//! The index lists the profiles, and `cmdline` gives the process arguments, as
//! Go does. The symbol route answers its GET as Go does, and a POST with no
//! symbols, since there is no symbol table to read.
//!
//! The handlers are plain ones, outside the CORS filter and the login check, as
//! Go registers them.

use axum::Router;
use axum::extract::Path;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};

use super::temporary_redirect;

/// The named profiles Go's `pprof.Handler` serves, as Go 1.27 lists them.
const PROFILES: [&str; 7] = [
    "allocs",
    "block",
    "goroutine",
    "goroutineleak",
    "heap",
    "mutex",
    "threadcreate",
];

/// The routes of the pprof handlers. `/debug/pprof` redirects to its index, the
/// index is at `/debug/pprof/`, and every other path under it is a named profile
/// or an unknown one.
pub(super) fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/debug/pprof", get(redirect_to_index))
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/{*name}", any(named))
}

/// Go's redirect from `/debug/pprof` to `/debug/pprof/`.
async fn redirect_to_index(method: Method) -> Response {
    temporary_redirect(&method, "/debug/pprof/")
}

/// Go's `/debug/pprof/`: the index, with a link to each profile.
async fn index() -> Response {
    let links: String = PROFILES
        .iter()
        .map(|name| format!("<a href=\"/debug/pprof/{name}\">{name}</a><br>\n"))
        .collect();
    let body = format!(
        "<html>\n<head>\n<title>/debug/pprof/</title>\n</head>\n<body>\n/debug/pprof/<br>\n<br>\n\
         profiles:<br>\n{links}<a href=\"/debug/pprof/cmdline\">cmdline</a><br>\n\
         <a href=\"/debug/pprof/profile\">profile</a><br>\n\
         <a href=\"/debug/pprof/symbol\">symbol</a><br>\n\
         <a href=\"/debug/pprof/trace\">trace</a><br>\n</body>\n</html>\n"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

/// A path under `/debug/pprof/`. A named profile answers as Go's handler does,
/// and a name Go does not know is a 404 with the text `Unknown profile`.
async fn named(method: Method, Path(name): Path<String>) -> Response {
    match name.as_str() {
        "cmdline" => text(std::env::args().collect::<Vec<_>>().join("\0")),
        "symbol" if method == Method::GET => text("num_symbols: 1\n".to_owned()),
        "symbol" => text(String::new()),
        "profile" | "trace" => binary(),
        profile if PROFILES.contains(&profile) => binary(),
        _ => unknown_profile(),
    }
}

/// Go's text answer: `text/plain`, with the body as given.
fn text(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

/// A profile's answer. Go sends the profile's bytes; the Rust master has none to
/// send, so the body is empty.
fn binary() -> Response {
    (
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Vec::new(),
    )
        .into_response()
}

/// Go's answer to a name no profile has: a 404 with `Unknown profile`, and the
/// `X-Go-Pprof` header Go sets on its profile errors.
fn unknown_profile() -> Response {
    (
        StatusCode::NOT_FOUND,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::HeaderName::from_static("x-go-pprof"), "1"),
        ],
        "Unknown profile\n",
    )
        .into_response()
}
