// Ports: storage/data/*.go, storage/cache/*.go, storage/vectors/*.go, storage/blob/*.go, storage/meta/*.go @ add21b5

//! Data, cache, vector, blob and meta stores, one feature per backend.
//!
//! The data store's record types are in [`data::types`], the cache store's core
//! is in [`cache`], and the vector store's core is in [`vectors`]. The other
//! stores are Phase 2 work and are not ported yet.

pub mod cache;
pub mod data;
pub mod vectors;
