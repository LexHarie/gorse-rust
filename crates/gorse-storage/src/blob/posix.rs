// Ports: storage/blob/posix.go @ add21b5, storage/blob/posix_test.go @ add21b5

//! A blob store in a local directory: `blob.uri` without `://`.

use std::path::PathBuf;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::sync::oneshot;

use super::{BlobError, BlobWriter, Store, check_ctx, check_name};

/// A blob store that keeps each blob as a file in one directory (Go's
/// `blob.POSIX`). Each file call runs on tokio's blocking pool (`tokio::fs`).
#[derive(Debug, Clone)]
pub struct Posix {
    dir: PathBuf,
}

impl Posix {
    /// Creates a store over `dir`. The directory is created by [`Store::create`]
    /// when needed.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl Store for Posix {
    async fn open(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>, BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        Ok(Box::new(File::open(self.dir.join(name)).await?))
    }

    async fn create(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Box<dyn BlobWriter>, oneshot::Receiver<()>), BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let file = File::create(&path).await?;
        let (done, closed) = oneshot::channel();
        Ok((Box::new(PosixWriter { file, _done: done }), closed))
    }

    async fn list(&self, ctx: &Context) -> Result<Vec<String>, BlobError> {
        check_ctx(ctx)?;
        let mut names = Vec::new();
        let mut entries = fs::read_dir(&self.dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            check_ctx(ctx)?;
            if !entry.file_type().await?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        // Go's os.ReadDir returns entries sorted by name; read_dir does not.
        names.sort();
        Ok(names)
    }

    async fn remove(&self, ctx: &Context, name: &str) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        check_name(name)?;
        let path = self.dir.join(name);
        if let Err(err) = fs::remove_file(&path).await {
            tracing::error!(file = %path.display(), error = %err, "failed to remove file");
            return Err(err.into());
        }
        Ok(())
    }
}

/// Writes straight into the file. Go copies through an `io.Pipe` on a
/// goroutine; the file is complete when the writer is dropped, which
/// disconnects the `done` channel.
struct PosixWriter {
    file: File,
    _done: oneshot::Sender<()>,
}

#[async_trait]
impl BlobWriter for PosixWriter {
    async fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        self.file.write_all(data).await?;
        Ok(())
    }

    async fn close(self: Box<Self>, ctx: &Context) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        let mut writer = self;
        writer.file.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gorse_storage_core::errors::Error as StorageError;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_posix() {
        let ctx = Context::background();
        let root = tempfile::tempdir().unwrap();
        // create client
        let client = Posix::new(root.path().join("blob"));

        // write a temp file
        let (mut w, done) = client.create(&ctx, "test").await.unwrap();
        w.write(&ctx, b"hello world").await.unwrap();
        w.close(&ctx).await.unwrap();
        // Go's done channel is closed here; the receiver fails instead.
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );

        // read the file
        let mut r = client.open(&ctx, "test").await.unwrap();
        let mut content = [0u8; 11];
        r.read_exact(&mut content).await.unwrap();
        assert_eq!(&content, b"hello world");
        drop(r);

        // list files
        let files = client.list(&ctx).await.unwrap();
        assert!(files.contains(&"test".to_string()));

        // remove the file
        client.remove(&ctx, "test").await.unwrap();
        let files = client.list(&ctx).await.unwrap();
        assert!(!files.contains(&"test".to_string()));
    }

    /// Not in Go's tests. A missing blob reports the storage sentinel NotFound,
    /// which callers test for (Go's `storage.ErrNotFound`).
    #[tokio::test]
    async fn open_missing_blob_is_not_found() {
        let ctx = Context::background();
        let root = tempfile::tempdir().unwrap();
        let client = Posix::new(root.path());

        let err = client.open(&ctx, "missing").await.err().unwrap();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
    }
}
