// Ports: storage/cache/sql.go, storage/cache/sql_test.go @ add21b5

//! The SQL cache store: MySQL (and MariaDB) and PostgreSQL through `sqlx`, and
//! SQLite through `rusqlite` (`porting/crate-selection.md`, D-18).
//!
//! Go's `SQLDatabase` runs its queries through GORM. Here each query is the SQL
//! GORM generates, written out, so the tables and the rows are the ones Go
//! writes. The schemas are the frozen dumps in `contracts/schema/cache/`.

#[cfg(test)]
mod tests {
    use gorse_common::gocontext::Context;
    use gorse_common::gotime::{self, Month, Time};

    use crate::cache::database_test;
    use crate::cache::{Database, Score, ScorePatch, ScoreCondition, TimeSeriesPoint, string};

    use super::*;

    /// The database the suites create and drop, as Go's `gorse_cache_test`.
    const TEST_DATABASE: &str = "gorse_cache_test";

    /// A score document with the given categories.
    fn score(id: &str, score: f64, categories: &[&str], timestamp: Time) -> Score {
        Score {
            id: id.to_owned(),
            score,
            is_hidden: false,
            categories: categories.iter().map(|c| (*c).to_owned()).collect(),
            timestamp,
        }
    }

    /// 2023-01-01 00:00:00.0000005 UTC: the sub-microsecond digit Go keeps in
    /// SQLite's text, as the probe of `cache.SQLDatabase` recorded it.
    fn probe_time() -> Time {
        gotime::date(2023, Month::JANUARY, 1, 0, 0, 0, 500, &gotime::utc())
    }

    /// Every row of `sql` on a SQLite store, each column as text (`None` for
    /// NULL).
    fn sqlite_rows(db: &SqlDatabase, sql: &str) -> Vec<Vec<Option<String>>> {
        let Client::Sqlite(conn) = &db.client else {
            panic!("the store is not SQLite");
        };
        let guard = conn.lock().unwrap();
        let conn = guard.as_ref().expect("the store is closed");
        let mut stmt = conn.prepare(sql).unwrap();
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                Ok((0..columns)
                    .map(|i| match row.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Ok(None),
                        rusqlite::types::ValueRef::Integer(n) => Ok(Some(n.to_string())),
                        rusqlite::types::ValueRef::Real(f) => Ok(Some(f.to_string())),
                        rusqlite::types::ValueRef::Text(t) => {
                            Ok(Some(String::from_utf8_lossy(t).into_owned()))
                        }
                        rusqlite::types::ValueRef::Blob(_) => panic!("unexpected blob"),
                    })
                    .collect::<Result<Vec<_>, rusqlite::Error>>()?)
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    /// The backend tests run only when the environment names a server, as Go's
    /// `t.Skip` does when `MYSQL_URI` or `POSTGRES_URI` is unset.
    fn dsn_from_env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|dsn| !dsn.is_empty())
    }

    #[tokio::test]
    async fn sqlite_suite() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        database_test::run(&db).await;
        db.close().await.unwrap();
    }

    /// The text GORM and the SQLite driver write, as the Go probe recorded:
    /// categories as JSON with `<` escaped, a nil list as NULL, and a
    /// timestamp as `time.Time.String()`.
    #[tokio::test]
    async fn sqlite_writes_the_text_go_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        let ctx = Context::background();
        let ts = probe_time();
        db.add_scores(&ctx, "a", "s", &[]).await.unwrap();
        db.add_scores(
            &ctx,
            "a",
            "s",
            &[
                score("nil", 1.0, &[], ts.clone()),
                score("cat", 3.0, &["x<y", "é"], ts),
            ],
        )
        .await
        .unwrap();
        let rows = sqlite_rows(
            &db,
            "SELECT id, categories, CAST(timestamp AS TEXT) FROM gorse_documents ORDER BY id",
        );
        assert_eq!(
            rows,
            vec![
                vec![
                    Some("cat".to_owned()),
                    Some(r#"["x<y","é"]"#.to_owned()),
                    Some("2023-01-01 00:00:00.0000005 +0000 UTC".to_owned()),
                ],
                vec![
                    Some("nil".to_owned()),
                    None,
                    Some("2023-01-01 00:00:00.0000005 +0000 UTC".to_owned()),
                ],
            ]
        );
        db.close().await.unwrap();
    }

    /// The `_pragma` parameters Go appends to the path.
    #[tokio::test]
    async fn sqlite_runs_go_connection_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        assert_eq!(
            sqlite_rows(&db, "PRAGMA busy_timeout"),
            vec![vec![Some("10000".to_owned())]]
        );
        assert_eq!(
            sqlite_rows(&db, "PRAGMA journal_mode"),
            vec![vec![Some("wal".to_owned())]]
        );
        db.close().await.unwrap();
    }

    /// `json_contains` is registered on every connection (Q-25), so category
    /// searches work on SQLite.
    #[tokio::test]
    async fn sqlite_searches_categories_with_json_contains() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        let ctx = Context::background();
        db.add_scores(
            &ctx,
            "a",
            "s",
            &[score("1", 1.0, &["a", "b"], probe_time())],
        )
        .await
        .unwrap();
        let found = db
            .search_scores(&ctx, "a", "s", &["a".to_owned()], 0, -1)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        let missing = db
            .search_scores(&ctx, "a", "s", &["c".to_owned()], 0, -1)
            .await
            .unwrap();
        assert!(missing.is_empty());
        db.close().await.unwrap();
    }

    /// Go's `Purge` deletes the values and the documents but not the time series
    /// points. The port keeps that quirk; see `porting/notes/WP-2-6.md`.
    #[tokio::test]
    async fn sqlite_purge_keeps_time_series_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        let ctx = Context::background();
        db.set(&ctx, &[string("key", "value")]).await.unwrap();
        db.add_time_series_points(
            &ctx,
            &[TimeSeriesPoint {
                name: "p".to_owned(),
                timestamp: probe_time(),
                value: 1.5,
            }],
        )
        .await
        .unwrap();
        db.purge().await.unwrap();
        assert!(!db.get(&ctx, "key").await.exists());
        assert_eq!(
            sqlite_rows(&db, "SELECT COUNT(*) FROM gorse_time_series_points"),
            vec![vec![Some("1".to_owned())]]
        );
        db.close().await.unwrap();
    }

    /// An empty patch of categories is a non-nil empty list, which Go writes
    /// as `[]`. A nil list would leave the field as it is, so it is `None`.
    #[tokio::test]
    async fn sqlite_writes_an_empty_patch_as_an_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let db = SqlDatabase::sqlite(&path, "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        let ctx = Context::background();
        db.add_scores(&ctx, "a", "s", &[score("1", 1.0, &["x"], probe_time())])
            .await
            .unwrap();
        db.update_scores(
            &ctx,
            &["a".to_owned()],
            Some("s"),
            "1",
            &ScorePatch {
                categories: Some(Vec::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            sqlite_rows(&db, "SELECT categories FROM gorse_documents"),
            vec![vec![Some("[]".to_owned())]]
        );
        // A delete with a condition that matches nothing is still valid.
        db.delete_scores(
            &ctx,
            &["a".to_owned()],
            &ScoreCondition {
                id: Some("missing".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        db.close().await.unwrap();
    }

    /// Go's `json.Marshal` escapes `<`, `>`, `&`, U+2028 and U+2029, which
    /// `serde_json` does not. The escapes are what the stores write.
    #[test]
    fn json_string_array_escapes_as_go_does() {
        assert_eq!(
            json_string_array(&["x<y&z>".to_owned(), "é\u{2028}".to_owned()]),
            r#"["x<y&z>","é "]"#
        );
        assert_eq!(json_string_array(&[]), "[]");
    }

    /// The MySQL suite. Go's `MySQLTestSuite.TestInit` reads back the isolation
    /// level the connection was opened with.
    #[tokio::test]
    async fn mysql_suite() {
        let Some(dsn) = dsn_from_env("MYSQL_URI") else {
            eprintln!("skipped: MYSQL_URI is not set");
            return;
        };
        let body = dsn.strip_prefix(MYSQL_PREFIX).expect("MYSQL_URI has the mysql:// prefix");
        let mut admin = mysql_admin_connection(body).await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {TEST_DATABASE}")))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {TEST_DATABASE}")))
            .execute(&mut admin)
            .await
            .unwrap();
        drop(admin);
        let db = SqlDatabase::mysql(&format!("{dsn}{TEST_DATABASE}"), "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        database_test::run(&db).await;
        assert_eq!(
            mysql_isolation_level(&db).await,
            "READ-UNCOMMITTED",
            "the session isolation level is the one Go sets on connect"
        );
        db.close().await.unwrap();
    }

    /// The PostgreSQL suite, as Go's `PostgresTestSuite`.
    #[tokio::test]
    async fn postgres_suite() {
        let Some(dsn) = dsn_from_env("POSTGRES_URI") else {
            eprintln!("skipped: POSTGRES_URI is not set");
            return;
        };
        let mut admin = postgres_admin_connection(&dsn).await;
        let lower = TEST_DATABASE.to_lowercase();
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {TEST_DATABASE}")))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {TEST_DATABASE}")))
            .execute(&mut admin)
            .await
            .unwrap();
        drop(admin);
        let db = SqlDatabase::postgres(&format!("{dsn}{lower}?sslmode=disable"), "gorse_", &[])
            .unwrap();
        db.init().await.unwrap();
        database_test::run(&db).await;
        db.close().await.unwrap();
    }
}
