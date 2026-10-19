// Ports: common/util/random.go @ add21b5

//! Gorse's random generator: Go's `math/rand` generator ([`Rand`]) with
//! helpers to fill vectors and matrices and to sample distinct integers.

use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

use super::rand::{LockedSource, Rand};

/// Go's `util.RandomGenerator`, a `*rand.Rand` with extra methods. Every
/// [`Rand`] method is available through `Deref`, as through Go's embedding.
#[derive(Clone)]
pub struct RandomGenerator(Rand);

impl RandomGenerator {
    /// `NewRandomGenerator(seed)`.
    pub fn new(seed: i64) -> Self {
        RandomGenerator(Rand::new(seed))
    }

    /// A vector of `size` uniform floats in `[low, high)`.
    pub fn uniform_vector(&mut self, size: usize, low: f32, high: f32) -> Vec<f32> {
        let scale = high - low;
        (0..size).map(|_| self.float32() * scale + low).collect()
    }

    /// A vector of `size` normal floats.
    pub fn normal_vector(&mut self, size: usize, mean: f32, std_dev: f32) -> Vec<f32> {
        (0..size)
            .map(|_| self.norm_float64() as f32 * std_dev + mean)
            .collect()
    }

    /// A `row` x `col` matrix of normal floats, filled row by row.
    pub fn normal_matrix(
        &mut self,
        row: usize,
        col: usize,
        mean: f32,
        std_dev: f32,
    ) -> Vec<Vec<f32>> {
        (0..row)
            .map(|_| self.normal_vector(col, mean, std_dev))
            .collect()
    }

    /// A `row` x `col` matrix of uniform floats in `[low, high)`, filled row
    /// by row.
    pub fn uniform_matrix(&mut self, row: usize, col: usize, low: f32, high: f32) -> Vec<Vec<f32>> {
        (0..row)
            .map(|_| self.uniform_vector(col, low, high))
            .collect()
    }

    /// A vector of `size` normal doubles.
    pub fn normal_vector64(&mut self, size: usize, mean: f64, std_dev: f64) -> Vec<f64> {
        (0..size)
            .map(|_| self.norm_float64() * std_dev + mean)
            .collect()
    }

    /// Samples `n` distinct integers in `[low, high)` that are in none of the
    /// `exclude` sets.
    ///
    /// When `n` is at least the interval length minus the number of excluded
    /// values (counting excluded values outside the interval too, as Go
    /// does), every allowed value is returned in ascending order, which may
    /// be more or fewer than `n`. Otherwise values are drawn with
    /// [`Rand::intn`] until `n` distinct allowed ones are found.
    ///
    /// Arithmetic wraps around on overflow, as Go's `int` does.
    ///
    /// # Panics
    ///
    /// If the interval length is not positive while values must be drawn
    /// (Go panics in `Intn` then too).
    pub fn sample(&mut self, low: i64, high: i64, n: usize, exclude: &[&HashSet<i64>]) -> Vec<i64> {
        let interval_length = high.wrapping_sub(low);
        let mut exclude_set: HashSet<i64> =
            exclude.iter().flat_map(|set| set.iter().copied()).collect();
        let mut sampled = Vec::with_capacity(n);
        if n as i64 >= interval_length - exclude_set.len() as i64 {
            for i in low..high {
                if exclude_set.insert(i) {
                    sampled.push(i);
                }
            }
        } else {
            while sampled.len() < n {
                let v = self.intn(interval_length).wrapping_add(low);
                if exclude_set.insert(v) {
                    sampled.push(v);
                }
            }
        }
        sampled
    }

    /// [`RandomGenerator::sample`] for 32-bit integers, drawing with
    /// [`Rand::int31n`]. The interval length wraps around on overflow, as Go's
    /// `int32` subtraction does.
    ///
    /// # Panics
    ///
    /// If the interval length is not positive while values must be drawn.
    pub fn sample_int32(
        &mut self,
        low: i32,
        high: i32,
        n: usize,
        exclude: &[&HashSet<i32>],
    ) -> Vec<i32> {
        let interval_length = high.wrapping_sub(low);
        let mut exclude_set: HashSet<i32> =
            exclude.iter().flat_map(|set| set.iter().copied()).collect();
        let mut sampled = Vec::with_capacity(n);
        if n as i64 >= i64::from(interval_length) - exclude_set.len() as i64 {
            for i in low..high {
                if exclude_set.insert(i) {
                    sampled.push(i);
                }
            }
        } else {
            while sampled.len() < n {
                let v = self.int31n(interval_length).wrapping_add(low);
                if exclude_set.insert(v) {
                    sampled.push(v);
                }
            }
        }
        sampled
    }
}

impl Deref for RandomGenerator {
    type Target = Rand;

    fn deref(&self) -> &Rand {
        &self.0
    }
}

impl DerefMut for RandomGenerator {
    fn deref_mut(&mut self) -> &mut Rand {
        &mut self.0
    }
}

/// `util.NewRand(seed)`: a generator whose source can be shared between
/// threads by cloning the returned value.
pub fn new_rand(seed: i64) -> Rand<LockedSource> {
    Rand::from_source(LockedSource::new(seed))
}

#[cfg(test)]
mod tests {
    // Ports: common/util/random_test.go @ add21b5
    use super::*;

    const RANDOM_EPSILON: f32 = 0.1;

    #[test]
    fn test_random_generator_make_normal_matrix() {
        let mut rng = RandomGenerator::new(0);
        let vec = rng.normal_matrix(1, 1000, 1.0, 2.0).remove(0);
        assert!((mean(&vec) - 1.0).abs() <= RANDOM_EPSILON);
        assert!((std_dev(&vec) - 2.0).abs() <= RANDOM_EPSILON);
    }

    #[test]
    fn test_random_generator_make_uniform_matrix() {
        let mut rng = RandomGenerator::new(0);
        let vec = rng.uniform_matrix(1, 1000, 1.0, 2.0).remove(0);
        // Go asserts that lo.Min(vec) is not below 1 and lo.Max(vec) not above
        // 2; checking every element is the same bound.
        assert!(vec.iter().all(|&v| v >= 1.0));
        assert!(vec.iter().all(|&v| v <= 2.0));
    }

    #[test]
    fn test_random_generator_sample() {
        let exclude_set: HashSet<i64> = [0, 1, 2, 3, 4].into();
        let mut rng = RandomGenerator::new(0);
        for i in 1..=10 {
            let sampled = rng.sample(0, 10, i, &[&exclude_set]);
            for v in &sampled {
                assert!(!exclude_set.contains(v));
            }
        }
    }

    #[test]
    fn test_random_generator_sample_int32() {
        let exclude_set: HashSet<i32> = [0, 1, 2, 3, 4].into();
        let mut rng = RandomGenerator::new(0);
        for i in 1..=10 {
            let sampled = rng.sample_int32(0, 10, i, &[&exclude_set]);
            for v in &sampled {
                assert!(!exclude_set.contains(v));
            }
        }
    }

    /// Mean of a slice of 32-bit floats (`lo.Sum` adds in order).
    fn mean(x: &[f32]) -> f32 {
        x.iter().sum::<f32>() / x.len() as f32
    }

    /// Sample standard deviation.
    fn std_dev(x: &[f32]) -> f32 {
        mean_variance(x).1.sqrt()
    }

    /// Sample mean and unbiased variance by the corrected two-pass algorithm
    /// (Chan, Golub and LeVeque), as in the Go test.
    fn mean_variance(x: &[f32]) -> (f32, f32) {
        let m = mean(x);
        let mut ss = 0.0f32;
        let mut compensation = 0.0f32;
        for &v in x {
            let d = v - m;
            ss += d * d;
            compensation += d;
        }
        let variance = (ss - compensation * compensation / x.len() as f32) / (x.len() - 1) as f32;
        (m, variance)
    }
}
