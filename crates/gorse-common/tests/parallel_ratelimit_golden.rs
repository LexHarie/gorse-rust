//! Gorse's `common/parallel/ratelimit.go` against Go:
//! `tests/data/parallel_ratelimit_golden.json`, written by
//! `contracts/tools/ratelimitgolden -gorse-out`. The limiters are
//! process-wide, so this file holds a single test, which starts from the
//! initial values as the Go tool does.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use gorse_common::gotime::Duration;
use gorse_common::parallel::{self, RateLimiter};
use gorse_ratelimit::Bucket;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    chat_completion: Vec<InitCall>,
    embedding: Vec<InitCall>,
}

#[derive(Deserialize)]
struct InitCall {
    rpm: i64,
    tpm: i64,
    panic: String,
    backoff: i64,
    requests: Limiter,
    tokens: Limiter,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Limiter {
    kind: String,
    capacity: i64,
    rate: String,
}

/// Go's type switch on the limiter.
fn limiter(l: Arc<dyn RateLimiter>) -> Limiter {
    let any: Arc<dyn Any + Send + Sync> = l;
    if any.is::<parallel::Unlimited>() {
        return Limiter {
            kind: "unlimited".to_owned(),
            capacity: 0,
            rate: String::new(),
        };
    }
    let bucket = any.downcast::<Bucket>().expect("a bucket or unlimited");
    Limiter {
        kind: "bucket".to_owned(),
        capacity: bucket.capacity(),
        rate: format!("{:x}", bucket.rate().to_bits()),
    }
}

fn catch(f: impl FnOnce()) -> String {
    panic::catch_unwind(AssertUnwindSafe(f))
        .err()
        .map(|payload| {
            payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

type Api = (
    &'static str,
    fn(i64, i64),
    fn() -> Duration,
    fn() -> Arc<dyn RateLimiter>,
    fn() -> Arc<dyn RateLimiter>,
);

#[test]
fn init_limiters_match_go() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/parallel_ratelimit_golden.json"
    );
    let golden: Golden =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read the golden file"))
            .expect("parse the golden file");
    let apis: [(Api, &[InitCall]); 2] = [
        (
            (
                "InitChatCompletionLimiters",
                parallel::init_chat_completion_limiters,
                parallel::chat_completion_backoff,
                parallel::chat_completion_requests_limiter,
                parallel::chat_completion_tokens_limiter,
            ),
            &golden.chat_completion,
        ),
        (
            (
                "InitEmbeddingLimiters",
                parallel::init_embedding_limiters,
                parallel::embedding_backoff,
                parallel::embedding_requests_limiter,
                parallel::embedding_tokens_limiter,
            ),
            &golden.embedding,
        ),
    ];
    let mut panics = 0;
    for ((name, init, backoff, requests, tokens), calls) in apis {
        assert_eq!(calls.len(), 16);
        // Before any call, as Go's package variables start.
        assert_eq!(backoff(), Duration(0));
        assert_eq!(limiter(requests()).kind, "unlimited");
        assert_eq!(limiter(tokens()).kind, "unlimited");
        for call in calls {
            let at = format!("{name}({}, {})", call.rpm, call.tpm);
            assert_eq!(catch(|| init(call.rpm, call.tpm)), call.panic, "{at}: panic");
            assert_eq!(backoff(), Duration(call.backoff), "{at}: backoff");
            assert_eq!(limiter(requests()), call.requests, "{at}: requests limiter");
            assert_eq!(limiter(tokens()), call.tokens, "{at}: tokens limiter");
            panics += usize::from(!call.panic.is_empty());
        }
    }
    assert_eq!(panics, 8);
}
