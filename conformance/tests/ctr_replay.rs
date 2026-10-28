// Ports: contracts/tools/modelreplay (ctr mode), the golden replay of model/ctr/fm.go @ add21b5

//! WP-3-2, the CTR replay check: Go's replay of the golden model, written again by
//! `contracts/tools/modelreplay -mode ctr`, equals the committed golden file
//! `crates/gorse-model/testdata/ctr/afm_replay.json`.
//!
//! The Rust side is `ctr::replay` in `gorse-model`, which reads the golden file
//! and compares the Rust fit with it. This test checks that the golden file is
//! exactly what Go writes, so it cannot drift from the oracle by hand.
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

/// The golden replay file that Go's oracle writes.
fn golden_path() -> PathBuf {
    repo_root().join("crates/gorse-model/testdata/ctr/afm_replay.json")
}

/// Runs `contracts/tools/modelreplay -mode ctr` from the repository root, writing
/// the replay to `out`. The global random source of the process is seeded with 1
/// (`GODEBUG=randautoseed=0`), as the replay requires, and the generator fits each
/// epoch count in a child process that sets it again. A missing Go and a failing
/// run are both test failures, with the reason.
fn write_replay(out: &Path) {
    let output = Command::new("go")
        .current_dir(repo_root())
        .env("GODEBUG", "randautoseed=0")
        .args([
            "run",
            "./contracts/tools/modelreplay",
            "-mode",
            "ctr",
            "-out",
        ])
        .arg(out)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "go is not on PATH ({e}): the CTR replay check runs contracts/tools/modelreplay, \
                 which needs Go 1.27.0 (conformance/README.md)"
            )
        });
    assert!(
        output.status.success(),
        "go run ./contracts/tools/modelreplay -mode ctr failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Asserts that `got` equals the golden file, naming the first line that differs,
/// and then that the bytes are equal, down to the final newline.
fn assert_matches_golden(got: &str, golden: &str) {
    let got_lines: Vec<&str> = got.lines().collect();
    let golden_lines: Vec<&str> = golden.lines().collect();
    for i in 0..got_lines.len().max(golden_lines.len()) {
        let (g, w) = (got_lines.get(i), golden_lines.get(i));
        assert!(
            g == w,
            "replay line {} differs\n  golden: {}\n  got:    {}",
            i + 1,
            w.unwrap_or(&"<end of file>"),
            g.unwrap_or(&"<end of file>"),
        );
    }
    assert!(
        got == golden,
        "the replay differs from the golden file in its line endings or final newline"
    );
}

#[test]
fn ctr_oracle_matches_golden() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let written = dir.path().join("afm_replay.json");
    write_replay(&written);
    let got = std::fs::read_to_string(&written).expect("the generator writes the replay file");
    let golden = std::fs::read_to_string(golden_path()).expect("the committed golden file");
    assert_matches_golden(&got, &golden);
}
