// Ports: cmd/gorse-bench/main.go (the cobra command tree, its flags and its help; runRankingBenchmark and benchEmbeddingCmd load the dataset) @ add21b5

//! The `gorse-bench` benchmark tool: the `reranker`, `decision` and `embedding`
//! subcommands, the cobra command tree they hang from, and the load of the
//! dataset they share.
//!
//! The flags are built with the builder API, as the other binaries' are, so
//! `tests/cli_contract.rs` can check them against
//! `contracts/cli/gorse-bench.help.txt`. The help text is laid out by
//! [`help_text`], not by clap, because the contract is the output of cobra.

use std::fmt::Write as _;

use clap::parser::ValueSource;
use clap::{Arg, ArgAction, ArgMatches, Command};
use gorse_common::gocontext::Context;
use gorse_config::config::load::load_config;
use gorse_config::config::schema::Config;
use gorse_dataset::dataset::Dataset;
use gorse_master::Master;
use gorse_model::ctr::data::Dataset as CtrDataset;
use gorse_storage::data;
use gorse_storage_core::options::with_isolation_level;

use crate::bench_embedding;
use crate::bench_ranking;
use crate::cli;

/// The name of the root command, which the help text and the usage lines print.
pub const NAME: &str = "gorse-bench";

/// The flags cobra calls persistent: the root's, which every command accepts.
const PERSISTENT: [&str; 2] = ["config", "jobs"];

/// Builds the root command with its subcommands in the order of `main.go`. The
/// cobra-generated `completion` and `help` commands are here too, because the
/// help text lists them.
pub fn command() -> Command {
    Command::new(NAME)
        .about("Gorse command line tool")
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        .args(persistent_flags())
        .arg(help_flag(NAME))
        .subcommand(ranking_command(
            "reranker",
            "Benchmark reranker models for ranking",
        ))
        .subcommand(ranking_command(
            "decision",
            "Benchmark decision models for ranking",
        ))
        .subcommand(embedding_command())
        .subcommand(completion_command())
        .subcommand(help_command())
}

/// The persistent flags, `-c/--config` and `-j/--jobs`. Go's default for
/// `--jobs` is `runtime.NumCPU()`, which is read when the command is built.
fn persistent_flags() -> Vec<Arg> {
    vec![
        cli::string_flag("config", Some('c'), "Path to configuration file", None),
        cli::int_flag(
            "jobs",
            Some('j'),
            "Number of jobs to run in parallel",
            Some(num_cpu() as i64),
        ),
    ]
}

/// cobra's `-h, --help` of a command, whose help is `help for <name>`.
fn help_flag(name: &str) -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::SetTrue)
        .help(format!("help for {name}"))
}

/// A `reranker` or `decision` command: the ranking benchmark, with its
/// `--user-auc` flag.
fn ranking_command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .disable_help_flag(true)
        .args(persistent_flags())
        .arg(help_flag(name))
        .arg(cli::bool_flag(
            "user-auc",
            None,
            "Export user-level AUC scores to CSV file",
        ))
}

/// The `embedding` command and its flags, in the order of `main.go`.
fn embedding_command() -> Command {
    Command::new("embedding")
        .about("Benchmark embedding models for item-to-item")
        .disable_help_flag(true)
        .args(persistent_flags())
        .arg(help_flag("embedding"))
        .arg(cli::int_flag(
            "top",
            Some('k'),
            "Number of top items to evaluate for each user",
            Some(10),
        ))
        .arg(cli::int_flag(
            "shots",
            Some('s'),
            "Number of shots for each user",
            Some(i64::MAX),
        ))
        .arg(cli::int_flag(
            "embedding-dimensions",
            None,
            "Embedding dimensions",
            Some(0),
        ))
        .arg(cli::string_flag(
            "embedding-model",
            None,
            "Embedding model",
            None,
        ))
        .arg(cli::string_flag(
            "embedding-column",
            None,
            "Column name of embedding in item label",
            None,
        ))
        .arg(cli::string_flag(
            "text-column",
            None,
            "Column name of text in item label",
            None,
        ))
}

/// The `completion` command. cobra generates the shell scripts and this port
/// does not, so `completion` prints its help, and a shell name is an unknown
/// command.
fn completion_command() -> Command {
    Command::new("completion")
        .about("Generate the autocompletion script for the specified shell")
        .disable_help_flag(true)
        .args(persistent_flags())
        .arg(help_flag("completion"))
}

/// The `help` command: the help of the root, or of the command it names.
fn help_command() -> Command {
    Command::new("help")
        .about("Help about any command")
        .disable_help_flag(true)
        .arg(Arg::new("command").num_args(0..))
}

/// The number of CPUs, as Go's `runtime.NumCPU()` reads it.
pub fn num_cpu() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// The values of the persistent flags a benchmark runs with.
pub struct Common {
    /// The `-c/--config` path, or empty for the defaults.
    pub config: String,
    /// The `-j/--jobs` count.
    pub jobs: usize,
}

impl Common {
    /// Reads the persistent flags from the subcommand, or from the root when the
    /// flag was given before the subcommand name. cobra accepts both places.
    fn from(root: &ArgMatches, sub: &ArgMatches) -> Common {
        let source = |name: &str| {
            if sub.value_source(name) == Some(ValueSource::CommandLine) {
                sub
            } else {
                root
            }
        };
        Common {
            config: cli::string(source("config"), "config"),
            jobs: source("jobs")
                .get_one::<i64>("jobs")
                .map_or(num_cpu(), |jobs| (*jobs).max(1) as usize),
        }
    }
}

/// The entry point of the `gorse-bench` binary. It parses the arguments, runs
/// the command they name and exits, as cobra's `Execute` does.
pub fn main() {
    // Go's default logger, zap, writes to stderr, and the tables are the standard
    // output. `common/log` sends the main log to stdout, so this binary sets its
    // own subscriber.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .init();
    let matches = cli::parse(command(), std::env::args_os());
    std::process::exit(run(&matches));
}

/// Runs the command `matches` names, and returns its exit code. With no
/// subcommand the root prints its help, as Go's root `Run` does.
fn run(matches: &ArgMatches) -> i32 {
    let Some((name, sub)) = matches.subcommand() else {
        print!("{}", help_text(&command(), NAME));
        return 0;
    };
    if cli::flag(sub, "help") {
        print!("{}", help_text(&named(name), &format!("{NAME} {name}")));
        return 0;
    }
    match name {
        "reranker" | "decision" => {
            let common = Common::from(matches, sub);
            bench_ranking::run(
                &common.config,
                name == "decision",
                cli::flag(sub, "user-auc"),
            );
            0
        }
        "embedding" => {
            let common = Common::from(matches, sub);
            bench_embedding::run(&common, &EmbeddingFlags::from(sub));
            0
        }
        "completion" => {
            print!("{}", help_text(&named(name), &format!("{NAME} {name}")));
            0
        }
        "help" => {
            let topic = sub
                .get_many::<String>("command")
                .and_then(|mut words| words.next().cloned());
            match topic {
                None => print!("{}", help_text(&command(), NAME)),
                Some(topic) => match command().find_subcommand(&topic) {
                    Some(_) => print!("{}", help_text(&named(&topic), &format!("{NAME} {topic}"))),
                    None => {
                        cli::fatal("unknown help topic", topic);
                    }
                },
            }
            0
        }
        _ => unreachable!("clap accepts only the subcommands of the command"),
    }
}

/// Go's `log.Logger().Fatal(message)`: logs the message and exits with 1.
pub fn fatal(message: &str) -> ! {
    tracing::error!("{message}");
    std::process::exit(cli::FATAL)
}

/// The subcommand `name` of the root, as [`command`] builds it.
fn named(name: &str) -> Command {
    command()
        .find_subcommand(name)
        .cloned()
        .unwrap_or_else(|| unreachable!("the root has the subcommand {name}"))
}

/// The flags of the `embedding` command.
pub struct EmbeddingFlags {
    /// `-k/--top`: the number of top items each user is evaluated on.
    pub top: i64,
    /// `-s/--shots`: the number of latest feedback kept as shots for each user.
    pub shots: i64,
    /// `--embedding-dimensions`: written to the OpenAI config, zero for the
    /// model's default.
    pub dimensions: i64,
    /// `--embedding-model`, when the flag was given. It overrides the model of
    /// the OpenAI config, as Go's `Flags().Changed` check does.
    pub model: Option<String>,
    /// `--embedding-column`: an expression that gives each item's embedding.
    pub embedding_column: String,
    /// `--text-column`: an expression that gives each item's text.
    pub text_column: String,
}

impl EmbeddingFlags {
    fn from(matches: &ArgMatches) -> EmbeddingFlags {
        let given = matches.value_source("embedding-model") == Some(ValueSource::CommandLine);
        EmbeddingFlags {
            top: matches.get_one::<i64>("top").copied().unwrap_or(10),
            shots: matches.get_one::<i64>("shots").copied().unwrap_or(i64::MAX),
            dimensions: matches
                .get_one::<i64>("embedding-dimensions")
                .copied()
                .unwrap_or(0),
            model: given.then(|| cli::string(matches, "embedding-model")),
            embedding_column: cli::string(matches, "embedding-column"),
            text_column: cli::string(matches, "text-column"),
        }
    }
}

/// The dataset a benchmark runs on: what Go's `LoadDataFromDatabase` returns
/// for the CTR split and the CF split (`dataset` and `cfDataset`), with the
/// config it was loaded from.
pub struct Loaded {
    /// The config, as `config.LoadConfig` reads it.
    pub config: Config,
    /// The CTR dataset: the users, the items and the feedback as samples.
    pub ctr: CtrDataset,
    /// The CF dataset, whose items are the ones the rankers see.
    pub cf: Dataset,
}

/// Loads the config at `path`, opens the data store it names and loads the
/// dataset from it, in the order `runRankingBenchmark` and `benchEmbeddingCmd`
/// do. Any failure is fatal, as Go's `log.Logger().Fatal` is.
pub fn load(path: &str) -> Loaded {
    let config = match load_config(path) {
        Ok(config) => config,
        Err(err) => cli::fatal("failed to load config", err),
    };
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    runtime.block_on(async {
        let cache_folder = std::env::temp_dir();
        let master = Master::new(config.clone(), &cache_folder.to_string_lossy(), false, path);
        let mut registry = data::Registry::default();
        data::register_builtin(&mut registry);
        let database = match registry
            .open(
                &config.database.data_store,
                &config.database.data_table_prefix,
                &[with_isolation_level(
                    config.database.mysql.isolation_level.clone(),
                )],
            )
            .await
        {
            Ok(database) => database,
            Err(err) => cli::fatal("failed to open data client", err),
        };
        let parent = Context::background();
        match master
            .load_benchmark_dataset(&parent, database.as_ref(), &config)
            .await
        {
            Ok((ctr, cf, _)) => Loaded { config, ctr, cf },
            Err(err) => cli::fatal("failed to load dataset", err),
        }
    })
}

/// The help text of `command`, laid out as cobra lays it out: the description,
/// the usage lines, the subcommands, and the flags. The root's flags are all
/// under `Flags:`. A subcommand lists the persistent flags under
/// `Global Flags:`. `path` is the command's invocation, such as
/// `gorse-bench reranker`.
pub fn help_text(command: &Command, path: &str) -> String {
    // cobra lists the commands by name, and pflag lists the flags by name.
    let mut subcommands: Vec<&Command> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .collect();
    subcommands.sort_by(|a, b| a.get_name().cmp(b.get_name()));
    let mut flags: Vec<&Arg> = command
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set() && arg.get_long().is_some())
        .collect();
    flags.sort_by_key(|arg| arg.get_long());
    let (global, local): (Vec<&Arg>, Vec<&Arg>) = if path == NAME {
        (Vec::new(), flags)
    } else {
        flags
            .into_iter()
            .partition(|arg| PERSISTENT.contains(&arg.get_id().as_str()))
    };

    let mut out = String::new();
    let about = command
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    writeln!(out, "{about}").expect("write to a string");
    out.push_str("\nUsage:\n");
    writeln!(out, "  {path} [flags]").expect("write to a string");
    if !subcommands.is_empty() {
        writeln!(out, "  {path} [command]").expect("write to a string");
        out.push_str("\nAvailable Commands:\n");
        let width = subcommands
            .iter()
            .map(|sub| sub.get_name().len())
            .max()
            .unwrap_or(0)
            + 2;
        for sub in &subcommands {
            let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
            writeln!(out, "  {:<width$}{about}", sub.get_name()).expect("write to a string");
        }
    }
    if !local.is_empty() {
        out.push_str("\nFlags:\n");
        out.push_str(&flag_lines(&local));
    }
    if !global.is_empty() {
        out.push_str("\nGlobal Flags:\n");
        out.push_str(&flag_lines(&global));
    }
    if !subcommands.is_empty() {
        writeln!(
            out,
            "\nUse \"{NAME} [command] --help\" for more information about a command."
        )
        .expect("write to a string");
    }
    out
}

/// The flag lines of a help text. Each is the flag, such as `  -c, --config
/// string`, and then its help, which starts three columns past the longest
/// flag, as pflag lays them out. A default is printed unless it is the zero
/// value of its type.
fn flag_lines(args: &[&Arg]) -> String {
    let lines: Vec<(String, String)> = args
        .iter()
        .map(|arg| {
            let long = arg.get_long().unwrap_or_default();
            let mut left = match arg.get_short() {
                Some(short) => format!("  -{short}, --{long}"),
                None => format!("      --{long}"),
            };
            if let Some(names) = arg.get_value_names() {
                left.push(' ');
                left.push_str(names[0].as_str());
            }
            let mut right = arg.get_help().map(ToString::to_string).unwrap_or_default();
            if let Some(default) = default_text(arg) {
                write!(right, " (default {default})").expect("write to a string");
            }
            (left, right)
        })
        .collect();
    let width = lines.iter().map(|(left, _)| left.len()).max().unwrap_or(0) + 3;
    lines
        .iter()
        .map(|(left, right)| format!("{left:<width$}{right}\n"))
        .collect()
}

/// The default a flag's help prints, as pflag prints it: a string in quotes, a
/// number bare, and nothing for the zero value of the type.
fn default_text(arg: &Arg) -> Option<String> {
    let value = arg
        .get_default_values()
        .first()?
        .to_string_lossy()
        .into_owned();
    let is_string = arg
        .get_value_names()
        .is_some_and(|names| names[0].as_str() == "string");
    match (is_string, value.as_str()) {
        (_, "") | (false, "0") => None,
        (true, value) => Some(format!("\"{value}\"")),
        (false, value) => Some(value.to_owned()),
    }
}

#[cfg(test)]
mod tests;
