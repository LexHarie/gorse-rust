// Ports: storage/data/sql.go (the user methods of SQLDatabase and NewSQLUser) @ add21b5

//! The user methods of the SQL data store. The user table keeps the labels as
//! JSON text, as the item table does, and `update_at` as text; see
//! [`super::items`] and [`super::time`].

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_common::jsonutil;
use sqlx::{ColumnIndex, Decode, Row, Type};

use super::super::Error;
use super::super::types::{Labels, User, UserPatch};
use super::items::{
    decode_cursor, encode_cursor, json_column, json_text, unmarshal, upsert_clause,
};
use super::query::{self, Driver};
use super::{Pools, SqlDatabase, time};

/// The SQL that selects the columns of a user, in the order [`user_from_row`]
/// reads them.
pub fn columns(driver: Driver) -> String {
    format!(
        "user_id, {}, comment, {}",
        json_column(driver, "labels"),
        time::select(driver, "update_at"),
    )
}

/// Decodes a user from the row [`columns`] selects.
pub fn user_from_row<R>(row: &R) -> Result<User, Error>
where
    R: Row,
    usize: ColumnIndex<R>,
    for<'r> String: Decode<'r, R::Database> + Type<R::Database>,
{
    Ok(User {
        user_id: query::text(row, 0)?,
        labels: unmarshal::<Labels>(&query::text(row, 1)?)?,
        comment: query::text(row, 2)?,
        update_at: time::parse(&query::text(row, 3)?)?,
    })
}

impl SqlDatabase {
    /// Go's `BatchInsertUsers`: writes the users, the first of each id, and
    /// overwrites the labels and comment of the ones already stored.
    pub(super) async fn batch_insert_users(
        &self,
        ctx: &Context,
        users: &[User],
    ) -> Result<(), Error> {
        query::check(ctx)?;
        if users.is_empty() {
            return Ok(());
        }
        let driver = self.driver();
        let now = gotime::now().utc();
        let mut seen = std::collections::HashSet::new();
        let mut sql = self.sql();
        sql.push(&format!(
            "INSERT INTO {} (user_id, labels, comment, update_at) VALUES ",
            self.users_table()
        ));
        let mut first = true;
        for user in users {
            if !seen.insert(user.user_id.as_str()) {
                continue;
            }
            if !first {
                sql.push(", ");
            }
            first = false;
            let labels = jsonutil::marshal(&user.labels)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            sql.push("(")
                .bind(user.user_id.as_str())
                .push(", ")
                .bind(json_text(labels))
                .cast("json");
            sql.push(", ").bind(user.comment.as_str());
            sql.push(", ")
                .bind(time::format(driver, &now))
                .cast("timestamptz");
            sql.push(")");
        }
        if first {
            return Ok(());
        }
        sql.push(&upsert_clause(
            driver,
            "user_id",
            &["labels", "comment", "update_at"],
        ));
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `DeleteUser`: deletes the user, then the feedback from that user.
    pub(super) async fn delete_user(&self, ctx: &Context, user_id: &str) -> Result<(), Error> {
        query::check(ctx)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "DELETE FROM {} WHERE user_id = ",
            self.users_table()
        ))
        .bind(user_id);
        self.exec(&sql).await?;
        let mut sql = self.sql();
        sql.push(&format!(
            "DELETE FROM {} WHERE user_id = ",
            self.feedback_table()
        ))
        .bind(user_id);
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `GetUser`: the user, or `ErrNotFound`.
    pub(super) async fn get_user(&self, ctx: &Context, user_id: &str) -> Result<User, Error> {
        query::check(ctx)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {} WHERE user_id = ",
            columns(self.driver()),
            self.users_table()
        ))
        .bind(user_id);
        let user = match &self.pools {
            Pools::MySql(pool) => query::fetch_optional(pool, &sql, user_from_row).await?,
            Pools::Postgres(pool) => query::fetch_optional(pool, &sql, user_from_row).await?,
            Pools::Sqlite(pool) => query::fetch_optional(pool, &sql, user_from_row).await?,
        };
        user.ok_or(Error::Storage(gorse_storage_core::errors::Error::NotFound))
    }

    /// Go's `ModifyUser`: sets the labels and comment that `patch` sets, and
    /// the write time. A patch that sets nothing changes nothing.
    pub(super) async fn modify_user(
        &self,
        ctx: &Context,
        user_id: &str,
        patch: &UserPatch,
    ) -> Result<(), Error> {
        query::check(ctx)?;
        if patch.labels == Labels::Nil && patch.comment.is_none() {
            tracing::debug!("empty user patch");
            return Ok(());
        }
        let driver = self.driver();
        let now = gotime::now().utc();
        let mut sql = self.sql();
        sql.push(&format!("UPDATE {} SET update_at = ", self.users_table()))
            .bind(time::format(driver, &now))
            .cast("timestamptz");
        if let Some(comment) = &patch.comment {
            sql.push(", comment = ").bind(comment.as_str());
        }
        if patch.labels != Labels::Nil {
            let labels = jsonutil::marshal(&patch.labels)
                .map_err(|err| Error::Backend(std::sync::Arc::new(err)))?;
            sql.push(", labels = ").bind(json_text(labels)).cast("json");
        }
        sql.push(" WHERE user_id = ").bind(user_id);
        self.exec(&sql).await?;
        Ok(())
    }

    /// Go's `GetUsers`: up to `n` users after the cursor, in id order, with the
    /// cursor for the next page, which is empty at the end.
    pub(super) async fn get_users(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
    ) -> Result<(String, Vec<User>), Error> {
        query::check(ctx)?;
        let after = decode_cursor(cursor)?;
        let mut sql = self.sql();
        sql.push(&format!(
            "SELECT {} FROM {}",
            columns(self.driver()),
            self.users_table()
        ));
        if !after.is_empty() {
            sql.push(" WHERE user_id >= ").bind(after);
        }
        sql.push(" ORDER BY user_id LIMIT ").bind(n + 1);
        let mut users = match &self.pools {
            Pools::MySql(pool) => query::fetch_all(pool, &sql, user_from_row).await?,
            Pools::Postgres(pool) => query::fetch_all(pool, &sql, user_from_row).await?,
            Pools::Sqlite(pool) => query::fetch_all(pool, &sql, user_from_row).await?,
        };
        if users.len() as i64 == n + 1 {
            let next = users.pop().map(|user| user.user_id).unwrap_or_default();
            return Ok((encode_cursor(&next), users));
        }
        Ok((String::new(), users))
    }
}
