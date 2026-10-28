// Ports: storage/data/*.go, storage/cache/*.go, storage/vectors/*.go, storage/blob/*.go, storage/meta/*.go @ add21b5

//! Ports of the Go `storage` packages.
//!
//! The blob stores are in [`blob`], the data store's record types are in
//! [`data::types`], the cache store's core is in [`cache`], and the vector
//! store's core is in [`vectors`], and the meta store is in [`meta`].

pub mod blob;
pub mod cache;
pub mod data;
pub mod meta;
pub mod vectors;
