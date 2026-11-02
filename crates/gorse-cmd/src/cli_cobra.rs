// Ports: github.com/spf13/cobra v1.10.2 (command.go: Find, stripFlags, findNext, argsMinusFirstX, findSuggestions, SuggestionsFor; args.go: legacyArgs, NoArgs, ExactArgs, RangeArgs) and github.com/spf13/pflag v1.0.10 (flag.go: parseArgs, parseLongArg, parseShortArg, parseSingleShortArg, set; errors.go) @ add21b5 (module versions from go.mod)

//! The part of cobra and pflag that decides which command a `gorse-cli` line
//! runs, which flags it sets, and the error text a wrong line gets.
//!
//! Clap parses a line that is right, and prints the help. A line that is wrong
//! is checked here first, with Go's rules and Go's words, so the user sees
//! the same `Error:` text as with the Go binary. Every error here is one that
//! Go's cobra returns before the command runs.

use clap::{ArgAction, Command};
use gorse_common::gostrconv;

use crate::cli_context::go_quote;

/// The kind of value a flag takes, named as pflag names its types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A boolean flag, which needs no value and has `NoOptDefVal` `true`.
    Bool,
    /// A string flag.
    String,
    /// An integer flag, parsed with `strconv.ParseInt(s, 0, 64)`.
    Int,
    /// A repeatable string flag (`StringArray`).
    StringArray,
}

/// A flag of a command: its long name, its shorthand, and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    /// The long name, without the dashes.
    pub name: String,
    /// The shorthand letter, if the flag has one.
    pub short: Option<char>,
    /// The kind of value the flag takes.
    pub kind: Kind,
}

impl Flag {
    /// The flag as pflag names it in an invalid-value error: `-n, --n` when it
    /// has a shorthand, and `--name` when it has none.
    fn display_name(&self) -> String {
        match self.short {
            Some(short) => format!("-{short}, --{}", self.name),
            None => format!("--{}", self.name),
        }
    }
}

/// The flags a command defines, as clap holds them. The help flag is always
/// there, as cobra adds it to every command.
pub fn flags_of(command: &Command) -> Vec<Flag> {
    let mut flags = vec![Flag {
        name: "help".to_owned(),
        short: Some('h'),
        kind: Kind::Bool,
    }];
    for arg in command.get_arguments() {
        if arg.is_positional() {
            continue;
        }
        let Some(name) = arg.get_long() else {
            continue;
        };
        if flags.iter().any(|flag| flag.name == name) {
            continue;
        }
        let kind = match arg.get_action() {
            ArgAction::SetTrue | ArgAction::SetFalse => Kind::Bool,
            ArgAction::Append => Kind::StringArray,
            _ if arg
                .get_value_names()
                .and_then(|names| names.first())
                .is_some_and(|name| name.as_str() == "int") =>
            {
                Kind::Int
            }
            _ => Kind::String,
        };
        flags.push(Flag {
            name: name.to_owned(),
            short: arg.get_short(),
            kind,
        });
    }
    flags
}

/// The flags that apply to `command`: its own, and the persistent flags of the
/// root that every command inherits (`--version`).
fn flags_for(root: &Command, command: &Command) -> Vec<Flag> {
    let mut flags = flags_of(command);
    for flag in flags_of(root) {
        let inherited = root
            .get_arguments()
            .any(|arg| arg.get_long() == Some(flag.name.as_str()) && arg.is_global_set());
        if inherited && !flags.iter().any(|own| own.name == flag.name) {
            flags.push(flag);
        }
    }
    flags
}

/// Whether the flag named `name` (a long name) needs no value.
fn no_opt_def_val_long(name: &str, flags: &[Flag]) -> bool {
    flags
        .iter()
        .any(|flag| flag.name == name && flag.kind == Kind::Bool)
}

/// Whether the flag with shorthand `short` needs no value.
fn no_opt_def_val_short(short: char, flags: &[Flag]) -> bool {
    flags
        .iter()
        .any(|flag| flag.short == Some(short) && flag.kind == Kind::Bool)
}

/// Cobra's `stripFlags`: the words of `args` that are not flags, and not the
/// value of a flag. A flag with a separate value takes the word after it.
fn strip_flags(args: &[String], flags: &[Flag]) -> Vec<String> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let s = args[i].as_str();
        i += 1;
        if s == "--" {
            break;
        }
        let long_takes_value =
            s.starts_with("--") && !s.contains('=') && !no_opt_def_val_long(&s[2..], flags);
        let short_takes_value = s.starts_with('-')
            && !s.contains('=')
            && s.len() == 2
            && !s[1..]
                .chars()
                .next()
                .is_some_and(|c| no_opt_def_val_short(c, flags));
        if long_takes_value || short_takes_value {
            // `if len(args) <= 1 { break }`: the value is the next word.
            if args.len() - i <= 1 {
                break;
            }
            i += 1;
        } else if !s.is_empty() && !s.starts_with('-') {
            words.push(s.to_owned());
        }
    }
    words
}

/// Cobra's `argsMinusFirstX`: `args` without the first word `word` that is
/// not a flag's value.
fn args_minus_first(args: &[String], flags: &[Flag], word: &str) -> Vec<String> {
    let mut i = 0;
    while i < args.len() {
        let s = args[i].as_str();
        if s == "--" {
            break;
        }
        let long_takes_value =
            s.starts_with("--") && !s.contains('=') && !no_opt_def_val_long(&s[2..], flags);
        let short_takes_value = s.starts_with('-')
            && !s.contains('=')
            && s.len() == 2
            && !s[1..]
                .chars()
                .next()
                .is_some_and(|c| no_opt_def_val_short(c, flags));
        if s == word {
            let mut out = args.to_vec();
            out.remove(i);
            return out;
        }
        if long_takes_value || short_takes_value {
            i += 2;
        } else {
            i += 1;
        }
    }
    args.to_vec()
}

/// A line that names no command under the root, as cobra's `legacyArgs` reports.
pub struct UnknownCommand {
    /// The message, with its suggestions.
    pub message: String,
    /// The command path it was found under, such as `gorse-cli`.
    pub path: String,
}

/// The command a line runs (cobra's `Find`): the commands from the root down to
/// it, and the words that follow the last of the command names, flags included.
pub struct Found<'a> {
    /// The commands from the root to the one that runs.
    pub chain: Vec<&'a Command>,
    /// The words after the command names, in order.
    pub rest: Vec<String>,
}

impl Found<'_> {
    /// The command path, such as `gorse-cli get users`.
    pub fn path(&self) -> String {
        self.chain
            .iter()
            .map(|command| command.get_name())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The command that runs.
    pub fn command(&self) -> &Command {
        self.chain
            .last()
            .copied()
            .expect("the root is always in the chain")
    }
}

/// Cobra's `Find`, for the command tree `root`. A word that names no command of
/// the root is an error, as the root has no `Args` of its own. Under any other
/// command the word is left for the command to check.
pub fn find<'a>(root: &'a Command, args: &[String]) -> Result<Found<'a>, UnknownCommand> {
    let mut chain: Vec<&'a Command> = vec![root];
    let mut rest: Vec<String> = args.to_vec();
    loop {
        let current = *chain.last().expect("the root is in the chain");
        let flags = flags_for(root, current);
        let words = strip_flags(&rest, &flags);
        let Some(next) = words.first() else {
            break;
        };
        let Some(sub) = current.find_subcommand(next) else {
            break;
        };
        rest = args_minus_first(&rest, &flags, next);
        chain.push(sub);
    }
    let current = *chain.last().expect("the root is in the chain");
    if chain.len() == 1 {
        let flags = flags_for(root, current);
        if let Some(word) = strip_flags(&rest, &flags).first() {
            return Err(UnknownCommand {
                message: unknown_command(word, current, root),
                path: current.get_name().to_owned(),
            });
        }
    }
    Ok(Found { chain, rest })
}

/// Cobra's text for a word that is not a command of `parent`, with the
/// suggestions cobra adds under the root.
pub fn unknown_command(word: &str, parent: &Command, root: &Command) -> String {
    let at_root = std::ptr::eq(parent, root);
    let path = if at_root {
        root.get_name().to_owned()
    } else {
        format!("{} {}", root.get_name(), parent.get_name())
    };
    let mut message = format!("unknown command {} for {}", go_quote(word), go_quote(&path));
    if at_root {
        message.push_str(&suggestions(word, root));
    }
    message
}

/// The names cobra suggests for a mistyped word under the root: the commands,
/// and the `help` and `completion` commands cobra adds to the root.
fn suggestions(typed: &str, root: &Command) -> String {
    let mut names: Vec<String> = root
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .chain(["completion".to_owned(), "help".to_owned()])
        .collect();
    names.sort();
    names.dedup();
    let typed_lower = typed.to_lowercase();
    let matches: Vec<String> = names
        .into_iter()
        .filter(|name| {
            levenshtein(&typed_lower, &name.to_lowercase()) <= 2
                || name.to_lowercase().starts_with(&typed_lower)
        })
        .collect();
    if matches.is_empty() {
        return String::new();
    }
    let mut text = String::from("\n\nDid you mean this?\n");
    for name in matches {
        text.push_str(&format!("\t{name}\n"));
    }
    text
}

/// The edit distance between two strings, counted in characters (cobra's `ld`).
fn levenshtein(s: &str, t: &str) -> usize {
    let s: Vec<char> = s.chars().collect();
    let t: Vec<char> = t.chars().collect();
    let mut previous: Vec<usize> = (0..=t.len()).collect();
    for (i, sc) in s.iter().enumerate() {
        let mut current = vec![i + 1; t.len() + 1];
        for (j, tc) in t.iter().enumerate() {
            let cost = usize::from(sc != tc);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }
    previous[t.len()]
}

/// Pflag's `parseArgs` for the flags of the command that runs. The flags are
/// checked and their values validated, in the order the words come. The words
/// that are not flags come back in order. The first error is returned as pflag
/// words it.
pub fn parse_flags(flags: &[Flag], args: &[String]) -> Result<Vec<String>, String> {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let s = args[i].as_str();
        i += 1;
        if s.is_empty() || !s.starts_with('-') || s.len() == 1 {
            positional.push(s.to_owned());
            continue;
        }
        if s.starts_with("--") {
            if s.len() == 2 {
                positional.extend(args[i..].iter().cloned());
                break;
            }
            parse_long(flags, s, args, &mut i)?;
        } else {
            parse_short(flags, s, args, &mut i)?;
        }
    }
    Ok(positional)
}

/// Pflag's `parseLongArg`: `--name`, `--name=value`, or `--name value`.
fn parse_long(flags: &[Flag], s: &str, args: &[String], i: &mut usize) -> Result<(), String> {
    let name = &s[2..];
    if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
        return Err(format!("bad flag syntax: {s}"));
    }
    let (name, inline) = match name.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (name, None),
    };
    let Some(flag) = flags.iter().find(|flag| flag.name == name) else {
        return Err(format!("unknown flag: --{name}"));
    };
    let value = if let Some(value) = inline {
        value.to_owned()
    } else if flag.kind == Kind::Bool {
        "true".to_owned()
    } else if *i < args.len() {
        *i += 1;
        args[*i - 1].clone()
    } else {
        return Err(format!("flag needs an argument: --{name}"));
    };
    check_value(flag, &value)
}

/// Pflag's `parseShortArg` and `parseSingleShortArg`: `-n5`, `-n 5`, `-n=5`,
/// and a run of boolean shorthands such as `-vh`.
fn parse_short(flags: &[Flag], s: &str, args: &[String], i: &mut usize) -> Result<(), String> {
    let mut shorthands = s[1..].to_owned();
    while let Some(c) = shorthands.chars().next() {
        let rest = shorthands[c.len_utf8()..].to_owned();
        let Some(flag) = flags.iter().find(|flag| flag.short == Some(c)) else {
            return Err(format!(
                "unknown shorthand flag: {} in -{shorthands}",
                gostrconv::quote_rune(c)
            ));
        };
        let (value, remaining) = if shorthands.len() > 2 && shorthands.as_bytes()[1] == b'=' {
            (shorthands[2..].to_owned(), String::new())
        } else if flag.kind == Kind::Bool {
            ("true".to_owned(), rest)
        } else if !rest.is_empty() {
            (rest, String::new())
        } else if *i < args.len() {
            *i += 1;
            (args[*i - 1].clone(), String::new())
        } else {
            return Err(format!(
                "flag needs an argument: {} in -{shorthands}",
                gostrconv::quote_rune(c)
            ));
        };
        check_value(flag, &value)?;
        shorthands = remaining;
    }
    Ok(())
}

/// Pflag's value check: an integer must parse as `strconv.ParseInt(s, 0, 64)`
/// does, and a boolean as `strconv.ParseBool` does. Other kinds take any text.
fn check_value(flag: &Flag, value: &str) -> Result<(), String> {
    let cause = match flag.kind {
        Kind::Int => gostrconv::parse_int(value, 0, 64).1,
        Kind::Bool => gostrconv::parse_bool(value).1,
        Kind::String | Kind::StringArray => None,
    };
    match cause {
        None => Ok(()),
        Some(err) => Err(format!(
            "invalid argument {} for {} flag: {err}",
            go_quote(value),
            go_quote(&flag.display_name())
        )),
    }
}

/// Parses the flags of the command at the end of `found`, with pflag's rules,
/// and returns the words that are not flags.
pub fn check_line(root: &Command, found: &Found<'_>) -> Result<Vec<String>, String> {
    let flags = flags_for(root, found.command());
    parse_flags(&flags, &found.rest)
}
