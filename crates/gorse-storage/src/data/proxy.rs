// Ports: storage/data/proxy.go @ add21b5 (ProxyServer, ProxyClient, and the
// conversions between records and protocol messages)

//! The data store over gRPC (`protocol.DataStore`, `protocol/data_store.proto`).
//! [`ProxyServer`] serves one backend, and [`ProxyClient`] is the store a
//! server or worker uses when `database.data_store` starts with `sqlite://`
//! and the master holds the database.
//!
//! Go's streams are goroutines feeding channels. A server stream here is a
//! task that pushes into a bounded channel, so gRPC's flow control stops the
//! backend as it stops Go's sender. A client stream is the gRPC stream, mapped
//! batch by batch.
//!
//! Contexts do not cross the wire. Go passes the call's context into the
//! backend; here a backend call gets a background context, and a client that
//! goes away drops the call's future, which stops the work the same way.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_config::config::SearchConfig;
use gorse_protocol as pb;
use gorse_protocol::data_store_client::DataStoreClient;
use gorse_protocol::data_store_server::{DataStore, DataStoreServer};
use gorse_storage_core::errors::Error as StorageError;
use prost_types::Timestamp;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use super::types::{Feedback, FeedbackKey, Item, ItemPatch, Labels, ScoredItem, User, UserPatch};
use super::{Database, Error, GetOptions, ScanOption, ScanOptions, new_scan_options};

/// How many batches a server stream holds before the backend waits for the
/// client to read them.
const STREAM_BUFFER: usize = 8;

/// Go's `ProxyServer`: serves one [`Database`] as the `DataStore` gRPC service.
pub struct ProxyServer {
    database: Arc<dyn Database>,
}

impl ProxyServer {
    /// Go's `NewProxyServer`.
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }

    /// The service to add to a tonic server (Go's `RegisterDataStoreServer`).
    pub fn into_service(self) -> DataStoreServer<Self> {
        DataStoreServer::new(self)
    }
}

/// Go's `timestamppb.New`: the seconds and nanoseconds of `t`.
pub(crate) fn time_to_pb(t: &Time) -> Timestamp {
    Timestamp {
        seconds: t.unix(),
        nanos: t.nanosecond() as i32,
    }
}

/// Go's `Timestamp.AsTime`. A missing timestamp reads as the Unix epoch, as Go's
/// nil receiver does.
pub(crate) fn time_from_pb(timestamp: Option<&Timestamp>) -> Time {
    let (seconds, nanos) = timestamp.map_or((0, 0), |t| (t.seconds, t.nanos));
    gotime::unix(seconds, i64::from(nanos)).utc()
}

/// Go's `timeFromPB`: a missing timestamp is the zero time.
pub(crate) fn optional_time_from_pb(timestamp: Option<&Timestamp>) -> Time {
    match timestamp {
        None => Time::default(),
        Some(_) => time_from_pb(timestamp),
    }
}

/// Go's `json.Marshal` of labels.
pub(crate) fn labels_to_pb(labels: &Labels) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(labels)
}

/// Go's `json.Unmarshal` of labels into an `any`. Go rejects empty input, and
/// the error text is Go's.
pub(crate) fn labels_from_pb(bytes: &[u8]) -> Result<Labels, String> {
    if bytes.is_empty() {
        return Err("unexpected end of JSON input".to_owned());
    }
    serde_json::from_slice(bytes).map_err(|err| err.to_string())
}

/// A list of strings that Go holds as a nil slice when it is empty.
pub(crate) fn optional_strings(values: Vec<String>) -> Option<Vec<String>> {
    (!values.is_empty()).then_some(values)
}

/// A gRPC status as Go's `status.Error` prints it: `rpc error: code = <Code>
/// desc = <message>`, with the code named as Go names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RpcError {
    code: tonic::Code,
    message: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rpc error: code = {} desc = {}",
            go_code_name(self.code),
            self.message
        )
    }
}

impl std::error::Error for RpcError {}

/// A status a Go client receives as an error.
pub(crate) fn rpc_error(status: Status) -> RpcError {
    RpcError {
        code: status.code(),
        message: status.message().to_owned(),
    }
}

/// Go's `codes.Code.String()`, which spells `Canceled` with one `l`.
fn go_code_name(code: tonic::Code) -> &'static str {
    use tonic::Code;
    match code {
        Code::Ok => "OK",
        Code::Cancelled => "Canceled",
        Code::Unknown => "Unknown",
        Code::InvalidArgument => "InvalidArgument",
        Code::DeadlineExceeded => "DeadlineExceeded",
        Code::NotFound => "NotFound",
        Code::AlreadyExists => "AlreadyExists",
        Code::PermissionDenied => "PermissionDenied",
        Code::ResourceExhausted => "ResourceExhausted",
        Code::FailedPrecondition => "FailedPrecondition",
        Code::Aborted => "Aborted",
        Code::OutOfRange => "OutOfRange",
        Code::Unimplemented => "Unimplemented",
        Code::Internal => "Internal",
        Code::Unavailable => "Unavailable",
        Code::DataLoss => "DataLoss",
        Code::Unauthenticated => "Unauthenticated",
    }
}

/// The status of a Go handler that returns a plain error: code `Unknown` with
/// the error's text.
pub(crate) fn unknown(err: impl fmt::Display) -> Status {
    Status::unknown(err.to_string())
}

/// The error of a Go client method that Go refuses, as `errors.Errorf` makes it.
pub(crate) fn refused(message: &str) -> Error {
    Error::Backend(Arc::new(std::io::Error::other(message.to_owned())))
}

/// A gRPC failure seen by a client.
fn backend(status: Status) -> Error {
    Error::Backend(Arc::new(rpc_error(status)))
}

fn feedback_to_pb(feedback: &[Feedback]) -> Result<Vec<pb::Feedback>, serde_json::Error> {
    feedback
        .iter()
        .map(|f| {
            Ok(pb::Feedback {
                feedback_type: f.feedback_key.feedback_type.clone(),
                user_id: f.feedback_key.user_id.clone(),
                item_id: f.feedback_key.item_id.clone(),
                value: f.value,
                timestamp: Some(time_to_pb(&f.timestamp)),
                updated: Some(time_to_pb(&f.updated)),
                labels: labels_to_pb(&f.labels)?,
                comment: f.comment.clone(),
                ..Default::default()
            })
        })
        .collect()
}

/// Go's `feedbackFromPB`. Empty labels read as nil here, unlike the other
/// messages (`contracts/grpc.md`).
fn feedback_from_pb(feedback: Vec<pb::Feedback>) -> Result<Vec<Feedback>, String> {
    feedback
        .into_iter()
        .map(|f| {
            let labels = if f.labels.is_empty() {
                Labels::Nil
            } else {
                labels_from_pb(&f.labels)?
            };
            Ok(Feedback {
                feedback_key: FeedbackKey {
                    feedback_type: f.feedback_type,
                    user_id: f.user_id,
                    item_id: f.item_id,
                },
                value: f.value,
                timestamp: time_from_pb(f.timestamp.as_ref()),
                updated: time_from_pb(f.updated.as_ref()),
                labels,
                comment: f.comment,
            })
        })
        .collect()
}

/// A record's item, as the server sends it, with its update time.
fn item_to_pb(item: &Item) -> Result<pb::Item, serde_json::Error> {
    Ok(pb::Item {
        item_id: item.item_id.clone(),
        is_hidden: item.is_hidden,
        categories: item.categories.clone().unwrap_or_default(),
        timestamp: Some(time_to_pb(&item.timestamp)),
        labels: labels_to_pb(&item.labels)?,
        comment: item.comment.clone(),
        update_at: Some(time_to_pb(&item.update_at)),
        ..Default::default()
    })
}

/// Go's `Item` decoding on the server side of an insert: no update time.
fn item_from_pb(item: pb::Item) -> Result<Item, String> {
    Ok(Item {
        item_id: item.item_id,
        is_hidden: item.is_hidden,
        categories: optional_strings(item.categories),
        timestamp: time_from_pb(item.timestamp.as_ref()),
        update_at: Time::default(),
        labels: labels_from_pb(&item.labels)?,
        comment: item.comment,
    })
}

/// The item a client gets back, as Go's client decodes it.
fn item_from_response(item: pb::Item) -> Result<Item, String> {
    Ok(Item {
        item_id: item.item_id,
        is_hidden: item.is_hidden,
        categories: optional_strings(item.categories),
        timestamp: time_from_pb(item.timestamp.as_ref()),
        update_at: optional_time_from_pb(item.update_at.as_ref()),
        labels: labels_from_pb(&item.labels)?,
        comment: item.comment,
    })
}

fn user_to_pb(user: &User) -> Result<pb::User, serde_json::Error> {
    Ok(pb::User {
        user_id: user.user_id.clone(),
        labels: labels_to_pb(&user.labels)?,
        comment: user.comment.clone(),
        update_at: Some(time_to_pb(&user.update_at)),
        ..Default::default()
    })
}

fn user_from_response(user: pb::User) -> Result<User, String> {
    Ok(User {
        user_id: user.user_id,
        labels: labels_from_pb(&user.labels)?,
        comment: user.comment,
        update_at: optional_time_from_pb(user.update_at.as_ref()),
    })
}

fn expressions_to_pb(types: &[FeedbackTypeExpression]) -> Vec<pb::FeedbackTypeExpression> {
    types.iter().map(FeedbackTypeExpression::to_pb).collect()
}

/// Go's `FeedbackTypeExpression.FromPB` applied to a list.
fn expressions_from_pb(types: &[pb::FeedbackTypeExpression]) -> Vec<FeedbackTypeExpression> {
    types
        .iter()
        .map(|t| {
            let mut expression = FeedbackTypeExpression::default();
            expression.from_pb(t);
            expression
        })
        .collect()
}

/// Go's `[]string` of a list of expressions: each feedback type name, in order.
fn feedback_type_names(types: &[pb::FeedbackTypeExpression]) -> Vec<String> {
    types.iter().map(|t| t.feedback_type.clone()).collect()
}

fn strings_to_expressions(types: &[String]) -> Vec<pb::FeedbackTypeExpression> {
    types
        .iter()
        .map(|t| pb::FeedbackTypeExpression {
            feedback_type: t.clone(),
            ..Default::default()
        })
        .collect()
}

fn items_to_pb(items: &[Item]) -> Result<Vec<pb::Item>, serde_json::Error> {
    items.iter().map(item_to_pb).collect()
}

fn scored_item_to_pb(item: &ScoredItem) -> Result<pb::ScoredItem, serde_json::Error> {
    Ok(pb::ScoredItem {
        item: Some(item_to_pb(&item.item)?),
        score: item.score,
    })
}

/// The scan options a server stream starts with, in Go's order.
fn scan_options_from_pb(options: pb::ScanOptions) -> Vec<ScanOption> {
    use super::{
        with_begin_item_id, with_begin_time, with_begin_user_id, with_end_item_id, with_end_time,
        with_end_user_id, with_feedback_types, with_order_by_item_id,
    };
    let mut opts = Vec::new();
    if let Some(t) = &options.begin_time {
        opts.push(with_begin_time(time_from_pb(Some(t))));
    }
    if let Some(t) = &options.end_time {
        opts.push(with_end_time(time_from_pb(Some(t))));
    }
    if !options.feedback_types.is_empty() {
        opts.push(with_feedback_types(expressions_from_pb(
            &options.feedback_types,
        )));
    }
    if let Some(id) = options.begin_user_id {
        opts.push(with_begin_user_id(id));
    }
    if let Some(id) = options.end_user_id {
        opts.push(with_end_user_id(id));
    }
    if let Some(id) = options.begin_item_id {
        opts.push(with_begin_item_id(id));
    }
    if let Some(id) = options.end_item_id {
        opts.push(with_end_item_id(id));
    }
    if options.order_by_item_id {
        opts.push(with_order_by_item_id());
    }
    opts
}

/// The scan options a client sends, from the options of a call (Go's `ScanOptions`).
fn scan_options_to_pb(options: &ScanOptions) -> pb::ScanOptions {
    pb::ScanOptions {
        begin_user_id: options.begin_user_id.clone(),
        end_user_id: options.end_user_id.clone(),
        begin_item_id: options.begin_item_id.clone(),
        end_item_id: options.end_item_id.clone(),
        begin_time: options.begin_time.as_ref().map(time_to_pb),
        end_time: options.end_time.as_ref().map(time_to_pb),
        feedback_types: expressions_to_pb(&options.feedback_types),
        order_by_item_id: options.order_by_item_id,
    }
}

/// The patch a client sends for an item (Go's `ProxyClient.ModifyItem`).
fn item_patch_to_pb(patch: &ItemPatch) -> Result<pb::ItemPatch, serde_json::Error> {
    Ok(pb::ItemPatch {
        is_hidden: patch.is_hidden,
        categories: patch.categories.clone().unwrap_or_default(),
        labels: match patch.labels {
            Labels::Nil => Vec::new(),
            ref labels => labels_to_pb(labels)?,
        },
        timestamp: patch.timestamp.as_ref().map(time_to_pb),
        comment: patch.comment.clone(),
    })
}

/// The user patch a client sends (Go's `ProxyClient.ModifyUser`).
fn user_patch_to_pb(patch: &UserPatch) -> Result<pb::UserPatch, serde_json::Error> {
    Ok(pb::UserPatch {
        labels: match patch.labels {
            Labels::Nil => Vec::new(),
            ref labels => labels_to_pb(labels)?,
        },
        comment: patch.comment.clone(),
        ..Default::default()
    })
}

/// The item patch the server applies (Go's `ProxyServer.ModifyItem`).
fn item_patch_from_pb(patch: pb::ItemPatch) -> Result<ItemPatch, String> {
    Ok(ItemPatch {
        is_hidden: patch.is_hidden,
        categories: optional_strings(patch.categories),
        timestamp: patch.timestamp.as_ref().map(|t| time_from_pb(Some(t))),
        labels: if patch.labels.is_empty() {
            Labels::Nil
        } else {
            labels_from_pb(&patch.labels)?
        },
        comment: patch.comment,
    })
}

fn user_patch_from_pb(patch: pb::UserPatch) -> Result<UserPatch, String> {
    Ok(UserPatch {
        labels: if patch.labels.is_empty() {
            Labels::Nil
        } else {
            labels_from_pb(&patch.labels)?
        },
        comment: patch.comment,
    })
}

/// Go's `ProxyClient.GetItem` on a response: a missing item is `item <id>
/// not found` with the storage sentinel kept.
fn item_result(item_id: &str, response: pb::GetItemResponse) -> Result<Item, Error> {
    match response.item {
        None => Err(not_found(format!("item {item_id} "))),
        Some(item) => item_from_response(item).map_err(backend_text),
    }
}

/// Go's `fmt.Errorf("<prefix>%w", storage.ErrNotFound)`.
pub(crate) fn not_found(prefix: String) -> Error {
    Error::Wrapped {
        kind: StorageError::NotFound,
        message: format!("{prefix}{}", StorageError::NotFound),
    }
}

/// A decoding failure seen by a client, as Go's client returns it.
fn backend_text(text: String) -> Error {
    Error::Backend(Arc::new(std::io::Error::other(text)))
}

/// Go's `ProxyClient.GetUser` on a response.
fn user_result(user_id: &str, response: pb::GetUserResponse) -> Result<User, Error> {
    match response.user {
        None => Err(not_found(format!("user {user_id} "))),
        Some(user) => user_from_response(user).map_err(backend_text),
    }
}

/// Go's `ProxyServer.GetItem` and `GetUser`: a missing record is a response
/// with nothing in it, not an error.
fn missing_is_empty<T>(result: Result<T, Error>) -> Result<Option<T>, Status> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == Some(StorageError::NotFound) => Ok(None),
        Err(err) => Err(unknown(err)),
    }
}

#[tonic::async_trait]
impl DataStore for ProxyServer {
    type GetUserStreamStream = BoxStream<'static, Result<pb::GetUserStreamResponse, Status>>;
    type GetItemStreamStream = BoxStream<'static, Result<pb::GetItemStreamResponse, Status>>;
    type GetFeedbackStreamStream =
        BoxStream<'static, Result<pb::GetFeedbackStreamResponse, Status>>;

    async fn ping(
        &self,
        _request: Request<pb::PingRequest>,
    ) -> Result<Response<pb::PingResponse>, Status> {
        self.database.ping().await.map_err(unknown)?;
        Ok(Response::new(pb::PingResponse {}))
    }

    async fn reconcile(
        &self,
        request: Request<pb::ReconcileRequest>,
    ) -> Result<Response<pb::ReconcileResponse>, Status> {
        let config = SearchConfig {
            columns: optional_strings(request.into_inner().search_columns),
        };
        self.database.reconcile(&config).await.map_err(unknown)?;
        Ok(Response::new(pb::ReconcileResponse {}))
    }

    async fn batch_insert_items(
        &self,
        request: Request<pb::BatchInsertItemsRequest>,
    ) -> Result<Response<pb::BatchInsertItemsResponse>, Status> {
        let items = request
            .into_inner()
            .items
            .into_iter()
            .map(item_from_pb)
            .collect::<Result<Vec<_>, _>>()
            .map_err(Status::unknown)?;
        self.database
            .batch_insert_items(&Context::background(), &items)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::BatchInsertItemsResponse {}))
    }

    async fn batch_get_items(
        &self,
        request: Request<pb::BatchGetItemsRequest>,
    ) -> Result<Response<pb::BatchGetItemsResponse>, Status> {
        let request = request.into_inner();
        let mut opts = GetOptions::default();
        if let Some(get_options) = request.get_options {
            opts.categories = get_options.categories;
            opts.skip_hidden = get_options.skip_hidden;
            opts.return_id = get_options.return_id;
            opts.after = get_options.after.as_ref().map(|t| time_from_pb(Some(t)));
        }
        let items = self
            .database
            .batch_get_items(&Context::background(), &request.item_ids, &opts)
            .await
            .map_err(unknown)?;
        let items = items_to_pb(&items).map_err(unknown)?;
        Ok(Response::new(pb::BatchGetItemsResponse { items }))
    }

    async fn delete_item(
        &self,
        request: Request<pb::DeleteItemRequest>,
    ) -> Result<Response<pb::DeleteItemResponse>, Status> {
        self.database
            .delete_item(&Context::background(), &request.into_inner().item_id)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteItemResponse {}))
    }

    async fn get_item(
        &self,
        request: Request<pb::GetItemRequest>,
    ) -> Result<Response<pb::GetItemResponse>, Status> {
        let item_id = request.into_inner().item_id;
        let item = self
            .database
            .get_item(&Context::background(), &item_id)
            .await;
        let item = missing_is_empty(item)?
            .map(|item| item_to_pb(&item))
            .transpose()
            .map_err(unknown)?;
        Ok(Response::new(pb::GetItemResponse { item }))
    }

    async fn search_items(
        &self,
        request: Request<pb::SearchItemsRequest>,
    ) -> Result<Response<pb::SearchItemsResponse>, Status> {
        let request = request.into_inner();
        let items = self
            .database
            .search_items(&Context::background(), &request.query, i64::from(request.n))
            .await
            .map_err(unknown)?;
        let items = items
            .iter()
            .map(scored_item_to_pb)
            .collect::<Result<Vec<_>, _>>()
            .map_err(unknown)?;
        Ok(Response::new(pb::SearchItemsResponse { items }))
    }

    async fn modify_item(
        &self,
        request: Request<pb::ModifyItemRequest>,
    ) -> Result<Response<pb::ModifyItemResponse>, Status> {
        let request = request.into_inner();
        let patch =
            item_patch_from_pb(request.patch.unwrap_or_default()).map_err(Status::unknown)?;
        self.database
            .modify_item(&Context::background(), &request.item_id, &patch)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::ModifyItemResponse {}))
    }

    async fn get_items(
        &self,
        request: Request<pb::GetItemsRequest>,
    ) -> Result<Response<pb::GetItemsResponse>, Status> {
        let request = request.into_inner();
        let begin_time = request.begin_time.as_ref().map(|t| time_from_pb(Some(t)));
        let (cursor, items) = self
            .database
            .get_items(
                &Context::background(),
                &request.cursor,
                i64::from(request.n),
                begin_time,
            )
            .await
            .map_err(unknown)?;
        let items = items_to_pb(&items).map_err(unknown)?;
        Ok(Response::new(pb::GetItemsResponse { cursor, items }))
    }

    async fn get_item_feedback(
        &self,
        request: Request<pb::GetItemFeedbackRequest>,
    ) -> Result<Response<pb::GetFeedbackResponse>, Status> {
        let request = request.into_inner();
        let types = feedback_type_names(&request.feedback_types);
        let feedback = self
            .database
            .get_item_feedback(&Context::background(), &request.item_id, &types)
            .await
            .map_err(unknown)?;
        let feedback = feedback_to_pb(&feedback).map_err(unknown)?;
        Ok(Response::new(pb::GetFeedbackResponse {
            cursor: String::new(),
            feedback,
        }))
    }

    async fn batch_insert_users(
        &self,
        request: Request<pb::BatchInsertUsersRequest>,
    ) -> Result<Response<pb::BatchInsertUsersResponse>, Status> {
        let users = request
            .into_inner()
            .users
            .into_iter()
            .map(|user| {
                Ok(User {
                    user_id: user.user_id,
                    labels: labels_from_pb(&user.labels)?,
                    comment: user.comment,
                    update_at: Time::default(),
                })
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(Status::unknown)?;
        self.database
            .batch_insert_users(&Context::background(), &users)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::BatchInsertUsersResponse {}))
    }

    async fn delete_user(
        &self,
        request: Request<pb::DeleteUserRequest>,
    ) -> Result<Response<pb::DeleteUserResponse>, Status> {
        self.database
            .delete_user(&Context::background(), &request.into_inner().user_id)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteUserResponse {}))
    }

    async fn get_user(
        &self,
        request: Request<pb::GetUserRequest>,
    ) -> Result<Response<pb::GetUserResponse>, Status> {
        let user_id = request.into_inner().user_id;
        let user = self
            .database
            .get_user(&Context::background(), &user_id)
            .await;
        let user = missing_is_empty(user)?
            .map(|user| user_to_pb(&user))
            .transpose()
            .map_err(unknown)?;
        Ok(Response::new(pb::GetUserResponse { user }))
    }

    async fn modify_user(
        &self,
        request: Request<pb::ModifyUserRequest>,
    ) -> Result<Response<pb::ModifyUserResponse>, Status> {
        let request = request.into_inner();
        let patch =
            user_patch_from_pb(request.patch.unwrap_or_default()).map_err(Status::unknown)?;
        self.database
            .modify_user(&Context::background(), &request.user_id, &patch)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::ModifyUserResponse {}))
    }

    async fn get_users(
        &self,
        request: Request<pb::GetUsersRequest>,
    ) -> Result<Response<pb::GetUsersResponse>, Status> {
        let request = request.into_inner();
        let (cursor, users) = self
            .database
            .get_users(
                &Context::background(),
                &request.cursor,
                i64::from(request.n),
            )
            .await
            .map_err(unknown)?;
        let users = users
            .iter()
            .map(user_to_pb)
            .collect::<Result<Vec<_>, _>>()
            .map_err(unknown)?;
        Ok(Response::new(pb::GetUsersResponse { cursor, users }))
    }

    async fn get_user_feedback(
        &self,
        request: Request<pb::GetUserFeedbackRequest>,
    ) -> Result<Response<pb::GetFeedbackResponse>, Status> {
        let request = request.into_inner();
        let end_time = request.end_time.as_ref().map(|t| time_from_pb(Some(t)));
        let types = expressions_from_pb(&request.feedback_types);
        let feedback = self
            .database
            .get_user_feedback(&Context::background(), &request.user_id, end_time, &types)
            .await
            .map_err(unknown)?;
        let feedback = feedback_to_pb(&feedback).map_err(unknown)?;
        Ok(Response::new(pb::GetFeedbackResponse {
            cursor: String::new(),
            feedback,
        }))
    }

    async fn get_user_item_feedback(
        &self,
        request: Request<pb::GetUserItemFeedbackRequest>,
    ) -> Result<Response<pb::GetFeedbackResponse>, Status> {
        let request = request.into_inner();
        let types = feedback_type_names(&request.feedback_types);
        let feedback = self
            .database
            .get_user_item_feedback(
                &Context::background(),
                &request.user_id,
                &request.item_id,
                &types,
            )
            .await
            .map_err(unknown)?;
        let feedback = feedback_to_pb(&feedback).map_err(unknown)?;
        Ok(Response::new(pb::GetFeedbackResponse {
            cursor: String::new(),
            feedback,
        }))
    }

    async fn delete_user_item_feedback(
        &self,
        request: Request<pb::DeleteUserItemFeedbackRequest>,
    ) -> Result<Response<pb::DeleteUserItemFeedbackResponse>, Status> {
        let request = request.into_inner();
        let count = self
            .database
            .delete_user_item_feedback(
                &Context::background(),
                &request.user_id,
                &request.item_id,
                &request.feedback_types,
            )
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteUserItemFeedbackResponse {
            count: count as i32,
        }))
    }

    async fn batch_insert_feedback(
        &self,
        request: Request<pb::BatchInsertFeedbackRequest>,
    ) -> Result<Response<pb::BatchInsertFeedbackResponse>, Status> {
        let request = request.into_inner();
        let feedback = feedback_from_pb(request.feedback).map_err(Status::unknown)?;
        self.database
            .batch_insert_feedback(
                &Context::background(),
                &feedback,
                request.insert_user,
                request.insert_item,
                request.overwrite,
            )
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::BatchInsertFeedbackResponse {}))
    }

    async fn get_feedback(
        &self,
        request: Request<pb::GetFeedbackRequest>,
    ) -> Result<Response<pb::GetFeedbackResponse>, Status> {
        let request = request.into_inner();
        let begin_time = request.begin_time.as_ref().map(|t| time_from_pb(Some(t)));
        let end_time = request.end_time.as_ref().map(|t| time_from_pb(Some(t)));
        let types = feedback_type_names(&request.feedback_types);
        let (cursor, feedback) = self
            .database
            .get_feedback(
                &Context::background(),
                &request.cursor,
                i64::from(request.n),
                begin_time,
                end_time,
                &types,
            )
            .await
            .map_err(unknown)?;
        let feedback = feedback_to_pb(&feedback).map_err(unknown)?;
        Ok(Response::new(pb::GetFeedbackResponse { cursor, feedback }))
    }

    async fn get_user_stream(
        &self,
        request: Request<pb::GetUserStreamRequest>,
    ) -> Result<Response<Self::GetUserStreamStream>, Status> {
        let batch_size = i64::from(request.into_inner().batch_size);
        let database = Arc::clone(&self.database);
        let (tx, rx) = mpsc::channel(STREAM_BUFFER);
        tokio::spawn(async move {
            let ctx = Context::background();
            let mut batches = database.get_user_stream(&ctx, batch_size);
            while let Some(batch) = batches.next().await {
                let message = match batch {
                    Ok(users) => users
                        .iter()
                        .map(user_to_pb)
                        .collect::<Result<Vec<_>, _>>()
                        .map(|users| pb::GetUserStreamResponse { users })
                        .map_err(unknown),
                    Err(err) => Err(unknown(err)),
                };
                let failed = message.is_err();
                if tx.send(message).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn get_item_stream(
        &self,
        request: Request<pb::GetItemStreamRequest>,
    ) -> Result<Response<Self::GetItemStreamStream>, Status> {
        let request = request.into_inner();
        let batch_size = i64::from(request.batch_size);
        let time_limit = request.time_limit.as_ref().map(|t| time_from_pb(Some(t)));
        let database = Arc::clone(&self.database);
        let (tx, rx) = mpsc::channel(STREAM_BUFFER);
        tokio::spawn(async move {
            let ctx = Context::background();
            let mut batches = database.get_item_stream(&ctx, batch_size, time_limit);
            while let Some(batch) = batches.next().await {
                let message = match batch {
                    Ok(items) => items_to_pb(&items)
                        .map(|items| pb::GetItemStreamResponse { items })
                        .map_err(unknown),
                    Err(err) => Err(unknown(err)),
                };
                let failed = message.is_err();
                if tx.send(message).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn get_feedback_stream(
        &self,
        request: Request<pb::GetFeedbackStreamRequest>,
    ) -> Result<Response<Self::GetFeedbackStreamStream>, Status> {
        let request = request.into_inner();
        // Go dereferences `ScanOptions` without a check and panics, so a request
        // without it crashes the master (`contracts/grpc.md`). Here it is refused.
        let scan_options = request
            .scan_options
            .ok_or_else(|| Status::invalid_argument("scan_options is required"))?;
        let batch_size = i64::from(request.batch_size);
        let opts = scan_options_from_pb(scan_options);
        let database = Arc::clone(&self.database);
        let (tx, rx) = mpsc::channel(STREAM_BUFFER);
        tokio::spawn(async move {
            let ctx = Context::background();
            let mut batches = database.get_feedback_stream(&ctx, batch_size, &opts);
            while let Some(batch) = batches.next().await {
                let message = match batch {
                    Ok(feedback) => feedback_to_pb(&feedback)
                        .map(|feedback| pb::GetFeedbackStreamResponse { feedback })
                        .map_err(unknown),
                    Err(err) => Err(unknown(err)),
                };
                let failed = message.is_err();
                if tx.send(message).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn count_users(
        &self,
        _request: Request<pb::CountUsersRequest>,
    ) -> Result<Response<pb::CountUsersResponse>, Status> {
        let count = self
            .database
            .count_users(&Context::background())
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::CountUsersResponse {
            count: count as i32,
        }))
    }

    async fn count_items(
        &self,
        _request: Request<pb::CountItemsRequest>,
    ) -> Result<Response<pb::CountItemsResponse>, Status> {
        let count = self
            .database
            .count_items(&Context::background())
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::CountItemsResponse {
            count: count as i32,
        }))
    }

    async fn count_feedback(
        &self,
        _request: Request<pb::CountFeedbackRequest>,
    ) -> Result<Response<pb::CountFeedbackResponse>, Status> {
        let count = self
            .database
            .count_feedback(&Context::background())
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::CountFeedbackResponse {
            count: count as i32,
        }))
    }

    async fn get_latest_items(
        &self,
        request: Request<pb::GetLatestItemsRequest>,
    ) -> Result<Response<pb::GetLatestItemsResponse>, Status> {
        let request = request.into_inner();
        let after = request.after.as_ref().map(|t| time_from_pb(Some(t)));
        let items = self
            .database
            .get_latest_items(
                &Context::background(),
                i64::from(request.n),
                &request.categories,
                after,
            )
            .await
            .map_err(unknown)?;
        let items = items_to_pb(&items).map_err(unknown)?;
        Ok(Response::new(pb::GetLatestItemsResponse { items }))
    }
}

/// Go's `ProxyClient`: a [`Database`] that calls a remote [`ProxyServer`].
///
/// `Init` and `Purge` are refused, as in Go, and so is nothing else. `Close`
/// and `Optimize` do nothing.
pub struct ProxyClient {
    client: DataStoreClient<Channel>,
    /// The backend that the suite's `SetupTest` and `TearDownTest` reach
    /// directly, as Go's proxy suite does. Only the suite's tests set it.
    #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
    suite_backend: Option<Arc<dyn Database>>,
}

impl ProxyClient {
    /// Go's `NewProxyClient`.
    pub fn new(channel: Channel) -> Self {
        Self {
            client: DataStoreClient::new(channel),
            #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
            suite_backend: None,
        }
    }

    /// A client for the proxy suite: `ping` and `purge` reach `backend` directly,
    /// as Go's `ProxyTestSuite.SetupTest` does, and every other call goes over
    /// the channel.
    #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
    pub(crate) fn for_suite(channel: Channel, backend: Arc<dyn Database>) -> Self {
        Self {
            client: DataStoreClient::new(channel),
            suite_backend: Some(backend),
        }
    }

    /// A handle to the gRPC client for one call. The generated client takes
    /// `&mut self`, and a clone shares the connection.
    fn grpc(&self) -> DataStoreClient<Channel> {
        self.client.clone()
    }
}

#[async_trait]
impl Database for ProxyClient {
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
        if let Some(backend) = &self.suite_backend {
            return backend.ping().await;
        }
        self.grpc()
            .ping(pb::PingRequest {})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn init(&self) -> Result<(), Error> {
        Err(refused("method Init is not allowed in ProxyClient"))
    }

    async fn reconcile(&self, config: &SearchConfig) -> Result<(), Error> {
        self.grpc()
            .reconcile(pb::ReconcileRequest {
                search_columns: config.columns.clone().unwrap_or_default(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn optimize(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
        if let Some(backend) = &self.suite_backend {
            return backend.purge().await;
        }
        Err(refused("method Purge is not allowed in ProxyClient"))
    }

    async fn batch_insert_items(&self, _ctx: &Context, items: &[Item]) -> Result<(), Error> {
        let items = items
            .iter()
            .map(|item| {
                Ok(pb::Item {
                    item_id: item.item_id.clone(),
                    is_hidden: item.is_hidden,
                    categories: item.categories.clone().unwrap_or_default(),
                    timestamp: Some(time_to_pb(&item.timestamp)),
                    labels: labels_to_pb(&item.labels).map_err(backend_encoding)?,
                    comment: item.comment.clone(),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        self.grpc()
            .batch_insert_items(pb::BatchInsertItemsRequest { items })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn batch_get_items(
        &self,
        _ctx: &Context,
        item_ids: &[String],
        opts: &GetOptions,
    ) -> Result<Vec<Item>, Error> {
        let mut request = pb::BatchGetItemsRequest {
            item_ids: item_ids.to_vec(),
            get_options: None,
        };
        if !opts.categories.is_empty() || opts.skip_hidden || opts.return_id || opts.after.is_some()
        {
            request.get_options = Some(pb::GetOptions {
                categories: opts.categories.clone(),
                skip_hidden: opts.skip_hidden,
                return_id: opts.return_id,
                after: opts.after.as_ref().map(time_to_pb),
            });
        }
        let response = self
            .grpc()
            .batch_get_items(request)
            .await
            .map_err(backend)?;
        response
            .into_inner()
            .items
            .into_iter()
            .map(|item| item_from_response(item).map_err(backend_text))
            .collect()
    }

    async fn delete_item(&self, _ctx: &Context, item_id: &str) -> Result<(), Error> {
        self.grpc()
            .delete_item(pb::DeleteItemRequest {
                item_id: item_id.to_owned(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_item(&self, _ctx: &Context, item_id: &str) -> Result<Item, Error> {
        let response = self
            .grpc()
            .get_item(pb::GetItemRequest {
                item_id: item_id.to_owned(),
            })
            .await
            .map_err(backend)?
            .into_inner();
        item_result(item_id, response)
    }

    async fn search_items(
        &self,
        _ctx: &Context,
        query: &str,
        n: i64,
    ) -> Result<Vec<ScoredItem>, Error> {
        let response = self
            .grpc()
            .search_items(pb::SearchItemsRequest {
                query: query.to_owned(),
                n: n as i32,
            })
            .await
            .map_err(backend)?
            .into_inner();
        response
            .items
            .into_iter()
            .map(|scored| {
                Ok(ScoredItem {
                    item: item_from_response(scored.item.unwrap_or_default())
                        .map_err(backend_text)?,
                    score: scored.score,
                })
            })
            .collect()
    }

    async fn modify_item(
        &self,
        _ctx: &Context,
        item_id: &str,
        patch: &ItemPatch,
    ) -> Result<(), Error> {
        let patch = item_patch_to_pb(patch).map_err(backend_encoding)?;
        self.grpc()
            .modify_item(pb::ModifyItemRequest {
                item_id: item_id.to_owned(),
                patch: Some(patch),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_latest_items(
        &self,
        _ctx: &Context,
        n: i64,
        categories: &[String],
        after: Option<Time>,
    ) -> Result<Vec<Item>, Error> {
        let response = self
            .grpc()
            .get_latest_items(pb::GetLatestItemsRequest {
                n: n as i32,
                categories: categories.to_vec(),
                after: after.as_ref().map(time_to_pb),
            })
            .await
            .map_err(backend)?
            .into_inner();
        response
            .items
            .into_iter()
            .map(|item| item_from_response(item).map_err(backend_text))
            .collect()
    }

    async fn get_items(
        &self,
        _ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error> {
        let response = self
            .grpc()
            .get_items(pb::GetItemsRequest {
                cursor: cursor.to_owned(),
                n: n as i32,
                begin_time: begin_time.as_ref().map(time_to_pb),
            })
            .await
            .map_err(backend)?
            .into_inner();
        let items = response
            .items
            .into_iter()
            .map(|item| item_from_response(item).map_err(backend_text))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.cursor, items))
    }

    async fn get_item_feedback(
        &self,
        _ctx: &Context,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        let response = self
            .grpc()
            .get_item_feedback(pb::GetItemFeedbackRequest {
                item_id: item_id.to_owned(),
                feedback_types: strings_to_expressions(feedback_types),
            })
            .await
            .map_err(backend)?
            .into_inner();
        feedback_from_pb(response.feedback).map_err(backend_text)
    }

    async fn batch_insert_users(&self, _ctx: &Context, users: &[User]) -> Result<(), Error> {
        let users = users
            .iter()
            .map(|user| {
                Ok(pb::User {
                    user_id: user.user_id.clone(),
                    labels: labels_to_pb(&user.labels).map_err(backend_encoding)?,
                    comment: user.comment.clone(),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        self.grpc()
            .batch_insert_users(pb::BatchInsertUsersRequest { users })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn delete_user(&self, _ctx: &Context, user_id: &str) -> Result<(), Error> {
        self.grpc()
            .delete_user(pb::DeleteUserRequest {
                user_id: user_id.to_owned(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_user(&self, _ctx: &Context, user_id: &str) -> Result<User, Error> {
        let response = self
            .grpc()
            .get_user(pb::GetUserRequest {
                user_id: user_id.to_owned(),
            })
            .await
            .map_err(backend)?
            .into_inner();
        user_result(user_id, response)
    }

    async fn modify_user(
        &self,
        _ctx: &Context,
        user_id: &str,
        patch: &UserPatch,
    ) -> Result<(), Error> {
        let patch = user_patch_to_pb(patch).map_err(backend_encoding)?;
        self.grpc()
            .modify_user(pb::ModifyUserRequest {
                user_id: user_id.to_owned(),
                patch: Some(patch),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_users(
        &self,
        _ctx: &Context,
        cursor: &str,
        n: i64,
    ) -> Result<(String, Vec<User>), Error> {
        let response = self
            .grpc()
            .get_users(pb::GetUsersRequest {
                cursor: cursor.to_owned(),
                n: n as i32,
            })
            .await
            .map_err(backend)?
            .into_inner();
        let users = response
            .users
            .into_iter()
            .map(|user| user_from_response(user).map_err(backend_text))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((response.cursor, users))
    }

    async fn get_user_feedback(
        &self,
        _ctx: &Context,
        user_id: &str,
        end_time: Option<Time>,
        feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error> {
        let response = self
            .grpc()
            .get_user_feedback(pb::GetUserFeedbackRequest {
                user_id: user_id.to_owned(),
                end_time: end_time.as_ref().map(time_to_pb),
                feedback_types: expressions_to_pb(feedback_types),
            })
            .await
            .map_err(backend)?
            .into_inner();
        feedback_from_pb(response.feedback).map_err(backend_text)
    }

    async fn get_user_item_feedback(
        &self,
        _ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        let response = self
            .grpc()
            .get_user_item_feedback(pb::GetUserItemFeedbackRequest {
                user_id: user_id.to_owned(),
                item_id: item_id.to_owned(),
                feedback_types: strings_to_expressions(feedback_types),
            })
            .await
            .map_err(backend)?
            .into_inner();
        feedback_from_pb(response.feedback).map_err(backend_text)
    }

    async fn delete_user_item_feedback(
        &self,
        _ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<i64, Error> {
        let response = self
            .grpc()
            .delete_user_item_feedback(pb::DeleteUserItemFeedbackRequest {
                user_id: user_id.to_owned(),
                item_id: item_id.to_owned(),
                feedback_types: feedback_types.to_vec(),
            })
            .await
            .map_err(backend)?
            .into_inner();
        Ok(i64::from(response.count))
    }

    async fn batch_insert_feedback(
        &self,
        _ctx: &Context,
        feedback: &[Feedback],
        insert_user: bool,
        insert_item: bool,
        overwrite: bool,
    ) -> Result<(), Error> {
        let feedback = feedback_to_pb(feedback).map_err(backend_encoding)?;
        self.grpc()
            .batch_insert_feedback(pb::BatchInsertFeedbackRequest {
                feedback,
                insert_user,
                insert_item,
                overwrite,
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_feedback(
        &self,
        _ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
        end_time: Option<Time>,
        feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error> {
        let response = self
            .grpc()
            .get_feedback(pb::GetFeedbackRequest {
                cursor: cursor.to_owned(),
                n: n as i32,
                begin_time: begin_time.as_ref().map(time_to_pb),
                end_time: end_time.as_ref().map(time_to_pb),
                feedback_types: strings_to_expressions(feedback_types),
            })
            .await
            .map_err(backend)?
            .into_inner();
        let feedback = feedback_from_pb(response.feedback).map_err(backend_text)?;
        Ok((response.cursor, feedback))
    }

    fn get_user_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>> {
        let mut client = self.grpc();
        let request = pb::GetUserStreamRequest {
            batch_size: batch_size as i32,
        };
        Box::pin(
            futures_util::stream::once(async move {
                let stream = client.get_user_stream(request).await.map_err(backend)?;
                Ok::<_, Error>(stream.into_inner().map(|message| {
                    let response = message.map_err(backend)?;
                    response
                        .users
                        .into_iter()
                        .map(|user| user_from_response(user).map_err(backend_text))
                        .collect()
                }))
            })
            .try_flatten(),
        )
    }

    /// Go's `ProxyClient.GetItemStream` never sends `time_limit`, so the limit
    /// is dropped here too (`contracts/grpc.md`). The server honours it when it
    /// is sent.
    fn get_item_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
        _time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>> {
        let mut client = self.grpc();
        let request = pb::GetItemStreamRequest {
            batch_size: batch_size as i32,
            time_limit: None,
        };
        Box::pin(
            futures_util::stream::once(async move {
                let stream = client.get_item_stream(request).await.map_err(backend)?;
                Ok::<_, Error>(stream.into_inner().map(|message| {
                    let response = message.map_err(backend)?;
                    response
                        .items
                        .into_iter()
                        .map(|item| item_from_response(item).map_err(backend_text))
                        .collect()
                }))
            })
            .try_flatten(),
        )
    }

    fn get_feedback_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
        options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>> {
        let scan_options = scan_options_to_pb(&new_scan_options(options));
        let mut client = self.grpc();
        let request = pb::GetFeedbackStreamRequest {
            batch_size: batch_size as i32,
            scan_options: Some(scan_options),
        };
        Box::pin(
            futures_util::stream::once(async move {
                let stream = client.get_feedback_stream(request).await.map_err(backend)?;
                Ok::<_, Error>(stream.into_inner().map(|message| {
                    let response = message.map_err(backend)?;
                    feedback_from_pb(response.feedback).map_err(backend_text)
                }))
            })
            .try_flatten(),
        )
    }

    async fn count_users(&self, _ctx: &Context) -> Result<i64, Error> {
        let response = self
            .grpc()
            .count_users(pb::CountUsersRequest {})
            .await
            .map_err(backend)?
            .into_inner();
        Ok(i64::from(response.count))
    }

    async fn count_items(&self, _ctx: &Context) -> Result<i64, Error> {
        let response = self
            .grpc()
            .count_items(pb::CountItemsRequest {})
            .await
            .map_err(backend)?
            .into_inner();
        Ok(i64::from(response.count))
    }

    async fn count_feedback(&self, _ctx: &Context) -> Result<i64, Error> {
        let response = self
            .grpc()
            .count_feedback(pb::CountFeedbackRequest {})
            .await
            .map_err(backend)?
            .into_inner();
        Ok(i64::from(response.count))
    }
}

/// A marshalling failure seen by a client, as Go's `json.Marshal` error.
fn backend_encoding(err: serde_json::Error) -> Error {
    Error::Backend(Arc::new(err))
}

#[cfg(test)]
mod tests {
    use super::super::types::nested_labels;
    use super::*;

    #[test]
    fn labels_nested_ten_thousand_levels_round_trip_through_pb() {
        labels_round_trip_through_pb(10_000);
    }

    #[test]
    fn labels_nested_one_hundred_thousand_levels_round_trip_through_pb() {
        labels_round_trip_through_pb(100_000);
    }

    /// Writes labels nested `depth` levels to the protobuf bytes and reads them
    /// back. The bytes are the text Go writes, and the value reads back as it.
    fn labels_round_trip_through_pb(depth: usize) {
        let (labels, text) = nested_labels(depth);
        let bytes = labels_to_pb(&labels).expect("marshal");
        assert!(bytes == text.as_bytes(), "labels were written differently");
        let read = labels_from_pb(&bytes).expect("unmarshal");
        let again = String::from_utf8(gorse_common::jsonutil::marshal(&read).expect("marshal"))
            .expect("utf-8");
        assert!(again == text, "labels changed on the way back");
    }

    #[test]
    fn zero_time_is_go_zero_time_on_the_wire() {
        // Go's timestamppb.New(time.Time{}) is January 1, year 1 UTC.
        let pb = time_to_pb(&Time::default());
        assert_eq!(pb.seconds, -62_135_596_800);
        assert_eq!(pb.nanos, 0);
        assert_eq!(optional_time_from_pb(Some(&pb)), Time::default().utc());
    }

    #[test]
    fn missing_timestamp_reads_as_epoch_except_where_go_says_zero() {
        // Go's Timestamp.AsTime on a nil receiver is the Unix epoch, and
        // timeFromPB gives the zero time for the same nil.
        assert_eq!(time_from_pb(None).unix(), 0);
        assert!(optional_time_from_pb(None).is_zero());
    }

    #[test]
    fn timestamps_keep_nanoseconds() {
        let t = gotime::unix(1_700_000_000, 123_456_789).utc();
        let pb = time_to_pb(&t);
        assert_eq!((pb.seconds, pb.nanos), (1_700_000_000, 123_456_789));
        assert!(time_from_pb(Some(&pb)).equal(&t));
    }

    #[test]
    fn empty_labels_are_rejected_as_go_rejects_them() {
        assert_eq!(
            labels_from_pb(&[]).unwrap_err(),
            "unexpected end of JSON input"
        );
        assert_eq!(labels_from_pb(b"null").unwrap(), Labels::Nil);
    }

    #[test]
    fn go_code_names_match_go() {
        let status = rpc_error(Status::not_found("nope"));
        assert_eq!(status.to_string(), "rpc error: code = NotFound desc = nope");
        let status = rpc_error(Status::cancelled("stop"));
        assert_eq!(status.to_string(), "rpc error: code = Canceled desc = stop");
    }

    #[test]
    fn item_not_found_keeps_the_sentinel_and_go_text() {
        // Go: fmt.Errorf("item %s %w", itemId, storage.ErrNotFound).
        let err = item_result("abc", pb::GetItemResponse { item: None }).unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
        assert_eq!(err.to_string(), "item abc not found");
    }

    #[test]
    fn user_not_found_keeps_the_sentinel_and_go_text() {
        let err = user_result("alice", pb::GetUserResponse { user: None }).unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
        assert_eq!(err.to_string(), "user alice not found");
    }

    #[test]
    fn feedback_with_empty_labels_reads_as_nil() {
        let feedback = pb::Feedback {
            feedback_type: "click".to_owned(),
            user_id: "1".to_owned(),
            item_id: "2".to_owned(),
            ..Default::default()
        };
        let decoded = feedback_from_pb(vec![feedback]).unwrap();
        assert_eq!(decoded[0].labels, Labels::Nil);
        assert_eq!(
            decoded[0].timestamp.unix(),
            0,
            "a nil timestamp is the epoch"
        );
    }

    #[test]
    fn expressions_round_trip() {
        let expression = FeedbackTypeExpression {
            feedback_type: "like".to_owned(),
            expr_type: gorse_common::expression::ExprType::Greater,
            value: 2.5,
        };
        let back = expressions_from_pb(&[expression.to_pb()]);
        assert_eq!(back, vec![expression]);
    }

    #[tokio::test]
    async fn a_client_refuses_init_and_purge_without_a_server() {
        // The refusals are local, so the channel never connects.
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let client = ProxyClient::new(channel);
        let err = client.init().await.unwrap_err();
        assert_eq!(err.to_string(), "method Init is not allowed in ProxyClient");
        let err = client.purge().await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "method Purge is not allowed in ProxyClient"
        );
    }
}

/// Serving a store over a local port, for the tests of this crate's proxies.
#[cfg(test)]
pub(crate) mod support {
    use tonic::transport::{Channel, Endpoint, server::Router};

    /// Serves `router` on a free local port, and returns a channel to it.
    pub(crate) async fn connect(router: Router) -> Channel {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let addr = listener.local_addr().expect("local address");
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tokio::spawn(router.serve_with_incoming(incoming));
        Endpoint::from_shared(format!("http://{addr}"))
            .expect("endpoint")
            .connect()
            .await
            .expect("connect to the local server")
    }
}

#[cfg(test)]
mod wire {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use tonic::transport::Server;

    use super::support::connect;
    use super::*;
    use crate::data::no_database::NoDatabase;

    #[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
    impl crate::data::database_test::Backend for ProxyClient {
        fn time_stamps(&self) -> crate::data::database_test::TimeStamps {
            // Go's type switch knows no proxy client, so its timezone case is
            // skipped for it (`storage/data/database_test.go`).
            crate::data::database_test::TimeStamps::Unknown
        }
    }

    /// A client of `database` served through the proxy, with every call on the wire.
    async fn wire_client_of(database: Arc<dyn Database>) -> ProxyClient {
        let router = Server::builder().add_service(ProxyServer::new(database).into_service());
        ProxyClient::new(connect(router).await)
    }

    /// A client of `database` served through the proxy, as the proxy suite has it.
    #[cfg(all(test, feature = "mysql", feature = "postgres", feature = "sqlite"))]
    async fn client_of(database: Arc<dyn Database>) -> ProxyClient {
        let router =
            Server::builder().add_service(ProxyServer::new(database.clone()).into_service());
        ProxyClient::for_suite(connect(router).await, database)
    }

    /// A store with no database behind it errors through the proxy with the
    /// same text Go's client shows: the status code and the message.
    #[tokio::test]
    async fn no_database_errors_cross_the_wire() {
        let client = wire_client_of(Arc::new(NoDatabase)).await;
        let ctx = Context::background();
        let text = "rpc error: code = Unknown desc = database not assigned";
        assert_eq!(client.ping().await.unwrap_err().to_string(), text);
        assert_eq!(
            client.get_item(&ctx, "a").await.unwrap_err().to_string(),
            text
        );
        assert_eq!(
            client.count_users(&ctx).await.unwrap_err().to_string(),
            text
        );
        let mut batches = client.get_user_stream(&ctx, 10);
        let first = batches.next().await.expect("one item");
        assert_eq!(first.unwrap_err().to_string(), text);
    }

    /// The suite of Go's proxy test, on SQLite, through the proxy.
    #[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
    #[tokio::test]
    async fn suite_through_the_proxy_on_sqlite() {
        use crate::data::database_test::run_through_proxy;
        use crate::data::sql;

        let dir = tempfile::tempdir().expect("temp dir");
        let mut registry = super::super::Registry::default();
        sql::register(&mut registry);
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let backend: Arc<dyn Database> =
            Arc::from(registry.open(&path, "gorse_", &[]).await.expect("open"));
        backend.init().await.expect("init");
        let client = client_of(backend).await;
        run_through_proxy(&client).await;
    }

    /// A missing item on the backend reads back through the proxy with Go's
    /// text and the NotFound kind.
    #[cfg(all(feature = "mysql", feature = "postgres", feature = "sqlite"))]
    #[tokio::test]
    async fn missing_item_reads_back_with_go_text() {
        use crate::data::sql;

        let dir = tempfile::tempdir().expect("temp dir");
        let mut registry = super::super::Registry::default();
        sql::register(&mut registry);
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let backend = registry.open(&path, "gorse_", &[]).await.expect("open");
        backend.init().await.expect("init");
        let client = client_of(Arc::from(backend)).await;
        let ctx = Context::background();
        let err = client.get_item(&ctx, "missing").await.unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
        assert_eq!(err.to_string(), "item missing not found");
        let err = client.get_user(&ctx, "missing").await.unwrap_err();
        assert_eq!(err.to_string(), "user missing not found");
    }
}
