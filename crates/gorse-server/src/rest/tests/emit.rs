// Ports: server/rest_test.go (TestEmitRequest) @ add21b5

//! The request event each handled request emits, as Go's `TestEmitRequest`.
//! The handler is process-wide, as in Go, so the test waits for the event
//! that carries its own request id and ignores the rest.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration as StdDuration;

use axum::http::StatusCode;
use gorse_common::event::{self, Handler, NopHandler, Request};
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use serde_json::json;

use super::{Suite, post};

/// Sends every request event to a channel, as Go's `requestsHandler` does.
struct RequestsHandler {
    requests: Sender<Request>,
}

impl Handler for RequestsHandler {
    fn emit_request(&self, _ctx: &Context, event: &Request) {
        let _ = self.requests.send(event.clone());
    }

    fn emit_snapshot(&self, _ctx: &Context, _event: &gorse_common::event::Snapshot) {}
}

/// Restores the no-op handler when the test ends, even by a panic, as Go's
/// `Cleanup` does.
struct RestoreHandler;

impl Drop for RestoreHandler {
    fn drop(&mut self) {
        event::set_event_handler(Arc::new(NopHandler));
    }
}

/// Waits for the event of the request with `request_id`, up to one second.
fn wait_for_request(requests: &Receiver<Request>, request_id: &str) -> Option<Request> {
    let deadline = StdDuration::from_secs(1);
    loop {
        match requests.recv_timeout(deadline) {
            Ok(request) if request.request_id == request_id => return Some(request),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

#[tokio::test]
async fn emit_request() {
    let (sender, requests) = mpsc::channel();
    event::set_event_handler(Arc::new(RequestsHandler { requests: sender }));
    let _restore = RestoreHandler;
    let suite = Suite::default_suite().await;

    let body = json!({"UserId": "emit-request", "Labels": null, "Comment": ""}).to_string();
    let before = gotime::now();
    let reply = suite
        .send(
            post("/api/user")
                .key()
                .raw(&body)
                .header("Content-Type", "application/json"),
        )
        .await;
    let after = gotime::now();
    reply.status(StatusCode::OK).json(json!({"RowAffected": 1}));

    let request_id = reply
        .headers
        .get("X-Request-ID")
        .and_then(|value| value.to_str().ok())
        .expect("the reply carries X-Request-ID")
        .to_owned();
    assert!(!request_id.is_empty());
    let request = wait_for_request(&requests, &request_id).expect("request event was not emitted");
    assert_eq!(request.request_id, request_id);
    assert_eq!(request.request_bytes, body.len() as i64);
    assert_eq!(request.method, "POST");
    assert_eq!(request.route, "/api/user");
    assert_eq!(request.status_code, i64::from(StatusCode::OK.as_u16()));
    assert!(request.response_time.nanoseconds() > 0);
    assert_eq!(request.response_bytes, reply.body.len() as i64);
    assert!(!request.timestamp.before(&before));
    assert!(!request.timestamp.after(&after));
    assert!(request.remote_addr.is_empty());
}
