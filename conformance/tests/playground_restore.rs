//! The GitHub playground dump restored through the Go master and through the Rust
//! master, with the rows each run stores counted in its SQLite file. The counts
//! must match Go's.
//!
//! The dump is `/mnt/project-files/datasets/github.bin.gz`, the archive the
//! playground import downloads. The Go master restores it with `POST /api/restore`,
//! and the Rust master with `Master::restore`, which that route calls. Both
//! restores read the decompressed dump.
//!
//! The test fails, and does not skip, when the Go toolchain is missing or a node
//! does not start.

use std::io::Read;
use std::path::Path;

use axum::body::Bytes;
use flate2::read::GzDecoder;
use gorse_conformance::cluster::{Cluster, GoBuild};

/// The GitHub playground archive, as published in the project's dataset folder.
const GITHUB_ARCHIVE: &str = "/mnt/project-files/datasets/github.bin.gz";

/// The tables the restore writes, in the order of [`EXPECTED`].
const TABLES: [&str; 3] = ["users", "items", "feedback"];

/// The rows each table holds after the restore. The dump holds 357,294 feedback
/// records. 25,393 of them name a user that the dump does not hold, and the
/// restore drops them, as Go's `BatchInsertFeedback` does with `insertUser` false.
const EXPECTED: [i64; 3] = [2_079, 22_320, 331_901];

#[tokio::test(flavor = "multi_thread")]
async fn github_playground_restore_counts_match_go() {
    let build = GoBuild::shared().expect("the Go nodes build");
    let dump = Bytes::from(read_dump());

    let go_dir = tempfile::tempdir().expect("a directory for the Go run");
    let go_db = go_dir.path().join("data.db");
    let mut go = Cluster::new_with_store(build, &[], Some(&sqlite_uri(&go_db)))
        .expect("the Go run's config");
    go.start_master().await.expect("start the Go master");
    let http = go.ports.master_http;
    go.wait_for("the Go master HTTP port", move || async move {
        reqwest::get(format!("http://127.0.0.1:{http}/apidocs.json"))
            .await
            .is_ok_and(|response| response.status().is_success())
    })
    .await
    .expect("the Go master HTTP port answers");
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http}/api/restore"))
        .body(dump.clone())
        .send()
        .await
        .expect("POST /api/restore to the Go master");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(status, reqwest::StatusCode::OK, "the Go restore: {body}");
    go.stop();
    let go_counts = row_counts(&go_db);

    let rust_dir = tempfile::tempdir().expect("a directory for the Rust run");
    let rust_db = rust_dir.path().join("data.db");
    let mut rust = Cluster::new_with_store(build, &[], Some(&sqlite_uri(&rust_db)))
        .expect("the Rust run's config");
    let master = rust
        .start_rust_master()
        .await
        .expect("start the Rust master");
    master
        .restore(&dump)
        .await
        .expect("restore through the Rust master");
    master.stop().await;
    let rust_counts = row_counts(&rust_db);
    eprintln!("stored rows of {TABLES:?}: Go {go_counts:?}, Rust {rust_counts:?}");

    assert_eq!(
        go_counts, rust_counts,
        "rows stored by the Go and the Rust restore"
    );
    assert_eq!(go_counts, EXPECTED, "rows stored by the Go restore");
}

/// The decompressed GitHub playground dump.
fn read_dump() -> Vec<u8> {
    let archive = std::fs::read(GITHUB_ARCHIVE)
        .unwrap_or_else(|err| panic!("read the playground archive {GITHUB_ARCHIVE}: {err}"));
    let mut dump = Vec::new();
    GzDecoder::new(&archive[..])
        .read_to_end(&mut dump)
        .expect("decompress the playground archive");
    dump
}

/// The SQLite URI the Go and the Rust nodes open `path` by.
fn sqlite_uri(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

/// The rows of each of [`TABLES`] in the SQLite file at `db`, read from the file.
fn row_counts(db: &Path) -> [i64; 3] {
    let connection = rusqlite::Connection::open(db).expect("open the stored data");
    TABLES.map(|table| {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count the rows")
    })
}
