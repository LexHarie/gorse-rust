// Ports: common/** (ann, blas, copier, datautil, decision, encoding, event, expression, floats, heap, jsonutil, log, mock, monitor, nn, parallel, reranker, sizeof, util) @ add21b5

//! Shared utilities: float kernels, parallelism, encodings, the neural-network
//! layer used by the FM model, logging and monitoring.
//!
//! Each module ports one Go package under `common/` and names its Go sources in
//! its header.

pub mod blas;
pub mod consistent;
pub mod dateparse;
pub mod encoding;
pub mod expression;
pub mod floats;
pub mod gostrconv;
pub mod gotime;
pub mod heap;
pub mod math32;
