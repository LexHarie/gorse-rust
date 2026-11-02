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

/// Serves a data store with two items of 3 MiB each, so that their
/// `get_latest_items` reply is about 6 MiB. The temporary directory holds the
/// database, so the caller keeps it alive.
#[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
async fn large_items_channel() -> (Channel, tempfile::TempDir) {
    use gorse_common::gocontext::Context;
    use gorse_common::gotime;
    use gorse_storage::data::proxy::ProxyServer;
    use gorse_storage::data::types::Item;
    use gorse_storage::data::{self, Registry, sql};

    let dir = tempfile::tempdir().expect("temp dir");
    let mut registry = Registry::default();
    sql::register(&mut registry);
    let path = format!("sqlite://{}/data.db", dir.path().display());
    let backend: Arc<dyn data::Database> =
        Arc::from(registry.open(&path, "gorse_", &[]).await.expect("open"));
    backend.init().await.expect("init");
    let comment = "x".repeat(3 << 20);
    let items: Vec<Item> = ["i1", "i2"]
        .into_iter()
        .map(|item_id| Item {
            item_id: item_id.into(),
            timestamp: gotime::unix(1704164645, 0).utc(),
            comment: comment.clone(),
            ..Default::default()
        })
        .collect();
    backend
        .batch_insert_items(&Context::background(), &items)
        .await
        .expect("insert the items");
    let router = Server::builder().add_service(ProxyServer::new(backend).into_service());
    (connect(router).await, dir)
}

/// Go's client refuses a reply over its receive limit, 4 MiB by default, with
/// `ResourceExhausted` and `grpc: received message larger than max (N vs.
/// 4194304)`, where N is the reply's size. The Rust client must refuse the
/// 6 MiB reply with the same code and text.
#[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
#[tokio::test]
async fn proxy_oversized_reply_is_refused_as_go_refuses_it() {
    use gorse_common::gocontext::Context;
    use gorse_storage::data::Database as _;
    use gorse_storage::data::proxy::ProxyClient;

    let (channel, _dir) = large_items_channel().await;
    let text = match ProxyClient::new(channel)
        .get_latest_items(&Context::background(), 10, &[], None)
        .await
    {
        Ok(_) => panic!("a reply over 4 MiB was accepted"),
        Err(err) => err.to_string(),
    };
    let size = text
        .strip_prefix(
            "rpc error: code = ResourceExhausted desc = grpc: received message larger than max (",
        )
        .and_then(|rest| rest.strip_suffix(" vs. 4194304)"))
        .unwrap_or_else(|| panic!("unexpected error: {text}"));
    let size: usize = size.parse().expect("the reply's size");
    assert!(size >= 6 << 20, "the reply is {size} bytes");
}

/// Go's worker raises its receive limit to 512 MiB on every call of its
/// connection, so the same 6 MiB reply reaches a worker's proxy client.
#[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
#[tokio::test]
async fn proxy_worker_client_accepts_a_reply_under_512_mib() {
    use gorse_common::gocontext::Context;
    use gorse_storage::data::Database as _;
    use gorse_storage::data::proxy::ProxyClient;

    let (channel, _dir) = large_items_channel().await;
    let items = ProxyClient::new(channel)
        .max_decoding_message_size(512 << 20)
        .get_latest_items(&Context::background(), 10, &[], None)
        .await
        .expect("the worker's limit admits the reply");
    assert_eq!(items.len(), 2);
}
