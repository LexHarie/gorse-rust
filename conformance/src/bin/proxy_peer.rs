//! The Rust peer of the proxy wire check (WP-2-12). `tests/proxy_wire.rs` runs
//! it against Go's proxies through `contracts/tools/proxygolden`, in both
//! directions. Ports: `contracts/tools/proxygolden`.
//!
//! - `proxy_peer serve` serves the Rust proxy servers on a free local port,
//!   prints `listening <addr>`, and exits when stdin closes.
//! - `proxy_peer client <addr>` runs the scenario through the Rust proxy clients
//!   on `<addr>`, and prints the transcript.

#[path = "../../../crates/gorse-storage/tests/common/proxy_scenario.rs"]
mod proxy_scenario;

use std::error::Error;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

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
        Some("serve") => serve().await,
        Some("client") => match args.get(2) {
            Some(addr) => client(addr).await,
            None => Err("client needs the address to connect to".into()),
        },
        _ => Err("usage: proxy_peer serve | proxy_peer client <addr>".into()),
    }
}

/// Opens a fresh backend of each store under `dir`, as `serveGo` does in the
/// oracle, and initialises it.
async fn backends(
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

    let mut cache_registry = cache::Registry::default();
    cache::register_builtin(&mut cache_registry);
    let cache_path = format!("sqlite://{}/cache.db", dir.display());
    let cache_backend: Arc<dyn cache::Database> =
        Arc::from(cache_registry.open(&cache_path, "gorse_", &[]).await?);
    cache_backend.init().await?;

    let vectors_path = format!("xvec://{}/xvec", dir.display());
    let vectors_backend: Arc<dyn vectors::Database> =
        Arc::from(vectors::open(&vectors_path, "gorse_", &[]).await?);
    vectors_backend.init().await?;
    Ok((data_backend, cache_backend, vectors_backend))
}

/// Serves the three Rust proxy servers until stdin closes.
async fn serve() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (data_backend, cache_backend, vectors_backend) = backends(dir.path()).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(
        Server::builder()
            .add_service(data::proxy::ProxyServer::new(data_backend).into_service())
            .add_service(cache::proxy::ProxyServer::new(cache_backend).into_service())
            .add_service(vectors::proxy::ProxyServer::new(vectors_backend).into_service())
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    println!("listening {addr}");
    tokio::task::spawn_blocking(|| std::io::stdin().read_to_end(&mut Vec::new())).await??;
    server.abort();
    Ok(())
}

/// Runs the scenario through the Rust proxy clients on `addr`.
async fn client(addr: &str) -> Result<()> {
    let channel: Channel = Endpoint::from_shared(format!("http://{addr}"))?
        .connect()
        .await?;
    let mut transcript = proxy_scenario::data_section(channel.clone()).await;
    transcript.push_str(&proxy_scenario::cache_section(channel.clone()).await);
    transcript.push_str(&proxy_scenario::vectors_section(channel).await);
    print!("{transcript}");
    Ok(())
}
