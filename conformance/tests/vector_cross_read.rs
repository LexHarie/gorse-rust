//! The vector stores, cross-read: the Rust stores and the Go stores write and read
//! the same dataset on Qdrant, Milvus and Weaviate, in both directions.
//!
//! The Go side is `contracts/tools/vectorxread`, run from this tree with `go run`.
//! Both directions need the Go toolchain and the three servers from
//! `storage/docker-compose.yml`, so the tests are `#[ignore]`d and `cargo test` stays
//! offline. Run them with:
//!
//! ```text
//! QDRANT_URI=qdrant://127.0.0.1:6334 MILVUS_URI=milvus://127.0.0.1:19530 \
//! WEAVIATE_URI=weaviate://127.0.0.1:8080 \
//!     cargo test -p gorse-conformance --test vector_cross_read -- --ignored --nocapture
//! ```
//!
//! Once they run, a missing Go toolchain, an unset URI or a server that does not
//! answer fails the test. Nothing is skipped.

use std::path::Path;
use std::process::Command;

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_storage::vectors::{Database, Distance, QuantizationType, Vector, VectorConfig, open};
use gorse_storage_core::options::Opt;

/// The timestamps of the dataset, in milliseconds. The Go tool writes the same.
const TS_A: i64 = 1_800_000_000_123;
const TS_B: i64 = 1_800_000_001_456;
const TS_C: i64 = 1_800_000_002_789;

/// A backend the cross-read covers. Weaviate has no sparse vectors: Go's `TestSparse`
/// skips it.
struct Backend {
    env: &'static str,
    name: &'static str,
    sparse: bool,
}

const BACKENDS: [Backend; 3] = [
    Backend {
        env: "QDRANT_URI",
        name: "Qdrant",
        sparse: true,
    },
    Backend {
        env: "MILVUS_URI",
        name: "Milvus",
        sparse: true,
    },
    Backend {
        env: "WEAVIATE_URI",
        name: "Weaviate",
        sparse: false,
    },
];

/// An open store.
struct Store {
    name: &'static str,
    db: Box<dyn Database>,
    sparse: bool,
}

/// Opens every backend the cross-read covers, and checks that each one answers. An
/// unset URI or a server that does not answer panics, naming the server.
async fn open_all() -> Vec<Store> {
    let mut stores = Vec::new();
    for backend in &BACKENDS {
        let uri = std::env::var(backend.env)
            .ok()
            .filter(|uri| !uri.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "{} is not set: the vector cross-read needs a live {} server; start it from storage/docker-compose.yml and set {}",
                    backend.env, backend.name, backend.env
                )
            });
        let db = open(&uri, "gorse_", &[] as &[Opt])
            .await
            .unwrap_or_else(|e| panic!("cannot open {} at {uri}: {e}", backend.name));
        db.init()
            .await
            .unwrap_or_else(|e| panic!("cannot initialise {} at {uri}: {e}", backend.name));
        if let Err(e) = db.list_collections(&Context::background()).await {
            panic!(
                "cannot reach {} at {uri}: {e}; is the server from storage/docker-compose.yml running?",
                backend.name
            );
        }
        stores.push(Store {
            name: backend.name,
            db,
            sparse: backend.sparse,
        });
    }
    stores
}

async fn close_all(stores: Vec<Store>) {
    for store in stores {
        store
            .db
            .close()
            .await
            .unwrap_or_else(|e| panic!("cannot close {}: {e}", store.name));
    }
}

/// Fails unless the Go toolchain is on `PATH`.
fn require_go() {
    let output = Command::new("go")
        .arg("version")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "the Go toolchain is needed for the vector cross-read: cannot run `go version`: {e}"
            )
        });
    assert!(
        output.status.success(),
        "`go version` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs the Go side in one mode over one prefix, from the repository root. The Go
/// side asserts its own reads and writes, so a failure carries its output.
fn run_go(mode: &str, prefix: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let output = Command::new("go")
        .current_dir(&root)
        .args([
            "run",
            "./contracts/tools/vectorxread",
            "-mode",
            mode,
            "-prefix",
            prefix,
        ])
        .output()
        .unwrap_or_else(|e| panic!("cannot run go: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "go run ./contracts/tools/vectorxread -mode {mode} -prefix {prefix} failed:\n{stdout}{stderr}"
    );
    print!("{stdout}");
}

fn dense(id: &str, values: &[f32], categories: &[&str], hidden: bool, ms: i64) -> Vector {
    Vector {
        id: id.to_owned(),
        values: values.to_vec(),
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        is_hidden: hidden,
        timestamp: gotime::unix_milli(ms),
        ..Default::default()
    }
}

fn dense_vectors() -> Vec<Vector> {
    vec![
        dense(
            "a",
            &[1.0, 0.0, 0.0, 0.0],
            &["cat-a", "common"],
            false,
            TS_A,
        ),
        dense(
            "b",
            &[0.9, 0.1, 0.0, 0.0],
            &["cat-b", "common"],
            false,
            TS_B,
        ),
        dense("c", &[0.0, 1.0, 0.0, 0.0], &["common"], true, TS_C),
    ]
}

fn sparse_vectors() -> Vec<Vector> {
    vec![
        Vector {
            id: "s1".to_owned(),
            indices: vec![1, 100],
            values: vec![1.0, 2.0],
            categories: vec!["common".to_owned()],
            is_hidden: false,
            timestamp: gotime::unix_milli(TS_A),
            ..Default::default()
        },
        Vector {
            id: "s2".to_owned(),
            indices: vec![2, 5],
            values: vec![0.5, 0.5],
            categories: vec!["common".to_owned()],
            is_hidden: true,
            timestamp: gotime::unix_milli(TS_B),
            ..Default::default()
        },
    ]
}

fn ids(vectors: &[Vector]) -> Vec<&str> {
    vectors.iter().map(|v| v.id.as_str()).collect()
}

fn assert_close(expected: &[f32], actual: &[f32]) {
    assert_eq!(expected.len(), actual.len(), "length of {actual:?}");
    for (e, a) in expected.iter().zip(actual) {
        assert!(
            (e - a).abs() < 1e-2,
            "expected {expected:?}, got {actual:?}"
        );
    }
}

/// Cosine stores may normalise a vector (Qdrant does), so dense values are compared
/// by direction.
fn assert_direction(expected: &[f32], actual: &[f32]) {
    let unit = |values: &[f32]| -> Vec<f32> {
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        values.iter().map(|v| v / norm).collect()
    };
    assert_close(&unit(expected), &unit(actual));
}

fn names(prefix: &str) -> [String; 3] {
    [
        format!("{prefix}_dense"),
        format!("{prefix}_sq"),
        format!("{prefix}_sparse"),
    ]
}

async fn write(db: &dyn Database, ctx: &Context, prefix: &str, sparse: bool) {
    let [dense_name, sq_name, sparse_name] = names(prefix);
    for name in [&dense_name, &sq_name, &sparse_name] {
        let _ = db.delete_collection(ctx, name).await;
    }
    db.add_collection(
        ctx,
        &dense_name,
        4,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .await
    .expect("add dense");
    db.add_vectors(ctx, &dense_name, &dense_vectors())
        .await
        .expect("write dense");
    db.add_collection(
        ctx,
        &sq_name,
        4,
        Distance::Cosine,
        VectorConfig {
            quantization: QuantizationType::Sq,
            bits: 0,
        },
    )
    .await
    .expect("add sq");
    db.add_vectors(ctx, &sq_name, &dense_vectors())
        .await
        .expect("write sq");
    if sparse {
        db.add_collection(ctx, &sparse_name, 0, Distance::Dot, VectorConfig::default())
            .await
            .expect("add sparse");
        db.add_vectors(ctx, &sparse_name, &sparse_vectors())
            .await
            .expect("write sparse");
    }
}

async fn check(db: &dyn Database, ctx: &Context, prefix: &str, sparse: bool) {
    let [dense_name, sq_name, sparse_name] = names(prefix);

    // Weaviate's describe reports no dimension, as Go's does not.
    let dimension = if sparse { 4 } else { 0 };
    let info = db
        .describe_collection(ctx, &dense_name)
        .await
        .expect("describe dense");
    assert_eq!(info.dimension, dimension);
    assert_eq!(info.distance, Distance::Cosine);
    assert_eq!(info.config, VectorConfig::default());
    assert_eq!(
        db.count_vectors(ctx, &dense_name)
            .await
            .expect("count dense"),
        3
    );
    let got = db
        .get_vectors(ctx, &dense_name, &["a", "b", "c", "zz"])
        .await
        .expect("get dense");
    assert_eq!(ids(&got), ["a", "b", "c"]);
    for (vector, expected) in got.iter().zip(dense_vectors()) {
        assert_direction(&expected.values, &vector.values);
        assert_eq!(
            vector.categories, expected.categories,
            "categories of {}",
            vector.id
        );
        assert_eq!(
            vector.is_hidden, expected.is_hidden,
            "hidden of {}",
            vector.id
        );
        assert_eq!(
            vector.timestamp.unix_milli(),
            expected.timestamp.unix_milli(),
            "timestamp of {}",
            vector.id
        );
    }
    let query = Vector {
        values: vec![1.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let hits = db
        .query_vectors(ctx, &dense_name, &query, &["common"], 10)
        .await
        .expect("query dense");
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.vector.id.as_str()).collect();
    assert_eq!(hit_ids, ["a", "b"], "query hits");
    assert!(hits[0].score >= hits[1].score, "scores {hits:?}");

    let info = db
        .describe_collection(ctx, &sq_name)
        .await
        .expect("describe sq");
    assert_eq!(info.dimension, dimension);
    assert_eq!(info.distance, Distance::Cosine);
    assert_eq!(info.config.quantization, QuantizationType::Sq, "sq config");
    let got = db
        .get_vectors(ctx, &sq_name, &["a", "b", "c"])
        .await
        .expect("get sq");
    assert_eq!(ids(&got), ["a", "b", "c"]);
    assert_direction(&dense_vectors()[1].values, &got[1].values);

    if !sparse {
        return;
    }
    let info = db
        .describe_collection(ctx, &sparse_name)
        .await
        .expect("describe sparse");
    assert_eq!(info.dimension, 0);
    assert_eq!(info.distance, Distance::Dot);
    assert_eq!(info.config, VectorConfig::default());
    assert_eq!(
        db.count_vectors(ctx, &sparse_name)
            .await
            .expect("count sparse"),
        2
    );
    let got = db
        .get_vectors(ctx, &sparse_name, &["s1", "s2"])
        .await
        .expect("get sparse");
    assert_eq!(ids(&got), ["s1", "s2"]);
    assert_eq!(got[0].indices, [1, 100], "indices of s1");
    assert_close(&[1.0, 2.0], &got[0].values);
    assert_eq!(got[0].categories, ["common"]);
    assert!(!got[0].is_hidden, "s1 is hidden");
    assert_eq!(got[0].timestamp.unix_milli(), TS_A);
    assert!(got[1].is_hidden, "s2 is not hidden");
    let query = Vector {
        indices: vec![1],
        values: vec![1.0],
        ..Default::default()
    };
    let hits = db
        .query_vectors(ctx, &sparse_name, &query, &["common"], 10)
        .await
        .expect("query sparse");
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.vector.id.as_str()).collect();
    assert_eq!(hit_ids, ["s1"], "sparse query hits");
}

/// Deletes the collections of a prefix, so a run leaves nothing behind.
async fn cleanup(db: &dyn Database, ctx: &Context, prefix: &str) {
    for name in names(prefix) {
        let _ = db.delete_collection(ctx, &name).await;
    }
}

/// Rust writes `xr_*` on every backend and the Go tool reads and asserts it.
#[tokio::test]
#[ignore = "needs Go and the Qdrant, Milvus and Weaviate servers; see the module docs"]
async fn rust_writes_go_checks() {
    require_go();
    let stores = open_all().await;
    let ctx = Context::background();
    for store in &stores {
        write(store.db.as_ref(), &ctx, "xr", store.sparse).await;
        println!("rust wrote xr on {}", store.name);
    }
    close_all(stores).await;
    run_go("check", "xr");
}

/// The Go tool writes `xg_*` on every backend and Rust reads and asserts it.
#[tokio::test]
#[ignore = "needs Go and the Qdrant, Milvus and Weaviate servers; see the module docs"]
async fn go_writes_rust_checks() {
    require_go();
    close_all(open_all().await).await;
    run_go("write", "xg");
    let stores = open_all().await;
    let ctx = Context::background();
    for store in &stores {
        check(store.db.as_ref(), &ctx, "xg", store.sparse).await;
        cleanup(store.db.as_ref(), &ctx, "xg").await;
        println!("rust checked xg on {}", store.name);
    }
    close_all(stores).await;
}
