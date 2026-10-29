// Ports: server/rest.go (SearchDocuments, getLatest, getNonPersonalized,
// getItemToItem, FilterVisibleItemsByCategories, SearchItemToItem,
// getUserToUser, SearchUserToUser, SetLastModified, getItemNeighbors,
// getUserNeighbors, getCollaborativeFiltering, getRecommend, sessionRecommend)
// @ add21b5

//! The recommendation routes of the REST API.
//!
//! Several Go handlers return a nil list where the list is empty, and `null`
//! is what the JSON of a nil slice is. Each such handler says which case gives
//! `null` (`O-76`), and the `Option` results here carry that choice.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use gorse_common::expression::match_feedback_type_expressions;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, RFC1123};
use gorse_common::heap::TopKFilter;
use gorse_config::config::schema::{Config, ItemToItemConfig, UserToUserConfig};
use gorse_logics::item_to_item::query_item_to_item;
use gorse_logics::recommend::Recommender;
use gorse_logics::user_to_user::query_user_to_user;
use gorse_storage::cache::{self, Score};
use gorse_storage::data;
use gorse_storage::data::types::sort_feedbacks;
use gorse_storage::data::types::{Feedback as DataFeedback, FeedbackKey, Item};
use serde::{Deserialize, Serialize};

use super::RestServer;
use super::entity::read_entity;
use super::feedback::Feedback;
use super::response::{
    ApiError, PathParams, Query, ok, parse_duration, parse_int, path_param, read_categories,
};

/// Go's `cache.Score` as the REST API writes it: only `Id` and `Score` are
/// encoded, since the other fields are tagged `json:"-"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScoreBody {
    /// The document's id (`Id`).
    #[serde(rename = "Id")]
    pub id: String,
    /// The score (`Score`).
    #[serde(rename = "Score")]
    pub score: f64,
}

impl From<Score> for ScoreBody {
    fn from(score: Score) -> Self {
        Self {
            id: score.id,
            score: score.score,
        }
    }
}

/// Go's `Last-Modified` header, which `SetLastModified` sets from a cache time.
/// Go formats it with `time.RFC1123`, which writes the zone as `UTC`.
async fn last_modified(rest: &RestServer, key: &str) -> Option<HeaderValue> {
    let value = rest
        .cache()
        .get(&Context::background(), key)
        .await
        .time()
        .ok()?;
    HeaderValue::from_str(&value.format(RFC1123)).ok()
}

/// Writes `header`, when there is one, on the response, as Go's `AddHeader`
/// does before a handler writes its body. The header stays on an error response.
fn with_last_modified(response: Response, header_value: Option<HeaderValue>) -> Response {
    let mut response = response;
    if let Some(value) = header_value {
        response.headers_mut().insert(header::LAST_MODIFIED, value);
    }
    response
}

/// The result of a handler that writes a list of scores. `None` is Go's nil
/// slice, which is `null`; `Some` is written as a list, which is `[]` when
/// empty.
type ScoreList = Option<Vec<ScoreBody>>;

/// Go's `ParseInt` with `"n"` defaulting to `server.default_n`.
fn default_n(query: &Query, config: &Config) -> Result<i64, ApiError> {
    parse_int(query, "n", config.server.default_n)
}

/// Go's `filterVisibleItemsByCategories`: keeps the scores whose item is not
/// hidden and has every one of `categories`. The result is never nil.
async fn filter_visible_items_by_categories(
    rest: &RestServer,
    scores: Vec<Score>,
    categories: &[String],
) -> Result<Vec<Score>, ApiError> {
    let ids: Vec<String> = scores.iter().map(|score| score.id.clone()).collect();
    let items = rest
        .data()
        .batch_get_items(&Context::background(), &ids, &data::GetOptions::default())
        .await
        .map_err(ApiError::internal)?;
    let visible: HashSet<String> = items
        .iter()
        .filter(|item| !item.is_hidden && has_categories(item, categories))
        .map(|item| item.item_id.clone())
        .collect();
    Ok(scores
        .into_iter()
        .filter(|score| visible.contains(&score.id))
        .collect())
}

/// Whether an item has every category in `categories`, which is true when
/// there are none.
fn has_categories(item: &Item, categories: &[String]) -> bool {
    let own = item.categories.as_deref().unwrap_or(&[]);
    categories
        .iter()
        .all(|category| own.iter().any(|existing| existing == category))
}

/// The item ids of a user's feedback, which Go keeps in a set: the items the
/// user has read.
async fn read_items(rest: &RestServer, user_id: &str) -> Result<HashSet<String>, ApiError> {
    if user_id.is_empty() {
        return Ok(HashSet::new());
    }
    let feedback = rest
        .data()
        .get_user_feedback(
            &Context::background(),
            user_id,
            Some(rest.config().now()),
            &[],
        )
        .await
        .map_err(ApiError::internal)?;
    Ok(feedback
        .into_iter()
        .map(|feedback| feedback.feedback_key.item_id)
        .collect())
}

/// Go's `SearchDocuments`: a page of the scores of `subset` in `collection`,
/// without the items the user has read. The result is `null` only when the
/// cache returns nothing and no user is given.
async fn search_documents(
    rest: &RestServer,
    collection: &str,
    subset: &str,
    categories: &[String],
    query: &Query,
) -> Result<Response, ApiError> {
    let config = rest.config();
    let offset = parse_int(query, "offset", 0)?;
    let n = default_n(query, &config)?;
    let user_id = query.get("user-id");
    let read = read_items(rest, user_id).await?;
    let mut end = offset + n;
    if end > 0 && !read.is_empty() {
        end += read.len() as i64;
    }
    let items = rest
        .cache()
        .search_scores(
            &Context::background(),
            collection,
            subset,
            categories,
            offset,
            end,
        )
        .await
        .map_err(ApiError::internal)?;
    let mut items: Vec<Score> = if user_id.is_empty() {
        items
    } else {
        items
            .into_iter()
            .filter(|item| !read.contains(&item.id))
            .collect()
    };
    if n > 0 && items.len() as i64 > n {
        items.truncate(n as usize);
    }
    let body: Vec<ScoreBody> = items.into_iter().map(ScoreBody::from).collect();
    ok(&body)
}

/// Go's `getLatest`: `GET /api/latest` and `GET /api/latest/{category}`.
pub(crate) async fn get_latest(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let config = rest.config();
    let offset = parse_int(&query, "offset", 0).map_err(|err| {
        ApiError::BadRequest(format!("invalid offset parameter: {}", err.message()))
    })?;
    let n = parse_int(&query, "n", config.server.default_n)
        .map_err(|err| ApiError::BadRequest(format!("invalid n parameter: {}", err.message())))?;
    let categories = read_categories(&params, &query, &[]);
    let user_id = query.get("user-id");
    let read = read_items(&rest, user_id).await?;
    let mut limit = offset + n;
    if !read.is_empty() {
        limit += read.len() as i64;
    }
    let after = if config.recommend.data_source.item_ttl > 0 {
        Some(gotime::now().add_date(
            0,
            0,
            -i64::try_from(config.recommend.data_source.item_ttl).unwrap_or(i64::MAX),
        ))
    } else {
        None
    };
    let mut items = rest
        .data()
        .get_latest_items(&Context::background(), limit, &categories, after)
        .await
        .map_err(ApiError::internal)?;
    if !read.is_empty() {
        items.retain(|item| !read.contains(&item.item_id));
    }
    let skip = usize::try_from(offset).unwrap_or(0).min(items.len());
    items.drain(..skip);
    if n > 0 && items.len() as i64 > n {
        items.truncate(n as usize);
    }
    let scores: Vec<ScoreBody> = items
        .iter()
        .map(|item| ScoreBody {
            id: item.item_id.clone(),
            score: item.timestamp.unix() as f64,
        })
        .collect();
    ok(&scores)
}

/// Go's `getNonPersonalized`: `GET /api/non-personalized/{name}`.
pub(crate) async fn get_non_personalized(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let name = path_param(&params, "name");
    let categories = read_categories(&params, &query, &[""]);
    let header_value = last_modified(
        &rest,
        &cache::key(&[cache::NON_PERSONALIZED_UPDATE_TIME, name]),
    )
    .await;
    let response = search_documents(&rest, cache::NON_PERSONALIZED, name, &categories, &query)
        .await
        .unwrap_or_else(IntoResponse::into_response);
    Ok(with_last_modified(response, header_value))
}

/// Go's `getItemToItem`: `GET /api/item-to-item/{name}/{item-id}`.
pub(crate) async fn get_item_to_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let name = path_param(&params, "name");
    let item_id = path_param(&params, "item-id");
    let categories = read_categories(&params, &query, &[]);
    let config = rest.config();
    let Some(item_to_item) = config.recommend.get_item_to_item_config(name) else {
        return Err(ApiError::PageNotFound(format!(
            "item-to-item recommender {name} not found"
        )));
    };
    search_item_to_item(&rest, item_to_item, item_id, &categories, &query).await
}

/// Go's `SearchItemToItem`: the items nearest to `item_id` in the collection of
/// `item_to_item`, without the items the user has read.
async fn search_item_to_item(
    rest: &RestServer,
    item_to_item: &ItemToItemConfig,
    item_id: &str,
    categories: &[String],
    query: &Query,
) -> Result<Response, ApiError> {
    let config = rest.config();
    let offset = parse_int(query, "offset", 0)?;
    let n = default_n(query, &config)?;
    let read = read_items(rest, query.get("user-id")).await?;
    let wanted = (offset + n + read.len() as i64).max(config.recommend.cache_size);
    let results = query_item_to_item(
        &Context::background(),
        rest.vectors().as_ref(),
        item_to_item,
        item_id,
        &[],
        usize::try_from(wanted).unwrap_or(0),
    )
    .await
    .map_err(ApiError::internal)?;
    let results = filter_visible_items_by_categories(rest, results, categories).await?;
    let mut results: Vec<Score> = results
        .into_iter()
        .filter(|result| !read.contains(&result.id))
        .collect();
    let results = page(&mut results, offset, n);
    ok(&results)
}

/// Go's paging of a list in `getItemToItem`, `getUserToUser` and their
/// neighbours: `results[offset:]`, then `[:n]`, where an offset past the end
/// makes the list nil (`null`).
fn page(results: &mut Vec<Score>, offset: i64, n: i64) -> ScoreList {
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    if offset >= results.len() {
        return None;
    }
    let mut paged: Vec<ScoreBody> = results.drain(offset..).map(ScoreBody::from).collect();
    if n > 0 && paged.len() as i64 > n {
        paged.truncate(n as usize);
    }
    Some(paged)
}

/// Go's `getUserToUser`: `GET /api/user-to-user/{name}/{user-id}`.
pub(crate) async fn get_user_to_user(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let name = path_param(&params, "name");
    let user_id = path_param(&params, "user-id");
    let config = rest.config();
    let Some(user_to_user) = config.recommend.get_user_to_user_config(name) else {
        return Err(ApiError::PageNotFound(format!(
            "user-to-user recommender {name} not found"
        )));
    };
    search_user_to_user(&rest, user_to_user, user_id, &query).await
}

/// Go's `SearchUserToUser`: the users nearest to `user_id` in the collection of
/// `user_to_user`.
async fn search_user_to_user(
    rest: &RestServer,
    user_to_user: &UserToUserConfig,
    user_id: &str,
    query: &Query,
) -> Result<Response, ApiError> {
    let config = rest.config();
    let offset = parse_int(query, "offset", 0)?;
    let n = default_n(query, &config)?;
    let wanted = usize::try_from(offset + n).unwrap_or(0);
    let mut results = query_user_to_user(
        &Context::background(),
        rest.vectors().as_ref(),
        user_to_user,
        user_id,
        wanted,
    )
    .await
    .map_err(ApiError::internal)?;
    let results = page(&mut results, offset, n);
    ok(&results)
}

/// Go's `getItemNeighbors`: `GET /api/item/{item-id}/neighbors/` and
/// `GET /api/item/{item-id}/neighbors/{category}`, the first item-to-item
/// recommender.
pub(crate) async fn get_item_neighbors(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let item_id = path_param(&params, "item-id");
    let categories = read_categories(&params, &query, &[]);
    let config = rest.config();
    let Some(first) = config
        .recommend
        .item_to_item
        .as_deref()
        .and_then(<[_]>::first)
    else {
        return Err(ApiError::PageNotFound(
            "item-to-item recommendation is not enabled".to_owned(),
        ));
    };
    search_item_to_item(&rest, first, item_id, &categories, &query).await
}

/// Go's `getUserNeighbors`: `GET /api/user/{user-id}/neighbors/`, the first
/// user-to-user recommender.
pub(crate) async fn get_user_neighbors(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = Query::parse(raw.as_deref());
    let user_id = path_param(&params, "user-id");
    let config = rest.config();
    let Some(first) = config
        .recommend
        .user_to_user
        .as_deref()
        .and_then(<[_]>::first)
    else {
        return Err(ApiError::PageNotFound(
            "user-to-user recommendation is not enabled".to_owned(),
        ));
    };
    search_user_to_user(&rest, first, user_id, &query).await
}

/// Go's `getCollaborativeFiltering`: `GET /api/collaborative-filtering/{user-id}`
/// and its deprecated `/{category}` form.
pub(crate) async fn get_collaborative_filtering(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let config = rest.config();
    if config
        .recommend
        .collaborative
        .kind
        .eq_ignore_ascii_case("none")
    {
        return Err(ApiError::PageNotFound(
            "collaborative filtering recommendation is disabled".to_owned(),
        ));
    }
    let query = Query::parse(raw.as_deref());
    let user_id = path_param(&params, "user-id");
    let categories = read_categories(&params, &query, &[]);
    let header_value =
        last_modified(&rest, &cache::key(&[cache::RECOMMEND_UPDATE_TIME, user_id])).await;
    let response = search_documents(&rest, cache::RECOMMEND, user_id, &categories, &query)
        .await
        .unwrap_or_else(IntoResponse::into_response);
    Ok(with_last_modified(response, header_value))
}

/// Go's `getRecommend`: `GET /api/recommend/{user-id}` and its deprecated
/// `/{category}` form. A write-back type stores the returned items as feedback.
pub(crate) async fn get_recommend(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    headers: axum::http::HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let user_id = path_param(&params, "user-id").to_owned();
    let api_version = headers
        .get("X-API-Version")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_owned();
    let query = Query::parse(raw.as_deref());
    let n = default_n(&query, &config)?;
    let categories = read_categories(&params, &query, &[]);
    let offset = parse_int(&query, "offset", 0)?;
    let write_back_type = query.get("write-back-type").to_owned();
    let write_back_delay = parse_duration(&query, "write-back-delay")?;
    let mut recommender = Recommender::new(
        config.recommend.clone(),
        rest.cache(),
        rest.data(),
        rest.vectors(),
        true,
        user_id.clone(),
        categories,
    )
    .await
    .map_err(ApiError::internal)?;
    let scores = recommender
        .recommend(&ctx, n + offset)
        .await
        .map_err(ApiError::internal)?;
    let scores: Vec<Score> = if scores.len() as i64 > offset {
        scores.into_iter().skip(offset as usize).collect()
    } else {
        Vec::new()
    };
    let results: Vec<String> = scores.iter().map(|score| score.id.clone()).collect();
    if !write_back_type.is_empty() {
        let start_time = gotime::now();
        for item_id in &results {
            let feedback = DataFeedback {
                feedback_key: FeedbackKey {
                    feedback_type: write_back_type.clone(),
                    user_id: user_id.clone(),
                    item_id: item_id.clone(),
                },
                timestamp: start_time.add(write_back_delay),
                ..Default::default()
            };
            rest.data()
                .batch_insert_feedback(&ctx, &[feedback], false, false, false)
                .await
                .map_err(ApiError::internal)?;
        }
    }
    if api_version == "2" {
        let scores: Vec<ScoreBody> = scores.into_iter().map(ScoreBody::from).collect();
        return ok(&scores);
    }
    ok(&results)
}

/// The body Go's `sessionRecommend` takes: the session's feedback.
type SessionFeedback = Vec<Feedback>;

/// Go's `sessionRecommend`: `POST /api/session/recommend` and its deprecated
/// `/{category}` form. The items similar to the session's positive feedback
/// are ranked by their summed similarity.
pub(crate) async fn session_recommend(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<PathParams>,
    RawQuery(raw): RawQuery,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let Some(item_to_item) = config
        .recommend
        .item_to_item
        .as_deref()
        .and_then(<[_]>::first)
    else {
        return Err(ApiError::PageNotFound(
            "item-to-item recommendation is not enabled".to_owned(),
        ));
    };
    let feedbacks: SessionFeedback = read_entity(&headers, &body)?;
    let query = Query::parse(raw.as_deref());
    let n = default_n(&query, &config)?;
    let category = path_param(&params, "category");
    let categories: Vec<String> = if category.is_empty() {
        Vec::new()
    } else {
        vec![category.to_owned()]
    };
    let offset = parse_int(&query, "offset", 0)?;

    let mut data_feedback = Vec::with_capacity(feedbacks.len());
    for feedback in &feedbacks {
        data_feedback.push(feedback.to_data_feedback()?);
    }
    sort_feedbacks(&mut data_feedback);

    let mut exclude: HashSet<String> = HashSet::new();
    let mut user_feedback: Vec<DataFeedback> = Vec::new();
    let positive = config
        .recommend
        .data_source
        .positive_feedback_types
        .as_deref()
        .unwrap_or(&[]);
    for feedback in data_feedback {
        exclude.insert(feedback.feedback_key.item_id.clone());
        if match_feedback_type_expressions(
            positive,
            &feedback.feedback_key.feedback_type,
            feedback.value,
        ) {
            user_feedback.push(feedback);
        }
    }

    // Go ranges over a map here, so ties come out in an order the program does
    // not fix. A sorted map gives the same ranking every time.
    let mut candidates: BTreeMap<String, f64> = BTreeMap::new();
    let mut used_feedback_count = 0_i64;
    for feedback in &user_feedback {
        let similar = query_item_to_item(
            &ctx,
            rest.vectors().as_ref(),
            item_to_item,
            &feedback.feedback_key.item_id,
            &[],
            usize::try_from(config.recommend.cache_size).unwrap_or(0),
        )
        .await
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
        let similar = filter_visible_items_by_categories(&rest, similar, &categories).await?;
        for item in &similar {
            if !exclude.contains(&item.id) {
                *candidates.entry(item.id.clone()).or_insert(0.0) += item.score;
            }
        }
        if !similar.is_empty() {
            used_feedback_count += 1;
            if used_feedback_count >= config.recommend.context_size {
                break;
            }
        }
    }

    let k = usize::try_from(n + offset).unwrap_or(0);
    let mut filter = TopKFilter::new(k);
    for (id, score) in candidates {
        filter.push(id, score);
    }
    let mut result: ScoreList = Some(
        filter
            .pop_all()
            .into_iter()
            .map(|elem| ScoreBody {
                id: elem.value,
                score: elem.weight,
            })
            .collect(),
    );
    // Go's `result[offset:]` is nil when the offset reaches the end, and a
    // nil list stays nil through the truncation to `n`.
    let offset = usize::try_from(offset).unwrap_or(0);
    result = match result {
        Some(mut list) if list.len() > offset => {
            list.drain(..offset);
            Some(list)
        }
        _ => None,
    };
    if let Some(list) = result.as_mut() {
        let keep = usize::try_from(n.max(0)).unwrap_or(0).min(list.len());
        list.truncate(keep);
    }
    ok(&result)
}
