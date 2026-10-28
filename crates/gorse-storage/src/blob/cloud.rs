// Ports: storage/blob/s3.go, storage/blob/gcs.go, storage/blob/azure.go @ add21b5 (the shared `Store` methods)

//! The part that S3, GCS and Azure have in common: keys under a prefix, the
//! `object_store` calls, and streaming reads and writes.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, ready};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use gorse_common::gocontext::Context;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, WriteMultipart};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::oneshot;

use super::{BlobError, BlobWriter, Store, check_ctx, check_name};

/// Data up to this size is uploaded in one request. Larger data starts a
/// multipart upload in parts of this size (the S3 minimum part size).
const PART_SIZE: usize = 5 * 1024 * 1024;

/// How many parts may be in flight before a write waits for one to finish.
const MAX_PARTS_IN_FLIGHT: usize = 4;

/// A [`Store`] over one `object_store` backend, with keys under `prefix`
/// (Go's `S3`, `GCS` and `AzureBlob` structs). It holds no runtime: the calls
/// run on the caller's tokio runtime.
pub struct CloudStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    provider: &'static str,
}

impl CloudStore {
    /// Wraps `store`. `provider` names the service in log lines.
    pub(crate) fn new(store: Arc<dyn ObjectStore>, prefix: &str, provider: &'static str) -> Self {
        Self {
            store,
            prefix: prefix.to_string(),
            provider,
        }
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

#[async_trait]
impl Store for CloudStore {
    async fn open(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>, BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        let result = self.store.get(&self.path(name)?).await?;
        Ok(Box::new(CloudReader {
            stream: result.into_stream(),
            chunk: Bytes::new(),
        }))
    }

    async fn create(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Box<dyn BlobWriter>, oneshot::Receiver<()>), BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        let (done, closed) = oneshot::channel();
        Ok((
            Box::new(CloudWriter {
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

    async fn list(&self, ctx: &Context) -> Result<Vec<String>, BlobError> {
        check_ctx(ctx)?;
        let prefix = self.prefix();
        let filter = if prefix.is_empty() {
            None
        } else {
            Some(Path::parse(prefix).map_err(|err| BlobError::Config(err.to_string()))?)
        };
        let mut objects = self.store.list(filter.as_ref());
        let mut names = Vec::new();
        while let Some(object) = objects.try_next().await? {
            check_ctx(ctx)?;
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

    async fn remove(&self, ctx: &Context, name: &str) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        let path = self.path(name)?;
        if let Err(err) = self.store.delete(&path).await {
            tracing::error!(file = %path, error = %err, "failed to remove file from {}", self.provider);
            return Err(err.into());
        }
        Ok(())
    }
}

/// Reads an object as its chunks arrive.
struct CloudReader {
    stream: BoxStream<'static, object_store::Result<Bytes>>,
    chunk: Bytes,
}

impl AsyncRead for CloudReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        while this.chunk.is_empty() {
            match ready!(this.stream.poll_next_unpin(cx)) {
                Some(Ok(bytes)) => this.chunk = bytes,
                Some(Err(err)) => return Poll::Ready(Err(std::io::Error::other(err))),
                None => return Poll::Ready(Ok(())),
            }
        }
        let n = out.remaining().min(this.chunk.len());
        out.put_slice(&this.chunk.split_to(n));
        Poll::Ready(Ok(()))
    }
}

/// Uploads the data written to it. Data up to [`PART_SIZE`] goes up in one
/// request on close; more starts a multipart upload.
struct CloudWriter {
    store: Arc<dyn ObjectStore>,
    path: Path,
    buffer: Vec<u8>,
    upload: Option<WriteMultipart>,
    provider: &'static str,
    _done: oneshot::Sender<()>,
}

impl CloudWriter {
    /// Hands the buffered data to the multipart upload, starting it on first use.
    async fn send_buffered(&mut self) -> object_store::Result<()> {
        if self.upload.is_none() {
            let upload = self.store.put_multipart(&self.path).await?;
            self.upload = Some(WriteMultipart::new(upload));
        }
        if let Some(upload) = self.upload.as_mut() {
            upload.wait_for_capacity(MAX_PARTS_IN_FLIGHT).await?;
            upload.write(&self.buffer);
        }
        self.buffer.clear();
        Ok(())
    }
}

#[async_trait]
impl BlobWriter for CloudWriter {
    async fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        self.buffer.extend_from_slice(data);
        if self.buffer.len() > PART_SIZE {
            self.send_buffered().await?;
        }
        Ok(())
    }

    async fn close(self: Box<Self>, ctx: &Context) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        let CloudWriter {
            store,
            path,
            buffer,
            upload,
            provider,
            _done,
        } = *self;
        let result = match upload {
            None => store.put(&path, buffer.into()).await.map(|_| ()),
            Some(mut upload) => {
                upload.write(&buffer);
                upload.finish().await.map(|_| ())
            }
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
