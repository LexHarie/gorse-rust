// Ports: c-bata/goptuna@v0.9.0 tpe/sampler.go (DefaultGamma, HyperoptDefaultGamma, DefaultWeights, Sampler, NewSampler, Sample, splitObservationPairs, sampleFromGMM, normalCDF, logNormalCDF, logsumRows, gmmLogPDF, sampleFromCategoricalDist, categoricalLogPDF, compare, sampleNumerical, sampleUniform, sampleLogUniform, sampleInt, sampleStepInt, sampleDiscreteUniform, sampleCategorical, getObservationPairs), tpe/sampler_option.go, tpe/array.go (ones1d, linspace, bincount, argSort2d)

//! The tree-structured Parzen estimator (TPE) sampler: after a few startup
//! trials, it models the good and the bad observations of each parameter with
//! a Parzen estimator and suggests the candidate that is most likely under the
//! good model relative to the bad one.

use std::cmp::Ordering;
use std::sync::Mutex;

use gorse_common::util::rand::Rand;
use gorse_common::util::rand::math::{exp, log};

use super::distribution::Distribution;
use super::gomath::{erf, go_max, go_min};
use super::parzen::{EPS, FuncWeights, ParzenEstimator, ParzenEstimatorParams, choice, sum};
use super::random::{
    GlobalRand, RandomSampler, arg_max_multinomial, lock, multinomial, new_global_rand,
};
use super::{Direction, Error, Sampler, Study, TrialState};

/// goptuna's `FuncGamma`: the number of observations counted as good, from the
/// number of observations.
pub type FuncGamma = fn(usize) -> usize;

/// goptuna's `DefaultGamma`: the best tenth of the observations, at most 25.
pub fn default_gamma(x: usize) -> usize {
    let a = (0.1 * x as f64).ceil() as usize;
    if a > 25 { 25 } else { a }
}

/// goptuna's `HyperoptDefaultGamma`: the best quarter of the observations, and
/// at least 25.
pub fn hyperopt_default_gamma(x: usize) -> usize {
    let a = (0.25 * x as f64).ceil() as usize;
    if a > 25 { a } else { 25 }
}

/// goptuna's `DefaultWeights`: a linear ramp over the oldest observations, then
/// weight 1 for the latest 25.
pub fn default_weights(x: usize) -> Vec<f64> {
    if x == 0 {
        vec![]
    } else if x < 25 {
        vec![1.0; x]
    } else {
        let mut ramp = linspace(1.0 / x as f64, 1.0, x - 25);
        ramp.extend(std::iter::repeat_n(1.0, 25));
        ramp
    }
}

/// goptuna's `linspace(start, stop, num, true)`.
fn linspace(start: f64, stop: f64, num: usize) -> Vec<f64> {
    if num == 1 {
        return vec![start];
    }
    let step = (stop - start) / (num as f64 - 1.0);
    (0..num).map(|i| start + i as f64 * step).collect()
}

/// goptuna's `bincount`: the weighted count of each value in `x`, with at
/// least `minlength` buckets. A value's weight is `weights[value]` when there
/// is one, and 1 otherwise.
fn bincount(x: &[usize], weights: &[f64], minlength: usize) -> Vec<f64> {
    let mut counts = vec![0.0; minlength];
    for &value in x {
        if value >= counts.len() {
            counts.resize(value + 1, 0.0);
        }
        if value >= weights.len() {
            counts[value] += 1.0;
        } else {
            counts[value] += weights[value];
        }
    }
    counts
}

/// goptuna's `argSort2d`: the indices of the loss pairs in ascending order, by
/// the first loss and then the second. Equal pairs keep their order.
fn arg_sort_2d(loss_vals: &[[f64; 2]]) -> Vec<usize> {
    let less = |a: usize, b: usize| {
        let (x, y) = (loss_vals[a], loss_vals[b]);
        if x[0] == y[0] {
            x[1] < y[1]
        } else {
            x[0] < y[0]
        }
    };
    let mut indices: Vec<usize> = (0..loss_vals.len()).collect();
    indices.sort_by(|&a, &b| {
        if less(a, b) {
            Ordering::Less
        } else if less(b, a) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    indices
}

/// gonum's `floats.Max`: the first largest value that is not NaN, or the
/// first element when every element is NaN.
fn floats_max(s: &[f64]) -> f64 {
    let mut max = f64::NAN;
    let mut ind = 0;
    for (i, &v) in s.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        if v > max || max.is_nan() {
            max = v;
            ind = i;
        }
    }
    s[ind]
}

/// Go's `normalCDF`: the normal cumulative distribution at `x` for each component.
fn normal_cdf(x: f64, mu: &[f64], sigma: &[f64]) -> Vec<f64> {
    mu.iter()
        .zip(sigma)
        .map(|(&m, &s)| {
            let denominator = x - m;
            let numerator = go_max(2f64.sqrt() * s, EPS);
            let z = denominator / numerator;
            0.5 * (1.0 + erf(z))
        })
        .collect()
}

/// Go's `logNormalCDF`: the log-normal cumulative distribution at `x` for each
/// component. `x` must not be negative.
fn log_normal_cdf(x: f64, mu: &[f64], sigma: &[f64]) -> Vec<f64> {
    assert!(x >= 0.0, "negative argument is given to logNormalCDF");
    mu.iter()
        .zip(sigma)
        .map(|(&m, &s)| {
            let denominator = log(go_max(x, EPS)) - m;
            let numerator = go_max(2f64.sqrt() * s, EPS);
            let z = denominator / numerator;
            0.5 + 0.5 * erf(z)
        })
        .collect()
}

/// goptuna's `logsumRows`: for each row, the log of the sum of its exponentials.
fn logsum_rows(x: &[Vec<f64>]) -> Vec<f64> {
    x.iter()
        .map(|row| {
            let m = floats_max(row);
            let mut sum = 0.0;
            for &v in row {
                sum += log(exp(v - m));
            }
            sum + m
        })
        .collect()
}

/// goptuna's `categoricalLogPDF`: the log of the probability of each sample.
fn categorical_log_pdf(sample: &[usize], p: &[f64]) -> Vec<f64> {
    sample.iter().map(|&s| log(p[s])).collect()
}

/// goptuna's `compare`: the sample whose log-likelihood ratio, below over above,
/// is the largest. The first one wins a tie.
fn compare(samples: &[f64], log_l: &[f64], log_g: &[f64]) -> f64 {
    assert_eq!(
        log_l.len(),
        log_g.len(),
        "the size of the log_l and log_g should be same"
    );
    let score: Vec<f64> = log_l.iter().zip(log_g).map(|(l, g)| l - g).collect();
    assert_eq!(
        samples.len(),
        score.len(),
        "the size of the samples and score should be same"
    );
    let mut best = 0;
    let mut max = score[0];
    for (i, &s) in score.iter().enumerate().skip(1) {
        if s > max {
            max = s;
            best = i;
        }
    }
    samples[best]
}

/// goptuna's `getObservationPairs`: for each trial that has the parameter, its
/// value and its loss pair. A complete trial's pair is `(-Inf, sign*value)`. A
/// pruned one's is `(-step, sign*value)` at its last intermediate step, or
/// `(+Inf, 0)` with no intermediate value. `sign` is -1 when maximizing.
fn get_observation_pairs(study: &Study, name: &str) -> (Vec<f64>, Vec<[f64; 2]>) {
    let sign = if study.direction() == Direction::Maximize {
        -1.0
    } else {
        1.0
    };
    let mut values = Vec::new();
    let mut scores = Vec::new();
    for trial in study.trials() {
        if trial.state != TrialState::Complete && trial.state != TrialState::Pruned {
            continue;
        }
        let Some(&value) = trial.internal_params.get(name) else {
            continue;
        };
        let (score0, score1) = if trial.state == TrialState::Complete {
            (f64::NEG_INFINITY, sign * trial.value)
        } else if !trial.intermediate_values.is_empty() {
            let mut step = 0;
            let mut intermediate = 0.0;
            for (&key, &v) in &trial.intermediate_values {
                if key > step {
                    step = key;
                    intermediate = v;
                }
            }
            (-(step as f64), sign * intermediate)
        } else {
            (f64::INFINITY, 0.0)
        };
        values.push(value);
        scores.push([score0, score1]);
    }
    (values, scores)
}

/// goptuna's `Sampler` for TPE: random draws during the startup trials, then
/// the candidate that the good and bad Parzen estimators rank best.
pub struct TpeSampler {
    number_of_startup_trials: usize,
    number_of_ei_candidates: usize,
    gamma: FuncGamma,
    params: ParzenEstimatorParams,
    /// Go's `rng`, which the normal draws come from. Its lock is held for a
    /// whole [`Sampler::sample`] call, as Go's `mu` is.
    rng: Mutex<Rand>,
    /// Go's process-wide `math/rand` source, for the multinomial draws.
    global: GlobalRand,
    random_sampler: RandomSampler,
}

impl Default for TpeSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl TpeSampler {
    /// goptuna's `NewSampler` with its defaults: 10 startup trials, 24 candidates,
    /// `DefaultGamma`, a prior of weight 1 with magic clipping, and no endpoint
    /// sigmas. Its generators are seeded 0, as goptuna's are.
    pub fn new() -> Self {
        let global = new_global_rand();
        TpeSampler {
            number_of_startup_trials: 10,
            number_of_ei_candidates: 24,
            gamma: default_gamma,
            params: ParzenEstimatorParams {
                consider_prior: true,
                consider_magic_clip: true,
                consider_endpoints: false,
                weights: default_weights,
                prior_weight: 1.0,
            },
            rng: Mutex::new(Rand::new(0)),
            random_sampler: RandomSampler::with_seed_and_global(0, global.clone()),
            global,
        }
    }

    /// goptuna's `SamplerOptionSeed`: seeds the sampler's generator and the
    /// random sampler's.
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.rng = Mutex::new(Rand::new(seed));
        self.random_sampler = RandomSampler::with_seed_and_global(seed, self.global.clone());
        self
    }

    /// goptuna's `SamplerOptionNumberOfStartupTrials`.
    pub fn with_number_of_startup_trials(mut self, n: usize) -> Self {
        self.number_of_startup_trials = n;
        self
    }

    /// goptuna's `SamplerOptionNumberOfEICandidates`.
    pub fn with_number_of_ei_candidates(mut self, n: usize) -> Self {
        self.number_of_ei_candidates = n;
        self
    }

    /// goptuna's `SamplerOptionGammaFunc`.
    pub fn with_gamma(mut self, gamma: FuncGamma) -> Self {
        self.gamma = gamma;
        self
    }

    /// goptuna's `SamplerOptionParzenEstimatorParams`: replaces all the
    /// Parzen estimator settings at once.
    pub fn with_parzen_estimator_params(mut self, params: ParzenEstimatorParams) -> Self {
        self.params = params;
        self
    }

    /// goptuna's `SamplerOptionConsiderPrior`.
    pub fn with_consider_prior(mut self, consider_prior: bool) -> Self {
        self.params.consider_prior = consider_prior;
        self
    }

    /// goptuna's `SamplerOptionPriorWeight`.
    pub fn with_prior_weight(mut self, prior_weight: f64) -> Self {
        self.params.prior_weight = prior_weight;
        self
    }

    /// goptuna's `SamplerOptionConsiderMagicClip`.
    pub fn with_consider_magic_clip(mut self, consider_magic_clip: bool) -> Self {
        self.params.consider_magic_clip = consider_magic_clip;
        self
    }

    /// goptuna's `SamplerOptionConsiderEndpoints`.
    pub fn with_consider_endpoints(mut self, consider_endpoints: bool) -> Self {
        self.params.consider_endpoints = consider_endpoints;
        self
    }

    /// goptuna's `SamplerOptionWeights`.
    pub fn with_weights(mut self, weights: FuncWeights) -> Self {
        self.params.weights = weights;
        self
    }

    /// goptuna's `splitObservationPairs`: the values of the good observations and
    /// of the bad ones.
    fn split_observation_pairs(
        &self,
        config_vals: &[f64],
        loss_vals: &[[f64; 2]],
    ) -> (Vec<f64>, Vec<f64>) {
        let nbelow = (self.gamma)(config_vals.len());
        let mut loss_ascending = arg_sort_2d(loss_vals);

        loss_ascending[..nbelow].sort_unstable();
        let below = choice(config_vals, &loss_ascending[..nbelow]);

        loss_ascending[nbelow..].sort_unstable();
        let above = choice(config_vals, &loss_ascending[nbelow..]);
        (below, above)
    }

    /// goptuna's `sampleFromGMM`: `size` samples from the mixture, inside
    /// `[low, high)`, exponentiated when `is_log`, and rounded to `q` when `q > 0`.
    #[allow(clippy::too_many_arguments)]
    fn sample_from_gmm(
        &self,
        rng: &mut Rand,
        pe: &ParzenEstimator,
        low: f64,
        high: f64,
        size: usize,
        q: f64,
        is_log: bool,
    ) -> Vec<f64> {
        assert!(low <= high, "the low should be lower than the high");
        let mut samples = Vec::with_capacity(size);
        while samples.len() != size {
            let active = {
                let mut global = lock(&self.global);
                arg_max_multinomial(&mut global, &pe.weights).unwrap_or_else(|e| panic!("{e}"))
            };
            let x = rng.norm_float64();
            let draw = x * pe.sigmas[active] + pe.mus[active];
            if low <= draw && draw < high {
                samples.push(draw);
            }
        }
        if is_log {
            for s in &mut samples {
                *s = exp(*s);
            }
        }
        if q > 0.0 {
            for s in &mut samples {
                *s = (*s / q).round() * q;
            }
        }
        samples
    }

    /// goptuna's `gmmLogPDF`: the log density of each sample under the mixture,
    /// with the q-discretized form when `q > 0`.
    #[allow(clippy::too_many_arguments)]
    fn gmm_log_pdf(
        &self,
        samples: &[f64],
        pe: &ParzenEstimator,
        low: f64,
        high: f64,
        q: f64,
        is_log: bool,
    ) -> Vec<f64> {
        if samples.is_empty() {
            return vec![];
        }
        let weights = &pe.weights;
        let mus = &pe.mus;
        let sigmas = &pe.sigmas;

        let high_normal_cdf = normal_cdf(high, mus, sigmas);
        let low_normal_cdf = normal_cdf(low, mus, sigmas);
        assert_eq!(
            weights.len(),
            high_normal_cdf.len(),
            "the length should be the same with weights"
        );
        let mut paccept = 0.0;
        for i in 0..high_normal_cdf.len() {
            paccept += high_normal_cdf[i] * weights[i] - low_normal_cdf[i];
        }

        if q > 0.0 {
            assert!(
                weights.len() == mus.len() && weights.len() == sigmas.len(),
                "should be the same length of weights, mus and sigmas"
            );
            let mut probabilities = vec![0.0; samples.len()];
            for ((&w, &mu), &sigma) in weights.iter().zip(mus).zip(sigmas) {
                let mut upper_bound = vec![0.0; samples.len()];
                let mut lower_bound = vec![0.0; samples.len()];
                for (i, &sample) in samples.iter().enumerate() {
                    if is_log {
                        upper_bound[i] = go_min(sample + q / 2.0, exp(high));
                        lower_bound[i] = go_max(sample - q / 2.0, exp(low));
                        lower_bound[i] = go_max(0.0, lower_bound[i]);
                    } else {
                        upper_bound[i] = go_min(sample + q / 2.0, high);
                        lower_bound[i] = go_max(sample - q / 2.0, low);
                    }
                }
                let mut inc_amt = vec![0.0; samples.len()];
                for j in 0..samples.len() {
                    if is_log {
                        inc_amt[j] = w * log_normal_cdf(upper_bound[j], &[mu], &[sigma])[0];
                        inc_amt[j] -= w * log_normal_cdf(lower_bound[j], &[mu], &[sigma])[0];
                    } else {
                        inc_amt[j] = w * normal_cdf(upper_bound[j], &[mu], &[sigma])[0];
                        inc_amt[j] -= w * normal_cdf(lower_bound[j], &[mu], &[sigma])[0];
                    }
                }
                for j in 0..samples.len() {
                    probabilities[j] += inc_amt[j];
                }
            }
            return probabilities
                .iter()
                .map(|&p| log(p + EPS) + log(paccept + EPS))
                .collect();
        }

        let jacobian: Vec<f64> = if is_log {
            samples.to_vec()
        } else {
            vec![1.0; samples.len()]
        };
        let distance: Vec<Vec<f64>> = samples
            .iter()
            .map(|&sample| {
                mus.iter()
                    .map(|&mu| {
                        if is_log {
                            log(sample) - mu
                        } else {
                            sample - mu
                        }
                    })
                    .collect()
            })
            .collect();
        let sigma_sq: Vec<f64> = sigmas
            .iter()
            .map(|&s| {
                let s = go_max(s, EPS);
                s * s
            })
            .collect();
        let y: Vec<Vec<f64>> = distance
            .iter()
            .enumerate()
            .map(|(i, row)| {
                row.iter()
                    .enumerate()
                    .map(|(j, &d)| {
                        let mahalanobis = d / sigma_sq[j];
                        let z = (2.0 * std::f64::consts::PI).sqrt() * sigmas[j] * jacobian[i];
                        let coefficient = weights[j] / z / paccept;
                        -0.5 * mahalanobis + log(coefficient)
                    })
                    .collect()
            })
            .collect();
        logsum_rows(&y)
    }

    /// goptuna's `sampleNumerical`: the best of the candidates drawn from the
    /// good mixture, for a continuous or a log-scale range.
    #[allow(clippy::too_many_arguments)]
    fn sample_numerical(
        &self,
        rng: &mut Rand,
        mut low: f64,
        mut high: f64,
        below: &mut [f64],
        above: &mut [f64],
        q: f64,
        is_log: bool,
    ) -> f64 {
        if is_log {
            low = log(low);
            high = log(high);
            for b in below.iter_mut() {
                *b = log(*b);
            }
            for a in above.iter_mut() {
                *a = log(*a);
            }
        }
        let size = self.number_of_ei_candidates;
        let parzen_estimator_below = ParzenEstimator::new(below, low, high, &self.params);
        let sample_below =
            self.sample_from_gmm(rng, &parzen_estimator_below, low, high, size, q, is_log);
        let log_likelihoods_below =
            self.gmm_log_pdf(&sample_below, &parzen_estimator_below, low, high, q, is_log);

        let parzen_estimator_above = ParzenEstimator::new(above, low, high, &self.params);
        let log_likelihoods_above =
            self.gmm_log_pdf(&sample_below, &parzen_estimator_above, low, high, q, is_log);

        compare(
            &sample_below,
            &log_likelihoods_below,
            &log_likelihoods_above,
        )
    }

    /// goptuna's `sampleDiscreteUniform`: a sample from a discretized range,
    /// shifted so that its grid starts at `low`.
    fn sample_discrete_uniform(
        &self,
        rng: &mut Rand,
        low: f64,
        high: f64,
        q: f64,
        below: &mut [f64],
        above: &mut [f64],
    ) -> f64 {
        let r = high - low;
        // [low, high] is shifted to [0, r] to align the samples on the grid.
        let grid_low = 0.0 - 0.5 * q;
        let grid_high = r + 0.5 * q;
        for b in below.iter_mut() {
            *b -= low;
        }
        for a in above.iter_mut() {
            *a -= low;
        }
        let best = self.sample_numerical(rng, grid_low, grid_high, below, above, q, false) + low;
        go_min(go_max(best, low), high)
    }

    /// goptuna's `sampleCategorical`: the choice whose weighted count is the
    /// most likely among the good observations, relative to the bad ones.
    fn sample_categorical(&self, upper: usize, below: &[f64], above: &[f64]) -> f64 {
        let below_int: Vec<usize> = below.iter().map(|&b| b as usize).collect();
        let above_int: Vec<usize> = above.iter().map(|&a| a as usize).collect();
        let size = if self.number_of_ei_candidates >= upper {
            upper
        } else {
            self.number_of_ei_candidates
        };

        // below
        let weighted_below = self.weighted_counts(&below_int, below.len(), upper);
        let samples: Vec<usize> = if self.number_of_ei_candidates != size {
            (0..size).collect()
        } else {
            let mut global = lock(&self.global);
            multinomial(&mut global, &weighted_below, size)
        };
        let log_likelihoods_below = categorical_log_pdf(&samples, &weighted_below);

        // above
        let weighted_above = self.weighted_counts(&above_int, above.len(), upper);
        let log_likelihoods_above = categorical_log_pdf(&samples, &weighted_above);

        let float_samples: Vec<f64> = samples.iter().map(|&s| s as f64).collect();
        compare(
            &float_samples,
            &log_likelihoods_below,
            &log_likelihoods_above,
        )
    }

    /// The probability of each choice from its weighted count, with the prior.
    fn weighted_counts(&self, values: &[usize], n: usize, upper: usize) -> Vec<f64> {
        let weights = (self.params.weights)(n);
        let counts = bincount(values, &weights, upper);
        let mut weighted: Vec<f64> = counts
            .iter()
            .map(|&c| c + self.params.prior_weight)
            .collect();
        let weighted_sum = sum(&weighted);
        for w in &mut weighted {
            *w /= weighted_sum;
        }
        weighted
    }
}

impl Sampler for TpeSampler {
    fn sample(&self, study: &Study, name: &str, distribution: &Distribution) -> Result<f64, Error> {
        let mut rng = lock(&self.rng);
        let (values, scores) = get_observation_pairs(study, name);
        if values.len() < self.number_of_startup_trials {
            return self.random_sampler.sample(study, name, distribution);
        }

        let (mut below, mut above) = self.split_observation_pairs(&values, &scores);
        Ok(match distribution {
            Distribution::Uniform { low, high } => {
                self.sample_numerical(&mut rng, *low, *high, &mut below, &mut above, 0.0, false)
            }
            Distribution::LogUniform { low, high } => {
                self.sample_numerical(&mut rng, *low, *high, &mut below, &mut above, 0.0, true)
            }
            Distribution::IntUniform { low, high } => {
                let q = 1.0;
                let low = *low as f64 - 0.5 * q;
                let high = *high as f64 + 0.5 * q;
                self.sample_numerical(&mut rng, low, high, &mut below, &mut above, q, false)
            }
            Distribution::StepIntUniform { low, high, .. } => {
                let q = 1.0;
                let low = *low as f64 - 0.5 * q;
                let high = *high as f64 + 0.5 * q;
                self.sample_numerical(&mut rng, low, high, &mut below, &mut above, q, false)
            }
            Distribution::DiscreteUniform { low, high, q } => {
                self.sample_discrete_uniform(&mut rng, *low, *high, *q, &mut below, &mut above)
            }
            Distribution::Categorical { choices } => {
                self.sample_categorical(choices.len(), &below, &above)
            }
        })
    }
}

#[cfg(test)]
impl TpeSampler {
    /// The sampler of the Go golden run (`contracts/tools/tpegolden`): seeded
    /// like `tpe.SamplerOptionSeed(seed)`, with the global source seeded 1 as
    /// `GODEBUG=randautoseed=0` seeds Go's.
    pub(super) fn with_go_golden_seeds(seed: i64) -> Self {
        let global: GlobalRand = std::sync::Arc::new(Mutex::new(Rand::new(1)));
        TpeSampler {
            rng: Mutex::new(Rand::new(seed)),
            random_sampler: RandomSampler::with_seed_and_global(seed, global.clone()),
            global,
            ..TpeSampler::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gamma_matches_goptuna() {
        assert_eq!(default_gamma(5), 1);
        assert_eq!(default_gamma(100), 10);
        assert_eq!(default_gamma(255), 25);
    }

    #[test]
    fn hyperopt_default_gamma_matches_goptuna() {
        assert_eq!(hyperopt_default_gamma(5), 25);
    }

    #[test]
    fn default_weights_matches_goptuna() {
        // goptuna's TestDefaultWeights with x = 30: a ramp of five, then 25 ones.
        let want: Vec<f64> = (0..5)
            .map(|i| (1.0 - 1.0 / 30.0) * i as f64 / 4.0 + 1.0 / 30.0)
            .chain(std::iter::repeat_n(1.0, 25))
            .collect();
        assert_eq!(default_weights(30), want);
    }

    #[test]
    fn get_observation_pairs_minimize_matches_goptuna() {
        // goptuna's TestGetObservationPairs_MINIMIZE, the study's four trials.
        let mut study =
            Study::new(Direction::Minimize, Box::new(RandomSampler::new())).with_ignore_error(true);
        study
            .optimize(
                |trial| {
                    let x = trial
                        .suggest_int("x", 5, 5)
                        .map_err(super::super::Failure::error)?;
                    match trial.number() {
                        0 => Ok(x as f64),
                        1 => {
                            trial
                                .set_intermediate_value(4, 1.0)
                                .map_err(super::super::Failure::error)?;
                            trial
                                .set_intermediate_value(7, 2.0)
                                .map_err(super::super::Failure::error)?;
                            Err(super::super::Failure::Pruned)
                        }
                        2 => Err(super::super::Failure::Pruned),
                        _ => Err(super::super::Failure::error(std::io::Error::other(
                            "runtime error",
                        ))),
                    }
                },
                4,
            )
            .unwrap();
        let (values, scores) = get_observation_pairs(&study, "x");
        assert_eq!(values, vec![5.0, 5.0, 5.0]);
        assert_eq!(
            scores,
            vec![[f64::NEG_INFINITY, 5.0], [-7.0, 2.0], [f64::INFINITY, 0.0]]
        );
    }

    #[test]
    fn get_observation_pairs_maximize_matches_goptuna() {
        let mut study =
            Study::new(Direction::Maximize, Box::new(RandomSampler::new())).with_ignore_error(true);
        study
            .optimize(
                |trial| {
                    let x = trial
                        .suggest_int("x", 5, 5)
                        .map_err(super::super::Failure::error)?;
                    match trial.number() {
                        0 => Ok(x as f64),
                        1 => {
                            trial
                                .set_intermediate_value(4, 1.0)
                                .map_err(super::super::Failure::error)?;
                            trial
                                .set_intermediate_value(7, 2.0)
                                .map_err(super::super::Failure::error)?;
                            Err(super::super::Failure::Pruned)
                        }
                        2 => Err(super::super::Failure::Pruned),
                        _ => Err(super::super::Failure::error(std::io::Error::other(
                            "runtime error",
                        ))),
                    }
                },
                4,
            )
            .unwrap();
        let (values, scores) = get_observation_pairs(&study, "x");
        assert_eq!(values, vec![5.0, 5.0, 5.0]);
        assert_eq!(
            scores,
            vec![
                [f64::NEG_INFINITY, -5.0],
                [-7.0, -2.0],
                [f64::INFINITY, 0.0]
            ]
        );
    }

    #[test]
    fn split_observation_pairs_matches_goptuna() {
        // goptuna's TestSampler_splitObservationPairs, generated from Optuna.
        let config_vals = [
            7.515720606531342,
            5.350185623031333,
            5.124041307972975,
            1.4089387361626944,
            -2.895952062621281,
            -8.814621912214118,
            7.603846274084024,
            5.915757103674883,
            8.364607575197955,
            1.4694727910185534,
        ];
        let loss_vals = [
            [f64::NEG_INFINITY, 51.07650573447907],
            [f64::NEG_INFINITY, 100.79007507622603],
            [f64::NEG_INFINITY, 20.712990047058412],
            [f64::NEG_INFINITY, 142.49871053544777],
            [f64::NEG_INFINITY, 61.74467260557292],
            [f64::NEG_INFINITY, 116.44303200021926],
            [f64::NEG_INFINITY, 132.8075795417795],
            [f64::NEG_INFINITY, 25.243709057350483],
            [f64::NEG_INFINITY, 141.5303287376019],
            [f64::NEG_INFINITY, 33.64359889992425],
        ];
        let sampler = TpeSampler::new()
            .with_number_of_startup_trials(0)
            .with_gamma(default_gamma);
        let (below, above) = sampler.split_observation_pairs(&config_vals, &loss_vals);
        let want_below = [5.12404131];
        let want_above = [
            7.51572061,
            5.35018562,
            1.40893874,
            -2.89595206,
            -8.81462191,
            7.60384627,
            5.9157571,
            8.36460758,
            1.46947279,
        ];
        assert_close(&below, &want_below, 1e-6);
        assert_close(&above, &want_above, 1e-6);
    }

    #[test]
    fn sample_categorical_matches_goptuna() {
        // goptuna's TestSampler_SampleCategorical.
        let below = [1.0];
        let above = [1.0, 3.0, 3.0, 2.0, 3.0, 0.0, 2.0, 3.0, 3.0];
        let sampler = TpeSampler::new();
        assert_eq!(sampler.sample_categorical(4, &below, &above), 1.0);
    }

    fn assert_close(got: &[f64], want: &[f64], eps: f64) {
        assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() <= eps, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn linspace_matches_goptuna() {
        // goptuna's TestLinspace. Go writes each value as an exact constant,
        // which rounds once; these literals are those rounded values.
        assert_eq!(
            linspace(1.0 / 30.0, 1.0, 5),
            vec![1.0 / 30.0, 0.275, 62.0 / 120.0, 91.0 / 120.0, 1.0]
        );
    }

    #[test]
    fn arg_sort_2d_matches_goptuna() {
        // goptuna's TestArgSort2DFloat64.
        let inf = f64::NEG_INFINITY;
        let loss_vals = [
            [inf, 93.80856756],
            [inf, 85.64538195],
            [inf, 44.58783514],
            [inf, 4.23458368],
            [inf, 42.17125041],
            [inf, 62.14283937],
            [inf, 94.45778947],
            [inf, 64.66469149],
            [inf, 36.1033201],
            [inf, 105.69868952],
        ];
        assert_eq!(arg_sort_2d(&loss_vals), vec![3, 8, 4, 2, 5, 7, 1, 0, 6, 9]);

        let loss_vals = [
            [3.0, 93.80856756],
            [5.0, 85.64538195],
            [inf, 44.58783514],
            [inf, 4.23458368],
            [inf, 42.17125041],
            [inf, 62.14283937],
            [inf, 94.45778947],
            [inf, 64.66469149],
            [inf, 36.1033201],
            [inf, 105.69868952],
        ];
        assert_eq!(arg_sort_2d(&loss_vals), vec![3, 8, 4, 2, 5, 7, 6, 9, 0, 1]);
    }

    #[test]
    fn log_uniform_samples_stay_in_range() {
        // goptuna's TestSampler_SampleLogUniform, with no startup trials.
        let sampler = TpeSampler::new().with_number_of_startup_trials(0);
        let study = Study::new(Direction::Minimize, Box::new(TpeSampler::new()));
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
        // goptuna's TestSampler_SampleDiscreteUniform, with no startup trials.
        let sampler = TpeSampler::new().with_number_of_startup_trials(0);
        let study = Study::new(Direction::Minimize, Box::new(TpeSampler::new()));
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
