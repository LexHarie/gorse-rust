// Ports: storage/blob/blob.go @ add21b5

//! Blob stores: the `Store` interface of Go's `storage/blob` package and the
//! factory that picks an implementation from `blob.uri`.
//!
//! Go's `Store` is synchronous, so the Rust trait is too. The cloud stores and
//! the master's gRPC client are async underneath and block on a tokio runtime
//! they own (see [`cloud`] and [`master`]). Their methods must therefore not be
//! called from inside a tokio task; use `spawn_blocking` there.

pub mod master;
pub mod posix;

#[cfg(any(feature = "s3", feature = "gcs", feature = "azure"))]
mod cloud;

#[cfg(feature = "azure")]
pub mod azure;
#[cfg(feature = "gcs")]
pub mod gcs;
#[cfg(feature = "s3")]
pub mod s3;

use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

use gorse_config::config::schema::BlobConfig;
use thiserror::Error;
use tonic::transport::Channel;

/// A named blob store (Go's `blob.Store`).
///
/// `create` returns the writer and a channel that disconnects once the blob
/// is complete, which is the Rust form of Go's `done chan struct{}` that is
/// closed at the same point.
pub trait Store: Send + Sync {
    /// Opens the blob `name` for reading.
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError>;
    /// Starts writing the blob `name`.
    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError>;
    /// Lists the names of all blobs.
    fn list(&self) -> Result<Vec<String>, BlobError>;
    /// Removes the blob `name`.
    fn remove(&self, name: &str) -> Result<(), BlobError>;
}

/// The writer returned by [`Store::create`] (Go's `io.WriteCloser`).
pub trait BlobWriter: Write + Send {
    /// Finishes the blob. Dropping the writer without closing it leaves the
    /// blob incomplete.
    fn close(self: Box<Self>) -> Result<(), BlobError>;
}

/// Errors of the blob stores.
#[derive(Debug, Error)]
pub enum BlobError {
    /// A name that would leave the store's directory (decision D-12).
    #[error("invalid blob name")]
    InvalidName,
    /// A configuration error from [`new_store`] or a constructor.
    #[error("{0}")]
    Config(String),
    /// A `blob.uri` scheme that has no store.
    #[error("unsupported blob.uri scheme: {0}")]
    UnsupportedScheme(String),
    /// A `blob.uri` that does not parse.
    #[error(transparent)]
    Url(#[from] url::ParseError),
    /// A local file error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// An error returned by the master's gRPC `BlobStore` service.
    #[error(transparent)]
    Status(#[from] tonic::Status),
    /// A failure to connect to the master.
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    /// An error from a cloud provider.
    #[cfg(any(feature = "s3", feature = "gcs", feature = "azure"))]
    #[error(transparent)]
    Cloud(#[from] object_store::Error),
}

/// Rejects names that would leave the store's directory: empty names, names
/// containing `/` or `\`, and `.` and `..` (decision D-12, quirk X-2).
pub fn check_name(name: &str) -> Result<(), BlobError> {
    unimplemented!("RED: check_name {name:?}")
}

/// Returns the store for `cfg.uri` (Go's `blob.NewStore`).
///
/// A URI with a scheme selects a cloud store. Otherwise the store is the
/// master's gRPC service when `master` is given, or a local directory.
pub fn new_store(cfg: &BlobConfig, master: Option<Channel>) -> Result<Box<dyn Store>, BlobError> {
    unimplemented!("RED: new_store {:?} {}", cfg.uri, master.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_name_rejects_unsafe_names() {
        for name in ["", ".", "..", "a/b", "../x", "a\\b"] {
            assert!(
                matches!(check_name(name), Err(BlobError::InvalidName)),
                "{name:?} must be rejected"
            );
        }
        for name in ["1", "123456", "model-7", "test.txt"] {
            assert!(check_name(name).is_ok(), "{name:?} must be accepted");
        }
    }

    #[test]
    fn new_store_rejects_unknown_scheme() {
        let cfg = BlobConfig {
            uri: "ftp://host/blob".to_string(),
            ..Default::default()
        };
        match new_store(&cfg, None) {
            Err(BlobError::UnsupportedScheme(scheme)) => assert_eq!(scheme, "ftp"),
            Err(err) => panic!("unexpected error: {err}"),
            Ok(_) => panic!("ftp:// must not produce a store"),
        }
    }
}
