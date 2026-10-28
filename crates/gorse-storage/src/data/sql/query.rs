// Ports: storage/data/sql.go (the statements GORM builds, written out) @ add21b5

//! The statement builder and the generic runners the SQL data store uses on
//! all three dialects.
//!
//! GORM builds each statement from Go values and binds them. Here a [`Sql`]
//! collects the text and the values, and writes the placeholder each dialect
//! wants: `?` for MySQL and SQLite, `$n` for PostgreSQL. The runners are
//! generic over the sqlx database, so one body serves the three pools; they
//! take the bound types and the row decoders as parameters, and the callers
//! dispatch on [`super::Pools`].
//!
//! No std lock is held across an `.await` here (D-122). A stream runs its
//! query on a spawned task that sends each batch over a channel, as Go's
//! `GetItemStream` runs its query on a goroutine.

use std::sync::Arc;

use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use gorse_common::gocontext::Context;
use sqlx::{AssertSqlSafe, Database, Decode, Encode, IntoArguments, Row, Type};
use tokio::sync::mpsc;

use super::super::Error;

/// The SQL dialect a store speaks. Go's `SQLDriver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// MySQL, and MariaDB as far as the Go CI runs it.
    MySql,
    /// PostgreSQL.
    Postgres,
    /// SQLite, through `modernc.org/sqlite` in Go and bundled SQLite here.
    Sqlite,
}

/// One value a statement binds. Every value the store writes is one of these:
/// text (including times and JSON), an integer, a float or a boolean, or a
/// `NULL`.
#[derive(Debug, Clone, PartialEq)]
pub enum Bind {
    /// `NULL`, as a nil pointer or a `json` column left empty.
    Null,
    /// Text, which a time or a JSON document is written as.
    Text(String),
    /// An `INTEGER`.
    Int(i64),
    /// A `REAL`.
    Float(f64),
    /// A `BOOLEAN`, which MySQL and SQLite store as an integer.
    Bool(bool),
}

impl From<String> for Bind {
    fn from(value: String) -> Bind {
        Bind::Text(value)
    }
}

impl From<&str> for Bind {
    fn from(value: &str) -> Bind {
        Bind::Text(value.to_owned())
    }
}

impl From<i64> for Bind {
    fn from(value: i64) -> Bind {
        Bind::Int(value)
    }
}

impl From<f64> for Bind {
    fn from(value: f64) -> Bind {
        Bind::Float(value)
    }
}

impl From<bool> for Bind {
    fn from(value: bool) -> Bind {
        Bind::Bool(value)
    }
}

impl From<Option<String>> for Bind {
    fn from(value: Option<String>) -> Bind {
        value.map_or(Bind::Null, Bind::Text)
    }
}

/// A statement under construction: its text and the values it binds, in the
/// order their placeholders appear.
#[derive(Debug, Clone)]
pub struct Sql {
    driver: Driver,
    text: String,
    binds: Vec<Bind>,
}

impl Sql {
    /// An empty statement for `driver`.
    pub fn new(driver: Driver) -> Sql {
        Sql {
            driver,
            text: String::new(),
            binds: Vec::new(),
        }
    }

    /// Appends SQL text.
    pub fn push(&mut self, text: &str) -> &mut Sql {
        self.text.push_str(text);
        self
    }

    /// Appends the placeholder for `value` and binds it.
    pub fn bind(&mut self, value: impl Into<Bind>) -> &mut Sql {
        self.binds.push(value.into());
        match self.driver {
            Driver::Postgres => {
                self.text.push('$');
                self.text.push_str(&self.binds.len().to_string());
            }
            Driver::MySql | Driver::Sqlite => self.text.push('?'),
        }
        self
    }

    /// Appends `text` as the placeholder's cast on PostgreSQL, and nothing on
    /// the others. The cast types a bound text as the column's type.
    pub fn cast(&mut self, pg_type: &str) -> &mut Sql {
        if self.driver == Driver::Postgres {
            self.text.push_str("::");
            self.text.push_str(pg_type);
        }
        self
    }
}

/// The sqlx query for `sql`, with every value bound.
fn query_of<'q, DB>(sql: &'q Sql) -> sqlx::query::Query<'q, DB, <DB as Database>::Arguments>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    String: Encode<'q, DB> + Type<DB>,
    i64: Encode<'q, DB> + Type<DB>,
    f64: Encode<'q, DB> + Type<DB>,
    bool: Encode<'q, DB> + Type<DB>,
    Option<String>: Encode<'q, DB> + Type<DB>,
{
    // sqlx asks for an audit of dynamic SQL text. This text holds only the
    // statement's literals, the table names from the operator's prefix and
    // the placeholders; every value goes through `bind`, never into the text.
    let mut query = sqlx::query(AssertSqlSafe(sql.text.as_str()));
    for bind in &sql.binds {
        query = match bind {
            Bind::Null => query.bind(None::<String>),
            Bind::Text(text) => query.bind(text.clone()),
            Bind::Int(value) => query.bind(*value),
            Bind::Float(value) => query.bind(*value),
            Bind::Bool(value) => query.bind(*value),
        };
    }
    query
}

/// Wraps a driver error as the data store's backend error.
pub fn backend(err: sqlx::Error) -> Error {
    Error::Backend(Arc::new(err))
}

/// Go's `ctx.Err()` before a query: a cancelled context fails the query
/// before it starts, as GORM's `WithContext` does.
pub fn check(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(Error::Context(err)),
        None => Ok(()),
    }
}

/// The rows a statement changed, which each dialect's result type reports
/// through its own inherent method.
pub trait Affected {
    /// The number of rows the statement changed.
    fn affected(&self) -> u64;
}

impl Affected for sqlx::mysql::MySqlQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}

impl Affected for sqlx::postgres::PgQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}

impl Affected for sqlx::sqlite::SqliteQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}

/// Runs a statement that returns no rows and reports how many rows it
/// changed (Go's `RowsAffected`).
pub async fn execute<DB>(pool: &sqlx::Pool<DB>, sql: &Sql) -> Result<u64, Error>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    <DB as Database>::QueryResult: Affected,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> bool: Encode<'q, DB> + Type<DB>,
    for<'q> Option<String>: Encode<'q, DB> + Type<DB>,
{
    let result = query_of::<DB>(sql).execute(pool).await.map_err(backend)?;
    Ok(result.affected())
}

/// Runs a query and decodes every row it returns, in order.
pub async fn fetch_all<DB, T, F>(
    pool: &sqlx::Pool<DB>,
    sql: &Sql,
    decode: F,
) -> Result<Vec<T>, Error>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    F: Fn(&DB::Row) -> Result<T, Error>,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> bool: Encode<'q, DB> + Type<DB>,
    for<'q> Option<String>: Encode<'q, DB> + Type<DB>,
{
    let rows = query_of::<DB>(sql).fetch_all(pool).await.map_err(backend)?;
    rows.iter().map(decode).collect()
}

/// Runs a query and decodes its first row, or returns `None` when it has no
/// rows. Go's `Row().Scan` and `Scan` into a single value.
pub async fn fetch_optional<DB, T, F>(
    pool: &sqlx::Pool<DB>,
    sql: &Sql,
    decode: F,
) -> Result<Option<T>, Error>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    F: Fn(&DB::Row) -> Result<T, Error>,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> bool: Encode<'q, DB> + Type<DB>,
    for<'q> Option<String>: Encode<'q, DB> + Type<DB>,
{
    let row = query_of::<DB>(sql)
        .fetch_optional(pool)
        .await
        .map_err(backend)?;
    row.as_ref().map(decode).transpose()
}

/// Runs a query and streams its rows, decoded and grouped into batches of
/// `batch_size`, as Go's `GetItemStream` and its siblings do with a goroutine
/// and two channels. A cancelled `ctx` ends the stream with its error, checked
/// before each batch. Dropping the stream drops the receiver, and the task then
/// stops reading rows.
pub fn stream_batches<DB, T, F>(
    pool: sqlx::Pool<DB>,
    sql: Sql,
    ctx: Context,
    batch_size: usize,
    decode: F,
) -> BoxStream<'static, Result<Vec<T>, Error>>
where
    DB: Database,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    F: Fn(&DB::Row) -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> bool: Encode<'q, DB> + Type<DB>,
    for<'q> Option<String>: Encode<'q, DB> + Type<DB>,
{
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        let mut rows = query_of::<DB>(&sql).fetch(&pool);
        let mut batch = Vec::with_capacity(batch_size);
        while let Some(row) = rows.next().await {
            let item = match row.map_err(backend).and_then(|row| decode(&row)) {
                Ok(item) => item,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };
            batch.push(item);
            if batch.len() == batch_size {
                if let Err(err) = check(&ctx) {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
                let full = std::mem::replace(&mut batch, Vec::with_capacity(batch_size));
                if tx.send(Ok(full)).await.is_err() {
                    return;
                }
            }
        }
        if !batch.is_empty() {
            let _ = tx.send(Ok(batch)).await;
        }
    });
    Box::pin(futures_util::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|item| (item, rx))
    }))
}

/// Reads the text column at `index`. Every text the store reads is a column
/// of text, after the dialect's cast; see [`super::time`] for the times.
pub fn text<'r, R>(row: &'r R, index: usize) -> Result<String, Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
{
    row.try_get(index).map_err(backend)
}

/// Reads the nullable text column at `index`.
pub fn optional_text<'r, R>(row: &'r R, index: usize) -> Result<Option<String>, Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    String: Decode<'r, R::Database> + Type<R::Database>,
{
    row.try_get(index).map_err(backend)
}

/// Reads the boolean column at `index`.
pub fn boolean<'r, R>(row: &'r R, index: usize) -> Result<bool, Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    bool: Decode<'r, R::Database> + Type<R::Database>,
{
    row.try_get(index).map_err(backend)
}

/// Reads the float column at `index`.
pub fn float<'r, R>(row: &'r R, index: usize) -> Result<f64, Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    f64: Decode<'r, R::Database> + Type<R::Database>,
{
    row.try_get(index).map_err(backend)
}

/// Reads the integer column at `index`.
pub fn integer<'r, R>(row: &'r R, index: usize) -> Result<i64, Error>
where
    R: Row,
    usize: sqlx::ColumnIndex<R>,
    i64: Decode<'r, R::Database> + Type<R::Database>,
{
    row.try_get(index).map_err(backend)
}
