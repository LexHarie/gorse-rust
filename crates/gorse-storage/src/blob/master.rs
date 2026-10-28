// Ports: storage/blob/blob.go @ add21b5 (MasterStoreServer, MasterStoreClient), storage/blob/blob_test.go @ add21b5

//! The master's `BlobStore` gRPC service, and the client a worker or server
//! uses to reach it when `blob.uri` is a local directory on the master.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use gorse_protocol::blob_store_client::BlobStoreClient;
use gorse_protocol::blob_store_server::BlobStore;
use gorse_protocol::{
    DownloadBlobRequest, DownloadBlobResponse, ListBlobsRequest, ListBlobsResponse,
    RemoveBlobRequest, RemoveBlobResponse, UploadBlobRequest, UploadBlobResponse,
};
use tokio::runtime::Runtime;
use tonic::transport::Channel;
use tonic::{Request, Response, Status, Streaming};

use super::{BlobError, BlobWriter, Store};

/// Serves the blobs of one directory over gRPC (Go's `blob.MasterStoreServer`).
#[derive(Debug)]
pub struct MasterStoreServer {
    dir: PathBuf,
}

impl MasterStoreServer {
    /// Creates the server over `dir`, creating the directory if needed.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        unimplemented!("RED: master server new {:?}", dir.into())
    }
}

#[tonic::async_trait]
impl BlobStore for MasterStoreServer {
    async fn upload_blob(
        &self,
        request: Request<Streaming<UploadBlobRequest>>,
    ) -> Result<Response<UploadBlobResponse>, Status> {
        unimplemented!("RED: upload_blob {:?}", request.metadata())
    }

    type DownloadBlobStream =
        tokio_stream::wrappers::ReceiverStream<Result<DownloadBlobResponse, Status>>;

    async fn download_blob(
        &self,
        request: Request<DownloadBlobRequest>,
    ) -> Result<Response<Self::DownloadBlobStream>, Status> {
        unimplemented!("RED: download_blob {:?}", request.get_ref().name)
    }

    async fn list_blobs(
        &self,
        request: Request<ListBlobsRequest>,
    ) -> Result<Response<ListBlobsResponse>, Status> {
        unimplemented!("RED: list_blobs {:?}", request.metadata())
    }

    async fn remove_blob(
        &self,
        request: Request<RemoveBlobRequest>,
    ) -> Result<Response<RemoveBlobResponse>, Status> {
        unimplemented!("RED: remove_blob {:?}", request.get_ref().name)
    }
}

/// A [`Store`] that reads and writes blobs through the master's gRPC service
/// (Go's `blob.MasterStoreClient`). It owns the runtime that drives the calls.
pub struct MasterStoreClient {
    client: BlobStoreClient<Channel>,
    runtime: Runtime,
}

impl MasterStoreClient {
    /// Creates a client over `channel`, which must be connected in a tokio
    /// runtime that stays alive while the client is used.
    pub fn new(channel: Channel) -> std::io::Result<Self> {
        let _ = channel;
        unimplemented!("RED: master client new")
    }
}

impl Store for MasterStoreClient {
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError> {
        unimplemented!("RED: master open {name:?}")
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        unimplemented!("RED: master create {name:?}")
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        unimplemented!("RED: master list")
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        unimplemented!("RED: master remove {name:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gorse_protocol::blob_store_server::BlobStoreServer;
    use std::io::Read;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;
    use tonic::transport::{Endpoint, Server};

    #[test]
    fn test_blob() {
        let root = tempfile::tempdir().unwrap();
        // start server
        let server_runtime = Runtime::new().unwrap();
        let channel = server_runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let service = MasterStoreServer::new(root.path().join("blob")).unwrap();
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

        // create client
        let client = MasterStoreClient::new(channel).unwrap();

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
}
