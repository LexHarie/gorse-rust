// Ports: common/parallel/ratelimit.go @ add21b5

//! Gorse's OpenAI rate limiters: per-minute limits on chat-completion and
//! embedding requests and tokens, set from the config at master start-up.
//!
//! Go keeps them in exported package variables, which start as
//! [`Unlimited`] with no backoff; here they sit behind locks, read through
//! the getters and set by [`init_chat_completion_limiters`] and
//! [`init_embedding_limiters`]. In add21b5 only `master/master.go:137-138`
//! uses this file, to call the two init functions; nothing reads the limiters
//! or the backoffs yet.
//!
//! The limiters are juju/ratelimit token buckets ([`gorse_ratelimit`],
//! LGPLv3 with a static-linking exception) refilled every second with a
//! sixtieth of the per-minute limit. A limit from 1 to 59 makes that zero,
//! and the bucket panics with `token bucket capacity is not > 0`: Gorse's
//! master fails to start, and so does this port.

use std::any::Any;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use gorse_ratelimit::Bucket;

use crate::gotime::{Duration, MINUTE, SECOND};

/// Go's `RateLimiter` interface.
pub trait RateLimiter: Any + Send + Sync {
    /// Takes `count` tokens and returns how long the caller should wait
    /// before using them.
    fn take(&self, count: i64) -> Duration;
}

/// Go's `Unlimited`: never waits.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unlimited;

impl RateLimiter for Unlimited {
    fn take(&self, _count: i64) -> Duration {
        Duration(0)
    }
}

/// Go's `*ratelimit.Bucket`, which implements `RateLimiter`.
impl RateLimiter for Bucket {
    fn take(&self, count: i64) -> Duration {
        Duration(Bucket::take(self, count))
    }
}

/// One API's backoff and limiters, Go's three package variables.
struct Limiters {
    backoff: Duration,
    requests: Arc<dyn RateLimiter>,
    tokens: Arc<dyn RateLimiter>,
}

impl Limiters {
    fn unlimited() -> RwLock<Limiters> {
        RwLock::new(Limiters {
            backoff: Duration(0),
            requests: Arc::new(Unlimited),
            tokens: Arc::new(Unlimited),
        })
    }
}

static CHAT_COMPLETION: LazyLock<RwLock<Limiters>> = LazyLock::new(Limiters::unlimited);
static EMBEDDING: LazyLock<RwLock<Limiters>> = LazyLock::new(Limiters::unlimited);

/// Go's `ChatCompletionBackoff`.
pub fn chat_completion_backoff() -> Duration {
    read(&CHAT_COMPLETION, |l| l.backoff)
}

/// Go's `ChatCompletionRequestsLimiter`.
pub fn chat_completion_requests_limiter() -> Arc<dyn RateLimiter> {
    read(&CHAT_COMPLETION, |l| Arc::clone(&l.requests))
}

/// Go's `ChatCompletionTokensLimiter`.
pub fn chat_completion_tokens_limiter() -> Arc<dyn RateLimiter> {
    read(&CHAT_COMPLETION, |l| Arc::clone(&l.tokens))
}

/// Go's `EmbeddingBackoff`.
pub fn embedding_backoff() -> Duration {
    read(&EMBEDDING, |l| l.backoff)
}

/// Go's `EmbeddingRequestsLimiter`.
pub fn embedding_requests_limiter() -> Arc<dyn RateLimiter> {
    read(&EMBEDDING, |l| Arc::clone(&l.requests))
}

/// Go's `EmbeddingTokensLimiter`.
pub fn embedding_tokens_limiter() -> Arc<dyn RateLimiter> {
    read(&EMBEDDING, |l| Arc::clone(&l.tokens))
}

/// `InitChatCompletionLimiters(rpm, tpm)`: see `init_limiters`.
///
/// # Panics
///
/// When `rpm` or `tpm` is from 1 to 59, as Go.
pub fn init_chat_completion_limiters(rpm: i64, tpm: i64) {
    init_limiters(&CHAT_COMPLETION, rpm, tpm);
}

/// `InitEmbeddingLimiters(rpm, tpm)`: see `init_limiters`.
///
/// # Panics
///
/// When `rpm` or `tpm` is from 1 to 59, as Go.
pub fn init_embedding_limiters(rpm: i64, tpm: i64) {
    init_limiters(&EMBEDDING, rpm, tpm);
}

/// The body both Go init functions share. A positive `rpm` sets the backoff
/// to a minute over `rpm` and the requests limiter to a bucket holding
/// `rpm/60` tokens, refilled with `rpm/60` every second; a positive `tpm`
/// sets the tokens limiter likewise. A limit of zero or less leaves the
/// previous value. When the bucket panics (a limit from 1 to 59), the backoff
/// has already been set, as in Go.
fn init_limiters(limiters: &RwLock<Limiters>, rpm: i64, tpm: i64) {
    if rpm > 0 {
        write(limiters, |l| l.backoff = Duration(MINUTE.0 / rpm));
        let bucket = Bucket::with_quantum(SECOND.0, rpm / 60, rpm / 60);
        write(limiters, |l| l.requests = Arc::new(bucket));
    }
    if tpm > 0 {
        let bucket = Bucket::with_quantum(SECOND.0, tpm / 60, tpm / 60);
        write(limiters, |l| l.tokens = Arc::new(bucket));
    }
}

fn read<T>(limiters: &RwLock<Limiters>, f: impl FnOnce(&Limiters) -> T) -> T {
    f(&limiters.read().unwrap_or_else(PoisonError::into_inner))
}

fn write(limiters: &RwLock<Limiters>, f: impl FnOnce(&mut Limiters)) {
    f(&mut limiters.write().unwrap_or_else(PoisonError::into_inner));
}

/// What a limiter is, for tests: Go's type switch on the interface.
#[cfg(test)]
pub(crate) fn as_bucket(limiter: Arc<dyn RateLimiter>) -> Option<Arc<Bucket>> {
    let any: Arc<dyn Any + Send + Sync> = limiter;
    any.downcast::<Bucket>().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    //! Go's `ratelimit_test.go`.

    use super::*;
    use std::sync::Mutex;

    /// The limiters are process-wide, as Go's package variables; Go's tests
    /// run one at a time, these take this lock.
    pub(crate) static GLOBALS: Mutex<()> = Mutex::new(());

    fn in_delta(expected: Duration, actual: Duration, delta: i64) {
        assert!(
            (expected.0 - actual.0).abs() <= delta,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn test_unlimited() {
        let rate_limiter = Unlimited;
        assert_eq!(rate_limiter.take(1), Duration(0));
    }

    #[test]
    fn test_init_embedding_limiters() {
        let _globals = GLOBALS.lock().unwrap_or_else(PoisonError::into_inner);
        init_embedding_limiters(120, 180);
        assert_eq!(Duration(0), embedding_requests_limiter().take(1));
        in_delta(SECOND, embedding_requests_limiter().take(2), 1_000_000);
        assert_eq!(Duration(0), embedding_tokens_limiter().take(2));
        in_delta(
            Duration(2 * SECOND.0),
            embedding_tokens_limiter().take(5),
            1_000_000,
        );
    }

    #[test]
    fn test_init_chat_completion_limiters() {
        let _globals = GLOBALS.lock().unwrap_or_else(PoisonError::into_inner);
        init_chat_completion_limiters(120, 180);
        assert_eq!(Duration(0), chat_completion_requests_limiter().take(1));
        in_delta(
            SECOND,
            chat_completion_requests_limiter().take(2),
            1_000_000,
        );
        assert_eq!(Duration(0), chat_completion_tokens_limiter().take(2));
        in_delta(
            Duration(2 * SECOND.0),
            chat_completion_tokens_limiter().take(5),
            1_000_000,
        );
    }

    #[test]
    fn unlimited_until_initialised_and_buckets_after() {
        assert!(as_bucket(Arc::new(Unlimited)).is_none());
        let bucket = as_bucket(Arc::new(Bucket::with_quantum(SECOND.0, 2, 2)));
        assert_eq!(bucket.map(|b| b.capacity()), Some(2));
    }
}
