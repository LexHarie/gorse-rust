//! The `gorse-server` binary: the server node. See `gorse_cmd::server`.

#[tokio::main]
async fn main() {
    gorse_cmd::server::main().await;
}
