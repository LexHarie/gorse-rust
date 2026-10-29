// Ports: server/rest.go @ add21b5

//! The server's REST API (`server/rest.go`).
//!
//! The 42 routes under `/api/` are built by [`router`], which also carries
//! Go's three filters: the log filter (request id, access log and request
//! event), the auth filter (the API key), and the metrics filter (the request
//! histogram). The handlers are in the submodules, one per group of routes.

mod embedding;
mod entity;
mod feedback;
mod gojson;
mod inflate;
mod items;
mod printable;
mod quota;
mod recommend;
mod response;
mod users;
mod xml;
mod xmlname;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::uri::PathAndQuery;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use gorse_common::event;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::data::types::{ItemPatch, User, UserPatch};
use gorse_storage::vectors;
use http_body_util::BodyExt;
use serde::Serialize;
use tower::ServiceExt;
use uuid::Uuid;

use crate::metrics::REST_API_REQUEST_SECONDS;

use self::response::{ApiError, ok, plain};

use self::response::go_json;

/// Go's `HealthStatus`: the readiness of the node's stores.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HealthStatus {
    /// Whether both stores answer.
    #[serde(rename = "Ready")]
    pub ready: bool,
    /// The data store's ping error, `null` when it answers.
    #[serde(rename = "DataStoreError")]
    pub data_store_error: Option<ErrorText>,
    /// The cache store's ping error, `null` when it answers.
    #[serde(rename = "CacheStoreError")]
    pub cache_store_error: Option<ErrorText>,
    /// Whether the data store answers.
    #[serde(rename = "DataStoreConnected")]
    pub data_store_connected: bool,
    /// Whether the cache store answers.
    #[serde(rename = "CacheStoreConnected")]
    pub cache_store_connected: bool,
}

/// An `error` value in a Go struct. `encoding/json` writes a Go error as the
/// JSON of its dynamic type, which for the errors the stores return is an
/// object with no exported fields, `{}`.
#[derive(Debug, Clone, Default)]
pub struct ErrorText(pub String);

impl Serialize for ErrorText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        serializer.serialize_map(Some(0))?.end()
    }
}

/// The REST server's state: the configuration and the three stores the handlers
/// read and write. [`crate::server::Server`] replaces them when the master's
/// meta changes, as Go's `Sync` does.
pub struct RestServer {
    config: RwLock<Arc<Config>>,
    cache: RwLock<Arc<dyn cache::Database>>,
    data: RwLock<Arc<dyn data::Database>>,
    vectors: RwLock<Arc<dyn vectors::Database>>,
    disable_log: bool,
}

impl RestServer {
    /// Creates the state with `config` and the given stores.
    pub fn new(
        config: Config,
        cache: Arc<dyn cache::Database>,
        data: Arc<dyn data::Database>,
        vectors: Arc<dyn vectors::Database>,
    ) -> Self {
        Self {
            config: RwLock::new(Arc::new(config)),
            cache: RwLock::new(cache),
            data: RwLock::new(data),
            vectors: RwLock::new(vectors),
            disable_log: false,
        }
    }

    /// Turns the access log and the request events off, as Go's `DisableLog`.
    #[must_use]
    pub fn with_disabled_log(mut self) -> Self {
        self.disable_log = true;
        self
    }

    /// The configuration in force now.
    pub fn config(&self) -> Arc<Config> {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the configuration.
    pub fn set_config(&self, config: Config) {
        *self.config.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(config);
    }

    /// The cache store in use now.
    pub fn cache(&self) -> Arc<dyn cache::Database> {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the cache store.
    pub fn set_cache(&self, cache: Arc<dyn cache::Database>) {
        *self.cache.write().unwrap_or_else(PoisonError::into_inner) = cache;
    }

    /// The data store in use now.
    pub fn data(&self) -> Arc<dyn data::Database> {
        self.data
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the data store.
    pub fn set_data(&self, data: Arc<dyn data::Database>) {
        *self.data.write().unwrap_or_else(PoisonError::into_inner) = data;
    }

    /// The vector store in use now.
    pub fn vectors(&self) -> Arc<dyn vectors::Database> {
        self.vectors
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the vector store.
    pub fn set_vectors(&self, vectors: Arc<dyn vectors::Database>) {
        *self.vectors.write().unwrap_or_else(PoisonError::into_inner) = vectors;
    }
}

/// Builds the `/api/` routes with their filters, in Go's order: the log
/// filter, then the auth filter, then the metrics filter, then the handler.
/// The filters wrap matched routes only (`route_layer`), as go-restful's do, so
/// an unknown path or method keeps go-restful's own 404 or 405 without an
/// `X-Request-ID`.
pub fn router(rest: Arc<RestServer>) -> Router {
    let routes = Router::new()
        .route("/api/health/live", get(health_live))
        .route("/api/health/ready", get(health_ready))
        .route("/api/user", post(users::insert_user))
        .route(
            "/api/user/{user-id}",
            patch(users::modify_user)
                .get(users::get_user)
                .delete(users::delete_user),
        )
        .route(
            "/api/users",
            post(users::insert_users).get(users::get_users),
        )
        .route("/api/item", post(items::insert_item))
        .route(
            "/api/item/{item-id}",
            patch(items::modify_item)
                .get(items::get_item)
                .delete(items::delete_item),
        )
        .route(
            "/api/items",
            get(items::get_items).post(items::insert_items),
        )
        .route(
            "/api/item/{item-id}/category/{category}",
            put(items::insert_item_category).delete(items::delete_item_category),
        )
        .route(
            "/api/feedback",
            post(feedback::insert_feedback_overwrite_false)
                .put(feedback::insert_feedback_overwrite_true)
                .get(feedback::get_feedback),
        )
        .route(
            "/api/feedback/{user-id}/{item-id}",
            get(feedback::get_user_item_feedback).delete(feedback::delete_user_item_feedback),
        )
        .route(
            "/api/feedback/{feedback-type}",
            get(feedback::get_typed_feedback),
        )
        .route(
            "/api/feedback/{feedback-type}/{user-id}/{item-id}",
            get(feedback::get_typed_user_item_feedback)
                .delete(feedback::delete_typed_user_item_feedback),
        )
        .route(
            "/api/user/{user-id}/feedback/{feedback-type}",
            get(feedback::get_typed_feedback_by_user),
        )
        .route(
            "/api/user/{user-id}/feedback",
            get(feedback::get_feedback_by_user),
        )
        .route(
            "/api/item/{item-id}/feedback/{feedback-type}",
            get(feedback::get_typed_feedback_by_item),
        )
        .route(
            "/api/item/{item-id}/feedback",
            get(feedback::get_feedback_by_item),
        )
        .route(
            "/api/collaborative-filtering/{user-id}",
            get(recommend::get_collaborative_filtering),
        )
        .route(
            "/api/collaborative-filtering/{user-id}/{category}",
            get(recommend::get_collaborative_filtering),
        )
        .route("/api/latest", get(recommend::get_latest))
        .route("/api/latest/{category}", get(recommend::get_latest))
        .route(
            "/api/non-personalized/{name}",
            get(recommend::get_non_personalized),
        )
        .route(
            "/api/item-to-item/{name}/{item-id}",
            get(recommend::get_item_to_item),
        )
        .route(
            "/api/user-to-user/{name}/{user-id}",
            get(recommend::get_user_to_user),
        )
        .route(
            "/api/item/{item-id}/neighbors",
            get(recommend::get_item_neighbors),
        )
        .route(
            "/api/item/{item-id}/neighbors/{category}",
            get(recommend::get_item_neighbors),
        )
        .route(
            "/api/user/{user-id}/neighbors",
            get(recommend::get_user_neighbors),
        )
        .route("/api/recommend/{user-id}", get(recommend::get_recommend))
        .route(
            "/api/recommend/{user-id}/{category}",
            get(recommend::get_recommend),
        )
        .route("/api/session/recommend", post(recommend::session_recommend))
        .route(
            "/api/session/recommend/{category}",
            post(recommend::session_recommend),
        )
        .route_layer(middleware::from_fn_with_state(rest.clone(), metrics_filter))
        .route_layer(middleware::from_fn_with_state(rest.clone(), auth_filter))
        .route_layer(middleware::from_fn_with_state(rest.clone(), log_filter))
        .with_state(rest);
    Router::new().fallback_service(routes.map_request(trim_trailing_slash))
}

/// Go's router ignores a trailing slash, so `/api/latest/` is `/api/latest`.
/// The path is trimmed before the routes see it, and the routes have none.
fn trim_trailing_slash(mut request: Request) -> Request {
    let path = request.uri().path();
    if path.len() <= 1 || !path.ends_with('/') {
        return request;
    }
    let trimmed = match path.trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    };
    let path_and_query = match request.uri().query() {
        Some(query) => format!("{trimmed}?{query}"),
        None => trimmed.to_owned(),
    };
    let mut parts = request.uri().clone().into_parts();
    if let Ok(path_and_query) = PathAndQuery::from_maybe_shared(path_and_query) {
        parts.path_and_query = Some(path_and_query);
        if let Ok(uri) = Uri::from_parts(parts) {
            *request.uri_mut() = uri;
        }
    }
    request
}

/// Compile-time check (Q-254): the REST state is shared by every request task
/// and by the sync task, so it must be `Send` and `Sync`.
const fn assert_send_sync<T: Send + Sync>() {}
const _: () = assert_send_sync::<RestServer>();

/// Go's `LogFilter`: gives the response a fresh `X-Request-ID`, writes the
/// access log, and emits the request event, unless the log is disabled or the
/// route is one the dashboard polls.
async fn log_filter(State(rest): State<Arc<RestServer>>, request: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let route = matched_route(&request);
    let remote_addr = String::new();
    let start_time = gotime::now();
    let start = Instant::now();
    // Go counts the bytes the handler reads from the body (`CountingReadCloser`),
    // not the declared length, so the body is wrapped to count what is read.
    let (parts, body) = request.into_parts();
    let bytes_read = Arc::new(AtomicI64::new(0));
    let counter = bytes_read.clone();
    let body = Body::new(body.map_frame(move |frame| {
        if let Some(data) = frame.data_ref() {
            counter.fetch_add(
                i64::try_from(data.len()).unwrap_or(i64::MAX),
                Ordering::Relaxed,
            );
        }
        frame
    }));
    let mut response = next.run(Request::from_parts(parts, body)).await;
    let request_bytes = bytes_read.load(Ordering::Relaxed);
    let response_time = start.elapsed();
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("X-Request-ID", value);
    }
    let status = response.status().as_u16();
    if !rest.disable_log && path != "/api/dashboard/cluster" && path != "/api/dashboard/tasks" {
        tracing::info!(
            request_id = %request_id,
            status_code = status,
            response_time = ?response_time,
            remote_addr = %remote_addr,
            "{method} {path}"
        );
        let request = event::Request {
            request_id,
            request_bytes,
            method,
            route,
            status_code: i64::from(status),
            response_time: gotime::Duration(
                i64::try_from(response_time.as_nanos()).unwrap_or(i64::MAX),
            ),
            response_bytes: response_size(&response),
            timestamp: start_time,
            remote_addr,
        };
        // Go emits the event on a goroutine, so the response does not wait for
        // the handler of the event.
        tokio::task::spawn_blocking(move || event::emit(&Context::background(), request));
    }
    response
}

/// Go's `AuthFilter`: when `server.api_key` is set, every route except the
/// health checks needs the same key in `X-API-Key`.
async fn auth_filter(
    State(rest): State<Arc<RestServer>>,
    request: Request,
    next: Next,
) -> Response {
    if matched_route(&request).starts_with("/api/health/") {
        return next.run(request).await;
    }
    let config = rest.config();
    if config.server.api_key.is_empty() {
        return next.run(request).await;
    }
    let api_key = request
        .headers()
        .get("X-API-Key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if api_key == config.server.api_key {
        return next.run(request).await;
    }
    tracing::error!(
        api_key = %config.server.api_key,
        x_api_key = %api_key,
        "unauthorized"
    );
    plain(StatusCode::UNAUTHORIZED, "unauthorized".to_owned())
}

/// Go's `MetricsFilter`: observes the duration of each request that succeeds,
/// labelled with its method and route template. The dashboard's routes are not
/// observed.
async fn metrics_filter(
    State(_rest): State<Arc<RestServer>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let route = matched_route(&request);
    let start = Instant::now();
    let response = next.run(request).await;
    if response.status() == StatusCode::OK && !route.starts_with("/api/dashboard") {
        REST_API_REQUEST_SECONDS
            .with_label_values(&[format!("{method} {route}").as_str()])
            .observe(start.elapsed().as_secs_f64());
    }
    response
}

/// The route template the request matched, such as `/api/user/{user-id}`, or
/// `""` when none did.
fn matched_route(request: &Request) -> String {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(String::new, |path| path.as_str().to_owned())
}

fn response_size(response: &Response) -> i64 {
    use axum::body::HttpBody;
    response
        .body()
        .size_hint()
        .exact()
        .and_then(|size| i64::try_from(size).ok())
        .unwrap_or(0)
}

/// Go's `checkHealth`: pings both stores.
async fn check_health(rest: &RestServer) -> HealthStatus {
    let data_store_error = rest.data().ping().await.err();
    let cache_store_error = rest.cache().ping().await.err();
    let data_store_connected = data_store_error.is_none();
    let cache_store_connected = cache_store_error.is_none();
    HealthStatus {
        ready: data_store_connected && cache_store_connected,
        data_store_error: data_store_error.map(|_| ErrorText::default()),
        cache_store_error: cache_store_error.map(|_| ErrorText::default()),
        data_store_connected,
        cache_store_connected,
    }
}

/// Go's `checkLive`: always answers with the health status.
async fn health_live(State(rest): State<Arc<RestServer>>) -> Result<Response, ApiError> {
    let status = check_health(&rest).await;
    ok(&status)
}

/// Go's `checkReady`: answers with the health status, or 503 with that status
/// as its text when a store does not answer.
async fn health_ready(State(rest): State<Arc<RestServer>>) -> Result<Response, ApiError> {
    let status = check_health(&rest).await;
    if status.ready {
        return ok(&status);
    }
    let reason = go_json(&status).map_err(ApiError::internal)?;
    let reason = String::from_utf8_lossy(&reason).into_owned();
    Err(ApiError::Status(StatusCode::SERVICE_UNAVAILABLE, reason))
}

/// The request body of the route `target`, decoded as the route reads it, and
/// printed as Go's `json.Marshal` prints the decoded value. The error is the
/// text Go gives. For the conformance tests, which check it against Go's
/// `ReadEntity` (`porting/evidence/wp-4-2/go-readentity-probe`).
///
/// `content_type` and `content_encoding` are empty when the request has no such
/// header.
#[doc(hidden)]
pub fn decode_body_json(
    target: &str,
    content_type: &str,
    content_encoding: &str,
    body: &[u8],
) -> Result<String, String> {
    let mut headers = HeaderMap::new();
    if !content_type.is_empty() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).map_err(|err| err.to_string())?,
        );
    }
    if !content_encoding.is_empty() {
        headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_str(content_encoding).map_err(|err| err.to_string())?,
        );
    }
    let body = axum::body::Bytes::copy_from_slice(body);
    match target {
        "user" => printed(entity::read_entity::<User>(&headers, &body)),
        "users" => printed(entity::read_entity::<Vec<User>>(&headers, &body)),
        "user-patch" => printed(entity::read_entity::<UserPatch>(&headers, &body)),
        "item" => printed(entity::read_entity::<items::Item>(&headers, &body)),
        "items" => printed(entity::read_entity::<Vec<items::Item>>(&headers, &body)),
        "item-patch" => printed(entity::read_entity::<ItemPatch>(&headers, &body)),
        "feedback" => printed(entity::read_entity::<Vec<feedback::Feedback>>(
            &headers, &body,
        )),
        other => Err(format!("no route reads {other}")),
    }
}

/// The decoded value as `json.Marshal` writes it, or the decoder's error. A value
/// that decodes but cannot be written is `MARSHAL ERROR`, as the probe prints it.
fn printed<T: serde::Serialize>(result: Result<T, ApiError>) -> Result<String, String> {
    match result {
        Ok(value) => Ok(match gorse_common::jsonutil::marshal(&value) {
            Ok(json) => String::from_utf8_lossy(&json).into_owned(),
            Err(_) => "MARSHAL ERROR".to_owned(),
        }),
        Err(ApiError::BadRequest(text)) => Err(text),
        Err(_) => Err("a decode error is a bad request".to_owned()),
    }
}
