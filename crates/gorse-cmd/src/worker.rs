// Ports: cmd/gorse-worker/main.go @ add21b5

//! The `gorse-worker` binary (`cmd/gorse-worker/main.go`).
//!
//! Go's `NewWorker` takes a cache file that `Worker` never reads, so the port's
//! options do not carry it. The flag is still defined, with its default.

use std::sync::Arc;

use clap::{Arg, Command, value_parser};
use gorse_worker::{Worker, WorkerOptions};

use crate::cli;
use crate::version;

/// The command of `gorse-worker`.
pub fn command() -> Command {
    cli::command(
        "gorse-worker",
        "The worker node of gorse recommender system.",
    )
    .arg(cli::string_flag(
        "master-host",
        None,
        "host of master node",
        Some("127.0.0.1".to_owned()),
    ))
    .arg(cli::port_flag("master-port", "port of master node", 8086))
    .arg(cli::string_flag(
        "http-host",
        None,
        "host for Prometheus metrics export",
        Some("127.0.0.1".to_owned()),
    ))
    .arg(cli::port_flag(
        "http-port",
        "port for Prometheus metrics export",
        8089,
    ))
    .arg(jobs_flag())
    .arg(cli::string_flag(
        "cache-path",
        None,
        "path of cache file",
        Some("worker_cache.data".to_owned()),
    ))
    .arg(ssl_flag("ssl-ca", "path of SSL CA"))
    .arg(ssl_flag("ssl-cert", "path to SSL certificate"))
    .arg(ssl_flag("ssl-key", "path to SSL key"))
    .arg(cli::duration_flag(
        "interval",
        "interval between checking users",
        "1m0s",
    ))
}

/// `-j, --jobs`: the number of users a recommendation run ranks at a time.
fn jobs_flag() -> Arg {
    Arg::new("jobs")
        .short('j')
        .long("jobs")
        .help("number of working jobs.")
        .value_name("int")
        .value_parser(value_parser!(usize))
        .default_value("1")
}

/// An SSL file flag, which has no default.
fn ssl_flag(name: &'static str, help: &'static str) -> Arg {
    cli::string_flag(name, None, help, None)
}

/// Runs `gorse-worker` with the process arguments.
pub async fn main() {
    let matches = cli::parse(command(), std::env::args_os());
    if cli::flag(&matches, "version") {
        println!("{}", version::build_info());
        return;
    }
    cli::set_logger(&matches);

    let options = WorkerOptions {
        master_host: cli::string(&matches, "master-host"),
        master_port: cli::port(&matches, "master-port"),
        http_host: cli::string(&matches, "http-host"),
        http_port: cli::port(&matches, "http-port"),
        jobs: matches.get_one::<usize>("jobs").copied().unwrap_or(1),
        tls: cli::tls_config(&matches),
        tick_interval: matches
            .get_one::<std::time::Duration>("interval")
            .copied()
            .unwrap_or_default(),
    };
    let worker = match Worker::new(options) {
        Ok(worker) => Arc::new(worker),
        Err(err) => cli::fatal("failed to create worker", err),
    };

    let stopping = worker.clone();
    tokio::spawn(async move {
        cli::interrupted().await;
        stopping.shutdown();
    });
    if let Err(err) = worker.serve().await {
        cli::fatal("failed to serve", err);
    }
    tracing::info!("stop gorse worker successfully");
}
