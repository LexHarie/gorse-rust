// Ports: logics/vector_writer.go (sparseVectorPruner, appendSparseVector) @ add21b5 (WP-3-4).

//! WP-3-4, the sparse vectors of the item-to-item and user-to-user recommenders.
//!
//! The Go side is `conformance/testdata/sparse_oracle_test.go`. The test puts it
//! into the Go `logics` package with `go test -overlay`, so the unexported
//! pruner and `appendSparseVector` that run are the Go ones, and the Go tree is
//! not changed. Go generates 420 cases from seeded random inputs (NaN,
//! infinities, ties, duplicate indices, and appends that reach the 1024-entry
//! limit). Rust must return the same indices and the same FP32 bits for each.
//!
//! The test fails when the Go toolchain is missing; it is never skipped
//! (`conformance/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;

use gorse_logics::vector_writer::{SparseVectorPruner, append_sparse_vector};
use gorse_storage::vectors::Vector;
use serde_json::{Value as Json, json};

/// The cases the Go test writes: 400 pruner cases and 20 `appendSparseVector`
/// cases.
const CASES: usize = 420;

fn repo_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .expect("the repository root")
}

/// A JSON list of unsigned numbers. Go writes a nil slice as `null`, which is
/// the empty list.
fn u32s(v: &Json) -> Vec<u32> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| u32::try_from(x.as_u64().expect("uint")).expect("uint32"))
        .collect()
}

fn i32s(v: &Json) -> Vec<i32> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|x| i32::try_from(x.as_i64().expect("int")).expect("int32"))
        .collect()
}

/// FP32 values as their bits, so that NaN payloads compare exactly.
fn floats(bits: &Json) -> Vec<f32> {
    u32s(bits).into_iter().map(f32::from_bits).collect()
}

/// An unsigned number, where an absent field (Go's `omitempty`) is 0.
fn uint(v: &Json) -> u32 {
    v.as_u64().map_or(0, |x| u32::try_from(x).expect("uint32"))
}

/// Runs the Go test that writes the cases, and returns them.
fn go_cases() -> Vec<Json> {
    let root = repo_root();
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("cases.json");
    // The overlay adds the oracle file to package logics for this build only.
    let oracle = std::fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sparse_oracle_test.go"),
    )
    .expect("the Go oracle");
    let target = root.join("logics/sparse_oracle_conformance_test.go");
    let mut replace = serde_json::Map::new();
    replace.insert(
        target.display().to_string(),
        Json::String(oracle.display().to_string()),
    );
    let overlay_path = dir.path().join("overlay.json");
    std::fs::write(&overlay_path, json!({ "Replace": replace }).to_string())
        .expect("write the overlay");

    let output = Command::new("go")
        .current_dir(&root)
        .args(["test", "-count=1", "-run", "^TestSparseOracle$"])
        .arg(format!("-overlay={}", overlay_path.display()))
        .arg("./logics")
        .env("SPARSE_ORACLE_OUT", &out)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "starting go: {e}; the sparse oracle needs the Go toolchain \
                 (conformance/README.md)"
            )
        });
    assert!(
        output.status.success(),
        "go test ./logics -run TestSparseOracle failed ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&out)
        .unwrap_or_else(|e| panic!("the Go test did not write {}: {e}", out.display()));
    serde_json::from_str(&text).expect("the Go cases are JSON")
}

/// The pruner and `appendSparseVector` agree with Go bit for bit on every case,
/// including the order of ties and the handling of NaN.
#[test]
fn sparse_vectors_match_go() {
    let cases = go_cases();
    assert_eq!(cases.len(), CASES, "cases");
    for (n, case) in cases.iter().enumerate() {
        let (indices, values) = match case["kind"].as_str().expect("kind") {
            "pruner" => {
                let max_nnz = usize::try_from(case["max_nnz"].as_u64().unwrap_or(0))
                    .expect("max_nnz fits in usize");
                let mut pruner = SparseVectorPruner::new(max_nnz);
                for add in case["adds"].as_array().into_iter().flatten() {
                    pruner.add(uint(&add["index"]), f32::from_bits(uint(&add["value"])));
                }
                pruner.result()
            }
            "append" => {
                let vector = Vector {
                    indices: u32s(&case["in_indices"]),
                    values: floats(&case["in_values"]),
                    ..Vector::default()
                };
                let out = append_sparse_vector(
                    vector,
                    &i32s(&case["ids"]),
                    &floats(&case["idf"]),
                    uint(&case["offset"]),
                );
                (out.indices, out.values)
            }
            other => panic!("case {n}: unknown kind {other}"),
        };
        let bits: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
        assert_eq!(indices, u32s(&case["indices"]), "case {n} indices");
        assert_eq!(bits, u32s(&case["values"]), "case {n} values");
    }
}
