// Ports: logics/*.go @ add21b5

//! Recommendation pipeline logic shared by the master, worker and server nodes.
//!
//! The CF embeddings, the vector writer, the non-personalised, external and
//! reranker logics, the item-to-item and user-to-user recommenders (WP-3-4,
//! WP-3-6, WP-3-5) and the recommendation pipeline (WP-3-7) are ported.

pub mod cf;
pub mod external;
pub mod item_to_item;
pub mod non_personalized;
pub mod recommend;
pub mod reranker;
pub mod user_to_user;
pub mod vector_writer;

#[cfg(test)]
mod testing;
