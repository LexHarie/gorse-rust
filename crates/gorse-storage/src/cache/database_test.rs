// Ports: storage/cache/database_test.go @ add21b5 (the suite, TestKey; the
// benchmarks and their prime table are not ported, since `go test` does not
// run them)

//! The suite every cache backend runs against a real service. The backend
//! tests (WP-2-5 Redis, WP-2-6 SQL, WP-2-7 MongoDB) call [`run`] with their
//! store. No backend exists in this crate yet, so [`run`] is compiled and not
//! called here; only `TestKey` runs in this crate.
//!
//! Go's testify suite becomes plain functions. Each one does the setup and
//! teardown that `SetupTest` and `TearDownTest` do, through [`run`].
//!
//! A Go test deadline (`context.WithTimeout`) is ported as a context that a
//! timer thread cancels, since `gorse_common::gocontext` has no deadlines.

#![allow(dead_code, reason = "called by the backend tests of WP-2-5..2-7")]

use std::time::Duration as StdDuration;

use gorse_common::gocontext::{Context, ContextError};
use gorse_common::gotime::{self, Duration, Month, Time};
use gorse_storage_core::errors::Error as StorageError;

use super::{
    Database, Error, Score, ScoreCondition, ScorePatch, TimeSeriesPoint, integer, key, string, time,
};

/// Runs every suite test against `db`, each on a purged store as Go's
/// `SetupTest` leaves it. The store is not closed.
pub async fn run(db: &dyn Database) {
    test_init(db).await;
    test_meta(db).await;
    test_exists(db).await;
    test_scan(db).await;
    test_purge(db).await;
    test_document(db).await;
    test_subset_document(db).await;
    test_scan_scores(db).await;
    test_time_series(db).await;
    test_timestamp_precision(db).await;
}

/// Go's `SetupTest`: pings the store and purges it.
async fn setup(db: &dyn Database) {
    db.ping().await.unwrap();
    db.purge().await.unwrap();
}

/// Go's `TearDownTest`: purges the store.
async fn teardown(db: &dyn Database) {
    db.purge().await.unwrap();
}

/// A UTC date at midnight.
fn utc(year: i64, month: Month, day: i64) -> Time {
    gotime::date(year, month, day, 0, 0, 0, 0, &gotime::utc())
}

/// The 2023-01-01 timestamp most cases use.
fn ts() -> Time {
    utc(2023, Month::JANUARY, 1)
}

/// A score document with the given fields and no hidden flag.
fn doc(id: &str, score: f64, categories: &[&str], timestamp: Time) -> Score {
    Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: strs(categories),
        timestamp,
    }
}

/// Owned strings from string literals.
fn strs(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|x| (*x).to_owned()).collect()
}

/// The `Some` a Go `new("...")` argument makes.
fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

/// Compares two score lists in order. Timestamps compare as instants, as
/// testify compares `time.Time` values for the same instant.
fn assert_scores(got: &[Score], want: &[Score]) {
    let key = |d: &Score| {
        (
            d.id.clone(),
            d.score,
            d.is_hidden,
            d.categories.clone(),
            d.timestamp.unix_nano(),
        )
    };
    let got: Vec<_> = got.iter().map(key).collect();
    let want: Vec<_> = want.iter().map(key).collect();
    assert_eq!(got, want);
}

/// Go's `TestInit`.
async fn test_init(db: &dyn Database) {
    setup(db).await;
    db.init().await.unwrap();
    teardown(db).await;
}

/// Go's `TestMeta`.
async fn test_meta(db: &dyn Database) {
    setup(db).await;
    let ctx = Context::background();
    // Set meta string.
    db.set(
        &ctx,
        &[
            string(key(&["meta", "1"]), "2"),
            string(key(&["meta", "1000"]), "10"),
        ],
    )
    .await
    .unwrap();
    // Get meta string.
    assert_eq!(
        db.get(&ctx, &key(&["meta", "1"])).await.string().unwrap(),
        "2"
    );
    assert_eq!(
        db.get(&ctx, &key(&["meta", "1000"]))
            .await
            .string()
            .unwrap(),
        "10"
    );
    // Delete string.
    db.delete(&ctx, &key(&["meta", "1"])).await.unwrap();
    // Get meta not existed.
    let ret = db.get(&ctx, &key(&["meta", "1"])).await;
    assert!(!ret.exists());
    assert_eq!(ret.string().unwrap(), "");
    // Set meta int.
    db.set(&ctx, &[integer(key(&["meta", "1"]), 2)])
        .await
        .unwrap();
    // Get meta int.
    assert_eq!(
        db.get(&ctx, &key(&["meta", "1"])).await.integer().unwrap(),
        2
    );
    // Set meta time.
    db.set(
        &ctx,
        &[time(key(&["meta", "1"]), utc(1996, Month::APRIL, 8))],
    )
    .await
    .unwrap();
    // Get meta time.
    let val_time = db.get(&ctx, &key(&["meta", "1"])).await.time().unwrap();
    assert_eq!(val_time.year(), 1996);
    assert_eq!(val_time.month(), Month::APRIL);
    assert_eq!(val_time.day(), 8);

    // Set nothing.
    db.set(&ctx, &[]).await.unwrap();
    // Set a duplicate.
    db.set(&ctx, &[string("100", "1"), string("100", "2")])
        .await
        .unwrap();
    teardown(db).await;
}

/// Go's `TestExists`.
async fn test_exists(db: &dyn Database) {
    setup(db).await;
    let ctx = Context::background();

    // A key that does not exist.
    let ret = db.get(&ctx, &key(&["test", "nonexistent"])).await;
    assert!(!ret.exists());
    assert_eq!(ret.string().unwrap(), "");

    // Set a value.
    db.set(&ctx, &[string(key(&["test", "exists"]), "somevalue")])
        .await
        .unwrap();

    // A key that exists.
    let ret = db.get(&ctx, &key(&["test", "exists"])).await;
    assert!(ret.exists());
    assert_eq!(ret.string().unwrap(), "somevalue");

    // Delete the key.
    db.delete(&ctx, &key(&["test", "exists"])).await.unwrap();

    // A deleted key no longer exists.
    assert!(!db.get(&ctx, &key(&["test", "exists"])).await.exists());

    // Integer values.
    db.set(&ctx, &[integer(key(&["test", "int"]), 42)])
        .await
        .unwrap();
    let ret = db.get(&ctx, &key(&["test", "int"])).await;
    assert!(ret.exists());
    assert_eq!(ret.integer().unwrap(), 42);

    // A missing integer reads as 0 with no error.
    let ret = db.get(&ctx, &key(&["test", "noint"])).await;
    assert!(!ret.exists());
    assert_eq!(ret.integer().unwrap(), 0);
    teardown(db).await;
}

/// Go's `TestScan`.
async fn test_scan(db: &dyn Database) {
    setup(db).await;
    let ctx = Context::background();
    db.set(&ctx, &[string("1", "1")]).await.unwrap();

    let mut keys = Vec::new();
    db.scan(&mut |s: &str| {
        keys.push(s.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    keys.sort();
    assert_eq!(keys, ["1"]);
    teardown(db).await;
}

/// Go's `TestPurge`.
async fn test_purge(db: &dyn Database) {
    setup(db).await;
    let ctx = Context::background();
    // Insert data.
    db.set(&ctx, &[string("key", "value")]).await.unwrap();
    let ret = db.get(&ctx, "key").await;
    assert_eq!(ret.string().unwrap(), "value");
    assert!(ret.exists());

    // Purge data.
    db.purge().await.unwrap();
    assert!(!db.get(&ctx, "key").await.exists());

    // Purge an empty store.
    db.purge().await.unwrap();
    teardown(db).await;
}

/// Go's `TestDocument`.
async fn test_document(db: &dyn Database) {
    setup(db).await;
    let ts = ts();
    let ctx = Context::background();
    db.add_scores(
        &ctx,
        "a",
        "",
        &[Score {
            id: "0".to_owned(),
            score: f64::MAX,
            is_hidden: true,
            categories: strs(&["a", "b"]),
            timestamp: ts.clone(),
        }],
    )
    .await
    .unwrap();
    db.add_scores(&ctx, "a", "", &[doc("1", 100.0, &["a", "b"], ts.clone())])
        .await
        .unwrap();
    let old = utc(2020, Month::JANUARY, 1);
    db.add_scores(
        &ctx,
        "a",
        "",
        &[
            doc("1", 1.0, &["a", "b"], ts.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
            doc("3", 3.0, &["b"], old.clone()),
            doc("4", 4.0, &[""], ts.clone()),
            doc("5", 5.0, &["b"], ts.clone()),
        ],
    )
    .await
    .unwrap();
    db.add_scores(&ctx, "b", "", &[doc("6", 6.0, &["b"], ts.clone())])
        .await
        .unwrap();

    // Search documents.
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["b"]), 1, 3)
        .await
        .unwrap();
    assert_scores(
        &documents,
        &[
            doc("3", 3.0, &["b"], old.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
        ],
    );
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["b"]), 0, -1)
        .await
        .unwrap();
    assert_scores(
        &documents,
        &[
            doc("5", 5.0, &["b"], ts.clone()),
            doc("3", 3.0, &["b"], old.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
            doc("1", 1.0, &["a", "b"], ts.clone()),
        ],
    );

    // Search documents with a nil category.
    let documents = db.search_scores(&ctx, "a", "", &[], 0, -1).await.unwrap();
    assert_scores(
        &documents,
        &[
            doc("5", 5.0, &["b"], ts.clone()),
            doc("4", 4.0, &[""], ts.clone()),
            doc("3", 3.0, &["b"], old.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
            doc("1", 1.0, &["a", "b"], ts.clone()),
        ],
    );

    // Search documents with the empty category.
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&[""]), 0, -1)
        .await
        .unwrap();
    assert_scores(&documents, &[doc("4", 4.0, &[""], ts.clone())]);

    // Delete nothing: a condition with no field set is an error.
    let err = db
        .delete_scores(&ctx, &strs(&["a"]), &ScoreCondition::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), Some(StorageError::InvalidArgument));
    // Delete by value.
    db.delete_scores(
        &ctx,
        &strs(&["a"]),
        &ScoreCondition {
            id: some("5"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "3");
    // Delete by timestamp.
    db.delete_scores(
        &ctx,
        &strs(&["a"]),
        &ScoreCondition {
            before: Some(utc(2022, Month::JANUARY, 1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");

    // Update categories.
    db.update_scores(
        &ctx,
        &strs(&["a"]),
        None,
        "2",
        &ScorePatch {
            categories: Some(strs(&["c", "s"])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["s"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");
    db.update_scores(
        &ctx,
        &strs(&["a"]),
        None,
        "2",
        &ScorePatch {
            categories: Some(strs(&["c"])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["s"]), 0, 1)
        .await
        .unwrap();
    assert!(documents.is_empty());

    // Update is hidden.
    db.update_scores(
        &ctx,
        &strs(&["a"]),
        None,
        "0",
        &ScorePatch {
            is_hidden: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "0");
    teardown(db).await;
}

/// Go's `TestSubsetDocument`.
async fn test_subset_document(db: &dyn Database) {
    setup(db).await;
    let ts = ts();
    let ctx = Context::background();
    db.add_scores(
        &ctx,
        "a",
        "a",
        &[
            doc("1", 1.0, &["a", "b"], ts.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
            doc("3", 3.0, &["b"], ts.clone()),
        ],
    )
    .await
    .unwrap();
    db.add_scores(
        &ctx,
        "b",
        "",
        &[
            doc("4", 4.0, &[], ts.clone()),
            doc("3", 3.0, &["b"], ts.clone()),
            doc("2", 2.0, &["b"], ts.clone()),
        ],
    )
    .await
    .unwrap();

    // Search documents.
    let documents = db
        .search_scores(&ctx, "a", "a", &strs(&["b"]), 0, -1)
        .await
        .unwrap();
    assert_scores(
        &documents,
        &[
            doc("3", 3.0, &["b"], ts.clone()),
            doc("2", 2.0, &["b", "c"], ts.clone()),
            doc("1", 1.0, &["a", "b"], ts.clone()),
        ],
    );

    // Update categories.
    db.update_scores(
        &ctx,
        &strs(&["a", "b"]),
        None,
        "2",
        &ScorePatch {
            categories: Some(strs(&["b", "s"])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "a", &strs(&["s"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");
    let documents = db
        .search_scores(&ctx, "b", "", &strs(&["s"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");

    // Update categories in a subset.
    db.update_scores(
        &ctx,
        &strs(&["a", "b"]),
        Some("a"),
        "2",
        &ScorePatch {
            categories: Some(strs(&["b", "x"])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "a", &strs(&["x"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");
    let documents = db
        .search_scores(&ctx, "b", "", &strs(&["x"]), 0, 1)
        .await
        .unwrap();
    assert!(documents.is_empty());

    // Delete by value.
    db.delete_scores(
        &ctx,
        &strs(&["a", "b"]),
        &ScoreCondition {
            id: some("3"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "a", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");
    let documents = db
        .search_scores(&ctx, "b", "", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");

    // Delete in a subset.
    db.delete_scores(
        &ctx,
        &strs(&["a", "b"]),
        &ScoreCondition {
            subset: some("a"),
            id: some("2"),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let documents = db
        .search_scores(&ctx, "a", "a", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "1");
    let documents = db
        .search_scores(&ctx, "b", "", &strs(&["b"]), 0, 1)
        .await
        .unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].id, "2");
    teardown(db).await;
}

/// Go's `TestScanScores`.
async fn test_scan_scores(db: &dyn Database) {
    setup(db).await;
    // Add scores.
    let timestamp = utc(2023, Month::JANUARY, 1);
    let scores = [
        (
            ("a", "b"),
            vec![
                doc("1", 1.0, &["a", "b"], timestamp.clone()),
                doc("2", 2.0, &["b", "c"], timestamp.clone()),
                doc("3", 3.0, &["b"], timestamp.clone()),
            ],
        ),
        (
            ("a", "c"),
            vec![
                doc("4", 4.0, &["a", "b"], timestamp.clone()),
                doc("5", 5.0, &["b", "c"], timestamp.clone()),
                doc("6", 6.0, &["b"], timestamp.clone()),
            ],
        ),
        (
            ("b", "c"),
            vec![
                doc("7", 7.0, &["a", "b"], timestamp.clone()),
                doc("8", 8.0, &["b", "c"], timestamp.clone()),
                doc("9", 9.0, &["b"], timestamp.clone()),
            ],
        ),
    ];
    let ctx = Context::background();
    for ((collection, subset), documents) in &scores {
        db.add_scores(&ctx, collection, subset, documents)
            .await
            .unwrap();
    }

    // Scan scores.
    let mut total_scores = 0;
    db.scan_scores(&ctx, &mut |_: &str, _: &str, _: &str, t: Time| {
        total_scores += 1;
        assert!(timestamp.equal(&t.utc()));
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(total_scores, 9);

    // Scan scores until the context is cancelled. A timer thread cancels it
    // after a millisecond, as Go's test deadline does.
    let mut scan_scores = 0;
    let (ctx, cancel) = Context::background().with_cancel();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(StdDuration::from_millis(1));
        cancel.cancel();
    });
    let result = db
        .scan_scores(&ctx, &mut |_: &str, _: &str, _: &str, _: Time| {
            std::thread::sleep(StdDuration::from_millis(1));
            scan_scores += 1;
            Ok(())
        })
        .await;
    timer.join().unwrap();
    if let Err(err) = result {
        assert!(
            matches!(err, Error::Context(ContextError::Canceled)),
            "{err}"
        );
    }
    assert!(scan_scores < 9);
    teardown(db).await;
}

/// Go's `TestTimeSeries`.
async fn test_time_series(db: &dyn Database) {
    setup(db).await;
    let ts = ts();
    let ctx = Context::background();
    let secs = |n: i64| ts.add(Duration(n * 1_000_000_000));
    let point = |name: &str, value: f64, t: Time| TimeSeriesPoint {
        name: name.to_owned(),
        value,
        timestamp: t,
    };
    db.add_time_series_points(
        &ctx,
        &[
            point("a", 1.0, secs(1)),
            point("a", 2.0, secs(2)),
            point("a", 3.0, secs(3)),
            point("a", 4.0, secs(4)),
            point("a", 5.0, secs(5)),
            point("b", 3.0, secs(3)),
        ],
    )
    .await
    .unwrap();

    let points = db
        .get_time_series_points(&ctx, "a", secs(2), secs(4), Duration(1_000_000_000))
        .await
        .unwrap();
    assert_points(
        &points,
        &[
            point("a", 2.0, secs(2)),
            point("a", 3.0, secs(3)),
            point("a", 4.0, secs(4)),
        ],
    );

    let points = db
        .get_time_series_points(&ctx, "a", secs(2), secs(4), Duration(2_000_000_000))
        .await
        .unwrap();
    assert_points(
        &points,
        &[point("a", 3.0, secs(2)), point("a", 4.0, secs(4))],
    );

    let points = db
        .get_time_series_points(&ctx, "missing", secs(2), secs(4), Duration(1_000_000_000))
        .await
        .unwrap();
    assert!(points.is_empty());
    teardown(db).await;
}

/// Compares two point lists in order, timestamps as instants.
fn assert_points(got: &[TimeSeriesPoint], want: &[TimeSeriesPoint]) {
    let key = |p: &TimeSeriesPoint| (p.name.clone(), p.value, p.timestamp.unix_nano());
    let got: Vec<_> = got.iter().map(key).collect();
    let want: Vec<_> = want.iter().map(key).collect();
    assert_eq!(got, want);
}

/// Go's `TestTimestampPrecision`: a timestamp with sub-microsecond digits
/// still matches its own `Before` condition's delete-or-keep rule.
async fn test_timestamp_precision(db: &dyn Database) {
    setup(db).await;
    let ctx = Context::background();
    let timestamp = ts().add(Duration(500));
    // Add scores.
    db.add_scores(&ctx, "a", "s", &[doc("1", 1.0, &[""], timestamp.clone())])
        .await
        .unwrap();
    // Remove by timestamp.
    db.delete_scores(
        &ctx,
        &strs(&["a"]),
        &ScoreCondition {
            subset: some("s"),
            before: Some(timestamp),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Search scores.
    let documents = db.search_scores(&ctx, "a", "s", &[], 0, -1).await.unwrap();
    assert!(!documents.is_empty());
    teardown(db).await;
}

#[test]
fn test_key() {
    assert_eq!(key(&[]), "");
    assert_eq!(key(&["a"]), "a");
    assert_eq!(key(&["a", ""]), "a");
    assert_eq!(key(&["a", "b"]), "a/b");
}
