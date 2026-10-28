// Ports: github.com/juju/ratelimit (ratelimit.go, reader.go) @ v1.0.2
//
// Copyright 2014 Canonical Ltd.
// Licensed under the LGPLv3 with static-linking exception.
// See LICENSE file for details.
//
// Modified 2026-10-19 for gorse-rust: translated from Go to Rust.

//! A token bucket: a port of [juju/ratelimit](https://github.com/juju/ratelimit)
//! v1.0.2, which Gorse's OpenAI rate limiters
//! (`common/parallel/ratelimit.go`) are built on.
//!
//! Licensed, like juju/ratelimit, under the LGPLv3 with a static-linking
//! exception (`LICENSE`, `COPYING`); see `README.md`.
//!
//! The shape follows Go: Go's `time.Duration` is `i64` nanoseconds here
//! (fill intervals, waits), and Go's `time.Time` is [`Instant`] (Go's
//! monotonic reading). Integer arithmetic that wraps in Go wraps here. The
//! one difference: when the wait for tokens is so long that the end time
//! overflows Go's monotonic clock reading (a wait near 292 years), Go falls
//! back to the wall clock, which can differ from the monotonic clock by the
//! clock drift since the bucket was made; here the wait is always measured
//! on [`Instant`].
//!
//! ```
//! use gorse_ratelimit::Bucket;
//!
//! // Two tokens a second, at most two in the bucket, which starts full.
//! let bucket = Bucket::with_quantum(1_000_000_000, 2, 2);
//! assert_eq!(bucket.take(1), 0);
//! // One token left: the second must wait for the next tick, a second away.
//! let wait = bucket.take(2);
//! assert!(wait > 990_000_000 && wait <= 1_000_000_000);
//! ```

mod format;
mod reader;

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub use reader::{Reader, Writer};

#[doc(hidden)]
pub mod internal {
    //! Helpers the golden tests check against Go. Not part of the API.

    pub use crate::format::format_g;
}

// The algorithm that this implementation uses does computational work
// only when tokens are removed from the bucket, and that work completes
// in short, bounded-constant time (Bucket.Wait benchmarks at 175ns on
// my laptop).
//
// Time is measured in equal measured ticks, a given interval
// (fillInterval) apart. On each tick a number of tokens (quantum) are
// added to the bucket.
//
// When any of the methods are called the bucket updates the number of
// tokens that are in the bucket, and it records the current tick
// number too. Note that it doesn't record the current time - by
// keeping things in units of whole ticks, it's easy to dish out tokens
// at exactly the right intervals as measured from the start time.
//
// This allows us to calculate the number of tokens that will be
// available at some time in the future with a few simple arithmetic
// operations.
//
// The main reason for being able to transfer multiple tokens on each tick
// is so that we can represent rates greater than 1e9 (the resolution of the Go
// time package) tokens per second, but it's also useful because
// it means we can easily represent situations like "a person gets
// five tokens an hour, replenished on the hour".

/// Bucket represents a token bucket that fills at a predetermined rate.
/// Methods on Bucket may be called concurrently.
pub struct Bucket {
    clock: Arc<dyn Clock>,

    /// startTime holds the moment when the bucket was
    /// first created and ticks began.
    start_time: Instant,

    /// capacity holds the overall capacity of the bucket.
    capacity: i64,

    /// quantum holds how many tokens are added on
    /// each tick.
    quantum: i64,

    /// fillInterval holds the interval between each tick, in nanoseconds.
    fill_interval: i64,

    /// mu guards the fields below it.
    mu: Mutex<Tokens>,
}

struct Tokens {
    /// availableTokens holds the number of available
    /// tokens as of the associated latestTick.
    /// It will be negative when there are consumers
    /// waiting for tokens.
    available_tokens: i64,

    /// latestTick holds the latest tick for which
    /// we know the number of tokens in the bucket.
    latest_tick: i64,
}

/// rateMargin specifes the allowed variance of actual
/// rate from specified rate. 1% seems reasonable.
const RATE_MARGIN: f64 = 0.01;

/// Go's `infinityDuration`: the longest `time.Duration`.
const INFINITY_DURATION: i64 = i64::MAX;

impl Bucket {
    /// NewBucket returns a new token bucket that fills at the
    /// rate of one token every fillInterval (nanoseconds), up to the given
    /// maximum capacity. Both arguments must be
    /// positive. The bucket is initially full.
    ///
    /// # Panics
    ///
    /// As [`Bucket::with_quantum_and_clock`].
    pub fn new(fill_interval: i64, capacity: i64) -> Bucket {
        Bucket::with_clock(fill_interval, capacity, None)
    }

    /// NewBucketWithClock is identical to NewBucket but injects a testable clock
    /// interface.
    ///
    /// # Panics
    ///
    /// As [`Bucket::with_quantum_and_clock`].
    pub fn with_clock(fill_interval: i64, capacity: i64, clock: Option<Arc<dyn Clock>>) -> Bucket {
        Bucket::with_quantum_and_clock(fill_interval, capacity, 1, clock)
    }

    /// NewBucketWithRate returns a token bucket that fills the bucket
    /// at the rate of rate tokens per second up to the given
    /// maximum capacity. Because of limited clock resolution,
    /// at high rates, the actual rate may be up to 1% different from the
    /// specified rate.
    ///
    /// # Panics
    ///
    /// As [`Bucket::with_rate_and_clock`].
    pub fn with_rate(rate: f64, capacity: i64) -> Bucket {
        Bucket::with_rate_and_clock(rate, capacity, None)
    }

    /// NewBucketWithRateAndClock is identical to NewBucketWithRate but injects a
    /// testable clock interface.
    ///
    /// # Panics
    ///
    /// With `token bucket capacity is not > 0` when `capacity <= 0`, and with
    /// `cannot find suitable quantum for <rate>` (`rate` as Go's
    /// `strconv.FormatFloat(rate, 'g', -1, 64)`) when no quantum below 2^50
    /// gives the rate within 1%: a rate that is not positive, is NaN or
    /// infinite, or is too far from what a nanosecond interval can give.
    pub fn with_rate_and_clock(rate: f64, capacity: i64, clock: Option<Arc<dyn Clock>>) -> Bucket {
        // Use the same bucket each time through the loop
        // to save allocations.
        let mut tb = Bucket::with_quantum_and_clock(1, capacity, 1, clock);
        let mut quantum: i64 = 1;
        while quantum < 1 << 50 {
            let fill_interval = go_f64_to_i64(1e9 * quantum as f64 / rate);
            if fill_interval > 0 {
                tb.fill_interval = fill_interval;
                tb.quantum = quantum;
                let diff = (tb.rate() - rate).abs();
                if diff / rate <= RATE_MARGIN {
                    return tb;
                }
            }
            quantum = next_quantum(quantum);
        }
        panic!(
            "cannot find suitable quantum for {}",
            format::format_g(rate)
        );
    }

    /// NewBucketWithQuantum is similar to NewBucket, but allows
    /// the specification of the quantum size - quantum tokens
    /// are added every fillInterval.
    ///
    /// # Panics
    ///
    /// As [`Bucket::with_quantum_and_clock`].
    pub fn with_quantum(fill_interval: i64, capacity: i64, quantum: i64) -> Bucket {
        Bucket::with_quantum_and_clock(fill_interval, capacity, quantum, None)
    }

    /// NewBucketWithQuantumAndClock is like NewBucketWithQuantum, but
    /// also has a clock argument that allows clients to fake the passing
    /// of time. If clock is `None`, the system clock will be used.
    ///
    /// # Panics
    ///
    /// With Go's messages: `token bucket fill interval is not > 0`,
    /// `token bucket capacity is not > 0` or
    /// `token bucket quantum is not > 0`, checked in that order.
    pub fn with_quantum_and_clock(
        fill_interval: i64,
        capacity: i64,
        quantum: i64,
        clock: Option<Arc<dyn Clock>>,
    ) -> Bucket {
        let clock = clock.unwrap_or_else(|| Arc::new(RealClock));
        if fill_interval <= 0 {
            panic!("token bucket fill interval is not > 0");
        }
        if capacity <= 0 {
            panic!("token bucket capacity is not > 0");
        }
        if quantum <= 0 {
            panic!("token bucket quantum is not > 0");
        }
        Bucket {
            start_time: clock.now(),
            clock,
            fill_interval,
            capacity,
            quantum,
            mu: Mutex::new(Tokens {
                latest_tick: 0,
                available_tokens: capacity,
            }),
        }
    }

    /// Wait takes count tokens from the bucket, waiting until they are
    /// available.
    pub fn wait(&self, count: i64) {
        let d = self.take(count);
        if d > 0 {
            self.clock.sleep(d);
        }
    }

    /// WaitMaxDuration is like Wait except that it will
    /// only take tokens from the bucket if it needs to wait
    /// for no greater than maxWait. It reports whether
    /// any tokens have been removed from the bucket
    /// If no tokens have been removed, it returns immediately.
    pub fn wait_max_duration(&self, count: i64, max_wait: i64) -> bool {
        let (d, ok) = self.take_max_duration(count, max_wait);
        if d > 0 {
            self.clock.sleep(d);
        }
        ok
    }

    /// Take takes count tokens from the bucket without blocking. It returns
    /// the time (nanoseconds) that the caller should wait until the tokens
    /// are actually available.
    ///
    /// Note that if the request is irrevocable - there is no way to return
    /// tokens to the bucket once this method commits us to taking them.
    pub fn take(&self, count: i64) -> i64 {
        let mut tokens = self.lock();
        self.take_at(&mut tokens, self.clock.now(), count, INFINITY_DURATION)
            .0
    }

    /// TakeMaxDuration is like Take, except that
    /// it will only take tokens from the bucket if the wait
    /// time for the tokens is no greater than maxWait.
    ///
    /// If it would take longer than maxWait for the tokens
    /// to become available, it does nothing and reports false,
    /// otherwise it returns the time that the caller should
    /// wait until the tokens are actually available, and reports
    /// true.
    pub fn take_max_duration(&self, count: i64, max_wait: i64) -> (i64, bool) {
        let mut tokens = self.lock();
        self.take_at(&mut tokens, self.clock.now(), count, max_wait)
    }

    /// TakeAvailable takes up to count immediately available tokens from the
    /// bucket. It returns the number of tokens removed, or zero if there are
    /// no available tokens. It does not block.
    pub fn take_available(&self, count: i64) -> i64 {
        let mut tokens = self.lock();
        self.take_available_at(&mut tokens, self.clock.now(), count)
    }

    /// takeAvailable is the internal version of TakeAvailable - it takes the
    /// current time as an argument to enable easy testing.
    fn take_available_at(&self, tokens: &mut Tokens, now: Instant, mut count: i64) -> i64 {
        if count <= 0 {
            return 0;
        }
        self.adjust_available_tokens(tokens, self.current_tick(now));
        if tokens.available_tokens <= 0 {
            return 0;
        }
        if count > tokens.available_tokens {
            count = tokens.available_tokens;
        }
        tokens.available_tokens -= count;
        count
    }

    /// Available returns the number of available tokens. It will be negative
    /// when there are consumers waiting for tokens. Note that if this
    /// returns greater than zero, it does not guarantee that calls that take
    /// tokens from the buffer will succeed, as the number of available
    /// tokens could have changed in the meantime. This method is intended
    /// primarily for metrics reporting and debugging.
    pub fn available(&self) -> i64 {
        self.available_at(self.clock.now())
    }

    /// available is the internal version of available - it takes the current time as
    /// an argument to enable easy testing.
    fn available_at(&self, now: Instant) -> i64 {
        let mut tokens = self.lock();
        self.adjust_available_tokens(&mut tokens, self.current_tick(now));
        tokens.available_tokens
    }

    /// Capacity returns the capacity that the bucket was created with.
    pub fn capacity(&self) -> i64 {
        self.capacity
    }

    /// Rate returns the fill rate of the bucket, in tokens per second.
    pub fn rate(&self) -> f64 {
        1e9 * self.quantum as f64 / self.fill_interval as f64
    }

    /// take is the internal version of Take - it takes the current time as
    /// an argument to enable easy testing.
    fn take_at(&self, tokens: &mut Tokens, now: Instant, count: i64, max_wait: i64) -> (i64, bool) {
        if count <= 0 {
            return (0, true);
        }

        let tick = self.current_tick(now);
        self.adjust_available_tokens(tokens, tick);
        let avail = tokens.available_tokens.wrapping_sub(count);
        if avail >= 0 {
            tokens.available_tokens = avail;
            return (0, true);
        }
        // Round up the missing tokens to the nearest multiple
        // of quantum - the tokens won't be available until
        // that tick.

        // endTick holds the tick when all the requested tokens will
        // become available.
        let end_tick = tick.wrapping_add(
            avail
                .wrapping_neg()
                .wrapping_add(self.quantum)
                .wrapping_sub(1)
                / self.quantum,
        );
        // endTime := tb.startTime.Add(time.Duration(endTick) * tb.fillInterval)
        // waitTime := endTime.Sub(now)
        let wait_time = clamp_i64(
            i128::from(end_tick.wrapping_mul(self.fill_interval))
                - nanos_since(now, self.start_time),
        );
        if wait_time > max_wait {
            return (0, false);
        }
        tokens.available_tokens = avail;
        (wait_time, true)
    }

    /// currentTick returns the current time tick, measured
    /// from tb.startTime.
    fn current_tick(&self, now: Instant) -> i64 {
        // Go's now.Sub(startTime) saturates.
        clamp_i64(nanos_since(now, self.start_time)) / self.fill_interval
    }

    /// adjustavailableTokens adjusts the current number of tokens
    /// available in the bucket at the given time, which must
    /// be in the future (positive) with respect to tb.latestTick.
    fn adjust_available_tokens(&self, tokens: &mut Tokens, tick: i64) {
        let last_tick = tokens.latest_tick;
        tokens.latest_tick = tick;
        if tokens.available_tokens >= self.capacity {
            return;
        }
        tokens.available_tokens = tokens
            .available_tokens
            .wrapping_add(tick.wrapping_sub(last_tick).wrapping_mul(self.quantum));
        if tokens.available_tokens > self.capacity {
            tokens.available_tokens = self.capacity;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Tokens> {
        // No code that can panic runs under the lock; recover anyway.
        self.mu.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for Bucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bucket")
            .field("capacity", &self.capacity)
            .field("quantum", &self.quantum)
            .field("fill_interval", &self.fill_interval)
            .finish_non_exhaustive()
    }
}

/// nextQuantum returns the next quantum to try after q.
/// We grow the quantum exponentially, but slowly, so we
/// get a good fit in the lower numbers.
fn next_quantum(q: i64) -> i64 {
    let mut q1 = q * 11 / 10;
    if q1 == q {
        q1 += 1;
    }
    q1
}

/// Go's `int64(f)` on amd64 (`CVTTSD2SQ`): truncates toward zero, and gives
/// the minimum `int64` for NaN and values out of range. (Rust's `as`
/// saturates instead, which would accept fill intervals Go rejects.)
fn go_f64_to_i64(f: f64) -> i64 {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    if f.is_nan() || !(-TWO_63..TWO_63).contains(&f) {
        return i64::MIN;
    }
    f as i64
}

/// `now - start` in nanoseconds, exactly.
fn nanos_since(now: Instant, start: Instant) -> i128 {
    let nanos = |d: Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
    match now.checked_duration_since(start) {
        Some(d) => nanos(d),
        None => -nanos(start.duration_since(now)),
    }
}

/// Go's saturation of a `time.Duration` result.
fn clamp_i64(n: i128) -> i64 {
    i64::try_from(n).unwrap_or(if n < 0 { i64::MIN } else { i64::MAX })
}

/// Clock represents the passage of time in a way that
/// can be faked out for tests.
pub trait Clock: Send + Sync {
    /// Now returns the current time.
    fn now(&self) -> Instant;
    /// Sleep sleeps for at least the given duration (nanoseconds).
    fn sleep(&self, d: i64);
}

/// realClock implements Clock in terms of standard time functions.
struct RealClock;

impl Clock for RealClock {
    /// Now implements Clock.Now by calling time.Now.
    fn now(&self) -> Instant {
        Instant::now()
    }

    /// Now implements Clock.Sleep by calling time.Sleep.
    fn sleep(&self, d: i64) {
        // Go's time.Sleep returns at once for d <= 0.
        if let Ok(d) = u64::try_from(d) {
            std::thread::sleep(Duration::from_nanos(d));
        }
    }
}

#[cfg(test)]
mod tests {
    //! juju/ratelimit's `ratelimit_test.go` @ v1.0.2.

    use super::*;
    use std::panic;

    const MILLISECOND: i64 = 1_000_000;
    const SECOND: i64 = 1_000_000_000;

    fn at(tb: &Bucket, offset: i64) -> Instant {
        match u64::try_from(offset) {
            Ok(n) => tb.start_time + Duration::from_nanos(n),
            Err(_) => tb.start_time - Duration::from_nanos(offset.unsigned_abs()),
        }
    }

    fn take(tb: &Bucket, offset: i64, count: i64, max_wait: i64) -> (i64, bool) {
        tb.take_at(&mut tb.lock(), at(tb, offset), count, max_wait)
    }

    fn take_available(tb: &Bucket, offset: i64, count: i64) -> i64 {
        tb.take_available_at(&mut tb.lock(), at(tb, offset), count)
    }

    struct TakeReq {
        time: i64,
        count: i64,
        expect_wait: i64,
    }

    struct TakeTest {
        about: &'static str,
        fill_interval: i64,
        capacity: i64,
        reqs: Vec<TakeReq>,
    }

    fn req(time: i64, count: i64, expect_wait: i64) -> TakeReq {
        TakeReq {
            time,
            count,
            expect_wait,
        }
    }

    fn take_tests() -> Vec<TakeTest> {
        let ms = MILLISECOND;
        vec![
            TakeTest {
                about: "serial requests",
                fill_interval: 250 * ms,
                capacity: 10,
                reqs: vec![
                    req(0, 0, 0),
                    req(0, 10, 0),
                    req(0, 1, 250 * ms),
                    req(250 * ms, 1, 250 * ms),
                ],
            },
            TakeTest {
                about: "concurrent requests",
                fill_interval: 250 * ms,
                capacity: 10,
                reqs: vec![
                    req(0, 10, 0),
                    req(0, 2, 500 * ms),
                    req(0, 2, 1000 * ms),
                    req(0, 1, 1250 * ms),
                ],
            },
            TakeTest {
                about: "more than capacity",
                fill_interval: ms,
                capacity: 10,
                reqs: vec![req(0, 10, 0), req(20 * ms, 15, 5 * ms)],
            },
            TakeTest {
                about: "sub-quantum time",
                fill_interval: 10 * ms,
                capacity: 10,
                reqs: vec![
                    req(0, 10, 0),
                    req(7 * ms, 1, 3 * ms),
                    req(8 * ms, 1, 12 * ms),
                ],
            },
            TakeTest {
                about: "within capacity",
                fill_interval: 10 * ms,
                capacity: 5,
                reqs: vec![
                    req(0, 5, 0),
                    req(60 * ms, 5, 0),
                    req(60 * ms, 1, 10 * ms),
                    req(80 * ms, 2, 10 * ms),
                ],
            },
        ]
    }

    #[test]
    fn test_take() {
        for (i, test) in take_tests().iter().enumerate() {
            let tb = Bucket::new(test.fill_interval, test.capacity);
            for (j, req) in test.reqs.iter().enumerate() {
                let (d, ok) = take(&tb, req.time, req.count, INFINITY_DURATION);
                assert!(ok);
                assert_eq!(d, req.expect_wait, "test {i}.{j}, {}", test.about);
            }
        }
    }

    #[test]
    fn test_take_max_duration() {
        for (i, test) in take_tests().iter().enumerate() {
            let tb = Bucket::new(test.fill_interval, test.capacity);
            for (j, req) in test.reqs.iter().enumerate() {
                if req.expect_wait > 0 {
                    let (d, ok) = take(&tb, req.time, req.count, req.expect_wait - 1);
                    assert!(!ok);
                    assert_eq!(d, 0);
                }
                let (d, ok) = take(&tb, req.time, req.count, req.expect_wait);
                assert!(ok);
                assert_eq!(d, req.expect_wait, "test {i}.{j}, {}", test.about);
            }
        }
    }

    #[test]
    fn test_take_available() {
        let ms = MILLISECOND;
        // (about, fillInterval, capacity, [(time, count, expect)])
        type Case<'a> = (&'a str, i64, i64, &'a [(i64, i64, i64)]);
        let tests: [Case<'_>; 4] = [
            (
                "serial requests",
                250 * ms,
                10,
                &[(0, 0, 0), (0, 10, 10), (0, 1, 0), (250 * ms, 1, 1)],
            ),
            (
                "concurrent requests",
                250 * ms,
                10,
                &[(0, 5, 5), (0, 2, 2), (0, 5, 3), (0, 1, 0)],
            ),
            (
                "more than capacity",
                ms,
                10,
                &[(0, 10, 10), (20 * ms, 15, 10)],
            ),
            (
                "within capacity",
                10 * ms,
                5,
                &[(0, 5, 5), (60 * ms, 5, 5), (70 * ms, 1, 1)],
            ),
        ];
        for (i, (about, fill_interval, capacity, reqs)) in tests.iter().enumerate() {
            let tb = Bucket::new(*fill_interval, *capacity);
            for (j, (time, count, expect)) in reqs.iter().enumerate() {
                let d = take_available(&tb, *time, *count);
                assert_eq!(d, *expect, "test {i}.{j}, {about}");
            }
        }
    }

    fn panic_message(f: impl FnOnce() + panic::UnwindSafe) -> String {
        let payload = panic::catch_unwind(f).expect_err("should panic");
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    }

    #[test]
    fn test_panics() {
        assert_eq!(
            panic_message(|| drop(Bucket::new(0, 1))),
            "token bucket fill interval is not > 0"
        );
        assert_eq!(
            panic_message(|| drop(Bucket::new(-2, 1))),
            "token bucket fill interval is not > 0"
        );
        assert_eq!(
            panic_message(|| drop(Bucket::new(1, 0))),
            "token bucket capacity is not > 0"
        );
        assert_eq!(
            panic_message(|| drop(Bucket::new(1, -2))),
            "token bucket capacity is not > 0"
        );
        // Beyond juju's test: the quantum check.
        assert_eq!(
            panic_message(|| drop(Bucket::with_quantum(1, 1, 0))),
            "token bucket quantum is not > 0"
        );
    }

    fn is_close_to(x: f64, y: f64, tolerance: f64) -> bool {
        (x - y).abs() / y < tolerance
    }

    #[test]
    fn test_rate() {
        let tb = Bucket::new(1, 1);
        assert!(
            is_close_to(tb.rate(), 1e9, 0.00001),
            "got {} want 1e9",
            tb.rate()
        );
        let tb = Bucket::new(2 * SECOND, 1);
        assert!(
            is_close_to(tb.rate(), 0.5, 0.00001),
            "got {} want 0.5",
            tb.rate()
        );
        let tb = Bucket::with_quantum(100 * MILLISECOND, 1, 5);
        assert!(
            is_close_to(tb.rate(), 50.0, 0.00001),
            "got {} want 50",
            tb.rate()
        );
    }

    fn check_rate(rate: f64) {
        let tb = Bucket::with_rate(rate, 1 << 62);
        assert!(
            is_close_to(tb.rate(), rate, RATE_MARGIN),
            "got {} want {rate}",
            tb.rate()
        );
        let (d, ok) = take(&tb, 0, 1 << 62, INFINITY_DURATION);
        assert!(ok);
        assert_eq!(d, 0);

        // Check that the actual rate is as expected by
        // asking for a not-quite multiple of the bucket's
        // quantum and checking that the wait time
        // correct.
        let (d, ok) = take(&tb, 0, tb.quantum * 2 - tb.quantum / 2, INFINITY_DURATION);
        assert!(ok);
        let expect_time = 1e9 * tb.quantum as f64 * 2.0 / rate;
        assert!(
            is_close_to(d as f64, expect_time, RATE_MARGIN),
            "rate {rate}: got {} want {expect_time}",
            d as f64
        );
    }

    /// juju's `NewBucketWithRate` suite method, which gocheck never runs (its
    /// name lacks the `Test` prefix); it passes here.
    #[test]
    fn new_bucket_with_rate() {
        let mut rate = 1.0;
        while rate < 1e6 {
            check_rate(rate);
            rate += 7.0;
        }
        for rate in [
            1024.0 * 1024.0 * 1024.0,
            1e-5,
            0.9e-5,
            0.5,
            0.9,
            0.9e8,
            3e12,
            4e18,
            i64::MAX as f64,
        ] {
            check_rate(rate);
            check_rate(rate / 3.0);
            check_rate(rate * 1.3);
        }
    }

    #[test]
    fn test_available() {
        // (about, capacity, fillInterval, take, sleep, afterTake, afterSleep)
        let tests = [
            (
                "should fill tokens after interval",
                5,
                SECOND,
                5,
                SECOND,
                0,
                1,
            ),
            (
                "should fill tokens plus existing count",
                2,
                SECOND,
                1,
                SECOND,
                1,
                2,
            ),
            (
                "shouldn't fill before interval",
                2,
                2 * SECOND,
                1,
                SECOND,
                1,
                1,
            ),
            (
                "should fill only once after 1*interval before 2*interval",
                2,
                2 * SECOND,
                1,
                3 * SECOND,
                1,
                2,
            ),
        ];
        for (i, (about, capacity, fill_interval, take, sleep, after_take, after_sleep)) in
            tests.into_iter().enumerate()
        {
            let tb = Bucket::new(fill_interval, capacity);
            let c = take_available(&tb, 0, take);
            assert_eq!(c, take, "#{i}: {about}, take = {c}, want = {take}");
            let c = tb.available_at(tb.start_time);
            assert_eq!(c, after_take, "#{i}: {about}, after take");
            let c = tb.available_at(at(&tb, sleep));
            assert_eq!(c, after_sleep, "#{i}: {about}, after some time");
        }
    }

    /// juju sleeps five real seconds; the clock is faked here.
    #[test]
    fn test_no_bonus_token_after_bucket_is_full() {
        struct Fake(Mutex<Instant>);
        impl Clock for Fake {
            fn now(&self) -> Instant {
                *self.0.lock().unwrap_or_else(PoisonError::into_inner)
            }
            fn sleep(&self, d: i64) {
                let mut now = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                *now += Duration::from_nanos(u64::try_from(d).unwrap_or(0));
            }
        }
        let clock = Arc::new(Fake(Mutex::new(Instant::now())));
        let tb = Bucket::with_quantum_and_clock(SECOND, 100, 20, Some(clock.clone()));
        assert_eq!(tb.available(), 100, "initially");

        clock.sleep(5 * SECOND);

        assert_eq!(tb.available(), 100, "after pause");
        assert_eq!(tb.take_available(100), 100, "taking");
        assert_eq!(tb.available(), 0, "after taken");
    }

    #[test]
    fn go_float_conversion() {
        assert_eq!(go_f64_to_i64(1.9), 1);
        assert_eq!(go_f64_to_i64(-1.9), -1);
        assert_eq!(go_f64_to_i64(f64::NAN), i64::MIN);
        assert_eq!(go_f64_to_i64(f64::INFINITY), i64::MIN);
        assert_eq!(go_f64_to_i64(9.223_372_036_854_776e18), i64::MIN);
        assert_eq!(
            go_f64_to_i64(9.223_372_036_854_775e18),
            9_223_372_036_854_774_784
        );
        assert_eq!(go_f64_to_i64(-9.223_372_036_854_776e18), i64::MIN);
    }
}
