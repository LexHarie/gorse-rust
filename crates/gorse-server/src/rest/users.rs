// Ports: server/rest.go (insertUser, modifyUser, getUser, insertUsers, getUsers,
// deleteUser, Success, UserIterator) @ add21b5

//! The user routes of the REST API.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_storage::cache;
use gorse_storage::data::Error as DataError;
use gorse_storage::data::types::{User, UserPatch};
use serde::Serialize;

use super::RestServer;
use super::quota;
use super::response::{ApiError, Query, ok, parse_int, read_entity, validate_request_labels};

use std::sync::Arc;

/// Go's `Success`: the result of a write, with the number of rows it changed.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Success {
    /// The number of rows affected (`RowAffected`).
    #[serde(rename = "RowAffected")]
    pub row_affected: i64,
}

/// Go's `UserIterator`: one page of users and the cursor for the next page.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct UserIterator {
    /// The cursor for the next page, empty at the end (`Cursor`).
    #[serde(rename = "Cursor")]
    pub cursor: String,
    /// The users of this page (`Users`).
    #[serde(rename = "Users")]
    pub users: Vec<User>,
}

/// Go's `insertUser`: `POST /api/user`.
pub(crate) async fn insert_user(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let mut user: User = read_entity(&body)?;
    validate_request_labels(&mut user.labels)?;
    quota::check_user_size(&rest.config(), &user).map_err(ApiError::TooManyRequests)?;
    quota::check_user_count_limit(&rest, std::slice::from_ref(&user.user_id)).await?;
    let ctx = Context::background();
    rest.data()
        .batch_insert_users(&ctx, std::slice::from_ref(&user))
        .await
        .map_err(ApiError::internal)?;
    rest.cache()
        .set(
            &ctx,
            &[cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, &user.user_id]),
                gotime::now(),
            )],
        )
        .await
        .map_err(ApiError::internal)?;
    ok(&Success { row_affected: 1 })
}

/// Go's `modifyUser`: `PATCH /api/user/{user-id}`. A failure of the timestamp
/// write is not reported, as in Go: the response is empty.
pub(crate) async fn modify_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user_id = super::response::path_param(&params, "user-id").to_owned();
    let mut patch: UserPatch = read_entity(&body)?;
    validate_request_labels(&mut patch.labels)?;
    quota::check_labels_size(&rest.config(), &patch.labels).map_err(ApiError::TooManyRequests)?;
    if let Some(comment) = &patch.comment {
        quota::check_comment_size(&rest.config(), comment).map_err(ApiError::TooManyRequests)?;
    }
    let ctx = Context::background();
    rest.data()
        .modify_user(&ctx, &user_id, &patch)
        .await
        .map_err(ApiError::internal)?;
    let timestamp = cache::time(
        cache::key(&[cache::LAST_MODIFY_USER_TIME, &user_id]),
        gotime::now(),
    );
    if rest.cache().set(&ctx, &[timestamp]).await.is_err() {
        return Ok(empty_ok());
    }
    ok(&Success { row_affected: 1 })
}

/// The response Go writes when a handler returns without writing anything: a
/// 200 with an empty body.
pub(crate) fn empty_ok() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::OK;
    response
}

/// Go's `getUser`: `GET /api/user/{user-id}`.
pub(crate) async fn get_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user_id = super::response::path_param(&params, "user-id");
    let user = rest
        .data()
        .get_user(&Context::background(), user_id)
        .await
        .map_err(|err| not_found_or_internal(&err, &format!("user {user_id}")))?;
    ok(&user)
}

/// Go's `errors.Is(err, storage.ErrNotFound)` check, which picks the 404 for a
/// missing record and the 500 for anything else. The 404 reads `subject not
/// found`, as Go's store wraps `ErrNotFound` with the record's name, for example
/// `item 6 not found`.
pub(crate) fn not_found_or_internal(err: &DataError, subject: &str) -> ApiError {
    if err.kind() == Some(gorse_storage_core::errors::Error::NotFound) {
        ApiError::PageNotFound(format!("{subject} not found"))
    } else {
        ApiError::internal(err)
    }
}

/// Go's `insertUsers`: `POST /api/users`.
pub(crate) async fn insert_users(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let mut users: Vec<User> = read_entity(&body)?;
    let config = rest.config();
    for user in &mut users {
        validate_request_labels(&mut user.labels)?;
        quota::check_user_size(&config, user).map_err(ApiError::TooManyRequests)?;
    }
    let user_ids: Vec<String> = users.iter().map(|user| user.user_id.clone()).collect();
    quota::check_user_count_limit(&rest, &user_ids).await?;
    let ctx = Context::background();
    rest.data()
        .batch_insert_users(&ctx, &users)
        .await
        .map_err(ApiError::internal)?;
    let now = gotime::now();
    let values: Vec<cache::Value> = users
        .iter()
        .map(|user| {
            cache::time(
                cache::key(&[cache::LAST_MODIFY_USER_TIME, &user.user_id]),
                now.clone(),
            )
        })
        .collect();
    rest.cache()
        .set(&ctx, &values)
        .await
        .map_err(ApiError::internal)?;
    ok(&Success {
        row_affected: users.len() as i64,
    })
}

/// Go's `getUsers`: `GET /api/users`.
pub(crate) async fn get_users(
    State(rest): State<Arc<RestServer>>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let n = parse_int(&query, "n", rest.config().server.default_n)?;
    let (cursor, users) = rest
        .data()
        .get_users(&Context::background(), query.get("cursor"), n)
        .await
        .map_err(ApiError::internal)?;
    ok(&UserIterator { cursor, users })
}

/// Go's `deleteUser`: `DELETE /api/user/{user-id}`.
pub(crate) async fn delete_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user_id = super::response::path_param(&params, "user-id");
    rest.data()
        .delete_user(&Context::background(), user_id)
        .await
        .map_err(ApiError::internal)?;
    ok(&Success { row_affected: 1 })
}
