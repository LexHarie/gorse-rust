// Ports: storage/blob/blob.go @ add21b5 (MasterStoreServer, MasterStoreClient), storage/blob/blob_test.go @ add21b5

//! The master's `BlobStore` gRPC service, and the client a worker or server
//! uses to reach it when `blob.uri` is a local directory on the master.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::SystemTime;

use gorse_protocol::blob_store_client::BlobStoreClient;
use gorse_protocol::blob_store_server::BlobStore;
use gorse_protocol::{
    DownloadBlobRequest, DownloadBlobResponse, ListBlobsRequest, ListBlobsResponse,
    RemoveBlobRequest, RemoveBlobResponse, UploadBlobRequest, UploadBlobResponse,
};
use prost_types::Timestamp;
use tokio::runtime::Runtime;
use tokio::sync::mpsc as async_mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status, Streaming};

use super::posix::Posix;
use super::{BlobError, BlobWriter, Store, check_name};

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
        let mut temp = tempfile::Builder::new()
            .prefix("upload-")
            .tempfile_in(&self.dir)
            .map_err(unknown)?;
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
            temp.write_all(&req.data).map_err(unknown)?;
        }
        check_name(&name).map_err(unknown)?;
        temp.persist(self.dir.join(&name))
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
        let mut file = std::fs::File::open(self.dir.join(&name)).map_err(unknown)?;
        let (tx, rx) = async_mpsc::channel(1);
        tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; CHUNK_SIZE];
            loop {
                match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let response = DownloadBlobResponse {
                            data: buf[..n].to_vec(),
                        };
                        if tx.blocking_send(Ok(response)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.blocking_send(Err(unknown(err)));
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
        let names = Posix::new(&self.dir).list().map_err(unknown)?;
        Ok(Response::new(ListBlobsResponse { names }))
    }

    async fn remove_blob(
        &self,
        request: Request<RemoveBlobRequest>,
    ) -> Result<Response<RemoveBlobResponse>, Status> {
        let name = request.into_inner().name;
        check_name(&name).map_err(unknown)?;
        std::fs::remove_file(self.dir.join(&name)).map_err(unknown)?;
        Ok(Response::new(RemoveBlobResponse {}))
    }
}

/// A [`Store`] that reads and writes blobs through the master's gRPC service
/// (Go's `blob.MasterStoreClient`). It owns the runtime that drives the calls,
/// so its methods must not be called from inside a tokio task.
pub struct MasterStoreClient {
    client: BlobStoreClient<Channel>,
    runtime: Arc<Runtime>,
}

impl MasterStoreClient {
    /// Creates a client over `channel`, which must be connected in a tokio
    /// runtime that stays alive while the client is used.
    pub fn new(channel: Channel) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            client: BlobStoreClient::new(channel),
            runtime: Arc::new(runtime),
        })
    }
}

impl Store for MasterStoreClient {
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError> {
        let mut client = self.client.clone();
        let response = self
            .runtime
            .block_on(client.download_blob(DownloadBlobRequest {
                name: name.to_string(),
            }))?;
        Ok(Box::new(MasterReader {
            runtime: Arc::clone(&self.runtime),
            stream: response.into_inner(),
            chunk: Vec::new(),
            pos: 0,
        }))
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        let (chunks, requests) = async_mpsc::channel(1);
        let (done, closed) = std::sync::mpsc::channel();
        let mut client = self.client.clone();
        self.runtime.spawn(async move {
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
            }),
            closed,
        ))
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        let mut client = self.client.clone();
        let response = self
            .runtime
            .block_on(client.list_blobs(ListBlobsRequest {}))?;
        Ok(response.into_inner().names)
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        let mut client = self.client.clone();
        self.runtime
            .block_on(client.remove_blob(RemoveBlobRequest {
                name: name.to_string(),
            }))?;
        Ok(())
    }
}

/// Streams the chunks of a `DownloadBlob` call as a reader.
struct MasterReader {
    runtime: Arc<Runtime>,
    stream: Streaming<DownloadBlobResponse>,
    chunk: Vec<u8>,
    pos: usize,
}

impl Read for MasterReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.pos == self.chunk.len() {
            match self.runtime.block_on(self.stream.message()) {
                Ok(Some(response)) => {
                    self.chunk = response.data;
                    self.pos = 0;
                }
                Ok(None) => return Ok(0),
                Err(status) => return Err(std::io::Error::other(status)),
            }
        }
        let n = out.len().min(self.chunk.len() - self.pos);
        out[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Sends the data written to it as `UploadBlob` chunks.
struct MasterWriter {
    name: String,
    chunks: async_mpsc::Sender<UploadBlobRequest>,
}

impl Write for MasterWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for chunk in buf.chunks(CHUNK_SIZE) {
            // Go stamps every chunk with its own time, so the server rejects a
            // second chunk of the same upload. The port keeps that (quirk
            // Q-WP210-1, for a decision).
            let request = UploadBlobRequest {
                name: self.name.clone(),
                timestamp: Some(Timestamp::from(SystemTime::now())),
                data: chunk.to_vec(),
            };
            self.chunks
                .blocking_send(request)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl BlobWriter for MasterWriter {
    fn close(self: Box<Self>) -> Result<(), BlobError> {
        // Dropping the sender ends the upload stream, as Go's pipe close does.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gorse_protocol::blob_store_server::BlobStoreServer;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;
    use tonic::transport::{Endpoint, Server};

    /// Serves `root/blob` on a local port. The returned runtime drives the
    /// server and must outlive the client.
    fn serve(root: &std::path::Path) -> (Runtime, MasterStoreClient) {
        let server_runtime = Runtime::new().unwrap();
        let channel = server_runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let service = MasterStoreServer::new(root.join("blob")).unwrap();
            tokio::spawn(
                Server::builder()
                    .add_service(BlobStoreServer::new(service))
                    .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
            );
            Endpoint::from_shared(format!("http://{addr}"))
                .unwrap()
                .connect()
                .await
                .unwrap()
        });
        (server_runtime, MasterStoreClient::new(channel).unwrap())
    }

    #[test]
    fn test_blob() {
        let root = tempfile::tempdir().unwrap();
        // start server and create client
        let (_server, client) = serve(root.path());

        // write a temp file
        let (mut w, done) = client.create("test").unwrap();
        w.write_all(b"hello world").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        ));

        // read the file
        let mut r = client.open("test").unwrap();
        let mut content = [0u8; 11];
        r.read_exact(&mut content).unwrap();
        assert_eq!(&content, b"hello world");
        // Go: the second Read returns io.EOF
        assert_eq!(r.read(&mut [0u8; 1]).unwrap(), 0);
        drop(r);

        // list files
        let files = client.list().unwrap();
        assert!(files.contains(&"test".to_string()));

        // remove the file
        client.remove("test").unwrap();
        let files = client.list().unwrap();
        assert!(!files.contains(&"test".to_string()));
    }

    /// Not in Go's tests. Go stamps every chunk with its own time, so the
    /// server rejects a second chunk of the same upload. The port keeps this
    /// (Q-WP210-1): the blob is not created, and Close still returns Ok.
    #[test]
    fn quirk_second_chunk_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let (_server, client) = serve(root.path());

        let (mut w, done) = client.create("split").unwrap();
        w.write_all(b"hello ").unwrap();
        w.write_all(b"world").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        ));
        assert!(!client.list().unwrap().contains(&"split".to_string()));
    }

    /// Not in Go's tests. A 4 MiB chunk with its framing is larger than gRPC's
    /// default 4 MiB receive limit, so the server refuses it and the blob is
    /// not created (Q-WP210-2), as in Go.
    #[test]
    fn quirk_full_size_chunk_exceeds_grpc_limit() {
        let root = tempfile::tempdir().unwrap();
        let (_server, client) = serve(root.path());

        let (mut w, done) = client.create("full").unwrap();
        w.write_all(&vec![1u8; 4 * 1024 * 1024]).unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        ));
        assert!(!client.list().unwrap().contains(&"full".to_string()));
    }
}
