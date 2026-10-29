//! The WP-4-5 acceptance test for the binaries. Each Rust node binary runs as
//! its own process, beside Go nodes in the other two roles, on one run's config
//! and stores, and every node reaches its ready state. The `--help` output of
//! each Rust binary is checked against `contracts/cli/*.help.txt`.
//!
//! The Go nodes and the Rust binaries are both built by the test (`GoBuild`,
//! `RustBuild`). The test fails, and does not skip, when a toolchain is missing
//! or a node does not start.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};

/// How often a worker checks for work. The first check runs after one interval.
const TICK: Duration = Duration::from_millis(500);

/// The Rust binaries whose `--help` this test checks.
const NODES: [&str; 4] = [
    "gorse-master",
    "gorse-server",
    "gorse-worker",
    "gorse-in-one",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_master_binary_runs_beside_go_server_and_worker() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let mut cluster = Cluster::new(go, &[]).expect("a run");
    cluster
        .start_rust_master_binary()
        .await
        .expect("the Rust master binary starts");
    cluster
        .start_go_server()
        .await
        .expect("the Go server joins the Rust master");
    cluster
        .start_go_worker(TICK, 1)
        .await
        .expect("the Go worker joins the Rust master");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_in_one_binary_runs_beside_go_server_and_worker() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let mut cluster = Cluster::new(go, &[]).expect("a run");
    cluster
        .start_rust_in_one_binary()
        .await
        .expect("the Rust gorse-in-one binary starts");
    cluster
        .start_go_server()
        .await
        .expect("the Go server joins the Rust gorse-in-one master");
    cluster
        .start_go_worker(TICK, 1)
        .await
        .expect("the Go worker joins the Rust gorse-in-one master");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_server_binary_runs_beside_go_master_and_worker() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let mut cluster = Cluster::new(go, &[]).expect("a run");
    cluster.start_master().await.expect("the Go master starts");
    cluster
        .start_go_worker(TICK, 1)
        .await
        .expect("the Go worker joins the Go master");
    cluster
        .start_rust_server_binary()
        .await
        .expect("the Rust server binary joins the Go master");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_worker_binary_runs_beside_go_master_and_server() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let mut cluster = Cluster::new(go, &[]).expect("a run");
    cluster.start_master().await.expect("the Go master starts");
    cluster
        .start_go_server()
        .await
        .expect("the Go server joins the Go master");
    cluster
        .start_rust_worker_binary(TICK, 1)
        .await
        .expect("the Rust worker binary joins the Go master");
}

#[test]
fn rust_binaries_help_matches_the_help_files() {
    let rust = RustBuild::shared().expect("the Rust nodes build");
    for name in NODES {
        let home = tempfile::tempdir().expect("a temporary home");
        let output = Command::new(rust.binary(name))
            .arg("--help")
            .env("HOME", home.path())
            .output()
            .unwrap_or_else(|err| panic!("run {name} --help: {err}"));
        assert!(output.status.success(), "{name} --help: {}", output.status);
        let shown = help_output_flags(&String::from_utf8_lossy(&output.stdout));
        let wanted = help_file_flags(name);
        assert_eq!(
            shown, wanted,
            "{name}: --help differs from contracts/cli/{name}.help.txt"
        );
    }
}

/// A flag: its short form and its type name (`string`, `int` or `duration`), or
/// `None` for a boolean.
type Flag = (Option<char>, Option<String>);

/// The type names the help files and clap's help name a value by.
const TYPES: [&str; 3] = ["string", "int", "duration"];

/// Splits the short form off a flag line, and returns the rest of the line.
fn split_short(line: &str) -> (Option<char>, &str) {
    if line.starts_with("--") {
        return (None, line);
    }
    let short = line[1..].chars().next();
    let rest = line[2..]
        .strip_prefix(", ")
        .unwrap_or_else(|| panic!("unexpected flag line: {line}"));
    (short, rest)
}

/// The flags of a help file, by long name, without `help`.
fn help_file_flags(name: &str) -> BTreeMap<String, Flag> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/cli")
        .join(format!("{name}.help.txt"));
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let mut flags = BTreeMap::new();
    for line in text.lines().filter(|line| line.starts_with(' ')) {
        let line = line.trim_start();
        if !line.starts_with('-') {
            continue;
        }
        let (short, rest) = split_short(line);
        let rest = rest.strip_prefix("--").expect("a long name");
        let long: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let after = rest[long.len()..].trim_start();
        let ty = TYPES
            .into_iter()
            .find(|ty| after.starts_with(ty) && after[ty.len()..].starts_with([' ', '[']))
            .map(str::to_owned);
        if long != "help" {
            flags.insert(long, (short, ty));
        }
    }
    flags
}

/// The flags of a `--help` output, by long name, without `help`. Clap names a
/// value in `<...>`; a boolean's name is upper case and is not a type.
fn help_output_flags(text: &str) -> BTreeMap<String, Flag> {
    let mut flags = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        if !line.starts_with('-') {
            continue;
        }
        let (short, rest) = split_short(line);
        let rest = rest.strip_prefix("--").expect("a long name");
        let long: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let after = &rest[long.len()..];
        let ty = after
            .split_once('<')
            .and_then(|(_, value)| value.split_once('>'))
            .map(|(value, _)| value.to_owned())
            .filter(|value| TYPES.contains(&value.as_str()));
        if long != "help" {
            flags.insert(long, (short, ty));
        }
    }
    flags
}
