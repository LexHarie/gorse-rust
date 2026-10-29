// Ports: storage/data/sql.go (the item methods of SQLDatabase and NewSQLItem) @ add21b5

//! The item methods of the SQL data store: the writes, the reads, the streams
//! and the row decoders they share with the search.
//!
//! Go's `SQLItem` keeps the categories and labels as JSON text, with `null` for
//! a nil value, so an item the store writes reads back the same. `time_stamp`
//! and `update_at` are text on every dialect; see [`super::time`].

use std::collections::HashSet;

use base64::Engine;
use futures_core::stream::BoxStream;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::jsonutil;
use sqlx::{ColumnIndex, Decode, Row, Type};

use super::super::types::{self, Item, ItemPatch, Labels, ScoredItem};
use super::super::{Error, GetOptions};
use super::query::{self, Driver, Sql};
use super::{Pools, SqlDatabase, time};

/// The SQL that selects the columns of an item, in the order
/// [`item_from_row`] reads them. `prefix` is `items.` when the table is
/// aliased, and empty otherwise.
pub fn columns(driver: Driver, prefix: &str) -> String {
    format!(
        "{prefix}item_id, {prefix}is_hidden, {}, {}, {}, {prefix}comment, {}",
        json_column(driver, &format!("{prefix}categories")),
        time::select(driver, &format!("{prefix}time_stamp")),
        json_column(driver, &format!("{prefix}labels")),
        time::select(driver, &format!("{prefix}update_at")),
    )
}

/// A JSON column as text: MySQL returns `json` as bytes that are not text to
/// the decoder, and PostgreSQL returns `json` as a type the text decoder does
/// not take, so both are cast.
pub fn json_column(driver: Driver, column: &str) -> String {
    match driver {
        Driver::MySql => format!("CAST({column} AS CHAR)"),
        Driver::Postgres => format!("{column}::text"),
        Driver::Sqlite => column.to_owned(),
    }
}

/// Go's `jsonutil.Unmarshal` of a stored value, as a `storage` error.
pub(super) fn unmarshal<T: serde::de::DeserializeOwned + Default>(text: &str) -> Result<T, Error> {
    let mut value = T::default();
    jsonutil::unmarshal(text.as_bytes(), &mut value)
        .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
    Ok(value)
}

/// Go's `jsonutil.Unmarshal` of a stored labels value, as a `storage` error.
/// Labels nested past Go's limit are refused with Go's text.
pub(super) fn unmarshal_labels(text: &str) -> Result<Labels, Error> {
    types::unmarshal_labels(text.as_bytes()).map_err(|err| Error::Backend(std::sync::Arc::new(err)))
}

/// Decodes an item from the row [`columns`] selects.
pub fn item_from_row<R>(row: &R) -> Result<Item, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> bool: Decode<'r, R::Database> + Type<R::Database>,
{
    Ok(Item {
        item_id: query::text(row, 0)?,
        is_hidden: query::boolean(row, 1)?,
        categories: unmarshal(&query::text(row, 2)?)?,
        timestamp: time::parse(&query::text(row, 3)?)?,
        labels: unmarshal_labels(&query::text(row, 4)?)?,
        comment: query::text(row, 5)?,
        update_at: time::parse(&query::text(row, 6)?)?,
    })
}

/// Decodes an item that holds only its id, as `ReturnId` selects it.
pub fn item_id_from_row<R>(row: &R) -> Result<Item, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
{
    Ok(Item {
        item_id: query::text(row, 0)?,
        ..Item::default()
    })
}

/// Decodes a scored item whose score is a float column (MySQL and SQLite).
pub fn scored_item_float<R>(row: &R) -> Result<ScoredItem, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> bool: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> f64: Decode<'r, R::Database> + Type<R::Database>,
{
    Ok(ScoredItem {
        item: item_from_row(row)?,
        score: query::float(row, 7)?,
    })
}

/// Decodes a scored item whose score is selected as text (PostgreSQL), and
/// parsed the way Go's `lib/pq` parses it.
pub fn scored_item_text<R>(row: &R) -> Result<ScoredItem, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> bool: Decode<'r, R::Database> + Type<R::Database>,
{
    let score = query::text(row, 7)?;
    Ok(ScoredItem {
        item: item_from_row(row)?,
        score: score
            .parse()
            .map_err(|err: std::num::ParseFloatError| Error::Backend(std::sync::Arc::new(err)))?,
    })
}

/// The JSON text of the categories filter `opts` and the item methods use,
/// with the operator of the dialect: `@>` on PostgreSQL, `JSON_CONTAINS`
/// elsewhere. SQLite's `JSON_CONTAINS` is the function the connection
/// registers (see [`super::sqlite`]).
fn push_categories_filter(
    sql: &mut Sql,
    driver: Driver,
    column: &str,
    categories: &[String],
) -> Result<(), Error> {
    let json =
        jsonutil::marshal(categories).map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
    let json = String::from_utf8(json).map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
    match driver {
        Driver::Postgres => {
            sql.push(&format!(" AND {column}::jsonb @> "))
                .bind(json)
                .cast("jsonb");
        }
        Driver::MySql | Driver::Sqlite => {
            sql.push(&format!(" AND JSON_CONTAINS({column},"))
                .bind(json)
                .push(")");
        }
    }
    Ok(())
}

/// GORM's `CreateBatchSize` in `storage.NewGORMConfig`: the rows of one
/// `INSERT` statement in a batch insert (X-245).
const CREATE_BATCH_SIZE: usize = 1_000;

impl SqlDatabase {
    /// Go's `BatchInsertItems`: writes the items, the first of each id, and
    /// overwrites the ones already stored with that id.
    ///
    /// GORM writes the rows in statements of [`CREATE_BATCH_SIZE`], one after
    /// another, with no transaction around them (`SkipDefaultTransaction`).
    /// The port does the same, so a batch of any size stays under the bind
    /// parameter limit of one statement (X-245).
    pub(super) async fn batch_insert_items(
        &self,
        ctx: &Context,
        items: &[Item],
    ) -> Result<(), Error> {
        query::check(ctx)?;
        if items.is_empty() {
            return Ok(());
        }
        let now = gotime::now().utc();
        let mut seen = HashSet::new();
        let rows: Vec<&Item> = items
            .iter()
            .filter(|item| seen.insert(item.item_id.as_str()))
            .collect();
        for batch in rows.chunks(CREATE_BATCH_SIZE) {
            query::check(ctx)?;
            let sql = self.item_insert(batch, &now)?;
            self.exec(&sql).await?;
        }
        Ok(())
    }

    /// The `INSERT` of one batch of items, with the upsert clause.
    fn item_insert(&self, items: &[&Item], now: &Time) -> Result<Sql, Error> {
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "INSERT INTO {} (item_id, is_hidden, categories, time_stamp, labels, comment, update_at) VALUES ",
            self.items_table()
        ));
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                sql.push(", ");
            }
            let categories = jsonutil::marshal(&item.categories)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            let labels = jsonutil::marshal(&item.labels)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            sql.push("(")
                .bind(item.item_id.as_str())
                .push(", ")
                .bind(item.is_hidden);
            sql.push(", ").bind(json_text(categories)).cast("json");
            sql.push(", ")
                .bind(time::format(
                    driver,
                    &time::normalize_for_write(driver, &item.timestamp),
                ))
                .cast("timestamptz");
            sql.push(", ").bind(json_text(labels)).cast("json");
            sql.push(", ").bind(item.comment.as_str());
            sql.push(", ")
                .bind(time::format(driver, now))
                .cast("timestamptz");
            sql.push(")");
        }
        sql.push(&upsert_clause(
            driver,
            "item_id",
            &[
                "is_hidden",
                "categories",
                "time_stamp",
                "labels",
                "comment",
                "update_at",
            ],
        ));
        Ok(sql)
    }

    /// Go's `BatchGetItems`: the items with the ids that match `opts`.
    pub(super) async fn batch_get_items(
        &self,
        ctx: &Context,
        item_ids: &[String],
        opts: &GetOptions,
    ) -> Result<Vec<Item>, Error> {
        query::check(ctx)?;
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let driver = self.driver();
        let select = if opts.return_id {
            "item_id".to_owned()
        } else {
            columns(driver, "")
        };
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {select} FROM {} WHERE item_id IN (",
            self.items_table()
        ));
        for (i, id) in item_ids.iter().enumerate() {
            if i > 0 {
                sql.push(",");
            }
            sql.bind(id.as_str());
        }
        sql.push(")");
        if !opts.categories.is_empty() {
            push_categories_filter(&mut sql, driver, "categories", &opts.categories)?;
        }
        if opts.skip_hidden {
            sql.push(" AND is_hidden = ").bind(false);
        }
        if let Some(after) = &opts.after {
            sql.push(" AND time_stamp > ")
                .bind(time::format(driver, after))
                .cast("timestamptz");
        }
        self.fetch_items_all(&sql, opts.return_id).await
    }

    /// Go's `DeleteItem`: deletes the item, then the feedback on it.
    pub(super) async fn delete_item(&self, ctx: &Context, item_id: &str) -> Result<(), Error> {
        query::check(ctx)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "DELETE FROM {} WHERE item_id = ",
            self.items_table()
        ))
        .bind(item_id);
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "DELETE FROM {} WHERE item_id = ",
            self.feedback_table()
        ))
        .bind(item_id);
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `GetItem`: the item, or `ErrNotFound`.
    pub(super) async fn get_item(&self, ctx: &Context, item_id: &str) -> Result<Item, Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE item_id = ",
            columns(driver, ""),
            self.items_table()
        ))
        .bind(item_id);
        match self.fetch_optional_item(&sql).await? {
            Some(item) => Ok(item),
            None => Err(Error::Storage(gorse_storage_core::errors::Error::NotFound)),
        }
    }

    /// Go's `ModifyItem`: sets the fields of `patch` that it sets, and the
    /// write time. A patch that sets nothing changes nothing.
    pub(super) async fn modify_item(
        &self,
        ctx: &Context,
        item_id: &str,
        patch: &ItemPatch,
    ) -> Result<(), Error> {
        query::check(ctx)?;
        let nothing = patch.is_hidden.is_none()
            && patch.categories.is_none()
            && patch.labels == Labels::Nil
            && patch.comment.is_none()
            && patch.timestamp.is_none();
        if nothing {
            tracing::debug!("empty item patch");
            return Ok(());
        }
        let driver = self.driver();
        let now = gotime::now().utc();
        let mut sql = self.sql();
        sql.push(&format!("UPDATE {} SET update_at = ", self.items_table()))
            .bind(time::format(driver, &now))
            .cast("timestamptz");
        if let Some(is_hidden) = patch.is_hidden {
            sql.push(", is_hidden = ").bind(is_hidden);
        }
        if let Some(categories) = &patch.categories {
            let json = jsonutil::marshal(categories)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            sql.push(", categories = ")
                .bind(json_text(json))
                .cast("json");
        }
        if let Some(comment) = &patch.comment {
            sql.push(", comment = ").bind(comment.as_str());
        }
        if patch.labels != Labels::Nil {
            let json = jsonutil::marshal(&patch.labels)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            sql.push(", labels = ").bind(json_text(json)).cast("json");
        }
        if let Some(timestamp) = &patch.timestamp {
            let timestamp = time::normalize_for_write(driver, timestamp);
            sql.push(", time_stamp = ")
                .bind(time::format(driver, &timestamp))
                .cast("timestamptz");
        }
        sql.push(" WHERE item_id = ").bind(item_id);
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `GetItems`: up to `n` items after the cursor, in id order, with the
    /// cursor for the next page, which is empty at the end.
    pub(super) async fn get_items(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let after = decode_cursor(cursor)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE TRUE",
            columns(driver, ""),
            self.items_table()
        ));
        if !after.is_empty() {
            sql.push(" AND item_id >= ").bind(after);
        }
        if let Some(begin_time) = &begin_time {
            sql.push(" AND time_stamp >= ")
                .bind(time::format(driver, begin_time))
                .cast("timestamptz");
        }
        sql.push(" ORDER BY item_id LIMIT ").bind(n + 1);
        let mut items = self.fetch_items_all(&sql, false).await?;
        if items.len() as i64 == n + 1 {
            let next = items.pop().map(|item| item.item_id).unwrap_or_default();
            return Ok((encode_cursor(&next), items));
        }
        Ok((String::new(), items))
    }

    /// Go's `GetLatestItems`: up to `n` visible items, newest first, that have
    /// every category in `categories` and a time stamp after `after`.
    pub(super) async fn get_latest_items(
        &self,
        ctx: &Context,
        n: i64,
        categories: &[String],
        after: Option<Time>,
    ) -> Result<Vec<Item>, Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE is_hidden = ",
            columns(driver, ""),
            self.items_table()
        ))
        .bind(false);
        if !categories.is_empty() {
            push_categories_filter(&mut sql, driver, "categories", categories)?;
        }
        if let Some(after) = &after {
            sql.push(" AND time_stamp > ")
                .bind(time::format(driver, after))
                .cast("timestamptz");
        }
        sql.push(" ORDER BY time_stamp DESC LIMIT ").bind(n);
        self.fetch_items_all(&sql, false).await
    }

    /// Go's `GetItemStream`: every item, in batches of `batch_size`, skipping
    /// those stamped after `time_limit`.
    pub(super) fn get_item_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>> {
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            columns(driver, ""),
            self.items_table()
        ));
        if let Some(time_limit) = &time_limit {
            sql.push(" WHERE time_stamp >= ")
                .bind(time::format(driver, time_limit))
                .cast("timestamptz");
        }
        let batch = usize::try_from(batch_size).unwrap_or(0);
        match &self.pools {
            Pools::MySql(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, item_from_row)
            }
            Pools::Postgres(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, item_from_row)
            }
            Pools::Sqlite(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, item_from_row)
            }
        }
    }

    /// Runs an item query, decoding its rows as whole items or as ids.
    async fn fetch_items_all(&self, sql: &Sql, return_id: bool) -> Result<Vec<Item>, Error> {
        match (&self.pools, return_id) {
            (Pools::MySql(pool), false) => query::fetch_all(pool, sql, item_from_row).await,
            (Pools::Postgres(pool), false) => query::fetch_all(pool, sql, item_from_row).await,
            (Pools::Sqlite(pool), false) => query::fetch_all(pool, sql, item_from_row).await,
            (Pools::MySql(pool), true) => query::fetch_all(pool, sql, item_id_from_row).await,
            (Pools::Postgres(pool), true) => query::fetch_all(pool, sql, item_id_from_row).await,
            (Pools::Sqlite(pool), true) => query::fetch_all(pool, sql, item_id_from_row).await,
        }
    }

    /// Runs a query for one item and decodes it, or `None` when there is none.
    async fn fetch_optional_item(&self, sql: &Sql) -> Result<Option<Item>, Error> {
        match &self.pools {
            Pools::MySql(pool) => query::fetch_optional(pool, sql, item_from_row).await,
            Pools::Postgres(pool) => query::fetch_optional(pool, sql, item_from_row).await,
            Pools::Sqlite(pool) => query::fetch_optional(pool, sql, item_from_row).await,
        }
    }
}

/// The text of a JSON document a store writes. JSON is UTF-8, so the bytes
/// convert without loss.
pub(super) fn json_text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The `ON CONFLICT` (or `ON DUPLICATE KEY`) clause that overwrites the columns
/// of a conflicting row with the values it was given, as GORM's
/// `clause.OnConflict` writes it.
pub fn upsert_clause(driver: Driver, key: &str, columns: &[&str]) -> String {
    match driver {
        Driver::MySql => {
            let assignments: Vec<String> = columns
                .iter()
                .map(|column| format!("{column} = VALUES({column})"))
                .collect();
            format!(" ON DUPLICATE KEY UPDATE {}", assignments.join(", "))
        }
        Driver::Postgres | Driver::Sqlite => {
            let assignments: Vec<String> = columns
                .iter()
                .map(|column| format!("{column} = excluded.{column}"))
                .collect();
            format!(
                " ON CONFLICT ({key}) DO UPDATE SET {}",
                assignments.join(", ")
            )
        }
    }
}

/// Go's cursor for an item page: the id, base64 encoded, or nothing.
pub(super) fn decode_cursor(cursor: &str) -> Result<String, Error> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cursor)
        .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
    String::from_utf8(bytes).map_err(|err| Error::Backend(std::sync::Arc::new(err)))
}

/// The inverse of [`decode_cursor`].
pub(super) fn encode_cursor(id: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(id)
}
