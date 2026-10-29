// Ports: server/rest_test.go (the suite's SetupSuite, SetupTest and helpers) @
// add21b5

//! The REST suite of `server/rest_test.go`. Go's suite shares one set of
//! stores between its methods; each test here opens its own, so no test sees
//! another's rows. The expectations are Go's. JSON bodies compare as values, as
//! `apitest`'s `JSONEq` does, and every other body compares as text.

mod emit;
mod feedback;
mod health;
mod items;
mod quota;
mod recommend;
mod search;
mod users;
mod visibility;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use gorse_config::config::defaults::get_default_config;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::vectors;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use super::RestServer;
use super::router;

/// The API key the suite configures, as Go's `apiKey`.
pub(super) const API_KEY: &str = "test_api_key";

/// One test's stores, configuration and router. The directory of the SQLite
/// files and the vector store lives as long as the suite.
pub(super) struct Suite {
    _dir: TempDir,
    pub rest: Arc<RestServer>,
    pub data: Arc<dyn data::Database>,
    pub cache: Arc<dyn cache::Database>,
    pub vectors: Arc<dyn vectors::Database>,
}

impl Suite {
    /// Opens and initialises the stores, with Go's `SetupSuite` and the
    /// per-test settings of `SetupTest`, then lets `configure` change them.
    pub(super) async fn new(configure: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut data_registry = data::Registry::default();
        data::sql::register(&mut data_registry);
        let data: Arc<dyn data::Database> = Arc::from(
            data_registry
                .open(&format!("sqlite://{}/data.db", dir.path().display()), "", &[])
                .await
                .expect("open the data store"),
        );
        let mut cache_registry = cache::Registry::default();
        cache::sql::register(&mut cache_registry);
        let cache: Arc<dyn cache::Database> = Arc::from(
            cache_registry
                .open(&format!("sqlite://{}/cache.db", dir.path().display()), "", &[])
                .await
                .expect("open the cache store"),
        );
        let vectors: Arc<dyn vectors::Database> = Arc::from(
            vectors::open(&format!("xvec://{}/vectors", dir.path().display()), "", &[])
                .await
                .expect("open the vector store"),
        );
        data.init().await.expect("init the data store");
        cache.init().await.expect("init the cache store");
        vectors.init().await.expect("init the vector store");

        let mut config = get_default_config();
        config.server.api_key = API_KEY.to_owned();
        config.recommend.collaborative.kind = "mf".to_owned();
        config.recommend.ranker.kind = "fm".to_owned();
        config.recommend.fallback.recommenders = Some(vec!["latest".to_owned()]);
        configure(&mut config);
        let rest = Arc::new(RestServer::new(
            config,
            cache.clone(),
            data.clone(),
            vectors.clone(),
        ));
        Self {
            _dir: dir,
            rest,
            data,
            cache,
            vectors,
        }
    }

    /// The suite with Go's default settings.
    pub(super) async fn default_suite() -> Self {
        Self::new(|_| {}).await
    }

    /// Replaces the configuration, as Go's tests assign `suite.Config`.
    pub(super) fn set_config(&self, config: Config) {
        self.rest.set_config(config);
    }

    /// Changes one setting of the configuration and keeps the rest, as a Go
    /// test does with `suite.Config.Quota.MaxUsersCount = 1`.
    pub(super) fn update_config(&self, change: impl FnOnce(&mut Config)) {
        let mut config = (*self.rest.config()).clone();
        change(&mut config);
        self.rest.set_config(config);
    }

    /// Sends one request through the API router and reads the whole reply.
    pub(super) async fn send(&self, request: Req) -> Reply {
        let app: Router = router(self.rest.clone());
        let response = app
            .oneshot(request.build())
            .await
            .expect("the router is infallible");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("read the body")
            .to_bytes();
        Reply {
            status,
            headers,
            body: String::from_utf8(body.to_vec()).expect("a UTF-8 body"),
        }
    }
}

/// A request to send, built with the methods below.
pub(super) struct Req {
    method: Method,
    uri: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

/// A request with `method` to `uri`.
pub(super) fn req(method: Method, uri: &str) -> Req {
    Req {
        method,
        uri: uri.to_owned(),
        headers: Vec::new(),
        body: None,
    }
}

/// A `GET` request to `uri`.
pub(super) fn get(uri: &str) -> Req {
    req(Method::GET, uri)
}

/// A `POST` request to `uri`.
pub(super) fn post(uri: &str) -> Req {
    req(Method::POST, uri)
}

/// A `PUT` request to `uri`.
pub(super) fn put(uri: &str) -> Req {
    req(Method::PUT, uri)
}

/// A `PATCH` request to `uri`.
pub(super) fn patch(uri: &str) -> Req {
    req(Method::PATCH, uri)
}

/// A `DELETE` request to `uri`.
pub(super) fn delete(uri: &str) -> Req {
    req(Method::DELETE, uri)
}

impl Req {
    /// Adds a header.
    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Adds the suite's API key, as the Go tests' `Header("X-API-Key", apiKey)`.
    pub(super) fn key(self) -> Self {
        self.header("X-API-Key", API_KEY)
    }

    /// Sets a JSON body from a value, as `apitest`'s `JSON` does.
    pub(super) fn json(mut self, body: &serde_json::Value) -> Self {
        self.body = Some(body.to_string());
        self.headers
            .push(("Content-Type".to_owned(), "application/json".to_owned()));
        self
    }

    /// Sets a body as it is.
    pub(super) fn raw(mut self, body: &str) -> Self {
        self.body = Some(body.to_owned());
        self
    }

    fn build(self) -> Request<Body> {
        let mut builder = Request::builder().method(self.method).uri(self.uri);
        for (name, value) in self.headers {
            builder = builder.header(name, value);
        }
        let body = match self.body {
            Some(body) => Body::from(body),
            None => Body::empty(),
        };
        builder.body(body).expect("a valid request")
    }
}

/// A reply read in full.
pub(super) struct Reply {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: String,
}

impl Reply {
    /// Asserts the status code.
    #[track_caller]
    pub(super) fn status(&self, expected: StatusCode) -> &Self {
        assert_eq!(self.status, expected, "body: {}", self.body);
        self
    }

    /// Asserts a JSON body equal to `expected`, comparing values, as `apitest`
    /// does with `JSONEq`.
    #[track_caller]
    pub(super) fn json(&self, expected: serde_json::Value) -> &Self {
        let actual: serde_json::Value = serde_json::from_str(&self.body)
            .unwrap_or_else(|err| panic!("body is not JSON ({err}): {}", self.body));
        assert_eq!(as_float64s(actual), as_float64s(expected), "body: {}", self.body);
        self
    }

    /// Asserts a body equal to `expected` as text.
    #[track_caller]
    pub(super) fn text(&self, expected: &str) -> &Self {
        assert_eq!(self.body, expected);
        self
    }

    /// Asserts that the header is present.
    #[track_caller]
    pub(super) fn header_present(&self, name: &str) -> &Self {
        assert!(self.headers.contains_key(name), "missing header {name}");
        self
    }
}

/// Reads every number as a `float64`, as `apitest`'s `JSONEq` does by
/// unmarshalling both sides into `interface{}`. Go writes `946684800` for a
/// whole `float64`, and this keeps that equal to `946684800.0`.
fn as_float64s(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(number) => {
            let float = number.as_f64().expect("a JSON number is an f64");
            serde_json::Number::from_f64(float).map_or(Value::Null, Value::Number)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(as_float64s).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, as_float64s(value)))
                .collect(),
        ),
        other => other,
    }
}

/// The `Content-Type` a reply carries.
pub(super) fn content_type(reply: &Reply) -> Option<&str> {
    reply
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
}
