// Ports: storage/data/sql.go (the feedback methods of SQLDatabase and the
// FeedbackTypeExpressionToSQL helper) @ add21b5

//! The feedback methods of the SQL data store.
//!
//! A feedback row is keyed by feedback type, user and item. Writing a feedback
//! that already exists either replaces it (`overwrite`) or accumulates into
//! it: the value adds up, the time stamp keeps the earliest and the update
//! time the latest. The accumulation is written per dialect, as Go's
//! `clause.OnConflict` writes it.

use std::collections::HashSet;

use futures_core::stream::BoxStream;
use gorse_common::expression::{ExprType, FeedbackTypeExpression};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::jsonutil;
use sqlx::{ColumnIndex, Decode, Row, Type};

use super::super::types::{Feedback, FeedbackKey, Labels};
use super::super::{Error, ScanOption, new_scan_options};
use super::items::{
    decode_cursor, encode_cursor, json_column, unmarshal, unmarshal_labels, upsert_clause,
};
use super::query::{self, Driver, Sql};
use super::{Pools, SqlDatabase, time};

/// The columns a feedback query selects, in the order [`feedback_from_row`]
/// reads them.
pub fn columns(driver: Driver) -> String {
    format!(
        "feedback_type, user_id, item_id, value, {}, {}, {}, comment",
        time::select(driver, "time_stamp"),
        time::select(driver, "updated"),
        json_column(driver, "labels"),
    )
}

/// Decodes a feedback record from the row [`columns`] selects. Go's nil labels
/// are `NULL` in the column, which decodes to [`Labels::Nil`].
pub fn feedback_from_row<R>(row: &R) -> Result<Feedback, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> f64: Decode<'r, R::Database> + Type<R::Database>,
    for<'r> Option<String>: Decode<'r, R::Database> + Type<R::Database>,
{
    let labels = match query::optional_text(row, 6)? {
        Some(text) => unmarshal_labels(&text)?,
        None => Labels::Nil,
    };
    Ok(Feedback {
        feedback_key: FeedbackKey {
            feedback_type: query::text(row, 0)?,
            user_id: query::text(row, 1)?,
            item_id: query::text(row, 2)?,
        },
        value: query::float(row, 3)?,
        timestamp: time::parse(&query::text(row, 4)?)?,
        updated: time::parse(&query::text(row, 5)?)?,
        labels,
        comment: query::text(row, 7)?,
    })
}

/// Starts the `WHERE` of a query, or continues it with `AND`.
fn and(sql: &mut Sql, first: &mut bool) {
    sql.push(if *first { " WHERE " } else { " AND " });
    *first = false;
}

/// Go's `FeedbackTypeExpressionToSQL`, for every expression: an `OR` of the
/// conditions, each parenthesized so it binds as one group. A feedback type
/// with no comparison matches on its type alone.
fn push_expressions(sql: &mut Sql, expressions: &[FeedbackTypeExpression]) {
    sql.push("(");
    for (i, expression) in expressions.iter().enumerate() {
        if i > 0 {
            sql.push(" OR ");
        }
        let operator = match expression.expr_type {
            ExprType::Less => Some("<"),
            ExprType::LessOrEqual => Some("<="),
            ExprType::Greater => Some(">"),
            ExprType::GreaterOrEqual => Some(">="),
            _ => None,
        };
        match operator {
            Some(operator) => {
                sql.push("(feedback_type = ")
                    .bind(expression.feedback_type.as_str())
                    .push(&format!(" AND value {operator} "))
                    .bind(expression.value)
                    .push(")");
            }
            None => {
                sql.push("feedback_type = ")
                    .bind(expression.feedback_type.as_str());
            }
        }
    }
    sql.push(")");
}

/// `(feedback_type = ? OR feedback_type = ? ...)` for the types given, which
/// is how Go's `db.Or` chains combine.
fn push_feedback_types(sql: &mut Sql, feedback_types: &[String]) {
    sql.push("(");
    for (i, feedback_type) in feedback_types.iter().enumerate() {
        if i > 0 {
            sql.push(" OR ");
        }
        sql.push("feedback_type = ").bind(feedback_type.as_str());
    }
    sql.push(")");
}

/// Go's `DO NOTHING` upsert: a row that exists is left as it is.
fn do_nothing_clause(driver: Driver, key: &str) -> String {
    match driver {
        Driver::MySql => format!(" ON DUPLICATE KEY UPDATE {key} = {key}"),
        Driver::Postgres | Driver::Sqlite => format!(" ON CONFLICT ({key}) DO NOTHING"),
    }
}

/// The clause of a feedback insert that accumulates into an existing record:
/// the value adds up, the time stamp keeps the earliest, the update time the
/// latest, and the labels and comment are replaced.
fn accumulate_clause(driver: Driver, table: &str) -> String {
    match driver {
        Driver::MySql => " ON DUPLICATE KEY UPDATE value = value + VALUES(value), time_stamp = LEAST(time_stamp, VALUES(time_stamp)), updated = GREATEST(updated, VALUES(updated)), labels = VALUES(labels), comment = VALUES(comment)".to_owned(),
        Driver::Postgres => format!(" ON CONFLICT (feedback_type, user_id, item_id) DO UPDATE SET value = {table}.value + EXCLUDED.value, time_stamp = LEAST({table}.time_stamp, EXCLUDED.time_stamp), updated = GREATEST({table}.updated, EXCLUDED.updated), labels = EXCLUDED.labels, comment = EXCLUDED.comment"),
        Driver::Sqlite => " ON CONFLICT (feedback_type, user_id, item_id) DO UPDATE SET value = value + excluded.value, time_stamp = MIN(time_stamp, excluded.time_stamp), updated = MAX(updated, excluded.updated), labels = excluded.labels, comment = excluded.comment".to_owned(),
    }
}

/// The conflict clause of a feedback insert: a replacement when `overwrite` is
/// set, and an accumulation otherwise.
fn conflict_clause(driver: Driver, table: &str, overwrite: bool) -> String {
    if overwrite {
        upsert_clause(
            driver,
            "feedback_type, user_id, item_id",
            &["time_stamp", "updated", "labels", "comment", "value"],
        )
    } else {
        accumulate_clause(driver, table)
    }
}

impl SqlDatabase {
    /// Go's `GetItemFeedback`: the feedback on an item, of the types given when
    /// any are given, from the feedback whose time stamp has passed.
    pub(super) async fn get_item_feedback(
        &self,
        ctx: &Context,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let now = match driver {
            Driver::Sqlite => "DATETIME()",
            Driver::MySql | Driver::Postgres => "NOW()",
        };
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE time_stamp <= {now} AND item_id = ",
            columns(driver),
            self.feedback_table()
        ))
        .bind(item_id);
        if !feedback_types.is_empty() {
            sql.push(" AND ");
            push_feedback_types(&mut sql, feedback_types);
        }
        self.fetch_feedback(&sql).await
    }

    /// Go's `GetUserFeedback`: the feedback from a user up to `end_time`, of the
    /// types that match `feedback_types` when any are given.
    pub(super) async fn get_user_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        end_time: Option<Time>,
        feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            columns(driver),
            self.feedback_table()
        ));
        let mut first = true;
        if let Some(end_time) = &end_time {
            and(&mut sql, &mut first);
            sql.push("time_stamp <= ")
                .bind(time::format(driver, end_time))
                .cast("timestamptz");
        }
        and(&mut sql, &mut first);
        sql.push("user_id = ").bind(user_id);
        if !feedback_types.is_empty() {
            sql.push(" AND ");
            push_expressions(&mut sql, feedback_types);
        }
        self.fetch_feedback(&sql).await
    }

    /// Go's `GetUserItemFeedback`: the feedback from a user on an item, of the
    /// types given when any are given.
    pub(super) async fn get_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        query::check(ctx)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE user_id = ",
            columns(self.driver()),
            self.feedback_table()
        ))
        .bind(user_id)
        .push(" AND item_id = ")
        .bind(item_id);
        if !feedback_types.is_empty() {
            sql.push(" AND ");
            push_feedback_types(&mut sql, feedback_types);
        }
        self.fetch_feedback(&sql).await
    }

    /// Go's `DeleteUserItemFeedback`: deletes the feedback from a user on an
    /// item, of the types given when any are given, and returns how many
    /// records it deleted.
    pub(super) async fn delete_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<i64, Error> {
        query::check(ctx)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "DELETE FROM {} WHERE user_id = ",
            self.feedback_table()
        ))
        .bind(user_id)
        .push(" AND item_id = ")
        .bind(item_id);
        if !feedback_types.is_empty() {
            sql.push(" AND feedback_type IN (");
            for (i, feedback_type) in feedback_types.iter().enumerate() {
                if i > 0 {
                    sql.push(",");
                }
                sql.bind(feedback_type.as_str());
            }
            sql.push(")");
        }
        let deleted = self.exec(&sql).await?;
        Ok(deleted as i64)
    }

    /// Go's `BatchInsertFeedback`. The users and items the feedback names are
    /// added when `insert_user` and `insert_item` are set, and otherwise only
    /// the ones that exist are kept. Each feedback key is written once.
    pub(super) async fn batch_insert_feedback(
        &self,
        ctx: &Context,
        feedback: &[Feedback],
        insert_user: bool,
        insert_item: bool,
        overwrite: bool,
    ) -> Result<(), Error> {
        query::check(ctx)?;
        if feedback.is_empty() {
            return Ok(());
        }
        let driver = self.driver();
        let mut users = HashSet::new();
        let mut items = HashSet::new();
        for f in feedback {
            users.insert(f.feedback_key.user_id.as_str());
            items.insert(f.feedback_key.item_id.as_str());
        }
        let users = self
            .retain_known_ids(ctx, &users, insert_user, InsertKind::User)
            .await?;
        let items = self
            .retain_known_ids(ctx, &items, insert_item, InsertKind::Item)
            .await?;

        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for f in feedback {
            let key = &f.feedback_key;
            if users.contains(key.user_id.as_str())
                && items.contains(key.item_id.as_str())
                && seen.insert((
                    key.feedback_type.as_str(),
                    key.user_id.as_str(),
                    key.item_id.as_str(),
                ))
            {
                let mut row = f.clone();
                row.timestamp = time::normalize_for_write(driver, &f.timestamp);
                row.updated = row.timestamp.clone();
                rows.push(row);
            }
        }
        if rows.is_empty() {
            return Ok(());
        }

        let table = self.feedback_table();
        let mut sql = self.sql();
        sql.push(&format!(
            "INSERT INTO {table} (feedback_type, user_id, item_id, value, time_stamp, updated, labels, comment) VALUES "
        ));
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                sql.push(", ");
            }
            let labels = match &row.labels {
                Labels::Nil => None,
                labels => Some(json_text(
                    jsonutil::marshal(labels)
                        .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?,
                )),
            };
            sql.push("(")
                .bind(row.feedback_key.feedback_type.as_str())
                .push(", ")
                .bind(row.feedback_key.user_id.as_str())
                .push(", ")
                .bind(row.feedback_key.item_id.as_str())
                .push(", ")
                .bind(row.value);
            sql.push(", ")
                .bind(time::format(driver, &row.timestamp))
                .cast("timestamptz");
            sql.push(", ")
                .bind(time::format(driver, &row.updated))
                .cast("timestamptz");
            sql.push(", ").bind(labels).cast("json");
            sql.push(", ").bind(row.comment.as_str()).push(")");
        }
        sql.push(&conflict_clause(driver, &table, overwrite));
        self.exec(&sql).await?;
        Ok(())
    }

    /// The ids of `ids` that are kept for a feedback write: all of them when
    /// `insert` is set, after inserting the missing ones, and otherwise only
    /// the ones that already exist.
    async fn retain_known_ids<'a>(
        &self,
        ctx: &Context,
        ids: &HashSet<&'a str>,
        insert: bool,
        kind: InsertKind,
    ) -> Result<HashSet<&'a str>, Error> {
        if insert {
            self.insert_missing_ids(ctx, ids, kind).await?;
            return Ok(ids.clone());
        }
        let mut kept = HashSet::new();
        for id in ids {
            let mut sql = self.sql();
            sql.push(&format!(
                "SELECT {} FROM {} WHERE {} = ",
                kind.column(),
                kind.table(self),
                kind.column()
            ))
            .bind(*id);
            if self.one_text(&sql).await?.is_some() {
                kept.insert(*id);
            }
        }
        Ok(kept)
    }

    /// Inserts a row for each id that is not stored yet, as Go's `DoNothing`
    /// `Create` does.
    async fn insert_missing_ids(
        &self,
        ctx: &Context,
        ids: &HashSet<&str>,
        kind: InsertKind,
    ) -> Result<(), Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let now = gotime::now().utc();
        let mut sql = self.sql();
        sql.push(&format!("INSERT INTO {} ", kind.table(self)));
        match kind {
            InsertKind::User => sql.push("(user_id, labels, comment, update_at) VALUES "),
            InsertKind::Item => sql.push(
                "(item_id, is_hidden, categories, time_stamp, labels, comment, update_at) VALUES ",
            ),
        };
        for (i, id) in ids.iter().enumerate() {
            if i > 0 {
                sql.push(", ");
            }
            match kind {
                InsertKind::User => {
                    sql.push("(")
                        .bind(*id)
                        .push(", ")
                        .bind("null")
                        .cast("json")
                        .push(", ")
                        .bind("")
                        .push(", ")
                        .bind(time::format(driver, &now))
                        .cast("timestamptz")
                        .push(")");
                }
                InsertKind::Item => {
                    sql.push("(")
                        .bind(*id)
                        .push(", ")
                        .bind(false)
                        .push(", ")
                        .bind("null")
                        .cast("json")
                        .push(", ")
                        .bind(time::format(driver, &Time::default()))
                        .cast("timestamptz")
                        .push(", ")
                        .bind("null")
                        .cast("json")
                        .push(", ")
                        .bind("")
                        .push(", ")
                        .bind(time::format(driver, &now))
                        .cast("timestamptz")
                        .push(")");
                }
            }
        }
        sql.push(&do_nothing_clause(driver, kind.key()));
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `GetFeedback`: up to `n` feedback records after the cursor, between
    /// the times given and of the types given, with the cursor for the next
    /// page, which is empty at the end.
    pub(super) async fn get_feedback(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
        end_time: Option<Time>,
        feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error> {
        query::check(ctx)?;
        let driver = self.driver();
        let after = decode_cursor(cursor)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            columns(driver),
            self.feedback_table()
        ));
        let mut first = true;
        if !after.is_empty() {
            let key: FeedbackKey = unmarshal(&after)?;
            and(&mut sql, &mut first);
            sql.push("(feedback_type, user_id, item_id) >= (")
                .bind(key.feedback_type.as_str())
                .push(",")
                .bind(key.user_id.as_str())
                .push(",")
                .bind(key.item_id.as_str())
                .push(")");
        }
        if !feedback_types.is_empty() {
            and(&mut sql, &mut first);
            push_feedback_types(&mut sql, feedback_types);
        }
        if let Some(begin_time) = &begin_time {
            and(&mut sql, &mut first);
            sql.push("time_stamp >= ")
                .bind(time::format(driver, begin_time))
                .cast("timestamptz");
        }
        if let Some(end_time) = &end_time {
            and(&mut sql, &mut first);
            sql.push("time_stamp <= ")
                .bind(time::format(driver, end_time))
                .cast("timestamptz");
        }
        sql.push(" ORDER BY feedback_type, user_id, item_id LIMIT ")
            .bind(n + 1);
        let mut feedback = self.fetch_feedback(&sql).await?;
        if feedback.len() as i64 == n + 1 {
            let last = feedback.pop().map(|f| f.feedback_key).unwrap_or_default();
            let next =
                jsonutil::marshal(&last).map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            let next = String::from_utf8_lossy(&next).into_owned();
            return Ok((encode_cursor(&next), feedback));
        }
        Ok((String::new(), feedback))
    }

    /// Go's `GetFeedbackStream`: the feedback that `options` select, in batches.
    pub(super) fn get_feedback_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>> {
        let scan = new_scan_options(options);
        let driver = self.driver();
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            columns(driver),
            self.feedback_table()
        ));
        let mut first = true;
        if !scan.feedback_types.is_empty() {
            and(&mut sql, &mut first);
            push_expressions(&mut sql, &scan.feedback_types);
        }
        if let Some(begin_time) = &scan.begin_time {
            and(&mut sql, &mut first);
            sql.push("time_stamp >= ")
                .bind(time::format(driver, begin_time))
                .cast("timestamptz");
        }
        if let Some(end_time) = &scan.end_time {
            and(&mut sql, &mut first);
            sql.push("time_stamp <= ")
                .bind(time::format(driver, end_time))
                .cast("timestamptz");
        }
        let bounds = [
            ("user_id >= ", &scan.begin_user_id),
            ("user_id <= ", &scan.end_user_id),
            ("item_id >= ", &scan.begin_item_id),
            ("item_id <= ", &scan.end_item_id),
        ];
        for (condition, bound) in bounds {
            if let Some(bound) = bound {
                and(&mut sql, &mut first);
                sql.push(condition).bind(bound.as_str());
            }
        }
        if scan.order_by_item_id {
            sql.push(" ORDER BY item_id");
        } else {
            sql.push(" ORDER BY feedback_type, user_id, item_id");
        }
        let batch = usize::try_from(batch_size).unwrap_or(0);
        match &self.pools {
            Pools::MySql(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, feedback_from_row)
            }
            Pools::Postgres(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, feedback_from_row)
            }
            Pools::Sqlite(pool) => {
                query::stream_batches(pool.clone(), sql, ctx.clone(), batch, feedback_from_row)
            }
        }
    }

    /// Runs a feedback query and decodes its rows.
    async fn fetch_feedback(&self, sql: &Sql) -> Result<Vec<Feedback>, Error> {
        match &self.pools {
            Pools::MySql(pool) => query::fetch_all(pool, sql, feedback_from_row).await,
            Pools::Postgres(pool) => query::fetch_all(pool, sql, feedback_from_row).await,
            Pools::Sqlite(pool) => query::fetch_all(pool, sql, feedback_from_row).await,
        }
    }
}

/// Which table an insert from a feedback write goes to.
#[derive(Debug, Clone, Copy)]
enum InsertKind {
    User,
    Item,
}

impl InsertKind {
    /// The key column of the table.
    fn column(self) -> &'static str {
        match self {
            InsertKind::User => "user_id",
            InsertKind::Item => "item_id",
        }
    }

    /// The conflict key, for `DO NOTHING`.
    fn key(self) -> &'static str {
        self.column()
    }

    /// The table, with the store's prefix.
    fn table(self, db: &SqlDatabase) -> String {
        match self {
            InsertKind::User => db.users_table(),
            InsertKind::Item => db.items_table(),
        }
    }
}

/// The text of a JSON document a store writes.
fn json_text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}
