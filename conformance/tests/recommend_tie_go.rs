// Ports: logics/recommend.go @ add21b5 (X-247).

//! X-247: the Go recommender orders items with equal scores differently from
//! one run to the next, on identical input.
//!
//! `conformance/tests/data/recommend_tie_case.json` holds case-05 of the
//! generated set. `contracts/tools/recommendgolden -replay` runs that case 40
//! times in one process, and each replay runs all three runs of the case again
//! on new stores and a new recommender. The test counts the orders each run
//! gave. The Go side must show a run whose result came out in two or more
//! orders. That is the evidence for the oracle's tie rule (recommend_oracle.rs):
//! items with bit-identical scores are compared as a set, because Go does not
//! fix their order.
//!
//! The test needs only the Go toolchain, so it runs without `-- --ignored`. It
//! fails when Go is missing, and it is never skipped (`conformance/README.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

/// The number of replays the test asks for.
const REPLAYS: u64 = 40;

fn repo_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .expect("the repository root")
}

/// Runs the Go replay, and returns for each run the count of each order.
fn go_orders() -> Vec<BTreeMap<String, u64>> {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("replay.json");
    let output = Command::new("go")
        .current_dir(repo_root())
        .args([
            "run",
            "./contracts/tools/recommendgolden",
            "-replay",
            "conformance/tests/data/recommend_tie_case.json",
            "-case",
            "case-05",
            "-replays",
        ])
        .arg(REPLAYS.to_string())
        .arg("-out")
        .arg(&out)
        .output()
        .unwrap_or_else(|e| {
            panic!("starting go: {e}; the tie test needs the Go toolchain (conformance/README.md)")
        });
    assert!(
        output.status.success(),
        "go run ./contracts/tools/recommendgolden -replay failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("the Go replay did not write {}: {e}", out.display()));
    let report: Json = serde_json::from_str(&text).expect("the replay report is JSON");
    assert_eq!(report["case"], "case-05", "the replay ran the wrong case");
    report["runs"]
        .as_array()
        .expect("the runs")
        .iter()
        .map(|run| {
            run["orders"]
                .as_object()
                .expect("the orders of a run")
                .iter()
                .map(|(order, count)| (order.clone(), count.as_u64().expect("a count")))
                .collect()
        })
        .collect()
}

#[test]
fn go_orders_equal_scores_differently_on_identical_input() {
    let runs = go_orders();
    assert_eq!(runs.len(), 3, "case-05 has three runs");
    for (run, orders) in runs.iter().enumerate() {
        let total: u64 = orders.values().sum();
        assert_eq!(total, REPLAYS, "run {run} was not replayed {REPLAYS} times");
    }
    // Go's order of tied items varies between runs on the same input, so some
    // run must give two or more orders. A run with one order everywhere shows
    // nothing about ties, so the test asks for at least one varying run.
    let varying: Vec<String> = runs
        .iter()
        .enumerate()
        .filter(|(_, orders)| orders.len() >= 2)
        .map(|(run, orders)| format!("run {run}: {orders:?}"))
        .collect();
    assert!(
        !varying.is_empty(),
        "Go gave one order in every replay, so the tie order did not vary (X-247)"
    );
    eprintln!(
        "X-247: runs whose tie order varied in Go: {}",
        varying.join("; ")
    );
}
