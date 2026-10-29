// Ports: master/rest.go (`CreateWebService`'s 23 dashboard routes and their handlers, `UserInfo`, `Status`, `User`, `DetailedFeedback`, `ScoredItem`, `ScoreUser`) @ add21b5

//! The dashboard's JSON API under `/api/dashboard/` (`master/rest.go`).
//!
//! Each handler writes the JSON of its Go handler, with the field names Go
//! writes. The read-only routes are in this module; the config, the ranker
//! prompt and the recommendation routes have their own modules.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::monitor::Progress;
use gorse_model::cf::model as cf;
use gorse_model::ctr::model as ctr;
use gorse_server::rest::response::{ApiError, PathParams, Query, ok, parse_int, path_param};
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::data::types;
use gorse_storage::meta;
use gorse_storage_core::errors::Error as StorageError;
use serde::{Deserialize, Serialize};

use crate::Master;
use crate::rest::{login_filter, recommend};
use crate::rest::config;
use crate::rest::ranker;

/// Go's `version.Version`: the binary version the status reports.
pub(crate) const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The dashboard routes, behind the login filter (Go's `LoginFilter` on the
/// WebService).
pub(crate) fn routes(master: Arc<Master>) -> Router {
    Router::new()
        .route("/api/dashboard/userinfo", get(user_info))
        .route("/api/dashboard/cluster", get(cluster))
        .route("/api/dashboard/categories", get(categories))
        .route(
            "/api/dashboard/config",
            get(config::get_config)
                .post(config::post_config)
                .delete(config::delete_config),
        )
        .route("/api/dashboard/config/schema", get(config::get_config_schema))
        .route("/api/dashboard/stats", get(stats))
        .route("/api/dashboard/tasks", get(tasks))
        .route("/api/dashboard/timeseries/{name}", get(timeseries))
        .route("/api/dashboard/user/{user-id}", get(user))
        .route("/api/dashboard/user/{user-id}/feedback", get(user_feedback))
        .route("/api/dashboard/user/{user-id}/feedback/", get(user_feedback))
        .route(
            "/api/dashboard/user/{user-id}/feedback/{feedback-type}",
            get(user_feedback),
        )
        .route("/api/dashboard/users", get(users))
        .route("/api/dashboard/latest", get(recommend::latest))
        .route(
            "/api/dashboard/non-personalized/{name}",
            get(recommend::non_personalized),
        )
        .route("/api/dashboard/recommend/{user-id}", get(recommend::recommend))
        .route(
            "/api/dashboard/recommend/{user-id}/{recommender}",
            get(recommend::recommend),
        )
        .route(
            "/api/dashboard/recommend/{user-id}/{recommender}/{name}",
            get(recommend::recommend),
        )
        .route(
            "/api/dashboard/item-to-item/{name}/{item-id}",
            get(recommend::item_to_item),
        )
        .route(
            "/api/dashboard/user-to-user/{name}/{user-id}",
            get(recommend::user_to_user),
        )
        .route("/api/dashboard/external", get(recommend::external))
        .route("/api/dashboard/ranker/prompt", get(ranker::prompt))
        .route_layer(middleware::from_fn_with_state(master.clone(), login_filter))
        .with_state(master)
}

/// The query string of a request, parsed as Go's `url.ParseQuery` parses it.
pub(crate) fn query_of(uri: &Uri) -> Query {
    Query::parse(uri.query())
}

/// Go's `UserInfo`: the claims of the logged-in user (`json` tags).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UserInfo {
    /// The full name.
    pub name: String,
    /// The family name.
    pub family_name: String,
    /// The given name.
    pub given_name: String,
    /// The middle name.
    pub middle_name: String,
    /// The nick name.
    pub nickname: String,
    /// The picture URL.
    pub picture: String,
    /// When the claims were last updated.
    pub updated_at: String,
    /// The email address.
    pub email: String,
    /// Whether the email address is verified.
    #[serde(rename = "email_verified")]
    pub verified: bool,
    /// How the user logged in.
    pub auth_type: String,
}

/// `GET /api/dashboard/userinfo`: the logged-in user. Without dashboard
/// credentials, or with no OIDC token, the body is `null`.
pub(crate) async fn user_info(State(master): State<Arc<Master>>, headers: HeaderMap) -> Response {
    let cfg = master.config();
    if cfg.oidc.enable {
        if let Some(mut info) = master.oidc_user_info(&headers) {
            info.auth_type = "OIDC".to_owned();
            return ok_response(&info);
        }
    } else if !cfg.master.dashboard_user_name.is_empty() {
        return ok_response(&UserInfo {
            name: cfg.master.dashboard_user_name.clone(),
            ..UserInfo::default()
        });
    }
    null_json()
}

/// Go's `Ok` for a value the handler built.
fn ok_response<T: Serialize + ?Sized>(value: &T) -> Response {
    match ok(value) {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

/// Writes `null` as JSON, with the content type Go sets for it.
fn null_json() -> Response {
    use axum::http::{HeaderValue, header};
    let mut response = Response::new(axum::body::Body::from("null"));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// `GET /api/dashboard/cluster`: the nodes the meta database lists, sorted by type.
pub(crate) async fn cluster(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    let mut nodes = master
        .meta_store()
        .list_nodes()
        .await
        .map_err(ApiError::internal)?;
    nodes.sort_by(|a, b| a.r#type.cmp(&b.r#type));
    let views: Vec<NodeView> = nodes.iter().map(NodeView::from).collect();
    ok(&views)
}

/// Go's `meta.Node` as JSON.
#[derive(Debug, Serialize)]
pub(crate) struct NodeView {
    #[serde(rename = "UUID")]
    uuid: String,
    #[serde(rename = "Hostname")]
    hostname: String,
    #[serde(rename = "Type")]
    r#type: String,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "UpdateTime")]
    update_time: Time,
}

impl From<&meta::Node> for NodeView {
    fn from(node: &meta::Node) -> Self {
        Self {
            uuid: node.uuid.clone(),
            hostname: node.hostname.clone(),
            r#type: node.r#type.clone(),
            version: node.version.clone(),
            update_time: node.update_time.clone(),
        }
    }
}

/// `GET /api/dashboard/categories`: the item categories the cache holds.
pub(crate) async fn categories(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    let scores = master
        .cache()
        .search_scores(&Context::background(), cache::ITEM_CATEGORIES, "", &[], 0, -1)
        .await
        .map_err(ApiError::internal)?;
    let categories: Vec<String> = scores.into_iter().map(|score| score.id).collect();
    ok(&categories)
}

/// `GET /api/dashboard/stats`: the counts and the update times the cache keeps,
/// with the number of servers and workers the meta database lists.
pub(crate) async fn stats(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cache = master.cache();
    let mut status = Status {
        binary_version: BINARY_VERSION.to_owned(),
        ..Status::default()
    };
    status.num_users = read_integer(&cache, &ctx, cache::NUM_USERS).await;
    status.num_items = read_integer(&cache, &ctx, cache::NUM_ITEMS).await;
    status.num_user_labels = read_integer(&cache, &ctx, cache::NUM_USER_LABELS).await;
    status.num_item_labels = read_integer(&cache, &ctx, cache::NUM_ITEM_LABELS).await;
    status.num_total_pos_feedback =
        read_integer(&cache, &ctx, cache::NUM_TOTAL_POS_FEEDBACKS).await;
    status.num_valid_pos_feedback =
        read_integer(&cache, &ctx, cache::NUM_VALID_POS_FEEDBACKS).await;
    status.num_valid_neg_feedback =
        read_integer(&cache, &ctx, cache::NUM_VALID_NEG_FEEDBACKS).await;
    let nodes = master
        .meta_store()
        .list_nodes()
        .await
        .map_err(ApiError::internal)?;
    for node in &nodes {
        match node.r#type.as_str() {
            "server" => status.num_servers += 1,
            "worker" => status.num_workers += 1,
            _ => {}
        }
    }
    status.popular_items_update_time =
        read_time(&cache, &ctx, cache::LAST_UPDATE_POPULAR_ITEMS_TIME).await;
    status.latest_items_update_time =
        read_time(&cache, &ctx, cache::LAST_UPDATE_LATEST_ITEMS_TIME).await;
    status.matching_model_fit_time =
        read_time(&cache, &ctx, cache::LAST_FIT_MATCHING_MODEL_TIME).await;
    status.ranking_model_fit_time =
        read_time(&cache, &ctx, cache::LAST_FIT_RANKING_MODEL_TIME).await;
    ok(&status)
}

/// The integer a global meta key holds. A missing or bad value reads as `0`,
/// which Go's handler logs and ignores.
async fn read_integer(cache: &Arc<dyn cache::Database>, ctx: &Context, name: &str) -> i64 {
    cache
        .get(ctx, &cache::key(&[cache::GLOBAL_META, name]))
        .await
        .integer()
        .unwrap_or(0)
}

/// The time a global meta key holds. A missing or bad value reads as the zero time.
async fn read_time(cache: &Arc<dyn cache::Database>, ctx: &Context, name: &str) -> Time {
    cache
        .get(ctx, &cache::key(&[cache::GLOBAL_META, name]))
        .await
        .time()
        .unwrap_or_default()
}

/// Go's `master.Status`, with the same field names.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Status {
    #[serde(rename = "BinaryVersion")]
    binary_version: String,
    #[serde(rename = "NumServers")]
    num_servers: i64,
    #[serde(rename = "NumWorkers")]
    num_workers: i64,
    #[serde(rename = "NumUsers")]
    num_users: i64,
    #[serde(rename = "NumItems")]
    num_items: i64,
    #[serde(rename = "NumUserLabels")]
    num_user_labels: i64,
    #[serde(rename = "NumItemLabels")]
    num_item_labels: i64,
    #[serde(rename = "NumTotalPosFeedback")]
    num_total_pos_feedback: i64,
    #[serde(rename = "NumValidPosFeedback")]
    num_valid_pos_feedback: i64,
    #[serde(rename = "NumValidNegFeedback")]
    num_valid_neg_feedback: i64,
    #[serde(rename = "PopularItemsUpdateTime")]
    popular_items_update_time: Time,
    #[serde(rename = "LatestItemsUpdateTime")]
    latest_items_update_time: Time,
    #[serde(rename = "MatchingModelFitTime")]
    matching_model_fit_time: Time,
    #[serde(rename = "MatchingModelScore")]
    matching_model_score: cf::Score,
    #[serde(rename = "RankingModelFitTime")]
    ranking_model_fit_time: Time,
    #[serde(rename = "RankingModelScore")]
    ranking_model_score: ctr::Score,
}

/// `GET /api/dashboard/tasks`: the progress of the local tasks and of the
/// workers' tasks.
pub(crate) async fn tasks(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    let nodes = master
        .meta_store()
        .list_nodes()
        .await
        .map_err(ApiError::internal)?;
    let workers: std::collections::HashSet<String> = nodes
        .iter()
        .filter(|node| node.r#type == "worker")
        .map(|node| node.uuid.clone())
        .collect();
    let mut progress: Vec<Progress> = master.tracer_list();
    for (name, remote) in master.remote_progress_snapshot() {
        if workers.contains(&name) {
            progress.extend(remote);
        }
    }
    let views: Vec<ProgressView> = progress.iter().map(ProgressView::from).collect();
    ok(&views)
}

/// Go's `monitor.Progress` as the dashboard serves it: its fields under their Go names.
#[derive(Serialize)]
pub(crate) struct ProgressView<'a> {
    #[serde(rename = "Tracer")]
    tracer: &'a str,
    #[serde(rename = "Name")]
    name: &'a str,
    #[serde(rename = "Status")]
    status: &'a str,
    #[serde(rename = "Error")]
    error: &'a str,
    #[serde(rename = "Count")]
    count: i64,
    #[serde(rename = "Total")]
    total: i64,
    #[serde(rename = "StartTime")]
    start_time: &'a Time,
    #[serde(rename = "FinishTime")]
    finish_time: &'a Time,
}

impl<'a> From<&'a Progress> for ProgressView<'a> {
    fn from(progress: &'a Progress) -> Self {
        Self {
            tracer: &progress.tracer,
            name: &progress.name,
            status: progress.status.as_str(),
            error: &progress.error,
            count: progress.count,
            total: progress.total,
            start_time: &progress.start_time,
            finish_time: &progress.finish_time,
        }
    }
}


/// `GET /api/dashboard/timeseries/{name}`: the points of a series between
/// `begin` and `end`, bucketed by `duration`.
pub(crate) async fn timeseries(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let name = path_param(&params, "name").to_owned();
    let query = query_of(&uri);
    let begin = query_time_or(&query, "begin", |now| now.add(gotime::Duration(-7 * 24 * 3600 * gotime::SECOND.0)))?;
    let end = query_time_or(&query, "end", |now| now)?;
    let duration_text = match query.get("duration") {
        "" => "24h",
        text => text,
    };
    let duration = gotime::parse_duration(duration_text).map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let points = master
        .cache()
        .get_time_series_points(&Context::background(), &cache::key(&[&name]), begin, end, duration)
        .await
        .map_err(ApiError::internal)?;
    ok(&points.iter().map(TimeSeriesView::from).collect::<Vec<_>>())
}

/// Go's `cache.TimeSeriesPoint` as JSON.
#[derive(Debug, Serialize)]
pub(crate) struct TimeSeriesView {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Timestamp")]
    timestamp: Time,
    #[serde(rename = "Value")]
    value: f64,
}

impl From<&cache::TimeSeriesPoint> for TimeSeriesView {
    fn from(point: &cache::TimeSeriesPoint) -> Self {
        Self {
            name: point.name.clone(),
            timestamp: point.timestamp.clone(),
            value: point.value,
        }
    }
}

/// The time of a query parameter, parsed with Go's `dateparse.ParseAny`. When
/// the parameter is empty, `default` gives it from the current time (Go's
/// `time.Now()`, formatted with RFC 3339 and parsed back).
fn query_time_or(
    query: &Query,
    name: &str,
    default: impl FnOnce(Time) -> Time,
) -> Result<Time, ApiError> {
    let text = query.get(name);
    if text.is_empty() {
        return Ok(default(gotime::now()));
    }
    gorse_common::dateparse::parse_any(text, &[]).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// `GET /api/dashboard/user/{user-id}`: a user, with the times the cache holds
/// for when the user was last active and last recommended.
pub(crate) async fn user(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let user_id = path_param(&params, "user-id").to_owned();
    let user = match master.data().get_user(&ctx, &user_id).await {
        Ok(user) => user,
        Err(err) => return Err(not_found_or_internal(&err)),
    };
    let detail = user_detail(&master, &ctx, user).await?;
    ok(&detail)
}

/// Go's `master.User`: a user with the times the cache holds for it.
#[derive(Debug, Default, Serialize)]
pub(crate) struct User {
    #[serde(flatten)]
    user: types::User,
    #[serde(rename = "LastActiveTime")]
    last_active_time: Time,
    #[serde(rename = "LastUpdateTime")]
    last_update_time: Time,
}

/// The user with its times, as Go's `getUser` and `getUsers` build it.
async fn user_detail(master: &Master, ctx: &Context, user: types::User) -> Result<User, ApiError> {
    let cache = master.cache();
    let last_active_time = read_user_time(&cache, ctx, cache::LAST_MODIFY_USER_TIME, &user.user_id).await?;
    let last_update_time = read_user_time(&cache, ctx, cache::RECOMMEND_UPDATE_TIME, &user.user_id).await?;
    Ok(User {
        user,
        last_active_time,
        last_update_time,
    })
}

/// The time a per-user key holds. A missing key reads as the zero time; any
/// other error fails the request, as Go's handler does.
async fn read_user_time(
    cache: &Arc<dyn cache::Database>,
    ctx: &Context,
    prefix: &str,
    user_id: &str,
) -> Result<Time, ApiError> {
    match cache.get(ctx, &cache::key(&[prefix, user_id])).await.time() {
        Ok(time) => Ok(time),
        Err(err) if err.kind() == Some(StorageError::NotFound) => Ok(Time::default()),
        Err(err) => Err(ApiError::internal(err)),
    }
}

/// Go's `server.PageNotFound` for a not-found store error, and
/// `InternalServerError` for any other.
pub(crate) fn not_found_or_internal(err: &data::Error) -> ApiError {
    if err.kind() == Some(StorageError::NotFound) {
        ApiError::PageNotFound(err.to_string())
    } else {
        ApiError::internal(err)
    }
}

/// `GET /api/dashboard/users`: one page of users with their times.
pub(crate) async fn users(State(master): State<Arc<Master>>, uri: Uri) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let query = query_of(&uri);
    let cursor = query.get("cursor").to_owned();
    let n = parse_int(&query, "n", master.config().server.default_n)?;
    let (cursor, users) = master
        .data()
        .get_users(&ctx, &cursor, n)
        .await
        .map_err(ApiError::internal)?;
    let mut details = Vec::with_capacity(users.len());
    for user in users {
        details.push(user_detail(&master, &ctx, user).await?);
    }
    ok(&UserIterator {
        cursor,
        users: details,
    })
}

/// Go's `master.UserIterator`.
#[derive(Debug, Serialize)]
pub(crate) struct UserIterator {
    #[serde(rename = "Cursor")]
    cursor: String,
    #[serde(rename = "Users")]
    users: Vec<User>,
}

/// `GET /api/dashboard/user/{user-id}/feedback[/{feedback-type}]`: the feedback
/// of a user, newest first, one page of it, with the items it is on.
pub(crate) async fn user_feedback(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cfg = master.config();
    let query = query_of(&uri);
    let feedback_type = path_param(&params, "feedback-type");
    let feedback_types = if feedback_type.is_empty() {
        cfg.recommend.data_source.positive_feedback_types.clone().unwrap_or_default()
    } else {
        let mut expression = gorse_common::expression::FeedbackTypeExpression::default();
        expression
            .from_string(feedback_type)
            .map_err(|err| ApiError::BadRequest(format!("invalid feedback type `{feedback_type}`: {err}")))?;
        vec![expression]
    };
    let n = if query.get("n").is_empty() {
        cfg.server.default_n
    } else {
        parse_int(&query, "n", cfg.server.default_n)?
    };
    let offset = if query.get("offset").is_empty() {
        0
    } else {
        parse_int(&query, "offset", 0)?
    };
    let user_id = path_param(&params, "user-id").to_owned();
    let mut feedback = master
        .data()
        .get_user_feedback(&ctx, &user_id, Some(cfg.now()), &feedback_types)
        .await
        .map_err(ApiError::internal)?;
    types::sort_feedbacks(&mut feedback);
    let offset = offset.max(0) as usize;
    if offset <= feedback.len() {
        feedback.drain(..offset);
    }
    if (n.max(0) as usize) < feedback.len() {
        feedback.truncate(n.max(0) as usize);
    }
    let item_ids: Vec<String> = feedback.iter().map(|f| f.feedback_key.item_id.clone()).collect();
    let items = master
        .data()
        .batch_get_items(&ctx, &item_ids, &data::GetOptions::default())
        .await
        .map_err(ApiError::internal)?;
    let items: HashMap<String, types::Item> = items
        .into_iter()
        .map(|item| (item.item_id.clone(), item))
        .collect();
    let details: Vec<DetailedFeedback> = feedback
        .into_iter()
        .map(|f| {
            let item = items.get(&f.feedback_key.item_id).cloned().unwrap_or_else(|| types::Item {
                item_id: f.feedback_key.item_id.clone(),
                comment: "** This item doesn't exist in Gorse **".to_owned(),
                ..types::Item::default()
            });
            DetailedFeedback {
                feedback_type: f.feedback_key.feedback_type,
                user_id: f.feedback_key.user_id,
                item,
                value: f.value,
                timestamp: f.timestamp,
                comment: f.comment,
            }
        })
        .collect();
    ok(&details)
}

/// Go's `master.DetailedFeedback`.
#[derive(Debug, Serialize)]
pub(crate) struct DetailedFeedback {
    #[serde(rename = "FeedbackType")]
    feedback_type: String,
    #[serde(rename = "UserId")]
    user_id: String,
    #[serde(rename = "Item")]
    item: types::Item,
    #[serde(rename = "Value")]
    value: f64,
    #[serde(rename = "Timestamp")]
    timestamp: Time,
    #[serde(rename = "Comment")]
    comment: String,
}
