// Ports: storage/data/sql_test.go @ add21b5

//! The shared data suite against each SQL dialect: MySQL and PostgreSQL on the
//! services `MYSQL_URI` and `POSTGRES_URI` name, and SQLite on a file in a
//! temporary directory. Without a URI a test skips, as Go's `TestMySQL` and
//! `TestPostgres` do.

use std::str::FromStr;

use async_trait::async_trait;
use gorse_common::mysql_dsn::parse_dsn;
use sqlx::{AssertSqlSafe, Connection as _};

use super::super::Database;
use super::super::Error;
use super::super::database_test::{Backend, TimeStamps, run};
use super::SqlDatabase;
use super::open::{
    mysql_connect_options, open_mysql, open_postgres, open_sqlite, probe_isolation_variable_name,
};
use super::query::Driver;

/// The database each suite creates and drops, as Go's `gorse_data_test`.
const TEST_DATABASE: &str = "gorse_data_test";

#[async_trait]
impl Backend for SqlDatabase {
    /// Go's `analyzeTables`: PostgreSQL refreshes the planner statistics of
    /// the three tables, so the counts see the rows a case inserted.
    async fn analyze_tables(&self) -> Result<(), Error> {
        if self.driver() != Driver::Postgres {
            return Ok(());
        }
        for table in [
            self.items_table(),
            self.users_table(),
            self.feedback_table(),
        ] {
            let mut sql = self.sql();
            sql.push(&format!("ANALYZE {table}"));
            self.exec(&sql).await?;
        }
        Ok(())
    }

    /// PostgreSQL keeps microseconds and SQLite keeps the stamps exactly. Go
    /// skips the timezone case for MySQL, which is `Unknown` here.
    fn time_stamps(&self) -> TimeStamps {
        match self.driver() {
            Driver::Postgres => TimeStamps::Microseconds,
            Driver::Sqlite => TimeStamps::Exact,
            Driver::MySql => TimeStamps::Unknown,
        }
    }
}

#[tokio::test]
async fn test_mysql() {
    let Ok(uri) = std::env::var("MYSQL_URI") else {
        panic!("MYSQL_URI is not set");
    };
    // Go's SetupSuite drops and creates the database with a connection of its
    // own, to the server with no database named.
    let admin = parse_dsn(&uri["mysql://".len()..]).expect("MYSQL_URI is a DSN");
    let mut conn = sqlx::MySqlConnection::connect_with(&mysql_connect_options(&admin))
        .await
        .expect("connect to MySQL");
    sqlx::query(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {TEST_DATABASE}"
    )))
    .execute(&mut conn)
    .await
    .expect("drop the test database");
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {TEST_DATABASE}")))
        .execute(&mut conn)
        .await
        .expect("create the test database");
    conn.close().await.expect("close the admin connection");

    let db = open_mysql(&format!("{uri}{TEST_DATABASE}"), "gorse_", &[])
        .await
        .expect("open MySQL");
    db.init().await.expect("create the schema");
    // Go's TestInit: the session has the isolation level and sql_mode that
    // the store sets on connect.
    let name = probe_isolation_variable_name(&admin).await.expect("probe");
    let mut sql = db.sql();
    sql.push(&format!("SELECT @@{name}"));
    assert_eq!(
        db.one_text(&sql)
            .await
            .expect("read the isolation level")
            .as_deref(),
        Some("READ-UNCOMMITTED")
    );
    let mut sql = db.sql();
    sql.push("SELECT @@sql_mode");
    assert_eq!(
        db.one_text(&sql).await.expect("read sql_mode").as_deref(),
        Some(
            "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION"
        )
    );
    run(&db).await;
    db.close().await.expect("close");
}

#[tokio::test]
async fn test_postgres() {
    let Ok(uri) = std::env::var("POSTGRES_URI") else {
        panic!("POSTGRES_URI is not set");
    };
    let admin = sqlx::postgres::PgConnectOptions::from_str(&format!("{uri}?sslmode=disable"))
        .expect("POSTGRES_URI is a URL");
    let mut conn = sqlx::PgConnection::connect_with(&admin)
        .await
        .expect("connect to PostgreSQL");
    sqlx::query(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {TEST_DATABASE}"
    )))
    .execute(&mut conn)
    .await
    .expect("drop the test database");
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {TEST_DATABASE}")))
        .execute(&mut conn)
        .await
        .expect("create the test database");
    conn.close().await.expect("close the admin connection");

    let db = open_postgres(
        &format!("{uri}{TEST_DATABASE}?sslmode=disable"),
        "gorse_",
        &[],
    )
    .await
    .expect("open PostgreSQL");
    db.init().await.expect("create the schema");
    run(&db).await;
    db.close().await.expect("close");
}

#[tokio::test]
async fn test_sqlite() {
    let dir = tempfile::tempdir().expect("create a temporary directory");
    let path = format!("sqlite://{}/sqlite.db", dir.path().display());
    let db = open_sqlite(&path, "gorse_", &[])
        .await
        .expect("open SQLite");
    db.init().await.expect("create the schema");
    run(&db).await;
    db.close().await.expect("close");
}
