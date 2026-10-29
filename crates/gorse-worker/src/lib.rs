// Ports: worker/*.go @ add21b5

//! The worker node of the Gorse port: the offline per-user recommendation.
//!
//! A [`Worker`] syncs with the master, pulls the ranking models and the user
//! index, and runs [`Pipeline::recommend`] for the users it is assigned. Each
//! part follows Go's `worker` package, and the tests check them against the Go
//! suites.

mod metrics;
mod pipeline;
#[cfg(test)]
mod testing;
mod worker;

pub use pipeline::{CachedItem, ItemCache, Pipeline, ProgressFn};
pub use worker::{
    HealthError, HealthStatus, Worker, WorkerOptions, check_live, check_ready, pull_users,
    worker_name,
};

/// The errors of the worker node.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The worker is not one of the workers the master names, so it has no users.
    #[error("current node isn't in worker nodes")]
    NotInWorkers,
    /// The worker has no connection to its master yet.
    #[error("the worker is not connected to the master")]
    NotConnected,
    /// The worker is already serving, so it cannot serve again.
    #[error("the worker is already serving")]
    AlreadyServing,
    /// The TLS files of the master connection cannot be loaded.
    #[error(transparent)]
    Tls(#[from] gorse_common::util::tls::TlsError),
    /// The master address is not a valid URI.
    #[error("invalid master address: {0}")]
    MasterAddress(String),
    /// The master's configuration does not parse.
    #[error("invalid master config: {0}")]
    Config(String),
    /// The host name cannot be read.
    #[error("failed to read the host name: {0}")]
    Hostname(#[source] std::io::Error),
    /// The data store failed.
    #[error(transparent)]
    Data(#[from] gorse_storage::data::Error),
    /// The cache store failed.
    #[error(transparent)]
    Cache(#[from] gorse_storage::cache::Error),
    /// The vector store failed.
    #[error(transparent)]
    Vectors(#[from] gorse_storage::vectors::Error),
    /// The blob store failed.
    #[error(transparent)]
    Blob(#[from] gorse_storage::blob::BlobError),
    /// A recommender failed.
    #[error(transparent)]
    Recommend(#[from] gorse_logics::recommend::Error),
    /// A reranker or decision API failed.
    #[error(transparent)]
    Rank(#[from] gorse_logics::reranker::RankError),
    /// A matrix factorisation model does not read.
    #[error(transparent)]
    Cf(#[from] gorse_logics::cf::Error),
    /// A click-through rate model does not read.
    #[error(transparent)]
    Model(#[from] gorse_model::ctr::model::Error),
    /// The consistent-hash ring has no hosts.
    #[error(transparent)]
    Consistent(#[from] gorse_common::consistent::Error),
    /// A run was cancelled.
    #[error(transparent)]
    Context(#[from] gorse_common::gocontext::ContextError),
    /// The master's gRPC call failed.
    #[error(transparent)]
    Grpc(#[from] tonic::Status),
    /// The master connection failed.
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    /// An I/O operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// Q-254: the types that cross threads are `Send` and `Sync`. This fails to
// compile when one of them is not.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Pipeline>();
    assert_send_sync::<ItemCache>();
    assert_send_sync::<CachedItem>();
    assert_send_sync::<Worker>();
    assert_send_sync::<WorkerOptions>();
    assert_send_sync::<HealthStatus>();
};
