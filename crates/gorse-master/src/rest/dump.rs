// Ports: master/rest.go (`dump`, `restore`, `Restore`, `writeDump`, `readDump`, `DumpStats`) @ add21b5

//! The dump and restore of the whole data store (`master/rest.go`).
//!
//! A dump is a sequence of sections. Each section starts with a little-endian
//! `int64` flag: [`USER_STREAM`], [`ITEM_STREAM`] or [`FEEDBACK_STREAM`]. Its
//! records follow, each a little-endian `int64` length and the protobuf bytes
//! of the record. A length of zero or less ends the section. The dump ends with
//! the flag [`EOF`]. The JSON stats of the dump are appended after it.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_protocol::{Feedback as PbFeedback, Item as PbItem, User as PbUser};
use gorse_server::rest::response::{ApiError, go_json, ok};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User, unmarshal_labels};
use gorse_storage::data::with_end_time;
use prost_types::Timestamp;
use serde::Serialize;

use crate::Master;
use crate::rest::bulk::BATCH_SIZE;
use crate::rest::{auth, method_not_allowed, write_error};

/// The flag that ends the dump (Go's `EOF`).
const EOF: i64 = 0;
/// The flag that starts the user section (Go's `UserStream`).
const USER_STREAM: i64 = -1;
/// The flag that starts the item section (Go's `ItemStream`).
const ITEM_STREAM: i64 = -2;
/// The flag that starts the feedback section (Go's `FeedbackStream`).
const FEEDBACK_STREAM: i64 = -3;

/// The counts of a dump or a restore, and how long it took (Go's `DumpStats`).
#[derive(Debug, Default, Serialize)]
pub(crate) struct DumpStats {
    /// The number of users (`Users`).
    #[serde(rename = "Users")]
    pub users: i64,
    /// The number of items (`Items`).
    #[serde(rename = "Items")]
    pub items: i64,
    /// The number of feedback (`Feedback`).
    #[serde(rename = "Feedback")]
    pub feedback: i64,
    /// The time taken, in nanoseconds, as Go's `time.Duration` marshals (`Duration`).
    #[serde(rename = "Duration")]
    pub duration: i64,
}

/// Go's `timestamppb.New`: the seconds and nanoseconds of `t`.
fn time_to_pb(t: &Time) -> Timestamp {
    Timestamp {
        seconds: t.unix(),
        nanos: t.nanosecond() as i32,
    }
}

/// Go's `Timestamp.AsTime`: a missing time stamp reads as the Unix epoch.
fn time_from_pb(timestamp: Option<&Timestamp>) -> Time {
    let (seconds, nanos) = timestamp.map_or((0, 0), |t| (t.seconds, t.nanos));
    gotime::unix(seconds, i64::from(nanos)).utc()
}

/// Appends `flag` as a little-endian `int64`.
fn put_flag(out: &mut Vec<u8>, flag: i64) {
    out.extend_from_slice(&flag.to_le_bytes());
}

/// Go's `writeDump`: the length of the encoded record, then the record.
fn put_record<M: prost::Message>(out: &mut Vec<u8>, record: &M) {
    let bytes = record.encode_to_vec();
    put_flag(out, bytes.len() as i64);
    out.extend_from_slice(&bytes);
}

/// Reads a dump from memory, the way Go's `Restore` reads its `io.Reader`. The
/// errors carry the texts Go's `binary` and `io` packages give.
struct Reader<'a> {
    buf: &'a [u8],
}

impl Reader<'_> {
    /// Go's `binary.Read` of an `int64`: `EOF` when nothing is left, and
    /// `unexpected EOF` when the bytes run out inside the value.
    fn flag(&mut self) -> Result<i64, String> {
        if self.buf.len() < 8 {
            return Err(if self.buf.is_empty() {
                "EOF"
            } else {
                "unexpected EOF"
            }
            .to_owned());
        }
        let (head, rest) = self.buf.split_at(8);
        self.buf = rest;
        let mut bytes = [0; 8];
        bytes.copy_from_slice(head);
        Ok(i64::from_le_bytes(bytes))
    }

    /// Go's `readDump`: the length of the next record, and the record when the
    /// length is positive. A length of zero or less returns an empty record.
    fn record<M: prost::Message + Default>(&mut self) -> Result<(i64, M), String> {
        let size = self.flag()?;
        if size <= 0 {
            return Ok((size, M::default()));
        }
        let size = usize::try_from(size).map_err(|err| err.to_string())?;
        if self.buf.len() < size {
            return Err("unexpected EOF".to_owned());
        }
        let (bytes, rest) = self.buf.split_at(size);
        self.buf = rest;
        let record = M::decode(bytes).map_err(|err| err.to_string())?;
        Ok((size as i64, record))
    }
}

/// Writes the whole dump into memory and counts its records (Go's `dump` loop).
async fn write_dump(master: &Master) -> Result<(Vec<u8>, DumpStats), ApiError> {
    let ctx = Context::background();
    let data = master.data();
    let mut out = Vec::new();
    let mut stats = DumpStats::default();

    put_flag(&mut out, USER_STREAM);
    let mut users = data.get_user_stream(&ctx, BATCH_SIZE as i64);
    while let Some(batch) = users.next().await {
        for user in batch.map_err(ApiError::internal)? {
            let labels = serde_json::to_vec(&user.labels).map_err(ApiError::internal)?;
            put_record(
                &mut out,
                &PbUser {
                    user_id: user.user_id,
                    labels,
                    comment: user.comment,
                    ..Default::default()
                },
            );
            stats.users += 1;
        }
    }

    put_flag(&mut out, ITEM_STREAM);
    let mut items = data.get_item_stream(&ctx, BATCH_SIZE as i64, None);
    while let Some(batch) = items.next().await {
        for item in batch.map_err(ApiError::internal)? {
            let labels = serde_json::to_vec(&item.labels).map_err(ApiError::internal)?;
            put_record(
                &mut out,
                &PbItem {
                    item_id: item.item_id,
                    is_hidden: item.is_hidden,
                    categories: item.categories.unwrap_or_default(),
                    timestamp: Some(time_to_pb(&item.timestamp)),
                    labels,
                    comment: item.comment,
                    ..Default::default()
                },
            );
            stats.items += 1;
        }
    }

    put_flag(&mut out, FEEDBACK_STREAM);
    let end_time = master.config().now();
    let mut feedback =
        data.get_feedback_stream(&ctx, BATCH_SIZE as i64, &[with_end_time(end_time)]);
    while let Some(batch) = feedback.next().await {
        for record in batch.map_err(ApiError::internal)? {
            let labels = serde_json::to_vec(&record.labels).map_err(ApiError::internal)?;
            put_record(
                &mut out,
                &PbFeedback {
                    feedback_type: record.feedback_key.feedback_type,
                    user_id: record.feedback_key.user_id,
                    item_id: record.feedback_key.item_id,
                    value: record.value,
                    timestamp: Some(time_to_pb(&record.timestamp)),
                    labels,
                    comment: record.comment,
                    updated: Some(time_to_pb(&record.updated)),
                    ..Default::default()
                },
            );
            stats.feedback += 1;
        }
    }

    put_flag(&mut out, EOF);
    Ok((out, stats))
}

/// `ANY /api/dump`: writes the whole data store as a binary dump with `GET`.
/// The stats are appended after the dump, as Go's `server.Ok` appends them.
pub(crate) async fn dump(State(master): State<Arc<Master>>, request: Request) -> Response {
    if !auth::check_login(&master, request.headers()) {
        return write_error(axum::http::StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if *request.method() != Method::GET {
        return method_not_allowed();
    }
    let start = Instant::now();
    let (mut body, mut stats) = match write_dump(&master).await {
        Ok(dumped) => dumped,
        Err(err) => return err.into_response(),
    };
    stats.duration = i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX);
    match go_json(&stats) {
        Ok(json) => body.extend(json),
        Err(err) => return ApiError::internal(err).into_response(),
    }
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
}

/// Go's `Restore`: reads a dump and writes its records in batches. `delta`
/// moves the feedback time stamps, and the handler passes none.
pub(crate) async fn restore_dump(
    master: &Master,
    body: &[u8],
    delta: Option<gotime::Duration>,
) -> Result<DumpStats, ApiError> {
    let ctx = Context::background();
    let data = master.data();
    let mut reader = Reader { buf: body };
    let mut stats = DumpStats::default();
    let mut flag = reader.flag().map_err(ApiError::internal)?;
    while flag != EOF {
        match flag {
            USER_STREAM => {
                let mut batch: Vec<User> = Vec::with_capacity(BATCH_SIZE);
                loop {
                    let (size, record) = reader.record::<PbUser>().map_err(ApiError::internal)?;
                    flag = size;
                    if flag <= 0 {
                        break;
                    }
                    let labels = unmarshal_labels(&record.labels).map_err(ApiError::internal)?;
                    batch.push(User {
                        user_id: record.user_id,
                        labels,
                        comment: record.comment,
                        ..Default::default()
                    });
                    stats.users += 1;
                    if batch.len() == BATCH_SIZE {
                        data.batch_insert_users(&ctx, &batch)
                            .await
                            .map_err(ApiError::internal)?;
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    data.batch_insert_users(&ctx, &batch)
                        .await
                        .map_err(ApiError::internal)?;
                }
            }
            ITEM_STREAM => {
                let mut batch: Vec<Item> = Vec::with_capacity(BATCH_SIZE);
                loop {
                    let (size, record) = reader.record::<PbItem>().map_err(ApiError::internal)?;
                    flag = size;
                    if flag <= 0 {
                        break;
                    }
                    let labels = unmarshal_labels(&record.labels).map_err(ApiError::internal)?;
                    batch.push(Item {
                        item_id: record.item_id,
                        is_hidden: record.is_hidden,
                        categories: if record.categories.is_empty() {
                            None
                        } else {
                            Some(record.categories)
                        },
                        timestamp: time_from_pb(record.timestamp.as_ref()),
                        labels,
                        comment: record.comment,
                        ..Default::default()
                    });
                    stats.items += 1;
                    if batch.len() == BATCH_SIZE {
                        data.batch_insert_items(&ctx, &batch)
                            .await
                            .map_err(ApiError::internal)?;
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    data.batch_insert_items(&ctx, &batch)
                        .await
                        .map_err(ApiError::internal)?;
                }
            }
            FEEDBACK_STREAM => {
                let mut batch: Vec<Feedback> = Vec::with_capacity(BATCH_SIZE);
                loop {
                    let (size, record) =
                        reader.record::<PbFeedback>().map_err(ApiError::internal)?;
                    flag = size;
                    if flag <= 0 {
                        break;
                    }
                    let mut timestamp = time_from_pb(record.timestamp.as_ref());
                    if let Some(delta) = delta {
                        timestamp = timestamp.add(delta);
                    }
                    // Go decodes the labels only when the record has some.
                    let labels = if record.labels.is_empty() {
                        Default::default()
                    } else {
                        unmarshal_labels(&record.labels).map_err(ApiError::internal)?
                    };
                    batch.push(Feedback {
                        feedback_key: FeedbackKey {
                            feedback_type: record.feedback_type,
                            user_id: record.user_id,
                            item_id: record.item_id,
                        },
                        value: record.value,
                        timestamp,
                        labels,
                        comment: record.comment,
                        ..Default::default()
                    });
                    stats.feedback += 1;
                    if batch.len() == BATCH_SIZE {
                        data.batch_insert_feedback(&ctx, &batch, false, false, true)
                            .await
                            .map_err(ApiError::internal)?;
                        batch.clear();
                    }
                }
                if !batch.is_empty() {
                    data.batch_insert_feedback(&ctx, &batch, false, false, true)
                        .await
                        .map_err(ApiError::internal)?;
                }
            }
            other => {
                return Err(ApiError::internal(format!("unknown flag {other}")));
            }
        }
    }
    Ok(stats)
}

/// `ANY /api/restore`: reads a dump from the request body with `POST`, then
/// cancels the running task and asks the task loop to run again.
pub(crate) async fn restore(State(master): State<Arc<Master>>, request: Request) -> Response {
    if !auth::check_login(&master, request.headers()) {
        return write_error(axum::http::StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if *request.method() != Method::POST {
        return method_not_allowed();
    }
    let start = Instant::now();
    let body = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(err) => return ApiError::internal(err).into_response(),
    };
    let mut stats = match restore_dump(&master, &body, None).await {
        Ok(stats) => stats,
        Err(err) => return err.into_response(),
    };
    master.cancel_task();
    let _ = master.scheduled.try_send(());
    stats.duration = i64::try_from(start.elapsed().as_nanos()).unwrap_or(i64::MAX);
    ok(&stats).unwrap_or_else(IntoResponse::into_response)
}
