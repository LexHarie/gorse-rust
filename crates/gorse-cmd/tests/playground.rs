// Ports: cmd/gorse-in-one/main.go (`setup`, the playground branch) @ add21b5

//! The playground import: `gorse-in-one --playground=ml-100k` writes its config,
//! downloads the dump from `cdn.gorse.io`, restores it, and prints the welcome
//! text. The download host is blocked in this environment (O-1), so the test is
//! ignored. It does not pass by being skipped: run it with `--ignored` where the
//! dataset can be reached.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use tempfile::TempDir;

#[test]
#[ignore = "O-1: dataset from cdn.gorse.io"]
fn playground_ml_100k_imports_the_dataset_and_serves() {
    let home = TempDir::new().expect("a temporary home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gorse-in-one"))
        .arg("--playground=ml-100k")
        .env("HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start gorse-in-one");
    let stdout = child.stdout.take().expect("a piped stdout");
    // The welcome text comes after the import, and the node serves from then on.
    let welcomed = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("Welcome to Gorse Playground"));
    let _ = child.kill();
    let _ = child.wait();
    assert!(welcomed, "the playground did not finish its import");
    assert!(home.path().join(".gorse/etc/config.toml").is_file());
}
