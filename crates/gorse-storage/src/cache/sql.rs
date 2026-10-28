// Ports: storage/cache/sql.go, storage/cache/sql_test.go @ add21b5

//! The SQL cache store: MySQL (and MariaDB) and PostgreSQL through `sqlx`, and
//! SQLite through `rusqlite` (`porting/crate-selection.md`, D-18).
//!
//! Go's `SQLDatabase` runs its queries through GORM. Here each query is the SQL
//! GORM generates, written out, so the tables and the rows are the ones Go
//! writes. The schemas are the frozen dumps in `contracts/schema/cache/`.

use std::sync::Arc;
#[cfg(feature = "sqlite")]
use std::sync::Mutex;

use async_trait::async_trait;
#[cfg(feature = "sqlite")]
use gorse_common::dateparse;
use gorse_common::gocontext::Context;
#[cfg(any(feature = "mysql", feature = "postgres"))]
use gorse_common::gotime;
use gorse_common::gotime::{Duration, Time};
#[cfg(any(feature = "mysql", feature = "postgres"))]
use gorse_storage_core::options;
use gorse_storage_core::options::Opt;
use gorse_storage_core::scheme::{self, TablePrefix};
#[cfg(any(feature = "mysql", feature = "postgres"))]
use sqlx::AssertSqlSafe;
#[cfg(feature = "mysql")]
use sqlx::types::chrono::NaiveDateTime;
#[cfg(any(feature = "mysql", feature = "postgres"))]
use sqlx::types::chrono::{DateTime, Utc};

use super::{
    Database, Error, Registry, ReturnValue, Score, ScoreCondition, ScorePatch, TimeSeriesPoint,
    Value, new_return_error, new_return_value,
};
use crate::sqlconn;

/// How many rows one `INSERT` carries, as GORM's `CreateBatchSize` does.
const BATCH_SIZE: usize = 1000;

/// The SQL dialect a store speaks (Go's `SQLDriver`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driver {
    #[cfg_attr(not(feature = "mysql"), allow(dead_code))]
    MySql,
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    Postgres,
    #[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
    Sqlite,
}

/// The connection a store runs its queries on.
enum Client {
    #[cfg(feature = "mysql")]
    MySql(sqlx::MySqlPool),
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// SQLite is one `rusqlite` connection, shared behind a mutex and used on
    /// the blocking pool. It is `None` once the store is closed.
    #[cfg(feature = "sqlite")]
    Sqlite(Arc<Mutex<Option<rusqlite::Connection>>>),
}

/// A SQL cache store (Go's `SQLDatabase`).
pub struct SqlDatabase {
    driver: Driver,
    client: Client,
    values: String,
    documents: String,
    points: String,
}

/// One value bound to a statement. Each driver binds it in its own type; the
/// comments on the variants say which.
#[derive(Debug, Clone)]
enum Arg {
    Bool(bool),
    Int(i64),
    /// A float. Postgres binds its shortest decimal text, which the statement
    /// casts to `numeric` (`?n`), as Go's `lib/pq` does; the others bind the
    /// number.
    Float(f64),
    Text(String),
    NullText,
    /// Postgres `text[]`. The other drivers store the same list as JSON text.
    Strings(Vec<String>),
    NullStrings,
    /// A timestamp: `datetime` for MySQL, `timestamptz` for Postgres, and the
    /// text of `time.Time.String()` for SQLite, as Go writes it.
    Time(Time),
}

/// The type a result column is read as.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Text,
    Int,
    Real,
    /// A JSON list of strings (MySQL, SQLite) or a `text[]` (Postgres).
    Categories,
    Time,
}

/// One decoded column.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Strings(Vec<String>),
    Time(Time),
}

impl Cell {
    /// The text, or "" for NULL (Go's zero value).
    fn into_text(self) -> String {
        match self {
            Cell::Text(s) => s,
            Cell::Int(n) => n.to_string(),
            _ => String::new(),
        }
    }

    fn into_real(self) -> f64 {
        match self {
            Cell::Real(f) => f,
            Cell::Int(n) => n as f64,
            _ => 0.0,
        }
    }

    fn into_strings(self) -> Vec<String> {
        match self {
            Cell::Strings(v) => v,
            _ => Vec::new(),
        }
    }

    /// The time, or the zero time for NULL.
    fn into_time(self) -> Time {
        match self {
            Cell::Time(t) => t,
            _ => Time::default(),
        }
    }
}

/// Wraps a driver error as a [`Error::Backend`].
fn backend<E>(err: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::Backend(Arc::new(err))
}

/// Fails with the context's error when it is done (D-122).
fn check(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(Error::Context(err)),
        None => Ok(()),
    }
}

/// Go's `json.Marshal` of a `[]string`. `serde_json` writes the same text
/// except for `<`, `>`, `&`, U+2028 and U+2029, which Go escapes.
fn json_string_array(values: &[String]) -> String {
    let json = serde_json::to_string(values).expect("a list of strings serializes");
    json.replace('<', r"\u003c")
        .replace('>', r"\u003e")
        .replace('&', r"\u0026")
        .replace('\u{2028}', r"\u2028")
        .replace('\u{2029}', r"\u2029")
}

/// Reads a JSON list of strings, as Go's `json` serializer reads a stored
/// category list. NULL and `null` are the nil list.
#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn parse_json_strings(text: &str) -> Result<Vec<String>, Error> {
    let values: Option<Vec<String>> = serde_json::from_str(text).map_err(backend)?;
    Ok(values.unwrap_or_default())
}

/// The `time.Time` of a chrono timestamp, in UTC.
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn from_chrono(t: DateTime<Utc>) -> Time {
    gotime::unix(t.timestamp(), i64::from(t.timestamp_subsec_nanos())).utc()
}

/// The chrono timestamp of a time. Sub-microsecond digits are dropped, as the
/// drivers drop them.
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn to_chrono(t: &Time) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(t.unix(), t.nanosecond() as u32)
        .expect("a Go time is within chrono's range")
}

/// The seconds of a duration, as Go's `int(duration.Seconds())`.
fn whole_seconds(duration: Duration) -> i64 {
    duration.0 / 1_000_000_000
}

/// Parses a timestamp the SQLite text or a `datetime` expression wrote.
#[cfg(feature = "sqlite")]
fn parse_time(text: &str) -> Result<Time, Error> {
    Ok(dateparse::parse_any(text, &[])?.utc())
}

impl SqlDatabase {
    /// Opens a SQLite store (Go's `sqlite://` creator). The connection gets
    /// Go's pragmas, and `json_contains` is registered on it (Q-25).
    #[cfg(feature = "sqlite")]
    pub fn sqlite(path: &str, table_prefix: &str, _opts: &[Opt]) -> Result<Self, Error> {
        let file = path.strip_prefix(scheme::SQLITE_PREFIX).unwrap_or(path);
        let conn = rusqlite::Connection::open(file).map_err(backend)?;
        conn.pragma_update(None, "busy_timeout", 10_000)
            .map_err(backend)?;
        conn.pragma_update_and_check(None, "journal_mode", "wal", |_| Ok(()))
            .map_err(backend)?;
        sqlconn::register_json_contains(&conn).map_err(backend)?;
        Ok(Self::new(
            Driver::Sqlite,
            table_prefix,
            Client::Sqlite(Arc::new(Mutex::new(Some(conn)))),
        ))
    }

    /// Opens a MySQL store (Go's `mysql://` creator). The connection is made
    /// when the first query needs it.
    #[cfg(feature = "mysql")]
    pub fn mysql(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Self, Error> {
        let body = path.strip_prefix(scheme::MYSQL_PREFIX).unwrap_or(path);
        let (options, sets) = sqlconn::mysql_connect_options(body).map_err(backend)?;
        let settings = options::new_options(opts);
        let isolation = settings.isolation_level.clone();
        let pool = sqlconn::mysql_pool_options(&settings).after_connect(move |conn, _| {
            let sets = sets.clone();
            let isolation = isolation.clone();
            Box::pin(async move { sqlconn::mysql_after_connect(conn, &sets, &isolation).await })
        });
        Ok(Self::new(
            Driver::MySql,
            table_prefix,
            Client::MySql(pool.connect_lazy_with(options)),
        ))
    }

    /// Opens a PostgreSQL store (Go's `postgres://` and `postgresql://`
    /// creators). The connection is made when the first query needs it.
    #[cfg(feature = "postgres")]
    pub fn postgres(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Self, Error> {
        let options: sqlx::postgres::PgConnectOptions = path.parse().map_err(backend)?;
        let settings = options::new_options(opts);
        let pool = sqlconn::postgres_pool_options(&settings);
        Ok(Self::new(
            Driver::Postgres,
            table_prefix,
            Client::Postgres(pool.connect_lazy_with(options)),
        ))
    }

    fn new(driver: Driver, table_prefix: &str, client: Client) -> Self {
        let prefix = TablePrefix(table_prefix.as_bytes().to_vec());
        let name = |table: Vec<u8>| String::from_utf8(table).expect("a table name is UTF-8");
        Self {
            driver,
            client,
            values: name(prefix.values_table()),
            documents: name(prefix.document_table()),
            points: name(prefix.points_table()),
        }
    }

    /// Quotes an identifier for the store's dialect.
    fn quote(&self, ident: &str) -> String {
        match self.driver {
            Driver::Postgres => format!("\"{ident}\""),
            Driver::MySql | Driver::Sqlite => format!("`{ident}`"),
        }
    }

    /// The statement with its placeholders for the store's dialect. `?` is a
    /// placeholder; `?n` is a placeholder for a number, which Postgres casts to
    /// `numeric` from its text (see [`Arg::Float`]).
    fn render(&self, template: &str) -> String {
        if self.driver != Driver::Postgres {
            return template.replace("?n", "?");
        }
        let mut out = String::with_capacity(template.len() + 16);
        let mut index = 0;
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '?' {
                out.push(c);
                continue;
            }
            index += 1;
            if chars.peek() == Some(&'n') {
                chars.next();
                out.push_str(&format!("CAST(${index} AS NUMERIC)"));
            } else {
                out.push_str(&format!("${index}"));
            }
        }
        out
    }

    /// The quoted names of columns, comma separated.
    fn columns(&self, names: &[&str]) -> String {
        names
            .iter()
            .map(|n| self.quote(n))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The categories of a document as the store writes them. An empty list is
    /// NULL, which Go writes for a nil list, the usual case from gRPC and REST.
    fn categories_arg(&self, categories: &[String]) -> Arg {
        match (self.driver, categories.is_empty()) {
            (Driver::Postgres, true) => Arg::NullStrings,
            (Driver::Postgres, false) => Arg::Strings(categories.to_vec()),
            (_, true) => Arg::NullText,
            (_, false) => Arg::Text(json_string_array(categories)),
        }
    }

    /// The categories of a patch. A patch's list is never nil, so an empty
    /// list is an empty array.
    fn patch_categories_arg(&self, categories: &[String]) -> Arg {
        match self.driver {
            Driver::Postgres => Arg::Strings(categories.to_vec()),
            _ => Arg::Text(json_string_array(categories)),
        }
    }

    /// The timestamp of a document or point, bound for the store's dialect.
    fn time_arg(&self, t: &Time) -> Arg {
        Arg::Time(t.clone())
    }

    /// Runs a statement that returns no rows.
    async fn execute(&self, template: &str, args: &[Arg]) -> Result<(), Error> {
        let sql = self.render(template);
        match &self.client {
            #[cfg(feature = "mysql")]
            Client::MySql(pool) => {
                let mut query = sqlx::query(AssertSqlSafe(sql));
                for arg in args {
                    query = bind_mysql(query, arg);
                }
                query.execute(pool).await.map_err(backend)?;
            }
            #[cfg(feature = "postgres")]
            Client::Postgres(pool) => {
                let mut query = sqlx::query(AssertSqlSafe(sql));
                for arg in args {
                    query = bind_postgres(query, arg);
                }
                query.execute(pool).await.map_err(backend)?;
            }
            #[cfg(feature = "sqlite")]
            Client::Sqlite(_) => {
                self.sqlite_run(sql, args.to_vec(), |conn, sql, args| {
                    let values = sqlite_values(&args);
                    conn.execute(&sql, rusqlite::params_from_iter(values))
                        .map(|_| ())
                        .map_err(backend)
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Runs a query and decodes each column as `kinds` says.
    async fn query(
        &self,
        template: &str,
        args: &[Arg],
        kinds: &[Kind],
    ) -> Result<Vec<Vec<Cell>>, Error> {
        let sql = self.render(template);
        match &self.client {
            #[cfg(feature = "mysql")]
            Client::MySql(pool) => {
                let mut query = sqlx::query(AssertSqlSafe(sql));
                for arg in args {
                    query = bind_mysql(query, arg);
                }
                let rows = query.fetch_all(pool).await.map_err(backend)?;
                rows.iter()
                    .map(|row| {
                        kinds
                            .iter()
                            .enumerate()
                            .map(|(i, kind)| mysql_cell(row, i, *kind))
                            .collect()
                    })
                    .collect()
            }
            #[cfg(feature = "postgres")]
            Client::Postgres(pool) => {
                let mut query = sqlx::query(AssertSqlSafe(sql));
                for arg in args {
                    query = bind_postgres(query, arg);
                }
                let rows = query.fetch_all(pool).await.map_err(backend)?;
                rows.iter()
                    .map(|row| {
                        kinds
                            .iter()
                            .enumerate()
                            .map(|(i, kind)| postgres_cell(row, i, *kind))
                            .collect()
                    })
                    .collect()
            }
            #[cfg(feature = "sqlite")]
            Client::Sqlite(_) => {
                let kinds = kinds.to_vec();
                self.sqlite_run(sql, args.to_vec(), move |conn, sql, args| {
                    let values = sqlite_values(&args);
                    let mut stmt = conn.prepare(&sql).map_err(backend)?;
                    let mut rows = stmt
                        .query(rusqlite::params_from_iter(values))
                        .map_err(backend)?;
                    let mut out = Vec::new();
                    while let Some(row) = rows.next().map_err(backend)? {
                        let mut cells = Vec::with_capacity(kinds.len());
                        for (i, kind) in kinds.iter().enumerate() {
                            let value = row.get_ref(i).map_err(backend)?;
                            cells.push(sqlite_cell(value, *kind)?);
                        }
                        out.push(cells);
                    }
                    Ok(out)
                })
                .await
            }
        }
    }

    /// Runs `work` on the SQLite connection, on the blocking pool, since
    /// `rusqlite` is synchronous. The mutex is held only inside `work`.
    #[cfg(feature = "sqlite")]
    async fn sqlite_run<T, F>(&self, sql: String, args: Vec<Arg>, work: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection, String, Vec<Arg>) -> Result<T, Error> + Send + 'static,
    {
        let conn = match &self.client {
            Client::Sqlite(conn) => Arc::clone(conn),
            #[cfg(any(feature = "mysql", feature = "postgres"))]
            _ => unreachable!("sqlite_run is called on SQLite stores only"),
        };
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| backend(std::io::Error::other("the SQLite mutex is poisoned")))?;
            let conn = guard
                .as_mut()
                .ok_or_else(|| backend(std::io::Error::other("the store is closed")))?;
            work(conn, sql, args)
        })
        .await
        .map_err(backend)?
    }
}

#[cfg(feature = "mysql")]
fn bind_mysql<'q>(
    mut query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    arg: &Arg,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match arg {
        Arg::Bool(b) => query = query.bind(*b),
        Arg::Int(n) => query = query.bind(*n),
        Arg::Float(f) => query = query.bind(*f),
        Arg::Text(s) => query = query.bind(s.clone()),
        Arg::NullText | Arg::NullStrings => query = query.bind(None::<String>),
        Arg::Strings(v) => query = query.bind(json_string_array(v)),
        Arg::Time(t) => query = query.bind(to_chrono(t).naive_utc()),
    }
    query
}

#[cfg(feature = "postgres")]
fn bind_postgres<'q>(
    mut query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    arg: &Arg,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match arg {
        Arg::Bool(b) => query = query.bind(*b),
        Arg::Int(n) => query = query.bind(*n),
        Arg::Float(f) => query = query.bind(f.to_string()),
        Arg::Text(s) => query = query.bind(s.clone()),
        Arg::NullText => query = query.bind(None::<String>),
        Arg::Strings(v) => query = query.bind(v.clone()),
        Arg::NullStrings => query = query.bind(None::<Vec<String>>),
        Arg::Time(t) => query = query.bind(to_chrono(t)),
    }
    query
}

#[cfg(feature = "sqlite")]
fn sqlite_values(args: &[Arg]) -> Vec<rusqlite::types::Value> {
    use rusqlite::types::Value;
    args.iter()
        .map(|arg| match arg {
            Arg::Bool(b) => Value::Integer(i64::from(*b)),
            Arg::Int(n) => Value::Integer(*n),
            Arg::Float(f) => Value::Real(*f),
            Arg::Text(s) => Value::Text(s.clone()),
            Arg::NullText | Arg::NullStrings => Value::Null,
            Arg::Strings(v) => Value::Text(json_string_array(v)),
            Arg::Time(t) => Value::Text(t.string()),
        })
        .collect()
}

#[cfg(feature = "mysql")]
fn mysql_cell(row: &sqlx::mysql::MySqlRow, i: usize, kind: Kind) -> Result<Cell, Error> {
    use sqlx::Row;
    Ok(match kind {
        Kind::Text => row
            .try_get::<Option<String>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Text),
        Kind::Int => row
            .try_get::<Option<i64>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Int),
        Kind::Real => row
            .try_get::<Option<f64>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Real),
        Kind::Categories => match row.try_get::<Option<String>, _>(i).map_err(backend)? {
            Some(text) => Cell::Strings(parse_json_strings(&text)?),
            None => Cell::Strings(Vec::new()),
        },
        Kind::Time => row
            .try_get::<Option<NaiveDateTime>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, |t| Cell::Time(from_chrono(t.and_utc()))),
    })
}

#[cfg(feature = "postgres")]
fn postgres_cell(row: &sqlx::postgres::PgRow, i: usize, kind: Kind) -> Result<Cell, Error> {
    use sqlx::Row;
    Ok(match kind {
        Kind::Text => row
            .try_get::<Option<String>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Text),
        Kind::Int => row
            .try_get::<Option<i64>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Int),
        Kind::Real => row
            .try_get::<Option<f64>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, Cell::Real),
        Kind::Categories => Cell::Strings(
            row.try_get::<Option<Vec<String>>, _>(i)
                .map_err(backend)?
                .unwrap_or_default(),
        ),
        Kind::Time => row
            .try_get::<Option<DateTime<Utc>>, _>(i)
            .map_err(backend)?
            .map_or(Cell::Null, |t| Cell::Time(from_chrono(t))),
    })
}

#[cfg(feature = "sqlite")]
fn sqlite_cell(value: rusqlite::types::ValueRef<'_>, kind: Kind) -> Result<Cell, Error> {
    use rusqlite::types::ValueRef;
    Ok(match (value, kind) {
        (ValueRef::Null, _) => Cell::Null,
        (ValueRef::Integer(n), Kind::Real) => Cell::Real(n as f64),
        (ValueRef::Integer(n), _) => Cell::Int(n),
        (ValueRef::Real(f), _) => Cell::Real(f),
        (ValueRef::Text(bytes), kind) => {
            let text = std::str::from_utf8(bytes).map_err(backend)?;
            match kind {
                Kind::Text => Cell::Text(text.to_owned()),
                Kind::Int => Cell::Int(text.parse().map_err(backend)?),
                Kind::Real => Cell::Real(text.parse().map_err(backend)?),
                Kind::Categories => Cell::Strings(parse_json_strings(text)?),
                Kind::Time => Cell::Time(parse_time(text)?),
            }
        }
        (ValueRef::Blob(_), _) => {
            return Err(backend(std::io::Error::other(
                "unexpected BLOB in a cache store",
            )));
        }
    })
}

/// The `Database` methods of the SQL store. Each statement is the one GORM
/// generates for the Go method of the same name.
#[async_trait]
impl Database for SqlDatabase {
    async fn close(&self) -> Result<(), Error> {
        match &self.client {
            #[cfg(feature = "mysql")]
            Client::MySql(pool) => pool.close().await,
            #[cfg(feature = "postgres")]
            Client::Postgres(pool) => pool.close().await,
            #[cfg(feature = "sqlite")]
            Client::Sqlite(conn) => {
                let taken = conn
                    .lock()
                    .map_err(|_| backend(std::io::Error::other("the SQLite mutex is poisoned")))?
                    .take();
                drop(taken);
            }
        }
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        match self.driver {
            // `rusqlite::execute` refuses a statement that returns rows.
            Driver::Sqlite => self.query("SELECT 1", &[], &[Kind::Int]).await.map(|_| ()),
            Driver::MySql | Driver::Postgres => self.execute("SELECT 1", &[]).await,
        }
    }

    async fn init(&self) -> Result<(), Error> {
        let (values, documents, points) = (
            self.quote(&self.values),
            self.quote(&self.documents),
            self.quote(&self.points),
        );
        match self.driver {
            Driver::MySql => {
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {values} (\
                         `name` varchar(256) NOT NULL, \
                         `value` varchar(256) NOT NULL, \
                         PRIMARY KEY (`name`))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {points} (\
                         `name` varchar(191) NOT NULL, \
                         `timestamp` datetime(3) NOT NULL, \
                         `value` double DEFAULT NULL, \
                         PRIMARY KEY (`name`,`timestamp`))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {documents} (\
                         `collection` varchar(191) NOT NULL, \
                         `subset` varchar(191) NOT NULL, \
                         `id` varchar(191) NOT NULL, \
                         `is_hidden` tinyint(1) DEFAULT NULL, \
                         `categories` text, \
                         `score` double DEFAULT NULL, \
                         `timestamp` datetime(3) DEFAULT NULL, \
                         PRIMARY KEY (`collection`,`subset`,`id`))"
                    ),
                    &[],
                )
                .await?;
                // MariaDB cannot index `CAST(... AS CHAR ARRAY)`, so Go skips
                // that index there. A duplicate index is not an error.
                let is_mariadb = self
                    .query("SELECT VERSION()", &[], &[Kind::Text])
                    .await?
                    .first()
                    .and_then(|row| row.first().cloned())
                    .is_some_and(
                        |version| matches!(version, Cell::Text(v) if v.contains("MariaDB")),
                    );
                if !is_mariadb {
                    self.add_index_ignoring_duplicates(&format!(
                        "ALTER TABLE {documents} ADD INDEX idx_collection_subset_categories (collection, subset, (CAST(categories AS CHAR(255) ARRAY)))"
                    ))
                    .await?;
                }
                self.add_index_ignoring_duplicates(&format!(
                    "ALTER TABLE {documents} ADD INDEX idx_collection_id (collection, id)"
                ))
                .await?;
            }
            Driver::Postgres => {
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {values} (\
                         name character varying(256) NOT NULL, \
                         value character varying(256) NOT NULL, \
                         PRIMARY KEY (name))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {points} (\
                         name text NOT NULL, \
                         \"timestamp\" timestamp with time zone NOT NULL, \
                         value numeric, \
                         PRIMARY KEY (name, \"timestamp\"))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {documents} (\
                         collection text NOT NULL, \
                         subset text NOT NULL, \
                         id text NOT NULL, \
                         is_hidden boolean, \
                         categories text[], \
                         score numeric, \
                         \"timestamp\" timestamp with time zone, \
                         PRIMARY KEY (collection, subset, id))"
                    ),
                    &[],
                )
                .await?;
                self.execute("CREATE EXTENSION IF NOT EXISTS btree_gin", &[])
                    .await?;
                self.execute(
                    &format!(
                        "CREATE INDEX IF NOT EXISTS idx_collection_subset_categories ON {documents} USING GIN (collection, subset, categories)"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE INDEX IF NOT EXISTS idx_collection_id ON {documents} (collection, id)"
                    ),
                    &[],
                )
                .await?;
            }
            Driver::Sqlite => {
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {values} (`name` varchar(256),`value` varchar(256) NOT NULL,PRIMARY KEY (`name`))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {points} (`name` text,`timestamp` datetime,`value` real,PRIMARY KEY (`name`,`timestamp`))"
                    ),
                    &[],
                )
                .await?;
                self.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS {documents} (`collection` text,`subset` text,`id` text,`is_hidden` numeric,`categories` text,`score` real,`timestamp` datetime,PRIMARY KEY (`collection`,`subset`,`id`))"
                    ),
                    &[],
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn scan(
        &self,
        work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error> {
        let sql = format!("SELECT name FROM {}", self.quote(&self.values));
        let rows = self.query(&sql, &[], &[Kind::Text]).await?;
        for row in rows {
            let name = row.into_iter().next().unwrap_or(Cell::Null).into_text();
            work(&name)?;
        }
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        // Go's Purge clears the values and the documents, not the time series
        // points. The port keeps that; see `porting/notes/WP-2-6.md`.
        for table in [&self.values, &self.documents] {
            let sql = format!("DELETE FROM {}", self.quote(table));
            self.execute(&sql, &[]).await?;
        }
        Ok(())
    }

    async fn set(&self, ctx: &Context, values: &[Value]) -> Result<(), Error> {
        check(ctx)?;
        if values.is_empty() {
            return Ok(());
        }
        // Go keeps the first value of each name.
        let mut seen = std::collections::HashSet::new();
        let rows: Vec<&Value> = values
            .iter()
            .filter(|v| seen.insert(v.name().to_owned()))
            .collect();
        let table = self.quote(&self.values);
        for batch in rows.chunks(BATCH_SIZE) {
            let placeholders = vec!["(?, ?)"; batch.len()].join(", ");
            let conflict = match self.driver {
                Driver::MySql => "ON DUPLICATE KEY UPDATE `value` = VALUES(`value`)".to_owned(),
                _ => "ON CONFLICT (name) DO UPDATE SET value = excluded.value".to_owned(),
            };
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES {placeholders} {conflict}",
                self.columns(&["name", "value"])
            );
            let mut args = Vec::with_capacity(batch.len() * 2);
            for v in batch {
                args.push(Arg::Text(v.name().to_owned()));
                args.push(Arg::Text(v.value().to_owned()));
            }
            self.execute(&sql, &args).await?;
        }
        Ok(())
    }

    async fn get(&self, ctx: &Context, name: &str) -> ReturnValue {
        if let Err(err) = check(ctx) {
            return new_return_error(err);
        }
        let sql = format!(
            "SELECT value FROM {} WHERE name = ?",
            self.quote(&self.values)
        );
        match self
            .query(&sql, &[Arg::Text(name.to_owned())], &[Kind::Text])
            .await
        {
            Ok(rows) => match rows.into_iter().next() {
                Some(row) => new_return_value(
                    row.into_iter().next().unwrap_or(Cell::Null).into_text(),
                    true,
                ),
                None => new_return_value("", false),
            },
            Err(err) => new_return_error(err),
        }
    }

    async fn delete(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check(ctx)?;
        let sql = format!("DELETE FROM {} WHERE name = ?", self.quote(&self.values));
        self.execute(&sql, &[Arg::Text(name.to_owned())]).await
    }

    async fn add_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        documents: &[Score],
    ) -> Result<(), Error> {
        check(ctx)?;
        if documents.is_empty() {
            return Ok(());
        }
        let columns = [
            "collection",
            "subset",
            "id",
            "is_hidden",
            "categories",
            "score",
            "timestamp",
        ];
        let (score, categories, timestamp) = (
            self.quote("score"),
            self.quote("categories"),
            self.quote("timestamp"),
        );
        let conflict = match self.driver {
            Driver::MySql => format!(
                "ON DUPLICATE KEY UPDATE {score} = VALUES({score}), {categories} = VALUES({categories}), {timestamp} = VALUES({timestamp})"
            ),
            _ => format!(
                "ON CONFLICT ({}, {}, {}) DO UPDATE SET {score} = excluded.{score}, {categories} = excluded.{categories}, {timestamp} = excluded.{timestamp}",
                self.quote("collection"),
                self.quote("subset"),
                self.quote("id")
            ),
        };
        for batch in documents.chunks(BATCH_SIZE) {
            let placeholders = vec!["(?, ?, ?, ?, ?, ?n, ?)"; batch.len()].join(", ");
            let sql = format!(
                "INSERT INTO {} ({}) VALUES {placeholders} {conflict}",
                self.quote(&self.documents),
                self.columns(&columns)
            );
            let mut args = Vec::with_capacity(batch.len() * columns.len());
            for document in batch {
                args.push(Arg::Text(collection.to_owned()));
                args.push(Arg::Text(subset.to_owned()));
                args.push(Arg::Text(document.id.clone()));
                args.push(Arg::Bool(document.is_hidden));
                args.push(self.categories_arg(&document.categories));
                args.push(Arg::Float(document.score));
                args.push(self.time_arg(&document.timestamp));
            }
            self.execute(&sql, &args).await?;
        }
        Ok(())
    }

    async fn search_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        query: &[String],
        begin: i64,
        end: i64,
    ) -> Result<Vec<Score>, Error> {
        check(ctx)?;
        let mut args = vec![
            Arg::Text(collection.to_owned()),
            Arg::Text(subset.to_owned()),
        ];
        let mut sql = format!(
            "SELECT {}, {}, {}, {} FROM {} WHERE collection = ? and subset = ? and is_hidden = false",
            self.quote("id"),
            match self.driver {
                Driver::Postgres => "CAST(score AS DOUBLE PRECISION)".to_owned(),
                _ => self.quote("score"),
            },
            self.quote("categories"),
            self.quote("timestamp"),
            self.quote(&self.documents),
        );
        if !query.is_empty() {
            match self.driver {
                Driver::Postgres => {
                    sql.push_str(" and categories @> ?");
                    args.push(Arg::Strings(query.to_vec()));
                }
                _ => {
                    sql.push_str(" and JSON_CONTAINS(categories,?)");
                    args.push(Arg::Text(json_string_array(query)));
                }
            }
        }
        sql.push_str(" ORDER BY score desc");
        // GORM emits LIMIT only for a non-negative limit and OFFSET only for a
        // positive offset.
        let limit = if end != -1 { end - begin } else { i64::MAX };
        if limit >= 0 {
            sql.push_str(" LIMIT ?");
            args.push(Arg::Int(limit));
        }
        if begin > 0 {
            sql.push_str(" OFFSET ?");
            args.push(Arg::Int(begin));
        }
        let rows = self
            .query(
                &sql,
                &args,
                &[Kind::Text, Kind::Real, Kind::Categories, Kind::Time],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let mut cells = row.into_iter();
                let id = cells.next().unwrap_or(Cell::Null).into_text();
                let score = cells.next().unwrap_or(Cell::Null).into_real();
                let categories = cells.next().unwrap_or(Cell::Null).into_strings();
                let timestamp = cells.next().unwrap_or(Cell::Null).into_time();
                Score {
                    id,
                    score,
                    is_hidden: false,
                    categories,
                    timestamp,
                }
            })
            .collect())
    }

    async fn update_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        subset: Option<&str>,
        id: &str,
        patch: &ScorePatch,
    ) -> Result<(), Error> {
        check(ctx)?;
        if collections.is_empty() {
            return Ok(());
        }
        if patch.score.is_none() && patch.is_hidden.is_none() && patch.categories.is_none() {
            return Ok(());
        }
        let mut sets = Vec::new();
        let mut args = Vec::new();
        if let Some(score) = patch.score {
            sets.push("score = ?n");
            args.push(Arg::Float(score));
        }
        if let Some(is_hidden) = patch.is_hidden {
            sets.push("is_hidden = ?");
            args.push(Arg::Bool(is_hidden));
        }
        if let Some(categories) = &patch.categories {
            sets.push("categories = ?");
            args.push(self.patch_categories_arg(categories));
        }
        let mut sql = format!(
            "UPDATE {} SET {} WHERE collection in ({}) and id = ?",
            self.quote(&self.documents),
            sets.join(", "),
            in_list(collections.len()),
        );
        args.extend(collections.iter().map(|c| Arg::Text(c.clone())));
        args.push(Arg::Text(id.to_owned()));
        if let Some(subset) = subset {
            sql.push_str(" and subset = ?");
            args.push(Arg::Text(subset.to_owned()));
        }
        self.execute(&sql, &args).await
    }

    async fn delete_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        condition: &ScoreCondition,
    ) -> Result<(), Error> {
        check(ctx)?;
        condition.check()?;
        let mut sql = format!(
            "DELETE FROM {} WHERE collection in ({})",
            self.quote(&self.documents),
            in_list(collections.len())
        );
        let mut args: Vec<Arg> = collections.iter().map(|c| Arg::Text(c.clone())).collect();
        if let Some(subset) = &condition.subset {
            sql.push_str(" and subset = ?");
            args.push(Arg::Text(subset.clone()));
        }
        if let Some(id) = &condition.id {
            sql.push_str(" and id = ?");
            args.push(Arg::Text(id.clone()));
        }
        if let Some(before) = &condition.before {
            sql.push_str(" and timestamp < ?");
            // MySQL rounds a DATETIME(3) to the millisecond, so Go truncates
            // the bound the same way before comparing.
            let before = match self.driver {
                Driver::MySql => before.truncate(Duration(1_000_000)),
                _ => before.clone(),
            };
            args.push(self.time_arg(&before));
        }
        self.execute(&sql, &args).await
    }

    async fn scan_scores(
        &self,
        ctx: &Context,
        callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error> {
        check(ctx)?;
        let sql = format!(
            "SELECT collection, id, subset, {} FROM {}",
            self.quote("timestamp"),
            self.quote(&self.documents)
        );
        let rows = self
            .query(&sql, &[], &[Kind::Text, Kind::Text, Kind::Text, Kind::Time])
            .await?;
        for row in rows {
            // The rows are read first; the context is checked before each
            // callback, the unit of work here (D-122).
            check(ctx)?;
            let mut cells = row.into_iter();
            let collection = cells.next().unwrap_or(Cell::Null).into_text();
            let id = cells.next().unwrap_or(Cell::Null).into_text();
            let subset = cells.next().unwrap_or(Cell::Null).into_text();
            let timestamp = cells.next().unwrap_or(Cell::Null).into_time();
            callback(&collection, &id, &subset, timestamp)?;
        }
        Ok(())
    }

    async fn add_time_series_points(
        &self,
        ctx: &Context,
        points: &[TimeSeriesPoint],
    ) -> Result<(), Error> {
        check(ctx)?;
        if points.is_empty() {
            return Ok(());
        }
        let conflict = match self.driver {
            Driver::MySql => "ON DUPLICATE KEY UPDATE value = VALUES(value)",
            _ => "ON CONFLICT (name, \"timestamp\") DO UPDATE SET value = excluded.value",
        };
        for batch in points.chunks(BATCH_SIZE) {
            let placeholders = vec!["(?, ?, ?n)"; batch.len()].join(", ");
            let sql = format!(
                "INSERT INTO {} ({}) VALUES {placeholders} {conflict}",
                self.quote(&self.points),
                self.columns(&["name", "timestamp", "value"])
            );
            let mut args = Vec::with_capacity(batch.len() * 3);
            for point in batch {
                args.push(Arg::Text(point.name.clone()));
                args.push(self.time_arg(&point.timestamp));
                args.push(Arg::Float(point.value));
            }
            self.execute(&sql, &args).await?;
        }
        Ok(())
    }

    async fn get_time_series_points(
        &self,
        ctx: &Context,
        name: &str,
        begin: Time,
        end: Time,
        duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error> {
        check(ctx)?;
        let points = self.quote(&self.points);
        let seconds = whole_seconds(duration);
        // Go binds the bucket width three times, then the name and the bounds.
        let (sql, args) = match self.driver {
            Driver::Postgres => (
                format!(
                    "SELECT name, bucket_timestamp AS timestamp, CAST(value AS DOUBLE PRECISION) AS value FROM (\
                     SELECT *, TO_TIMESTAMP((EXTRACT(epoch FROM \"timestamp\")::int / CAST(? AS INT)) * CAST(? AS INT)) AS bucket_timestamp,\
                     ROW_NUMBER() OVER (PARTITION BY (EXTRACT(epoch FROM \"timestamp\")::int / CAST(? AS INT)) ORDER BY \"timestamp\" DESC) AS rn \
                     FROM {points} WHERE name = ? and \"timestamp\" >= ? and \"timestamp\" <= ?) AS t WHERE rn = 1"
                ),
                vec![
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Text(name.to_owned()),
                    self.time_arg(&begin),
                    self.time_arg(&end),
                ],
            ),
            Driver::MySql => (
                format!(
                    "SELECT name, bucket_timestamp AS timestamp, value FROM (\
                     SELECT *, FROM_UNIXTIME(FLOOR(UNIX_TIMESTAMP(timestamp) / ?) * ?) AS bucket_timestamp,\
                     ROW_NUMBER() OVER (PARTITION BY FLOOR(UNIX_TIMESTAMP(timestamp) / ?) ORDER BY timestamp DESC) AS rn \
                     FROM {points} WHERE name = ? and timestamp >= ? and timestamp <= ?) AS t WHERE rn = 1"
                ),
                vec![
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Text(name.to_owned()),
                    self.time_arg(&begin),
                    self.time_arg(&end),
                ],
            ),
            Driver::Sqlite => (
                format!(
                    "select name, bucket_timestamp as timestamp, value from (\
                     select *, datetime(strftime('%s', substr(timestamp, 0, 20)) / ? * ?, 'unixepoch') as bucket_timestamp,\
                     row_number() over (partition by strftime('%s', substr(timestamp, 0, 20)) / ? order by timestamp desc) as rn \
                     from {points} where name = ? and timestamp >= ? and timestamp <= ?) where rn = 1"
                ),
                vec![
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Int(seconds),
                    Arg::Text(name.to_owned()),
                    self.time_arg(&begin),
                    self.time_arg(&end),
                ],
            ),
        };
        let rows = self
            .query(&sql, &args, &[Kind::Text, Kind::Time, Kind::Real])
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let mut cells = row.into_iter();
                TimeSeriesPoint {
                    name: cells.next().unwrap_or(Cell::Null).into_text(),
                    timestamp: cells.next().unwrap_or(Cell::Null).into_time(),
                    value: cells.next().unwrap_or(Cell::Null).into_real(),
                }
            })
            .collect())
    }
}

impl SqlDatabase {
    /// Runs `ALTER TABLE ... ADD INDEX`, ignoring MySQL's duplicate-index error
    /// (1061), as Go's `Init` does.
    #[cfg(feature = "mysql")]
    async fn add_index_ignoring_duplicates(&self, sql: &str) -> Result<(), Error> {
        let pool = match &self.client {
            Client::MySql(pool) => pool.clone(),
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => unreachable!("MySQL indexes are added on MySQL stores only"),
        };
        match sqlx::query(AssertSqlSafe(sql.to_owned()))
            .execute(&pool)
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let duplicate = err
                    .as_database_error()
                    .and_then(|e| e.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>())
                    .is_some_and(|e| e.number() == 1061);
                if duplicate { Ok(()) } else { Err(backend(err)) }
            }
        }
    }

    #[cfg(not(feature = "mysql"))]
    async fn add_index_ignoring_duplicates(&self, _sql: &str) -> Result<(), Error> {
        unreachable!("MySQL indexes are added on MySQL stores only");
    }
}

/// The placeholders of an `IN (...)` list. An empty list is `NULL`, as GORM
/// renders an empty slice, which matches nothing.
fn in_list(n: usize) -> String {
    if n == 0 {
        "NULL".to_owned()
    } else {
        vec!["?"; n].join(", ")
    }
}

/// Creates a SQL store for each registered prefix (Go's `init`).
pub fn register(registry: &mut Registry) {
    #[cfg(feature = "postgres")]
    registry.register(
        &[scheme::POSTGRES_PREFIX, scheme::POSTGRESQL_PREFIX],
        open_postgres,
    );
    #[cfg(feature = "mysql")]
    registry.register(&[scheme::MYSQL_PREFIX], open_mysql);
    #[cfg(feature = "sqlite")]
    registry.register(&[scheme::SQLITE_PREFIX], open_sqlite);
}

#[cfg(feature = "postgres")]
fn open_postgres(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Box<dyn Database>, Error> {
    Ok(Box::new(SqlDatabase::postgres(path, table_prefix, opts)?))
}

#[cfg(feature = "mysql")]
fn open_mysql(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Box<dyn Database>, Error> {
    Ok(Box::new(SqlDatabase::mysql(path, table_prefix, opts)?))
}

#[cfg(feature = "sqlite")]
fn open_sqlite(path: &str, table_prefix: &str, opts: &[Opt]) -> Result<Box<dyn Database>, Error> {
    Ok(Box::new(SqlDatabase::sqlite(path, table_prefix, opts)?))
}

#[cfg(test)]
mod tests {
    use crate::cache::Database;
    use crate::cache::database_test;

    #[cfg(feature = "sqlite")]
    use crate::cache::{Score, ScoreCondition, ScorePatch, TimeSeriesPoint, string};
    #[cfg(feature = "sqlite")]
    use gorse_common::gocontext::Context;
    #[cfg(feature = "sqlite")]
    use gorse_common::gotime::{self, Month, Time};

    #[cfg(feature = "mysql")]
    use gorse_storage_core::scheme::{MYSQL_ISOLATION_VARIABLE_QUERY, MYSQL_PREFIX};
    #[cfg(any(feature = "mysql", feature = "postgres"))]
    use sqlx::Connection as _;

    use super::*;

    /// The database the suites create and drop, as Go's `gorse_cache_test`.
    #[cfg(any(feature = "mysql", feature = "postgres"))]
    const TEST_DATABASE: &str = "gorse_cache_test";

    /// A score document with the given categories.
    #[cfg(feature = "sqlite")]
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
    #[cfg(feature = "sqlite")]
    fn probe_time() -> Time {
        gotime::date(2023, Month::JANUARY, 1, 0, 0, 0, 500, &gotime::utc())
    }

    /// Every row of `sql` on a SQLite store, each column as text (`None` for
    /// NULL).
    #[cfg(feature = "sqlite")]
    fn sqlite_rows(db: &SqlDatabase, sql: &str) -> Vec<Vec<Option<String>>> {
        let conn = match &db.client {
            Client::Sqlite(conn) => Arc::clone(conn),
            #[cfg(any(feature = "mysql", feature = "postgres"))]
            _ => panic!("the store is not SQLite"),
        };
        let guard = conn.lock().unwrap();
        let conn = guard.as_ref().expect("the store is closed");
        let mut stmt = conn.prepare(sql).unwrap();
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                (0..columns)
                    .map(|i| match row.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Ok(None),
                        rusqlite::types::ValueRef::Integer(n) => Ok(Some(n.to_string())),
                        rusqlite::types::ValueRef::Real(f) => Ok(Some(f.to_string())),
                        rusqlite::types::ValueRef::Text(t) => {
                            Ok(Some(String::from_utf8_lossy(t).into_owned()))
                        }
                        rusqlite::types::ValueRef::Blob(_) => panic!("unexpected blob"),
                    })
                    .collect::<Result<Vec<_>, rusqlite::Error>>()
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    /// The backend tests run only when the environment names a server, as Go's
    /// `t.Skip` does when `MYSQL_URI` or `POSTGRES_URI` is unset.
    #[cfg(any(feature = "mysql", feature = "postgres"))]
    fn dsn_from_env(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|dsn| !dsn.is_empty())
    }

    /// A MySQL connection to the server `body` names, not to a database, as
    /// Go's suite opens its `sql.DB` to create the test database.
    #[cfg(feature = "mysql")]
    async fn mysql_admin_connection(body: &str) -> sqlx::MySqlConnection {
        let (options, _) = sqlconn::mysql_connect_options(body).unwrap();
        sqlx::MySqlConnection::connect_with(&options).await.unwrap()
    }

    /// A PostgreSQL connection to the default database of the server, as Go's
    /// suite does.
    #[cfg(feature = "postgres")]
    async fn postgres_admin_connection(dsn: &str) -> sqlx::PgConnection {
        let options: sqlx::postgres::PgConnectOptions =
            format!("{dsn}?sslmode=disable").parse().unwrap();
        sqlx::PgConnection::connect_with(&options).await.unwrap()
    }

    /// The transaction isolation the MySQL session of the store has, read
    /// under the name the server uses, as Go's `TestInit` reads it.
    #[cfg(feature = "mysql")]
    async fn mysql_isolation_level(db: &SqlDatabase) -> String {
        use sqlx::Row;
        let pool = match &db.client {
            Client::MySql(pool) => pool.clone(),
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => panic!("the store is not MySQL"),
        };
        let row = sqlx::query(MYSQL_ISOLATION_VARIABLE_QUERY)
            .fetch_one(&pool)
            .await
            .unwrap();
        let name: String = row.try_get(0).unwrap();
        let sql = format!("SELECT @@{name}");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap()
    }

    #[cfg(feature = "sqlite")]
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
    #[cfg(feature = "sqlite")]
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
                    Some(r#"["x\u003cy","é"]"#.to_owned()),
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
    #[cfg(feature = "sqlite")]
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
    #[cfg(feature = "sqlite")]
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
    #[cfg(feature = "sqlite")]
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
    #[cfg(feature = "sqlite")]
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
            r#"["x\u003cy\u0026z\u003e","é\u2028"]"#
        );
        assert_eq!(json_string_array(&[]), "[]");
    }

    /// The MySQL suite. Go's `MySQLTestSuite.TestInit` reads back the isolation
    /// level the connection was opened with.
    #[cfg(feature = "mysql")]
    #[tokio::test]
    async fn mysql_suite() {
        let Some(dsn) = dsn_from_env("MYSQL_URI") else {
            eprintln!("skipped: MYSQL_URI is not set");
            return;
        };
        let body = dsn
            .strip_prefix(MYSQL_PREFIX)
            .expect("MYSQL_URI has the mysql:// prefix");
        let mut admin = mysql_admin_connection(body).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {TEST_DATABASE}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {TEST_DATABASE}"
        )))
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
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn postgres_suite() {
        let Some(dsn) = dsn_from_env("POSTGRES_URI") else {
            eprintln!("skipped: POSTGRES_URI is not set");
            return;
        };
        let mut admin = postgres_admin_connection(&dsn).await;
        let lower = TEST_DATABASE.to_lowercase();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {TEST_DATABASE}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {TEST_DATABASE}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        drop(admin);
        let db =
            SqlDatabase::postgres(&format!("{dsn}{lower}?sslmode=disable"), "gorse_", &[]).unwrap();
        db.init().await.unwrap();
        database_test::run(&db).await;
        db.close().await.unwrap();
    }
}
