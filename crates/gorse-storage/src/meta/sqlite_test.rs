// Ports: storage/meta/sqlite_test.go @ add21b5

//! Runs the meta suite against SQLite, as Go's `SQLiteTestSuite` does. Each
//! test gets a new file in a temporary directory, with the schema created,
//! and closes it afterwards. SQLite is embedded, so these tests run without
//! any service.

use gorse_common::gotime::SECOND;
use tempfile::TempDir;

use super::{Database, database_test, open};

/// Go's `SetupTest`: a new database file in a temporary directory, with the
/// schema created. The directory is returned so that it lives as long as the
/// test does.
async fn setup() -> (TempDir, Box<dyn Database>) {
    let dir = tempfile::tempdir().unwrap();
    let path = format!("sqlite://{}/sqlite.db", dir.path().display());
    let db = open(&path, SECOND).await.unwrap();
    db.init().await.unwrap();
    (dir, db)
}

/// Go's `TearDownTest`: closes the database.
async fn teardown(db: &dyn Database) {
    db.close().await.unwrap();
}

#[tokio::test]
async fn test_nodes() {
    let (_dir, db) = setup().await;
    database_test::test_nodes(db.as_ref()).await;
    teardown(db.as_ref()).await;
}

#[tokio::test]
async fn test_key_values() {
    let (_dir, db) = setup().await;
    database_test::test_key_values(db.as_ref()).await;
    teardown(db.as_ref()).await;
}
