// Ports: storage/vectors/milvus.go @ add21b5, storage/vectors/milvus_test.go @ add21b5

//! The Milvus store, `milvus://host:port`, over Milvus's REST API v2
//! (`/v2/vectordb`), served on the same port as its gRPC API. Go's
//! `milvus-io/milvus/client/v2` is not used here: the port is a hand-written
//! adapter over `reqwest`, as `PORTING.md` D-17 decides. It sends the requests
//! Go's client sends, in the same shapes, so the collections, indexes and object
//! ids are the ones Go writes.
//!
//! Two things differ on the wire, not in the stored data. The REST gateway nests
//! an index's own parameters as a JSON string under `params`, which Go's gRPC
//! client sees flat; `index_params` unnests them. A query's category
//! filter carries the category as a literal, not as a template parameter. The table
//! prefix is not used, as Go does not use it (`PORTING_NOTES.md` Q-24).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_storage_core::errors::Error as StorageError;
use serde_json::{Map, Value, json};
use url::Url;

use super::{
    CollectionInfo, Database, Distance, Error, QuantizationType, ScoredVector, Vector,
    VectorConfig, check_context, float16_bytes, float16_from_bytes, not_found, not_supported,
    order_vectors,
};

/// The primary key field, Go's `milvusIdField`.
const FIELD_ID: &str = "id";
/// The vector field, Go's `milvusVectorField`.
const FIELD_VECTOR: &str = "vector";
/// The categories field, Go's `milvusCategoriesField`.
const FIELD_CATEGORIES: &str = "categories";
/// The hidden flag field, Go's `milvusHiddenField`.
const FIELD_HIDDEN: &str = "hidden";
/// The timestamp field, in milliseconds, Go's `milvusTimestampField`.
const FIELD_TIMESTAMP: &str = "timestamp";
/// The description of a collection, Go's `WithDescription("gorse collection")`.
const DESCRIPTION: &str = "gorse collection";
/// The data type of a sparse vector field, as the REST API names it.
const TYPE_SPARSE_VECTOR: &str = "SparseFloatVector";

/// The key of an index's type in its parameters, Go's `index.IndexTypeKey`.
const INDEX_TYPE_KEY: &str = "index_type";
/// The key of an index's metric in its parameters, Go's `index.MetricTypeKey`.
const METRIC_TYPE_KEY: &str = "metric_type";
/// The key of an index's own parameters in REST: a JSON object in a string.
const PARAMS_KEY: &str = "params";

/// Go's `entity.COSINE`.
const METRIC_COSINE: &str = "COSINE";
/// Go's `entity.L2`.
const METRIC_L2: &str = "L2";
/// Go's `entity.IP`.
const METRIC_IP: &str = "IP";

/// Go's `index.HNSW`.
const INDEX_HNSW: &str = "HNSW";
/// Go's `index.IvfPQ`.
const INDEX_IVF_PQ: &str = "IVF_PQ";
/// Go's `index.IvfSQ8`.
const INDEX_IVF_SQ8: &str = "IVF_SQ8";
/// Go's `index.IvfRabitQ`.
const INDEX_IVF_RABITQ: &str = "IVF_RABITQ";
/// Go's `index.SparseInverted`.
const INDEX_SPARSE_INVERTED: &str = "SPARSE_INVERTED_INDEX";
/// Go's `index.SparseWAND`.
const INDEX_SPARSE_WAND: &str = "SPARSE_WAND";
/// Go's `index.Sorted`, the index of the timestamp field.
const INDEX_STL_SORT: &str = "STL_SORT";

/// The index state of a finished build, as the REST API reports it.
const INDEX_STATE_FINISHED: &str = "Finished";
/// The index state of a failed build, as the REST API reports it.
const INDEX_STATE_FAILED: &str = "Failed";
/// The load state of a loaded collection, as the REST API reports it.
const LOAD_STATE_LOADED: &str = "LoadStateLoaded";

/// Go's `defaultMilvusIVFNList`.
const DEFAULT_IVF_NLIST: i64 = 128;
/// Go's `defaultMilvusPQBits`.
const DEFAULT_PQ_BITS: i64 = 8;
/// Go's `defaultMilvusRQNProbe`, also the probe count of the SQ and PQ searches.
const DEFAULT_NPROBE: i64 = 8;
/// Go's `defaultMilvusRQRefineK`.
const DEFAULT_REFINE_K: i64 = 1;
/// The `M` of Go's `milvusIndex` HNSW index.
const HNSW_M: i64 = 16;
/// The `efConstruction` of Go's `milvusIndex` HNSW index.
const HNSW_EF_CONSTRUCTION: i64 = 200;
/// The `ef` of Go's HNSW searches.
const HNSW_EF: i64 = 100;

/// How often a build or a load is checked. Go's `Await` polls the same way.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A Milvus store (Go's `Milvus`).
pub struct Milvus {
    http: reqwest::Client,
    /// The API root, `http://host:port/v2/vectordb`.
    base: String,
}

/// Go's `errors.WithStack`: the error's own text, with no storage sentinel.
fn other(err: impl std::fmt::Display) -> Error {
    Error::Other(err.to_string())
}

/// Go's `milvus.New` for `milvus://` paths. The address is the path's host and
/// port, as Go's `Address: u.Host`.
pub(crate) fn connect(path: &str) -> Result<Milvus, Error> {
    let url = Url::parse(path).map_err(other)?;
    let host = url.host_str().unwrap_or_default();
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    Ok(Milvus {
        http: reqwest::Client::new(),
        base: format!("http://{authority}/v2/vectordb"),
    })
}

/// Go's `entity.MetricType` of a distance.
fn metric_of(distance: Distance) -> &'static str {
    match distance {
        Distance::Cosine => METRIC_COSINE,
        Distance::Euclidean => METRIC_L2,
        Distance::Dot => METRIC_IP,
    }
}

/// Go's metric switch: the distance a metric type ranks by. An empty metric is
/// cosine, as Go reads it.
fn distance_of(metric: &str) -> Result<Distance, Error> {
    match metric {
        "" | METRIC_COSINE => Ok(Distance::Cosine),
        METRIC_L2 => Ok(Distance::Euclidean),
        METRIC_IP => Ok(Distance::Dot),
        other => Err(not_supported(&format!("distance method {other}"))),
    }
}

/// The text of a JSON value as Go's `KvPairsMap` holds it: a string as written,
/// anything else as its JSON text.
fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A list of `{key, value}` pairs as a map, Go's `entity.KvPairsMap`.
fn key_values(list: Option<&Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for item in list.and_then(Value::as_array).into_iter().flatten() {
        let key = item.get("key").and_then(Value::as_str).unwrap_or_default();
        let value = item.get("value").map(json_text).unwrap_or_default();
        map.insert(key.to_owned(), value);
    }
    map
}

/// Go's `strconv.Atoi` of a parameter. An absent parameter is the empty string,
/// which does not parse.
fn parse_int(params: &HashMap<String, String>, key: &str) -> Result<i64, Error> {
    let value = params.get(key).map(String::as_str).unwrap_or_default();
    value
        .parse()
        .map_err(|_| Error::Other(format!("strconv.Atoi: parsing {value:?}: invalid syntax")))
}

/// Go's `milvusVectorDimension`: the dimensions of the vector field, 0 when it is
/// sparse.
fn vector_dimension(collection: &Value) -> Result<i64, Error> {
    let fields = collection
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for field in fields {
        if field.get("name").and_then(Value::as_str) != Some(FIELD_VECTOR) {
            continue;
        }
        if field.get("type").and_then(Value::as_str) == Some(TYPE_SPARSE_VECTOR) {
            return Ok(0);
        }
        return parse_int(&key_values(field.get("params")), "dim");
    }
    Err(Error::Storage {
        kind: StorageError::NotFound,
        message: format!("vector field: {}", StorageError::NotFound),
    })
}

/// The categories of a row. Milvus writes a `Data` with no `data` for an empty
/// array.
fn categories_of(row: &Value) -> Vec<String> {
    row.pointer("/categories/Data/StringData/data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// A string literal of a filter expression. JSON escapes the characters a Milvus
/// string literal escapes.
fn literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// The dense values of a base64 FP16 string, as Go's `ToFloat32Vector`.
fn dense_values(encoded: &str) -> Result<Vec<f32>, Error> {
    let bytes = STANDARD.decode(encoded).map_err(other)?;
    Ok(floats::to_float32(&float16_from_bytes(&bytes)))
}

/// The REST form of a sparse vector: an object from each dimension to its value.
/// Go's `entity.NewSliceSparseEmbedding` checks the lengths match.
fn sparse_object(indices: &[u32], values: &[f32]) -> Result<Value, Error> {
    if indices.len() != values.len() {
        return Err(Error::Other(
            "invalid sparse embedding input, positions shall have same number of values".to_owned(),
        ));
    }
    let object: Map<String, Value> = indices
        .iter()
        .zip(values)
        .map(|(index, value)| (index.to_string(), json!(value)))
        .collect();
    Ok(Value::Object(object))
}

/// The indices and values of a sparse vector read from an object, sorted by index
/// as Go's sparse embeddings are.
fn sparse_values(object: &Map<String, Value>) -> Result<(Vec<u32>, Vec<f32>), Error> {
    let mut entries = object
        .iter()
        .map(|(index, value)| {
            let index = index.parse::<u32>().map_err(other)?;
            Ok((index, value.as_f64().unwrap_or_default() as f32))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    entries.sort_by_key(|(index, _)| *index);
    Ok(entries.into_iter().unzip())
}

/// Go's `milvusVectors` for one row: a vector as the query returned it.
fn row_vector(row: &Value, collection: &str) -> Result<Vector, Error> {
    let id = row.get(FIELD_ID).and_then(Value::as_str).ok_or_else(|| {
        Error::Other(format!(
            "failed to parse vector ids for collection {collection}"
        ))
    })?;
    let is_hidden = row
        .get(FIELD_HIDDEN)
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            Error::Other(format!(
                "failed to parse vector visibility for collection {collection}"
            ))
        })?;
    let timestamp = row
        .get(FIELD_TIMESTAMP)
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            Error::Other(format!(
                "failed to parse vector timestamps for collection {collection}"
            ))
        })?;
    let mut vector = Vector {
        id: id.to_owned(),
        is_hidden,
        categories: categories_of(row),
        timestamp: gotime::unix_milli(timestamp).utc(),
        ..Default::default()
    };
    match row.get(FIELD_VECTOR) {
        Some(Value::String(encoded)) => vector.values = dense_values(encoded)?,
        Some(Value::Object(sparse)) => {
            (vector.indices, vector.values) = sparse_values(sparse)?;
        }
        _ => {
            return Err(Error::Other(format!(
                "failed to parse vector values for collection {collection}"
            )));
        }
    }
    Ok(vector)
}

/// The index Go's `milvusIndex` builds for a dense vector field: its type and its
/// parameters.
fn milvus_index(dimensions: i64, config: &VectorConfig) -> Result<(&'static str, Value), Error> {
    match &config.quantization {
        QuantizationType::None => Ok((
            INDEX_HNSW,
            json!({ "M": HNSW_M, "efConstruction": HNSW_EF_CONSTRUCTION }),
        )),
        QuantizationType::Rq => {
            if config.bits != 0 {
                return Err(not_supported(&format!(
                    "RQ quantization bits {} for Milvus",
                    config.bits
                )));
            }
            Ok((INDEX_IVF_RABITQ, json!({ "nlist": DEFAULT_IVF_NLIST })))
        }
        QuantizationType::Pq => {
            let bits = if config.bits == 0 {
                DEFAULT_PQ_BITS
            } else {
                config.bits
            };
            if bits <= 0 || dimensions <= 0 || dimensions * bits % DEFAULT_PQ_BITS != 0 {
                return Err(not_supported(&format!(
                    "PQ quantization bits {} for Milvus",
                    config.bits
                )));
            }
            let m = dimensions * bits / DEFAULT_PQ_BITS;
            if m <= 0 || m > dimensions || dimensions % m != 0 {
                return Err(not_supported(&format!(
                    "PQ quantization bits {} for Milvus",
                    config.bits
                )));
            }
            Ok((
                INDEX_IVF_PQ,
                json!({ "nlist": DEFAULT_IVF_NLIST, "m": m, "nbits": DEFAULT_PQ_BITS }),
            ))
        }
        QuantizationType::Sq => {
            if config.bits != 0 && config.bits != 8 {
                return Err(not_supported(&format!(
                    "SQ quantization bits {} for Milvus",
                    config.bits
                )));
            }
            Ok((INDEX_IVF_SQ8, json!({ "nlist": DEFAULT_IVF_NLIST })))
        }
        other => Err(not_supported(&format!(
            "quantization type {other} for Milvus"
        ))),
    }
}

impl Milvus {
    /// Sends a request and returns its `data`. Milvus answers `code` 0 with the
    /// result; any other code is a failure with the server's message.
    async fn call(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .map_err(other)?;
        let status = response.status();
        let text = response.text().await.map_err(other)?;
        if status != reqwest::StatusCode::OK {
            return Err(Error::Other(format!("milvus {path}: {status}: {text}")));
        }
        let reply: Value = serde_json::from_str(&text).map_err(other)?;
        let code = reply.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code != 0 {
            let message = reply
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(Error::Other(message.to_owned()));
        }
        Ok(reply.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Go's `HasCollection`.
    async fn has(&self, name: &str) -> Result<bool, Error> {
        let data = self
            .call("/collections/has", &json!({ "collectionName": name }))
            .await?;
        Ok(data.get("has").and_then(Value::as_bool).unwrap_or(false))
    }

    /// Go's `DescribeIndex`, with the index's parameters flat as the gRPC client
    /// sees them. REST nests the index's own parameters as a JSON object in a
    /// string under `params`, so they are unnested here.
    async fn index_params(
        &self,
        collection: &str,
        index_name: &str,
    ) -> Result<HashMap<String, String>, Error> {
        let data = self
            .call(
                "/indexes/describe",
                &json!({ "collectionName": collection, "indexName": index_name }),
            )
            .await?;
        let index = data
            .as_array()
            .and_then(|indexes| indexes.first())
            .ok_or_else(|| Error::Other(format!("index {index_name} not found")))?;
        let mut params = key_values(index.get("indexParams"));
        if let Some(nested) = params.remove(PARAMS_KEY) {
            let nested: Map<String, Value> = serde_json::from_str(&nested).map_err(other)?;
            for (key, value) in nested {
                params.insert(key, json_text(&value));
            }
        }
        Ok(params)
    }

    /// Go's `searchParam`: the search parameters of the index on a collection's
    /// vector field, and the distance it ranks by.
    async fn search_param(&self, collection: &str) -> Result<(Value, Distance), Error> {
        let params = self.index_params(collection, FIELD_VECTOR).await?;
        let distance = distance_of(
            params
                .get(METRIC_TYPE_KEY)
                .map(String::as_str)
                .unwrap_or_default(),
        )?;
        let search = match params
            .get(INDEX_TYPE_KEY)
            .map(String::as_str)
            .unwrap_or_default()
        {
            INDEX_SPARSE_INVERTED | INDEX_SPARSE_WAND => json!({}),
            INDEX_IVF_RABITQ => json!({ "nprobe": DEFAULT_NPROBE, "refine_k": DEFAULT_REFINE_K }),
            INDEX_IVF_PQ | INDEX_IVF_SQ8 => json!({ "nprobe": DEFAULT_NPROBE }),
            _ => json!({ "ef": HNSW_EF }),
        };
        Ok((search, distance))
    }

    /// Runs a query with strong consistency, as Go's `Query` does, and returns its
    /// rows.
    async fn query(
        &self,
        collection: &str,
        filter: &str,
        fields: &[&str],
    ) -> Result<Vec<Value>, Error> {
        let data = self
            .call(
                "/entities/query",
                &json!({
                    "collectionName": collection,
                    "filter": filter,
                    "outputFields": fields,
                    "consistencyLevel": "Strong",
                }),
            )
            .await?;
        Ok(match data {
            Value::Array(rows) => rows,
            _ => Vec::new(),
        })
    }

    /// Go's `CreateIndex` followed by `Await`: the index is ready once Milvus
    /// reports it finished. The index is named after its field, as Go names it.
    async fn create_index(
        &self,
        ctx: &Context,
        collection: &str,
        field: &str,
        index_type: &str,
        metric: Option<&str>,
        params: Value,
    ) -> Result<(), Error> {
        let mut index = json!({ "fieldName": field, "indexName": field, "indexType": index_type });
        if let Some(metric) = metric {
            index["metricType"] = json!(metric);
        }
        if params.as_object().is_some_and(|params| !params.is_empty()) {
            index[PARAMS_KEY] = params;
        }
        self.call(
            "/indexes/create",
            &json!({ "collectionName": collection, "indexParams": [index] }),
        )
        .await?;
        loop {
            check_context(ctx)?;
            let data = self
                .call(
                    "/indexes/describe",
                    &json!({ "collectionName": collection, "indexName": field }),
                )
                .await?;
            let index = data.as_array().and_then(|indexes| indexes.first());
            let state = index
                .and_then(|index| index.get("indexState"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            match state {
                INDEX_STATE_FINISHED => return Ok(()),
                INDEX_STATE_FAILED => {
                    let reason = index
                        .and_then(|index| index.get("failReason"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    return Err(Error::Other(reason.to_owned()));
                }
                _ => tokio::time::sleep(POLL_INTERVAL).await,
            }
        }
    }

    /// Go's `LoadCollection` followed by `Await`.
    async fn load_collection(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        self.call("/collections/load", &json!({ "collectionName": name }))
            .await?;
        loop {
            check_context(ctx)?;
            let data = self
                .call(
                    "/collections/get_load_state",
                    &json!({ "collectionName": name }),
                )
                .await?;
            if data.get("loadState").and_then(Value::as_str) == Some(LOAD_STATE_LOADED) {
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

#[async_trait]
impl Database for Milvus {
    // Go's `Init` returns nil.
    async fn init(&self) -> Result<(), Error> {
        Ok(())
    }

    // Go's `Optimize` returns nil.
    async fn optimize(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    // Go's `Close` closes the gRPC client. The REST client holds no connection to
    // close.
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error> {
        check_context(ctx)?;
        let data = self.call("/collections/list", &json!({})).await?;
        Ok(data
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn describe_collection(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<CollectionInfo, Error> {
        check_context(ctx)?;
        if !self.has(name).await? {
            return Err(not_found(name));
        }
        let collection = self
            .call("/collections/describe", &json!({ "collectionName": name }))
            .await?;
        let dimension = vector_dimension(&collection)?;
        let params = self.index_params(name, FIELD_VECTOR).await?;
        let distance = distance_of(
            params
                .get(METRIC_TYPE_KEY)
                .map(String::as_str)
                .unwrap_or_default(),
        )?;
        let mut config = VectorConfig::default();
        match params
            .get(INDEX_TYPE_KEY)
            .map(String::as_str)
            .unwrap_or_default()
        {
            INDEX_IVF_RABITQ => config.quantization = QuantizationType::Rq,
            INDEX_IVF_SQ8 => {
                config.quantization = QuantizationType::Sq;
                config.bits = 8;
            }
            INDEX_IVF_PQ => {
                config.quantization = QuantizationType::Pq;
                let m = parse_int(&params, "m")?;
                let nbits = parse_int(&params, "nbits")?;
                if dimension > 0 {
                    config.bits = m * nbits / dimension;
                }
            }
            _ => {}
        }
        Ok(CollectionInfo {
            name: name.to_owned(),
            dimension,
            distance,
            config,
        })
    }

    async fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        if dimensions == 0 {
            if distance != Distance::Dot {
                return Err(not_supported("distance method for sparse vector"));
            }
            if config != VectorConfig::default() {
                return Err(not_supported("quantization for sparse vector"));
            }
        }

        let vector_field = if dimensions == 0 {
            json!({ "fieldName": FIELD_VECTOR, "dataType": TYPE_SPARSE_VECTOR })
        } else {
            json!({
                "fieldName": FIELD_VECTOR,
                "dataType": "Float16Vector",
                "elementTypeParams": { "dim": dimensions },
            })
        };
        self.call(
            "/collections/create",
            &json!({
                "collectionName": name,
                "description": DESCRIPTION,
                "schema": {
                    "autoId": false,
                    "enableDynamicField": false,
                    "fields": [
                        {
                            "fieldName": FIELD_ID,
                            "dataType": "VarChar",
                            "isPrimary": true,
                            "elementTypeParams": { "max_length": 65535 },
                        },
                        {
                            "fieldName": FIELD_CATEGORIES,
                            "dataType": "Array",
                            "elementDataType": "VarChar",
                            "elementTypeParams": { "max_capacity": 100, "max_length": 65535 },
                        },
                        { "fieldName": FIELD_HIDDEN, "dataType": "Bool" },
                        { "fieldName": FIELD_TIMESTAMP, "dataType": "Int64" },
                        vector_field,
                    ],
                },
            }),
        )
        .await?;

        // Go checks the index of a dense field after the collection exists, so a
        // rejected quantization leaves the collection behind, as it does in Go.
        let metric = metric_of(distance);
        if dimensions == 0 {
            // Go's `NewSparseInvertedIndex(entity.IP, 0)`.
            self.create_index(
                ctx,
                name,
                FIELD_VECTOR,
                INDEX_SPARSE_INVERTED,
                Some(METRIC_IP),
                json!({ "drop_ratio_build": 0 }),
            )
            .await?;
        } else {
            let (index_type, params) = milvus_index(dimensions, &config)?;
            self.create_index(ctx, name, FIELD_VECTOR, index_type, Some(metric), params)
                .await?;
        }
        self.create_index(ctx, name, FIELD_TIMESTAMP, INDEX_STL_SORT, None, json!({}))
            .await?;
        self.load_collection(ctx, name).await
    }

    async fn delete_collection(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check_context(ctx)?;
        if !self.has(name).await? {
            return Err(not_found(name));
        }
        self.call("/collections/drop", &json!({ "collectionName": name }))
            .await?;
        Ok(())
    }

    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, Error> {
        check_context(ctx)?;
        let rows = self.query(collection, "", &["count(*)"]).await?;
        rows.first()
            .and_then(|row| row.get("count(*)"))
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                Error::Other(format!(
                    "failed to parse vector count for collection {collection}"
                ))
            })
    }

    async fn add_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        vectors: &[Vector],
    ) -> Result<(), Error> {
        check_context(ctx)?;
        if vectors.is_empty() {
            return Ok(());
        }
        let mut sparse_count = 0;
        let mut rows = Vec::with_capacity(vectors.len());
        for vector in vectors {
            let values = if vector.is_sparse() {
                sparse_count += 1;
                sparse_object(&vector.indices, &vector.values)?
            } else {
                json!(STANDARD.encode(float16_bytes(&vector.float16_values())))
            };
            rows.push(json!({
                FIELD_ID: vector.id,
                FIELD_CATEGORIES: vector.categories,
                FIELD_HIDDEN: vector.is_hidden,
                FIELD_TIMESTAMP: vector.timestamp.unix_milli(),
                FIELD_VECTOR: values,
            }));
        }
        if sparse_count > 0 && sparse_count != vectors.len() {
            return Err(Error::Other(
                "cannot mix dense and sparse vectors".to_owned(),
            ));
        }
        self.call(
            "/entities/upsert",
            &json!({ "collectionName": collection, "data": rows }),
        )
        .await?;
        Ok(())
    }

    async fn get_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<Vector>, Error> {
        check_context(ctx)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let quoted: Vec<String> = ids.iter().map(|id| literal(id)).collect();
        let filter = format!("{FIELD_ID} in [{}]", quoted.join(","));
        let rows = self
            .query(
                collection,
                &filter,
                &[
                    FIELD_ID,
                    FIELD_CATEGORIES,
                    FIELD_HIDDEN,
                    FIELD_TIMESTAMP,
                    FIELD_VECTOR,
                ],
            )
            .await?;
        let vectors = rows
            .iter()
            .map(|row| row_vector(row, collection))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(order_vectors(ids, vectors))
    }

    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        self.call(
            "/entities/delete",
            &json!({
                "collectionName": collection,
                "filter": format!("{FIELD_TIMESTAMP} < {}", timestamp.unix_milli()),
            }),
        )
        .await?;
        Ok(())
    }

    async fn query_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        q: &Vector,
        categories: &[&str],
        top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error> {
        check_context(ctx)?;
        if top_k <= 0 {
            return Ok(Vec::new());
        }
        let mut filter = format!("{FIELD_HIDDEN} == false");
        if !categories.is_empty() {
            let conditions: Vec<String> = categories
                .iter()
                .map(|category| {
                    format!("array_contains({FIELD_CATEGORIES}, {})", literal(category))
                })
                .collect();
            filter += &format!(" and ({})", conditions.join(" and "));
        }
        let (search_params, distance) = self.search_param(collection).await?;
        let query = if q.is_sparse() {
            sparse_object(&q.indices, &q.values)?
        } else {
            json!(STANDARD.encode(float16_bytes(&q.float16_values())))
        };
        let response = self
            .call(
                "/entities/search",
                &json!({
                    "collectionName": collection,
                    "data": [query],
                    "annsField": FIELD_VECTOR,
                    "filter": filter,
                    "limit": top_k,
                    "outputFields": [FIELD_ID, FIELD_CATEGORIES, FIELD_HIDDEN],
                    "searchParams": { PARAMS_KEY: search_params },
                    "consistencyLevel": "Strong",
                }),
            )
            .await?;
        Ok(response
            .as_array()
            .into_iter()
            .flatten()
            .map(|hit| {
                let distance_value = hit
                    .get("distance")
                    .and_then(Value::as_f64)
                    .unwrap_or_default() as f32;
                let score = if distance == Distance::Euclidean {
                    -distance_value
                } else {
                    distance_value
                };
                ScoredVector {
                    vector: Vector {
                        id: hit
                            .get(FIELD_ID)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        is_hidden: hit
                            .get(FIELD_HIDDEN)
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        categories: categories_of(hit),
                        ..Default::default()
                    },
                    score,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;
    use crate::vectors::{open, suite};
    use gorse_storage_core::options::Opt;

    /// Go's `milvusUri`, from `MILVUS_URI`. Unset, the Milvus tests are skipped, as
    /// Go's `t.Skip` skips them.
    fn milvus_dsn() -> Option<String> {
        let dsn = std::env::var("MILVUS_URI").ok().filter(|s| !s.is_empty());
        if dsn.is_none() {
            eprintln!("MILVUS_URI is not set, skipping Milvus test");
        }
        dsn
    }

    /// The cases of this module share one server and the collections' names, so
    /// they run one at a time (see the Qdrant tests).
    static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

    /// Runs one case on the server at `MILVUS_URI`, then closes the store.
    async fn run(case: impl for<'a> AsyncFnOnce(&'a Context, &'a dyn Database)) {
        let Some(dsn) = milvus_dsn() else {
            return;
        };
        let _serial = SERIAL.lock().await;
        let db = open(&dsn, "gorse_", &[] as &[Opt]).await.expect("open");
        db.init().await.expect("init");
        case(&Context::background(), db.as_ref()).await;
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn purge() {
        run(suite::test_purge).await;
    }

    #[tokio::test]
    async fn collections() {
        run(suite::test_collections).await;
    }

    #[tokio::test]
    async fn vectors() {
        run(suite::test_vectors).await;
    }

    #[tokio::test]
    async fn half_vectors() {
        run(suite::test_half_vectors).await;
    }

    #[tokio::test]
    async fn get_vectors() {
        run(suite::test_get_vectors).await;
    }

    #[tokio::test]
    async fn sparse() {
        run(suite::test_sparse).await;
    }

    #[tokio::test]
    async fn hidden() {
        run(suite::test_hidden).await;
    }

    #[tokio::test]
    async fn dot() {
        run(suite::test_dot).await;
    }

    #[tokio::test]
    async fn delete_vectors() {
        run(suite::test_delete_vectors).await;
    }

    /// Go's `TestInvalidSparseCollection`: a sparse collection takes only the dot
    /// distance and no quantization, and a rejected one is not created.
    #[tokio::test]
    async fn invalid_sparse_collection() {
        run(async |ctx, db| {
            let err = db
                .add_collection(
                    ctx,
                    "test_invalid_sparse",
                    0,
                    Distance::Cosine,
                    VectorConfig::default(),
                )
                .await
                .expect_err("add");
            assert_eq!(err.kind(), Some(StorageError::NotSupported));
            let err = db
                .describe_collection(ctx, "test_invalid_sparse")
                .await
                .expect_err("describe");
            assert_eq!(err.kind(), Some(StorageError::NotFound));
        })
        .await;
    }

    /// Go's `TestQuantization` for Milvus: every quantization the store accepts,
    /// with the bits it accepts.
    #[tokio::test]
    async fn quantization() {
        run(async |ctx, db| {
            suite::test_quantization(ctx, db, QuantizationType::None, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Sq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Sq, 8).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 8).await;
        })
        .await;
    }
}
