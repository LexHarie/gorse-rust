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
//! are explicit routes, and every other path goes to [`dispatch`].

pub(crate) mod auth;
mod bulk;
mod chat;
mod config;
mod config_map;
pub(crate) mod dashboard_api;
mod dump;
mod ranker;
mod recommend;

use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use gorse_server::rest::response::{ApiError, plain};
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
        .with_state(master.clone());
    let fallback = {
        let master = master.clone();
        tower::service_fn(move |request: Request| {
            let master = master.clone();
            let server = server.clone();
            async move { Ok::<_, Infallible>(dispatch(master, server, request).await) }
        })
    };
    Router::new()
        .merge(api)
        .merge(raw)
        .fallback_service(fallback)
}

/// Routes a request that no explicit route matches. `/api/` paths go to the
/// server routes, which are guarded by the login filter as go-restful's
/// WebService guards them. Every other path is the dashboard page.
async fn dispatch(master: Arc<Master>, server: Router, mut request: Request) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return dashboard_page(&master, request).await;
    }
    if auth::check_login(&master, request.headers()) {
        set_api_key(&master, &mut request);
    }
    match server.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
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
        ApiError::Status(StatusCode::UNAUTHORIZED, "unauthorized".to_owned()).into_response()
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
