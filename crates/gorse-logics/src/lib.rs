// Ports: logics/*.go @ add21b5

//! Recommendation pipeline logic shared by the master, worker and server nodes.
//!
//! The CF embeddings, the vector writer, and the non-personalised, external and
//! reranker logics are ported (WP-3-4, WP-3-6). The other files of the Go package
//! are not yet.

pub mod cf;
pub mod external;
pub mod non_personalized;
pub mod reranker;
pub mod vector_writer;

#[cfg(test)]
mod testing;
