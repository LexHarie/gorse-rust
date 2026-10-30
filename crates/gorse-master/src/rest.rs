// Ports: master/rest.go (`CreateWebService`, `StartHttpServer`, `LoginFilter`) @ add21b5

//! The master's REST API (`master/rest.go`).
//!
//! Go's master serves two kinds of route on one listener. The first is its
//! go-restful WebService under `/api/`: the 42 server routes, which
//! [`gorse_server::rest::router`] serves with the master's REST state, and the
//! 23 dashboard routes under `/api/dashboard/`. A login filter guards both. The
//! second is plain `net/http` handlers (the dashboard page, login, bulk import
//! and export, dump and restore, the chat proxy), which the login filter does
//! not see. [`router`] builds both: the dashboard routes and the raw handlers
//! are explicit routes, and every other path goes to `dispatch`.
//!
//! The WebService routes, and `/apidocs.json`, go through the CORS filter in
//! `cors.rs`. The OpenAPI document and the Swagger UI page (`apidocs.rs`) and
//! the pprof handlers (`pprof.rs`) are plain routes of their own, as Go
//! registers them on the same container.

mod apidocs;
pub(crate) mod auth;
mod bulk;
mod chat;
mod config;
mod config_map;
mod cors;
pub(crate) mod dashboard_api;
mod dump;
mod pprof;
mod ranker;
mod recommend;
#[cfg(test)]
mod tests;

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use gorse_server::rest::response::plain;
use tower::util::ServiceExt;

use crate::Master;
use crate::dashboard;

/// The header the server routes read the API key from.
const API_KEY_HEADER: &str = "x-api-key";

/// Builds the master's HTTP routes. The result serves the dashboard, the raw
/// handlers and every `/api/` route Go's master serves.
pub fn router(master: Arc<Master>) -> Router {
    let server = gorse_server::rest::router(master.rest.clone());
    let api = dashboard_api::routes(master.clone());
    let raw = Router::new()
        .route("/login", get(auth::login_get).post(auth::login_post))
        .route("/logout", any(auth::logout))
        .route("/callback/oauth2", get(auth::oauth2_callback))
        .route("/api/purge", any(bulk::purge))
        .route("/api/bulk/users", any(bulk::export_import_users))
        .route("/api/bulk/items", any(bulk::export_import_items))
        .route("/api/bulk/feedback", any(bulk::export_import_feedback))
        .route("/api/dump", any(dump::dump))
        .route("/api/restore", any(dump::restore))
        .route("/api/chat/completions", any(chat::chat_completions))
        .merge(apidocs::routes())
        .merge(pprof::routes())
        .with_state(master.clone());
    let fallback = {
        let master = master.clone();
        let api = api.clone();
        tower::service_fn(move |request: Request| {
            let master = master.clone();
            let api = api.clone();
            let server = server.clone();
            async move { Ok::<_, Infallible>(dispatch(master, api, server, request).await) }
        })
    };
    Router::new()
        .merge(api)
        .merge(raw)
        .route("/apidocs.json", get(apidocs::document))
        .fallback_service(fallback)
        .layer(middleware::from_fn_with_state(master, cors::filter))
}

/// Go's `http.Redirect` with the temporary status, as `ServeMux` sends it for a
/// path that lacks the slash its pattern ends with. The body is the link Go
/// writes for GET and HEAD, and empty for any other method.
pub(super) fn temporary_redirect(method: &Method, to: &str) -> axum::response::Response {
    let body = if *method == Method::GET || *method == Method::HEAD {
        format!("<a href=\"{to}\">Temporary Redirect</a>.\n\n")
    } else {
        String::new()
    };
    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::LOCATION, to),
        ],
        body,
    )
        .into_response()
}

/// Routes a request that no explicit route matches. `/api/dashboard/` paths go
/// to the dashboard routes, and the other `/api/` paths to the server routes;
/// both are guarded as go-restful's WebService guards them. Every other path is
/// the dashboard page.
async fn dispatch(
    master: Arc<Master>,
    api: Router,
    server: Router,
    mut request: Request,
) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return dashboard_page(&master, request).await;
    }
    strip_trailing_slash(&mut request);
    let response = if request.uri().path().starts_with("/api/dashboard/") {
        api.oneshot(request).await
    } else {
        if auth::check_login(&master, request.headers()) {
            set_api_key(&master, &mut request);
        }
        server.oneshot(request).await
    };
    match response {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

/// Drops the trailing slash of a request path, keeping its query. go-restful
/// matches a route with or without the slash, so the routes here do not list
/// both forms.
fn strip_trailing_slash(request: &mut Request) {
    let uri = request.uri();
    if !uri.path().ends_with('/') {
        return;
    }
    let trimmed = uri.path().trim_end_matches('/');
    let text = match uri.query() {
        Some(query) => format!("{trimmed}?{query}"),
        None => trimmed.to_owned(),
    };
    if let Ok(stripped) = text.parse::<Uri>() {
        *request.uri_mut() = stripped;
    }
}

/// The dashboard page: `/` and the files it names, served only to a logged-in
/// request when the path is `/` or names no file (Go's `dashboard`).
async fn dashboard_page(master: &Master, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let is_root = request.uri().path() == "/";
    let names_file = path.trim_start_matches('/');
    if is_root || (!names_file.is_empty() && !is_embedded(names_file)) {
        if !auth::check_login(master, request.headers()) {
            return auth::redirect_to_login(master, request.method());
        }
        let mut response = dashboard::serve_file(request.method(), &path);
        dashboard::no_cache(&mut response);
        return response;
    }
    dashboard::serve_file(request.method(), &path)
}

/// Whether `name` is a file the dashboard bundle holds.
fn is_embedded(name: &str) -> bool {
    dashboard::Dashboard::get(name).is_some()
}

/// Go's `LoginFilter` for the WebService routes: a logged-in request has its
/// `X-API-Key` replaced by the server's key, and a request that is not logged in
/// is refused on `/api/dashboard` only.
pub(crate) async fn login_filter(
    State(master): State<Arc<Master>>,
    mut request: Request,
    next: Next,
) -> Response {
    if auth::check_login(&master, request.headers()) {
        set_api_key(&master, &mut request);
        next.run(request).await
    } else if !request.uri().path().starts_with("/api/dashboard") {
        next.run(request).await
    } else {
        plain(StatusCode::UNAUTHORIZED, "unauthorized".to_owned())
    }
}

/// Replaces the request's `X-API-Key` with the server's API key, as Go's
/// `LoginFilter` does for a logged-in request.
fn set_api_key(master: &Master, request: &mut Request) {
    let key = master.rest.config().server.api_key.clone();
    if let Ok(value) = HeaderValue::from_str(&key) {
        request.headers_mut().insert(API_KEY_HEADER, value);
    }
}

/// Go's `writeError`: the status, the message as the whole body, and the CORS
/// header. Its body is plain text.
pub(crate) fn write_error(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response = plain(status, message.into());
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

/// Go's `writeError` for a method the handler does not take.
pub(crate) fn method_not_allowed() -> Response {
    write_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
}
