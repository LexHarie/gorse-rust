// Ports: storage/meta/database.go @ add21b5

//! The meta store: the node registry, the models the master trains and a small
//! key-value table, the [`Database`] interface every backend implements, and
//! [`open`], which picks the backend from the path.
//!
//! Go has one backend, SQLite (`sqlite://<file>`), so [`sqlite`] is the only
//! implementation here. Go's `Database` methods take no context, so neither
//! does this trait; the trait is async and `dyn`-compatible (D-122).

use async_trait::async_trait;
use gorse_common::gojson::{self, Value};
use gorse_common::gotime::{Duration, Time};
use gorse_common::jsonutil;
use gorse_model_core::params::{ParamValue, Params};
use gorse_storage_core::scheme::SQLITE_PREFIX;
use serde::de::Deserializer;
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub mod sqlite;

#[cfg(test)]
mod database_test;
#[cfg(test)]
mod golden_test;
#[cfg(test)]
mod sqlite_test;

/// Go's `COLLABORATIVE_FILTERING_MODEL`: the key of the collaborative
/// filtering model's metadata.
pub const COLLABORATIVE_FILTERING_MODEL: &str = "COLLABORATIVE_FILTERING_MODEL";
/// Go's `CLICK_THROUGH_RATE_MODEL`: the key of the click-through rate model's
/// metadata.
pub const CLICK_THROUGH_RATE_MODEL: &str = "CLICK_THROUGH_RATE_MODEL";
/// Go's `RECOMMEND_CONFIG`: the key of the recommendation configuration.
pub const RECOMMEND_CONFIG: &str = "RECOMMEND_CONFIG";

/// Go's `Model[T]`: a trained model's id, type, hyper-parameters and score.
#[derive(Debug, Clone, Default)]
pub struct Model<T> {
    /// The model's id (Go's `ID`).
    pub id: i64,
    /// The model's type (Go's `Type`).
    pub r#type: String,
    /// The hyper-parameters the model was trained with (Go's `Params`).
    pub params: Params,
    /// The model's score (Go's `Score`).
    pub score: T,
}

/// The JSON shape of a [`Model`]: the fields of Go's struct, which has no
/// tags, so each one is named by its Go field name.
#[derive(Serialize)]
struct ModelJson<'a, T> {
    #[serde(rename = "ID")]
    id: i64,
    #[serde(rename = "Type")]
    r#type: &'a str,
    #[serde(rename = "Params")]
    params: ParamsJson<'a>,
    #[serde(rename = "Score")]
    score: &'a T,
}

/// The JSON shape `FromJSON` reads. Go's `json.Unmarshal` leaves a field the
/// JSON does not name as it was, and a `null` for these types is a no-op, so
/// each field is an `Option`.
#[derive(Deserialize)]
struct ModelWire<T> {
    #[serde(rename = "ID")]
    id: Option<i64>,
    #[serde(rename = "Type")]
    r#type: Option<String>,
    #[serde(rename = "Params", default, deserialize_with = "present_raw")]
    params: Option<Box<RawValue>>,
    #[serde(rename = "Score")]
    score: Option<T>,
}

/// Reads `Params` as its raw JSON, so that a `null` (which clears a Go map)
/// is told apart from a field the JSON does not name.
fn present_raw<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(de).map(Some)
}

impl<T> Model<T> {
    /// Go's `Equal`: whether two models have the same type and the same
    /// parameters. Ids and scores are not compared.
    pub fn equal(&self, other: &Model<T>) -> bool {
        self.r#type == other.r#type && self.params == other.params
    }
}

impl<T: Serialize> Model<T> {
    /// Go's `ToJSON`: the model as `json.Marshal` writes it.
    ///
    /// Go panics when the encoding fails (`lo.Must1`). This returns the error
    /// instead. Go's encoder refuses a NaN, and a [`ParamValue::Other`], which
    /// Gorse never stores.
    pub fn to_json(&self) -> Result<String, Error> {
        let json = ModelJson {
            id: self.id,
            r#type: &self.r#type,
            params: ParamsJson(&self.params),
            score: &self.score,
        };
        Ok(String::from_utf8(jsonutil::marshal(&json)?).expect("serde_json writes UTF-8"))
    }
}

impl<T: serde::de::DeserializeOwned> Model<T> {
    /// Go's `FromJSON`: reads `data` into the model with `json.Unmarshal`.
    ///
    /// Fields the JSON does not name keep their values. `Params` merges into
    /// the parameters the model already holds, as Go's map decoding does, and
    /// a `null` clears them.
    pub fn from_json(&mut self, data: &str) -> Result<(), Error> {
        let Some(wire) = serde_json::from_str::<Option<ModelWire<T>>>(data)? else {
            return Ok(());
        };
        if let Some(id) = wire.id {
            self.id = id;
        }
        if let Some(model_type) = wire.r#type {
            self.r#type = model_type;
        }
        if let Some(raw) = wire.params {
            match gojson::unmarshal_map_any(raw.get().as_bytes())? {
                None => self.params.0.clear(),
                Some(values) => {
                    for (name, value) in values {
                        self.params.0.insert(name, param_value(&value));
                    }
                }
            }
        }
        if let Some(score) = wire.score {
            self.score = score;
        }
        Ok(())
    }
}

/// The value a `map[ParamName]any` holds for a JSON value, as Go's decoder
/// gives it: numbers are `float64`, and an array or object is a `[]any` or a
/// `map[string]any`, which [`ParamValue::Json`] holds.
fn param_value(value: &Value) -> ParamValue {
    match value {
        Value::Null => ParamValue::Nil,
        Value::Bool(b) => ParamValue::Bool(*b),
        Value::Number(n) => ParamValue::Float64(*n),
        Value::String(s) => ParamValue::String(s.clone()),
        Value::Array(_) | Value::Object(_) => ParamValue::Json(value.clone()),
    }
}

/// A decoded JSON value as `json.Marshal` writes the `any` it was decoded into:
/// a number as a `float64`, and an object with its keys sorted.
struct AnyJson<'a>(&'a Value);

impl Serialize for AnyJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Null => serializer.serialize_none(),
            Value::Bool(v) => serializer.serialize_bool(*v),
            Value::Number(v) => serializer.serialize_f64(*v),
            Value::String(v) => serializer.serialize_str(v),
            Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&AnyJson(item))?;
                }
                seq.end()
            }
            Value::Object(entries) => {
                // Go sorts the keys of a map by their bytes, which is how
                // `String` orders them.
                let mut sorted: Vec<(&String, &Value)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                let mut map = serializer.serialize_map(Some(sorted.len()))?;
                for (name, value) in sorted {
                    map.serialize_entry(name, &AnyJson(value))?;
                }
                map.end()
            }
        }
    }
}

/// [`Params`] as `json.Marshal` writes a `map[ParamName]any`: each value in its
/// dynamic type, the keys sorted (a [`BTreeMap`](std::collections::BTreeMap)
/// keeps them in that order).
struct ParamsJson<'a>(&'a Params);

impl Serialize for ParamsJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.0.len()))?;
        for (name, value) in &self.0.0 {
            map.serialize_entry(name, &ParamValueJson(value))?;
        }
        map.end()
    }
}

/// One [`ParamValue`] as `json.Marshal` writes its dynamic type.
struct ParamValueJson<'a>(&'a ParamValue);

impl Serialize for ParamValueJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            ParamValue::Nil => serializer.serialize_none(),
            ParamValue::Int(v) | ParamValue::Int64(v) => serializer.serialize_i64(*v),
            ParamValue::Int32(v) => serializer.serialize_i32(*v),
            ParamValue::Float32(v) => serializer.serialize_f32(*v),
            ParamValue::Float64(v) => serializer.serialize_f64(*v),
            ParamValue::Bool(v) => serializer.serialize_bool(*v),
            ParamValue::String(v) => serializer.serialize_str(v),
            ParamValue::IntSlice(v) => v.serialize(serializer),
            ParamValue::Json(v) => AnyJson(v).serialize(serializer),
            ParamValue::Other(name) => Err(serde::ser::Error::custom(format_args!(
                "json: unsupported type: {name}"
            ))),
        }
    }
}

/// Go's `Node`: a node of the cluster, as the master records it.
#[derive(Debug, Clone)]
pub struct Node {
    /// The node's UUID.
    pub uuid: String,
    /// The node's host name.
    pub hostname: String,
    /// The node's type: `master`, `worker` or `server`.
    pub r#type: String,
    /// The node's Gorse version.
    pub version: String,
    /// When the node last reported.
    pub update_time: Time,
}

/// Go's `meta.Database`: the interface every meta backend implements.
#[async_trait]
pub trait Database: Send + Sync {
    /// Go's `Close`: releases the store. Calls after it fail.
    async fn close(&self) -> Result<(), Error>;
    /// Go's `Init`: creates the tables the store needs.
    async fn init(&self) -> Result<(), Error>;
    /// Go's `UpdateNode`: records `node`, replacing the record of the same UUID.
    async fn update_node(&self, node: &Node) -> Result<(), Error>;
    /// Go's `ListNodes`: the nodes that reported within the store's time to
    /// live. Nodes that did not are removed.
    async fn list_nodes(&self) -> Result<Vec<Node>, Error>;
    /// Go's `Put`: sets `key` to `value`.
    async fn put(&self, key: &str, value: &str) -> Result<(), Error>;
    /// Go's `Get`: the value of `key`, or `None` when it has none.
    async fn get(&self, key: &str) -> Result<Option<String>, Error>;
    /// Go's `Delete`: removes `key`. A missing key is not an error.
    async fn delete(&self, key: &str) -> Result<(), Error>;
}

/// Errors from the meta store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Go's `Unknown database: %s`: the path has no backend.
    #[error("Unknown database: {0}")]
    UnknownDatabase(String),
    /// A call on a closed store. Go's `database/sql` says the same.
    #[error("sql: database is closed")]
    Closed,
    /// The blocking task that ran a call was cancelled before it started,
    /// which happens only while the runtime shuts down.
    #[error("blocking task was cancelled")]
    TaskCancelled,
    /// A failure from SQLite.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// A stored update time that matches none of the layouts the driver reads.
    /// Go's `Scan` fails with this text.
    #[error("unsupported Scan, storing driver.Value type string into type *time.Time")]
    UnsupportedScan,
    /// `encoding/json` rejected the model's JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The parameters are not valid JSON for Go's `any` decoder.
    #[error(transparent)]
    Unmarshal(#[from] gojson::UnmarshalError),
}

/// Go's `Open`: opens the meta store at `path`. Only `sqlite://` paths name a
/// backend; any other path is an `Unknown database` error.
pub async fn open(path: &str, ttl: Duration) -> Result<Box<dyn Database>, Error> {
    match path.strip_prefix(SQLITE_PREFIX) {
        Some(data_source_name) => Ok(Box::new(sqlite::SQLite::open(data_source_name, ttl).await?)),
        None => Err(Error::UnknownDatabase(path.to_owned())),
    }
}
