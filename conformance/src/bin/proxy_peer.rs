//! The Rust peer of the proxy wire check (WP-2-12). `tests/proxy_wire.rs` runs
//! it against Go's proxies through `contracts/tools/proxygolden`, in both
//! directions. Ports: `contracts/tools/proxygolden`.
//!
//! - `proxy_peer serve <qdrant-uri>` serves the Rust proxy servers on free local
//!   ports: the data, cache and xvec stores on the first, and the Qdrant store
//!   (a live server at `<qdrant-uri>`) on the second. It prints `listening <addr>`
//!   and `listening-qdrant <addr>`, and exits when stdin closes.
//! - `proxy_peer client <addr> <qdrant-addr>` runs the scenario through the Rust
//!   proxy clients on those addresses, and prints the transcript.

#[path = "../../../crates/gorse-storage/tests/common/proxy_scenario.rs"]
mod proxy_scenario;

use std::error::Error;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use gorse_common::gocontext::Context;
use gorse_storage::cache;
use gorse_storage::data::{self, sql};
use gorse_storage::vectors;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Endpoint, Server};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => match args.get(2) {
            Some(qdrant_uri) => serve(qdrant_uri).await,
            None => Err("serve needs the Qdrant server, such as qdrant://127.0.0.1:6334".into()),
        },
        Some("client") => match (args.get(2), args.get(3)) {
            (Some(addr), Some(qdrant_addr)) => client(addr, qdrant_addr).await,
            _ => Err("client needs the data address and the Qdrant address".into()),
        },
        _ => Err(
            "usage: proxy_peer serve <qdrant-uri> | proxy_peer client <addr> <qdrant-addr>".into(),
        ),
    }
}

/// Opens a fresh data, cache and xvec backend under `dir`, as `serveGo` does in
/// the oracle, and initialises each. The data store is reconciled for search.
async fn local_backends(
    dir: &Path,
) -> Result<(
    Arc<dyn data::Database>,
    Arc<dyn cache::Database>,
    Arc<dyn vectors::Database>,
)> {
    let mut data_registry = data::Registry::default();
    sql::register(&mut data_registry);
    let data_path = format!("sqlite://{}/data.db", dir.display());
    let data_backend: Arc<dyn data::Database> =
        Arc::from(data_registry.open(&data_path, "gorse_", &[]).await?);
    data_backend.init().await?;
    data_backend
        .reconcile(&proxy_scenario::search_config())
        .await?;

    let mut cache_registry = cache::Registry::default();
    cache::register_builtin(&mut cache_registry);
    let cache_path = format!("sqlite://{}/cache.db", dir.display());
    let cache_backend: Arc<dyn cache::Database> =
        Arc::from(cache_registry.open(&cache_path, "gorse_", &[]).await?);
    cache_backend.init().await?;

    let xvec_path = format!("xvec://{}/xvec", dir.display());
    let xvec_backend: Arc<dyn vectors::Database> =
        Arc::from(vectors::open(&xvec_path, "gorse_", &[]).await?);
    xvec_backend.init().await?;
    Ok((data_backend, cache_backend, xvec_backend))
}

/// Opens the live Qdrant store at `uri`, and deletes the "docs" collection that
/// a run which stopped early may have left there.
async fn qdrant_backend(uri: &str) -> Result<Arc<dyn vectors::Database>> {
    let backend: Arc<dyn vectors::Database> = Arc::from(vectors::open(uri, "gorse_", &[]).await?);
    backend.init().await?;
    let ctx = Context::background();
    if backend
        .list_collections(&ctx)
        .await?
        .iter()
        .any(|name| name == "docs")
    {
        backend.delete_collection(&ctx, "docs").await?;
    }
    Ok(backend)
}

/// Serves the Rust proxy servers until stdin closes.
async fn serve(qdrant_uri: &str) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (data_backend, cache_backend, xvec_backend) = local_backends(dir.path()).await?;
    let qdrant = qdrant_backend(qdrant_uri).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let main = tokio::spawn(
        Server::builder()
            .add_service(data::proxy::ProxyServer::new(data_backend).into_service())
            .add_service(cache::proxy::ProxyServer::new(cache_backend).into_service())
            .add_service(vectors::proxy::ProxyServer::new(xvec_backend).into_service())
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let qdrant_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let qdrant_addr = qdrant_listener.local_addr()?;
    let qdrant_server = tokio::spawn(
        Server::builder()
            .add_service(vectors::proxy::ProxyServer::new(qdrant).into_service())
            .serve_with_incoming(TcpListenerStream::new(qdrant_listener)),
    );

    println!("listening {addr}");
    println!("listening-qdrant {qdrant_addr}");
    tokio::task::spawn_blocking(|| std::io::stdin().read_to_end(&mut Vec::new())).await??;
    main.abort();
    qdrant_server.abort();
    Ok(())
}

/// Runs the scenario through the Rust proxy clients on `addr` (the data, cache
/// and xvec stores) and `qdrant_addr` (the Qdrant store).
async fn client(addr: &str, qdrant_addr: &str) -> Result<()> {
    let channel = connect(addr).await?;
    let qdrant_channel = connect(qdrant_addr).await?;
    let mut transcript = proxy_scenario::data_section(channel.clone()).await;
    transcript.push_str(&proxy_scenario::cache_section(channel.clone()).await);
    transcript.push_str(&proxy_scenario::vectors_section(channel, "v").await);
    transcript.push_str(&proxy_scenario::vectors_section(qdrant_channel, "q").await);
    print!("{transcript}");
    Ok(())
}

async fn connect(addr: &str) -> Result<Channel> {
    Ok(Endpoint::from_shared(format!("http://{addr}"))?
        .connect()
        .await?)
}
