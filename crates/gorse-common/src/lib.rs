// Ports: common/** (ann, blas, copier, datautil, decision, encoding, event, expression, floats, heap, jsonutil, log, mock, monitor, nn, parallel, reranker, sizeof, util) @ add21b5

//! Shared utilities: float kernels, parallelism, encodings, the neural-network
//! layer used by the FM model, logging and monitoring.
//!
//! Each module ports one Go package under `common/` and names its Go sources in
//! its header.

pub mod ann;
pub mod blas;
pub mod consistent;
pub mod copier;
pub mod datautil;
pub mod dateparse;
pub mod decision;
pub mod encoding;
pub mod event;
pub mod expression;
pub mod floats;
pub mod gocontext;
pub mod gojson;
pub mod gonet;
pub mod gosort;
pub mod gostrconv;
pub mod gostrings;
pub mod gotime;
pub mod gounicode;
pub mod gourl;
pub mod goutf8;
pub mod heap;
pub(crate) mod http;
pub mod jsonutil;
pub mod log;
pub mod math32;
pub mod mock;
pub mod monitor;
pub mod mysql_dsn;
pub mod nn;
pub mod parallel;
pub mod reranker;
pub mod sizeof;
pub mod util;
