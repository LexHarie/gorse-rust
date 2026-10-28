//! The proxy stores over gRPC, checked against the transcript that Go's proxy
//! client and server write (`contracts/tools/proxygolden`, WP-2-12). Each test
//! runs one store's steps through the Rust proxy client and server on a local
//! port, and compares the lines with the golden transcript.
//!
//! The data and cache sections need their SQL features, so the module is
//! compiled in every build but only some of its sections are called.

// Ports: contracts/tools/proxygolden (the golden transcript and the scenario).

#[allow(dead_code)] // The data and cache sections need their features.
#[path = "common/proxy_scenario.rs"]
mod proxy_scenario;

use std::sync::Arc;

use tonic::transport::server::Router;
use tonic::transport::{Channel, Endpoint, Server};

/// Go's transcript of the scenario, one line per step.
const TRANSCRIPT: &str = include_str!("data/proxy_transcript.txt");

/// The golden lines of one store: those whose step name starts with `prefix`.
fn golden(prefix: &str) -> String {
    TRANSCRIPT
        .lines()
        .filter(|line| line.starts_with(prefix))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Serves `router` on a free local port, and returns a channel to it.
async fn connect(router: Router) -> Channel {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let addr = listener.local_addr().expect("local address");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(router.serve_with_incoming(incoming));
    Endpoint::from_shared(format!("http://{addr}"))
        .expect("endpoint")
        .connect()
        .await
        .expect("connect to the local server")
}

#[tokio::test]
async fn proxy_vectors_match_go_transcript() {
    use gorse_storage::vectors::{self, proxy::ProxyServer};

    let dir = tempfile::tempdir().expect("temp dir");
    let path = format!("xvec://{}", dir.path().join("xvec").display());
    let backend: Arc<dyn vectors::Database> =
        Arc::from(vectors::open(&path, "gorse_", &[]).await.expect("open"));
    backend.init().await.expect("init");
    let router = Server::builder().add_service(ProxyServer::new(backend).into_service());
    let channel = connect(router).await;
    assert_eq!(
        proxy_scenario::vectors_section(channel, "v").await,
        golden("v.")
    );
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn proxy_cache_matches_go_transcript() {
    use gorse_storage::cache::{self, Registry, proxy::ProxyServer, register_builtin};

    let dir = tempfile::tempdir().expect("temp dir");
    let mut registry = Registry::default();
    register_builtin(&mut registry);
    let path = format!("sqlite://{}/cache.db", dir.path().display());
    let backend: Arc<dyn cache::Database> =
        Arc::from(registry.open(&path, "gorse_", &[]).await.expect("open"));
    backend.init().await.expect("init");
    let router = Server::builder().add_service(ProxyServer::new(backend).into_service());
    let channel = connect(router).await;
    assert_eq!(proxy_scenario::cache_section(channel).await, golden("c."));
}

#[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
#[tokio::test]
async fn proxy_data_matches_go_transcript() {
    use gorse_storage::data::{self, Registry, proxy::ProxyServer, sql};

    let dir = tempfile::tempdir().expect("temp dir");
    let mut registry = Registry::default();
    sql::register(&mut registry);
    let path = format!("sqlite://{}/data.db", dir.path().display());
    let backend: Arc<dyn data::Database> =
        Arc::from(registry.open(&path, "gorse_", &[]).await.expect("open"));
    backend.init().await.expect("init");
    backend
        .reconcile(&proxy_scenario::search_config())
        .await
        .expect("reconcile");
    let router = Server::builder().add_service(ProxyServer::new(backend).into_service());
    let channel = connect(router).await;
    assert_eq!(proxy_scenario::data_section(channel).await, golden("d."));
}
