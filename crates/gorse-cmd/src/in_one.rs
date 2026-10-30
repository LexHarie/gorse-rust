// Ports: cmd/gorse-in-one/main.go @ add21b5

//! The `gorse-in-one` binary (`cmd/gorse-in-one/main.go`): a master in
//! standalone mode, which also runs the recommendation itself, with
//! `--playground` to set up a recommender system from a published dump. Go's
//! binary starts no server or worker of its own.
//!
//! Go's `setup` imports the playground data before `Serve`. Here the stores are
//! opened by `Serve` first, then the dump is restored, then the HTTP listener
//! starts. The scheduled tasks of `Serve` may run once before the import; Go's
//! order runs them after it (`porting/notes/WP-4-5.md`, deviation 6).

use std::fmt::Display;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Arg, Command};
use flate2::read::GzDecoder;
use gorse_common::gotime;
use gorse_config::config::{CONFIG_TOML, Config, get_default_config};
use gorse_master::Master;

use crate::cli;
use crate::master;
use crate::version;

/// The config a `ml-100k` playground writes (Go's `client.ConfigTOML`).
const CLIENT_CONFIG_TOML: &str = include_str!("../../../client/config.toml");

/// Where the playground datasets are published (Go's `cdn.gorse.io/example`).
const PLAYGROUND_URL: &str = "https://cdn.gorse.io/example/";

/// The moment the `ml-100k` playground's feedback was recorded: 1998-04-22
/// 23:10:38 UTC, in Unix seconds. Its time stamps move forward to now.
const ML_100K_EPOCH_SECS: i64 = 893_286_638;

/// The command of `gorse-in-one`: the master's flags, and `--playground`.
pub fn command() -> Command {
    cli::command(
        "gorse-in-one",
        "The all in one distribution of gorse recommender system.",
    )
    .arg(playground_flag())
    .arg(cli::string_flag(
        "config",
        Some('c'),
        "configuration file path",
        None,
    ))
    .arg(master::cache_path_flag())
}

/// `--playground[=name]`. A bare flag means `default`, and a following word is
/// not its value, as pflag's `NoOptDefVal` works.
fn playground_flag() -> Arg {
    Arg::new("playground")
        .long("playground")
        .help("playground mode (setup a recommender system for GitHub repositories)")
        .value_name("string")
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value("default")
}

/// Runs `gorse-in-one` with the process arguments.
pub async fn main() {
    let matches = cli::parse(command(), std::env::args_os());
    if cli::flag(&matches, "version") {
        println!("{}", version::build_info());
        return;
    }
    cli::set_logger(&matches);

    let cache_path = cli::string(&matches, "cache-path");
    let playground = matches.get_one::<String>("playground").cloned();
    let config_path = match &playground {
        Some(name) => write_playground_config(name, &cache_path),
        None => {
            let path = cli::string(&matches, "config");
            tracing::info!(config = %path, "load config");
            path
        }
    };
    let mut conf = master::load(&config_path);
    if playground.is_some() {
        use_playground_stores(&mut conf);
    }

    let master = Master::new(conf, &cache_path, true, &config_path);
    if let Err(err) = master.serve().await {
        cli::fatal("failed to start master", err);
    }
    if let Some(name) = &playground {
        import_playground(&master, name).await;
    }
    let listener = master::bind_http(&master).await;
    master::serve_http(master, listener).await;
    tracing::info!("stop gorse-in-one successfully");
}

/// Go's playground config: `$HOME/.gorse/etc/config.toml`, written over any file
/// already there. It returns the path, which the master loads.
fn write_playground_config(name: &str, cache_path: &str) -> String {
    let home = match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => cli::fatal("failed to get user home directory", "$HOME is not set"),
    };
    let etc = home.join(".gorse").join("etc");
    if let Err(err) = fs::create_dir_all(&etc) {
        cli::fatal("failed to create config directory", err);
    }
    let path = etc.join("config.toml");
    let text = if name == "ml-100k" {
        CLIENT_CONFIG_TOML
    } else {
        CONFIG_TOML
    };
    if let Err(err) = fs::write(&path, text) {
        cli::fatal("failed to write playground config", err);
    }
    println!("Generated config file: {}", path.display());
    println!("Using cache directory: {cache_path}");
    path.display().to_string()
}

/// Go's `setup`, up to the import: the playground keeps its stores in the
/// user's home directory, and uses every CPU core.
fn use_playground_stores(conf: &mut Config) {
    let defaults = get_default_config();
    conf.database.data_store = defaults.database.data_store;
    println!("Using database: {}", conf.database.data_store);
    conf.database.cache_store = defaults.database.cache_store;
    println!("Using cache: {}", conf.database.cache_store);
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    conf.master.n_jobs = i64::try_from(cores).unwrap_or(i64::MAX);
    println!("Using {cores} CPU cores");
}

/// Go's `setup`, from the download on: restores the dump into the data store,
/// and prints where the dashboard is.
async fn import_playground(master: &Master, name: &str) {
    if let Err(err) = restore_playground(master, PLAYGROUND_URL, name).await {
        cli::fatal(err.message, err.cause);
    }

    let port = master.config().master.http_port;
    println!("Welcome to Gorse Playground");
    println!();
    println!("    Dashboard:     http://127.0.0.1:{port}/overview");
    println!("    RESTful APIs:  http://127.0.0.1:{port}/apidocs");
    println!("    Documentation: https://gorse.io");
    println!();
}

/// A failed playground import: the message Go logs for the step that failed,
/// and the error the step failed with.
#[derive(Debug)]
pub struct ImportError {
    message: &'static str,
    cause: String,
}

impl ImportError {
    fn new(message: &'static str, cause: impl Display) -> Self {
        Self {
            message,
            cause: cause.to_string(),
        }
    }
}

/// Go's `setup` up to the restore: downloads the dump `name` from `base_url`,
/// decompresses it, and restores it into the data store. `base_url` ends with a
/// slash. `import_playground` passes [`PLAYGROUND_URL`]; a test passes the
/// address of a local server, which serves the same files.
///
/// # Errors
///
/// Fails with the message of the step that failed: the download, the
/// decompression or the restore.
pub async fn restore_playground(
    master: &Master,
    base_url: &str,
    name: &str,
) -> Result<(), ImportError> {
    let file = if name == "ml-100k" {
        "ml-100k.bin.gz"
    } else {
        "github.bin.gz"
    };
    let response = reqwest::get(format!("{base_url}{file}"))
        .await
        .map_err(|err| ImportError::new("failed to download playground data", err))?;
    let body = response
        .bytes()
        .await
        .map_err(|err| ImportError::new("failed to download playground data", err))?;
    let mut dump = Vec::new();
    GzDecoder::new(&body[..])
        .read_to_end(&mut dump)
        .map_err(|err| ImportError::new("failed to decompress playground data", err))?;
    let delta = (name == "ml-100k").then(playground_delta);
    master
        .restore(&dump, delta)
        .await
        .map_err(|err| ImportError::new("failed to import playground data", err))
}

/// How far the `ml-100k` feedback moves: from its recording to now.
fn playground_delta() -> gotime::Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX)
        });
    gotime::Duration(now - ML_100K_EPOCH_SECS * 1_000_000_000)
}
