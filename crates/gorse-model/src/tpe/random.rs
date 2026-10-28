// Ports: c-bata/goptuna@v0.9.0 sampler.go (RandomSampler, Sample), internal/random/multinomial.go (Multinomial, ArgMaxMultinomial)

//! Random search, the sampler a study uses before the TPE sampler has enough
//! trials, and the multinomial draws the TPE sampler makes.
//!
//! Go's `RandomSampler` draws from its own generator, seeded 0 by default, and
//! its categorical branch draws from Go's process-wide `math/rand` source. That
//! source is seeded at random since Go 1.20, so no output of it is
//! reproducible. Here each sampler holds its own handle to a source seeded at
//! random, shared by the random and the TPE sampler of one study.

use std::hash::{BuildHasher, RandomState};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gorse_common::util::rand::Rand;
use gorse_common::util::rand::math::{exp, log};

use super::distribution::Distribution;
use super::gomath::{go_max, go_min};
use super::{Error, Study};

/// The generator of Go's process-wide `math/rand` functions, shared by the
/// samplers of one study.
pub(super) type GlobalRand = Arc<Mutex<Rand>>;

/// A global source seeded at random, as Go's is (Go 1.20 and later).
pub(super) fn new_global_rand() -> GlobalRand {
    let seed = RandomState::new().hash_one(0x6770_7465_u64) as i64;
    Arc::new(Mutex::new(Rand::new(seed)))
}

/// Locks `m`, recovering the guard after a panic in another thread, since Go
/// has no poisoning and the generator is valid after any draw.
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Go's `random.Multinomial(1, pvals, size)`: for each of `size` draws, the index
/// of the first cumulative probability above a uniform draw from `global`.
/// When no cumulative probability is above the draw, the index is 0, as in Go.
pub(super) fn multinomial(global: &mut Rand, pvals: &[f64], size: usize) -> Vec<usize> {
    let cumsum = cum_sum(pvals);
    (0..size)
        .map(|_| {
            let r = global.float64();
            cumsum.iter().position(|&x| x > r).unwrap_or(0)
        })
        .collect()
}

/// Go's `random.ArgMaxMultinomial`: the index drawn from `pvals` by one uniform
/// draw from `global`, or an error when no cumulative probability is above it.
pub(super) fn arg_max_multinomial(global: &mut Rand, pvals: &[f64]) -> Result<usize, Error> {
    let cumsum = cum_sum(pvals);
    let r = global.float64();
    cumsum
        .iter()
        .position(|&x| x > r)
        .ok_or(Error::InvalidPvals)
}

/// Go's `floats.CumSum`, sequentially. gonum's amd64 routine is SIMD and
/// associates the sums differently, so its last bits can differ.
fn cum_sum(s: &[f64]) -> Vec<f64> {
    let mut dst = Vec::with_capacity(s.len());
    for (i, &v) in s.iter().enumerate() {
        dst.push(if i == 0 { v } else { dst[i - 1] + v });
    }
    dst
}

/// goptuna's `RandomSampler`: uniform draws within each distribution.
pub struct RandomSampler {
    rng: Mutex<Rand>,
    global: GlobalRand,
}

impl RandomSampler {
    /// goptuna's `NewRandomSampler`: a generator seeded with 0, which is what
    /// makes the samples of a study reproducible.
    pub fn new() -> Self {
        Self::with_seed(0)
    }

    /// goptuna's `RandomSamplerOptionSeed`.
    pub fn with_seed(seed: i64) -> Self {
        Self::with_seed_and_global(seed, new_global_rand())
    }

    /// A sampler whose categorical draws come from `global`, so that a sampler
    /// and the TPE sampler of one study share one global source.
    pub(super) fn with_seed_and_global(seed: i64, global: GlobalRand) -> Self {
        RandomSampler {
            rng: Mutex::new(Rand::new(seed)),
            global,
        }
    }
}

impl Default for RandomSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl super::Sampler for RandomSampler {
    fn sample(
        &self,
        _study: &Study,
        _name: &str,
        distribution: &Distribution,
    ) -> Result<f64, Error> {
        let mut rng = lock(&self.rng);
        Ok(match *distribution {
            Distribution::Uniform { low, high } => {
                if distribution.single() {
                    return Ok(low);
                }
                rng.float64() * (high - low) + low
            }
            Distribution::LogUniform { low, high } => {
                if distribution.single() {
                    return Ok(low);
                }
                let log_low = log(low);
                let log_high = log(high);
                exp(rng.float64() * (log_high - log_low) + log_low)
            }
            Distribution::IntUniform { low, high } => {
                if distribution.single() {
                    return Ok(low as f64);
                }
                (rng.intn(high - low) + low) as f64
            }
            Distribution::StepIntUniform { low, high, step } => {
                if distribution.single() {
                    return Ok(low as f64);
                }
                let r = (high - low) / step;
                (rng.intn(r) * step + low) as f64
            }
            Distribution::DiscreteUniform { low, high, q } => {
                if distribution.single() {
                    return Ok(low);
                }
                let r = high - low;
                // [low, high] is shifted to [0, r] to align the samples on the grid.
                let grid_low = 0.0 - 0.5 * q;
                let grid_high = r + 0.5 * q;
                let x = rng.float64() * (grid_high - grid_low) + grid_low;
                let v = (x / q).round() * q + low;
                go_min(go_max(v, low), high)
            }
            Distribution::Categorical { ref choices } => {
                if distribution.single() {
                    return Ok(0.0);
                }
                lock(&self.global).intn(choices.len() as i64) as f64
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tpe::{Direction, Sampler};

    #[test]
    fn cum_sum_accumulates_in_order() {
        assert_eq!(cum_sum(&[0.5, 0.25, 0.25]), vec![0.5, 0.75, 1.0]);
        assert!(cum_sum(&[]).is_empty());
    }

    #[test]
    fn arg_max_multinomial_reports_invalid_pvals() {
        let mut rng = Rand::new(1);
        assert!(matches!(
            arg_max_multinomial(&mut rng, &[0.0, 0.0]),
            Err(Error::InvalidPvals)
        ));
    }

    #[test]
    fn multinomial_defaults_to_index_zero_past_the_last_bucket() {
        // With pvals that do not sum to 1, a draw above the total falls through to 0.
        let mut rng = Rand::new(1);
        let got = multinomial(&mut rng, &[0.0, 0.0], 3);
        assert_eq!(got, vec![0, 0, 0]);
    }

    fn study() -> Study {
        Study::new(Direction::Minimize, Box::new(RandomSampler::new()))
    }

    #[test]
    fn seed_decides_the_sample() {
        // goptuna's TestRandomSamplerOptionSeed: the default seed and seed 2
        // draw different values, and seed 2 draws the same value twice.
        let study = study();
        let distributions = [
            Distribution::Uniform {
                low: 0.0,
                high: 10.0,
            },
            Distribution::IntUniform { low: 0, high: 10 },
        ];
        for distribution in distributions {
            let s1 = RandomSampler::new()
                .sample(&study, "foo", &distribution)
                .expect("sampling does not fail");
            let s2 = RandomSampler::with_seed(2)
                .sample(&study, "foo", &distribution)
                .expect("sampling does not fail");
            let s3 = RandomSampler::with_seed(2)
                .sample(&study, "foo", &distribution)
                .expect("sampling does not fail");
            assert_ne!(s1, s2, "{distribution:?}");
            assert_eq!(s2, s3, "{distribution:?}");
        }
    }

    #[test]
    fn log_uniform_samples_stay_in_range() {
        // goptuna's TestRandomSampler_SampleLogUniform.
        let study = study();
        let sampler = RandomSampler::new();
        let distribution = Distribution::LogUniform {
            low: 1e-7,
            high: 1.0,
        };
        for _ in 0..100 {
            let sampled = sampler
                .sample(&study, "x", &distribution)
                .expect("sampling does not fail");
            assert!((1e-7..=1.0).contains(&sampled), "got {sampled}");
        }
    }

    #[test]
    fn discrete_uniform_samples_are_on_the_grid() {
        // goptuna's TestRandomSampler_SampleDiscreteUniform.
        let study = study();
        let sampler = RandomSampler::new();
        let distribution = Distribution::DiscreteUniform {
            low: -10.0,
            high: 10.0,
            q: 0.1,
        };
        for _ in 0..100 {
            let sampled = sampler
                .sample(&study, "x", &distribution)
                .expect("sampling does not fail");
            assert!((-10.0..=10.0).contains(&sampled), "got {sampled}");
            let point = (sampled - -10.0) / 0.1;
            assert!(
                (point - point.round()).abs() <= 1e-6,
                "{sampled} is not on the grid"
            );
        }
    }
}
