// Ports: model/cf/model.go, model/cf/evaluator.go @ add21b5

//! Collaborative filtering: Bayesian personalized ranking (BPR) and alternating
//! least squares (ALS) matrix factorization, the top-n evaluator, and the
//! marshalling of the models.
//!
//! The hyper-parameter tuning hooks (`SuggestParams`, Go's `goptuna`) are not
//! here: WP-3-3 ports them with the TPE sampler.

pub mod evaluator;
pub mod model;

#[cfg(test)]
mod replay;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `m`, recovering the guard when a worker panicked while holding it.
/// Go has no poisoning, so the value is used as it was left.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
