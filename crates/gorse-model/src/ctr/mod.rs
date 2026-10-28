// Ports: model/ctr/*.go @ add21b5, except fm_xla.go and model.py (D-8, D-196)

//! Click-through-rate models (Go `model/ctr`): the datasets of sparse features,
//! their scalers, the factorization machine with attention (`fm.go`) and its
//! evaluation, and the hyper-parameter search in [`optimize`] on [`crate::tpe`].

pub mod data;
pub mod evaluator;
pub mod fm;
pub mod model;
pub mod optimize;
pub mod transformer;

#[cfg(test)]
mod replay;

#[cfg(test)]
use gorse_dataset::unified_index::{UnifiedDirectIndex, UnifiedIndex};

/// An empty dataset with a direct unified index of size 0, as Go's
/// `NewMapIndexDataset` makes it. It is the helper of `optimize_test.go`,
/// shared with the tests of this package.
#[cfg(test)]
pub(crate) fn new_map_index_dataset() -> data::Dataset {
    data::Dataset {
        index: Some(UnifiedIndex::Direct(UnifiedDirectIndex::new(0))),
        ..data::Dataset::default()
    }
}
