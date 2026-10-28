// Ports: storage/vectors/weaviate.go @ add21b5, storage/vectors/weaviate_test.go @ add21b5

//! The Weaviate store, `weaviate://host:port` (HTTP) and `weaviates://host:port`
//! (HTTPS), over Weaviate's REST and GraphQL APIs. Go's `weaviate-go-client` is
//! not used here: the port is a hand-written adapter over `reqwest`, as
//! `PORTING.md` D-17 decides. It sends the requests Go's client sends, in the
//! same shapes, so the classes, properties and object ids are the ones Go writes.
//!
//! Go's `Open` sets no gRPC config for the client, so its batches go over REST
//! too. The table prefix and the dimensions of a collection are not used, as
//! Go does not use them (`PORTING_NOTES.md` Q-24).

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, RFC3339_NANO, Time};
use gorse_storage_core::scheme::WEAVIATES_PREFIX;
use serde_json::{Map, Value, json};
use url::Url;

use super::{
    CollectionInfo, Database, Distance, Error, QuantizationType, ScoredVector, Vector,
    VectorConfig, check_context, not_found, not_supported, order_vectors,
};

/// The property of a vector's original id.
const PROPERTY_ORIGINAL_ID: &str = "originalId";
/// The property of a vector's categories.
const PROPERTY_CATEGORIES: &str = "categories";
/// The property of a vector's hidden flag.
const PROPERTY_HIDDEN: &str = "hidden";
/// The property of a vector's timestamp.
const PROPERTY_TIMESTAMP: &str = "timestamp";

/// A Weaviate store (Go's `Weaviate`).
pub struct Weaviate {
    http: reqwest::Client,
    /// The API root, `http://host:port/v1` or `https://host:port/v1`.
    base: String,
}

/// Go's `errors.WithStack`: the error's own text, with no storage sentinel.
fn other(err: impl std::fmt::Display) -> Error {
    Error::Other(err.to_string())
}

/// Go's `weaviate.NewClient` for `weaviate://` and `weaviates://` paths. The
/// scheme of the store is the scheme of the API.
pub(crate) fn connect(path: &str) -> Result<Weaviate, Error> {
    let url = Url::parse(path).map_err(other)?;
    let scheme = if path.starts_with(WEAVIATES_PREFIX) {
        "https"
    } else {
        "http"
    };
    let host = url.host_str().unwrap_or_default();
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    Ok(Weaviate {
        http: reqwest::Client::new(),
        base: format!("{scheme}://{authority}/v1"),
    })
}

/// Go's `capitalize`: the class name of a collection.
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Go's `uncapitalize`: the collection name of a class.
fn uncapitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The UUID of a vector id, as Go's `uuid.NewMD5(uuid.NameSpaceURL, ...)`.
fn object_uuid(id: &str) -> String {
    uuid::Uuid::new_v3(&uuid::Uuid::NAMESPACE_URL, id.as_bytes()).to_string()
}

/// The text of a GraphQL string literal, as Go's `%q` writes it for the strings
/// the stores send. JSON escapes the same characters.
fn gql_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// The GraphQL list of strings, as Go's `formatValues` writes it.
fn gql_strings(values: &[&str]) -> String {
    let items: Vec<String> = values.iter().map(|value| gql_string(value)).collect();
    format!("[{}]", items.join(","))
}

/// The GraphQL list of floats of a query vector.
fn gql_floats(values: &[f32]) -> String {
    let items: Vec<String> = values.iter().map(|value| value.to_string()).collect();
    format!("[{}]", items.join(","))
}

/// The JSON `vectorIndexConfig` of a quantization, as Go's
/// `weaviateApplyQuantization` writes it.
fn apply_quantization(index: &mut Map<String, Value>, config: &VectorConfig) -> Result<(), Error> {
    match &config.quantization {
        QuantizationType::None => Ok(()),
        QuantizationType::Sq => {
            index.insert("sq".to_owned(), json!({ "enabled": true }));
            if config.bits != 0 {
                return Err(not_supported("quantization bits for SQ"));
            }
            Ok(())
        }
        QuantizationType::Pq => {
            index.insert("pq".to_owned(), json!({ "enabled": true }));
            if config.bits != 0 {
                return Err(not_supported("quantization bits for PQ"));
            }
            Ok(())
        }
        QuantizationType::Rq => {
            let mut rq = Map::new();
            rq.insert("enabled".to_owned(), json!(true));
            if config.bits != 0 {
                rq.insert("bits".to_owned(), json!(config.bits));
            }
            index.insert("rq".to_owned(), Value::Object(rq));
            Ok(())
        }
        other => Err(not_supported(&format!(
            "quantization type {other} for Weaviate"
        ))),
    }
}

/// Go's `weaviateVectorConfig`: the quantization a class reports. A flag that
/// is not `enabled` reads as no quantization.
fn vector_config(index: &Map<String, Value>) -> VectorConfig {
    let enabled = |key: &str| -> Option<&Map<String, Value>> {
        index
            .get(key)
            .and_then(Value::as_object)
            .filter(|config| config.get("enabled").and_then(Value::as_bool) == Some(true))
    };
    if let Some(rq) = enabled("rq") {
        let bits = rq.get("bits").and_then(Value::as_f64).unwrap_or(0.0);
        return VectorConfig {
            quantization: QuantizationType::Rq,
            bits: bits as i64,
        };
    }
    if enabled("pq").is_some() {
        return VectorConfig {
            quantization: QuantizationType::Pq,
            bits: 0,
        };
    }
    if enabled("sq").is_some() {
        return VectorConfig {
            quantization: QuantizationType::Sq,
            bits: 0,
        };
    }
    VectorConfig::default()
}

/// The `where` clause of a GraphQL query, as Go's `WhereBuilder` writes it.
fn gql_where(path: &str, operator: &str, value: &str) -> String {
    format!(
        "{{operator: {operator}, path: [{}], {value}}}",
        gql_string(path)
    )
}

impl Weaviate {
    /// Sends a request to the API and returns its status and body text.
    async fn exchange(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, String), Error> {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(other)?;
        let status = response.status();
        let text = response.text().await.map_err(other)?;
        Ok((status, text))
    }

    /// Sends a request that must answer `200 OK`, and returns its body as JSON.
    /// A body that is empty, as a delete answers, is `null`.
    async fn expect_ok(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let (status, text) = self.exchange(method, path, body).await?;
        if status != reqwest::StatusCode::OK {
            return Err(Error::Other(format!("weaviate {path}: {status}: {text}")));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(other)
    }

    /// Runs a GraphQL query. Go's `runGraphQLQuery` reports a non-200 status as an
    /// error and the first GraphQL error as an error.
    async fn graphql(&self, query: &str) -> Result<Value, Error> {
        let response = self
            .expect_ok(
                reqwest::Method::POST,
                "/graphql",
                Some(&json!({ "query": query })),
            )
            .await?;
        if let Some(errors) = response.get("errors").and_then(Value::as_array)
            && let Some(first) = errors.first()
        {
            let message = first
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(Error::Other(message.to_owned()));
        }
        Ok(response)
    }

    /// Reads a class. A missing class is `None`, as Go's 404 check reads it.
    async fn class(&self, class: &str) -> Result<Option<Value>, Error> {
        let (status, text) = self
            .exchange(reqwest::Method::GET, &format!("/schema/{class}"), None)
            .await?;
        match status {
            reqwest::StatusCode::OK => serde_json::from_str(&text).map(Some).map_err(other),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Error::Other(format!(
                "weaviate /schema/{class}: {status}: {text}"
            ))),
        }
    }
}

#[async_trait]
impl Database for Weaviate {
    // Go's `Init` returns nil.
    async fn init(&self) -> Result<(), Error> {
        Ok(())
    }

    // Go's `Optimize` returns nil.
    async fn optimize(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    // Go's `Close` returns nil.
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error> {
        check_context(ctx)?;
        let schema = self
            .expect_ok(reqwest::Method::GET, "/schema", None)
            .await?;
        let mut names: Vec<String> = schema
            .get("classes")
            .and_then(Value::as_array)
            .map(|classes| {
                classes
                    .iter()
                    .filter_map(|class| class.get("class").and_then(Value::as_str))
                    .map(uncapitalize)
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        Ok(names)
    }

    async fn describe_collection(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<CollectionInfo, Error> {
        check_context(ctx)?;
        let Some(class) = self.class(&capitalize(name)).await? else {
            return Err(not_found(name));
        };
        let Some(index) = class.get("vectorIndexConfig").and_then(Value::as_object) else {
            return Err(Error::Other(format!(
                "failed to parse vector index config for collection {name}"
            )));
        };
        let distance = match index
            .get("distance")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "" | "cosine" => Distance::Cosine,
            "l2-squared" => Distance::Euclidean,
            "dot" => Distance::Dot,
            other => {
                return Err(not_supported(&format!("distance method {other}")));
            }
        };
        Ok(CollectionInfo {
            name: name.to_owned(),
            dimension: 0,
            distance,
            config: vector_config(index),
        })
    }

    async fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        _dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let weaviate_distance = match distance {
            Distance::Cosine => "cosine",
            Distance::Euclidean => "l2-squared",
            Distance::Dot => "dot",
        };
        let mut index = Map::new();
        index.insert("distance".to_owned(), json!(weaviate_distance));
        apply_quantization(&mut index, &config)?;

        let class = json!({
            "class": capitalize(name),
            "vectorizer": "none",
            "properties": [
                { "name": PROPERTY_ORIGINAL_ID, "dataType": ["string"] },
                { "name": PROPERTY_CATEGORIES, "dataType": ["string[]"] },
                {
                    "name": PROPERTY_HIDDEN,
                    "dataType": ["boolean"],
                    "indexFilterable": true,
                },
                {
                    "name": PROPERTY_TIMESTAMP,
                    "dataType": ["date"],
                    "indexFilterable": true,
                    "indexRangeFilters": true,
                },
            ],
            "vectorIndexConfig": Value::Object(index),
        });
        self.expect_ok(reqwest::Method::POST, "/schema", Some(&class))
            .await?;
        Ok(())
    }

    async fn delete_collection(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check_context(ctx)?;
        let class = capitalize(name);
        if self.class(&class).await?.is_none() {
            return Err(not_found(name));
        }
        self.expect_ok(reqwest::Method::DELETE, &format!("/schema/{class}"), None)
            .await?;
        Ok(())
    }

    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, Error> {
        check_context(ctx)?;
        let class = capitalize(collection);
        let response = self
            .graphql(&format!("{{Aggregate {{{class} {{meta {{count}}}}}}}}"))
            .await?;
        let count = response
            .pointer(&format!("/data/Aggregate/{class}/0/meta/count"))
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                Error::Other(format!(
                    "failed to parse aggregate response for collection {collection}"
                ))
            })?;
        Ok(count as i64)
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
        let objects: Vec<Value> = vectors
            .iter()
            .map(|vector| {
                json!({
                    "class": capitalize(collection),
                    "id": object_uuid(&vector.id),
                    "properties": {
                        PROPERTY_ORIGINAL_ID: vector.id,
                        PROPERTY_CATEGORIES: vector.categories,
                        PROPERTY_HIDDEN: vector.is_hidden,
                        PROPERTY_TIMESTAMP: vector.timestamp.format(RFC3339_NANO),
                    },
                    "vector": vector.float32_values(),
                })
            })
            .collect();
        // Go's batcher reads each object's own result, and does not check it.
        self.expect_ok(
            reqwest::Method::POST,
            "/batch/objects",
            Some(&json!({ "fields": ["ALL"], "objects": objects })),
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
        let object_ids: Vec<String> = ids.iter().map(|id| object_uuid(id)).collect();
        let object_ids: Vec<&str> = object_ids.iter().map(String::as_str).collect();
        let class = capitalize(collection);
        let query = format!(
            "{{Get {{{class} (where: {}, limit: {}) {{{PROPERTY_ORIGINAL_ID} {PROPERTY_CATEGORIES} {PROPERTY_HIDDEN} {PROPERTY_TIMESTAMP} _additional {{vector}}}}}}}}",
            gql_where(
                "id",
                "ContainsAny",
                &format!("valueString: {}", gql_strings(&object_ids))
            ),
            ids.len(),
        );
        let response = self.graphql(&query).await?;
        let items = response
            .pointer(&format!("/data/Get/{class}"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::Other(format!(
                    "failed to parse vectors for collection {collection}"
                ))
            })?;
        let mut vectors = Vec::with_capacity(items.len());
        for item in items {
            let original_id = item
                .get(PROPERTY_ORIGINAL_ID)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "failed to parse vector for collection {collection}"
                    ))
                })?;
            let mut vector = Vector {
                id: original_id.to_owned(),
                is_hidden: item
                    .get(PROPERTY_HIDDEN)
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                ..Default::default()
            };
            if let Some(categories) = item.get(PROPERTY_CATEGORIES).and_then(Value::as_array) {
                vector.categories = categories
                    .iter()
                    .map(|category| category.as_str().unwrap_or_default().to_owned())
                    .collect();
            }
            if let Some(timestamp) = item.get(PROPERTY_TIMESTAMP).and_then(Value::as_str) {
                vector.timestamp = gotime::parse(RFC3339_NANO, timestamp).map_err(other)?.utc();
            }
            let values = item
                .pointer("/_additional/vector")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "failed to parse vector values for collection {collection}"
                    ))
                })?;
            vector.values = values
                .iter()
                .map(|value| value.as_f64().unwrap_or_default() as f32)
                .collect();
            vectors.push(vector);
        }
        Ok(order_vectors(ids, vectors))
    }

    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let body = json!({
            "match": {
                "class": capitalize(collection),
                "where": {
                    "path": [PROPERTY_TIMESTAMP],
                    "operator": "LessThan",
                    "valueDate": timestamp.format(RFC3339_NANO),
                },
            },
        });
        self.expect_ok(reqwest::Method::DELETE, "/batch/objects", Some(&body))
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
        let class = capitalize(collection);
        let hidden = gql_where(PROPERTY_HIDDEN, "Equal", "valueBoolean: false");
        let filter = if categories.is_empty() {
            hidden
        } else {
            let categories = gql_where(
                PROPERTY_CATEGORIES,
                "ContainsAll",
                &format!("valueString: {}", gql_strings(categories)),
            );
            format!("{{operator: And, operands: [{hidden}, {categories}]}}")
        };
        let query = format!(
            "{{Get {{{class} (where: {filter}, nearVector: {{vector: {}}}, limit: {top_k}) {{{PROPERTY_ORIGINAL_ID} {PROPERTY_CATEGORIES} {PROPERTY_HIDDEN} _additional {{distance}}}}}}}}",
            gql_floats(&q.float32_values()),
        );
        let response = self.graphql(&query).await?;
        let items = response
            .pointer(&format!("/data/Get/{class}"))
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Other("failed to parse query response".to_owned()))?;
        Ok(items
            .iter()
            .map(|item| {
                let categories = item
                    .get(PROPERTY_CATEGORIES)
                    .and_then(Value::as_array)
                    .map(|categories| {
                        categories
                            .iter()
                            .map(|category| category.as_str().unwrap_or_default().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let distance = item
                    .pointer("/_additional/distance")
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                ScoredVector {
                    vector: Vector {
                        id: item
                            .get(PROPERTY_ORIGINAL_ID)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        is_hidden: item
                            .get(PROPERTY_HIDDEN)
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        categories,
                        ..Default::default()
                    },
                    score: -(distance as f32),
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

    /// Go's `weaviateUri`, from `WEAVIATE_URI`. Unset, the case fails rather than passes:
    /// these tests are `#[ignore]`d, so they run only when asked for, and then a
    /// missing server is an error.
    fn weaviate_dsn() -> String {
        std::env::var("WEAVIATE_URI")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("WEAVIATE_URI must name the Weaviate server; run with `-- --include-ignored`")
    }

    /// The cases of this module share one server and the collections' names, so
    /// they run one at a time (see the Qdrant tests).
    static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

    /// Runs one case on the server at `WEAVIATE_URI`, then closes the store.
    async fn run(case: impl for<'a> AsyncFnOnce(&'a Context, &'a dyn Database)) {
        let dsn = weaviate_dsn();
        let _serial = SERIAL.lock().await;
        let db = open(&dsn, "gorse_", &[] as &[Opt]).await.expect("open");
        db.init().await.expect("init");
        case(&Context::background(), db.as_ref()).await;
        db.close().await.expect("close");
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn purge() {
        run(suite::test_purge).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn collections() {
        run(suite::test_collections).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn vectors() {
        run(suite::test_vectors).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn half_vectors() {
        run(suite::test_half_vectors).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn get_vectors() {
        run(suite::test_get_vectors).await;
    }

    /// Go's `TestSparse` skips for Weaviate: "sparse vectors are not supported".
    #[tokio::test]
    #[ignore = "Go's TestSparse skips for Weaviate: sparse vectors are not supported"]
    async fn sparse() {
        run(suite::test_sparse).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn hidden() {
        run(suite::test_hidden).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn dot() {
        run(suite::test_dot).await;
    }

    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn delete_vectors() {
        run(suite::test_delete_vectors).await;
    }

    /// Go's `TestQuantization` for Weaviate: every quantization the store accepts,
    /// with the bits it accepts.
    #[tokio::test]
    #[ignore = "needs a Weaviate server at WEAVIATE_URI; run with -- --include-ignored"]
    async fn quantization() {
        run(async |ctx, db| {
            suite::test_quantization(ctx, db, QuantizationType::None, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Sq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 1).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 8).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 0).await;
        })
        .await;
    }
}
