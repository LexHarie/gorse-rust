// Ports: master/rest.go (`getLatest`, `getNonPersonalized`, `getRecommend`, `getItemToItem`, `getUserToUser`, `getExternal`, `GetItem`, `GetUser`, `SearchDocuments` as the master embeds it) and server/rest.go `SearchDocuments` @ add21b5

//! The master's recommendation routes under `/api/dashboard/` (`master/rest.go`).
//!
//! These differ from the server's routes in one way: they return the items
//! and users in full, not only their ids. The search over a score collection is
//! the server's `SearchDocuments` with an iteratee that loads each document, so
//! it is written here rather than reused.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::Uri;
use axum::response::Response;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_config::config::schema::ExternalConfig;
use gorse_logics::external::External;
use gorse_logics::item_to_item;
use gorse_logics::recommend::Recommender;
use gorse_logics::user_to_user;
use gorse_server::rest::recommend::{last_modified, with_last_modified};
use gorse_server::rest::response::{
    ApiError, PathParams, Query, ok, parse_int, path_param, read_categories,
};
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::data::types;
use serde::Serialize;

use crate::Master;
use crate::rest::dashboard_api::query_of;

/// The category filter the routes read: the `category` query values, or the
/// route's defaults when there are none.
fn categories_of(params: &PathParams, query: &Query, defaults: &[&str]) -> Vec<String> {
    read_categories(params, query, defaults)
}

/// `GET /api/dashboard/latest`: the newest items, sorted by timestamp, with
/// the Unix time of each as its score.
pub(crate) async fn latest(
    State(master): State<Arc<Master>>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let query = query_of(&uri);
    let categories = categories_of(&PathParams::new(), &query, &[]);
    let offset = parse_int(&query, "offset", 0)?;
    let n = parse_int(&query, "n", master.config().server.default_n)?;
    let cfg = master.config();
    let after = (cfg.recommend.data_source.item_ttl > 0)
        .then(|| gotime::now().add_date(0, 0, -(cfg.recommend.data_source.item_ttl as i64)));
    let mut items = master
        .data()
        .get_latest_items(&Context::background(), offset + n, &categories, after)
        .await
        .map_err(ApiError::internal)?;
    if (offset.max(0) as usize) < items.len() {
        items.drain(..offset.max(0) as usize);
    }
    if (n.max(0) as usize) < items.len() {
        items.truncate(n.max(0) as usize);
    }
    let scores: Vec<ScoredItemView> = items
        .into_iter()
        .map(|item| {
            let score = item.timestamp.unix() as f64;
            ScoredItemView::new(item, score)
        })
        .collect();
    // Go's `SetLastModified` reads the cache under a key made from the current
    // time. No such key exists, so Go's header is the zero time, and the port
    // makes the same read.
    let key = format!("latest/{}", gotime::now().unix());
    let header = last_modified(&master.rest, &key).await;
    ok(&scores).map(|response| with_last_modified(response, header))
}

/// `GET /api/dashboard/non-personalized/{name}`: one non-personalized
/// collection, read as the server reads it, each document loaded in full.
pub(crate) async fn non_personalized(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let name = path_param(&params, "name").to_owned();
    let query = query_of(&uri);
    let categories = categories_of(&params, &query, &[""]);
    let key = cache::key(&[cache::NON_PERSONALIZED_UPDATE_TIME, &name]);
    let header = last_modified(&master.rest, &key).await;
    let response =
        search_documents(&master, cache::NON_PERSONALIZED, &name, &categories, &query).await?;
    Ok(with_last_modified(response, header))
}

/// Go's `SearchDocuments`: the documents of one subset that match the
/// categories, with the user's read items removed, paged by `offset` and `n`.
/// Each document is loaded with `GetItem` or `GetUser` and written as that.
/// An empty result is Go's nil slice, which is `null`.
async fn search_documents(
    master: &Master,
    collection: &str,
    subset: &str,
    categories: &[String],
    query: &Query,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cfg = master.config();
    let offset = parse_int(query, "offset", 0)?;
    let n = parse_int(query, "n", cfg.server.default_n)?;
    let user_id = query.get("user-id").to_owned();
    let mut read_items: HashSet<String> = HashSet::new();
    if !user_id.is_empty() {
        let feedback = master
            .data()
            .get_user_feedback(&ctx, &user_id, Some(cfg.now()), &[])
            .await
            .map_err(ApiError::internal)?;
        read_items.extend(feedback.into_iter().map(|f| f.feedback_key.item_id));
    }
    let mut end = offset + n;
    if end > 0 && !read_items.is_empty() {
        end += read_items.len() as i64;
    }
    let mut items = master
        .cache()
        .search_scores(&ctx, collection, subset, categories, offset, end)
        .await
        .map_err(ApiError::internal)?;
    if !user_id.is_empty() {
        items.retain(|item| !read_items.contains(&item.id));
    }
    if n > 0 && items.len() as i64 > n {
        items.truncate(n as usize);
    }
    let mut results: Option<Vec<serde_json::Value>> = None;
    for item in &items {
        let value = load_score(master, item).await?;
        results.get_or_insert_with(Vec::new).push(value);
    }
    ok(&results)
}

/// Go's `GetItem`: the item of a score with the score, or an error when the
/// item is gone.
async fn load_score(master: &Master, score: &cache::Score) -> Result<serde_json::Value, ApiError> {
    let item = master
        .data()
        .get_item(&Context::background(), &score.id)
        .await
        .map_err(ApiError::internal)?;
    serde_json::to_value(ScoredItemView::new(item, score.score)).map_err(ApiError::internal)
}

/// `GET /api/dashboard/recommend/{user-id}[/{recommender}[/{name}]]`: the
/// recommendation of a user, from the whole pipeline or from one recommender
/// the path names. The items are returned in full, in the order of the scores.
pub(crate) async fn recommend(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cfg = master.config();
    let query = query_of(&uri);
    let recommender_type = path_param(&params, "recommender").to_owned();
    let recommender_name = path_param(&params, "name").to_owned();
    let user_id = path_param(&params, "user-id").to_owned();
    let categories = categories_of(&params, &query, &[]);
    let n = parse_int(&query, "n", cfg.server.default_n)?;
    let mut recommender = Recommender::new(
        cfg.recommend.clone(),
        master.cache(),
        master.data(),
        master.vectors(),
        true,
        user_id,
        categories,
    )
    .await
    .map_err(ApiError::internal)?;
    let scores = if recommender_type.is_empty() {
        recommender
            .recommend(&ctx, n)
            .await
            .map_err(ApiError::internal)?
    } else {
        let name = if recommender_name.is_empty() {
            recommender_type
        } else {
            format!("{recommender_type}/{recommender_name}")
        };
        recommender
            .recommend_sequential(&ctx, Vec::new(), n, &[name])
            .await
            .map_err(ApiError::internal)?
            .0
    };
    let ids: Vec<String> = scores.iter().map(|score| score.id.clone()).collect();
    let items = master
        .data()
        .batch_get_items(&ctx, &ids, &data::GetOptions::default())
        .await
        .map_err(ApiError::internal)?;
    let items: HashMap<String, types::Item> = items
        .into_iter()
        .map(|item| (item.item_id.clone(), item))
        .collect();
    let mut details: Vec<ScoredItemView> = Vec::with_capacity(ids.len());
    for (id, score) in ids.iter().zip(&scores) {
        match items.get(id) {
            Some(item) => details.push(ScoredItemView::new(item.clone(), score.score)),
            None => tracing::warn!("recommended item doesn't exist: {id}"),
        }
    }
    ok(&details)
}

/// `GET /api/dashboard/item-to-item/{name}/{item-id}`: the neighbours of an
/// item in the named item-to-item recommender, with the items in full.
pub(crate) async fn item_to_item(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cfg = master.config();
    let name = path_param(&params, "name").to_owned();
    let item_id = path_param(&params, "item-id").to_owned();
    let Some(item_to_item_config) = cfg.recommend.get_item_to_item_config(&name) else {
        return Err(ApiError::PageNotFound(format!(
            "item-to-item recommender {name} not found"
        )));
    };
    let query = query_of(&uri);
    let offset = parse_int(&query, "offset", 0)?;
    let n = parse_int(&query, "n", cfg.server.default_n)?;
    let limit = (offset + n).max(cfg.recommend.cache_size);
    let scores = item_to_item::query_item_to_item(
        &ctx,
        master.vectors().as_ref(),
        item_to_item_config,
        &item_id,
        &[],
        limit.max(0) as usize,
    )
    .await
    .map_err(ApiError::internal)?;
    let mut scores = gorse_server::rest::recommend::filter_visible_items_by_categories(
        &master.rest,
        scores,
        &query_categories(&query),
    )
    .await?;
    page_scores(&mut scores, offset, n);
    let mut items = Vec::with_capacity(scores.len());
    for score in &scores {
        items.push(load_score(&master, score).await?);
    }
    ok(&items)
}

/// The `category` query values, as the item-to-item route filters with them.
fn query_categories(query: &Query) -> Vec<String> {
    query
        .get_all("category")
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Go's `scores[offset:]` then `scores[:n]`, with a result past the end
/// emptied and `n` of `0` or less keeping the whole list.
fn page_scores<T>(scores: &mut Vec<T>, offset: i64, n: i64) {
    if (offset.max(0) as usize) < scores.len() {
        scores.drain(..offset.max(0) as usize);
    } else {
        scores.clear();
    }
    if n > 0 && scores.len() as i64 > n {
        scores.truncate(n as usize);
    }
}

/// `GET /api/dashboard/user-to-user/{name}/{user-id}`: the neighbours of a user
/// in the named user-to-user recommender, with the users in full.
pub(crate) async fn user_to_user(
    State(master): State<Arc<Master>>,
    Path(params): Path<PathParams>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let cfg = master.config();
    let user_id = path_param(&params, "user-id").to_owned();
    let name = path_param(&params, "name").to_owned();
    let Some(user_to_user_config) = cfg.recommend.get_user_to_user_config(&name) else {
        return Err(ApiError::PageNotFound(format!(
            "user-to-user recommender {name} not found"
        )));
    };
    let query = query_of(&uri);
    let offset = parse_int(&query, "offset", 0)?;
    let n = parse_int(&query, "n", cfg.server.default_n)?;
    let mut scores = user_to_user::query_user_to_user(
        &ctx,
        master.vectors().as_ref(),
        user_to_user_config,
        &user_id,
        (offset + n).max(0) as usize,
    )
    .await
    .map_err(ApiError::internal)?;
    page_scores(&mut scores, offset, n);
    let mut users = Vec::with_capacity(scores.len());
    for score in &scores {
        users.push(load_user_score(&master, score).await?);
    }
    ok(&users)
}

/// Go's `GetUser`: the user of a score with the score.
async fn load_user_score(
    master: &Master,
    score: &cache::Score,
) -> Result<serde_json::Value, ApiError> {
    let user = master
        .data()
        .get_user(&Context::background(), &score.id)
        .await
        .map_err(ApiError::internal)?;
    serde_json::to_value(ScoreUserView::new(user, score.score)).map_err(ApiError::internal)
}

/// `GET /api/dashboard/external?script=<base64>&user-id=`: the items an
/// external script recommends to a user, previewed from the script in the query.
pub(crate) async fn external(
    State(master): State<Arc<Master>>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let query = query_of(&uri);
    let script_base64 = query.get("script").to_owned();
    if script_base64.is_empty() {
        return Err(ApiError::BadRequest("script is required".to_owned()));
    }
    let user_id = query.get("user-id").to_owned();
    let script = base64_decode(&script_base64)
        .map_err(|err| ApiError::BadRequest(format!("invalid script encoding: {err}")))?;
    let config = ExternalConfig {
        name: "preview".to_owned(),
        script: String::from_utf8_lossy(&script).into_owned(),
    };
    // The script engine and its blocking HTTP client are built and run on a
    // blocking thread: a blocking client panics on an async worker thread.
    // A script whose list is `null` has no items, and the answer is `null`, as Go's
    // nil slice gives.
    let items = tokio::task::spawn_blocking(move || -> Result<Option<Vec<String>>, ApiError> {
        let external = External::new(&config).map_err(ApiError::internal)?;
        external.pull(&user_id).map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)??;
    // Go's `SetLastModified` reads the cache under a key made from the current
    // time, which no write sets, so the header is the zero time, as for `latest`.
    let key = format!("external/{}", gotime::now().unix());
    let header = last_modified(&master.rest, &key).await;
    ok(&items).map(|response| with_last_modified(response, header))
}

/// Go's `base64.StdEncoding.DecodeString`, with its error text.
fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| err.to_string())
}

/// Go's `ScoredItem`: an item with its score. The item's fields are flattened
/// into the object, as Go's embedding does.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScoredItemView {
    #[serde(flatten)]
    item: types::Item,
    #[serde(rename = "Score")]
    score: f64,
}

impl ScoredItemView {
    /// Pairs `item` with `score`.
    pub(crate) fn new(item: types::Item, score: f64) -> Self {
        Self { item, score }
    }
}

/// Go's `ScoreUser`: a user with its score. The user's fields are flattened into
/// the object, as Go's embedding does.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScoreUserView {
    #[serde(flatten)]
    user: types::User,
    #[serde(rename = "Score")]
    score: f64,
}

impl ScoreUserView {
    /// Pairs `user` with `score`.
    pub(crate) fn new(user: types::User, score: f64) -> Self {
        Self { user, score }
    }
}
