//! The `gorse-master` binary: the master node. See `gorse_cmd::master`.

#[tokio::main]
async fn main() {
    gorse_cmd::master::main().await;
}
