//! juju/ratelimit v1.0.2 against Go: `tests/data/ratelimit_golden.json`,
//! written by `contracts/tools/ratelimitgolden` (its doc says what each part
//! holds).

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use gorse_ratelimit::{Bucket, Clock, internal::format_g};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Golden {
    scripts: Vec<Script>,
    constructors: Vec<Constructor>,
    rates: Vec<RateCase>,
    sweep: Sweep,
    formats: Vec<Format>,
}

#[derive(Deserialize)]
struct Script {
    fill_interval: i64,
    capacity: i64,
    quantum: i64,
    rate: String,
    ops: Vec<Op>,
}

#[derive(Deserialize)]
struct Op {
    advance: i64,
    op: String,
    count: i64,
    max_wait: i64,
    result: i64,
    ok: bool,
    clock: i64,
}

#[derive(Deserialize)]
struct Constructor {
    fill_interval: i64,
    capacity: i64,
    quantum: i64,
    panic: String,
}

#[derive(Deserialize)]
struct RateCase {
    rate: String,
    capacity: i64,
    panic: String,
    rate_out: String,
    fill_interval: i64,
    quantum: i64,
}

#[derive(Deserialize)]
struct Sweep {
    start: f64,
    step: f64,
    end: f64,
    count: usize,
    sha256: String,
}

#[derive(Deserialize)]
struct Format {
    bits: String,
    text: String,
}

fn golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ratelimit_golden.json");
    let text = std::fs::read_to_string(path).expect("read the golden file");
    serde_json::from_str(&text).expect("parse the golden file")
}

fn float(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("hex float bits"))
}

fn bits(f: f64) -> String {
    format!("{:x}", f.to_bits())
}

/// The Go tool's fakeClock: time moves only when told to, and Sleep moves
/// it.
struct FakeClock {
    start: Instant,
    now: Mutex<Instant>,
}

impl FakeClock {
    fn new() -> Arc<FakeClock> {
        let now = Instant::now();
        Arc::new(FakeClock {
            start: now,
            now: Mutex::new(now),
        })
    }

    fn advance(&self, d: i64) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        match u64::try_from(d) {
            Ok(n) => *now += Duration::from_nanos(n),
            Err(_) => *now -= Duration::from_nanos(d.unsigned_abs()),
        }
    }

    /// Go's `now.Sub(start)`, which saturates.
    fn offset(&self) -> i64 {
        let now = self.now();
        match now.checked_duration_since(self.start) {
            Some(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
            None => i64::try_from(self.start.duration_since(now).as_nanos()).map_or(i64::MIN, |n| -n),
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sleep(&self, d: i64) {
        self.advance(d);
    }
}

fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

/// Go's probe: empty a bucket of capacity 1<<62, time one more token, move
/// to the first tick and count the tokens.
fn probe(tb: &Bucket, clock: &FakeClock) -> (i64, i64) {
    tb.take_available(1 << 62);
    let fill_interval = tb.take(1);
    clock.advance(fill_interval);
    (fill_interval, tb.available() + 1)
}

fn rate_case(rate: f64, capacity: i64) -> RateCase {
    let clock = FakeClock::new();
    let dyn_clock: Arc<dyn Clock> = clock.clone();
    let mut c = RateCase {
        rate: bits(rate),
        capacity,
        panic: String::new(),
        rate_out: String::new(),
        fill_interval: 0,
        quantum: 0,
    };
    match catch(|| Bucket::with_rate_and_clock(rate, capacity, Some(dyn_clock))) {
        Err(message) => c.panic = message,
        Ok(tb) => {
            c.rate_out = bits(tb.rate());
            if capacity == 1 << 62 {
                (c.fill_interval, c.quantum) = probe(&tb, &clock);
            }
        }
    }
    c
}

#[test]
fn scripts_match_go() {
    let golden = golden();
    assert_eq!(golden.scripts.len(), 211);
    let mut ops = 0;
    for (i, s) in golden.scripts.iter().enumerate() {
        let clock = FakeClock::new();
        let tb = Bucket::with_quantum_and_clock(
            s.fill_interval,
            s.capacity,
            s.quantum,
            Some(clock.clone()),
        );
        assert_eq!(bits(tb.rate()), s.rate, "script {i}: rate");
        assert_eq!(tb.capacity(), s.capacity, "script {i}: capacity");
        for (j, op) in s.ops.iter().enumerate() {
            clock.advance(op.advance);
            let (result, ok) = match op.op.as_str() {
                "take" => (tb.take(op.count), false),
                "take_max_duration" => tb.take_max_duration(op.count, op.max_wait),
                "take_available" => (tb.take_available(op.count), false),
                "available" => (tb.available(), false),
                "wait" => {
                    tb.wait(op.count);
                    (0, false)
                }
                "wait_max_duration" => (0, tb.wait_max_duration(op.count, op.max_wait)),
                other => panic!("unknown op {other}"),
            };
            let at = format!(
                "script {i} ({} {} {}) op {j}: {} count {} max_wait {}",
                s.fill_interval, s.capacity, s.quantum, op.op, op.count, op.max_wait
            );
            assert_eq!(result, op.result, "{at}: result");
            assert_eq!(ok, op.ok, "{at}: ok");
            assert_eq!(clock.offset(), op.clock, "{at}: clock");
            ops += 1;
        }
    }
    assert_eq!(ops, 8440);
}

#[test]
fn constructors_match_go() {
    let golden = golden();
    assert_eq!(golden.constructors.len(), 125);
    for c in &golden.constructors {
        let got = catch(|| Bucket::with_quantum(c.fill_interval, c.capacity, c.quantum))
            .err()
            .unwrap_or_default();
        assert_eq!(
            got, c.panic,
            "NewBucketWithQuantum({}, {}, {})",
            c.fill_interval, c.capacity, c.quantum
        );
    }
}

#[test]
fn rates_match_go() {
    let golden = golden();
    assert_eq!(golden.rates.len(), 561);
    let got: Vec<RateCase> = golden
        .rates
        .iter()
        .map(|c| rate_case(float(&c.rate), c.capacity))
        .collect();
    let mut panics = 0;
    for (want, got) in golden.rates.iter().zip(&got) {
        let at = format!("rate {} ({:e}) capacity {}", want.rate, float(&want.rate), want.capacity);
        assert_eq!(got.panic, want.panic, "{at}: panic");
        assert_eq!(got.rate_out, want.rate_out, "{at}: Rate()");
        assert_eq!(got.fill_interval, want.fill_interval, "{at}: fill interval");
        assert_eq!(got.quantum, want.quantum, "{at}: quantum");
        panics += usize::from(!want.panic.is_empty());
    }
    assert_eq!(panics, 168);
}

#[test]
fn rate_sweep_matches_go() {
    let golden = golden();
    let sweep = golden.sweep;
    let mut h = Sha256::new();
    let mut count = 0;
    let mut rate = sweep.start;
    while rate < sweep.end {
        let c = rate_case(rate, 1 << 62);
        h.update(
            format!(
                "{} {} {} {} {}\n",
                c.rate, c.panic, c.rate_out, c.fill_interval, c.quantum
            )
            .as_bytes(),
        );
        count += 1;
        rate += sweep.step;
    }
    assert_eq!(count, sweep.count);
    assert_eq!(hex::encode(h.finalize()), sweep.sha256);
}

#[test]
fn formats_match_go() {
    let golden = golden();
    assert_eq!(golden.formats.len(), 5054);
    for f in &golden.formats {
        assert_eq!(format_g(float(&f.bits)), f.text, "bits {}", f.bits);
    }
}
