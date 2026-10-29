//! The `gorse-worker` binary: the worker node. See `gorse_cmd::worker`.

#[tokio::main]
async fn main() {
    gorse_cmd::worker::main().await;
}
