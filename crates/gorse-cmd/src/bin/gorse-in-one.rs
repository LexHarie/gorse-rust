//! The `gorse-in-one` binary: a master, a server and a worker in one process.
//! See `gorse_cmd::in_one`.

#[tokio::main]
async fn main() {
    gorse_cmd::in_one::main().await;
}
