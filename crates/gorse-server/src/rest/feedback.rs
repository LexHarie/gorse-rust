// Ports: server/rest.go (Feedback, ToDataFeedback, insertFeedback,
// FeedbackIterator, getFeedback, getTypedFeedback, getUserItemFeedback,
// deleteUserItemFeedback, getTypedUserItemFeedback, deleteTypedUserItemFeedback,
// getTypedFeedbackByUser, getFeedbackByUser, getTypedFeedbackByItem,
// getFeedbackByItem) @ add21b5

//! The feedback routes of the REST API.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use gorse_common::dateparse;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback as DataFeedback, FeedbackKey, Labels};
use serde::{Deserialize, Serialize};

use super::RestServer;
use super::quota;
use super::response::{
    ApiError, GoFields, Query, ok, parse_int, path_param, read_entity, text,
    validate_request_labels,
};
use super::users::Success;

/// Go's `Feedback`: a feedback as the REST API takes it, with its time stamp as
/// text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct Feedback {
    /// The feedback type (`FeedbackType`).
    pub feedback_type: String,
    /// The user (`UserId`).
    pub user_id: String,
    /// The item (`ItemId`).
    pub item_id: String,
    /// The feedback's value (`Value`).
    pub value: f64,
    /// The time stamp, in any format `dateparse` reads (`Timestamp`).
    pub timestamp: String,
    /// The feedback's labels (`Labels`).
    pub labels: Labels,
    /// The feedback's comment (`Comment`).
    pub comment: String,
}

impl GoFields for Feedback {
    const FIELDS: &'static [&'static str] = &[
        "FeedbackType",
        "UserId",
        "ItemId",
        "Value",
        "Timestamp",
        "Labels",
        "Comment",
    ];
}

impl Feedback {
    /// Go's `ToDataFeedback`: the feedback with its time stamp parsed. An empty
    /// time stamp is the zero time.
    ///
    /// # Errors
    ///
    /// Fails with `dateparse`'s message when the time stamp is malformed.
    pub(crate) fn to_data_feedback(&self) -> Result<DataFeedback, ApiError> {
        let timestamp = if self.timestamp.is_empty() {
            Time::default()
        } else {
            dateparse::parse_any(&self.timestamp, &[])
                .map_err(|err| ApiError::BadRequest(err.to_string()))?
        };
        Ok(DataFeedback {
            feedback_key: FeedbackKey {
                feedback_type: self.feedback_type.clone(),
                user_id: self.user_id.clone(),
                item_id: self.item_id.clone(),
            },
            value: self.value,
            timestamp,
            updated: Time::default(),
            labels: self.labels.clone(),
            comment: self.comment.clone(),
        })
    }
}

/// Go's `FeedbackIterator`: one page of feedback and the cursor for the next
/// page.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct FeedbackIterator {
    /// The cursor for the next page, empty at the end (`Cursor`).
    #[serde(rename = "Cursor")]
    pub cursor: String,
    /// The feedback of this page (`Feedback`).
    #[serde(rename = "Feedback")]
    pub feedback: Vec<DataFeedback>,
}

/// Go's `insertFeedback`: `POST /api/feedback` adds to the feedback that exists
/// and `PUT /api/feedback` overwrites it.
async fn insert_feedback(
    rest: &RestServer,
    body: &Bytes,
    overwrite: bool,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let literals: Vec<Feedback> = read_entity(body)?;
    let mut feedback = Vec::with_capacity(literals.len());
    let mut users = BTreeSet::new();
    let mut items = BTreeSet::new();
    for literal in &literals {
        users.insert(literal.user_id.clone());
        items.insert(literal.item_id.clone());
        let mut data_feedback = literal.to_data_feedback()?;
        validate_request_labels(&mut data_feedback.labels)?;
        quota::check_labels_size(&config, &data_feedback.labels)
            .map_err(ApiError::TooManyRequests)?;
        quota::check_comment_size(&config, &data_feedback.comment)
            .map_err(ApiError::TooManyRequests)?;
        feedback.push(data_feedback);
    }
    if config.server.auto_insert_user {
        let user_ids: Vec<String> = users.iter().cloned().collect();
        quota::check_user_count_limit(rest, &user_ids).await?;
    }
    if config.server.auto_insert_item {
        let item_ids: Vec<String> = items.iter().cloned().collect();
        quota::check_item_count_limit(rest, &item_ids).await?;
    }
    rest.data()
        .batch_insert_feedback(
            &ctx,
            &feedback,
            config.server.auto_insert_user,
            config.server.auto_insert_item,
            overwrite,
        )
        .await
        .map_err(ApiError::internal)?;
    let now = gotime::now();
    let mut values: Vec<cache::Value> = Vec::with_capacity(users.len() + items.len());
    for user_id in &users {
        values.push(cache::time(
            cache::key(&[cache::LAST_MODIFY_USER_TIME, user_id]),
            now.clone(),
        ));
    }
    for item_id in &items {
        values.push(cache::time(
            cache::key(&[cache::LAST_MODIFY_ITEM_TIME, item_id]),
            now.clone(),
        ));
    }
    rest.cache()
        .set(&ctx, &values)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(
        num_feedback = feedback.len(),
        "Insert feedback successfully"
    );
    ok(&Success {
        row_affected: feedback.len() as i64,
    })
}

/// `POST /api/feedback`: adds the feedback to what exists.
pub(crate) async fn insert_feedback_overwrite_false(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    insert_feedback(&rest, &body, false).await
}

/// `PUT /api/feedback`: overwrites the feedback that exists.
pub(crate) async fn insert_feedback_overwrite_true(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    insert_feedback(&rest, &body, true).await
}

/// Go's `getFeedback`: `GET /api/feedback`, one page of feedback up to now.
pub(crate) async fn get_feedback(
    State(rest): State<Arc<RestServer>>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let n = parse_int(&query, "n", rest.config().server.default_n)?;
    let now = rest.config().now();
    let (cursor, feedback) = rest
        .data()
        .get_feedback(
            &Context::background(),
            query.get("cursor"),
            n,
            None,
            Some(now),
            &[],
        )
        .await
        .map_err(ApiError::internal)?;
    ok(&FeedbackIterator { cursor, feedback })
}

/// Go's `getTypedFeedback`: `GET /api/feedback/{feedback-type}`.
pub(crate) async fn get_typed_feedback(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let feedback_type = path_param(&params, "feedback-type");
    let n = parse_int(&query, "n", rest.config().server.default_n)?;
    let now = rest.config().now();
    let (cursor, feedback) = rest
        .data()
        .get_feedback(
            &Context::background(),
            query.get("cursor"),
            n,
            None,
            Some(now),
            &[feedback_type.to_owned()],
        )
        .await
        .map_err(ApiError::internal)?;
    ok(&FeedbackIterator { cursor, feedback })
}

/// Go's `getUserItemFeedback`: `GET /api/feedback/{user-id}/{item-id}`.
pub(crate) async fn get_user_item_feedback(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user_id = path_param(&params, "user-id");
    let item_id = path_param(&params, "item-id");
    let feedback = rest
        .data()
        .get_user_item_feedback(&Context::background(), user_id, item_id, &[])
        .await
        .map_err(ApiError::internal)?;
    ok(&feedback)
}

/// Go's `deleteUserItemFeedback`: `DELETE /api/feedback/{user-id}/{item-id}`.
pub(crate) async fn delete_user_item_feedback(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user_id = path_param(&params, "user-id");
    let item_id = path_param(&params, "item-id");
    let count = rest
        .data()
        .delete_user_item_feedback(&Context::background(), user_id, item_id, &[])
        .await
        .map_err(ApiError::internal)?;
    ok(&Success {
        row_affected: count,
    })
}

/// Go's `getTypedUserItemFeedback`: `GET /api/feedback/{feedback-type}/{user-id}/{item-id}`.
/// The first matching feedback is written, or the text `{}` when there is none.
pub(crate) async fn get_typed_user_item_feedback(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let feedback_type = path_param(&params, "feedback-type");
    let user_id = path_param(&params, "user-id");
    let item_id = path_param(&params, "item-id");
    let feedback = rest
        .data()
        .get_user_item_feedback(
            &Context::background(),
            user_id,
            item_id,
            &[feedback_type.to_owned()],
        )
        .await
        .map_err(ApiError::internal)?;
    match feedback.first() {
        None => Ok(text("{}")),
        Some(first) => ok(first),
    }
}

/// Go's `deleteTypedUserItemFeedback`:
/// `DELETE /api/feedback/{feedback-type}/{user-id}/{item-id}`.
pub(crate) async fn delete_typed_user_item_feedback(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let feedback_type = path_param(&params, "feedback-type");
    let user_id = path_param(&params, "user-id");
    let item_id = path_param(&params, "item-id");
    let count = rest
        .data()
        .delete_user_item_feedback(
            &Context::background(),
            user_id,
            item_id,
            &[feedback_type.to_owned()],
        )
        .await
        .map_err(ApiError::internal)?;
    ok(&Success {
        row_affected: count,
    })
}

/// Go's `getTypedFeedbackByUser`: `GET /api/user/{user-id}/feedback/{feedback-type}`.
/// The type is read as a feedback type expression, so `*` and `!` work.
pub(crate) async fn get_typed_feedback_by_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let feedback_type = path_param(&params, "feedback-type");
    let mut expression = FeedbackTypeExpression::default();
    expression
        .from_string(feedback_type)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let user_id = path_param(&params, "user-id");
    let now = rest.config().now();
    let feedback = rest
        .data()
        .get_user_feedback(&Context::background(), user_id, Some(now), &[expression])
        .await
        .map_err(ApiError::internal)?;
    ok(&feedback)
}

/// Go's `getFeedbackByUser`: `GET /api/user/{user-id}/feedback`.
pub(crate) async fn get_feedback_by_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let user_id = path_param(&params, "user-id");
    let now = rest.config().now();
    let feedback = rest
        .data()
        .get_user_feedback(&Context::background(), user_id, Some(now), &[])
        .await
        .map_err(ApiError::internal)?;
    ok(&feedback)
}

/// Go's `getTypedFeedbackByItem`: `GET /api/item/{item-id}/feedback/{feedback-type}`.
pub(crate) async fn get_typed_feedback_by_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let item_id = path_param(&params, "item-id");
    let feedback_type = path_param(&params, "feedback-type");
    let feedback = rest
        .data()
        .get_item_feedback(&Context::background(), item_id, &[feedback_type.to_owned()])
        .await
        .map_err(ApiError::internal)?;
    ok(&feedback)
}

/// Go's `getFeedbackByItem`: `GET /api/item/{item-id}/feedback/`.
pub(crate) async fn get_feedback_by_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let item_id = path_param(&params, "item-id");
    let feedback = rest
        .data()
        .get_item_feedback(&Context::background(), item_id, &[])
        .await
        .map_err(ApiError::internal)?;
    ok(&feedback)
}
