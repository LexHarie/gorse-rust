// Ports: master/rest.go (`getConfig`, `postConfig`, `deleteConfig`, `getConfigSchema`, `formatConfig`) and config/config.go (the `mapstructure` names and the `Duration` display) @ add21b5

//! The configuration routes of the dashboard (`master/rest.go`).
//!
//! The GET route serves the configuration with the `mapstructure` names, as
//! Go's `mapstructure.Decode` writes them ([`config_map`]). The POST route
//! decodes a configuration with the same names and replaces the recommendation
//! settings. The schema the GET schema route serves is the frozen
//! `contracts/config/config.schema.json`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gojson::{self, Value as GoValue};
use gorse_common::gotime::Duration;
use gorse_config::config::schema::Config;
use gorse_config::mapstructure::error_text;
use gorse_config::value::{Map as ConfigMap, Value as ConfigValue};
use gorse_server::rest::response::{ApiError, ok};
use gorse_storage::meta;
use serde_json::{Map, Value};

use super::config_map::config_map;
use crate::Master;

/// The frozen JSON Schema of the configuration (`config.schema.json`), which
/// the schema route serves.
const SCHEMA: &str = include_str!("../../../../contracts/config/config.schema.json");

/// A value of the configuration as the dashboard's JSON map holds it: the
/// `mapstructure` form of a struct, a durations as Go writes it in
/// `formatConfig`, and every scalar as itself.
pub(crate) trait ToConfigValue {
    /// The JSON value of this part of the configuration.
    fn to_config_value(&self) -> Value;
}

impl ToConfigValue for String {
    fn to_config_value(&self) -> Value {
        Value::String(self.clone())
    }
}

impl ToConfigValue for bool {
    fn to_config_value(&self) -> Value {
        Value::Bool(*self)
    }
}

impl ToConfigValue for i64 {
    fn to_config_value(&self) -> Value {
        Value::from(*self)
    }
}

impl ToConfigValue for u64 {
    fn to_config_value(&self) -> Value {
        Value::from(*self)
    }
}

impl ToConfigValue for f64 {
    fn to_config_value(&self) -> Value {
        serde_json::Number::from_f64(*self).map_or(Value::Null, Value::Number)
    }
}

impl ToConfigValue for Duration {
    /// Go's `time.Duration` after `formatConfig`: the duration's `String`,
    /// without a trailing `m0s` or `h0m`.
    fn to_config_value(&self) -> Value {
        Value::String(format_duration(&self.to_string()))
    }
}

impl<T: ToConfigValue> ToConfigValue for Option<T> {
    /// A nil Go slice, which the `None` stands for, is `null`.
    fn to_config_value(&self) -> Value {
        self.as_ref()
            .map_or(Value::Null, ToConfigValue::to_config_value)
    }
}

impl<T: ToConfigValue> ToConfigValue for Vec<T> {
    fn to_config_value(&self) -> Value {
        Value::Array(self.iter().map(ToConfigValue::to_config_value).collect())
    }
}

/// Go's `formatConfig` on one duration string: a trailing `m0s` loses its `0s`,
/// then a trailing `h0m` loses its `0m`, so `1h0m0s` becomes `1h` and `2m0s`
/// becomes `2m`.
fn format_duration(text: &str) -> String {
    let mut text = text;
    if let Some(rest) = text.strip_suffix("m0s") {
        text = &text[..rest.len() + 1];
    }
    if let Some(rest) = text.strip_suffix("h0m") {
        text = &text[..rest.len() + 1];
    }
    text.to_owned()
}

/// A feedback type expression is written as its text, as Go's `String` gives it
/// (`read>=3`), which is how the config's JSON form carries it.
impl ToConfigValue for FeedbackTypeExpression {
    fn to_config_value(&self) -> Value {
        Value::String(self.to_string())
    }
}

/// `GET /api/dashboard/config`: the configuration, without the database
/// section when the dashboard is redacted.
pub(crate) async fn get_config(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    let cfg = master.config();
    let mut map = config_map(&cfg);
    if cfg.master.dashboard_redacted {
        map.remove("database");
    }
    ok(&map)
}

/// `POST /api/dashboard/config`: replaces the recommendation settings with the
/// ones in the body, after checking the whole configuration still validates.
/// Go's `mapstructure` decode keeps the names of [`config_map`].
pub(crate) async fn post_config(
    State(master): State<Arc<Master>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let input = ConfigValue::Map(posted_map(&body)?);
    let mut new_config = Config::default();
    let mut errs = Vec::new();
    gorse_config::mapstructure::Decode::decode_value(&mut new_config, "", &input, &mut errs);
    if !errs.is_empty() {
        return Err(ApiError::BadRequest(error_text(&errs)));
    }
    let mut for_validation = master.config().as_ref().clone();
    for_validation.recommend = new_config.recommend.clone();
    if let Err(err) = for_validation.validate() {
        return Err(ApiError::BadRequest(err.to_string()));
    }
    let recommend_json = new_config
        .recommend
        .marshal_json()
        .map_err(ApiError::internal)?;
    master
        .meta_store()
        .put(meta::RECOMMEND_CONFIG, &recommend_json)
        .await
        .map_err(ApiError::internal)?;
    replace_recommend(&master, new_config.recommend.clone());
    master.cancel_task();
    let _ = master.scheduled.try_send(());
    let json = new_config.marshal_json().map_err(ApiError::internal)?;
    let value: Value = serde_json::from_str(&json).map_err(ApiError::internal)?;
    ok(&value)
}

/// Sets the recommendation settings in force, keeping the rest of the
/// configuration (Go writes `m.Config.Recommend` in place).
fn replace_recommend(master: &Master, recommend: gorse_config::config::schema::RecommendConfig) {
    let mut cfg = master.config().as_ref().clone();
    cfg.recommend = recommend;
    master.replace_config(cfg);
}

/// Reads the posted body as go-restful's `ReadEntity` reads a `map[string]any`:
/// a body with no value is `EOF`, and any other body is decoded by
/// `encoding/json`, with its error texts. A `null` body is the nil map, which
/// decodes to no change.
fn posted_map(body: &[u8]) -> Result<ConfigMap, ApiError> {
    if body
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        return Err(ApiError::BadRequest("EOF".to_owned()));
    }
    let members = gojson::unmarshal_map_any(body)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?
        .unwrap_or_default();
    Ok(members
        .iter()
        .fold(ConfigMap::new(), |mut map, (key, value)| {
            map.insert(key.clone(), to_config_value(value));
            map
        }))
}

/// Converts a value `encoding/json` decodes into the form the mapstructure
/// decoder reads: numbers are `float64`, and objects keep their names.
fn to_config_value(value: &GoValue) -> ConfigValue {
    match value {
        GoValue::Null => ConfigValue::Nil,
        GoValue::Bool(b) => ConfigValue::Bool(*b),
        GoValue::Number(n) => ConfigValue::Float64(*n),
        GoValue::String(s) => ConfigValue::String(s.clone()),
        GoValue::Array(items) => ConfigValue::Slice(items.iter().map(to_config_value).collect()),
        GoValue::Object(fields) => ConfigValue::Map(fields.iter().fold(
            ConfigMap::new(),
            |mut map, (key, value)| {
                map.insert(key.clone(), to_config_value(value));
                map
            },
        )),
    }
}

/// `DELETE /api/dashboard/config`: forgets the posted recommendation settings
/// and reverts to the ones in the configuration file.
pub(crate) async fn delete_config(State(master): State<Arc<Master>>) -> Result<Response, ApiError> {
    master
        .meta_store()
        .delete(meta::RECOMMEND_CONFIG)
        .await
        .map_err(ApiError::internal)?;
    let file = gorse_config::config::load::load_config(master.config_path())
        .map_err(ApiError::internal)?;
    replace_recommend(&master, file.recommend);
    master.cancel_task();
    let _ = master.scheduled.try_send(());
    ok(&Map::new())
}

/// `GET /api/dashboard/config/schema`: the frozen JSON Schema of the
/// configuration.
pub(crate) async fn get_config_schema() -> Result<Response, ApiError> {
    let schema: Value = serde_json::from_str(SCHEMA).map_err(ApiError::internal)?;
    ok(&schema)
}
