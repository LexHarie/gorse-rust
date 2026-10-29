//! Go's empty task list. The master's tasks route lists the spans of its tracer,
//! and a tracer with no span lists nil, which Go's JSON writes as `null`. The
//! Go unit test `TestEmptyListMarshalsAsNull` pins that answer. This test runs
//! it on every run, and the Rust tasks route is checked against the same answer
//! by `get_tasks_is_null_without_tasks` in `gorse-master`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, where the Go module is.
fn repo_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .expect("the repository root")
}

#[test]
fn empty_tracer_list_is_null_in_go() {
    let output = Command::new("go")
        .current_dir(repo_root())
        .args([
            "test",
            "-count=1",
            "-v",
            "-run",
            "^TestEmptyListMarshalsAsNull$",
            "./common/monitor",
        ])
        .output()
        .expect("the Go toolchain runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the Go test failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("--- PASS: TestEmptyListMarshalsAsNull"),
        "the Go test did not run:\n{stdout}"
    );
}
