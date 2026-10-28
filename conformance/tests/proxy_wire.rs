// Ports: contracts/tools/proxygolden (the Go oracle and its two wire checks), and crates/gorse-storage/src/{data,cache,vectors}/proxy.rs (WP-2-12).

//! WP-2-12, the proxy wire check: Go's proxy clients and servers against the
//! Rust ones, in both directions, on one scenario.
//!
//! `contracts/tools/proxygolden` runs the 66-step scenario through Go's proxy
//! clients and servers, and its transcript is the golden file
//! `crates/gorse-storage/tests/data/proxy_transcript.txt`. The Rust side is the
//! `proxy_peer` binary of this crate, which the Go tool drives:
//!
//! - `proxy_oracle_matches_golden`: Go writes the transcript again, and it equals
//!   the golden file.
//! - `proxy_go_client_against_rust_server`: Go's client runs against the Rust
//!   servers.
//! - `proxy_rust_client_against_go_server`: the Rust clients run against Go's
//!   servers.
//!
//! Go must be on PATH, at the version in `go.mod`. A test that cannot run Go
//! fails with the reason; it is never skipped (`conformance/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root: the parent of this crate.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the conformance crate sits in the repository root")
}

/// The golden transcript that Go's oracle writes.
fn golden_path() -> PathBuf {
    repo_root().join("crates/gorse-storage/tests/data/proxy_transcript.txt")
}

/// Runs `go` with `args` from the repository root, and returns its stdout. A
/// missing Go and a failing run are both test failures, with the reason.
fn go(args: &[&str]) -> String {
    let output = Command::new("go")
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "go is not on PATH ({e}): the proxy wire check runs contracts/tools/proxygolden, \
                 which needs Go 1.27.0 (conformance/README.md)"
            )
        });
    assert!(
        output.status.success(),
        "go {} failed ({}):\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Asserts that `got` equals the golden transcript, naming the first line that
/// differs.
fn assert_matches_golden(got: &str, golden: &str) {
    let got_lines: Vec<&str> = got.lines().collect();
    let golden_lines: Vec<&str> = golden.lines().collect();
    for i in 0..got_lines.len().max(golden_lines.len()) {
        let (g, w) = (got_lines.get(i), golden_lines.get(i));
        assert!(
            g == w,
            "transcript line {} differs\n  golden: {}\n  got:    {}",
            i + 1,
            w.unwrap_or(&"<end of transcript>"),
            g.unwrap_or(&"<end of transcript>"),
        );
    }
}

#[test]
fn proxy_oracle_matches_golden() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let written = dir.path().join("proxy_transcript.txt");
    go(&[
        "run",
        "./contracts/tools/proxygolden",
        "-out",
        written.to_str().expect("UTF-8 path"),
    ]);
    let golden = std::fs::read_to_string(golden_path()).expect("the golden transcript");
    let written = std::fs::read_to_string(&written).expect("the oracle's transcript");
    assert_matches_golden(&written, &golden);
}

#[test]
fn proxy_go_client_against_rust_server() {
    let stdout = go(&[
        "run",
        "./contracts/tools/proxygolden",
        "-rust-server",
        env!("CARGO_BIN_EXE_proxy_peer"),
        "-golden",
        golden_path().to_str().expect("UTF-8 path"),
    ]);
    assert!(
        stdout.contains("Go client against Rust server: transcript matches"),
        "unexpected output:\n{stdout}"
    );
}

#[test]
fn proxy_rust_client_against_go_server() {
    let stdout = go(&[
        "run",
        "./contracts/tools/proxygolden",
        "-rust-client",
        env!("CARGO_BIN_EXE_proxy_peer"),
        "-golden",
        golden_path().to_str().expect("UTF-8 path"),
    ]);
    assert!(
        stdout.contains("Rust client against Go server: transcript matches"),
        "unexpected output:\n{stdout}"
    );
}
