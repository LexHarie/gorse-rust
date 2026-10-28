// Ports: storage/data/sql.go (the creators `init` registers, and the pool and connection setup they share) @ add21b5

//! The creators that open a SQL store from its path, one per dialect.
//!
//! Go hands each path to `otelsql.Open`, which opens a `database/sql` pool.
//! Here each path opens an sqlx pool. The MySQL DSN is parsed as Go parses it,
//! and every parameter the driver applies on connect is run as a `SET` on each
//! new connection. The deviations are logged in `PORTING_NOTES.md`.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use futures_util::future::BoxFuture;
use gorse_common::gotime::Duration;
use gorse_common::mysql_dsn::{Config, DsnError, parse_dsn};
use gorse_storage_core::options::{Opt, Options, SqlPool, apply_sql_pool, new_options};
use gorse_storage_core::scheme::{
    MYSQL_ISOLATION_VARIABLE_QUERY, MYSQL_PREFIX, SQLITE_PREFIX, TablePrefix, append_mysql_params,
    append_url_params,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlConnection, MySqlPoolOptions};
use sqlx::pool::PoolOptions;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, Connection as _, Row as _};

use super::super::{Database, Error};
use super::query::backend;
use super::sqlite;
use super::{Pools, SqlDatabase};

/// The `sql_mode` Go's MySQL store runs with (ported from `storage/data/sql.go`).
const SQL_MODE: &str =
    "'ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'";

/// The creator for `mysql://` paths, in the form the data registry takes.
pub fn mysql_creator<'a>(
    path: &'a str,
    table_prefix: &'a str,
    opts: &'a [Opt],
) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
    Box::pin(async move {
        let db = open_mysql(path, table_prefix, opts).await?;
        Ok(Box::new(db) as Box<dyn Database>)
    })
}

/// The creator for `postgres://` and `postgresql://` paths.
pub fn postgres_creator<'a>(
    path: &'a str,
    table_prefix: &'a str,
    opts: &'a [Opt],
) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
    Box::pin(async move {
        let db = open_postgres(path, table_prefix, opts).await?;
        Ok(Box::new(db) as Box<dyn Database>)
    })
}

/// The creator for `sqlite://` paths.
pub fn sqlite_creator<'a>(
    path: &'a str,
    table_prefix: &'a str,
    opts: &'a [Opt],
) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
    Box::pin(async move {
        let db = open_sqlite(path, table_prefix, opts).await?;
        Ok(Box::new(db) as Box<dyn Database>)
    })
}

/// Go's MySQL creator: probes the isolation variable, adds the session
/// parameters to the DSN, and opens the pool.
pub(super) async fn open_mysql(
    path: &str,
    table_prefix: &str,
    opts: &[Opt],
) -> Result<SqlDatabase, Error> {
    let options = new_options(opts);
    let dsn = &path[MYSQL_PREFIX.len()..];
    // Go asks the server for the isolation variable's name on a connection
    // made before the session parameters are added.
    let probe = parse_dsn(dsn).map_err(dsn_error)?;
    let isolation_variable = probe_isolation_variable_name(&probe).await?;
    let params = vec![
        ("sql_mode".to_owned(), SQL_MODE.to_owned()),
        (isolation_variable, format!("'{}'", options.isolation_level)),
        ("parseTime".to_owned(), "true".to_owned()),
    ];
    let dsn = append_mysql_params(dsn, params).map_err(dsn_error)?;
    let cfg = parse_dsn(&dsn).map_err(dsn_error)?;
    let session = Arc::new(session_statements(&cfg));
    let pool = with_settings(MySqlPoolOptions::new(), &pool_settings(&options))
        .after_connect(move |conn, _| {
            let session = Arc::clone(&session);
            Box::pin(async move {
                for statement in session.iter() {
                    // The statements are `SET` lines built from the DSN, which
                    // the operator writes; no request data reaches them.
                    sqlx::query(AssertSqlSafe(statement.as_str()))
                        .execute(&mut *conn)
                        .await?;
                }
                Ok(())
            })
        })
        .connect_with(mysql_connect_options(&cfg))
        .await
        .map_err(backend)?;
    Ok(SqlDatabase::new(
        Pools::MySql(pool),
        TablePrefix::from(table_prefix),
    ))
}

/// Go's PostgreSQL creator: opens the pool on the URL as it is.
pub(super) async fn open_postgres(
    path: &str,
    table_prefix: &str,
    opts: &[Opt],
) -> Result<SqlDatabase, Error> {
    let options = new_options(opts);
    let connect = PgConnectOptions::from_str(path).map_err(backend)?;
    let pool = with_settings(PgPoolOptions::new(), &pool_settings(&options))
        .connect_with(connect)
        .await
        .map_err(backend)?;
    Ok(SqlDatabase::new(
        Pools::Postgres(pool),
        TablePrefix::from(table_prefix),
    ))
}

/// Go's SQLite creator: adds the busy timeout and the WAL journal to the
/// file, and registers `json_contains` on each connection.
pub(super) async fn open_sqlite(
    path: &str,
    table_prefix: &str,
    opts: &[Opt],
) -> Result<SqlDatabase, Error> {
    let options = new_options(opts);
    let data_source = append_url_params(
        &path[SQLITE_PREFIX.len()..],
        &[
            ("_pragma", "busy_timeout(10000)"),
            ("_pragma", "journal_mode(wal)"),
        ],
    )
    .map_err(|err| Error::Backend(Arc::new(err)))?;
    // The file is the part before the query, where modernc reads the pragmas.
    let file = data_source
        .split(|byte| *byte == b'?')
        .next()
        .unwrap_or_default();
    let connect = SqliteConnectOptions::new()
        .filename(String::from_utf8_lossy(file).into_owned())
        .create_if_missing(true)
        .busy_timeout(StdDuration::from_secs(10))
        .journal_mode(SqliteJournalMode::Wal);
    let pool = with_settings(SqlitePoolOptions::new(), &pool_settings(&options))
        .after_connect(|conn, _| {
            Box::pin(async move { sqlite::register_json_contains(conn).await })
        })
        .connect_with(connect)
        .await
        .map_err(backend)?;
    Ok(SqlDatabase::new(
        Pools::Sqlite(pool),
        TablePrefix::from(table_prefix),
    ))
}

/// The options a MySQL connection opens with: the host or socket, user,
/// password and database of `cfg`.
pub(super) fn mysql_connect_options(cfg: &Config) -> MySqlConnectOptions {
    let mut options = MySqlConnectOptions::new()
        .username(&String::from_utf8_lossy(&cfg.user))
        .password(&String::from_utf8_lossy(&cfg.passwd))
        .database(&String::from_utf8_lossy(&cfg.db_name));
    let addr = String::from_utf8_lossy(&cfg.addr).into_owned();
    if cfg.net == b"unix" {
        options = options.socket(addr);
    } else {
        let (host, port) = split_host_port(&addr);
        options = options.host(host).port(port);
    }
    options
}

/// Go's `ProbeMySQLIsolationVariableName`: the name of the server's isolation
/// variable, or empty when the server has neither name. Runs on one
/// connection of its own, which it closes.
pub(super) async fn probe_isolation_variable_name(cfg: &Config) -> Result<String, Error> {
    let mut conn = MySqlConnection::connect_with(&mysql_connect_options(cfg))
        .await
        .map_err(backend)?;
    let row = sqlx::query(MYSQL_ISOLATION_VARIABLE_QUERY)
        .fetch_optional(&mut conn)
        .await
        .map_err(backend)?;
    conn.close().await.map_err(backend)?;
    match row {
        Some(row) => row.try_get::<String, _>("Variable_name").map_err(backend),
        None => Ok(String::new()),
    }
}

/// The `SET` statements that apply the DSN's session parameters, as the Go
/// driver runs them on connect (`SET key=value`).
fn session_statements(cfg: &Config) -> Vec<String> {
    cfg.params
        .iter()
        .flatten()
        .map(|(key, value)| {
            format!(
                "SET {} = {}",
                String::from_utf8_lossy(key),
                String::from_utf8_lossy(value)
            )
        })
        .collect()
}

/// Splits a `host:port` address the way Go's `tcp(...)` does, with the
/// default host and port when the address leaves them out.
fn split_host_port(addr: &str) -> (&str, u16) {
    if addr.is_empty() {
        return ("127.0.0.1", 3306);
    }
    match addr.rsplit_once(':') {
        Some((host, port)) => (
            host.trim_start_matches('[').trim_end_matches(']'),
            port.parse().unwrap_or(3306),
        ),
        None => (addr, 3306),
    }
}

/// The pool limits Go's `ApplySQLPool` sets, collected from the options.
#[derive(Default)]
struct PoolSettings {
    max_open_conns: Option<i64>,
    conn_max_lifetime: Option<Duration>,
}

impl SqlPool for PoolSettings {
    fn set_max_open_conns(&mut self, n: i64) {
        self.max_open_conns = Some(n);
    }

    fn set_max_idle_conns(&mut self, _n: i64) {
        // sqlx has no cap on idle connections, so `MaxIdleConns` is not
        // applied (X-WP22-7).
    }

    fn set_conn_max_lifetime(&mut self, d: Duration) {
        self.conn_max_lifetime = Some(d);
    }
}

/// The pool limits `options` sets, as Go's `ApplySQLPool` leaves the others at
/// the driver's default.
fn pool_settings(options: &Options) -> PoolSettings {
    let mut settings = PoolSettings::default();
    apply_sql_pool(&mut settings, options);
    settings
}

/// Applies `settings` to a pool's options.
fn with_settings<DB: sqlx::Database>(
    options: PoolOptions<DB>,
    settings: &PoolSettings,
) -> PoolOptions<DB> {
    let mut options = options;
    if let Some(n) = settings.max_open_conns {
        options = options.max_connections(u32::try_from(n).unwrap_or(u32::MAX));
    }
    if let Some(d) = settings.conn_max_lifetime {
        let lifetime = StdDuration::from_nanos(u64::try_from(d.0).unwrap_or(0));
        options = options.max_lifetime(Some(lifetime));
    }
    options
}

fn dsn_error(err: DsnError) -> Error {
    Error::Backend(Arc::new(err))
}
