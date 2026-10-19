// Ports: math/exp_amd64.s, math/exp_amd64.go @ go1.27.0 (copied from WP-1-4's util/rand/math.rs, see below)
// Ports: math/rand rand.go (Float64, Float32) @ go1.27.0
// Ports: common/parallel/parallel.go (For) @ add21b5

//! Stand-ins for three dependencies of `common/nn` whose ports are not on the
//! integration branch yet. Each matches what Go computes; they are swapped for
//! the real modules when those land (`porting/notes/WP-1-11.md`, Open):
//!
//! - [`exp`]: Go's `math.Exp` on amd64. The code is WP-1-4's
//!   `util::rand::math::exp_with` (branch `claude/wp-1-4-util`, not yet
//!   integrated), copied unchanged except that the FMA path follows
//!   [`Feature::current`], which honours `GODEBUG` as Go's `internal/cpu`
//!   does.
//! - [`global`]: the source of Go's top-level `rand.Float32` and
//!   `rand.NormFloat64`. Go seeds it randomly at start-up, so no program can
//!   depend on its values; this one is splitmix64 with a random seed, with
//!   Go's `Float32`, and a Box-Muller `NormFloat64` (the same standard normal
//!   distribution, not Go's ziggurat).
//! - [`parallel_for`]: `parallel.For` with a background context.

#![allow(
    clippy::approx_constant,
    clippy::excessive_precision,
    reason = "constants are copied verbatim from Go's assembly"
)]

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::floats::Feature;

use super::Rng;

/// `log_e(2)` split into an upper part with trailing zero bits and the rest.
const LN2U: f64 = 0.693_147_180_559_662_956_511_601_805_686_950_683_593_75;
const LN2L: f64 = 0.282_352_905_630_315_771_225_884_481_750_134_360_255_254_120_68e-12;
const LOG2E: f64 = 1.442_695_040_888_963_407_359_924_681_001_892_0;
/// Above this `Exp` overflows.
const OVERFLOW: f64 = 7.097_827_128_933_84e+02;
/// Taylor coefficients, in the order of `exprodata` in `exp_amd64.s`.
const E: [f64; 9] = [
    0.5,
    1.0,
    2.0,
    1.666_666_666_666_666_666_7e-1,
    4.166_666_666_666_666_666_7e-2,
    8.333_333_333_333_333_333_3e-3,
    1.388_888_888_888_888_888_9e-3,
    1.984_126_984_126_984_127_0e-4,
    2.480_158_730_158_730_158_7e-5,
];

const POS_INF: u64 = 0x7FF0_0000_0000_0000;
const NEG_INF: u64 = 0xFFF0_0000_0000_0000;

/// Go's `math.Exp(x)` on amd64. Go takes the FMA path when
/// `cpu.X86.HasAVX && cpu.X86.HasFMA`, which `GODEBUG=cpu.avx=off` or
/// `cpu.fma=off` turns off.
pub(crate) fn exp(x: f64) -> f64 {
    exp_with(x, Feature::current().contains(Feature::AVX | Feature::FMA))
}

/// `CVTSD2SL`: round to nearest even, and the "integer indefinite" value
/// `i32::MIN` when the result does not fit.
fn cvtsd2sl(x: f64) -> i32 {
    let r = x.round_ties_even();
    if (-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
        r as i32
    } else {
        i32::MIN
    }
}

/// `archExp` with the FMA path chosen by `fma`.
fn exp_with(x: f64, fma: bool) -> f64 {
    let bits = x.to_bits();
    if bits & !(1 << 63) >= POS_INF {
        // NaN or ±Inf: -Inf gives 0, the others themselves.
        return if bits == NEG_INF { 0.0 } else { x };
    }
    if x > OVERFLOW {
        return f64::INFINITY;
    }
    let k = cvtsd2sl(LOG2E * x);
    let kf = f64::from(k);
    let mut x0 = x;
    if fma {
        x0 = (-kf).mul_add(LN2U, x0);
        x0 = (-kf).mul_add(LN2L, x0);
        x0 *= 0.0625;
        let mut x1 = E[8];
        for c in [E[7], E[6], E[5], E[4], E[3], E[0], E[1]] {
            x1 = x1.mul_add(x0, c);
        }
        x0 *= x1;
        for _ in 0..3 {
            x1 = x0 + E[2];
            x0 *= x1;
        }
        x1 = x0 + E[2];
        x0 = x0.mul_add(x1, E[1]);
    } else {
        x0 -= LN2U * kf;
        x0 -= LN2L * kf;
        x0 *= 0.0625;
        let mut x1 = E[8] * x0;
        for c in [E[7], E[6], E[5], E[4], E[3], E[0]] {
            x1 += c;
            x1 *= x0;
        }
        x1 += E[1];
        x0 *= x1;
        for _ in 0..4 {
            x1 = E[2] + x0;
            x0 *= x1;
        }
        x0 += E[1];
    }
    ldexp(x0, k)
}

/// The `ldexp` tail of `archExp`: `x0 * 2**k`, through a denormal step when
/// the biased exponent is not positive.
fn ldexp(mut x0: f64, k: i32) -> f64 {
    // ADDL $0x3FF, BX: k is above i32::MIN + 1023 or exactly i32::MIN.
    let mut biased = k.wrapping_add(0x3FF);
    if biased <= 0 {
        if biased < -52 {
            return 0.0;
        }
        biased += 0x3FE;
        x0 *= f64::from_bits(u64::from(biased.cast_unsigned()) << 52);
        biased = 1;
    } else if biased >= 0x7FF {
        return f64::INFINITY;
    }
    x0 * f64::from_bits(u64::from(biased.cast_unsigned()) << 52)
}

/// The global source of `math/rand`, which Go seeds randomly.
struct Global(u64);

impl Global {
    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `Int63`.
    fn int63(&mut self) -> i64 {
        (self.next() >> 1).cast_signed()
    }

    /// `Float64`: `Int63` scaled to [0, 1), drawn again in the rare case it
    /// rounds to 1.
    fn float64(&mut self) -> f64 {
        loop {
            let f = self.int63() as f64 / (1u64 << 63) as f64;
            if f != 1.0 {
                return f;
            }
        }
    }
}

impl Rng for Global {
    /// `Float32`: `Float64` rounded to `float32`, drawn again if that is 1.
    fn float32(&mut self) -> f32 {
        loop {
            let f = self.float64() as f32;
            if f != 1.0 {
                return f;
            }
        }
    }

    /// A standard normal value by the Box-Muller transform.
    fn norm_float64(&mut self) -> f64 {
        let u1 = 1.0 - self.float64();
        let u2 = self.float64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

static GLOBAL: LazyLock<Mutex<Global>> =
    LazyLock::new(|| Mutex::new(Global(RandomState::new().hash_one(0x6e6e_u64))));

/// Runs `f` with the global source, as Go's top-level functions use theirs.
pub(crate) fn global<R>(f: impl FnOnce(&mut dyn Rng) -> R) -> R {
    f(&mut *GLOBAL.lock().unwrap_or_else(PoisonError::into_inner))
}

/// `parallel.For(context.Background(), n_jobs, n_workers, worker)`: every
/// job runs once, on the calling thread when `n_workers <= 1`, otherwise on
/// `n_workers` threads that take the next job as they finish one (Go hands
/// them out through a channel).
pub(crate) fn parallel_for(n_jobs: usize, n_workers: usize, worker: impl Fn(usize) + Sync) {
    if n_workers <= 1 {
        (0..n_jobs).for_each(worker);
        return;
    }
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        // Workers beyond the number of jobs would find the channel closed.
        for _ in 0..n_workers.min(n_jobs) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n_jobs {
                        break;
                    }
                    worker(i);
                }
            });
        }
    });
}
