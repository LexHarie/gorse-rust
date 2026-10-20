// Ports: github.com/subosito/gotenv v1.6.0 gotenv.go (StrictParse, strictParse, parseLine, checkFormat, parseExport, varReplacement and the patterns), github.com/spf13/viper v1.21.0 internal/encoding/dotenv/codec.go (Codec.Decode) @ add21b5
//
// Derived from gotenv: Copyright (c) 2013 Alif Rachmawadi. Use of this source
// code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GOTENV-LICENSE. Derived from viper: Copyright
// (c) 2014 Steve Francia, MIT license, crates/gorse-config/licenses/VIPER-LICENSE.
// The submodules port parts of Go and golang.org/x/text (BSD-style license,
// crates/gorse-config/licenses/GO-LICENSE).

//! viper's dotenv codec (extensions `env` and `dotenv`): `gotenv.StrictParse`
//! over the file, then every variable is a string in the map.
//!
//! gotenv reads the file line by line (`"\n"`, `"\r"` or `"\r\n"`, after an
//! optional UTF-8 or UTF-16 byte order mark), joins the lines of a quoted
//! value that does not close on its first line, and matches each line against
//! `linePattern`. Values in double quotes have `\n` and `\r` turned into line
//! breaks and other escapes removed; values not in single quotes expand
//! `$VAR` and `${VAR}` from the environment, then from the variables set
//! earlier in the file, else to nothing.
//!
//! Go strings are bytes, so the parser works on bytes ([`strict_parse`]);
//! [`decode`] turns the result into Rust strings (deviation X-WP115E-1 for
//! bytes that are not UTF-8).

mod gostr;
mod scan;
mod transform;

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};

use crate::value::{Map, Value};
use gostr::{index_byte, last_index_byte, replace_all, split_once_byte, to_go_string, trim_space};

/// viper's dotenv codec `Decode`: the variables of the file, each a
/// [`Value::String`], in the order they were first set (a Go map has no
/// order). On failure, the error's `Error()` text, which viper wraps in
/// `While parsing config: `.
///
/// `env` looks up a process environment variable (`os.LookupEnv`); `$VAR`
/// and `${VAR}` read it first.
///
/// Go keeps keys and values as bytes; a value or error text that is not
/// valid UTF-8 has each byte that does not start a valid sequence replaced
/// by U+FFFD, as Go's `for range` reads it (X-WP115E-1). Keys are always
/// ASCII.
pub fn decode(input: &[u8], env: &dyn Fn(&str) -> Option<String>) -> Result<Map, String> {
    let (parsed, err) = strict_parse(input, env);
    if let Some(err) = err {
        return Err(to_go_string(&err));
    }
    let mut map = Map::new();
    for (key, value) in parsed.iter() {
        map.insert(to_go_string(key), Value::String(to_go_string(value)));
    }
    Ok(map)
}

/// gotenv's `Env` (`map[string]string`) with Go's byte strings, in the order
/// keys were first set.
#[derive(Clone, Debug, Default)]
pub struct Env {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    index: HashMap<Vec<u8>, usize>,
}

impl Env {
    /// `env[key]`.
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.index.get(key).map(|&i| self.entries[i].1.as_slice())
    }

    /// The entries, in the order keys were first set.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.entries
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// `len(env)`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `len(env) == 0`.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `env[key] = value`: an existing key keeps its place.
    fn set(&mut self, key: &[u8], value: Vec<u8>) {
        match self.index.get(key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.to_vec(), self.entries.len());
                self.entries.push((key.to_vec(), value));
            }
        }
    }
}

/// `gotenv.StrictParse(r)` over `input`: the variables, and the error's
/// `Error()` text if parsing stopped. As in Go, the variables set before the
/// error are returned with it (what `gotenv.Parse` returns); viper discards
/// them.
///
/// `env` is `os.LookupEnv`. Only `StrictParse`'s path is ported: `override`
/// is false (`Load`, `OverLoad` and `Apply` set process variables, which
/// viper does not do).
pub fn strict_parse(input: &[u8], env: &dyn Fn(&str) -> Option<String>) -> (Env, Option<Vec<u8>>) {
    let mut out = Env::default();
    let mut scanner = scan::new_scanner(input);

    while scanner.scan() {
        if let Some(err) = scanner.err() {
            return (out, Some(err.text().as_bytes().to_vec()));
        }

        let mut line = trim_space(scanner.text()).to_vec();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }

        let mut quote = None;
        // look for the delimiter character
        let idx = index_byte(&line, b'=').or_else(|| index_byte(&line, b':'));
        // look for a quote character
        if let Some(idx) = idx
            && idx > 0
            && idx < line.len() - 1
        {
            let val = trim_space(&line[idx + 1..]);
            if val[0] == b'"' || val[0] == b'\'' {
                quote = Some(val[0]);
                // Look for the closing quote character within the same line.
                // The index is into the trimmed val[1:] but is used on val,
                // so it tests the byte before the quote only when no space
                // follows the opening quote (Q-WP115E-3).
                if let Some(idx) = last_index_byte(trim_space(&val[1..]), val[0])
                    && val[idx] != b'\\'
                {
                    quote = None;
                }
            }
        }
        // look for the closing quote character
        while let Some(q) = quote {
            if !scanner.scan() {
                break;
            }
            let l = scanner.text();
            line.push(b'\n');
            line.extend_from_slice(l);
            match last_index_byte(l, q) {
                // found a matching quote character but it's escaped
                Some(idx) if idx > 0 && l[idx - 1] == b'\\' => continue,
                // found a matching quote
                Some(_) => quote = None,
                None => {}
            }
        }

        if quote.is_some() {
            return (out, Some(b"missing quotes".to_vec()));
        }

        if let Err(err) = parse_line(&line, &mut out, env) {
            return (out, Some(err));
        }
    }

    let err = scanner.err().map(|err| err.text().as_bytes().to_vec());
    (out, err)
}

/// Go's `\s` (`[\t\n\f\r ]`) and `\w` (`[0-9A-Za-z_]`) are ASCII and leave out
/// `\v`; the regex crate's are Unicode, and its ASCII `\s` has `\v`. So the
/// patterns spell the classes out, run on bytes with Unicode off (`.` and
/// negated classes match any byte but the ones listed, as Go matches any
/// rune, U+FFFD for an invalid byte), and `\'`, `\"`, `\#` are plain
/// characters. Both engines pick the leftmost-first match and its groups.
///
/// gotenv's `linePattern`:
/// `\A\s*(?:export\s+)?([\w\.]+)(?:\s*=\s*|:\s+?)('(?:\'|[^'])*'|"(?:\"|[^"])*"|[^#\n]+)?\s*(?:\s*\#.*)?\z`
const LINE_PATTERN: &str = concat!(
    r"(?-u)\A[\t\n\f\r ]*(?:export[\t\n\f\r ]+)?([0-9A-Za-z_.]+)",
    r"(?:[\t\n\f\r ]*=[\t\n\f\r ]*|:[\t\n\f\r ]+?)",
    r#"('(?:'|[^'])*'|"(?:"|[^"])*"|[^#\n]+)?"#,
    r"[\t\n\f\r ]*(?:[\t\n\f\r ]*#.*)?\z",
);

/// gotenv's `variablePattern`, `(\\)?(\$)(\{?([A-Z0-9_]+)?\}?)`.
const VARIABLE_PATTERN: &str = r"(?-u)(\\)?(\$)(\{?([A-Z0-9_]+)?\}?)";

/// gotenv's `unescapeRgx`, `\\([^$])`.
const UNESCAPE_PATTERN: &str = r"(?-u)\\([^$])";

/// gotenv's `varNameRgx`, `(\$)(\{?([A-Z0-9_]+)\}?)`.
const VAR_NAME_PATTERN: &str = r"(?-u)(\$)(\{?([A-Z0-9_]+)\}?)";

static LINE_RGX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(LINE_PATTERN).expect("linePattern compiles"));
static UNESCAPE_RGX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(UNESCAPE_PATTERN).expect("unescapeRgx compiles"));
static VAR_RGX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VARIABLE_PATTERN).expect("variablePattern compiles"));
static VAR_NAME_RGX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VAR_NAME_PATTERN).expect("varNameRgx compiles"));

/// A group of a match, `""` when it did not take part (Go's
/// `FindStringSubmatch`).
fn group<'h>(caps: &Captures<'h>, i: usize) -> &'h [u8] {
    caps.get(i).map_or(&[][..], |m| m.as_bytes())
}

/// gotenv's `parseLine`: sets the variable of a (possibly joined) line, or
/// returns the error text.
fn parse_line(
    s: &[u8],
    env: &mut Env,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), Vec<u8>> {
    let Some(rm) = LINE_RGX.captures(s) else {
        return check_format(s, env);
    };

    let key = trim_space(group(&rm, 1));
    let mut val = trim_space(group(&rm, 2)).to_vec();

    let mut hsq = false;
    let mut hdq = false;

    // check if the value is quoted
    if val.len() >= 2 {
        let l = val.len() - 1;
        // has double quotes
        hdq = val[0] == b'"' && val[l] == b'"';
        // has single quotes
        hsq = val[0] == b'\'' && val[l] == b'\'';

        // remove quotes '' or ""
        if hsq || hdq {
            val = val[1..l].to_vec();
        }
    }

    if hdq {
        val = replace_all(&val, br"\n", b"\n");
        val = replace_all(&val, br"\r", b"\r");

        // Unescape all characters except $ so variables can be escaped
        // properly.
        val = UNESCAPE_RGX
            .replace_all(&val, |c: &Captures<'_>| group(c, 1).to_vec())
            .into_owned();
    }

    if !hsq {
        val = VAR_RGX
            .replace_all(&val, |c: &Captures<'_>| {
                var_replacement(group(c, 0), hsq, env, lookup)
            })
            .into_owned();
    }

    env.set(key, val);
    Ok(())
}

/// gotenv's `parseExport`: an `export NAME` line is an error when `NAME` is
/// not set yet.
fn parse_export(st: &[u8], env: &Env) -> Result<(), Vec<u8>> {
    if st.starts_with(b"export") {
        let (_, rest) = split_once_byte(st, b' ');
        if let Some(name) = rest
            && env.get(name).is_none()
        {
            return Err([&b"line `"[..], st, b"` has an unset variable"].concat());
        }
    }
    Ok(())
}

/// gotenv's `varReplacement`: what a match of `variablePattern` becomes.
/// `hsq` is always false here, as in Go (the caller skips single-quoted
/// values).
fn var_replacement(
    s: &[u8],
    hsq: bool,
    env: &Env,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<u8> {
    if s.is_empty() {
        return s.to_vec();
    }

    if s[0] == b'\\' {
        // the dollar sign is escaped
        return s[1..].to_vec();
    }

    if hsq {
        return s.to_vec();
    }

    let Some(mn) = VAR_NAME_RGX.captures(s) else {
        return s.to_vec();
    };

    // The name matched [A-Z0-9_]+, so it is ASCII.
    let v = String::from_utf8_lossy(group(&mn, 3));

    // override is false: the process environment wins.
    if let Some(replace) = lookup(&v) {
        return replace.into_bytes();
    }

    if let Some(replace) = env.get(v.as_bytes()) {
        return replace.to_vec();
    }

    // os.Getenv(v): unset, so "".
    lookup(&v).unwrap_or_default().into_bytes()
}

/// gotenv's `checkFormat`: the error for a line `linePattern` does not match.
fn check_format(s: &[u8], env: &Env) -> Result<(), Vec<u8>> {
    let st = trim_space(s);

    if st.is_empty() || st[0] == b'#' {
        return Ok(());
    }

    parse_export(st, env)?;

    Err([&b"line `"[..], s, b"` doesn't match format"].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_patterns_spelled_out() {
        // The Go patterns, with \s, \w and the escaped punctuation replaced
        // as the comment on LINE_PATTERN says, give the Rust patterns.
        let go_line = r#"\A\s*(?:export\s+)?([\w\.]+)(?:\s*=\s*|:\s+?)('(?:\'|[^'])*'|"(?:\"|[^"])*"|[^#\n]+)?\s*(?:\s*\#.*)?\z"#;
        let spelled = go_line
            .replace(r"[\w\.]", "[0-9A-Za-z_.]")
            .replace(r"\s", r"[\t\n\f\r ]")
            .replace(r"\'", "'")
            .replace(r#"\""#, "\"")
            .replace(r"\#", "#");
        assert_eq!(format!("(?-u){spelled}"), LINE_PATTERN);
    }
}
