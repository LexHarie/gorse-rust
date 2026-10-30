// Ports: common/log/log.go (AddFlags, SetLogger), cmd/gorse-master/main.go (the cobra command and its exit path) @ add21b5

//! The command-line layer shared by the four binaries.
//!
//! Go's commands use pflag, which takes `--flag=value` for a flag that takes a
//! value, and never a separate word for a flag whose value is optional
//! (`NoOptDefVal`). The builders here reproduce that with `require_equals` and
//! `default_missing_value`. Flags are declared with the builder API, not the
//! derive macros, because the `--cache-path` default is made when the command is
//! built (`config.MkDir` in Go's `init`), and the help-file check reads the
//! command's own metadata.
//!
//! Exit codes follow Go: `--help` and `--version` exit 0, and a usage error or a
//! Go `log.Fatal` exits 1 (`clap`'s own usage exit is 2).

use std::ffi::OsString;

use clap::builder::ValueParser;
use clap::error::ErrorKind;
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use gorse_common::gotime;
use gorse_common::log::Options;
use gorse_common::util::tls::TlsConfig;

/// The exit code of a Go `log.Fatal`, and of a usage error.
pub const FATAL: i32 = 1;

/// The default `--log-max-size`, in megabytes (Go's `log.AddFlags`).
const LOG_MAX_SIZE: i64 = 100;

/// Builds the command of one binary: its name and its short description, with
/// the flags every binary shares. `-h` and `--help` come from clap.
pub fn command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .disable_version_flag(true)
        .arg(bool_flag("debug", None, "use debug log mode"))
        .arg(bool_flag("version", Some('v'), "gorse version"))
        .args(log_flags())
        // Go's cobra accepts any arguments on a root command with no subcommands
        // and ignores them, so a stray word is not an error. This hidden argument
        // takes them.
        .arg(
            Arg::new("ignored")
                .num_args(0..)
                .hide(true)
                .value_parser(value_parser!(String)),
        )
}

/// The log flags of Go's `common/log.AddFlags`, with the same defaults.
pub fn log_flags() -> Vec<Arg> {
    vec![
        string_flag("log-path", None, "path of log file", None),
        string_flag(
            "access-log-path",
            None,
            "path of access log file for RESTful API",
            None,
        ),
        int_flag(
            "log-max-size",
            None,
            "maximum size in megabytes of the log file",
            Some(LOG_MAX_SIZE),
        ),
        int_flag(
            "log-max-age",
            None,
            "maximum number of days to retain old log files",
            None,
        ),
        int_flag(
            "log-max-backups",
            None,
            "maximum number of old log files to retain",
            None,
        ),
    ]
}

/// A boolean flag in pflag's form: `--debug` means `--debug=true`, and a
/// following word is not its value.
pub fn bool_flag(name: &'static str, short: Option<char>, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .short(short)
        .help(help)
        .action(ArgAction::Set)
        .value_parser(value_parser!(bool))
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value("true")
}

/// A string flag. Its help name is Go's type name, `string`.
pub fn string_flag(
    name: &'static str,
    short: Option<char>,
    help: &'static str,
    default: Option<String>,
) -> Arg {
    let arg = Arg::new(name)
        .long(name)
        .short(short)
        .help(help)
        .value_name("string");
    match default {
        Some(default) => arg.default_value(leak(default)),
        None => arg,
    }
}

/// An integer flag, typed as Go's `int`.
pub fn int_flag(
    name: &'static str,
    short: Option<char>,
    help: &'static str,
    default: Option<i64>,
) -> Arg {
    let arg = Arg::new(name)
        .long(name)
        .short(short)
        .help(help)
        .value_name("int")
        .value_parser(value_parser!(i64));
    match default {
        Some(default) => arg.default_value(leak(default.to_string())),
        None => arg,
    }
}

/// A Go duration flag, such as `1m0s`. Its help name is Go's type name,
/// `duration`.
pub fn duration_flag(name: &'static str, help: &'static str, default: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .value_name("duration")
        .value_parser(ValueParser::new(parse_go_duration))
        .default_value(default)
}

/// A port flag, typed as Go's `int` and read as `u16`. A port out of range is a
/// usage error here, not a failure at the listener.
pub fn port_flag(name: &'static str, help: &'static str, default: u16) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .value_name("int")
        .value_parser(value_parser!(u16))
        .default_value(leak(default.to_string()))
}

/// A default that clap keeps for the life of the process. clap takes defaults as
/// `'static` text, so a default made at run time is leaked. There is one per
/// flag, and a command is built once per process.
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// Go's `time.ParseDuration`, for the flags that take a duration.
fn parse_go_duration(text: &str) -> Result<std::time::Duration, String> {
    let duration = gotime::parse_duration(text).map_err(|err| err.to_string())?;
    let nanos = u64::try_from(duration.nanoseconds())
        .map_err(|_| format!("duration {text} is negative"))?;
    Ok(std::time::Duration::from_nanos(nanos))
}

/// Parses the arguments the way Go's command does. Help and version print and
/// exit 0. A usage error prints and exits 1.
pub fn parse(command: Command, args: impl IntoIterator<Item = OsString>) -> ArgMatches {
    match command.try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(err) => {
            let ok = matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            // Print first, so the user sees the message before the process ends.
            let _ = err.print();
            std::process::exit(if ok { 0 } else { FATAL });
        }
    }
}

/// The log options of the flags, as Go's `SetLogger` reads them. `--log-path`
/// makes a file, as it does when the flag is changed in Go.
pub fn log_options(matches: &ArgMatches) -> Options {
    Options {
        log_path: matches.get_one::<String>("log-path").cloned(),
        access_log_path: matches
            .get_one::<String>("access-log-path")
            .cloned()
            .unwrap_or_default(),
        max_size: matches
            .get_one::<i64>("log-max-size")
            .copied()
            .unwrap_or(LOG_MAX_SIZE),
        max_age: matches.get_one::<i64>("log-max-age").copied().unwrap_or(0),
        max_backups: matches
            .get_one::<i64>("log-max-backups")
            .copied()
            .unwrap_or(0),
    }
}

/// Go's `log.SetLogger` from the flags of `matches`.
pub fn set_logger(matches: &ArgMatches) {
    gorse_common::log::set_logger(&log_options(matches), flag(matches, "debug"));
}

/// The value of a boolean flag, `false` when it was not given.
pub fn flag(matches: &ArgMatches, name: &str) -> bool {
    matches.get_one::<bool>(name).copied().unwrap_or(false)
}

/// The value of a port flag, `0` when it has none.
pub fn port(matches: &ArgMatches, name: &str) -> u16 {
    matches.get_one::<u16>(name).copied().unwrap_or_default()
}

/// The value of a string flag, or the empty string when it has none.
pub fn string(matches: &ArgMatches, name: &str) -> String {
    matches.get_one::<String>(name).cloned().unwrap_or_default()
}

/// Go's SSL rule: TLS is on only when `--ssl-ca`, `--ssl-cert` and `--ssl-key`
/// are all set. Any other non-empty subset is a fatal error, and so is the
/// error this returns.
pub fn tls_config(matches: &ArgMatches) -> Option<TlsConfig> {
    let ca = string(matches, "ssl-ca");
    let cert = string(matches, "ssl-cert");
    let key = string(matches, "ssl-key");
    if !ca.is_empty() && !cert.is_empty() && !key.is_empty() {
        Some(TlsConfig {
            ssl_ca: ca,
            ssl_cert: cert,
            ssl_key: key,
        })
    } else if ca.is_empty() && cert.is_empty() && key.is_empty() {
        None
    } else {
        tracing::error!(
            ssl_ca = %ca,
            ssl_cert = %cert,
            ssl_key = %key,
            "incomplete SSL configuration"
        );
        std::process::exit(FATAL);
    }
}

/// Resolves when the process gets SIGINT (`os.Interrupt` in Go).
pub async fn interrupted() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %err, "failed to listen for interrupt");
        std::process::exit(FATAL);
    }
}

/// Go's `log.Logger().Fatal`: the message is logged, and the process exits with
/// code 1.
pub fn fatal(message: &str, error: impl std::fmt::Display) -> ! {
    tracing::error!(error = %error, "{message}");
    std::process::exit(FATAL)
}
