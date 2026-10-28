// Ports: storage/blob/blob.go @ add21b5

//! Blob stores: the `Store` interface of Go's `storage/blob` package and the
//! factory that picks an implementation from `blob.uri`.
//!
//! The stores are async (decision D-122 on ba01cae1). Every method takes the
//! Go-style context first and checks it on entry and between units of work.
//! No store owns a runtime: the node that serves them owns one, so the methods
//! are called from inside a tokio task.

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

use async_trait::async_trait;
use gorse_common::gocontext::{Context, ContextError};
use gorse_config::config::schema::BlobConfig;
use gorse_storage_core::errors::Error as StorageError;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::sync::oneshot;
use tonic::transport::Channel;

/// A named blob store (Go's `blob.Store`).
///
/// `create` returns the writer and a receiver that completes once the blob is
/// finished. Go closes a `done` channel at that point; the receiver here fails
/// with `RecvError` instead, because the sender is dropped.
#[async_trait]
pub trait Store: Send + Sync {
    /// Opens the blob `name` for reading.
    async fn open(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>, BlobError>;
    /// Starts writing the blob `name`.
    async fn create(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Box<dyn BlobWriter>, oneshot::Receiver<()>), BlobError>;
    /// Lists the names of all blobs.
    async fn list(&self, ctx: &Context) -> Result<Vec<String>, BlobError>;
    /// Removes the blob `name`.
    async fn remove(&self, ctx: &Context, name: &str) -> Result<(), BlobError>;
}

/// The writer returned by [`Store::create`] (Go's `io.WriteCloser`).
///
/// Dropping the writer without closing it leaves the blob incomplete.
#[async_trait]
pub trait BlobWriter: Send {
    /// Writes `data` after the data written before it.
    async fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<(), BlobError>;
    /// Finishes the blob.
    async fn close(self: Box<Self>, ctx: &Context) -> Result<(), BlobError>;
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
    /// The context was cancelled before the operation finished.
    #[error(transparent)]
    Canceled(#[from] ContextError),
    /// An error from a cloud provider.
    #[cfg(any(feature = "s3", feature = "gcs", feature = "azure"))]
    #[error(transparent)]
    Cloud(#[from] object_store::Error),
}

impl BlobError {
    /// The storage sentinel this error stands for: [`StorageError::NotFound`]
    /// for a blob that does not exist, as Go's `storage.ErrNotFound`.
    pub fn kind(&self) -> Option<StorageError> {
        match self {
            BlobError::Io(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Some(StorageError::NotFound)
            }
            #[cfg(any(feature = "s3", feature = "gcs", feature = "azure"))]
            BlobError::Cloud(object_store::Error::NotFound { .. }) => Some(StorageError::NotFound),
            _ => None,
        }
    }
}

/// Rejects names that would leave the store's directory: empty names, names
/// containing `/` or `\`, and `.` and `..` (decision D-12, quirk X-2).
pub fn check_name(name: &str) -> Result<(), BlobError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(BlobError::InvalidName);
    }
    Ok(())
}

/// Fails with the cancellation error once `ctx` is done (Go's `ctx.Err()`).
pub(crate) fn check_ctx(ctx: &Context) -> Result<(), BlobError> {
    match ctx.err() {
        Some(err) => Err(BlobError::Canceled(err)),
        None => Ok(()),
    }
}

/// Returns the store for `cfg.uri` (Go's `blob.NewStore`).
///
/// A URI with a scheme selects a cloud store. Otherwise the store is the
/// master's gRPC service when `master` is given, or a local directory.
pub fn new_store(cfg: &BlobConfig, master: Option<Channel>) -> Result<Box<dyn Store>, BlobError> {
    if !cfg.uri.contains("://") {
        return match master {
            Some(channel) => Ok(Box::new(master::MasterStoreClient::new(channel))),
            None => Ok(Box::new(posix::Posix::new(&cfg.uri))),
        };
    }
    let parsed = url::Url::parse(&cfg.uri)?;
    // Go's url.Parse keeps the host's port and the decoded path.
    let host = match (parsed.host_str(), parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => String::new(),
    };
    let path = percent_encoding::percent_decode_str(parsed.path())
        .decode_utf8_lossy()
        .trim_start_matches('/')
        .to_string();
    match parsed.scheme() {
        "s3" => {
            if host.is_empty() {
                return Err(BlobError::Config(
                    "blob.uri must include bucket for s3://".to_string(),
                ));
            }
            s3_store(cfg, &host, &path)
        }
        "gs" => {
            if host.is_empty() {
                return Err(BlobError::Config(
                    "blob.uri must include bucket for gs://".to_string(),
                ));
            }
            gcs_store(cfg, &host, &path)
        }
        "az" => {
            if host.is_empty() {
                return Err(BlobError::Config(
                    "blob.uri must include container for az://".to_string(),
                ));
            }
            azure_store(cfg, &host, &path)
        }
        scheme => Err(BlobError::UnsupportedScheme(scheme.to_string())),
    }
}

#[cfg(feature = "s3")]
fn s3_store(cfg: &BlobConfig, bucket: &str, prefix: &str) -> Result<Box<dyn Store>, BlobError> {
    Ok(Box::new(s3::new(&cfg.s3, bucket, prefix)?))
}

#[cfg(not(feature = "s3"))]
fn s3_store(_: &BlobConfig, _: &str, _: &str) -> Result<Box<dyn Store>, BlobError> {
    Err(BlobError::UnsupportedScheme("s3".to_string()))
}

#[cfg(feature = "gcs")]
fn gcs_store(cfg: &BlobConfig, bucket: &str, prefix: &str) -> Result<Box<dyn Store>, BlobError> {
    Ok(Box::new(gcs::new(&cfg.gcs, bucket, prefix)?))
}

#[cfg(not(feature = "gcs"))]
fn gcs_store(_: &BlobConfig, _: &str, _: &str) -> Result<Box<dyn Store>, BlobError> {
    Err(BlobError::UnsupportedScheme("gs".to_string()))
}

#[cfg(feature = "azure")]
fn azure_store(
    cfg: &BlobConfig,
    container: &str,
    prefix: &str,
) -> Result<Box<dyn Store>, BlobError> {
    Ok(Box::new(azure::new(&cfg.azure, container, prefix)?))
}

#[cfg(not(feature = "azure"))]
fn azure_store(_: &BlobConfig, _: &str, _: &str) -> Result<Box<dyn Store>, BlobError> {
    Err(BlobError::UnsupportedScheme("az".to_string()))
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
