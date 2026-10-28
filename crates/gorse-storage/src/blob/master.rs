// Ports: storage/blob/blob.go @ add21b5 (MasterStoreServer, MasterStoreClient), storage/blob/blob_test.go @ add21b5

//! The master's `BlobStore` gRPC service, and the client a worker or server
//! uses to reach it when `blob.uri` is a local directory on the master.

use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll, ready};
use std::time::SystemTime;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_protocol::blob_store_client::BlobStoreClient;
use gorse_protocol::blob_store_server::BlobStore;
use gorse_protocol::{
    DownloadBlobRequest, DownloadBlobResponse, ListBlobsRequest, ListBlobsResponse,
    RemoveBlobRequest, RemoveBlobResponse, UploadBlobRequest, UploadBlobResponse,
};
use prost_types::Timestamp;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status, Streaming};

use super::posix::Posix;
use super::{BlobError, BlobWriter, Store, check_ctx, check_name};

/// The largest chunk of data in one gRPC message (Go's `1024*1024*4`).
const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Serves the blobs of one directory over gRPC (Go's `blob.MasterStoreServer`).
#[derive(Debug)]
pub struct MasterStoreServer {
    dir: PathBuf,
}

impl MasterStoreServer {
    /// Creates the server over `dir`, creating the directory if needed. Go
    /// exits the process when this fails; the Rust caller decides instead.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
}

/// The status Go's gRPC returns for an error that is not a status: code
/// `Unknown` with the error text.
fn unknown(err: impl std::fmt::Display) -> Status {
    Status::unknown(err.to_string())
}

#[tonic::async_trait]
impl BlobStore for MasterStoreServer {
    async fn upload_blob(
        &self,
        request: Request<Streaming<UploadBlobRequest>>,
    ) -> Result<Response<UploadBlobResponse>, Status> {
        let mut stream = request.into_inner();
        // The temp file is removed if the upload fails; Go leaves it behind.
        let dir = self.dir.clone();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new().prefix("upload-").tempfile_in(dir)
        })
        .await
        .map_err(unknown)?
        .map_err(unknown)?;
        let (file, path) = temp.into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let mut name = String::new();
        // Go's zero time is replaced by the first chunk's timestamp; a missing
        // timestamp reads as the Unix epoch, as protobuf's AsTime does.
        let mut timestamp: Option<(i64, i32)> = None;
        while let Some(req) = stream.message().await? {
            if name.is_empty() {
                name = req.name;
            } else if name != req.name {
                return Err(Status::unknown("inconsistent name"));
            }
            let ts = req.timestamp.unwrap_or_default();
            match timestamp {
                None => timestamp = Some((ts.seconds, ts.nanos)),
                Some(first) if first != (ts.seconds, ts.nanos) => {
                    return Err(Status::unknown("inconsistent timestamp"));
                }
                Some(_) => {}
            }
            file.write_all(&req.data).await.map_err(unknown)?;
        }
        file.flush().await.map_err(unknown)?;
        drop(file);
        check_name(&name).map_err(unknown)?;
        let dest = self.dir.join(&name);
        tokio::task::spawn_blocking(move || path.persist(dest))
            .await
            .map_err(unknown)?
            .map_err(|err| unknown(err.error))?;
        Ok(Response::new(UploadBlobResponse {}))
    }

    type DownloadBlobStream = ReceiverStream<Result<DownloadBlobResponse, Status>>;

    async fn download_blob(
        &self,
        request: Request<DownloadBlobRequest>,
    ) -> Result<Response<Self::DownloadBlobStream>, Status> {
        let name = request.into_inner().name;
        check_name(&name).map_err(unknown)?;
        // Go returns an open error before the stream starts; so does this.
        let mut file = tokio::fs::File::open(self.dir.join(&name))
            .await
            .map_err(unknown)?;
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let mut buf = vec![0u8; CHUNK_SIZE];
            loop {
                match file.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let response = DownloadBlobResponse {
                            data: buf[..n].to_vec(),
                        };
                        if tx.send(Ok(response)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(unknown(err))).await;
                        break;
                    }
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn list_blobs(
        &self,
        _request: Request<ListBlobsRequest>,
    ) -> Result<Response<ListBlobsResponse>, Status> {
        // The gRPC call has no Go context to carry; cancellation is the client
        // dropping the call.
        let names = Posix::new(&self.dir)
            .list(&Context::background())
            .await
            .map_err(unknown)?;
        Ok(Response::new(ListBlobsResponse { names }))
    }

    async fn remove_blob(
        &self,
        request: Request<RemoveBlobRequest>,
    ) -> Result<Response<RemoveBlobResponse>, Status> {
        let name = request.into_inner().name;
        check_name(&name).map_err(unknown)?;
        tokio::fs::remove_file(self.dir.join(&name))
            .await
            .map_err(unknown)?;
        Ok(Response::new(RemoveBlobResponse {}))
    }
}

/// A [`Store`] that reads and writes blobs through the master's gRPC service
/// (Go's `blob.MasterStoreClient`).
pub struct MasterStoreClient {
    client: BlobStoreClient<Channel>,
}

impl MasterStoreClient {
    /// Creates a client over `channel`. The channel is connected in the
    /// caller's tokio runtime.
    pub fn new(channel: Channel) -> Self {
        Self {
            client: BlobStoreClient::new(channel),
        }
    }
}

#[async_trait]
impl Store for MasterStoreClient {
    async fn open(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Box<dyn AsyncRead + Send + Unpin>, BlobError> {
        check_ctx(ctx)?;
        let mut client = self.client.clone();
        let response = client
            .download_blob(DownloadBlobRequest {
                name: name.to_string(),
            })
            .await?;
        Ok(Box::new(MasterReader {
            stream: response.into_inner(),
            chunk: Vec::new(),
            pos: 0,
        }))
    }

    async fn create(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Box<dyn BlobWriter>, oneshot::Receiver<()>), BlobError> {
        check_ctx(ctx)?;
        let (chunks, requests) = mpsc::channel(1);
        let (done, closed) = oneshot::channel();
        let mut client = self.client.clone();
        let task = tokio::spawn(async move {
            // Dropping `done` closes the channel once the upload has ended.
            let _done = done;
            if let Err(err) = client.upload_blob(ReceiverStream::new(requests)).await {
                tracing::error!(error = %err, "failed to close stream");
            }
        });
        Ok((
            Box::new(MasterWriter {
                name: name.to_string(),
                chunks,
                task,
            }),
            closed,
        ))
    }

    async fn list(&self, ctx: &Context) -> Result<Vec<String>, BlobError> {
        check_ctx(ctx)?;
        let mut client = self.client.clone();
        let response = client.list_blobs(ListBlobsRequest {}).await?;
        Ok(response.into_inner().names)
    }

    async fn remove(&self, ctx: &Context, name: &str) -> Result<(), BlobError> {
        check_ctx(ctx)?;
        let mut client = self.client.clone();
        client
            .remove_blob(RemoveBlobRequest {
                name: name.to_string(),
            })
            .await?;
        Ok(())
    }
}

/// Streams the chunks of a `DownloadBlob` call as a reader.
struct MasterReader {
    stream: Streaming<DownloadBlobResponse>,
    chunk: Vec<u8>,
    pos: usize,
}

impl AsyncRead for MasterReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        while this.pos == this.chunk.len() {
            match ready!(Pin::new(&mut this.stream).poll_next(cx)) {
                Some(Ok(response)) => {
                    this.chunk = response.data;
                    this.pos = 0;
                }
                Some(Err(status)) => return Poll::Ready(Err(std::io::Error::other(status))),
                None => return Poll::Ready(Ok(())),
            }
        }
        let n = out.remaining().min(this.chunk.len() - this.pos);
        out.put_slice(&this.chunk[this.pos..this.pos + n]);
        this.pos += n;
        Poll::Ready(Ok(()))
    }
}

/// Sends the data written to it as `UploadBlob` chunks. The upload runs on a
/// task of its own, which `task` keeps so that a cancelled write can abort it.
struct MasterWriter {
    name: String,
    chunks: mpsc::Sender<UploadBlobRequest>,
    task: JoinHandle<()>,
}

#[async_trait]
impl BlobWriter for MasterWriter {
    async fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<(), BlobError> {
        for chunk in data.chunks(CHUNK_SIZE) {
            if let Err(err) = check_ctx(ctx) {
                self.task.abort();
                return Err(err);
            }
            // Go stamps every chunk with its own time, so the server rejects a
            // second chunk of the same upload. The port keeps that (quirk
            // Q-WP210-1, for a decision).
            let request = UploadBlobRequest {
                name: self.name.clone(),
                timestamp: Some(Timestamp::from(SystemTime::now())),
                data: chunk.to_vec(),
            };
            self.chunks
                .send(request)
                .await
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        }
        Ok(())
    }

    async fn close(self: Box<Self>, ctx: &Context) -> Result<(), BlobError> {
        if let Err(err) = check_ctx(ctx) {
            self.task.abort();
            return Err(err);
        }
        // Dropping the sender ends the upload stream, as Go's pipe close does.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gorse_protocol::blob_store_server::BlobStoreServer;
    use std::time::Duration;
    use tonic::transport::{Endpoint, Server};

    /// Serves `root/blob` on a local port, in the test's runtime.
    async fn serve(root: &std::path::Path) -> MasterStoreClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let service = MasterStoreServer::new(root.join("blob")).unwrap();
        tokio::spawn(
            Server::builder()
                .add_service(BlobStoreServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let channel = Endpoint::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        MasterStoreClient::new(channel)
    }

    #[tokio::test]
    async fn test_blob() {
        let ctx = Context::background();
        let root = tempfile::tempdir().unwrap();
        // start server and create client
        let client = serve(root.path()).await;

        // write a temp file
        let (mut w, done) = client.create(&ctx, "test").await.unwrap();
        w.write(&ctx, b"hello world").await.unwrap();
        w.close(&ctx).await.unwrap();
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
        // Go: the second Read returns io.EOF
        assert_eq!(r.read(&mut [0u8; 1]).await.unwrap(), 0);
        drop(r);

        // list files
        let files = client.list(&ctx).await.unwrap();
        assert!(files.contains(&"test".to_string()));

        // remove the file
        client.remove(&ctx, "test").await.unwrap();
        let files = client.list(&ctx).await.unwrap();
        assert!(!files.contains(&"test".to_string()));
    }

    /// Not in Go's tests. Go stamps every chunk with its own time, so the
    /// server rejects a second chunk of the same upload. The port keeps this
    /// (Q-WP210-1): the blob is not created, and Close still returns Ok.
    #[tokio::test]
    async fn quirk_second_chunk_is_rejected() {
        let ctx = Context::background();
        let root = tempfile::tempdir().unwrap();
        let client = serve(root.path()).await;

        let (mut w, done) = client.create(&ctx, "split").await.unwrap();
        w.write(&ctx, b"hello ").await.unwrap();
        w.write(&ctx, b"world").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            !client
                .list(&ctx)
                .await
                .unwrap()
                .contains(&"split".to_string())
        );
    }

    /// Not in Go's tests. A 4 MiB chunk with its framing is larger than gRPC's
    /// default 4 MiB receive limit, so the server refuses it and the blob is
    /// not created (Q-WP210-2), as in Go.
    #[tokio::test]
    async fn quirk_full_size_chunk_exceeds_grpc_limit() {
        let ctx = Context::background();
        let root = tempfile::tempdir().unwrap();
        let client = serve(root.path()).await;

        let (mut w, done) = client.create(&ctx, "full").await.unwrap();
        w.write(&ctx, &vec![1u8; 4 * 1024 * 1024]).await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            !client
                .list(&ctx)
                .await
                .unwrap()
                .contains(&"full".to_string())
        );
    }

    /// Not in Go's tests. A cancelled upload is aborted, not ended: the server
    /// persists only a stream that ends normally, so no blob is created.
    #[tokio::test]
    async fn cancelled_upload_persists_nothing() {
        let root = tempfile::tempdir().unwrap();
        let client = serve(root.path()).await;

        let (ctx, cancel) = Context::background().with_cancel();
        let (mut w, done) = client.create(&ctx, "cut").await.unwrap();
        w.write(&ctx, b"partial").await.unwrap();
        cancel.cancel();
        assert!(matches!(
            w.write(&ctx, b"rest").await,
            Err(BlobError::Canceled(_))
        ));
        // The aborted upload task drops its done sender.
        assert!(
            tokio::time::timeout(Duration::from_secs(10), done)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            !client
                .list(&Context::background())
                .await
                .unwrap()
                .contains(&"cut".to_string())
        );
    }
}
