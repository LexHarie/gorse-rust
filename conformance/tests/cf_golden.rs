// Ports: contracts/tools/modelreplay (the Go oracle for the CF replay data), and crates/gorse-model/testdata/cf (WP-3-1).

//! WP-3-1, the CF golden check: the Go oracle writes the replay data for BPR and
//! ALS again, and its output must equal the committed copies in
//! `crates/gorse-model/testdata/cf`.
//!
//! The Rust side replays those copies in `gorse-model` (`cf::replay`). This test
//! shows that the copies are what the Go code writes, so the replay compares the
//! Rust models with the Go build and not with a file edited by hand.
//!
//! `contracts/tools/modelreplay` fits the Go models on fixed splits with one job
//! and writes `train.tsv`, `test.tsv` and `golden.json`. Go must be on PATH, at the
//! version in `go.mod`. A test that cannot run Go fails with the reason; it is
//! never skipped (`conformance/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The files that `contracts/tools/modelreplay` writes and that are committed.
const FILES: [&str; 3] = ["train.tsv", "test.tsv", "golden.json"];

/// The epoch count of the committed goldens, as the generator's `-epochs` flag.
const EPOCHS: &str = "5";

/// The repository root: the parent of this crate.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the conformance crate sits in the repository root")
}

/// The committed replay data that the Rust replay reads.
fn golden_dir() -> PathBuf {
    repo_root().join("crates/gorse-model/testdata/cf")
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
                "go is not on PATH ({e}): the CF golden check runs contracts/tools/modelreplay, \
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

/// Asserts that `got` equals the committed file `file`, naming the first line
/// that differs.
fn assert_matches_golden(file: &str, got: &str, golden: &str) {
    let got_lines: Vec<&str> = got.lines().collect();
    let golden_lines: Vec<&str> = golden.lines().collect();
    for i in 0..got_lines.len().max(golden_lines.len()) {
        let (g, w) = (got_lines.get(i), golden_lines.get(i));
        assert!(
            g == w,
            "{file} line {} differs from the committed golden\n  golden: {}\n  got:    {}",
            i + 1,
            w.unwrap_or(&"<end of file>"),
            g.unwrap_or(&"<end of file>"),
        );
    }
    assert!(
        got == golden,
        "{file} differs from the committed golden in its line endings or final newline"
    );
}

#[test]
fn cf_oracle_matches_golden() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let out = dir.path();
    go(&[
        "run",
        "./contracts/tools/modelreplay",
        "-mode",
        "cf",
        "-out",
        out.to_str().expect("UTF-8 path"),
        "-epochs",
        EPOCHS,
    ]);
    for file in FILES {
        let written = std::fs::read_to_string(out.join(file))
            .unwrap_or_else(|e| panic!("the oracle did not write {file}: {e}"));
        let golden = std::fs::read_to_string(golden_dir().join(file))
            .unwrap_or_else(|e| panic!("the committed golden {file} is unreadable: {e}"));
        assert_matches_golden(file, &written, &golden);
    }
}
