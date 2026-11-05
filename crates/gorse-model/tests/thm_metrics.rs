//! G8, the proptest mirrors of the T1 theorems (porting/verification-plan.md, G8).
//!
//! Each property has the name of the Lean theorem it mirrors, with a `thm_` prefix,
//! and checks the Rust metric on the inputs that theorem quantifies over. A
//! property is a sample of its theorem, not a proof: it runs [`CASES`] random
//! cases, and the theorem in `proofs/Gorse/Spec/Metrics.lean` covers all of them.
//!
//! The Rust metrics return `f32`. The theorems are over the reals, so each check
//! uses the same operations in the same order as the Rust code. Where a check
//! compares with a bound, the bound is one the theorem gives, not a tolerance.

use std::collections::HashSet;

use gorse_model::cf::evaluator::{ndcg, precision, recall};
use proptest::prelude::*;

/// The number of random cases per property (G8: at least 10⁵).
const CASES: u32 = 100_000;

/// The items are drawn from `0..ITEMS`, so targets and lists overlap often.
const ITEMS: i32 = 40;

/// The longest target set and list.
const MAX_LEN: usize = 15;

/// The metrics return `f32` before D-324 and `Option<f32>` after it. This converts
/// either form, so the mirrors compile on both sides of that change.
trait Metric {
    fn defined(self) -> Option<f32>;
}

impl Metric for f32 {
    fn defined(self) -> Option<f32> {
        Some(self)
    }
}

impl Metric for Option<f32> {
    fn defined(self) -> Option<f32> {
        self
    }
}

/// The value of a metric on an input where the theorem says it is defined.
fn value(metric: impl Metric) -> f32 {
    metric
        .defined()
        .expect("the metric is defined on this input")
}

/// A non-empty target set.
fn target() -> impl Strategy<Value = HashSet<i32>> {
    prop::collection::hash_set(0..ITEMS, 1..=MAX_LEN)
}

/// A non-empty list without duplicates, in random order.
fn nodup_list() -> impl Strategy<Value = Vec<i32>> {
    prop::collection::btree_set(0..ITEMS, 1..=MAX_LEN)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>())
        .prop_shuffle()
}

/// A non-empty list that may repeat items.
fn list() -> impl Strategy<Value = Vec<i32>> {
    prop::collection::vec(0..ITEMS, 1..=MAX_LEN)
}

/// A non-empty target set and a list without duplicates, for `ndcg_eq_one_iff`.
/// Half the cases are free draws, which rarely have the target hold the list's
/// prefix. The other half build the target from a prefix of the list, so both
/// sides of the equivalence are exercised.
fn equality_case() -> impl Strategy<Value = (HashSet<i32>, Vec<i32>)> {
    prop_oneof![
        (target(), nodup_list()),
        (
            nodup_list(),
            0..=MAX_LEN,
            prop::collection::btree_set(0..ITEMS, 0..=5),
        )
            .prop_map(|(l, k, extra)| {
                let mut t: HashSet<i32> = l[..k.min(l.len())].iter().copied().collect();
                t.extend(extra);
                if t.is_empty() {
                    t.insert(l[0]);
                }
                (t, l)
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    /// `hits_le_card`: with no duplicates in the list, the hits are at most |T|.
    /// Rust has no `hits`, so the count is recovered from recall, which is
    /// hits / |T|, and checked against the Lean definition's count.
    #[test]
    fn thm_hits_le_card(t in target(), l in nodup_list()) {
        let direct = l.iter().filter(|item| t.contains(item)).count();
        let recovered = (value(recall(&t, &l)) * t.len() as f32).round() as usize;
        prop_assert_eq!(recovered, direct);
        prop_assert!(direct <= t.len());
    }

    /// `precision_le_one`: precision is at most 1, with or without duplicates.
    #[test]
    fn thm_precision_le_one(t in target(), l in list()) {
        prop_assert!(value(precision(&t, &l)) <= 1.0);
    }

    /// `recall_le_one`: with no duplicates in the list, recall is at most 1.
    #[test]
    fn thm_recall_le_one(t in target(), l in nodup_list()) {
        prop_assert!(value(recall(&t, &l)) <= 1.0);
    }

    /// `ndcg_gt_one_of_dup`: a list with a repeated hit has NDCG above 1. The
    /// theorem gives one witness, the target `{1}` and the list `[1, 1]`. This
    /// checks the same shape for every item.
    #[test]
    fn thm_ndcg_gt_one_of_dup(x in any::<i32>()) {
        let t = HashSet::from([x]);
        let l = [x, x];
        prop_assert!(value(ndcg(&t, &l)) > 1.0);
    }

    /// `ndcg_mem_unitInterval`: with a non-empty target set and a non-empty list
    /// without duplicates, NDCG is defined and lies in [0, 1].
    // The name keeps the Lean theorem's camel case, as G8 asks.
    #[allow(non_snake_case)]
    #[test]
    fn thm_ndcg_mem_unitInterval(t in target(), l in nodup_list()) {
        let v = value(ndcg(&t, &l));
        prop_assert!((0.0..=1.0).contains(&v), "NDCG {v} is outside [0, 1]");
    }

    /// `ndcg_eq_one_iff`: NDCG is exactly 1 when each of the first min(|T|, |L|)
    /// entries of the list is in the target set, and only then.
    #[test]
    fn thm_ndcg_eq_one_iff((t, l) in equality_case()) {
        let prefix_in_target =
            (0..t.len().min(l.len())).all(|i| t.contains(&l[i]));
        prop_assert_eq!(value(ndcg(&t, &l)) == 1.0, prefix_in_target);
    }
}
