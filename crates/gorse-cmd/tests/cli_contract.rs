// Ports: none. Checks the flags of the four binaries against `contracts/cli/*.help.txt`, the `--help` output of the pinned Go binaries (WP-4-5 step 3).

//! Every flag in the help files exists on the Rust command with the same short
//! form, value type, default and help text, and the command has no flag the
//! help files lack. Go's help files are the contract; clap lays out its own
//! `--help`, so the check reads the commands' metadata, not their output.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use clap::{Arg, Command};

/// One flag as the help file prints it.
#[derive(Debug, PartialEq)]
struct Expected {
    short: Option<char>,
    /// The type name, `string`, `int` or `duration`. `None` is a boolean.
    value: Option<String>,
    /// The default the help file prints, with the home directory filled in.
    default: Option<String>,
    /// The value a bare flag takes, when the help file prints one (`[="default"]`).
    missing: Option<String>,
    help: String,
}

/// Reads the flags of one help file. `home` replaces `${HOME}`.
fn read_help(name: &str, home: &str) -> BTreeMap<String, Expected> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/cli")
        .join(format!("{name}.help.txt"));
    let text =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let mut flags = BTreeMap::new();
    for line in text.lines().filter(|line| line.starts_with(' ')) {
        let line = line.trim_start();
        if !line.starts_with('-') {
            continue;
        }
        let (short, rest) = if let Some(rest) = line.strip_prefix("--") {
            (None, rest)
        } else {
            let short = line[1..].chars().next();
            let rest = line[2..]
                .strip_prefix(", --")
                .unwrap_or_else(|| panic!("unexpected flag line: {line}"));
            (short, rest)
        };
        let (long, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if long == "help" {
            continue;
        }
        let after = after.trim_start();
        let (value, missing, after) = take_type(after);
        let (help, default) = match after.rfind(" (default ") {
            Some(at) => {
                let text = after[at + " (default ".len()..].trim_end_matches(')');
                let text = text.replace("${HOME}", home);
                let text = text.trim_matches('"').to_owned();
                (after[..at].to_owned(), Some(text))
            }
            None => (after.to_owned(), None),
        };
        let expected = Expected {
            short,
            value,
            default,
            missing,
            help,
        };
        assert!(
            flags.insert(long.to_owned(), expected).is_none(),
            "{name}: --{long} is listed twice"
        );
    }
    flags
}

/// Splits the type off the front of a flag's text: `string`, `int`, `duration`,
/// `string[="default"]`, or nothing for a boolean. Returns the type, the value
/// a bare flag takes, and the rest.
fn take_type(after: &str) -> (Option<String>, Option<String>, &str) {
    for ty in ["string", "int", "duration"] {
        if let Some(rest) = after.strip_prefix(ty) {
            if let Some(rest) = rest.strip_prefix("[=\"") {
                let (missing, rest) = rest.split_once("\"]").expect("a closing quote and bracket");
                return (
                    Some(ty.to_owned()),
                    Some(missing.to_owned()),
                    rest.trim_start(),
                );
            }
            if rest.starts_with(' ') {
                return (Some(ty.to_owned()), None, rest.trim_start());
            }
        }
    }
    (None, None, after)
}

/// Checks `command` against the help file `name`.
fn check(command: &Command, name: &str) {
    let home = std::env::var("HOME").expect("HOME is set");
    let expected = read_help(name, &home);
    let mut actual: BTreeMap<String, &Arg> = BTreeMap::new();
    for arg in command.get_arguments() {
        if arg.is_positional() || arg.get_id() == "help" {
            continue;
        }
        let long = arg
            .get_long()
            .expect("every flag has a long name")
            .to_owned();
        assert!(
            actual.insert(long.clone(), arg).is_none(),
            "--{long} is defined twice"
        );
    }
    let expected_names: Vec<&String> = expected.keys().collect();
    let actual_names: Vec<&String> = actual.keys().collect();
    assert_eq!(
        actual_names, expected_names,
        "{name}: flag set differs from the help file"
    );

    for (long, want) in &expected {
        let arg = actual[long];
        assert_eq!(arg.get_short(), want.short, "{name}: --{long} short form");
        let value_names = arg.get_value_names().map(|names| names[0].to_string());
        assert_eq!(value_names, want.value, "{name}: --{long} value type");
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        match &want.default {
            Some(default) => assert_eq!(defaults, [default.as_str()], "{name}: --{long} default"),
            // Go prints a default only when it is not the zero value of its type.
            None => assert!(
                defaults.is_empty()
                    || defaults
                        .iter()
                        .all(|d| matches!(d.as_str(), "0" | "false" | "")),
                "{name}: --{long} has a default {defaults:?}, which the help file does not print"
            ),
        }
        // A bare flag takes the value the help file prints, or "true" for a
        // boolean. Any other flag needs its value, so a bare one is a usage error.
        let bare = match (&want.missing, &want.value) {
            (Some(missing), _) => Some(missing.clone()),
            (None, None) => Some("true".to_owned()),
            (None, Some(_)) => None,
        };
        let parsed = command
            .clone()
            .try_get_matches_from([name, &format!("--{long}")]);
        match bare {
            Some(value) => {
                let matches = parsed.unwrap_or_else(|err| panic!("{name}: bare --{long}: {err}"));
                let raw = matches.get_raw(long).expect("a bare flag is set");
                let raw: Vec<String> = raw.map(|v| v.to_string_lossy().into_owned()).collect();
                assert_eq!(raw, [value], "{name}: --{long} value when given bare");
            }
            None => assert!(parsed.is_err(), "{name}: --{long} needs a value"),
        }
        let help = arg.get_help().map(ToString::to_string).unwrap_or_default();
        assert_eq!(help, want.help, "{name}: --{long} help text");
    }
}

#[test]
fn gorse_master_flags_match_the_help_file() {
    check(&gorse_cmd::master::command(), "gorse-master");
}

#[test]
fn gorse_server_flags_match_the_help_file() {
    check(&gorse_cmd::server::command(), "gorse-server");
}

#[test]
fn gorse_worker_flags_match_the_help_file() {
    check(&gorse_cmd::worker::command(), "gorse-worker");
}

#[test]
fn gorse_in_one_flags_match_the_help_file() {
    check(&gorse_cmd::in_one::command(), "gorse-in-one");
}
