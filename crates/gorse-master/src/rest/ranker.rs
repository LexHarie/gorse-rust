// Ports: master/rest.go (`getRankerPrompt`, `RerankerPrompt`) and logics/reranker.go (`FeedbackItem`) @ add21b5

//! The ranker prompt the dashboard previews (`master/rest.go`
//! `getRankerPrompt`). Go renders two gonja templates, a query and a document
//! template, with the user, the feedback and the items. gonja is Jinja2, so the
//! templates are rendered with minijinja (`porting/crate-selection.md`).

use std::sync::Arc;

use axum::extract::State;
use axum::http::Uri;
use axum::response::Response;
use base64::Engine as _;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_server::rest::response::{ApiError, Query, ok};
use gorse_storage::data;
use gorse_storage::data::types;
use minijinja::{Environment, Value, context};
use serde::Serialize;

use crate::Master;
use crate::rest::dashboard_api::{not_found_or_internal, query_of};

/// Go's `master.RerankerPrompt`.
#[derive(Debug, Serialize)]
pub(crate) struct RerankerPrompt {
    /// The rendered query (`Query`).
    #[serde(rename = "query")]
    query: String,
    /// The rendered documents, one per latest item (`Documents`).
    #[serde(rename = "documents")]
    documents: Vec<String>,
}

/// Go's `logics.FeedbackItem`: a feedback type with the item it is on.
#[derive(Debug, Serialize)]
struct FeedbackItem {
    #[serde(rename = "FeedbackType")]
    feedback_type: String,
    #[serde(rename = "Item")]
    item: types::Item,
}

/// The base64 text the route reads a template from, decoded as Go's
/// `base64.StdEncoding` decodes it, with its error text.
fn decode_template(name: &str, text: &str) -> Result<String, ApiError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| ApiError::BadRequest(format!("invalid {name} encoding: {err}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `GET /api/dashboard/ranker/prompt`: renders the query for a user and one
/// document for each of the 100 latest items.
pub(crate) async fn prompt(
    State(master): State<Arc<Master>>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let query: Query = query_of(&uri);
    let query_text = query.get("query-template").to_owned();
    let document_text = query.get("document-template").to_owned();
    if query_text.is_empty() || document_text.is_empty() {
        return Err(ApiError::BadRequest(
            "query-template and document-template are required".to_owned(),
        ));
    }
    let user_id = query.get("user-id").to_owned();
    if user_id.is_empty() {
        return Err(ApiError::BadRequest("user-id is required".to_owned()));
    }
    let query_template = decode_template("query-template", &query_text)?;
    let document_template = decode_template("document-template", &document_text)?;
    let mut env = Environment::new();
    env.add_template("query", &query_template)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    env.add_template("document", &document_template)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    let ctx = Context::background();
    let cfg = master.config();
    let user = master
        .data()
        .get_user(&ctx, &user_id)
        .await
        .map_err(|err| not_found_or_internal(&err))?;
    let mut feedback = master
        .data()
        .get_user_feedback(
            &ctx,
            &user_id,
            Some(cfg.now()),
            cfg.recommend
                .data_source
                .positive_feedback_types
                .as_deref()
                .unwrap_or_default(),
        )
        .await
        .map_err(ApiError::internal)?;
    types::sort_feedbacks(&mut feedback);
    feedback.truncate(10);
    let mut items_by_id = std::collections::HashMap::new();
    if !feedback.is_empty() {
        let ids: Vec<String> = feedback
            .iter()
            .map(|f| f.feedback_key.item_id.clone())
            .collect();
        let items = master
            .data()
            .batch_get_items(&ctx, &ids, &data::GetOptions::default())
            .await
            .map_err(ApiError::internal)?;
        items_by_id = items
            .into_iter()
            .map(|item| (item.item_id.clone(), item))
            .collect();
    }
    let feedback_items: Vec<FeedbackItem> = feedback
        .iter()
        .filter_map(|f| {
            items_by_id
                .get(&f.feedback_key.item_id)
                .map(|item| FeedbackItem {
                    feedback_type: f.feedback_key.feedback_type.clone(),
                    item: item.clone(),
                })
        })
        .collect();
    let latest_after = (cfg.recommend.data_source.item_ttl > 0)
        .then(|| gotime::now().add_date(0, 0, -(cfg.recommend.data_source.item_ttl as i64)));
    let latest = master
        .data()
        .get_latest_items(&ctx, 100, &[], latest_after)
        .await
        .map_err(ApiError::internal)?;

    let user_value = template_value(&user)?;
    let feedback_value = template_value(&feedback_items)?;
    let rendered_query = env
        .get_template("query")
        .and_then(|template| {
            template.render(context! { user => user_value, feedback => feedback_value })
        })
        .map_err(ApiError::internal)?;
    let mut documents = Vec::with_capacity(latest.len());
    for item in &latest {
        let item_value = template_value(item)?;
        let document = env
            .get_template("document")
            .and_then(|template| template.render(context! { item => item_value }))
            .map_err(ApiError::internal)?;
        documents.push(document);
    }
    ok(&RerankerPrompt {
        query: rendered_query,
        documents,
    })
}

/// Converts a value into the template's value through its JSON form, so the
/// template reads its fields by their JSON names.
fn template_value<T: Serialize>(value: &T) -> Result<Value, ApiError> {
    serde_json::to_value(value)
        .map(json_to_template)
        .map_err(ApiError::internal)
}

/// Converts a JSON value into the template's value, field by field.
fn json_to_template(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::from(()),
        serde_json::Value::Bool(flag) => Value::from(flag),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(int) => Value::from(int),
            None => Value::from(number.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(text) => Value::from(text),
        serde_json::Value::Array(items) => {
            Value::from(items.into_iter().map(json_to_template).collect::<Vec<_>>())
        }
        serde_json::Value::Object(fields) => Value::from_pairs(
            fields
                .into_iter()
                .map(|(key, value)| (key, json_to_template(value))),
        ),
    }
}
