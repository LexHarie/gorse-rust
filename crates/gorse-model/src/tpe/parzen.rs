// Ports: c-bata/goptuna@v0.9.0 tpe/parzen_estimator.go (ParzenEstimatorParams, ParzenEstimator, NewParzenEstimator, buildEstimator), tpe/array.go (choice, location, clip), gonum v0.17.0 floats.Argsort

//! The Parzen estimator of the TPE sampler: a mixture of Gaussians, one per
//! observed value, with a prior at the centre of the range.
//!
//! goptuna builds it with `floats.Argsort(mus, order)`, which sorts `mus` in
//! place. The estimator then reads `mus` again through `order`, so the mixture
//! it builds can be out of order (`PORTING_NOTES.md`, Q-tpe-argsort). This port
//! sorts the caller's slice in place too, and reproduces that behaviour.

use gorse_common::gosort;

use super::gomath::{go_max, go_min};

/// goptuna's `eps`: the floor for sigmas and probabilities.
pub(super) const EPS: f64 = 1e-12;

/// A function from the number of observations to their weights (goptuna's
/// `FuncWeights`).
pub type FuncWeights = fn(usize) -> Vec<f64>;

/// goptuna's `ParzenEstimatorParams`.
#[derive(Clone)]
pub struct ParzenEstimatorParams {
    /// Whether a Gaussian prior at the centre of the range is part of the mixture.
    pub consider_prior: bool,
    /// Whether sigmas are clipped to a minimum that depends on the number of observations.
    pub consider_magic_clip: bool,
    /// Whether the outermost sigmas use the full gap to the range's edge.
    pub consider_endpoints: bool,
    /// The weights of the observations.
    pub weights: FuncWeights,
    /// The weight of the prior.
    pub prior_weight: f64,
}

/// goptuna's `ParzenEstimator`: the mixture's weights, means and sigmas, in
/// the order they were built.
#[derive(Debug, Clone, PartialEq)]
pub struct ParzenEstimator {
    /// The normalized weight of each component.
    pub weights: Vec<f64>,
    /// The mean of each component.
    pub mus: Vec<f64>,
    /// The standard deviation of each component.
    pub sigmas: Vec<f64>,
}

impl ParzenEstimator {
    /// goptuna's `NewParzenEstimator`. `mus` is sorted in place, as goptuna's
    /// `floats.Argsort` sorts it.
    pub fn new(mus: &mut [f64], low: f64, high: f64, params: &ParzenEstimatorParams) -> Self {
        let (weights, mus, sigmas) = build_estimator(mus, low, high, params);
        ParzenEstimator {
            weights,
            mus,
            sigmas,
        }
    }
}

/// Go's `floats.Argsort`: sorts `values` in place in ascending order with
/// `sort.Sort`, and returns the original position of each element.
pub(super) fn argsort(values: &mut [f64]) -> Vec<usize> {
    let mut inds: Vec<usize> = (0..values.len()).collect();
    gosort::sort(&mut Argsort {
        values,
        inds: &mut inds,
    });
    inds
}

/// The `argsort` helper of gonum: swaps the values and their indices together.
struct Argsort<'a> {
    values: &'a mut [f64],
    inds: &'a mut [usize],
}

impl gosort::Interface for Argsort<'_> {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.values[i] < self.values[j]
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.values.swap(i, j);
        self.inds.swap(i, j);
    }
}

/// goptuna's `buildEstimator`.
fn build_estimator(
    mus: &mut [f64],
    low: f64,
    high: f64,
    params: &ParzenEstimatorParams,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let consider_prior = params.consider_prior;
    let prior_weight = params.prior_weight;
    let consider_magic_clip = params.consider_magic_clip;
    let consider_endpoints = params.consider_endpoints;
    let weights_func = params.weights;

    let mut prior_pos = 0;
    let mut prior_sigma = 0.0;
    let mut sorted_mus: Vec<f64>;
    let mut sigma: Vec<f64> = Vec::new();
    let order: Vec<usize>;

    if consider_prior {
        let prior_mu = 0.5 * (low + high);
        prior_sigma = 1.0 * (high - low);
        if mus.is_empty() {
            sorted_mus = vec![prior_mu];
            sigma = vec![prior_sigma];
            order = Vec::new();
        } else {
            order = argsort(mus);
            prior_pos = location(&choice(mus, &order), prior_mu);
            sorted_mus = Vec::with_capacity(mus.len() + 1);
            sorted_mus.extend(choice(mus, &order[..prior_pos]));
            sorted_mus.push(prior_mu);
            sorted_mus.extend(choice(mus, &order[prior_pos..]));
        }
    } else {
        order = argsort(mus);
        sorted_mus = choice(mus, &order);
    }

    // The sigmas: the gap to each neighbour, with the range's edges as the
    // outermost neighbours.
    if !mus.is_empty() {
        let mut low_sorted_mus_high = Vec::with_capacity(sorted_mus.len() + 2);
        low_sorted_mus_high.push(low);
        low_sorted_mus_high.extend(&sorted_mus);
        low_sorted_mus_high.push(high);
        let l = low_sorted_mus_high.len();
        let mut all = vec![0.0; l];
        for i in 0..l - 2 {
            all[i + 1] = go_max(
                low_sorted_mus_high[i + 1] - low_sorted_mus_high[i],
                low_sorted_mus_high[i + 2] - low_sorted_mus_high[i + 1],
            );
        }
        if !consider_endpoints && low_sorted_mus_high.len() > 2 {
            all[1] = low_sorted_mus_high[2] - low_sorted_mus_high[1];
            all[l - 2] = low_sorted_mus_high[l - 2] - low_sorted_mus_high[l - 3];
        }
        sigma = all[1..l - 1].to_vec();
    }

    // The weights, with the prior's weight where the prior sits.
    let unsorted_weights = weights_func(mus.len());
    let mut sorted_weights = if consider_prior {
        let mut w = Vec::with_capacity(sorted_mus.len());
        w.extend(choice(&unsorted_weights, &order[..prior_pos]));
        w.push(prior_weight);
        w.extend(choice(&unsorted_weights, &order[prior_pos..]));
        w
    } else {
        choice(&unsorted_weights, &order)
    };
    let sum_sorted_weights = sum(&sorted_weights);
    for w in &mut sorted_weights {
        *w /= sum_sorted_weights;
    }

    // Clip the sigmas to the range, and to a floor that shrinks as the count grows.
    let max_sigma = 1.0 * (high - low);
    let min_sigma = if consider_magic_clip {
        1.0 * (high - low) / go_min(100.0, 1.0 + sorted_mus.len() as f64)
    } else {
        EPS
    };
    clip(&mut sigma, min_sigma, max_sigma);
    if consider_prior {
        sigma[prior_pos] = prior_sigma;
    }
    (sorted_weights, sorted_mus, sigma)
}

/// goptuna's `choice`: the elements of `array` at `idxs`.
pub(super) fn choice(array: &[f64], idxs: &[usize]) -> Vec<f64> {
    idxs.iter().map(|&idx| array[idx]).collect()
}

/// goptuna's `location`: the first index of the sorted `array` whose element
/// is not less than `key`.
pub(super) fn location(array: &[f64], key: f64) -> usize {
    let mut i = 0;
    let mut size = array.len();
    loop {
        let mid = (i + size) / 2;
        if i == size {
            break;
        }
        if array[mid] < key {
            i = mid + 1;
        } else {
            size = mid;
        }
    }
    i
}

/// goptuna's `clip`, which clamps each element into `[min, max]`.
pub(super) fn clip(array: &mut [f64], min: f64, max: f64) {
    for x in array {
        if *x < min {
            *x = min;
        } else if *x > max {
            *x = max;
        }
    }
}

/// gonum's `floats.Sum`, sequentially. gonum's amd64 routine is SIMD, so its
/// last bits can differ.
pub(super) fn sum(s: &[f64]) -> f64 {
    s.iter().fold(0.0, |acc, &v| acc + v)
}

#[cfg(test)]
mod tests {
    // Ports: tpe/parzen_estimator_test.go, TestNewParzenEstimator @ c-bata/goptuna v0.9.0 (the value cases; the shape cases are below).

    use super::*;

    fn params(
        consider_prior: bool,
        consider_magic_clip: bool,
        consider_endpoints: bool,
    ) -> ParzenEstimatorParams {
        ParzenEstimatorParams {
            consider_prior,
            consider_magic_clip,
            consider_endpoints,
            weights: super::super::sampler::default_weights,
            prior_weight: 1.0,
        }
    }

    #[test]
    fn new_parzen_estimator_matches_goptuna_cases() {
        // (mus, prior, expected weights, mus, sigmas), with low -1, high 1,
        // magic clip off and endpoints on, as goptuna's test table has them.
        // The mus, whether the prior is considered, then the expected weights, mus and sigmas.
        type Case = (Vec<f64>, bool, Vec<f64>, Vec<f64>, Vec<f64>);
        let cases: Vec<Case> = vec![
            (vec![], false, vec![], vec![], vec![]),
            (vec![], true, vec![1.0], vec![0.0], vec![2.0]),
            (
                vec![0.4],
                true,
                vec![0.5, 0.5],
                vec![0.0, 0.4],
                vec![2.0, 0.6],
            ),
            (
                vec![-0.4],
                true,
                vec![0.5, 0.5],
                vec![-0.4, 0.0],
                vec![0.6, 2.0],
            ),
            (
                vec![-0.4, 0.4],
                true,
                vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
                vec![-0.4, 0.0, 0.4],
                vec![0.6, 2.0, 0.6],
            ),
        ];
        for (mut mus, prior, want_weights, want_mus, want_sigmas) in cases {
            let p = params(prior, false, true);
            let got = ParzenEstimator::new(&mut mus, -1.0, 1.0, &p);
            assert_eq!(got.weights, want_weights);
            assert_eq!(got.mus, want_mus);
            assert_eq!(got.sigmas, want_sigmas);
        }
    }

    #[test]
    fn argsort_sorts_its_input_in_place() {
        // gonum's floats.Argsort: the input ends sorted, and the indices say where
        // each sorted value was.
        let mut values = vec![3.0, 1.0, 2.0];
        let inds = argsort(&mut values);
        assert_eq!(values, vec![1.0, 2.0, 3.0]);
        assert_eq!(inds, vec![1, 2, 0]);
    }

    #[test]
    fn location_is_a_lower_bound() {
        let array = [1.0, 2.0, 2.0, 5.0];
        assert_eq!(location(&array, 0.0), 0);
        assert_eq!(location(&array, 2.0), 1);
        assert_eq!(location(&array, 3.0), 3);
        assert_eq!(location(&array, 9.0), 4);
        assert_eq!(location(&[], 1.0), 0);
    }

    #[test]
    fn clip_clamps_into_range() {
        let mut a = vec![-2.0, 0.5, 9.0];
        clip(&mut a, 0.0, 1.0);
        assert_eq!(a, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn new_parzen_estimator_has_one_weight_per_mu_and_the_prior() {
        // goptuna's TestNewParzenEstimatorShapeCheck: one weight per mu, plus
        // the prior when it is considered.
        let cases: [(Vec<f64>, bool, bool, bool); 9] = [
            (vec![], true, true, true),
            (vec![], false, true, true),
            (vec![], true, false, true),
            (vec![], true, true, false),
            (vec![0.4], true, true, true),
            (vec![0.4], false, true, true),
            (vec![0.4], true, false, true),
            (vec![0.4], true, true, false),
            (vec![-0.4, 0.4], true, true, true),
        ];
        for (mus, consider_prior, consider_magic_clip, consider_endpoints) in cases {
            let p = params(consider_prior, consider_magic_clip, consider_endpoints);
            let estimator = ParzenEstimator::new(&mut mus.clone(), -1.0, 1.0, &p);
            let expected = mus.len() + usize::from(consider_prior);
            assert_eq!(
                estimator.weights.len(),
                expected,
                "mus {mus:?}, prior {consider_prior}"
            );
        }
    }
}
