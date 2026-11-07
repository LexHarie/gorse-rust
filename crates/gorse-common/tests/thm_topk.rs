//! G8, the proptest mirrors of the top-k theorems (porting/verification-plan.md, G8).
//!
//! Each property has the name of the Lean theorem it mirrors, with a `thm_` prefix, and
//! checks [`TopKFilter`] on the inputs that theorem quantifies over. A property is a sample
//! of its theorem, not a proof: it runs [`CASES`] random cases, and the theorem in
//! `proofs/Gorse/Spec/TopK.lean` (or `TopKModel.lean`) covers all of them.
//!
//! The order is D-329: a higher weight first and, between equal weights, the smaller id
//! first. The ids of a case are distinct, so that order is total and the result is one list.
//!
//! The names keep the Lean spelling (`isTopK_merge`), which is camelCase, so the snake-case
//! lint is allowed for this file.

#![allow(non_snake_case)]

use gorse_common::heap::TopKFilter;
use proptest::prelude::*;

/// The number of random cases per property (G8: at least 10⁵).
const CASES: u32 = 100_000;

/// The longest input. The ids are drawn from `0..ID_RANGE`, so the ids of two inputs
/// can be kept apart.
const MAX_LEN: usize = 40;
const ID_RANGE: u32 = 1_000;

/// The reference: the `k` best of `items` under D-329, best first.
fn reference_topk(items: &[(u32, i64)], k: usize) -> Vec<(u32, i64)> {
    let mut sorted = items.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.truncate(k);
    sorted
}

/// Pushes `items` into a `TopKFilter` of size `k`, then pops everything, best first.
fn run_filter(items: &[(u32, i64)], k: usize) -> Vec<(u32, i64)> {
    let mut filter = TopKFilter::new(k);
    for &(id, weight) in items {
        filter.push(id, weight);
    }
    filter
        .pop_all()
        .into_iter()
        .map(|e| (e.value, e.weight))
        .collect()
}

/// A weight: mostly small, so ties are common, and sometimes the extremes.
fn weight() -> impl Strategy<Value = i64> {
    prop_oneof![
        8 => -5i64..=5,
        1 => Just(i64::MIN),
        1 => Just(i64::MAX),
        2 => any::<i64>(),
    ]
}

/// Up to `MAX_LEN` items with distinct ids, in random order.
fn items() -> impl Strategy<Value = Vec<(u32, i64)>> {
    prop::collection::hash_set(0..ID_RANGE, 0..=MAX_LEN)
        .prop_flat_map(|ids| {
            let ids: Vec<u32> = ids.into_iter().collect();
            let n = ids.len();
            (Just(ids), prop::collection::vec(weight(), n))
        })
        .prop_map(|(ids, weights)| ids.into_iter().zip(weights).collect())
}

/// The items of one input: an id and a weight each.
type Input = Vec<(u32, i64)>;

/// Two inputs with disjoint ids: the ids of one are below `ID_RANGE / 2`, the other's above.
fn two_inputs() -> impl Strategy<Value = (Input, Input)> {
    let half = ID_RANGE / 2;
    (
        prop::collection::hash_set(0..half, 0..=MAX_LEN),
        prop::collection::hash_set(half..ID_RANGE, 0..=MAX_LEN),
    )
        .prop_flat_map(|(a, b)| {
            let a: Vec<u32> = a.into_iter().collect();
            let b: Vec<u32> = b.into_iter().collect();
            let (na, nb) = (a.len(), b.len());
            (
                Just(a),
                prop::collection::vec(weight(), na),
                Just(b),
                prop::collection::vec(weight(), nb),
            )
        })
        .prop_map(|(a, wa, b, wb)| {
            (
                a.into_iter().zip(wa).collect(),
                b.into_iter().zip(wb).collect(),
            )
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    /// `topKFilter_correct`: the filter returns the top `k` of its items, best first.
    #[test]
    fn thm_topKFilter_correct(items in items(), k in 0usize..=MAX_LEN + 2) {
        prop_assert_eq!(run_filter(&items, k), reference_topk(&items, k));
    }

    /// `isTopK_merge`: the top `k` of two inputs is the top `k` of the two top-`k` lists
    /// together, so a merge of partial results loses nothing.
    #[test]
    fn thm_isTopK_merge(pair in two_inputs(), k in 0usize..=MAX_LEN + 2) {
        let (xs, ys) = pair;
        let mut all = xs.clone();
        all.extend(ys.iter().copied());
        let mut partial = run_filter(&xs, k);
        partial.extend(run_filter(&ys, k));
        prop_assert_eq!(run_filter(&all, k), reference_topk(&partial, k));
    }

    /// `isTopK_weights_unique`: the weights of the top `k` do not depend on the order the
    /// items arrive in. Here the same items are pushed forwards and backwards.
    #[test]
    fn thm_isTopK_weights_unique(items in items(), k in 0usize..=MAX_LEN + 2) {
        let mut backwards = items.clone();
        backwards.reverse();
        let forwards: Vec<i64> = run_filter(&items, k).into_iter().map(|(_, w)| w).collect();
        let reversed: Vec<i64> = run_filter(&backwards, k).into_iter().map(|(_, w)| w).collect();
        prop_assert_eq!(forwards, reversed);
    }
}
