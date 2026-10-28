//! `gosort` against `tests/data/sort_golden.json`, which
//! `contracts/tools/sortgolden` wrote by running go1.27.0's `sort.Sort` on
//! slices of indexes, compared by their keys, in both directions. Sort is not
//! stable, so the golden fixes the order of equal keys as Go leaves it.
//!
//! Each case replays the same comparisons on the Rust side and must produce
//! the same final order.

mod common;

use gorse_common::gosort::{Interface, sort};
use serde::Deserialize;

use common::load_golden;

#[derive(Deserialize)]
struct Golden {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    descending: bool,
    keys: Vec<i64>,
    order: Vec<usize>,
}

/// Indexes into `keys`, compared by their keys.
struct ByKey<'a> {
    idx: Vec<usize>,
    keys: &'a [i64],
    descending: bool,
}

impl Interface for ByKey<'_> {
    fn len(&self) -> usize {
        self.idx.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        let (x, y) = (self.keys[self.idx[i]], self.keys[self.idx[j]]);
        if self.descending { x > y } else { x < y }
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.idx.swap(i, j);
    }
}

#[test]
fn sort_matches_go_on_every_case() {
    let golden: Golden = load_golden("sort_golden.json");
    assert!(
        golden.cases.len() >= 1000,
        "the golden has {} cases",
        golden.cases.len()
    );
    let mut failures = Vec::new();
    for case in &golden.cases {
        let mut data = ByKey {
            idx: (0..case.keys.len()).collect(),
            keys: &case.keys,
            descending: case.descending,
        };
        sort(&mut data);
        if data.idx != case.order {
            failures.push(case.name.clone());
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Go, first: {:?}",
        failures.len(),
        golden.cases.len(),
        failures.iter().take(5).collect::<Vec<_>>()
    );
}
