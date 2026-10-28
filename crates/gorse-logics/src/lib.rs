// Ports: logics/*.go @ add21b5

//! Recommendation pipeline logic shared by the master, worker and server nodes.
//!
//! The CF embeddings and the vector writer are ported (WP-3-4). The other files
//! of the Go package are not yet.

pub mod cf;
pub mod vector_writer;
