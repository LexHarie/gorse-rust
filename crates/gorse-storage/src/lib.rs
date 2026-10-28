// Ports: storage/data/*.go, storage/cache/*.go, storage/vectors/*.go, storage/blob/*.go, storage/meta/*.go @ add21b5

//! Ports of the Go `storage` packages.
//!
//! The blob stores are in [`blob`], and the data store's record types are in
//! [`data::types`]. The cache, vector and meta stores are Phase 2 work and are
//! not ported yet.

pub mod blob;
pub mod data;
