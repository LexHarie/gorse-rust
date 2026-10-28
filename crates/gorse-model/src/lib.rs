// Ports: model/cf/*.go, model/ctr/*.go @ add21b5

//! Collaborative-filtering (BPR, ALS) and click-through-rate (factorization machine) models.
//!
//! [`cf`] holds the collaborative-filtering models, and [`ctr`] the click-through-rate models.

pub mod cf;
pub mod ctr;
