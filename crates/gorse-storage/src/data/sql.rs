// Ports: storage/data/sql.go @ add21b5

//! The SQL data store: MySQL, PostgreSQL and SQLite behind one
//! [`Database`] (Go's `SQLDatabase`).
//!
//! The statements are written out in [`query`]-built SQL, with the dialect
//! differences in the module that owns each table: [`items`], [`users`] and
//! [`feedback`] hold the methods, [`search`] the full-text index, [`schema`]
//! the `Init()` tables, [`time`] the time values and [`open`] the three
//! creators. The SQLite connections register the `json_contains` function Go
//! registers (see [`sqlite`]).
//!
//! The dialect is a value, not a type: one store holds one of three pools
//! ([`Pools`]), and each method dispatches on it. The row decoders are generic
//! over the sqlx database, so each dispatch is one line.
//!
//! Deviations are logged in `PORTING_NOTES.md` (X-WP22-1 to X-WP22-6).

mod feedback;
mod items;
mod open;
mod query;
mod schema;
mod search;
mod sqlite;
mod time;
mod users;

#[cfg(test)]
mod tests;

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_config::config::SearchConfig;
use gorse_storage_core::scheme::TablePrefix;
use sqlx::{AssertSqlSafe, Row as _};

use super::types::{Feedback, Item, ItemPatch, ScoredItem, User, UserPatch};
use super::{Database, Error, GetOptions, Registry, ScanOption};
pub use open::{mysql_creator, postgres_creator, sqlite_creator};
use query::{Driver, Sql};

/// The pool of one dialect. Each store holds exactly one (Go's `driver` field
/// selects the dialect, and `client` is the `*sql.DB`).
pub enum Pools {
    /// A MySQL pool.
    MySql(sqlx::MySqlPool),
    /// A PostgreSQL pool.
    Postgres(sqlx::PgPool),
    /// A SQLite pool.
    Sqlite(sqlx::SqlitePool),
}

/// Go's `SQLDatabase`: a data store on a SQL database.
pub struct SqlDatabase {
    prefix: TablePrefix,
    pools: Pools,
    /// The search vector `Reconcile` built for PostgreSQL, which the search
    /// query uses. Guarded by a std lock that is never held across an await.
    search_vector: Mutex<String>,
}

impl SqlDatabase {
    /// A store on `pools` whose tables carry `prefix`.
    fn new(pools: Pools, prefix: TablePrefix) -> SqlDatabase {
        SqlDatabase {
            prefix,
            pools,
            search_vector: Mutex::new(String::new()),
        }
    }

    /// The dialect of the store.
    fn driver(&self) -> Driver {
        match self.pools {
            Pools::MySql(_) => Driver::MySql,
            Pools::Postgres(_) => Driver::Postgres,
            Pools::Sqlite(_) => Driver::Sqlite,
        }
    }

    /// An empty statement for this store's dialect.
    fn sql(&self) -> Sql {
        Sql::new(self.driver())
    }

    /// The name of the `items` table, with the prefix.
    fn items_table(&self) -> String {
        table_name(self.prefix.items_table())
    }

    /// The name of the `users` table, with the prefix.
    fn users_table(&self) -> String {
        table_name(self.prefix.users_table())
    }

    /// The name of the `feedback` table, with the prefix.
    fn feedback_table(&self) -> String {
        table_name(self.prefix.feedback_table())
    }

    /// The search vector the last `Reconcile` built for PostgreSQL.
    fn search_vector(&self) -> String {
        self.search_vector
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_search_vector(&self, vector: String) {
        *self
            .search_vector
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = vector;
    }

    /// Runs a statement that returns no rows, and reports the rows it changed.
    async fn exec(&self, sql: &Sql) -> Result<u64, Error> {
        match &self.pools {
            Pools::MySql(pool) => query::execute(pool, sql).await,
            Pools::Postgres(pool) => query::execute(pool, sql).await,
            Pools::Sqlite(pool) => query::execute(pool, sql).await,
        }
    }

    /// Runs a query for its first column of text, or `None` when it has no row
    /// or the value is `NULL`.
    async fn one_text(&self, sql: &Sql) -> Result<Option<String>, Error> {
        let value = match &self.pools {
            Pools::MySql(pool) => {
                query::fetch_optional(pool, sql, |row| query::optional_text(row, 0)).await?
            }
            Pools::Postgres(pool) => {
                query::fetch_optional(pool, sql, |row| query::optional_text(row, 0)).await?
            }
            Pools::Sqlite(pool) => {
                query::fetch_optional(pool, sql, |row| query::optional_text(row, 0)).await?
            }
        };
        Ok(value.flatten())
    }

    /// Runs a query for its first column of integers, or `None` when it has no
    /// row.
    async fn one_integer(&self, sql: &Sql) -> Result<Option<i64>, Error> {
        match &self.pools {
            Pools::MySql(pool) => {
                query::fetch_optional(pool, sql, |row| query::integer(row, 0)).await
            }
            Pools::Postgres(pool) => {
                query::fetch_optional(pool, sql, |row| query::integer(row, 0)).await
            }
            Pools::Sqlite(pool) => {
                query::fetch_optional(pool, sql, |row| query::integer(row, 0)).await
            }
        }
    }

    /// Go's `Init`'s statements for this store, run in order.
    async fn init_tables(&self) -> Result<(), Error> {
        let prefix = String::from_utf8_lossy(&self.prefix.0).into_owned();
        for statement in schema::init_statements(self.driver(), &prefix) {
            let mut sql = self.sql();
            sql.push(&statement);
            self.exec(&sql).await?;
        }
        Ok(())
    }

    /// Go's `Count*`: the rows of `table`. MySQL and PostgreSQL estimate it from
    /// the table's statistics, as Go does; SQLite counts the rows.
    async fn count_table(&self, table: String) -> Result<i64, Error> {
        match &self.pools {
            Pools::MySql(pool) => {
                // The table name comes from the operator's prefix, so the text
                // is audited as built; no value is bound into it.
                let row = sqlx::query(AssertSqlSafe(format!("show table status like '{table}'")))
                    .fetch_optional(pool)
                    .await
                    .map_err(query::backend)?;
                match row {
                    Some(row) => {
                        let rows: Option<u64> = row.try_get("Rows").map_err(query::backend)?;
                        Ok(rows.unwrap_or_default() as i64)
                    }
                    None => Ok(0),
                }
            }
            Pools::Postgres(pool) => {
                let row = sqlx::query(AssertSqlSafe(format!(
                    "SELECT reltuples AS estimate FROM pg_class where relname = '{table}'"
                )))
                .fetch_optional(pool)
                .await
                .map_err(query::backend)?;
                match row {
                    Some(row) => {
                        let estimate: f32 = row.try_get(0).map_err(query::backend)?;
                        Ok((estimate as i64).max(0))
                    }
                    None => Ok(0),
                }
            }
            Pools::Sqlite(_) => {
                let mut sql = self.sql();
                sql.push(&format!("SELECT COUNT(*) FROM {table}"));
                Ok(self.one_integer(&sql).await?.unwrap_or_default())
            }
        }
    }

    /// Go's `GetUserStream`: every user, in batches of `batch_size`.
    fn get_user_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>> {
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            users::columns(self.driver()),
            self.users_table()
        ));
        let batch = usize::try_from(batch_size).unwrap_or(0);
        match &self.pools {
            Pools::MySql(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, users::user_from_row)
            }
            Pools::Postgres(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, users::user_from_row)
            }
            Pools::Sqlite(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, users::user_from_row)
            }
        }
    }
}

/// The name of a table, from the bytes [`TablePrefix`] keeps.
fn table_name(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

#[async_trait]
impl Database for SqlDatabase {
    async fn close(&self) -> Result<(), Error> {
        match &self.pools {
            Pools::MySql(pool) => pool.close().await,
            Pools::Postgres(pool) => pool.close().await,
            Pools::Sqlite(pool) => pool.close().await,
        }
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        use sqlx::Connection as _;
        match &self.pools {
            Pools::MySql(pool) => {
                let mut conn = pool.acquire().await.map_err(query::backend)?;
                conn.ping().await.map_err(query::backend)
            }
            Pools::Postgres(pool) => {
                let mut conn = pool.acquire().await.map_err(query::backend)?;
                conn.ping().await.map_err(query::backend)
            }
            Pools::Sqlite(pool) => {
                let mut conn = pool.acquire().await.map_err(query::backend)?;
                conn.ping().await.map_err(query::backend)
            }
        }
    }

    async fn init(&self) -> Result<(), Error> {
        self.init_tables().await
    }

    async fn reconcile(&self, config: &SearchConfig) -> Result<(), Error> {
        SqlDatabase::reconcile(self, config).await
    }

    async fn optimize(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        for table in [
            self.items_table(),
            self.feedback_table(),
            self.users_table(),
        ] {
            let mut sql = self.sql();
            sql.push(&format!("DELETE FROM {table}"));
            self.exec(&sql).await?;
        }
        Ok(())
    }

    async fn batch_insert_items(&self, ctx: &Context, items: &[Item]) -> Result<(), Error> {
        SqlDatabase::batch_insert_items(self, ctx, items).await
    }

    async fn batch_get_items(
        &self,
        ctx: &Context,
        item_ids: &[String],
        opts: &GetOptions,
    ) -> Result<Vec<Item>, Error> {
        SqlDatabase::batch_get_items(self, ctx, item_ids, opts).await
    }

    async fn delete_item(&self, ctx: &Context, item_id: &str) -> Result<(), Error> {
        SqlDatabase::delete_item(self, ctx, item_id).await
    }

    async fn get_item(&self, ctx: &Context, item_id: &str) -> Result<Item, Error> {
        SqlDatabase::get_item(self, ctx, item_id).await
    }

    async fn search_items(
        &self,
        ctx: &Context,
        query: &str,
        n: i64,
    ) -> Result<Vec<ScoredItem>, Error> {
        SqlDatabase::search_items(self, ctx, query, n).await
    }

    async fn modify_item(
        &self,
        ctx: &Context,
        item_id: &str,
        patch: &ItemPatch,
    ) -> Result<(), Error> {
        SqlDatabase::modify_item(self, ctx, item_id, patch).await
    }

    async fn get_items(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error> {
        SqlDatabase::get_items(self, ctx, cursor, n, begin_time).await
    }

    async fn get_latest_items(
        &self,
        ctx: &Context,
        n: i64,
        categories: &[String],
        after: Option<Time>,
    ) -> Result<Vec<Item>, Error> {
        SqlDatabase::get_latest_items(self, ctx, n, categories, after).await
    }

    async fn get_item_feedback(
        &self,
        ctx: &Context,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        SqlDatabase::get_item_feedback(self, ctx, item_id, feedback_types).await
    }

    async fn batch_insert_users(&self, ctx: &Context, users: &[User]) -> Result<(), Error> {
        SqlDatabase::batch_insert_users(self, ctx, users).await
    }

    async fn delete_user(&self, ctx: &Context, user_id: &str) -> Result<(), Error> {
        SqlDatabase::delete_user(self, ctx, user_id).await
    }

    async fn get_user(&self, ctx: &Context, user_id: &str) -> Result<User, Error> {
        SqlDatabase::get_user(self, ctx, user_id).await
    }

    async fn modify_user(
        &self,
        ctx: &Context,
        user_id: &str,
        patch: &UserPatch,
    ) -> Result<(), Error> {
        SqlDatabase::modify_user(self, ctx, user_id, patch).await
    }

    async fn get_users(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
    ) -> Result<(String, Vec<User>), Error> {
        SqlDatabase::get_users(self, ctx, cursor, n).await
    }

    async fn get_user_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        end_time: Option<Time>,
        feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error> {
        SqlDatabase::get_user_feedback(self, ctx, user_id, end_time, feedback_types).await
    }

    async fn get_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        SqlDatabase::get_user_item_feedback(self, ctx, user_id, item_id, feedback_types).await
    }

    async fn delete_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<i64, Error> {
        SqlDatabase::delete_user_item_feedback(self, ctx, user_id, item_id, feedback_types).await
    }

    async fn batch_insert_feedback(
        &self,
        ctx: &Context,
        feedback: &[Feedback],
        insert_user: bool,
        insert_item: bool,
        overwrite: bool,
    ) -> Result<(), Error> {
        SqlDatabase::batch_insert_feedback(self, ctx, feedback, insert_user, insert_item, overwrite)
            .await
    }

    async fn get_feedback(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
        end_time: Option<Time>,
        feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error> {
        SqlDatabase::get_feedback(self, ctx, cursor, n, begin_time, end_time, feedback_types).await
    }

    fn get_user_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>> {
        SqlDatabase::get_user_stream(self, ctx, batch_size)
    }

    fn get_item_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>> {
        SqlDatabase::get_item_stream(self, ctx, batch_size, time_limit)
    }

    fn get_feedback_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>> {
        SqlDatabase::get_feedback_stream(self, ctx, batch_size, options)
    }

    async fn count_users(&self, _ctx: &Context) -> Result<i64, Error> {
        self.count_table(self.users_table()).await
    }

    async fn count_items(&self, _ctx: &Context) -> Result<i64, Error> {
        self.count_table(self.items_table()).await
    }

    async fn count_feedback(&self, _ctx: &Context) -> Result<i64, Error> {
        self.count_table(self.feedback_table()).await
    }
}

/// Registers the SQL backends in the data registry. Go registers them in
/// `init`; a node calls this once on the registry it builds (D-126).
pub fn register(registry: &mut Registry) {
    use gorse_storage_core::scheme::{
        MYSQL_PREFIX, POSTGRES_PREFIX, POSTGRESQL_PREFIX, SQLITE_PREFIX,
    };
    registry.register(&[MYSQL_PREFIX], open::mysql_creator);
    registry.register(
        &[POSTGRES_PREFIX, POSTGRESQL_PREFIX],
        open::postgres_creator,
    );
    registry.register(&[SQLITE_PREFIX], open::sqlite_creator);
}
