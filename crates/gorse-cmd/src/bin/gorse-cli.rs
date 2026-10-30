//! The `gorse-cli` binary. See `gorse_cmd::gorse_cli`.

#[tokio::main]
async fn main() {
    gorse_cmd::gorse_cli::main().await;
}
