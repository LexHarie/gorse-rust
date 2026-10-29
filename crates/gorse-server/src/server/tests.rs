// Ports: server/server_test.go (TestServer_Sync and its mock master) @ add21b5

//! The meta sync of a server node, as Go's `TestServer_Sync`: a mock master
//! sends the config, and one sync opens the stores at the paths it names.
//! The config carries only the store paths; the rest of the defaults are the
//! ones `unmarshal` starts from, which is what the sync reads them with.

use std::net::SocketAddr;
use std::time::Duration;

use gorse_protocol::master_client::MasterClient;
use gorse_protocol::master_server::{Master, MasterServer};
use gorse_protocol::{Meta, NodeInfo, PushProgressRequest, PushProgressResponse};
use serde_json::json;
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Request, Response, Status};

use super::{OpenedPaths, new_server};

/// Go's `mockMaster`: it answers the meta request with a fixed meta, and rejects
/// the other calls as unimplemented, as its embedded `UnimplementedMasterServer`
/// does.
struct MockMaster {
    meta: Meta,
}

#[tonic::async_trait]
impl Master for MockMaster {
    async fn get_meta(&self, _request: Request<NodeInfo>) -> Result<Response<Meta>, Status> {
        Ok(Response::new(self.meta.clone()))
    }

    async fn push_progress(
        &self,
        _request: Request<PushProgressRequest>,
    ) -> Result<Response<PushProgressResponse>, Status> {
        Err(Status::unimplemented("push_progress"))
    }
}

/// Starts the mock master on a free local port, and returns its address.
async fn start_master(meta: Meta) -> SocketAddr {
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind the mock master")
        .local_addr()
        .expect("the mock master's address");
    tokio::spawn(
        Server::builder()
            .add_service(MasterServer::new(MockMaster { meta }))
            .serve(address),
    );
    address
}

/// Connects to the mock master, which may take a moment to listen.
async fn connect(address: SocketAddr) -> Channel {
    let endpoint = Endpoint::from_shared(format!("http://{address}")).expect("a valid endpoint");
    for _ in 0..50 {
        if let Ok(channel) = endpoint.connect().await {
            return channel;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the mock master did not start listening")
}

#[tokio::test]
async fn server_sync() {
    let dir = tempfile::tempdir().expect("temp dir");
    let data_store = format!("sqlite://{}/data.db", dir.path().display());
    let cache_store = format!("sqlite://{}/cache.db", dir.path().display());
    let config = json!({"database": {"data_store": data_store, "cache_store": cache_store}});
    let master = start_master(Meta {
        config: config.to_string(),
        ..Default::default()
    })
    .await;
    let channel = connect(master).await;
    let mut client = MasterClient::new(channel.clone());
    let server =
        new_server("127.0.0.1", master.port(), "127.0.0.1", 0, None).expect("a server node");

    let mut opened = OpenedPaths::default();
    server.sync_once(&mut client, &channel, &mut opened).await;

    assert_eq!(opened.data_path, data_store);
    assert_eq!(opened.cache_path, cache_store);
}
