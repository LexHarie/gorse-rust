// Ports: model/cf/evaluator.go @ add21b5

//! Top-n evaluation of a fitted model: the ranking metrics, [`rank`] and
//! [`evaluate`]. The tests are the Go tests of `evaluator_test.go`.

use std::collections::HashSet;
use std::sync::Mutex;

use gorse_common::gocontext::{Context, ContextError};
use gorse_common::heap::TopKFilter;
use gorse_common::parallel;
use gorse_dataset::dataset::CfSplit;

use super::lock;
use super::model::MatrixFactorization;

/// Go's `Metric`: scores a ranked list against the set of relevant items. `None` when
/// the metric is undefined for the list, as A1 requires (D-315).
pub type Metric = fn(&HashSet<i32>, &[i32]) -> Option<f32>;

/// The per-job partial results of [`evaluate`]: for each metric, the sum of its defined
/// values and the number of users it was defined for.
struct Partial {
    sum: Vec<f32>,
    count: Vec<f32>,
}

/// Go's `Evaluate`: the mean of each metric over the test users for whom it is defined,
/// and `None` when no user has a defined value (A1, D-324). Each user is ranked over its
/// test items and `num_candidates` sampled negatives, and the top `top_k` are scored.
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
) -> Vec<Option<f32>> {
    // A1 config check: a zero cut-off leaves every ranked list empty, so it is refused here.
    assert!(top_k >= 1, "top_k must be at least 1, got {top_k}");
    let negatives = test_set
        .sample_user_negatives(train_set, num_candidates)
        .to_vec();
    let num_users = test_set.count_users();
    let feedback = test_set.get_user_feedback();
    let partials: Vec<Mutex<Partial>> = (0..n_jobs)
        .map(|_| {
            Mutex::new(Partial {
                sum: vec![0.0; scorers.len()],
                count: vec![0.0; scorers.len()],
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
                let candidates = candidate_list(&feedback[user_index], &negatives[user_index]);
                // Find top-n ItemFeedback in predictions
                let rank_list = rank(estimator, user_index as i32, &candidates, top_k);
                let mut partial = lock(&partials[worker_id]);
                let Partial { sum, count } = &mut *partial;
                for ((total, n), metric) in sum.iter_mut().zip(count.iter_mut()).zip(scorers) {
                    if let Some(value) = metric(&target_set, &rank_list) {
                        *total += value;
                        *n += 1.0;
                    }
                }
            }
            Ok(())
        },
    );
    let mut sum = vec![0f32; scorers.len()];
    let mut count = vec![0f32; scorers.len()];
    for partial in partials {
        let partial = partial.into_inner().unwrap_or_else(|e| e.into_inner());
        for (total, part) in sum.iter_mut().zip(&partial.sum) {
            *total += part;
        }
        for (n, part) in count.iter_mut().zip(&partial.count) {
            *n += part;
        }
    }
    sum.iter()
        .zip(&count)
        .map(|(&total, &n)| (n > 0.0).then(|| total * (1.0 / n)))
        .collect()
}

/// Go's `NDCG`: normalized discounted cumulative gain. Undefined (`None`) when the target
/// set or the ranked list is empty, as in the T1 specification (D-317, D-318).
pub fn ndcg(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if target_set.is_empty() || rank_list.is_empty() {
        return None;
    }
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
    Some(dcg / idcg)
}

/// The binary32 values of log2 k for k = 2..=65, as bit patterns, entry k - 2. Each entry is the
/// single rounding of the exact logarithm to binary32, which equals `f32(f64::log2(k))` (D-354).
/// The Lean lemmas of Gorse.Spec.Log2Table prove the entries, and the G9 check of `oracle_log2`
/// compares them bit for bit.
pub const LOG2_TABLE: [u32; 64] = [
    0x3F800000, 0x3FCAE00D, 0x40000000, 0x40149A78, 0x40257007, 0x4033ABB4, 0x40400000, 0x404AE00D,
    0x40549A78, 0x405D6754, 0x40657007, 0x406CD401, 0x4073ABB4, 0x407A0A7F, 0x40800000, 0x4082CC7F,
    0x40857007, 0x4087EF06, 0x408A4D3C, 0x408C8DDD, 0x408EB3AA, 0x4090C105, 0x4092B803, 0x40949A78,
    0x40966A01, 0x4098280A, 0x4099D5DA, 0x409B7495, 0x409D053F, 0x409E88C7, 0x40A00000, 0x40A16BAD,
    0x40A2CC7F, 0x40A42316, 0x40A57007, 0x40A6B3D8, 0x40A7EF06, 0x40A92204, 0x40AA4D3C, 0x40AB7111,
    0x40AC8DDD, 0x40ADA3F6, 0x40AEB3AA, 0x40AFBD43, 0x40B0C105, 0x40B1BF31, 0x40B2B803, 0x40B3ABB4,
    0x40B49A78, 0x40B58482, 0x40B66A01, 0x40B74B20, 0x40B8280A, 0x40B900E6, 0x40B9D5DA, 0x40BAA709,
    0x40BB7495, 0x40BC3E9D, 0x40BD053F, 0x40BDC89A, 0x40BE88C7, 0x40BF45E1, 0x40C00000, 0x40C0B73D,
];

/// Go's `math32.Log2`, which is `float32(math.Log2(float64(x)))`. For the integers 2 to 65, which
/// are the only arguments `ndcg` passes for rank lists of up to 64 entries, the value is read from
/// `LOG2_TABLE`; it is the same value as the float64 logarithm rounded, checked by a unit test. Other
/// arguments use the standard library's float64 logarithm, not Go's `math.Log` (O-3 in
/// `PORTING_NOTES.md`); the rounding to float32 hides the difference.
fn log2(x: f32) -> f32 {
    if x.fract() == 0.0 && (2.0..=65.0).contains(&x) {
        return f32::from_bits(LOG2_TABLE[x as usize - 2]);
    }
    f64::from(x).log2() as f32
}

#[cfg(test)]
mod log2_table_tests {
    use super::{LOG2_TABLE, log2};

    /// The table holds `f32(f64::log2(k))` for every k in 2..=65 (D-354), and `log2` reads it.
    #[test]
    fn test_log2_table_matches_float64_logarithm() {
        for k in 2..=65u32 {
            let libm = (k as f64).log2() as f32;
            assert_eq!(
                f32::from_bits(LOG2_TABLE[k as usize - 2]).to_bits(),
                libm.to_bits(),
                "table entry for k = {k}"
            );
            assert_eq!(log2(k as f32).to_bits(), libm.to_bits(), "log2({k})");
        }
    }

    /// Arguments outside the table take the float64 logarithm. The guard of `log2` must send a
    /// non-integer in range, and an integer outside it, to that path: a guard that reads `||` for
    /// `&&`, or `!=` for `==`, reads the table for one of these and fails here.
    #[test]
    fn test_log2_outside_table_uses_float64_logarithm() {
        for x in [1.0f32, 1.5, 2.5, 3.7, 65.5, 66.0, 100.0] {
            let expected = (x as f64).log2() as f32;
            assert_eq!(log2(x).to_bits(), expected.to_bits(), "log2({x})");
        }
    }
}

/// Go's `Precision`, written `P@|L|` (D-315, A3): the fraction of recommended items that
/// are relevant. Undefined (`None`) for an empty ranked list.
pub fn precision(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if rank_list.is_empty() {
        return None;
    }
    let mut hit = 0f32;
    for item_id in rank_list {
        if target_set.contains(item_id) {
            hit += 1.0;
        }
    }
    Some(hit / rank_list.len() as f32)
}

/// Go's `Recall`: the fraction of relevant items that are recommended. Undefined (`None`)
/// for an empty target set.
pub fn recall(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if target_set.is_empty() {
        return None;
    }
    let mut hit = 0usize;
    for item_id in rank_list {
        if target_set.contains(item_id) {
            hit += 1;
        }
    }
    Some(hit as f32 / target_set.len() as f32)
}

/// Go's `HR`: 1 when some relevant item is recommended, else 0. Undefined (`None`) for an
/// empty ranked list.
pub fn hr(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if rank_list.is_empty() {
        return None;
    }
    for item_id in rank_list {
        if target_set.contains(item_id) {
            return Some(1.0);
        }
    }
    Some(0.0)
}

/// Go's `MAP`: mean average precision, which divides by `|T|` (the `AP/|T|` of D-315, A3).
/// Undefined (`None`) when the target set or the ranked list is empty.
/// mAP: <http://sdsawtelle.github.io/blog/output/mean-average-precision-MAP-for-recommender-systems.html>
pub fn map(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if target_set.is_empty() || rank_list.is_empty() {
        return None;
    }
    let mut sum_precision = 0f32;
    let mut hit = 0usize;
    for (i, item_id) in rank_list.iter().enumerate() {
        if target_set.contains(item_id) {
            hit += 1;
            sum_precision += hit as f32 / (i + 1) as f32;
        }
    }
    Some(sum_precision / target_set.len() as f32)
}

/// Go's `MRR`: the reciprocal rank of the first relevant item, or 0 when none
/// is recommended. Undefined (`None`) for an empty target set or an empty ranked list (A1).
pub fn mrr(target_set: &HashSet<i32>, rank_list: &[i32]) -> Option<f32> {
    if target_set.is_empty() || rank_list.is_empty() {
        return None;
    }
    for (i, item_id) in rank_list.iter().enumerate() {
        if target_set.contains(item_id) {
            return Some(1.0 / (i + 1) as f32);
        }
    }
    Some(0.0)
}

/// The candidates of one user: the feedback and the sampled negatives, each item once, in
/// first-seen order. A repeated item would enter the ranked list twice and could lift NDCG
/// above 1 (A4, D-315). Feedback repeats when a user has several rows for one item.
fn candidate_list(feedback: &[i32], negatives: &[i32]) -> Vec<i32> {
    let mut seen = HashSet::with_capacity(feedback.len() + negatives.len());
    feedback
        .iter()
        .chain(negatives)
        .copied()
        .filter(|item| seen.insert(*item))
        .collect()
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
    use gorse_model_core::model::{BaseModel, Model as CoreModel, SuggestTrial};
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
        assert!((0.6766372989 - ndcg(&target_set, &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_precision() {
        let target_set = set(&[1, 3, 5, 7]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.4 - precision(&target_set, &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_recall() {
        let target_set = set(&[1, 3, 15, 17, 19]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.4 - recall(&target_set, &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_ap() {
        let target_set = set(&[1, 3, 7, 9]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.44375 - map(&target_set, &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_rr() {
        let target_set = set(&[3]);
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((0.25 - mrr(&target_set, &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    #[test]
    fn test_hr() {
        let rank_list: Vec<i32> = (0..10).collect();
        assert!((1.0 - hr(&set(&[3]), &rank_list).unwrap()).abs() <= EVAL_EPSILON);
        assert!((0.0 - hr(&set(&[30]), &rank_list).unwrap()).abs() <= EVAL_EPSILON);
    }

    /// A1 (D-315): each metric is undefined, not NaN, on an empty target set or list.
    #[test]
    fn test_metrics_are_none_when_undefined() {
        let rank_list: Vec<i32> = (0..10).collect();
        assert_eq!(None, ndcg(&set(&[]), &rank_list));
        assert_eq!(None, ndcg(&set(&[1]), &[]));
        assert_eq!(None, precision(&set(&[1]), &[]));
        assert_eq!(None, recall(&set(&[]), &rank_list));
        assert_eq!(None, map(&set(&[]), &rank_list));
        assert_eq!(None, map(&set(&[1]), &[]));
        assert_eq!(None, mrr(&set(&[1]), &[]));
        assert_eq!(None, mrr(&set(&[]), &rank_list));
        assert_eq!(None, hr(&set(&[1]), &[]));
    }

    /// A1 (D-324): precision is defined on an empty target set, recall on an empty
    /// ranked list, and hr on an empty target set. Each is `Some(0)` there, because
    /// nothing in the ranked list is relevant.
    #[test]
    fn test_metrics_are_zero_where_defined_on_empty_target() {
        let rank_list: Vec<i32> = (0..10).collect();
        assert_eq!(Some(0.0), precision(&set(&[]), &rank_list));
        assert_eq!(Some(0.0), recall(&set(&[1]), &[]));
        assert_eq!(Some(0.0), hr(&set(&[]), &rank_list));
    }

    /// A1 config check (D-324): `evaluate` refuses a `top_k` of 0.
    #[test]
    #[should_panic(expected = "top_k must be at least 1, got 0")]
    fn test_evaluate_rejects_zero_top_k() {
        let train = Dataset::new(gotime::now(), 0, 0);
        let mut test = Dataset::new(gotime::now(), 0, 0);
        let m = MockMatrixFactorizationForEval::default();
        evaluate(&m, &mut test, &train, 0, 1, 1, &[precision]);
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

        fn suggest_params(&self, _trial: &mut dyn SuggestTrial) -> Params {
            unimplemented!("implement me")
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
        assert_eq!(Some(0.625), s[0]);
    }

    /// A4 (D-315), confirmed: a repeated candidate enters the ranked list twice, and NDCG
    /// then exceeds 1, as `ndcg_gt_one_of_dup` in proofs/Gorse/Spec/Metrics.lean states.
    #[test]
    fn test_ndcg_exceeds_one_on_repeated_candidate() {
        let m = MockMatrixFactorizationForEval {
            positive: vec![set(&[0])],
            negative: vec![set(&[])],
            ..Default::default()
        };
        let rank_list = rank(&m, 0, &[0, 0], 2);
        assert_eq!(vec![0, 0], rank_list);
        assert!(ndcg(&set(&[0]), &rank_list).unwrap() > 1.0);
    }

    /// A4 (D-315), the fix: the candidates are deduplicated before ranking, so NDCG of
    /// the same user stays at most 1.
    #[test]
    fn test_candidate_list_drops_repeats() {
        assert_eq!(vec![0, 1, 2], candidate_list(&[0, 0, 1], &[1, 2]));
        assert_eq!(Vec::<i32>::new(), candidate_list(&[], &[]));
    }

    #[test]
    fn test_ndcg_at_most_one_on_deduplicated_candidates() {
        let m = MockMatrixFactorizationForEval {
            positive: vec![set(&[0])],
            negative: vec![set(&[])],
            ..Default::default()
        };
        let candidates = candidate_list(&[0, 0], &[]);
        let rank_list = rank(&m, 0, &candidates, 2);
        assert_eq!(vec![0], rank_list);
        assert!(ndcg(&set(&[0]), &rank_list).unwrap() <= 1.0);
    }
}
