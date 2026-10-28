// Ports: contracts/tools/schemadump/main.go (initData, initCache, initMeta, insertDataSamples, insertCacheSamples, searchConfig, the database set-up in main) and contracts/tools/dump-schemas.sh @ WP-2-13.

//! Initialises each backend with the Rust stores, the way the Go tool does.
//!
//! For a data or cache store the steps are the Go tool's: open, `Init`, then
//! either the samples or `Reconcile` with every search column kind, then close.
//! The databases are created with the names below, which differ from the Go
//! tool's, so that the two builds never share a database.

use std::path::Path;

use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Month, SECOND, Time};
use gorse_config::config::SearchConfig;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels, User};
use indexmap::IndexMap;
use mongodb::Client;
use redis::aio::ConnectionManager;
use sqlx::{AssertSqlSafe, Connection as _, MySqlConnection, PgConnection};

use super::{Result, fail, mysql_options};

/// The data database the Go tool initialises on MySQL, MariaDB and MongoDB, and
/// the Postgres database it uses without search.
pub const GO_DATA_DB: &str = "gorse_schema_data";
/// The Postgres database the Go tool reconciles with search columns.
pub const GO_DATA_SEARCH_DB: &str = "gorse_schema_data_search";
/// The cache database the Go tool initialises.
pub const GO_CACHE_DB: &str = "gorse_schema_cache";
/// The data database the Rust stores initialise on MySQL, MariaDB and MongoDB.
pub const RUST_DATA_DB: &str = "gorse_rs_schema_data";
/// The Postgres database the Rust stores reconcile with search columns.
pub const RUST_DATA_SEARCH_DB: &str = "gorse_rs_schema_data_search";
/// The cache database the Rust stores initialise.
pub const RUST_CACHE_DB: &str = "gorse_rs_schema_cache";

/// The instant the samples are written at: Go's `sampleTime`, 2026-01-02
/// 03:04:05.678901 UTC.
pub fn sample_time() -> Time {
    gotime::date(
        2026,
        Month::JANUARY,
        2,
        3,
        4,
        5,
        678_901_000,
        &gotime::utc(),
    )
}

/// The search columns of schemadump's `searchConfig`: every column kind that
/// `Reconcile` accepts.
fn search_config() -> SearchConfig {
    SearchConfig {
        columns: Some(
            [
                "item.ItemId",
                "item.Categories",
                "item.Comment",
                "item.Labels",
                "item.Labels.title",
            ]
            .map(str::to_owned)
            .to_vec(),
        ),
    }
}

/// A data store registry with every built-in data store, as a node builds it.
fn data_registry() -> data::Registry {
    let mut registry = data::Registry::default();
    data::register_builtin(&mut registry);
    registry
}

/// A cache store registry with every built-in cache store, as a node builds it.
fn cache_registry() -> cache::Registry {
    let mut registry = cache::Registry::default();
    cache::register_builtin(&mut registry);
    registry
}

/// schemadump's `initData`: opens the data store at `url`, runs `Init`, and then
/// either writes the samples (`search` false) or runs `Reconcile` (`search` true).
pub async fn data(url: &str, search: bool) -> Result<()> {
    let db = data_registry().open(url, "", &[]).await.map_err(fail)?;
    db.init().await.map_err(fail)?;
    if search {
        db.reconcile(&search_config()).await.map_err(fail)?;
    } else {
        insert_data_samples(db.as_ref()).await?;
    }
    db.close().await.map_err(fail)
}

/// schemadump's `insertDataSamples`: one user, one item and one feedback.
async fn insert_data_samples(db: &dyn data::Database) -> Result<()> {
    let ctx = Context::background();
    let user_labels = IndexMap::from([
        ("gender".to_owned(), Labels::String("F".to_owned())),
        (
            "tags".to_owned(),
            Labels::Array(vec![
                Labels::String("a".to_owned()),
                Labels::String("b".to_owned()),
            ]),
        ),
    ]);
    db.batch_insert_users(
        &ctx,
        &[User {
            user_id: "u1".to_owned(),
            labels: Labels::Object(user_labels),
            comment: "user comment".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .map_err(fail)?;

    let item_labels = IndexMap::from([
        ("title".to_owned(), Labels::String("Hello".to_owned())),
        (
            "embedding".to_owned(),
            Labels::Array(vec![Labels::Float64(0.25), Labels::Float64(0.5)]),
        ),
    ]);
    db.batch_insert_items(
        &ctx,
        &[Item {
            item_id: "i1".to_owned(),
            is_hidden: true,
            categories: Some(vec!["c1".to_owned(), "c2".to_owned()]),
            timestamp: sample_time(),
            labels: Labels::Object(item_labels),
            comment: "item comment".to_owned(),
            ..Default::default()
        }],
    )
    .await
    .map_err(fail)?;

    db.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "like".to_owned(),
                user_id: "u1".to_owned(),
                item_id: "i1".to_owned(),
            },
            value: 1.5,
            timestamp: sample_time().add(SECOND),
            comment: "feedback comment".to_owned(),
            ..Default::default()
        }],
        true,
        true,
        true,
    )
    .await
    .map_err(fail)
}

/// schemadump's `initCache`: opens the cache store at `url`, runs `Init` and
/// writes one value of each kind.
pub async fn cache(url: &str) -> Result<()> {
    let db = cache_registry().open(url, "", &[]).await.map_err(fail)?;
    db.init().await.map_err(fail)?;
    insert_cache_samples(db.as_ref()).await?;
    db.close().await.map_err(fail)
}

/// schemadump's `insertCacheSamples`: an integer, a time and a string, one score
/// document and one time-series point.
async fn insert_cache_samples(db: &dyn cache::Database) -> Result<()> {
    let ctx = Context::background();
    db.set(
        &ctx,
        &[
            cache::integer(cache::key(&[cache::GLOBAL_META, cache::NUM_USERS]), 10),
            cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, "u1"]),
                sample_time(),
            ),
            cache::string(
                cache::key(&[cache::RECOMMEND_DIGEST, "u1"]),
                "0123456789abcdef0123456789abcdef",
            ),
        ],
    )
    .await
    .map_err(fail)?;
    db.add_scores(
        &ctx,
        cache::RECOMMEND,
        "u1",
        &[cache::Score {
            id: "i1".to_owned(),
            score: 0.5,
            is_hidden: false,
            categories: vec![String::new(), "c1".to_owned()],
            timestamp: sample_time(),
        }],
    )
    .await
    .map_err(fail)?;
    db.add_time_series_points(
        &ctx,
        &[cache::TimeSeriesPoint {
            name: cache::NUM_USERS.to_owned(),
            timestamp: sample_time(),
            value: 10.0,
        }],
    )
    .await
    .map_err(fail)
}

/// schemadump's `initMeta`: opens the SQLite meta store at `path` with a zero
/// TTL, runs `Init` and closes it.
pub async fn meta(path: &Path) -> Result<()> {
    let db = gorse_storage::meta::open(
        &format!("sqlite://{}", path.display()),
        gotime::Duration::default(),
    )
    .await
    .map_err(fail)?;
    db.init().await.map_err(fail)?;
    db.close().await.map_err(fail)
}

/// Drops and creates each named MySQL database. `dsn` is the server without a
/// database, as in `root:password@tcp(127.0.0.1:3306)/`.
pub async fn recreate_mysql(dsn: &str, databases: &[&str]) -> Result<()> {
    let mut conn = MySqlConnection::connect_with(&mysql_options(dsn)?)
        .await
        .map_err(fail)?;
    for database in databases {
        sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {database}")))
            .execute(&mut conn)
            .await
            .map_err(fail)?;
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .execute(&mut conn)
            .await
            .map_err(fail)?;
    }
    conn.close().await.map_err(fail)
}

/// Drops and creates each named PostgreSQL database, through the `postgres`
/// database of the server at `url`, as schemadump does.
pub async fn recreate_postgres(url: &str, databases: &[&str]) -> Result<()> {
    let mut conn = PgConnection::connect(&format!("{url}postgres?sslmode=disable"))
        .await
        .map_err(fail)?;
    for database in databases {
        sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {database}")))
            .execute(&mut conn)
            .await
            .map_err(fail)?;
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .execute(&mut conn)
            .await
            .map_err(fail)?;
    }
    conn.close().await.map_err(fail)
}

/// Drops each named MongoDB database. `uri` is the server without a database.
pub async fn recreate_mongodb(uri: &str, databases: &[&str]) -> Result<()> {
    let client = Client::with_uri_str(uri).await.map_err(fail)?;
    for database in databases {
        client.database(database).drop().await.map_err(fail)?;
    }
    Ok(())
}

/// Removes every key from the Redis server, as schemadump's `FlushAll` does.
pub async fn flush_redis(url: &str) -> Result<()> {
    let client = redis::Client::open(url).map_err(fail)?;
    let mut connection = ConnectionManager::new(client).await.map_err(fail)?;
    let _: redis::Value = redis::cmd("FLUSHALL")
        .query_async(&mut connection)
        .await
        .map_err(fail)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_sample_time_is_go_sample_time() {
        // 2026-01-02 03:04:05.678901 UTC in Unix microseconds.
        assert_eq!(sample_time().unix_micro(), 1_767_323_045_678_901);
    }

    #[test]
    fn schema_search_config_names_every_column_kind() {
        assert_eq!(search_config().columns.map(|c| c.len()), Some(5));
    }
}
