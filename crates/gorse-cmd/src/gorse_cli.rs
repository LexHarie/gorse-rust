// Ports: cmd/gorse-cli/main.go, cmd/gorse-cli/context.go, cmd/gorse-cli/format.go and cmd/gorse-cli/table.go @ add21b5

//! The `gorse-cli` command tree and its commands. Each command reads the admin
//! API through `gorse-client` and prints what the Go command prints. A failure
//! is returned as a [`Failure`], which the binary prints as Go's `fatal` does,
//! with `Error:` and the suggestions, and exits with status 1.

use std::io::{self, BufRead, Write};

use clap::{Arg, ArgAction, ArgMatches, Command};
use gorse_client::AdminClient;
use gorse_common::gotime::Duration as GoDuration;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli_context::{
    Contexts, Credentials, KEYRING_SERVICE, Keyring, SystemKeyring, go_quote, resolve,
};
use crate::cli_output::{
    format_progress_bar, format_table_value, print_array_table, print_struct, print_table,
};
use crate::cli_time::parse_rfc3339;
use crate::cli_yaml;
use crate::version;

/// The default page size of the commands that list items, users and feedback.
const DEFAULT_PAGE_SIZE: i64 = 10;

/// The columns of the Go structs a table prints when there are no rows to read
/// them from. Each list is the field names of the struct, as Go's reflection
/// reports them.
const NODE_COLUMNS: [&str; 5] = ["UUID", "Hostname", "Type", "Version", "UpdateTime"];
const USER_COLUMNS: [&str; 3] = ["UserId", "Labels", "Comment"];
const ITEM_COLUMNS: [&str; 6] = [
    "ItemId",
    "IsHidden",
    "Categories",
    "Timestamp",
    "Labels",
    "Comment",
];
const SCORED_ITEM_COLUMNS: [&str; 7] = [
    "ItemId",
    "IsHidden",
    "Categories",
    "Timestamp",
    "Labels",
    "Comment",
    "Score",
];
const SCORE_USER_COLUMNS: [&str; 4] = ["UserId", "Labels", "Comment", "Score"];
const FEEDBACK_COLUMNS: [&str; 8] = [
    "FeedbackType",
    "UserId",
    "ItemId",
    "Value",
    "Timestamp",
    "Updated",
    "Labels",
    "Comment",
];

/// A command that failed. The binary prints it as `Error: <message>`, then the
/// suggestions after a blank line, and exits with status 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What went wrong, in Go's words.
    pub message: String,
    /// The lines that tell the user what to do next, if any.
    pub suggestions: Vec<String>,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            suggestions: Vec::new(),
        }
    }

    fn with_suggestions(message: impl Into<String>, suggestions: &[&str]) -> Self {
        Self {
            message: message.into(),
            suggestions: suggestions.iter().map(|line| (*line).to_owned()).collect(),
        }
    }

    /// The text the binary prints for this failure, as Go's `fatal` writes it.
    pub fn render(&self) -> String {
        let mut text = format!("Error: {}\n", self.message);
        if !self.suggestions.is_empty() {
            text.push('\n');
            for line in &self.suggestions {
                text.push_str(line);
                text.push('\n');
            }
        }
        text
    }
}

/// `fatalErr`: the message, and the error after it.
fn fatal_err(message: &str, err: impl std::fmt::Display) -> Failure {
    Failure::new(format!("{message}: {err}"))
}

fn io_failure(err: io::Error) -> Failure {
    Failure::new(err.to_string())
}

/// The environment a command runs in: the keyring, and the endpoint and key that
/// `GORSE_ADMIN_ENDPOINT` and `GORSE_ADMIN_API_KEY` held when the CLI started.
pub struct Environment {
    /// The keyring that holds the contexts.
    pub keyring: Box<dyn Keyring>,
    /// The value of `GORSE_ADMIN_ENDPOINT`, or empty.
    pub endpoint: String,
    /// The value of `GORSE_ADMIN_API_KEY`, or empty.
    pub api_key: String,
    /// Whether standard input is a terminal, so a secret is read without echo.
    pub interactive: bool,
}

/// What a command reads and writes.
struct Run<'a> {
    env: &'a Environment,
    stdin: &'a mut dyn BufRead,
    out: &'a mut dyn Write,
}

/// Runs `gorse-cli` with `args`, which are the words after the program name.
/// Its output goes to `out`, and reading goes to `stdin`. A failure is returned,
/// not printed.
pub async fn execute(
    args: &[String],
    env: &Environment,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut words = vec!["gorse-cli".to_owned()];
    words.extend(args.iter().cloned());
    let matches = match command().try_get_matches_from(words) {
        Ok(matches) => matches,
        Err(err) => {
            if err.use_stderr() {
                return Err(Failure::new(first_line(&err.to_string())));
            }
            out.write_all(err.to_string().as_bytes())
                .map_err(io_failure)?;
            return Ok(());
        }
    };
    let mut run = Run { env, stdin, out };
    match matches.subcommand() {
        None => {
            if matches.get_flag("version") {
                writeln!(run.out, "{}", version::build_info()).map_err(io_failure)
            } else {
                print_help(run.out, &["gorse-cli"])
            }
        }
        Some((name, sub)) => dispatch(&mut run, name, sub).await,
    }
}

/// The first line of a clap error, without its `error: ` prefix.
fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .to_owned()
}

async fn dispatch(run: &mut Run<'_>, name: &str, sub: &ArgMatches) -> Result<(), Failure> {
    match name {
        "context" | "get" | "recommend" | "pipeline" => {
            let parent = format!("gorse-cli {name}");
            match sub.subcommand() {
                None => print_help(run.out, &["gorse-cli", name]),
                Some((child, child_matches)) => {
                    if !is_known_child(name, child) {
                        return Err(unknown_command(child, &parent));
                    }
                    let path = format!("{parent} {child}");
                    match name {
                        "context" => context_command(run, child, child_matches, &path),
                        "get" => get_command(run, child, child_matches, &path).await,
                        "recommend" => recommend_command(run, child, child_matches, &path).await,
                        _ => pipeline_command(run, child, child_matches, &path).await,
                    }
                }
            }
        }
        "cluster-info" => cluster_info(run, sub, "gorse-cli cluster-info").await,
        "stats" => stats(run, sub, "gorse-cli stats").await,
        "ps" => ps(run, sub, "gorse-cli ps").await,
        "dump" => dump(run, sub, "gorse-cli dump").await,
        "restore" => restore(run, sub, "gorse-cli restore").await,
        other => Err(unknown_command(other, "gorse-cli")),
    }
}

/// Whether `child` is a subcommand of `parent`, by the command tree.
fn is_known_child(parent: &str, child: &str) -> bool {
    let tree = command();
    tree.find_subcommand(parent)
        .is_some_and(|parent| parent.find_subcommand(child).is_some())
}

/// Cobra's error for a word that is not a command of the one it follows.
fn unknown_command(word: &str, path: &str) -> Failure {
    Failure::new(format!(
        "unknown command {} for {}",
        go_quote(word),
        go_quote(path)
    ))
}

/// Writes the help of the command at `path`, from the command tree.
fn print_help(out: &mut dyn Write, path: &[&str]) -> Result<(), Failure> {
    let mut command = command();
    for name in &path[1..] {
        command = command
            .find_subcommand(name)
            .cloned()
            .ok_or_else(|| Failure::new(format!("no command {name}")))?;
    }
    let text = command.render_help().to_string();
    out.write_all(text.as_bytes()).map_err(io_failure)
}

/// Checks the positional words of a command, as cobra's `Args` do.
fn check_args(args: &[String], path: &str, rule: Args) -> Result<(), Failure> {
    let received = args.len();
    match rule {
        Args::None => match args.first() {
            None => Ok(()),
            Some(word) => Err(unknown_command(word, path)),
        },
        Args::Exact(count) if received != count => Err(Failure::new(format!(
            "accepts {count} arg(s), received {received}"
        ))),
        Args::Range(min, max) if received < min || received > max => Err(Failure::new(format!(
            "accepts between {min} and {max} arg(s), received {received}"
        ))),
        _ => Ok(()),
    }
}

/// A command's rule for its positional words.
#[derive(Clone, Copy)]
enum Args {
    None,
    Exact(usize),
    Range(usize, usize),
}

/// The positional words of a command, which are all in the `args` argument.
fn words(matches: &ArgMatches) -> Vec<String> {
    matches
        .get_many::<String>("args")
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

/// The value of a string flag, or empty when the flag is not set or not defined.
fn flag_text(matches: &ArgMatches, name: &str) -> String {
    matches
        .try_get_one::<String>(name)
        .ok()
        .flatten()
        .cloned()
        .unwrap_or_default()
}

/// The value of an integer flag, which is zero when it is not set.
fn flag_int(matches: &ArgMatches, name: &str) -> i64 {
    matches
        .try_get_one::<i64>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(0)
}

/// The values of a repeatable flag.
fn flag_list(matches: &ArgMatches, name: &str) -> Vec<String> {
    matches
        .try_get_many::<String>(name)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

/// The endpoint and key for a command: its flags, then the environment, then
/// the contexts. Go's `requireEndpointAndKey` fails when either is still missing.
fn client(run: &Run<'_>, matches: &ArgMatches, path: &str) -> Result<AdminClient, Failure> {
    let flags = Credentials {
        endpoint: non_empty(flag_text(matches, "endpoint"), &run.env.endpoint),
        api_key: non_empty(flag_text(matches, "api-key"), &run.env.api_key),
    };
    let contexts = Contexts::new(run.env.keyring.as_ref(), KEYRING_SERVICE);
    let credentials = resolve(&contexts, flags, &flag_text(matches, "context")).map_err(
        |(message, suggestions)| Failure {
            message,
            suggestions,
        },
    )?;
    let endpoint_missing = credentials.endpoint.is_empty();
    let key_missing = credentials.api_key.is_empty();
    if endpoint_missing && key_missing {
        return Err(Failure::with_suggestions(
            "no Gorse context is selected and no endpoint/API key was provided.",
            &[
                "Create a context:",
                "  gorse-cli context add dev --endpoint http://localhost:8088",
                "Or use environment variables:",
                &format!(
                    "  GORSE_ADMIN_ENDPOINT=http://localhost:8088 GORSE_ADMIN_API_KEY=<api-key> {path}"
                ),
                "Or pass credentials for this command:",
                &format!("  {path} --endpoint http://localhost:8088 --api-key <api-key>"),
            ],
        ));
    }
    if endpoint_missing {
        return Err(Failure::with_suggestions(
            "missing Gorse base URL.",
            &[
                "Use a saved context:",
                "  gorse-cli context use <name>",
                "Or set an environment variable:",
                &format!("  GORSE_ADMIN_ENDPOINT=http://localhost:8088 {path}"),
                "Or pass an endpoint:",
                &format!("  {path} --endpoint http://localhost:8088"),
            ],
        ));
    }
    if key_missing {
        return Err(Failure::with_suggestions(
            "missing Gorse admin API key.",
            &[
                "Save it in a context:",
                "  gorse-cli context add <name> --endpoint http://localhost:8088",
                "Or set an environment variable:",
                &format!("  GORSE_ADMIN_API_KEY=<api-key> {path}"),
                "Or pass it for this command:",
                &format!("  {path} --api-key <api-key>"),
            ],
        ));
    }
    Ok(AdminClient::new(
        &credentials.endpoint,
        &credentials.api_key,
    ))
}

fn non_empty(flag: String, fallback: &str) -> String {
    if flag.is_empty() {
        fallback.to_owned()
    } else {
        flag
    }
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|err| Failure::new(err.to_string()))
}

/// Reads one line of confirmation. Go's `ReadString('\n')` returns what it read
/// on end of file, and only other errors fail.
fn read_line(stdin: &mut dyn BufRead) -> Result<String, Failure> {
    let mut line = String::new();
    match stdin.read_line(&mut line) {
        Ok(_) => Ok(line),
        Err(err) => Err(Failure::new(format!("failed to read confirmation: {err}"))),
    }
}

/// Reads a secret: without echo from a terminal, or a line from `stdin`.
fn read_secret(run: &mut Run<'_>, prompt: &str) -> Result<String, Failure> {
    eprint!("{prompt}");
    let secret = if run.env.interactive {
        let secret = rpassword::read_password().map_err(|err| Failure::new(err.to_string()))?;
        eprintln!();
        secret
    } else {
        let mut line = String::new();
        let read = run
            .stdin
            .read_line(&mut line)
            .map_err(|err| Failure::new(format!("failed to read API key: {err}")))?;
        if read == 0 {
            return Err(Failure::new("failed to read API key: EOF"));
        }
        line
    };
    Ok(secret.trim().to_owned())
}

/// The command tree of `gorse-cli`, with the flags and the arguments of each
/// command as the Go command defines them.
pub fn command() -> Command {
    Command::new("gorse-cli")
        .about("Gorse command line tool for cluster management")
        .disable_version_flag(true)
        .allow_external_subcommands(true)
        .arg(
            Arg::new("version")
                .short('v')
                .long("version")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("gorse-cli version"),
        )
        .subcommand(
            Command::new("context")
                .about("Manage Gorse CLI contexts")
                .allow_external_subcommands(true)
                .subcommand(
                    Command::new("add")
                        .about("Add or update a Gorse CLI context")
                        .arg(words_arg("name"))
                        .arg(connection_flag("endpoint", "Gorse base URL (default: GORSE_ADMIN_ENDPOINT)"))
                        .arg(connection_flag("api-key", "Gorse admin API key (default: GORSE_ADMIN_API_KEY; prefer prompt to avoid shell history)")),
                )
                .subcommand(connected(Command::new("list").about("List Gorse CLI contexts")))
                .subcommand(connected(Command::new("use").about("Switch the current Gorse CLI context").arg(words_arg("name"))))
                .subcommand(connected(Command::new("delete").about("Delete a Gorse CLI context").arg(words_arg("name"))))
                .subcommand(connected(Command::new("current").about("Show the current Gorse CLI context"))),
        )
        .subcommand(connected(Command::new("cluster-info").about("List cluster nodes")))
        .subcommand(connected(Command::new("stats").about("Get global statistics")))
        .subcommand(connected(Command::new("ps").about("List task progress")))
        .subcommand(
            Command::new("get")
                .about("Get items, users and feedback")
                .allow_external_subcommands(true)
                .subcommand(connected(Command::new("categories").about("Get item categories")))
                .subcommand(connected(Command::new("user").about("Get user details").arg(words_arg("user-id"))))
                .subcommand(connected(count_flag(Command::new("users").about("Get users"), "Number of returned users")))
                .subcommand(connected(Command::new("item").about("Get item details").arg(words_arg("item-id"))))
                .subcommand(connected(count_flag(
                    Command::new("items").about("Get or search items").arg(words_arg("query")),
                    "Number of returned items",
                )))
                .subcommand(connected(
                    count_flag(Command::new("feedback").about("Get feedback"), "Number of returned feedback")
                        .arg(text_flag("type", "Filter by feedback type"))
                        .arg(text_flag("user", "Filter by user ID"))
                        .arg(text_flag("item", "Filter by item ID")),
                )),
        )
        .subcommand(
            Command::new("recommend")
                .about("Get recommendations")
                .allow_external_subcommands(true)
                .subcommand(connected(count_flag(
                    Command::new("item-to-user")
                        .about("Get recommendations for a user")
                        .arg(words_arg("user-id [recommender] [name]")),
                    "Number of returned items",
                )
                .arg(category_flag())))
                .subcommand(connected(count_flag(Command::new("latest").about("Get latest items"), "Number of returned records").arg(category_flag())))
                .subcommand(connected(count_flag(
                    Command::new("non-personalized").about("Get non-personalized recommendations").arg(words_arg("name")),
                    "Number of returned records",
                )
                .arg(category_flag())
                .arg(text_flag("user-id", "Remove read items of a user"))))
                .subcommand(connected(count_flag(
                    Command::new("item-to-item").about("Get item-to-item recommendations").arg(words_arg("name item-id")),
                    "Number of returned records",
                )
                .arg(category_flag())))
                .subcommand(connected(count_flag(
                    Command::new("user-to-user").about("Get user-to-user recommendations").arg(words_arg("name user-id")),
                    "Number of returned records",
                ))),
        )
        .subcommand(
            Command::new("pipeline")
                .about("Config recommendation pipeline")
                .allow_external_subcommands(true)
                .subcommand(connected(Command::new("get").about("Get recommendation pipeline configuration")))
                .subcommand(connected(Command::new("schema").about("Get recommendation pipeline configuration schema")))
                .subcommand(connected(Command::new("patch").about("Patch recommendation pipeline configuration values").arg(words_arg("json-patch"))))
                .subcommand(connected(Command::new("reset").about("Reset recommendation pipeline configuration to the file defaults"))),
        )
        .subcommand(connected(Command::new("dump").about("Dump all Gorse data as a binary backup").arg(words_arg("file"))))
        .subcommand(connected(Command::new("restore").about("Restore Gorse data from a binary backup").arg(words_arg("file"))))
}

/// The positional words of a command, which the command reads from `args`.
fn words_arg(value_name: &'static str) -> Arg {
    Arg::new("args").value_name(value_name).num_args(0..)
}

/// The endpoint, key and context flags that the commands that call the API take.
fn connected(command: Command) -> Command {
    // A command that takes no words still reads them, so that cobra's
    // `unknown command` error for an extra word is the one the user sees.
    let takes_words = command.get_arguments().any(|arg| arg.get_id() == "args");
    let command = if takes_words {
        command
    } else {
        command.arg(Arg::new("args").num_args(0..).hide(true))
    };
    command
        .arg(connection_flag(
            "endpoint",
            "Gorse base URL (default: selected context or GORSE_ADMIN_ENDPOINT)",
        ))
        .arg(connection_flag(
            "api-key",
            "Gorse admin API key (default: selected context or GORSE_ADMIN_API_KEY)",
        ))
        .arg(connection_flag("context", "Gorse CLI context name"))
}

fn connection_flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).value_name("string").help(help)
}

fn text_flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).value_name("string").help(help)
}

fn category_flag() -> Arg {
    Arg::new("category")
        .long("category")
        .value_name("string")
        .action(ArgAction::Append)
        .help("Filter by category, repeatable")
}

/// The `-n` flag of a command that returns a page of records.
fn count_flag(command: Command, help: &'static str) -> Command {
    command.arg(
        Arg::new("n")
            .short('n')
            .long("n")
            .value_name("int")
            .value_parser(clap::value_parser!(i64))
            .default_value("0")
            .help(help),
    )
}

/// The page size a command uses: its `-n`, or ten when it is zero.
fn page_size(matches: &ArgMatches) -> i64 {
    match flag_int(matches, "n") {
        0 => DEFAULT_PAGE_SIZE,
        n => n,
    }
}

async fn cluster_info(run: &mut Run<'_>, matches: &ArgMatches, path: &str) -> Result<(), Failure> {
    check_args(&words(matches), path, Args::None)?;
    let client = client(run, matches, path)?;
    let nodes = client
        .get_cluster()
        .await
        .map_err(|err| fatal_err("admin API request failed", err))?;
    print_array_table(run.out, &to_value(&nodes)?, &NODE_COLUMNS).map_err(io_failure)
}

async fn stats(run: &mut Run<'_>, matches: &ArgMatches, path: &str) -> Result<(), Failure> {
    check_args(&words(matches), path, Args::None)?;
    let client = client(run, matches, path)?;
    let stats = client
        .get_stats()
        .await
        .map_err(|err| fatal_err("admin API request failed", err))?;
    print_struct(run.out, &to_value(&stats)?).map_err(io_failure)
}

async fn ps(run: &mut Run<'_>, matches: &ArgMatches, path: &str) -> Result<(), Failure> {
    check_args(&words(matches), path, Args::None)?;
    let client = client(run, matches, path)?;
    let tasks = client
        .get_tasks()
        .await
        .map_err(|err| fatal_err("admin API request failed", err))?;
    let rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|task| {
            vec![
                format_table_value(&Value::String(task.tracer.clone())),
                format_table_value(&Value::String(task.name.clone())),
                format_table_value(&Value::String(task.status.clone())),
                format_progress_bar(task.count, task.total),
                format_table_value(&Value::String(task.error.clone())),
                format_table_value(&Value::String(task.start_time.clone())),
                format_table_value(&Value::String(task.finish_time.clone())),
            ]
        })
        .collect();
    let headers: Vec<String> = [
        "Tracer",
        "Name",
        "Status",
        "Progress",
        "Error",
        "StartTime",
        "FinishTime",
    ]
    .iter()
    .map(|header| (*header).to_owned())
    .collect();
    print_table(run.out, &headers, &rows).map_err(io_failure)
}

async fn dump(run: &mut Run<'_>, matches: &ArgMatches, path: &str) -> Result<(), Failure> {
    let words = words(matches);
    check_args(&words, path, Args::Exact(1))?;
    let output_path = &words[0];
    let mut file = std::fs::File::create(output_path).map_err(|err| {
        fatal_err(
            &format!("failed to create dump file {}", go_quote(output_path)),
            err,
        )
    })?;
    let client = client(run, matches, path)?;
    let dump = client
        .dump()
        .await
        .map_err(|err| fatal_err("admin API request failed", err))?;
    file.write_all(&dump).map_err(|err| {
        fatal_err(
            "admin API request failed",
            format!("failed to write response: {err}"),
        )
    })?;
    writeln!(run.out, "Data dumped to {output_path}").map_err(io_failure)
}

async fn restore(run: &mut Run<'_>, matches: &ArgMatches, path: &str) -> Result<(), Failure> {
    let words = words(matches);
    check_args(&words, path, Args::Exact(1))?;
    let input_path = &words[0];
    write!(
        run.out,
        "Restore data from {input_path}? Existing users, items, feedback, and cache will be overwritten. Confirm [y/N]: "
    )
    .map_err(io_failure)?;
    let answer = read_line(run.stdin)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        return writeln!(run.out, "Restore canceled").map_err(io_failure);
    }
    let dump = std::fs::read(input_path).map_err(|err| {
        fatal_err(
            &format!("failed to open restore file {}", go_quote(input_path)),
            err,
        )
    })?;
    let client = client(run, matches, path)?;
    let stats = client
        .restore(dump)
        .await
        .map_err(|err| fatal_err("admin API request failed", err))?;
    writeln!(
        run.out,
        "Restored {} users, {} items, {} feedback in {}.",
        stats.users,
        stats.items,
        stats.feedback,
        GoDuration(stats.duration)
    )
    .map_err(io_failure)
}

/// The `context` commands. They read the keyring and never the master.
fn context_command(
    run: &mut Run<'_>,
    name: &str,
    matches: &ArgMatches,
    path: &str,
) -> Result<(), Failure> {
    let contexts = Contexts::new(run.env.keyring.as_ref(), KEYRING_SERVICE);
    match name {
        "add" => {
            let words = words(matches);
            check_args(&words, path, Args::Exact(1))?;
            let context = &words[0];
            let endpoint = non_empty(flag_text(matches, "endpoint"), &run.env.endpoint);
            let mut api_key = non_empty(flag_text(matches, "api-key"), &run.env.api_key);
            if api_key.is_empty() {
                api_key = read_secret(run, "Gorse admin API key: ").map_err(|err| {
                    Failure::new(format!("failed to read API key: {}", err.message))
                })?;
            }
            contexts.save(context, &endpoint, &api_key).map_err(|err| {
                Failure::with_suggestions(
                    format!("failed to save context {}: {err}", go_quote(context)),
                    &[
                        "Example:",
                        &format!(
                            "  gorse-cli context add {context} --endpoint http://localhost:8088"
                        ),
                    ],
                )
            })?;
            writeln!(run.out, "Context {} saved and selected.", go_quote(context))
                .map_err(io_failure)
        }
        "list" => {
            check_args(&words(matches), path, Args::None)?;
            let names = contexts.names().map_err(|err| {
                Failure::new(format!(
                    "failed to read contexts from the system keyring: {err}"
                ))
            })?;
            if names.is_empty() {
                return writeln!(run.out, "No contexts configured.").map_err(io_failure);
            }
            let current = contexts.current_name().map_err(|err| {
                Failure::new(format!(
                    "failed to read the current context from the system keyring: {err}"
                ))
            })?;
            let mut rows = Vec::with_capacity(names.len());
            for context in &names {
                let loaded = contexts.load(context).map_err(|err| {
                    Failure::new(format!(
                        "context {} is invalid or incomplete: {err}",
                        go_quote(context)
                    ))
                })?;
                let marker = if *context == current { "*" } else { "" };
                rows.push(vec![marker.to_owned(), context.clone(), loaded.endpoint]);
            }
            let headers: Vec<String> = ["Current", "Name", "Endpoint"]
                .iter()
                .map(|header| (*header).to_owned())
                .collect();
            print_table(run.out, &headers, &rows).map_err(io_failure)
        }
        "use" => {
            let words = words(matches);
            check_args(&words, path, Args::Exact(1))?;
            let context = &words[0];
            if contexts.load(context).is_err() {
                return Err(Failure::with_suggestions(
                    format!("context {} was not found.", go_quote(context)),
                    &["List available contexts:", "  gorse-cli context list"],
                ));
            }
            contexts.set_current_name(context).map_err(|err| {
                Failure::new(format!(
                    "failed to save the current context in the system keyring: {err}"
                ))
            })?;
            writeln!(run.out, "Switched to context {}.", go_quote(context)).map_err(io_failure)
        }
        "delete" => {
            let words = words(matches);
            check_args(&words, path, Args::Exact(1))?;
            let context = &words[0];
            contexts.delete(context).map_err(|_| {
                Failure::with_suggestions(
                    format!("context {} was not found.", go_quote(context)),
                    &["List available contexts:", "  gorse-cli context list"],
                )
            })?;
            writeln!(run.out, "Context {} deleted.", go_quote(context)).map_err(io_failure)
        }
        _ => {
            check_args(&words(matches), path, Args::None)?;
            let current = contexts.current_name().map_err(|err| {
                Failure::new(format!(
                    "failed to read the current context from the system keyring: {err}"
                ))
            })?;
            if current.is_empty() {
                return writeln!(run.out, "No current context.").map_err(io_failure);
            }
            let loaded = contexts.load(&current).map_err(|_| {
                Failure::with_suggestions(
                    format!(
                        "current context {} is invalid or incomplete.",
                        go_quote(&current)
                    ),
                    &[
                        "Choose another context:",
                        "  gorse-cli context use <name>",
                        "Or recreate it:",
                        &format!(
                            "  gorse-cli context add {current} --endpoint http://localhost:8088"
                        ),
                    ],
                )
            })?;
            write!(
                run.out,
                "Name:\t\t{}\nEndpoint:\t{}\n",
                loaded.name, loaded.endpoint
            )
            .map_err(io_failure)
        }
    }
}

/// The `get` commands.
async fn get_command(
    run: &mut Run<'_>,
    name: &str,
    matches: &ArgMatches,
    path: &str,
) -> Result<(), Failure> {
    let words = words(matches);
    match name {
        "categories" => {
            check_args(&words, path, Args::None)?;
            let client = client(run, matches, path)?;
            let categories = client
                .get_categories()
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&categories)?, &[]).map_err(io_failure)
        }
        "user" => {
            check_args(&words, path, Args::Exact(1))?;
            let client = client(run, matches, path)?;
            let user = client
                .get_user(&words[0])
                .await
                .map_err(|err| fatal_err("API request failed", err))?;
            print_struct(run.out, &to_value(&user)?).map_err(io_failure)
        }
        "users" => {
            check_args(&words, path, Args::None)?;
            let n = page_size(matches);
            let client = client(run, matches, path)?;
            let users = client
                .get_users(n, "")
                .await
                .map_err(|err| fatal_err("API request failed", err))?;
            print_array_table(run.out, &to_value(&users.users)?, &USER_COLUMNS).map_err(io_failure)
        }
        "item" => {
            check_args(&words, path, Args::Exact(1))?;
            let client = client(run, matches, path)?;
            let item = client
                .get_item(&words[0])
                .await
                .map_err(|err| fatal_err("API request failed", err))?;
            print_struct(run.out, &to_value(&item)?).map_err(io_failure)
        }
        "items" => {
            let n = page_size(matches);
            let client = client(run, matches, path)?;
            let items = if words.is_empty() {
                client.get_items(n, "").await
            } else {
                client.search_items(&words.join(" "), n).await
            };
            let items = items.map_err(|err| fatal_err("API request failed", err))?;
            print_array_table(run.out, &to_value(&items.items)?, &ITEM_COLUMNS).map_err(io_failure)
        }
        _ => {
            check_args(&words, path, Args::None)?;
            let n = page_size(matches);
            let feedback_type = flag_text(matches, "type");
            let user_id = flag_text(matches, "user");
            let item_id = flag_text(matches, "item");
            let client = client(run, matches, path)?;
            let mut feedback = match (
                user_id.is_empty(),
                item_id.is_empty(),
                feedback_type.is_empty(),
            ) {
                (false, false, false) => vec![
                    client
                        .get_typed_user_item_feedback(&feedback_type, &user_id, &item_id)
                        .await
                        .map_err(|err| fatal_err("admin API request failed", err))?,
                ],
                (false, false, true) => client
                    .get_user_item_feedback(&user_id, &item_id)
                    .await
                    .map_err(|err| fatal_err("admin API request failed", err))?,
                (false, true, false) => client
                    .get_typed_user_feedback(&user_id, &feedback_type)
                    .await
                    .map_err(|err| fatal_err("admin API request failed", err))?,
                (false, true, true) => client
                    .get_user_feedback(&user_id)
                    .await
                    .map_err(|err| fatal_err("admin API request failed", err))?,
                (true, false, false) => client
                    .get_typed_item_feedback(&item_id, &feedback_type)
                    .await
                    .map_err(|err| fatal_err("admin API request failed", err))?,
                (true, false, true) => client
                    .get_item_feedback(&item_id)
                    .await
                    .map_err(|err| fatal_err("admin API request failed", err))?,
                (true, true, false) => {
                    client
                        .get_typed_feedback(&feedback_type, n)
                        .await
                        .map_err(|err| fatal_err("admin API request failed", err))?
                        .feedback
                }
                (true, true, true) => {
                    client
                        .get_feedback(n)
                        .await
                        .map_err(|err| fatal_err("admin API request failed", err))?
                        .feedback
                }
            };
            // Go's `sort.Slice` is not stable; a stable sort gives the same order
            // whenever no two timestamps are equal.
            feedback.sort_by_key(|record| std::cmp::Reverse(time_key(&record.timestamp)));
            if n > 0 && (n as usize) < feedback.len() {
                feedback.truncate(n as usize);
            }
            print_array_table(run.out, &to_value(&feedback)?, &FEEDBACK_COLUMNS).map_err(io_failure)
        }
    }
}

/// The instant of an RFC 3339 time, for sorting; text that is not a time sorts first.
fn time_key(text: &str) -> i128 {
    parse_rfc3339(text).map_or(i128::MIN, |time| time.unix_nanos)
}

/// The `recommend` commands.
async fn recommend_command(
    run: &mut Run<'_>,
    name: &str,
    matches: &ArgMatches,
    path: &str,
) -> Result<(), Failure> {
    let words = words(matches);
    let categories = flag_list(matches, "category");
    match name {
        "latest" => {
            check_args(&words, path, Args::None)?;
            let n = page_size(matches);
            let client = client(run, matches, path)?;
            let latest = client
                .get_latest(n, &categories)
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&latest)?, &SCORED_ITEM_COLUMNS)
                .map_err(io_failure)
        }
        "non-personalized" => {
            check_args(&words, path, Args::Exact(1))?;
            let client = client(run, matches, path)?;
            let recommendations = client
                .get_non_personalized(
                    &words[0],
                    flag_int(matches, "n"),
                    &flag_text(matches, "user-id"),
                    &categories,
                )
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&recommendations)?, &SCORED_ITEM_COLUMNS)
                .map_err(io_failure)
        }
        "item-to-user" => {
            check_args(&words, path, Args::Range(1, 3))?;
            let n = page_size(matches);
            let recommender = words.get(1).cloned().unwrap_or_default();
            let name = words.get(2).cloned().unwrap_or_default();
            let client = client(run, matches, path)?;
            let recommendations = client
                .get_recommend(&words[0], &recommender, &name, n, &categories)
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&recommendations)?, &SCORED_ITEM_COLUMNS)
                .map_err(io_failure)
        }
        "item-to-item" => {
            check_args(&words, path, Args::Exact(2))?;
            let client = client(run, matches, path)?;
            let recommendations = client
                .get_item_to_item(&words[0], &words[1], flag_int(matches, "n"), &categories)
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&recommendations)?, &SCORED_ITEM_COLUMNS)
                .map_err(io_failure)
        }
        _ => {
            check_args(&words, path, Args::Exact(2))?;
            let client = client(run, matches, path)?;
            let recommendations = client
                .get_user_to_user(&words[0], &words[1], flag_int(matches, "n"))
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_array_table(run.out, &to_value(&recommendations)?, &SCORE_USER_COLUMNS)
                .map_err(io_failure)
        }
    }
}

/// The config section that holds the recommendation pipeline, under either name.
fn recommend_config(config: &Map<String, Value>) -> Option<(String, Value)> {
    ["recommend", "Recommend"].iter().find_map(|name| {
        config
            .get(*name)
            .map(|value| ((*name).to_owned(), value.clone()))
    })
}

/// The `pipeline` commands.
async fn pipeline_command(
    run: &mut Run<'_>,
    name: &str,
    matches: &ArgMatches,
    path: &str,
) -> Result<(), Failure> {
    let words = words(matches);
    match name {
        "get" => {
            check_args(&words, path, Args::None)?;
            let client = client(run, matches, path)?;
            let config = client
                .get_config()
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            let (_, recommend) = recommend_config(&config)
                .ok_or_else(|| Failure::new("recommend config not found"))?;
            write!(run.out, "{}", cli_yaml::encode(&recommend)).map_err(io_failure)
        }
        "schema" => {
            check_args(&words, path, Args::None)?;
            let client = client(run, matches, path)?;
            let schema = client
                .get_config_schema()
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            write!(run.out, "{}", cli_yaml::encode(&Value::Object(schema))).map_err(io_failure)
        }
        "patch" => {
            check_args(&words, path, Args::Exact(1))?;
            let client = client(run, matches, path)?;
            let mut current = client
                .get_config_map()
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            let (recommend_key, recommend_config) = recommend_config(&current)
                .ok_or_else(|| Failure::new("recommend config not found"))?;
            let mut document = recommend_config;
            let patch: json_patch::Patch = serde_json::from_str(&words[0]).map_err(|err| {
                fatal_err(
                    "failed to apply JSON patch: failed to decode JSON patch",
                    err,
                )
            })?;
            json_patch::patch(&mut document, &patch.0)
                .map_err(|err| fatal_err("failed to apply JSON patch", err))?;
            if !document.is_object() {
                return Err(fatal_err(
                    "failed to apply JSON patch: failed to decode patched config",
                    "json: cannot unmarshal into Go value of type map[string]interface {}",
                ));
            }
            current.insert(recommend_key, document);
            let updated = client
                .update_config(&current)
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            print_struct(run.out, &Value::Object(updated)).map_err(io_failure)
        }
        _ => {
            check_args(&words, path, Args::None)?;
            write!(
                run.out,
                "Reset recommendation pipeline configuration to file defaults? Current pipeline settings will be overwritten. Confirm [y/N]: "
            )
            .map_err(io_failure)?;
            let answer = read_line(run.stdin)?;
            if !answer.trim().eq_ignore_ascii_case("y") {
                return writeln!(run.out, "Pipeline reset canceled").map_err(io_failure);
            }
            let client = client(run, matches, path)?;
            let result = client
                .reset_config()
                .await
                .map_err(|err| fatal_err("admin API request failed", err))?;
            if !result.is_empty() {
                return print_struct(run.out, &Value::Object(result)).map_err(io_failure);
            }
            writeln!(run.out, "Pipeline reset to defaults.").map_err(io_failure)
        }
    }
}

/// The `gorse-cli` binary: runs the command with the process arguments, and
/// exits with status 1 when it fails.
pub async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = Environment {
        keyring: Box::new(SystemKeyring),
        endpoint: std::env::var("GORSE_ADMIN_ENDPOINT").unwrap_or_default(),
        api_key: std::env::var("GORSE_ADMIN_API_KEY").unwrap_or_default(),
        interactive: std::io::IsTerminal::is_terminal(&io::stdin()),
    };
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if let Err(failure) = execute(&args, &env, &mut stdin, &mut stdout).await {
        let _ = io::stdout().flush();
        eprint!("{}", failure.render());
        std::process::exit(1);
    }
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_has_the_commands_of_the_go_cli() {
        let tree = command();
        for name in [
            "context",
            "get",
            "recommend",
            "pipeline",
            "cluster-info",
            "stats",
            "ps",
            "dump",
            "restore",
        ] {
            assert!(tree.find_subcommand(name).is_some(), "missing {name}");
        }
        let get = tree.find_subcommand("get").expect("get");
        for name in ["categories", "user", "users", "item", "items", "feedback"] {
            assert!(get.find_subcommand(name).is_some(), "missing get {name}");
        }
    }

    #[test]
    fn failures_render_as_go_fatal_does() {
        let failure = Failure::with_suggestions(
            "missing Gorse base URL.",
            &["Use a saved context:", "  gorse-cli context use <name>"],
        );
        assert_eq!(
            failure.render(),
            "Error: missing Gorse base URL.\n\nUse a saved context:\n  gorse-cli context use <name>\n"
        );
        assert_eq!(Failure::new("boom").render(), "Error: boom\n");
    }

    #[test]
    fn unknown_commands_quote_the_word_and_the_path() {
        assert_eq!(
            unknown_command("click", "gorse-cli get feedback").message,
            "unknown command \"click\" for \"gorse-cli get feedback\""
        );
    }

    #[test]
    fn json_values_print_on_one_line() {
        let mut out = Vec::new();
        crate::cli_output::print_json_value(&mut out, &serde_json::json!({"a": 1})).expect("write");
        assert_eq!(String::from_utf8(out).expect("utf8"), "{\"a\":1}\n");
    }
}
