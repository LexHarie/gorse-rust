// Ports: master/rest.go (`importExportUsers`, `importExportItems`, `importExportFeedback`, `purge`, `checkList`) @ add21b5

//! The bulk export and import of users, items and feedback as JSON Lines, and
//! the purge of the stores (`master/rest.go`).
//!
//! Export writes one JSON object per line, with Go's `json.Encoder` escapes.
//! The port builds the whole export before it answers, where Go streams it.
//! Import reads the `file` field of a multipart form, checks each record in
//! Go's order, and inserts the records in batches of [`BATCH_SIZE`].

use std::collections::HashSet;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use gorse_common::dateparse;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_common::util::validate_id;
use gorse_server::rest::feedback::Feedback as ImportFeedback;
use gorse_server::rest::items::Item as ImportItem;
use gorse_server::rest::response::{ApiError, Query, go_json_compact, ok, plain};
use gorse_server::rest::users::Success;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use gorse_storage::data::with_end_time;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Master;
use crate::rest::{auth, method_not_allowed, write_error};

/// The number of records read or written at a time (Go's `batchSize`).
pub(crate) const BATCH_SIZE: usize = 1000;

/// The response of an export: the JSON Lines body, served as an attachment.
fn jsonl_attachment(body: Vec<u8>, filename: &str) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/jsonl"),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment;filename={filename}")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

/// Encodes each record as one line, as Go's `json.Encoder.Encode` does: the
/// compact JSON, its `<`, `>` and `&` escaped, and a newline.
fn json_lines<T: Serialize>(records: &[T]) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    for record in records {
        let line = go_json_compact(record).map_err(ApiError::internal)?;
        out.extend(line);
        out.push(b'\n');
    }
    Ok(out)
}

/// Reads the `file` field of the multipart body. Go's `FormFile` reports a
/// missing field as `http: no such file`.
async fn read_file(request: Request) -> Result<Vec<u8>, ApiError> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?
    {
        if field.name() == Some("file") {
            let bytes = field
                .bytes()
                .await
                .map_err(|err| ApiError::BadRequest(err.to_string()))?;
            return Ok(bytes.to_vec());
        }
    }
    Err(ApiError::BadRequest("http: no such file".to_owned()))
}

/// Decodes the JSON values of a JSON Lines body, one after another, as Go's
/// `json.Decoder` does. Each value is checked by the caller as it is read.
fn decode_values<T: DeserializeOwned>(body: &[u8]) -> Vec<Result<T, ApiError>> {
    serde_json::Deserializer::from_slice(body)
        .into_iter::<T>()
        .map(|value| value.map_err(|err| ApiError::BadRequest(err.to_string())))
        .collect()
}

/// Go's `ParseAny` of a time stamp, with the line-numbered error Go writes.
/// An empty stamp is the zero time.
fn parse_timestamp(text: &str, line: usize) -> Result<Time, ApiError> {
    if text.is_empty() {
        return Ok(Time::default());
    }
    dateparse::parse_any(text, &[]).map_err(|_| {
        ApiError::BadRequest(format!("failed to parse datetime `{text}` at line {line}"))
    })
}

/// Go's `ValidateId`, with the line-numbered error Go writes.
fn check_id(kind: &str, value: &str, line: usize) -> Result<(), ApiError> {
    validate_id(value).map_err(|err| {
        ApiError::BadRequest(format!("invalid {kind} `{value}` at line {line} ({err})"))
    })
}

/// Go's `writeError` with `WriteErrorString` for the login check of the bulk
/// routes: the status and the message, without the CORS header.
fn unauthorized_plain() -> Response {
    plain(StatusCode::UNAUTHORIZED, "unauthorized".to_owned())
}

/// `ANY /api/bulk/users`: exports every user with `GET`, imports users from the
/// `file` field with `POST`.
pub(crate) async fn export_import_users(
    State(master): State<Arc<Master>>,
    request: Request,
) -> Response {
    if !auth::check_login(&master, request.headers()) {
        return unauthorized_plain();
    }
    match *request.method() {
        Method::GET => match export_users(&master).await {
            Ok(body) => jsonl_attachment(body, "users.jsonl"),
            Err(err) => err.into_response(),
        },
        Method::POST => match import_users(&master, request).await {
            Ok(response) => response,
            Err(err) => err.into_response(),
        },
        _ => method_not_allowed(),
    }
}

async fn export_users(master: &Master) -> Result<Vec<u8>, ApiError> {
    let ctx = Context::background();
    let data = master.data();
    let mut stream = data.get_user_stream(&ctx, BATCH_SIZE as i64);
    let mut users: Vec<User> = Vec::new();
    while let Some(batch) = stream.next().await {
        users.extend(batch.map_err(ApiError::internal)?);
    }
    json_lines(&users)
}

async fn import_users(master: &Master, request: Request) -> Result<Response, ApiError> {
    let file = read_file(request).await?;
    let ctx = Context::background();
    let data = master.data();
    let mut line_count = 0usize;
    let mut batch: Vec<User> = Vec::with_capacity(BATCH_SIZE);
    for user in decode_values::<User>(&file) {
        let user = user?;
        check_id("user id", &user.user_id, line_count)?;
        batch.push(user);
        if batch.len() == BATCH_SIZE {
            data.batch_insert_users(&ctx, &batch)
                .await
                .map_err(ApiError::internal)?;
            batch.clear();
        }
        line_count += 1;
    }
    if !batch.is_empty() {
        data.batch_insert_users(&ctx, &batch)
            .await
            .map_err(ApiError::internal)?;
    }
    finish_import(master);
    ok(&Success {
        row_affected: line_count as i64,
    })
}

/// The step every import ends with: cancel the running task and ask the task
/// loop to run again (Go's `m.cancel()` and the `scheduled` send).
fn finish_import(master: &Master) {
    master.cancel_task();
    let _ = master.scheduled.try_send(());
}

/// `ANY /api/bulk/items`: exports every item with `GET`, imports items from the
/// `file` field with `POST`.
pub(crate) async fn export_import_items(
    State(master): State<Arc<Master>>,
    request: Request,
) -> Response {
    if !auth::check_login(&master, request.headers()) {
        return unauthorized_plain();
    }
    match *request.method() {
        Method::GET => match export_items(&master).await {
            Ok(body) => jsonl_attachment(body, "items.jsonl"),
            Err(err) => err.into_response(),
        },
        Method::POST => match import_items(&master, request).await {
            Ok(response) => response,
            Err(err) => err.into_response(),
        },
        _ => method_not_allowed(),
    }
}

async fn export_items(master: &Master) -> Result<Vec<u8>, ApiError> {
    let ctx = Context::background();
    let data = master.data();
    let mut stream = data.get_item_stream(&ctx, BATCH_SIZE as i64, None);
    let mut items: Vec<Item> = Vec::new();
    while let Some(batch) = stream.next().await {
        items.extend(batch.map_err(ApiError::internal)?);
    }
    json_lines(&items)
}

async fn import_items(master: &Master, request: Request) -> Result<Response, ApiError> {
    let file = read_file(request).await?;
    let ctx = Context::background();
    let data = master.data();
    let mut line_count = 0usize;
    let mut batch: Vec<Item> = Vec::with_capacity(BATCH_SIZE);
    for record in decode_values::<ImportItem>(&file) {
        let record = record?;
        check_id("item id", &record.item_id, line_count)?;
        for category in record.categories.iter().flatten() {
            check_id("category", category, line_count)?;
        }
        let timestamp = parse_timestamp(&record.timestamp, line_count)?;
        batch.push(Item {
            item_id: record.item_id,
            is_hidden: record.is_hidden,
            categories: record.categories,
            timestamp,
            labels: record.labels,
            comment: record.comment,
            ..Default::default()
        });
        if batch.len() == BATCH_SIZE {
            data.batch_insert_items(&ctx, &batch)
                .await
                .map_err(ApiError::internal)?;
            batch.clear();
        }
        line_count += 1;
    }
    if !batch.is_empty() {
        data.batch_insert_items(&ctx, &batch)
            .await
            .map_err(ApiError::internal)?;
    }
    finish_import(master);
    ok(&Success {
        row_affected: line_count as i64,
    })
}

/// `ANY /api/bulk/feedback`: exports every feedback with `GET`, imports feedback
/// from the `file` field with `POST`.
pub(crate) async fn export_import_feedback(
    State(master): State<Arc<Master>>,
    request: Request,
) -> Response {
    if !auth::check_login(&master, request.headers()) {
        return write_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match *request.method() {
        Method::GET => match export_feedback(&master).await {
            Ok(body) => jsonl_attachment(body, "feedback.jsonl"),
            Err(err) => err.into_response(),
        },
        Method::POST => match import_feedback(&master, request).await {
            Ok(response) => response,
            Err(err) => err.into_response(),
        },
        _ => method_not_allowed(),
    }
}

async fn export_feedback(master: &Master) -> Result<Vec<u8>, ApiError> {
    let ctx = Context::background();
    let end_time = master.config().now();
    let data = master.data();
    let mut stream = data.get_feedback_stream(&ctx, BATCH_SIZE as i64, &[with_end_time(end_time)]);
    let mut feedback: Vec<Feedback> = Vec::new();
    while let Some(batch) = stream.next().await {
        feedback.extend(batch.map_err(ApiError::internal)?);
    }
    json_lines(&feedback)
}

async fn import_feedback(master: &Master, request: Request) -> Result<Response, ApiError> {
    let file = read_file(request).await?;
    let ctx = Context::background();
    let data = master.data();
    let cfg = master.config();
    let mut line_count = 0usize;
    let mut batch: Vec<Feedback> = Vec::with_capacity(BATCH_SIZE);
    for record in decode_values::<ImportFeedback>(&file) {
        let record = record?;
        check_id("feedback type", &record.feedback_type, line_count)?;
        check_id("user id", &record.user_id, line_count)?;
        check_id("item id", &record.item_id, line_count)?;
        let timestamp = parse_timestamp(&record.timestamp, line_count)?;
        batch.push(Feedback {
            feedback_key: FeedbackKey {
                feedback_type: record.feedback_type,
                user_id: record.user_id,
                item_id: record.item_id,
            },
            value: record.value,
            timestamp,
            comment: record.comment,
            ..Default::default()
        });
        if batch.len() == BATCH_SIZE {
            data.batch_insert_feedback(
                &ctx,
                &batch,
                cfg.server.auto_insert_user,
                cfg.server.auto_insert_item,
                true,
            )
            .await
            .map_err(ApiError::internal)?;
            batch.clear();
        }
        line_count += 1;
    }
    if !batch.is_empty() {
        data.batch_insert_feedback(
            &ctx,
            &batch,
            cfg.server.auto_insert_user,
            cfg.server.auto_insert_item,
            true,
        )
        .await
        .map_err(ApiError::internal)?;
    }
    finish_import(master);
    ok(&Success {
        row_affected: line_count as i64,
    })
}

/// The stores a purge must all be checked for (Go's `checkList`).
const CHECK_LIST: [&str; 4] = [
    "delete_users",
    "delete_items",
    "delete_feedback",
    "delete_cache",
];

/// `ANY /api/purge`: empties the data and cache stores. It takes a `POST` from a
/// logged-in user, needs a dashboard password, and needs the `check_list` form
/// value to name every store in [`CHECK_LIST`].
pub(crate) async fn purge(State(master): State<Arc<Master>>, request: Request) -> Response {
    if *request.method() != Method::POST {
        return method_not_allowed();
    }
    if !auth::check_login(&master, request.headers()) {
        return unauthorized_plain();
    }
    if master.config().master.dashboard_password.is_empty() {
        return write_error(
            StatusCode::UNAUTHORIZED,
            "purge is not allowed without dashboard password",
        );
    }
    let uri = request.uri().clone();
    let body = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(err) => return ApiError::BadRequest(err.to_string()).into_response(),
    };
    let posted = Query::parse(std::str::from_utf8(&body).ok());
    let queried = Query::parse(uri.query());
    let checked = auth::form_value(&posted, &queried, "check_list");
    let checked: HashSet<&str> = checked.split(',').collect();
    let expected: HashSet<&str> = CHECK_LIST.into_iter().collect();
    if checked != expected {
        return write_error(StatusCode::UNAUTHORIZED, "please confirm by checking all");
    }
    let data = master.data();
    if let Err(err) = data.purge().await {
        return write_error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    let cache = master.cache();
    if let Err(err) = cache.purge().await {
        return write_error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    StatusCode::OK.into_response()
}
