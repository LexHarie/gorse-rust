// Ports: master/*.go @ add21b5

//! The master node: training, non-personalised recommendation, cluster metadata and the dashboard API.
//!
//! This crate holds the master core (WP-4-3): the gRPC `Master` service, the
//! tasks that load the dataset and train the models, and the Prometheus metrics.
//! The REST routes (WP-4-4) and the binaries (WP-4-5) are not here yet.

mod dashboard;
mod master;
mod metrics;
mod oidc;
pub mod rest;
mod rpc;
mod tasks;
#[cfg(test)]
mod test_support;

pub use master::{ClickThroughRateState, CollaborativeFilteringState, Master};

/// The errors the master reports (Go's errors out of `master.Serve` and its tasks).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A configuration could not be read, parsed or validated.
    #[error("{0}")]
    Config(String),
    /// A data store call failed.
    #[error(transparent)]
    Data(#[from] gorse_storage::data::Error),
    /// A cache store call failed.
    #[error(transparent)]
    Cache(#[from] gorse_storage::cache::Error),
    /// A vector store call failed.
    #[error(transparent)]
    Vectors(#[from] gorse_storage::vectors::Error),
    /// A meta store call failed.
    #[error(transparent)]
    Meta(#[from] gorse_storage::meta::Error),
    /// A blob store call failed.
    #[error(transparent)]
    Blob(#[from] gorse_storage::blob::BlobError),
    /// A TLS configuration could not be built.
    #[error(transparent)]
    Tls(#[from] gorse_common::util::tls::TlsError),
    /// A non-personalized recommender could not be built.
    #[error(transparent)]
    NonPersonalized(#[from] gorse_logics::non_personalized::Error),
    /// An item-to-item or user-to-user recommender failed (the logics share one error type).
    #[error(transparent)]
    ItemToItem(#[from] gorse_logics::item_to_item::Error),
    /// A collaborative filtering helper failed.
    #[error(transparent)]
    Cf(#[from] gorse_logics::cf::Error),
    /// A click-through rate model failed.
    #[error(transparent)]
    Ctr(#[from] gorse_model::ctr::model::Error),
    /// A hyperparameter search failed.
    #[error(transparent)]
    Tpe(#[from] gorse_model::tpe::Error),
    /// The ranking pipeline failed. Boxed, since the worker's error is large
    /// and would make every result of this crate large.
    #[error(transparent)]
    Worker(Box<gorse_worker::Error>),
    /// A task stopped with a message that Go reports as an error text.
    #[error("{0}")]
    Message(String),
    /// A file or socket operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A JSON value could not be encoded or decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<gorse_worker::Error> for Error {
    fn from(err: gorse_worker::Error) -> Self {
        Self::Worker(Box::new(err))
    }
}
