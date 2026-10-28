// Ports: model/cf/evaluator.go @ add21b5

//! Top-n evaluation of a fitted model: the ranking metrics, [`rank`] and
//! [`evaluate`]. The tests are the Go tests of `evaluator_test.go`.

use std::collections::HashSet;
use std::sync::Mutex;

use gorse_common::floats;
use gorse_common::gocontext::{Context, ContextError};
use gorse_common::heap::TopKFilter;
use gorse_common::parallel;
use gorse_dataset::dataset::CfSplit;

use super::lock;
use super::model::MatrixFactorization;

/// Go's `Metric`: scores a ranked list against the set of relevant items.
pub type Metric = fn(&HashSet<i32>, &[i32]) -> f32;

/// The per-job partial results of [`evaluate`]: the sum of each metric and the
/// number of users scored.
struct Partial {
    sum: Vec<f32>,
    count: f32,
}

/// Go's `Evaluate`: the mean of each metric over the test users who have
/// relevant items. Each user is ranked over its test items and `num_candidates`
/// sampled negatives, and the top `top_k` are scored.
///
/// The negatives are sampled once per split and cached, so every call on the
/// same split sees the same ones, as in Go.
pub fn evaluate(
    estimator: &dyn MatrixFactorization,
    test_set: &mut dyn CfSplit,
    train_set: &dyn CfSplit,
    top_k: usize,
    num_candidates: usize,
    n_jobs: usize,
    scorers: &[Metric],
) -> Vec<f32> {
    let negatives = test_set
        .sample_user_negatives(train_set, num_candidates)
        .to_vec();
    let num_users = test_set.count_users();
    let feedback = test_set.get_user_feedback();
    let partials: Vec<Mutex<Partial>> = (0..n_jobs)
        .map(|_| {
            Mutex::new(Partial {
                sum: vec![0.0; scorers.len()],
                count: 0.0,
            })
        })
        .collect();
    // Go ignores the error: a background context is never done.
    let _ = parallel::parallel::<ContextError, _>(
        &Context::background(),
        num_users,
        n_jobs,
        |worker_id, user_index| {
            // Find top-n ItemFeedback in test set
            let target_set: HashSet<i32> = feedback[user_index].iter().copied().collect();
            if !target_set.is_empty() {
                // Sample negative samples
                let negative_sample = &negatives[user_index];
                let mut candidates = Vec::with_capacity(target_set.len() + negative_sample.len());
                candidates.extend_from_slice(&feedback[user_index]);
                candidates.extend_from_slice(negative_sample);
                // Find top-n ItemFeedback in predictions
                let rank_list = rank(estimator, user_index as i32, &candidates, top_k);
                let mut partial = lock(&partials[worker_id]);
                partial.count += 1.0;
                for (sum, metric) in partial.sum.iter_mut().zip(scorers) {
                    *sum += metric(&target_set, &rank_list);
                }
            }
            Ok(())
        },
    );
    let mut sum = vec![0f32; scorers.len()];
    let mut count = 0f32;
    for partial in partials {
        let partial = partial.into_inner().unwrap_or_else(|e| e.into_inner());
        for (total, part) in sum.iter_mut().zip(&partial.sum) {
            *total += part;
        }
        count += partial.count;
    }
    floats::mul_const(&mut sum, 1.0 / count);
    sum
}

/// Go's `NDCG`: normalized discounted cumulative gain.
pub fn ndcg(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    // IDCG = \sum^{|REL|}_{i=1} \frac {1} {\log_2(i+1)}
    let mut idcg = 0f32;
    for i in 0..target_set.len().min(rank_list.len()) {
        idcg += 1.0 / log2(i as f32 + 2.0);
    }
    // DCG = \sum^{N}_{i=1} \frac {2^{rel_i}-1} {\log_2(i+1)}
    let mut dcg = 0f32;
    for (i, item_id) in rank_list.iter().enumerate() {
        if target_set.contains(item_id) {
            dcg += 1.0 / log2(i as f32 + 2.0);
        }
    }
    dcg / idcg
}

/// Go's `math32.Log2`, which is `float32(math.Log2(float64(x)))`. The float64
/// logarithm here is the standard library's, not Go's `math.Log` (O-3 in
/// `PORTING_NOTES.md`); the rounding to float32 hides the difference.
fn log2(x: f32) -> f32 {
    f64::from(x).log2() as f32
}

/// Go's `Precision`: the fraction of recommended items that are relevant.
pub fn precision(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    let mut hit = 0f32;
    for item_id in rank_list {
        if target_set.contains(item_id) {
            hit += 1.0;
        }
    }
    hit / rank_list.len() as f32
}

/// Go's `Recall`: the fraction of relevant items that are recommended.
pub fn recall(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    let mut hit = 0usize;
    for item_id in rank_list {
        if target_set.contains(item_id) {
            hit += 1;
        }
    }
    hit as f32 / target_set.len() as f32
}

/// Go's `HR`: 1 when some relevant item is recommended, else 0.
pub fn hr(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    for item_id in rank_list {
        if target_set.contains(item_id) {
            return 1.0;
        }
    }
    0.0
}

/// Go's `MAP`: mean average precision.
/// mAP: <http://sdsawtelle.github.io/blog/output/mean-average-precision-MAP-for-recommender-systems.html>
pub fn map(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    let mut sum_precision = 0f32;
    let mut hit = 0usize;
    for (i, item_id) in rank_list.iter().enumerate() {
        if target_set.contains(item_id) {
            hit += 1;
            sum_precision += hit as f32 / (i + 1) as f32;
        }
    }
    sum_precision / target_set.len() as f32
}

/// Go's `MRR`: the reciprocal rank of the first relevant item, or 0 when none
/// is recommended.
pub fn mrr(target_set: &HashSet<i32>, rank_list: &[i32]) -> f32 {
    for (i, item_id) in rank_list.iter().enumerate() {
        if target_set.contains(item_id) {
            return 1.0 / (i + 1) as f32;
        }
    }
    0.0
}

/// Go's `Rank`: the `top_n` candidates the model scores highest, best first.
pub fn rank(
    model: &dyn MatrixFactorization,
    user_id: i32,
    candidates: &[i32],
    top_n: usize,
) -> Vec<i32> {
    // Get top-n list
    let mut items_heap = TopKFilter::new(top_n);
    for &item_id in candidates {
        items_heap.push(item_id, model.internal_predict(user_id, item_id));
    }
    items_heap.pop_all_values()
}

#[cfg(test)]
mod tests {
    // Ports: model/cf/evaluator_test.go @ add21b5

    use std::collections::HashSet;

    use gorse_common::gotime;
    use gorse_dataset::dataset::Dataset;
    use gorse_model_core::model::{BaseModel, Model as CoreModel};
    use gorse_model_core::params::Params;
    use gorse_storage::data::types as data;

    use super::*;
    use crate::cf::model::{Error, FitConfig, Model, Score};

    const EVAL_EPSILON: f32 = 0.00001;

    fn set(items: &[i32]) -> HashSet<i32> {
        items.iter().copied().collect()
    }

    // The literal is the Go test's own value, kept as written.
    #[allow(clippy::excessive_precision)]
    #[test]
    fn test_ndcg() {
        let target_set = set(&[1, 3, 5, 7]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.6766372989 - ndcg(&target_set, &rank_list)).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_precision() {
        let target_set = set(&[1, 3, 5, 7]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.4 - precision(&target_set, &rank_list)).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_recall() {
        let target_set = set(&[1, 3, 15, 17, 19]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.4 - recall(&target_set, &rank_list)).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_ap() {
        let target_set = set(&[1, 3, 7, 9]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.44375 - map(&target_set, &rank_list)).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_rr() {
        let target_set = set(&[3]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.25 - mrr(&target_set, &rank_list)).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_hr() {
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((1.0 - hr(&set(&[3]), &rank_list)).abs() <= EVAL_EPSILON);
        assert!((0.0 - hr(&set(&[30]), &rank_list)).abs() <= EVAL_EPSILON);
    }

    /// Go's `mockMatrixFactorizationForEval`: scores from fixed positive and
    /// negative sets, and panics on every method the evaluator does not call.
    #[derive(Default)]
    struct MockMatrixFactorizationForEval {
        base: BaseModel,
        positive: Vec<HashSet<i32>>,
        negative: Vec<HashSet<i32>>,
    }

    impl CoreModel for MockMatrixFactorizationForEval {
        fn set_params(&mut self, params: Params) {
            self.base.set_params(params);
        }

        fn get_params(&self) -> &Params {
            self.base.get_params()
        }

        fn clear(&mut self) {
            // do nothing
        }

        fn invalid(&self) -> bool {
            unimplemented!("implement me")
        }
    }

    impl Model for MockMatrixFactorizationForEval {
        fn fit(
            &mut self,
            _: &Context,
            _: &dyn CfSplit,
            _: &mut dyn CfSplit,
            _: &FitConfig,
        ) -> Score {
            unimplemented!("don't call me")
        }

        fn get_item_index(&self) -> Option<&gorse_dataset::dict::FreqDict> {
            unimplemented!("don't call me")
        }

        fn marshal(&self, _: &mut dyn std::io::Write) -> Result<(), Error> {
            unimplemented!("implement me")
        }

        fn unmarshal(&mut self, _: &mut dyn std::io::Read) -> Result<(), Error> {
            unimplemented!("implement me")
        }

        fn get_user_factor(&self, _: i32) -> &[f32] {
            unimplemented!("implement me")
        }

        fn get_item_factor(&self, _: i32) -> &[f32] {
            unimplemented!("implement me")
        }

        fn model_name(&self) -> &'static str {
            "mock"
        }
    }

    impl MatrixFactorization for MockMatrixFactorizationForEval {
        fn predict(&self, _: &str, _: &str) -> f32 {
            unimplemented!("don't call me")
        }

        fn internal_predict(&self, user_id: i32, item_id: i32) -> f32 {
            if self.positive[user_id as usize].contains(&item_id) {
                return 1.0;
            }
            if self.negative[user_id as usize].contains(&item_id) {
                return -1.0;
            }
            0.0
        }

        fn get_user_index(&self) -> Option<&gorse_dataset::dict::FreqDict> {
            unimplemented!("don't call me")
        }

        fn is_user_predictable(&self, _: i32) -> bool {
            unimplemented!("implement me")
        }

        fn is_item_predictable(&self, _: i32) -> bool {
            unimplemented!("implement me")
        }
    }

    #[test]
    fn test_evaluate() {
        // create dataset
        let mut train = Dataset::new(gotime::now(), 0, 0);
        let mut test = Dataset::new(gotime::now(), 0, 0);
        for i in 0..4 {
            train.add_user(data::User {
                user_id: i.to_string(),
                ..Default::default()
            });
            test.add_user(data::User {
                user_id: (i / 4).to_string(),
                ..Default::default()
            });
        }
        for i in 0..16 {
            test.add_item(data::Item {
                item_id: i.to_string(),
                ..Default::default()
            });
            test.add_feedback(
                &(i / 4).to_string(),
                &i.to_string(),
                gotime::Time::default(),
            );
        }
        assert_eq!(16, test.count_feedback());
        assert_eq!(4, test.count_users());
        assert_eq!(16, test.count_items());
        // create model
        let m = MockMatrixFactorizationForEval {
            positive: vec![
                set(&[0, 1, 2, 3]),
                set(&[4, 5, 6]),
                set(&[8, 9]),
                set(&[12]),
            ],
            negative: vec![set(&[]), set(&[7]), set(&[10, 11]), set(&[13, 14, 15])],
            ..Default::default()
        };
        // evaluate model
        let num_candidates = test.count_items();
        let s = evaluate(&m, &mut test, &train, 4, num_candidates, 4, &[precision]);
        assert_eq!(1, s.len());
        assert_eq!(0.625, s[0]);
    }
}
