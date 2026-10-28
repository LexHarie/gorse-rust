// Ports: storage/blob/s3.go, storage/blob/gcs.go, storage/blob/azure.go @ add21b5 (the shared `Store` methods)

//! The part that S3, GCS and Azure have in common: keys under a prefix, the
//! blocking bridge to `object_store`, and streaming reads and writes.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, WriteMultipart};
use tokio::runtime::Runtime;

use super::{BlobError, BlobWriter, Store, check_name};

/// Data up to this size is uploaded in one request. Larger data starts a
/// multipart upload in parts of this size (the S3 minimum part size).
const PART_SIZE: usize = 5 * 1024 * 1024;

/// How many parts may be in flight before a write waits for one to finish.
const MAX_PARTS_IN_FLIGHT: usize = 4;

/// A [`Store`] over one `object_store` backend, with keys under `prefix`
/// (Go's `S3`, `GCS` and `AzureBlob` structs). It owns the runtime that drives
/// the calls, so its methods must not be called from inside a tokio task.
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            store,
            prefix: prefix.to_string(),
            runtime: Arc::new(runtime),
            provider,
        })
    }

    /// The prefix without leading or trailing slashes, as Go's `filepath.Join`
    /// treats it.
    fn prefix(&self) -> &str {
        self.prefix.trim_matches('/')
    }

    /// The object path of `name`: `prefix/name`.
    fn path(&self, name: &str) -> Result<Path, BlobError> {
        let prefix = self.prefix();
        let key = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        Path::parse(key).map_err(|err| BlobError::Config(err.to_string()))
    }
}

impl Store for CloudStore {
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError> {
        check_name(name)?;
        let result = self.runtime.block_on(self.store.get(&self.path(name)?))?;
        Ok(Box::new(CloudReader {
            runtime: Arc::clone(&self.runtime),
            stream: result.into_stream(),
            chunk: Vec::new(),
            pos: 0,
        }))
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        check_name(name)?;
        let (done, closed) = mpsc::channel();
        Ok((
            Box::new(CloudWriter {
                runtime: Arc::clone(&self.runtime),
                store: Arc::clone(&self.store),
                path: self.path(name)?,
                buffer: Vec::new(),
                upload: None,
                provider: self.provider,
                _done: done,
            }),
            closed,
        ))
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        let prefix = self.prefix();
        let filter = if prefix.is_empty() {
            None
        } else {
            Some(Path::parse(prefix).map_err(|err| BlobError::Config(err.to_string()))?)
        };
        let objects = self
            .runtime
            .block_on(self.store.list(filter.as_ref()).try_collect::<Vec<_>>())?;
        let mut names = Vec::new();
        for object in objects {
            // Go slices the key after the prefix and drops one leading '/'.
            let key = object.location.as_ref();
            let rest = key.strip_prefix(prefix).unwrap_or(key);
            let name = rest.strip_prefix('/').unwrap_or(rest);
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        check_name(name)?;
        let path = self.path(name)?;
        if let Err(err) = self.runtime.block_on(self.store.delete(&path)) {
            tracing::error!(file = %path, error = %err, "failed to remove file from {}", self.provider);
            return Err(err.into());
        }
        Ok(())
    }
}

/// Reads an object chunk by chunk, blocking on the store's runtime.
struct CloudReader {
    runtime: Arc<Runtime>,
    stream: BoxStream<'static, object_store::Result<Bytes>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl Read for CloudReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.pos == self.chunk.len() {
            match self.runtime.block_on(self.stream.next()) {
                Some(Ok(bytes)) => {
                    self.chunk = Vec::from(bytes);
                    self.pos = 0;
                }
                Some(Err(err)) => return Err(std::io::Error::other(err)),
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.chunk.len() - self.pos);
        out[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Uploads the data written to it. Data up to [`PART_SIZE`] goes up in one
/// request on close; more starts a multipart upload.
struct CloudWriter {
    runtime: Arc<Runtime>,
    store: Arc<dyn ObjectStore>,
    path: Path,
    buffer: Vec<u8>,
    upload: Option<WriteMultipart>,
    provider: &'static str,
    _done: Sender<()>,
}

impl CloudWriter {
    /// Hands the buffered data to the multipart upload, starting it on first use.
    fn send_buffered(&mut self) -> object_store::Result<()> {
        if self.upload.is_none() {
            let upload = self
                .runtime
                .block_on(self.store.put_multipart(&self.path))?;
            self.upload = Some(WriteMultipart::new(upload));
        }
        if let Some(upload) = self.upload.as_mut() {
            let buffer = &self.buffer;
            self.runtime.block_on(async {
                upload.wait_for_capacity(MAX_PARTS_IN_FLIGHT).await?;
                upload.write(buffer);
                Ok::<(), object_store::Error>(())
            })?;
        }
        self.buffer.clear();
        Ok(())
    }
}

impl Write for CloudWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        if self.buffer.len() > PART_SIZE {
            self.send_buffered().map_err(std::io::Error::other)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl BlobWriter for CloudWriter {
    fn close(self: Box<Self>) -> Result<(), BlobError> {
        let CloudWriter {
            runtime,
            store,
            path,
            buffer,
            upload,
            provider,
            _done,
        } = *self;
        let result = match upload {
            None => runtime
                .block_on(store.put(&path, buffer.into()))
                .map(|_| ()),
            Some(mut upload) => runtime
                .block_on(async move {
                    upload.write(&buffer);
                    upload.finish().await
                })
                .map(|_| ()),
        };
        if let Err(err) = &result {
            tracing::error!(file = %path, error = %err, "failed to upload file to {provider}");
        }
        result.map_err(BlobError::from)
    }
}

/// Reads an environment variable that a cloud test needs, failing the test
/// when it is missing. A silent skip would look like a pass.
#[cfg(test)]
pub(crate) fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set to run this test"))
}
