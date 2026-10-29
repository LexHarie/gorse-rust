// Ports: master/rpc_test.go @ add21b5

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use gorse_common::gotime::{self, Month};
use gorse_common::monitor::{self, Progress, Status};
use gorse_common::util::tls::{TlsConfig, new_client_config, new_server_config};
use gorse_config::config::get_default_config;
use gorse_config::config::schema::Config;
use gorse_protocol::master_client::MasterClient;
use gorse_protocol::master_server::{Master as MasterService, MasterServer};
use gorse_protocol::{NodeInfo, NodeType};
use hyper_rustls::HttpsConnectorBuilder;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tonic::Request;
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Server, Uri};

use crate::master::Master;
use crate::rpc;
use crate::test_support::new_master;

/// The node a test registers.
fn node(node_type: NodeType, uuid: &str) -> NodeInfo {
    NodeInfo {
        node_type: node_type as i32,
        uuid: uuid.to_owned(),
        hostname: "yoga".to_owned(),
        ..Default::default()
    }
}

/// Serves the master's gRPC service on a free port, as Go's `mockMasterRPC`
/// does, with `acceptor` as the TLS of each connection when there is one. The
/// returned sender stops the server, as Go's `Stop` does.
async fn serve(
    master: Arc<Master>,
    acceptor: Option<TlsAcceptor>,
) -> (String, watch::Sender<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let address = listener
        .local_addr()
        .expect("the bound address")
        .to_string();
    let (stop, stop_rx) = watch::channel(false);
    let incoming = rpc::incoming(listener, acceptor, stop_rx.clone());
    tokio::spawn(async move {
        let mut stop_rx = stop_rx;
        Server::builder()
            .add_service(MasterServer::from_arc(master))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = stop_rx.wait_for(|stop| *stop).await;
            })
            .await
            .expect("serve the master");
    });
    (address, stop)
}

/// A channel without TLS, as Go's `grpc.WithInsecure` dials.
fn plain_channel(address: &str) -> Channel {
    Endpoint::from_shared(format!("http://{address}"))
        .expect("a valid endpoint")
        .connect_lazy()
}

/// A channel that presents `tls`'s certificate and checks the server's against
/// `tls`'s CA, as Go's `util.NewClientCreds` does. It dials through the same
/// connector the worker uses for the master.
fn tls_channel(address: &str, tls: &TlsConfig) -> Channel {
    let mut config = new_client_config(tls).expect("a client configuration");
    // The connector sets its own ALPN list, and refuses one that is already set.
    config.alpn_protocols.clear();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_only()
        .enable_http2()
        .build();
    let target: Uri = format!("https://{address}").parse().expect("a valid uri");
    Endpoint::from_shared(format!("http://{address}"))
        .expect("a valid endpoint")
        .connect_with_connector_lazy(TargetConnector { connector, target })
}

/// The connector of a TLS channel. It ignores the URI tonic passes and dials
/// `target`, so the inner connector runs the TLS handshake.
#[derive(Clone)]
struct TargetConnector<C> {
    connector: C,
    target: Uri,
}

impl<C: Service<Uri>> Service<Uri> for TargetConnector<C> {
    type Response = C::Response;
    type Error = C::Error;
    type Future = C::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.connector.poll_ready(cx)
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        self.connector.call(self.target.clone())
    }
}

/// Writes a certificate authority, and a certificate and key that it signs, into
/// `dir`, and returns their paths. Go's `generateToTempFile` does the same with
/// `testcerts`.
fn write_tls_files(dir: &std::path::Path) -> TlsConfig {
    std::fs::create_dir_all(dir).expect("create the certificate directory");
    let ca_key = KeyPair::generate().expect("a CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).expect("a CA certificate");
    let issuer = Issuer::from_params(&ca_params, &ca_key);
    let key = KeyPair::generate().expect("a key");
    let cert = CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
        .expect("certificate parameters")
        .signed_by(&key, &issuer)
        .expect("a signed certificate");
    let tls = TlsConfig {
        ssl_ca: dir.join("ca.pem").display().to_string(),
        ssl_cert: dir.join("cert.pem").display().to_string(),
        ssl_key: dir.join("key.pem").display().to_string(),
    };
    std::fs::write(&tls.ssl_ca, ca_cert.pem()).expect("write the CA");
    std::fs::write(&tls.ssl_cert, cert.pem()).expect("write the certificate");
    std::fs::write(&tls.ssl_key, key.serialize_pem()).expect("write the key");
    tls
}

/// The progress records are equal field by field. Go compares the structs.
fn assert_same_progress(got: &Progress, want: &Progress) {
    assert_eq!(got.tracer, want.tracer);
    assert_eq!(got.name, want.name);
    assert_eq!(got.status, want.status);
    assert_eq!(got.error, want.error);
    assert_eq!(got.count, want.count);
    assert_eq!(got.total, want.total);
    assert_eq!(got.start_time, want.start_time);
    assert_eq!(got.finish_time, want.finish_time);
}

/// Go's `TestRPC`: a progress push is stored under its tracer, and `GetMeta`
/// registers the node, answers with the config and the model ids, and lists the
/// nodes that checked in within the meta store's time to live.
#[tokio::test]
async fn test_rpc() {
    let dir = TempDir::new().expect("a temporary directory");
    let master = new_master(dir.path(), get_default_config(), "").await;
    master
        .collaborative_filtering
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 123;
    master
        .click_through_rate
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .meta
        .id = 456;
    let (address, stop) = serve(Arc::clone(&master), None).await;
    let mut client = MasterClient::new(plain_channel(&address));

    let progress_list = vec![Progress {
        tracer: "tracer".to_owned(),
        name: "a".to_owned(),
        status: Status::Running,
        error: String::new(),
        total: 12,
        count: 6,
        start_time: gotime::date(2018, Month::JANUARY, 1, 0, 0, 0, 0, &gotime::local()),
        finish_time: gotime::date(2018, Month::JANUARY, 2, 0, 0, 0, 0, &gotime::local()),
    }];
    client
        .push_progress(monitor::encode_progress(&progress_list))
        .await
        .expect("push the progress");
    let remote = master
        .remote_progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get("tracer")
        .cloned()
        .expect("the progress is stored under its tracer");
    assert_eq!(remote.len(), progress_list.len());
    assert_same_progress(&remote[0], &progress_list[0]);

    // test get meta
    client
        .get_meta(node(NodeType::Server, "server1"))
        .await
        .expect("server1 registers");
    let meta = client
        .get_meta(node(NodeType::Worker, "worker1"))
        .await
        .expect("worker1 registers")
        .into_inner();
    assert_eq!(meta.collaborative_filtering_model_id, 123);
    assert_eq!(meta.click_through_rate_model_id, 456);
    assert_eq!(meta.me, "worker1");
    assert_eq!(meta.servers, vec!["server1".to_owned()]);
    assert_eq!(meta.workers, vec!["worker1".to_owned()]);
    let mut cfg = Config::default();
    cfg.unmarshal_json(&meta.config)
        .expect("the config is JSON");
    assert_eq!(cfg, *master.config());

    tokio::time::sleep(Duration::from_secs(2)).await;
    let meta = client
        .get_meta(node(NodeType::Worker, "worker2"))
        .await
        .expect("worker2 registers")
        .into_inner();
    assert_eq!(meta.workers, vec!["worker2".to_owned()]);

    let _ = stop.send(true);
    master
        .meta_store()
        .close()
        .await
        .expect("close the meta database");
}

/// Go's `TestReloadConfig`: the values of the config file reach `GetMeta` after
/// a reload.
#[tokio::test]
async fn test_reload_config() {
    let dir = TempDir::new().expect("a temporary directory");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[server]\ndefault_n = 42\n").expect("write the config file");
    let master = new_master(
        dir.path(),
        get_default_config(),
        &config_path.display().to_string(),
    )
    .await;

    master
        .reload_config_from_file()
        .await
        .expect("reload the config file");

    let meta = MasterService::get_meta(
        master.as_ref(),
        Request::new(NodeInfo {
            node_type: NodeType::Server as i32,
            uuid: "server1".to_owned(),
            binary_version: "v0.0.0".to_owned(),
            hostname: "test".to_owned(),
        }),
    )
    .await
    .expect("get meta")
    .into_inner();
    let mut cfg = Config::default();
    cfg.unmarshal_json(&meta.config)
        .expect("the config is JSON");
    assert_eq!(cfg.server.default_n, 42);

    master
        .meta_store()
        .close()
        .await
        .expect("close the meta database");
}

/// Go's `TestSSL`: a TLS master serves the client whose certificate its CA
/// signed, and refuses a plain client and a client from another CA.
#[tokio::test]
async fn test_ssl() {
    let dir = TempDir::new().expect("a temporary directory");
    let tls = write_tls_files(&dir.path().join("first"));
    let acceptor = TlsAcceptor::from(Arc::new(
        new_server_config(&tls).expect("a server configuration"),
    ));
    let master = new_master(dir.path(), get_default_config(), "").await;
    let (address, stop) = serve(Arc::clone(&master), Some(acceptor)).await;

    // success
    let mut client = MasterClient::new(tls_channel(&address, &tls));
    client
        .get_meta(node(NodeType::Server, "server1"))
        .await
        .expect("a client with a certificate from the CA is served");

    // insecure
    let mut client = MasterClient::new(plain_channel(&address));
    assert!(
        client
            .get_meta(node(NodeType::Server, "server1"))
            .await
            .is_err()
    );

    // certificate mismatch
    let other = write_tls_files(&dir.path().join("second"));
    let mut client = MasterClient::new(tls_channel(&address, &other));
    assert!(
        client
            .get_meta(node(NodeType::Server, "server1"))
            .await
            .is_err()
    );

    let _ = stop.send(true);
    master
        .meta_store()
        .close()
        .await
        .expect("close the meta database");
}
