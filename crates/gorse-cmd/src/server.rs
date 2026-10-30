// Ports: cmd/gorse-server/main.go @ add21b5

//! The `gorse-server` binary (`cmd/gorse-server/main.go`).
//!
//! Go's `NewServer` takes a cache file that `Server` never reads (it is stored
//! and not used), so the port's constructor does not take it. The flag is still
//! defined, with its default, so the command line is the same.

use std::sync::Arc;

use clap::{Arg, Command};
use gorse_server::server::new_server;

use crate::cli;
use crate::version;

/// The command of `gorse-server`.
///
/// The help texts of `--http-host` and `--http-port` are Go's as they are
/// printed, and Go's source gives them each other's text (`porting/notes/WP-4-5.md`,
/// deviation 2).
pub fn command() -> Command {
    cli::command(
        "gorse-server",
        "The server node of gorse recommender system.",
    )
    .arg(cli::port_flag("master-port", "port of master node", 8086))
    .arg(cli::string_flag(
        "master-host",
        None,
        "host of master node",
        Some("127.0.0.1".to_owned()),
    ))
    .arg(cli::port_flag(
        "http-port",
        "host for RESTful APIs and Prometheus metrics export",
        8087,
    ))
    .arg(cli::string_flag(
        "http-host",
        None,
        "port for RESTful APIs and Prometheus metrics export",
        Some("127.0.0.1".to_owned()),
    ))
    .arg(cli::string_flag(
        "cache-path",
        None,
        "path of cache file",
        Some("server_cache.data".to_owned()),
    ))
    .arg(ssl_flag("ssl-ca", "path of SSL CA"))
    .arg(ssl_flag("ssl-cert", "path of SSL certificate"))
    .arg(ssl_flag("ssl-key", "path of SSL key"))
}

/// An SSL file flag, which has no default.
fn ssl_flag(name: &'static str, help: &'static str) -> Arg {
    cli::string_flag(name, None, help, None)
}

/// Runs `gorse-server` with the process arguments.
pub async fn main() {
    let matches = cli::parse(command(), std::env::args_os());
    if cli::flag(&matches, "version") {
        println!("{}", version::build_info());
        return;
    }
    cli::set_logger(&matches);

    let master_host = cli::string(&matches, "master-host");
    let master_port = cli::port(&matches, "master-port");
    let http_host = cli::string(&matches, "http-host");
    let http_port = cli::port(&matches, "http-port");
    let tls = cli::tls_config(&matches);
    let server = match new_server(&master_host, master_port, &http_host, http_port, tls) {
        Ok(server) => Arc::new(server),
        Err(err) => cli::fatal("failed to create server", err),
    };

    let stopping = server.clone();
    tokio::spawn(async move {
        cli::interrupted().await;
        stopping.shutdown();
    });
    if let Err(err) = server.serve().await {
        cli::fatal("failed to serve", err);
    }
    tracing::info!("stop gorse server successfully");
}
