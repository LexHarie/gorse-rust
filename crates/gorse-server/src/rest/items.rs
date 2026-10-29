// Ports: server/rest.go (Item, batchInsertItems, insertItems, insertItem,
// modifyItem, ItemIterator, getItems, searchEmbedding, searchFullText, getItem,
// deleteItem, insertItemCategory, deleteItemCategory) @ add21b5

//! The item routes of the REST API.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::response::Response;
use gorse_common::dateparse;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_storage::cache::{self, ScoreCondition, ScorePatch};
use gorse_storage::data::types::{Item as DataItem, ItemPatch, Labels, validate_labels};
use serde::{Deserialize, Serialize};

use super::RestServer;
use super::embedding::create_embedding;
use super::quota;
use super::response::{ApiError, Query, ok, parse_int, path_param, read_entity, with_wild_card};
use super::users::{Success, empty_ok, not_found_or_internal};
use gorse_storage::vectors::{self, Vector};

/// Go's `Item`: the item as the REST API takes it, with its time stamp as text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct Item {
    /// The item id (`ItemId`).
    pub item_id: String,
    /// Whether the item is hidden (`IsHidden`).
    pub is_hidden: bool,
    /// The item's categories (`Categories`).
    pub categories: Option<Vec<String>>,
    /// The time stamp, in any format `dateparse` reads (`Timestamp`).
    pub timestamp: String,
    /// The item's labels (`Labels`).
    pub labels: Labels,
    /// The item's comment (`Comment`).
    pub comment: String,
}

/// Go's `ItemIterator`: one page of items and the cursor for the next page.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ItemIterator {
    /// The cursor for the next page, empty at the end (`Cursor`).
    #[serde(rename = "Cursor")]
    pub cursor: String,
    /// The items of this page (`Items`).
    #[serde(rename = "Items")]
    pub items: Vec<DataItem>,
}

/// The item cache's collections, Go's `cache.ItemCache`.
fn item_cache() -> Vec<String> {
    cache::ITEM_CACHE.iter().map(|name| (*name).to_owned()).collect()
}

/// Go's `batchInsertItems`: writes the items to the cache and the data store,
/// and stamps each item's modification time.
async fn batch_insert_items(rest: &RestServer, items: Vec<Item>) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let item_ids: Vec<String> = items.iter().map(|item| item.item_id.clone()).collect();
    // Go loads the existing items here and checks the count against the
    // quota. The loaded items are not used again.
    rest.data()
        .batch_get_items(&ctx, &item_ids, &Default::default())
        .await
        .map_err(ApiError::internal)?;
    quota::check_item_count_limit(rest, &item_ids).await?;
    let mut data_items = Vec::with_capacity(items.len());
    for item in items {
        let timestamp = if item.timestamp.is_empty() {
            Time::default()
        } else {
            dateparse::parse_any(&item.timestamp, &[])
                .map_err(|err| ApiError::BadRequest(err.to_string()))?
        };
        let data_item = DataItem {
            item_id: item.item_id.clone(),
            is_hidden: item.is_hidden,
            categories: item.categories.clone(),
            timestamp,
            update_at: Time::default(),
            labels: item.labels.clone(),
            comment: item.comment.clone(),
        };
        quota::check_item_size(&config, &data_item).map_err(ApiError::TooManyRequests)?;
        data_items.push(data_item);
        let patch = ScorePatch {
            is_hidden: Some(item.is_hidden),
            categories: Some(with_wild_card(item.categories.as_deref().unwrap_or(&[]))),
            score: None,
        };
        rest.cache()
            .update_scores(&ctx, &item_cache(), None, &item.item_id, &patch)
            .await
            .map_err(ApiError::internal)?;
    }
    let count = data_items.len() as i64;
    rest.data()
        .batch_insert_items(&ctx, &data_items)
        .await
        .map_err(ApiError::internal)?;
    let now = gotime::now();
    let values: Vec<cache::Value> = data_items
        .iter()
        .map(|item| cache::time(cache::key(&[cache::LAST_MODIFY_ITEM_TIME, &item.item_id]), now.clone()))
        .collect();
    rest.cache()
        .set(&ctx, &values)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(count, "batch insert items");
    ok(&super::users::Success { row_affected: count })
}

/// Go's `insertItems`: `POST /api/items`.
pub(crate) async fn insert_items(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let items: Vec<Item> = read_entity(&body)?;
    for item in &items {
        validate_labels(&item.labels).map_err(|err| ApiError::BadRequest(err.to_string()))?;
    }
    batch_insert_items(&rest, items).await
}

/// Go's `insertItem`: `POST /api/item`.
pub(crate) async fn insert_item(
    State(rest): State<Arc<RestServer>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let item: Item = read_entity(&body)?;
    validate_labels(&item.labels).map_err(|err| ApiError::BadRequest(err.to_string()))?;
    batch_insert_items(&rest, vec![item]).await
}

/// Go's `modifyItem`: `PATCH /api/item/{item-id}`. A failure of the timestamp
/// write is not reported, as in Go: the response is empty.
pub(crate) async fn modify_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let item_id = path_param(&params, "item-id").to_owned();
    let patch: ItemPatch = read_entity(&body)?;
    validate_labels(&patch.labels).map_err(|err| ApiError::BadRequest(err.to_string()))?;
    quota::check_labels_size(&config, &patch.labels).map_err(ApiError::TooManyRequests)?;
    if let Some(comment) = &patch.comment {
        quota::check_comment_size(&config, comment).map_err(ApiError::TooManyRequests)?;
    }
    if let Some(categories) = &patch.categories {
        quota::check_categories_size(&config, categories).map_err(ApiError::TooManyRequests)?;
    }
    if patch.is_hidden.is_some() {
        let score_patch = ScorePatch {
            is_hidden: patch.is_hidden,
            ..Default::default()
        };
        rest.cache()
            .update_scores(&ctx, &item_cache(), None, &item_id, &score_patch)
            .await
            .map_err(ApiError::internal)?;
    }
    if let Some(categories) = &patch.categories {
        let score_patch = ScorePatch {
            categories: Some(with_wild_card(categories)),
            ..Default::default()
        };
        rest.cache()
            .update_scores(&ctx, &item_cache(), None, &item_id, &score_patch)
            .await
            .map_err(ApiError::internal)?;
    }
    rest.data()
        .modify_item(&ctx, &item_id, &patch)
        .await
        .map_err(ApiError::internal)?;
    let timestamp = cache::time(cache::key(&[cache::LAST_MODIFY_ITEM_TIME, &item_id]), gotime::now());
    if rest.cache().set(&ctx, &[timestamp]).await.is_err() {
        return Ok(empty_ok());
    }
    ok(&Success { row_affected: 1 })
}

/// Go's `getItems`: `GET /api/items`. A non-empty `q` searches the items, by
/// an embedding index when `index` is set and by full text otherwise.
pub(crate) async fn get_items(
    State(rest): State<Arc<RestServer>>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let query = Query::parse(raw.as_deref());
    let n = parse_int(&query, "n", rest.config().server.default_n)?;
    let search = query.get("q");
    if !search.is_empty() {
        let index = query.get("index");
        return if index.is_empty() {
            search_full_text(&rest, search, n).await
        } else {
            search_embedding(&rest, search, index, n).await
        };
    }
    let (cursor, items) = rest
        .data()
        .get_items(&ctx, query.get("cursor"), n, None)
        .await
        .map_err(ApiError::internal)?;
    ok(&ItemIterator { cursor, items })
}

/// Go's `searchEmbedding`: embeds `query` with the OpenAI model and returns the
/// visible items nearest to it in the index's collection.
async fn search_embedding(
    rest: &RestServer,
    query: &str,
    index: &str,
    n: i64,
) -> Result<Response, ApiError> {
    let config = rest.config();
    let index_config = config.recommend.get_item_to_item_config(index).ok_or_else(|| {
        ApiError::BadRequest(format!("item search index {index} not found"))
    })?;
    if index_config.kind != "embedding" {
        return Err(ApiError::BadRequest(format!(
            "item search index {index} must be an embedding item-to-item recommender"
        )));
    }
    if n <= 0 {
        return ok(&ItemIterator::default());
    }
    let embedding = create_embedding(
        &config.openai.base_url,
        &config.openai.auth_token,
        &config.openai.embedding_model,
        config.openai.embedding_dimensions,
        query,
    )
    .await
    .map_err(ApiError::Internal)?;
    if embedding.is_empty() {
        return Err(ApiError::Internal(
            "embedding response contains no embedding".to_owned(),
        ));
    }
    let ctx = Context::background();
    let results = rest
        .vectors()
        .query_vectors(
            &ctx,
            &vectors::item_to_item_collection(index),
            &Vector {
                values: embedding,
                ..Default::default()
            },
            &[],
            n,
        )
        .await
        .map_err(ApiError::internal)?;
    let ids: Vec<String> = results.iter().map(|result| result.vector.id.clone()).collect();
    let items = rest
        .data()
        .batch_get_items(&ctx, &ids, &Default::default())
        .await
        .map_err(ApiError::internal)?;
    let by_id: HashMap<&str, &DataItem> = items
        .iter()
        .map(|item| (item.item_id.as_str(), item))
        .collect();
    let ordered: Vec<DataItem> = results
        .iter()
        .filter_map(|result| by_id.get(result.vector.id.as_str()))
        .filter(|item| !item.is_hidden)
        .map(|item| (*item).clone())
        .collect();
    ok(&ItemIterator {
        cursor: String::new(),
        items: ordered,
    })
}

/// Go's `searchFullText`: the items the data store matches to `query`.
async fn search_full_text(rest: &RestServer, query: &str, n: i64) -> Result<Response, ApiError> {
    let config = rest.config();
    if config.recommend.search.columns.as_ref().is_none_or(Vec::is_empty) {
        return Err(ApiError::BadRequest(
            "item search is not supported because [recommend.search].columns is empty"
                .to_owned(),
        ));
    }
    let scored = rest
        .data()
        .search_items(&Context::background(), query, n)
        .await
        .map_err(ApiError::internal)?;
    let items = scored.into_iter().map(|scored| scored.item).collect();
    ok(&ItemIterator {
        cursor: String::new(),
        items,
    })
}

/// Go's `getItem`: `GET /api/item/{item-id}`.
pub(crate) async fn get_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let item_id = path_param(&params, "item-id");
    let item = rest
        .data()
        .get_item(&Context::background(), item_id)
        .await
        .map_err(|err| not_found_or_internal(&err))?;
    ok(&item)
}

/// Go's `deleteItem`: `DELETE /api/item/{item-id}`.
pub(crate) async fn delete_item(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let item_id = path_param(&params, "item-id");
    rest.data()
        .delete_item(&ctx, item_id)
        .await
        .map_err(ApiError::internal)?;
    let condition = ScoreCondition {
        id: Some(item_id.to_owned()),
        ..Default::default()
    };
    rest.cache()
        .delete_scores(&ctx, &item_cache(), &condition)
        .await
        .map_err(ApiError::internal)?;
    ok(&Success { row_affected: 1 })
}

/// Go's `insertItemCategory`: `PUT /api/item/{item-id}/category/{category}`.
pub(crate) async fn insert_item_category(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let config = rest.config();
    let item_id = path_param(&params, "item-id");
    let category = path_param(&params, "category");
    let mut item = rest
        .data()
        .get_item(&ctx, item_id)
        .await
        .map_err(ApiError::internal)?;
    let categories = item.categories.get_or_insert_with(Vec::new);
    if !categories.iter().any(|existing| existing == category) {
        categories.push(category.to_owned());
    }
    quota::check_item_size(&config, &item).map_err(ApiError::TooManyRequests)?;
    rest.data()
        .batch_insert_items(&ctx, std::slice::from_ref(&item))
        .await
        .map_err(ApiError::internal)?;
    let patch = ScorePatch {
        categories: Some(with_wild_card(item.categories.as_deref().unwrap_or(&[]))),
        ..Default::default()
    };
    rest.cache()
        .update_scores(&ctx, &item_cache(), None, item_id, &patch)
        .await
        .map_err(ApiError::internal)?;
    ok(&Success { row_affected: 1 })
}

/// Go's `deleteItemCategory`: `DELETE /api/item/{item-id}/category/{category}`.
pub(crate) async fn delete_item_category(
    State(rest): State<Arc<RestServer>>,
    Path(params): Path<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let ctx = Context::background();
    let item_id = path_param(&params, "item-id");
    let category = path_param(&params, "category");
    let mut item = rest
        .data()
        .get_item(&ctx, item_id)
        .await
        .map_err(ApiError::internal)?;
    let categories: Vec<String> = item
        .categories
        .take()
        .unwrap_or_default()
        .into_iter()
        .filter(|existing| existing != category)
        .collect();
    item.categories = Some(categories.clone());
    let patch = ScorePatch {
        categories: Some(with_wild_card(&categories)),
        ..Default::default()
    };
    rest.cache()
        .update_scores(&ctx, &item_cache(), None, item_id, &patch)
        .await
        .map_err(ApiError::internal)?;
    rest.data()
        .batch_insert_items(&ctx, std::slice::from_ref(&item))
        .await
        .map_err(ApiError::internal)?;
    ok(&Success { row_affected: 1 })
}
