// Ports: cmd/gorse-master/main.go @ add21b5; the HTTP listener of master/rest.go (`StartHttpServer`) and the /metrics and /apidocs.json routes of server/rest.go, which the master serves too

//! The `gorse-master` binary (`cmd/gorse-master/main.go`).
//!
//! Go's master ends `Serve` with `StartHttpServer`, which blocks until the
//! process is interrupted. The router is in `gorse_master::rest`; this module
//! binds the listener and adds the two routes that Go's `StartHttpServer` also
//! registers at the root, `/metrics` and `/apidocs.json`. The rest of the HTTP
//! surface Go registers (the Swagger UI page, the pprof handlers and the CORS
//! filter with the configured domains) is not served here: see
//! `porting/notes/WP-4-5.md`.

use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use clap::{Arg, Command};
use gorse_config::config::{Config, load_config, mk_dir};
use gorse_master::Master;
use tokio::net::TcpListener;

use crate::cli;
use crate::version;

/// The master's OpenAPI document, served at `/apidocs.json`. It is the frozen
/// copy of the document the pinned Go master serves.
const OPENAPI_DOCUMENT: &str = include_str!("../../../contracts/rest/master.openapi.json");

/// The command of `gorse-master`: the shared flags, `--config` and `--cache-path`.
pub fn command() -> Command {
    cli::command(
        "gorse-master",
        "The master node of gorse recommender system.",
    )
    .arg(cli::string_flag(
        "config",
        Some('c'),
        "configuration file path",
        None,
    ))
    .arg(cache_path_flag())
}

/// The `--cache-path` flag of the master and of `gorse-in-one`. Its default is
/// made when the command is built, not when the flag is read. Go's
/// `config.MkDir` runs in `init`, so the directory is created even by `--help`.
pub fn cache_path_flag() -> Arg {
    let default = mk_dir(&["master"]);
    cli::string_flag(
        "cache-path",
        None,
        "path of cache folder",
        Some(default.display().to_string()),
    )
}

/// Go's `config.LoadConfig`. A failure is fatal, as in Go.
pub fn load(path: &str) -> Config {
    match load_config(path) {
        Ok(conf) => conf,
        Err(err) => cli::fatal("failed to load config", err),
    }
}

/// Runs `gorse-master` with the process arguments.
pub async fn main() {
    let matches = cli::parse(command(), std::env::args_os());
    if cli::flag(&matches, "version") {
        println!("{}", version::build_info());
        return;
    }
    cli::set_logger(&matches);

    let config_path = cli::string(&matches, "config");
    tracing::info!(config = %config_path, "load config");
    let conf = load(&config_path);
    let cache_path = cli::string(&matches, "cache-path");
    let master = Master::new(conf, &cache_path, false, &config_path);
    serve(master).await;
    tracing::info!("stop gorse master successfully");
}

/// Go's `Serve` followed by `StartHttpServer`: starts the master, then serves
/// HTTP until the process is interrupted, and shuts the master down.
pub async fn serve(master: Arc<Master>) {
    if let Err(err) = master.serve().await {
        cli::fatal("failed to start master", err);
    }
    let listener = bind_http(&master).await;
    serve_http(master, listener).await;
}

/// Binds the master's HTTP listener on the configured address. A failure is
/// fatal, as Go's `ListenAndServe` failure is.
pub async fn bind_http(master: &Master) -> TcpListener {
    let cfg = master.config();
    let address = format!("{}:{}", cfg.master.http_host, cfg.master.http_port);
    tracing::info!(url = %format!("http://{address}"), "start http server");
    match TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(err) => cli::fatal("failed to start http server", err),
    }
}

/// Serves HTTP on `listener` until SIGINT, then shuts the master down. Go's
/// `Shutdown` stops the HTTP server first, so this does too.
pub async fn serve_http(master: Arc<Master>, listener: TcpListener) {
    let app = app(master.clone());
    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(cli::interrupted())
        .await
    {
        cli::fatal("failed to serve http", err);
    }
    master.shutdown().await;
}

/// The master's application router: the REST routes and the dashboard, then
/// the two root routes Go's `StartHttpServer` adds.
fn app(master: Arc<Master>) -> Router {
    gorse_master::rest::router(master)
        .route("/apidocs.json", get(apidocs))
        .route("/metrics", get(metrics))
}

/// Go's `/apidocs.json`: the OpenAPI document.
async fn apidocs() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        OPENAPI_DOCUMENT,
    )
}

/// Go's `/metrics`: the Prometheus exposition of the default registry.
async fn metrics() -> impl IntoResponse {
    use prometheus::Encoder;
    let mut buffer = Vec::new();
    let families = prometheus::gather();
    let encoder = prometheus::TextEncoder::new();
    let _ = encoder.encode(&families, &mut buffer);
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_owned())],
        buffer,
    )
}
