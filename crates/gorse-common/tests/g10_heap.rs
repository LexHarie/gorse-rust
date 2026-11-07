//! G10 (P3-5): the tests that catch the mutants of `TopKFilter` in `heap.rs` which the unit
//! tests in that file leave alive.
//!
//! `scripts/check-mutants-topk.sh` runs these with the `heap` lib tests, so each test name
//! carries `heap`. The tests use only the public API of `TopKFilter`, and `heap.rs` is not
//! changed for them.

use gorse_common::heap::TopKFilter;

/// `is_empty` follows the held elements: true for a new filter, false while one is held,
/// and true again once `pop_all` has emptied it.
#[test]
fn heap_top_k_filter_is_empty_follows_the_held_elements() {
    let mut filter: TopKFilter<u32, i64> = TopKFilter::new(2);
    assert!(filter.is_empty());
    assert_eq!(filter.len(), 0);

    filter.push(7, 3);
    assert!(!filter.is_empty());
    assert_eq!(filter.len(), 1);

    assert_eq!(filter.pop_all().len(), 1);
    assert!(filter.is_empty());
}
