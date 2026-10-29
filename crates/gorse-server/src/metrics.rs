// Ports: server/metrics.go @ add21b5

//! The server's Prometheus metrics (`server/metrics.go`).

use std::sync::LazyLock;

use prometheus::{HistogramOpts, HistogramVec};

/// Go's `RestAPIRequestSecondsVec`: the duration of each successful REST API
/// request, labelled `api` with the method and the route template, such as
/// `GET /api/user/{user-id}`. It is registered once, on the default registry,
/// as `promauto` does.
pub static REST_API_REQUEST_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    // Go registers this histogram with an empty help text. The Rust crate
    // rejects an empty help text, so the text here is a short description
    // (recorded as a deviation in `porting/notes/WP-4-2.md`).
    let vec = HistogramVec::new(
        HistogramOpts::new(
            "rest_api_request_seconds",
            "Duration of REST API requests in seconds.",
        )
        .namespace("gorse")
        .subsystem("server"),
        &["api"],
    )
    .expect("the server histogram has a valid name and labels");
    prometheus::register(Box::new(vec.clone())).expect("the server histogram is registered once");
    vec
});
