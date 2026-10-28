// Ports: model/model.go, model/params.go, model/built_in.go @ add21b5

//! Hyper-parameter maps, the base model type and built-in dataset metadata.
//!
//! This is the Go `model` root package. It is a separate crate because `dataset`
//! and `storage/meta` import it while `model/cf` and `model/ctr` import them.
//!
//! [`params`] holds `Params` and `ParamsGrid`, [`model`] the `Model` interface
//! and `BaseModel`, and [`built_in`] the built-in datasets and `~/.gorse`
//! directories.

pub mod built_in;
pub mod model;
pub mod params;
