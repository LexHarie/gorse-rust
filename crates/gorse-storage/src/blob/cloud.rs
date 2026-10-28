// Ports: storage/blob/s3.go, storage/blob/gcs.go, storage/blob/azure.go @ add21b5 (the shared `Store` methods)

//! The part that S3, GCS and Azure have in common: keys under a prefix, the
//! blocking bridge to `object_store`, and streaming reads and writes.

use std::io::{Read, Write};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use object_store::ObjectStore;
use tokio::runtime::Runtime;

use super::{BlobError, BlobWriter, Store};

/// A [`Store`] over one `object_store` backend, with keys under `prefix`
/// (Go's `S3`, `GCS` and `AzureBlob` structs).
pub struct CloudStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    runtime: Arc<Runtime>,
    provider: &'static str,
}

impl CloudStore {
    /// Wraps `store`. `provider` names the service in log lines.
    pub(crate) fn new(
        store: Arc<dyn ObjectStore>,
        prefix: &str,
        provider: &'static str,
    ) -> Result<Self, BlobError> {
        let _ = store;
        unimplemented!("RED: cloud new {provider} {prefix:?}")
    }
}

impl Store for CloudStore {
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError> {
        unimplemented!("RED: cloud open {name:?} in {:?}", self.prefix)
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        unimplemented!("RED: cloud create {name:?} in {:?}", self.prefix)
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        unimplemented!("RED: cloud list in {:?}", self.prefix)
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        unimplemented!("RED: cloud remove {name:?} in {:?}", self.prefix)
    }
}

/// Reads an environment variable that a cloud test needs, failing the test
/// when it is missing. A silent skip would look like a pass.
#[cfg(test)]
pub(crate) fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set to run this test"))
}
