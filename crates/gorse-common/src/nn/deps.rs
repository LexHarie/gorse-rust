// Ports: math/rand rand.go (Float64, Float32) @ go1.27.0
// Ports: common/parallel/parallel.go (For) @ add21b5
// Ports: math/exp_amd64.s, math/exp_amd64.go @ go1.27.0 (through util::rand::math)

//! Stand-ins for three dependencies of `common/nn` whose ports are not on the
//! integration branch yet (`PORTING_NOTES.md`, Open):
//!
//! - [`exp`]: Go's `math.Exp` on amd64, which is the `exp_with` of WP-1-4's
//!   `util::rand::math`. This wrapper only chooses the FMA path the way Go
//!   does here: `util::rand::math` reads the CPU without `GODEBUG`, while Go's
//!   `internal/cpu` lets `GODEBUG=cpu.avx=off` or `cpu.fma=off` turn the path
//!   off, and [`Feature::current`] honours that.
//! - [`global`]: the source of Go's top-level `rand.Float32` and
//!   `rand.NormFloat64`. Go seeds it randomly at start-up, so no program can
//!   depend on its values; this one is splitmix64 with a random seed, with
//!   Go's `Float32`, and a Box-Muller `NormFloat64` (the same standard normal
//!   distribution, not Go's ziggurat).
//! - [`parallel_for`]: `parallel.For` with a background context.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::floats::Feature;
use crate::util::rand::math::exp_with;

use super::Rng;

/// Go's `math.Exp(x)` on amd64. Go takes the FMA path when
/// `cpu.X86.HasAVX && cpu.X86.HasFMA`, which `GODEBUG=cpu.avx=off` or
/// `cpu.fma=off` turns off.
pub(crate) fn exp(x: f64) -> f64 {
    exp_with(x, Feature::current().contains(Feature::AVX | Feature::FMA))
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
