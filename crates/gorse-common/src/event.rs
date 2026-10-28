// Ports: common/event/event.go @ add21b5

//! Request and snapshot events, and the process-wide handler that receives them.
//!
//! Go's `Emit[T Request | Snapshot]` dispatches on the dynamic type. Here the
//! [`Emittable`] trait picks the handler method for each event type.

use std::sync::{Arc, LazyLock, RwLock};

use crate::gocontext::Context;
use crate::gotime::{Duration, Time};

/// One handled HTTP request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Unique request identifier (X-Request-ID).
    pub request_id: String,
    /// Number of request body bytes read by the handler.
    pub request_bytes: i64,
    /// HTTP method (GET, POST, PUT, DELETE, PATCH).
    pub method: String,
    /// API route template (e.g., /api/recommend/{user-id}).
    pub route: String,
    /// HTTP response status code.
    pub status_code: i64,
    /// Response time.
    pub response_time: Duration,
    /// Number of response body bytes written by the handler.
    pub response_bytes: i64,
    /// Event timestamp.
    pub timestamp: Time,
    /// Client remote address.
    pub remote_addr: String,
}

/// Dataset sizes at one point in a training run.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Number of users.
    pub user_count: i64,
    /// Bytes the user table takes.
    pub user_bytes: i64,
    /// Number of items.
    pub item_count: i64,
    /// Bytes the item table takes.
    pub item_bytes: i64,
    /// Number of feedback records.
    pub feedback_count: i64,
    /// Bytes the feedback table takes.
    pub feedback_bytes: i64,
    /// Event timestamp.
    pub timestamp: Time,
}

/// Receives events. Go's `event.Handler`.
pub trait Handler: Send + Sync {
    /// Receives one request event.
    fn emit_request(&self, ctx: &Context, event: &Request);
    /// Receives one snapshot event.
    fn emit_snapshot(&self, ctx: &Context, event: &Snapshot);
}

/// Discards every event. Go's `event.NopHandler`, the default handler.
#[derive(Debug, Default, Clone, Copy)]
pub struct NopHandler;

impl Handler for NopHandler {
    fn emit_request(&self, _ctx: &Context, _event: &Request) {}

    fn emit_snapshot(&self, _ctx: &Context, _event: &Snapshot) {}
}

static HANDLER: LazyLock<RwLock<Arc<dyn Handler>>> =
    LazyLock::new(|| RwLock::new(Arc::new(NopHandler)));

/// Go's `event.SetEventHandler`: replaces the process-wide handler.
pub fn set_event_handler(handler: Arc<dyn Handler>) {
    *HANDLER.write().unwrap_or_else(|e| e.into_inner()) = handler;
}

fn current_handler() -> Arc<dyn Handler> {
    Arc::clone(&HANDLER.read().unwrap_or_else(|e| e.into_inner()))
}

/// The event types [`emit`] accepts: Go's `Request | Snapshot` constraint.
pub trait Emittable {
    /// Passes the event to the matching method of `handler`.
    fn emit_to(&self, ctx: &Context, handler: &dyn Handler);
}

impl Emittable for Request {
    fn emit_to(&self, ctx: &Context, handler: &dyn Handler) {
        handler.emit_request(ctx, self);
    }
}

impl Emittable for Snapshot {
    fn emit_to(&self, ctx: &Context, handler: &dyn Handler) {
        handler.emit_snapshot(ctx, self);
    }
}

/// Go's `event.Emit`: hands `event` to the current handler.
pub fn emit<T: Emittable>(ctx: &Context, event: T) {
    event.emit_to(ctx, current_handler().as_ref());
}

#[cfg(test)]
mod tests {
    // Ports: common/event/event_test.go @ add21b5

    use std::sync::Mutex;

    use super::*;
    use crate::gotime;

    /// Marks the context a handler receives, as the Go mock's `t.Context()` does.
    #[derive(Debug, PartialEq)]
    struct Marker(i64);

    #[derive(Debug, PartialEq)]
    enum Call {
        Request(Request, Option<i64>),
        Snapshot(Snapshot, Option<i64>),
    }

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<Call>>,
    }

    impl Handler for Recorder {
        fn emit_request(&self, ctx: &Context, event: &Request) {
            let marker = ctx.value::<Marker>().map(|m| m.0);
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Call::Request(event.clone(), marker));
        }

        fn emit_snapshot(&self, ctx: &Context, event: &Snapshot) {
            let marker = ctx.value::<Marker>().map(|m| m.0);
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Call::Snapshot(event.clone(), marker));
        }
    }

    #[test]
    fn set_event_recorder() {
        let now = gotime::now();
        let request = Request {
            request_id: "request-id".into(),
            request_bytes: 0,
            method: "GET".into(),
            route: "/api/recommend/{user-id}".into(),
            status_code: 200,
            response_time: gotime::Duration(10 * gotime::MILLISECOND.0),
            response_bytes: 0,
            timestamp: now.clone(),
            remote_addr: "127.0.0.1".into(),
        };
        let snapshot = Snapshot {
            user_count: 1,
            user_bytes: 0,
            item_count: 2,
            item_bytes: 0,
            feedback_count: 3,
            feedback_bytes: 0,
            timestamp: now,
        };
        let recorder = Arc::new(Recorder::default());
        set_event_handler(Arc::clone(&recorder) as Arc<dyn Handler>);
        let ctx = Context::background().with_value(Marker(7));
        emit(&ctx, request.clone());
        emit(&ctx, snapshot.clone());
        set_event_handler(Arc::new(NopHandler));
        // Each event reached the handler exactly once, with the caller's context.
        assert_eq!(
            *recorder.calls.lock().unwrap_or_else(|e| e.into_inner()),
            vec![
                Call::Request(request, Some(7)),
                Call::Snapshot(snapshot, Some(7)),
            ]
        );
    }
}
