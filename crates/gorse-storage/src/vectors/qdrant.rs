// Ports: storage/vectors/qdrant.go @ add21b5, storage/vectors/qdrant_test.go @ add21b5

//! The Qdrant store, `qdrant://host:port`, over Qdrant's gRPC API through the
//! `qdrant-client` crate. Go's client speaks the same gRPC API, so the
//! collections, named vector `vector`, payload keys and point ids are the
//! ones Go writes, and each store reads what the other wrote (`PORTING.md`
//! WP-2-9).
//!
//! The Go `Init` and `Optimize` do nothing and `Close` closes the connection.
//! The Rust client closes when the store is dropped, so `close` does nothing
//! more. The table prefix is ignored, as Go ignores it (`PORTING_NOTES.md`
//! Q-24).

use std::collections::HashMap;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_storage_core::errors::Error as StorageError;
use qdrant_client::Qdrant as Client;
use qdrant_client::qdrant;
use tonic::Code;
use url::Url;

use super::{
    CollectionInfo, Database, Distance, Error, QuantizationType, ScoredVector, Vector,
    VectorConfig, check_context, not_found, not_supported, order_vectors,
};

/// The payload key of a vector's categories.
const PAYLOAD_CATEGORIES: &str = "categories";
/// The payload key of a vector's hidden flag.
const PAYLOAD_HIDDEN: &str = "hidden";
/// The payload key of a vector's id.
const PAYLOAD_ID: &str = "id";
/// The payload key of a vector's timestamp, in milliseconds.
const PAYLOAD_TIMESTAMP: &str = "timestamp";
/// The name of the one vector in each point, and of the vector configuration.
const VECTOR_NAME: &str = "vector";

/// Go's `storage.QdrantPrefix` path, parsed to the host and port the gRPC client
/// dials. Go's `NewClient` takes the port as an integer, so a missing port is an
/// error.
pub(crate) fn connect(path: &str) -> Result<Qdrant, Error> {
    let url = Url::parse(path).map_err(|err| Error::Other(err.to_string()))?;
    let host = url.host_str().unwrap_or_default();
    let port = match url.port() {
        Some(port) => port,
        None => {
            return Err(Error::Other(
                "strconv.Atoi: parsing \"\": invalid syntax".to_owned(),
            ));
        }
    };
    let client = Client::from_url(&format!("http://{host}:{port}"))
        .build()
        .map_err(other)?;
    Ok(Qdrant { client })
}

/// A Qdrant store (Go's `Qdrant`).
pub struct Qdrant {
    client: Client,
}

/// Go's `errors.WithStack`: the error's own text, with no storage sentinel.
fn other(err: impl std::fmt::Display) -> Error {
    Error::Other(err.to_string())
}

/// Whether a Qdrant error is gRPC's `NotFound`, as Go's `status.Code` tests.
fn is_not_found(err: &qdrant_client::QdrantError) -> bool {
    matches!(
        err,
        qdrant_client::QdrantError::ResponseError { status } if status.code() == Code::NotFound
    )
}

/// The name of a proto enum value, or its number when the value is unknown, as
/// Go's `String()` prints it.
fn enum_name(value: i32, name: Option<&'static str>) -> String {
    name.map_or_else(|| value.to_string(), str::to_owned)
}

/// Go's `TurboQuantBitSize.String()` for the values of the Go client's enum, which
/// has no 8-bit value. Other values print as their number.
fn go_turbo_bits_name(value: i32) -> String {
    match value {
        0 => "Bits1".to_owned(),
        1 => "Bits1_5".to_owned(),
        2 => "Bits2".to_owned(),
        3 => "Bits4".to_owned(),
        other => other.to_string(),
    }
}

/// The UUID of a vector id: an MD5 name-based UUID in the URL namespace, as Go's
/// `uuid.NewMD5(uuid.NameSpaceURL, ...)`.
pub(crate) fn point_uuid(id: &str) -> String {
    uuid::Uuid::new_v3(&uuid::Uuid::NAMESPACE_URL, id.as_bytes()).to_string()
}

/// Go's `qdrantQuantizationConfig`: the quantization the collection is created
/// with. `None` for no quantization.
fn quantization_config(
    config: &VectorConfig,
) -> Result<Option<qdrant::quantization_config::Quantization>, Error> {
    match &config.quantization {
        QuantizationType::None => Ok(None),
        QuantizationType::Rq => {
            let mut turbo = qdrant::TurboQuantization::default();
            if config.bits != 0 {
                let bits = match config.bits {
                    1 => qdrant::TurboQuantBitSize::Bits1,
                    2 => qdrant::TurboQuantBitSize::Bits2,
                    4 => qdrant::TurboQuantBitSize::Bits4,
                    other => {
                        return Err(not_supported(&format!(
                            "RQ quantization bits {other} for Qdrant"
                        )));
                    }
                };
                turbo.bits = Some(bits as i32);
            }
            Ok(Some(qdrant::quantization_config::Quantization::Turboquant(
                turbo,
            )))
        }
        QuantizationType::Sq => {
            if config.bits != 0 && config.bits != 8 {
                return Err(not_supported("SQ quantization bits for Qdrant"));
            }
            Ok(Some(qdrant::quantization_config::Quantization::Scalar(
                qdrant::ScalarQuantization {
                    r#type: qdrant::QuantizationType::Int8 as i32,
                    ..Default::default()
                },
            )))
        }
        QuantizationType::Pq => {
            let mut product = qdrant::ProductQuantization::default();
            if config.bits != 0 {
                product.compression = match config.bits {
                    8 => qdrant::CompressionRatio::X4,
                    4 => qdrant::CompressionRatio::X8,
                    2 => qdrant::CompressionRatio::X16,
                    1 => qdrant::CompressionRatio::X32,
                    other => {
                        return Err(not_supported(&format!(
                            "PQ quantization bits {other} for Qdrant"
                        )));
                    }
                } as i32;
            }
            Ok(Some(qdrant::quantization_config::Quantization::Product(
                product,
            )))
        }
        other => Err(not_supported(&format!(
            "quantization type {other} for Qdrant"
        ))),
    }
}

/// Go's `qdrantVectorConfig`: the quantization a collection reports.
fn vector_config(config: Option<&qdrant::QuantizationConfig>) -> Result<VectorConfig, Error> {
    let Some(quantization) = config.and_then(|config| config.quantization.as_ref()) else {
        return Ok(VectorConfig::default());
    };
    match quantization {
        qdrant::quantization_config::Quantization::Turboquant(turbo) => {
            let mut bits = 0;
            if let Some(value) = turbo.bits {
                bits = match qdrant::TurboQuantBitSize::try_from(value) {
                    Ok(qdrant::TurboQuantBitSize::Bits1) => 1,
                    Ok(qdrant::TurboQuantBitSize::Bits2) => 2,
                    Ok(qdrant::TurboQuantBitSize::Bits4) => 4,
                    _ => {
                        return Err(not_supported(&format!(
                            "RQ quantization bits {} for Qdrant",
                            go_turbo_bits_name(value)
                        )));
                    }
                };
            }
            Ok(VectorConfig {
                quantization: QuantizationType::Rq,
                bits,
            })
        }
        qdrant::quantization_config::Quantization::Scalar(scalar) => {
            if scalar.r#type != qdrant::QuantizationType::Int8 as i32 {
                let name = enum_name(
                    scalar.r#type,
                    qdrant::QuantizationType::try_from(scalar.r#type)
                        .ok()
                        .map(|kind| kind.as_str_name()),
                );
                return Err(not_supported(&format!(
                    "SQ quantization type {name} for Qdrant"
                )));
            }
            Ok(VectorConfig {
                quantization: QuantizationType::Sq,
                bits: 8,
            })
        }
        qdrant::quantization_config::Quantization::Product(product) => {
            let bits = match qdrant::CompressionRatio::try_from(product.compression) {
                Ok(qdrant::CompressionRatio::X4) => 8,
                Ok(qdrant::CompressionRatio::X8) => 4,
                Ok(qdrant::CompressionRatio::X16) => 2,
                Ok(qdrant::CompressionRatio::X32) => 1,
                _ => {
                    let name = enum_name(
                        product.compression,
                        qdrant::CompressionRatio::try_from(product.compression)
                            .ok()
                            .map(|ratio| ratio.as_str_name()),
                    );
                    return Err(not_supported(&format!(
                        "PQ quantization compression {name} for Qdrant"
                    )));
                }
            };
            Ok(VectorConfig {
                quantization: QuantizationType::Pq,
                bits,
            })
        }
        qdrant::quantization_config::Quantization::Binary(_) => {
            Err(not_supported("binary quantization for Qdrant"))
        }
    }
}

/// The values and the indices of a point's named vector, as Go's `qdrantVector`
/// reads them: a sparse vector's values and indices, or a dense vector's values
/// with no indices. Both are empty when the point has no `vector`.
fn point_vector(output: Option<&qdrant::VectorsOutput>) -> (Vec<f32>, Vec<u32>) {
    let named = match output.and_then(|output| output.vectors_options.as_ref()) {
        Some(qdrant::vectors_output::VectorsOptions::Vectors(named)) => named,
        _ => return (Vec::new(), Vec::new()),
    };
    let Some(vector) = named.vectors.get(VECTOR_NAME) else {
        return (Vec::new(), Vec::new());
    };
    match vector.clone().into_vector() {
        qdrant::vector_output::Vector::Sparse(sparse) => (sparse.values, sparse.indices),
        qdrant::vector_output::Vector::Dense(dense) => (dense.data, Vec::new()),
        _ => (Vec::new(), Vec::new()),
    }
}

/// The id a payload holds, or empty when it has none.
fn payload_id(payload: &HashMap<String, qdrant::Value>) -> String {
    payload
        .get(PAYLOAD_ID)
        .map(string_value)
        .unwrap_or_default()
}

/// The string a payload value holds, or empty if it holds another kind.
fn string_value(value: &qdrant::Value) -> String {
    match &value.kind {
        Some(qdrant::value::Kind::StringValue(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Whether a payload's hidden flag is set. A missing flag is not set.
fn payload_hidden(payload: &HashMap<String, qdrant::Value>) -> bool {
    matches!(
        payload
            .get(PAYLOAD_HIDDEN)
            .and_then(|value| value.kind.as_ref()),
        Some(qdrant::value::Kind::BoolValue(true))
    )
}

/// A payload's timestamp, in UTC. A missing timestamp is the zero time.
fn payload_timestamp(payload: &HashMap<String, qdrant::Value>) -> Time {
    match payload
        .get(PAYLOAD_TIMESTAMP)
        .and_then(|value| value.kind.as_ref())
    {
        Some(qdrant::value::Kind::IntegerValue(millis)) => gotime::unix_milli(*millis).utc(),
        _ => Time::default(),
    }
}

/// A payload's categories. Each list item is read as a string, as Go's
/// `GetStringValue` reads it: an item of another kind gives an empty string.
fn payload_categories(payload: &HashMap<String, qdrant::Value>) -> Vec<String> {
    match payload
        .get(PAYLOAD_CATEGORIES)
        .and_then(|value| value.kind.as_ref())
    {
        Some(qdrant::value::Kind::ListValue(list)) => {
            list.values.iter().map(string_value).collect()
        }
        _ => Vec::new(),
    }
}

/// The payload value of a list of categories.
fn categories_value(categories: &[String]) -> qdrant::Value {
    qdrant::Value {
        kind: Some(qdrant::value::Kind::ListValue(qdrant::ListValue {
            values: categories
                .iter()
                .map(|category| category.as_str().into())
                .collect(),
        })),
    }
}

/// Go's `createCollection` field indexes: the three payload fields the queries
/// filter on, each created with `wait`.
const FIELD_INDEXES: [(&str, qdrant::FieldType); 3] = [
    (PAYLOAD_TIMESTAMP, qdrant::FieldType::Integer),
    (PAYLOAD_HIDDEN, qdrant::FieldType::Bool),
    (PAYLOAD_CATEGORIES, qdrant::FieldType::Keyword),
];

#[async_trait]
impl Database for Qdrant {
    // Go's `Init` returns nil.
    async fn init(&self) -> Result<(), Error> {
        Ok(())
    }

    // Go's `Optimize` returns nil.
    async fn optimize(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    // The client closes when the store is dropped (see the module doc).
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error> {
        check_context(ctx)?;
        let response = self.client.list_collections().await.map_err(other)?;
        let mut names: Vec<String> = response
            .collections
            .into_iter()
            .map(|collection| collection.name)
            .collect();
        names.sort();
        Ok(names)
    }

    async fn describe_collection(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<CollectionInfo, Error> {
        check_context(ctx)?;
        let info = match self.client.collection_info(name).await {
            Ok(info) => info,
            Err(err) if is_not_found(&err) => return Err(not_found(name)),
            Err(err) => return Err(other(err)),
        };
        let config = info.result.and_then(|info| info.config);
        let params = config.as_ref().and_then(|config| config.params.as_ref());
        let sparse = params
            .and_then(|params| params.sparse_vectors_config.as_ref())
            .is_some_and(|sparse| sparse.map.contains_key(VECTOR_NAME));
        if sparse {
            return Ok(CollectionInfo {
                name: name.to_owned(),
                dimension: 0,
                distance: Distance::Dot,
                config: VectorConfig::default(),
            });
        }
        let dense = params
            .and_then(|params| params.vectors_config.as_ref())
            .and_then(|vectors| match &vectors.config {
                Some(qdrant::vectors_config::Config::ParamsMap(map)) => map.map.get(VECTOR_NAME),
                _ => None,
            });
        let Some(dense) = dense else {
            return Err(Error::Storage {
                kind: StorageError::NotFound,
                message: format!("vector field {VECTOR_NAME}: {}", StorageError::NotFound),
            });
        };
        let distance = match qdrant::Distance::try_from(dense.distance) {
            Ok(qdrant::Distance::Cosine) => Distance::Cosine,
            Ok(qdrant::Distance::Euclid) => Distance::Euclidean,
            Ok(qdrant::Distance::Dot) => Distance::Dot,
            _ => {
                let method = enum_name(
                    dense.distance,
                    qdrant::Distance::try_from(dense.distance)
                        .ok()
                        .map(|distance| distance.as_str_name()),
                );
                return Err(not_supported(&format!("distance method {method}")));
            }
        };
        let config = vector_config(
            config
                .as_ref()
                .and_then(|config| config.quantization_config.as_ref()),
        )?;
        Ok(CollectionInfo {
            name: name.to_owned(),
            dimension: dense.size as i64,
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
            let sparse = qdrant::SparseVectorConfig {
                map: HashMap::from([(
                    VECTOR_NAME.to_owned(),
                    qdrant::SparseVectorParams::default(),
                )]),
            };
            return self
                .create_collection(
                    name,
                    qdrant::CreateCollectionBuilder::new(name).sparse_vectors_config(sparse),
                )
                .await;
        }
        let qdrant_distance = match distance {
            Distance::Cosine => qdrant::Distance::Cosine,
            Distance::Euclidean => qdrant::Distance::Euclid,
            Distance::Dot => qdrant::Distance::Dot,
        };
        let quantization = quantization_config(&config)?;
        let vector_params = qdrant::VectorParamsBuilder::new(dimensions as u64, qdrant_distance)
            .datatype(qdrant::Datatype::Float16);
        let mut vectors = qdrant::VectorsConfigBuilder::default();
        vectors.add_named_vector_params(VECTOR_NAME, vector_params);
        let mut request = qdrant::CreateCollectionBuilder::new(name).vectors_config(vectors);
        if let Some(quantization) = quantization {
            request = request.quantization_config(quantization);
        }
        self.create_collection(name, request).await
    }

    async fn delete_collection(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check_context(ctx)?;
        let response = self.client.delete_collection(name).await.map_err(other)?;
        // Go's client turns a `false` result into an error. The Rust client
        // returns the flag as it is, so the check is repeated here.
        if !response.result {
            return Err(Error::Other(format!(
                "DeleteCollection: {name}: failed to delete collection"
            )));
        }
        Ok(())
    }

    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, Error> {
        check_context(ctx)?;
        let response = self
            .client
            .count(qdrant::CountPointsBuilder::new(collection).exact(true))
            .await
            .map_err(other)?;
        Ok(response.result.map_or(0, |result| result.count) as i64)
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
        let points = vectors
            .iter()
            .map(|vector| {
                let value = if vector.is_sparse() {
                    qdrant::Vector::new_sparse(vector.indices.clone(), vector.values.clone())
                } else {
                    qdrant::Vector::new_dense(vector.float32_values().into_owned())
                };
                let payload = HashMap::from([
                    (
                        PAYLOAD_CATEGORIES.to_owned(),
                        categories_value(&vector.categories),
                    ),
                    (PAYLOAD_HIDDEN.to_owned(), vector.is_hidden.into()),
                    (PAYLOAD_ID.to_owned(), vector.id.as_str().into()),
                    (
                        PAYLOAD_TIMESTAMP.to_owned(),
                        vector.timestamp.unix_milli().into(),
                    ),
                ]);
                qdrant::PointStruct::new(
                    point_uuid(&vector.id),
                    qdrant::NamedVectors::default().add_vector(VECTOR_NAME, value),
                    payload,
                )
            })
            .collect::<Vec<_>>();
        self.client
            .upsert_points(qdrant::UpsertPointsBuilder::new(collection, points).wait(true))
            .await
            .map_err(other)?;
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
        let point_ids: Vec<qdrant::PointId> = ids.iter().map(|id| point_uuid(id).into()).collect();
        let response = self
            .client
            .get_points(
                qdrant::GetPointsBuilder::new(collection, point_ids)
                    .with_payload(true)
                    .with_vectors(qdrant::VectorsSelector {
                        names: vec![VECTOR_NAME.to_owned()],
                    }),
            )
            .await
            .map_err(other)?;
        let vectors = response
            .result
            .iter()
            .map(|point| {
                let (values, indices) = point_vector(point.vectors.as_ref());
                Vector {
                    id: payload_id(&point.payload),
                    values,
                    indices,
                    is_hidden: payload_hidden(&point.payload),
                    categories: payload_categories(&point.payload),
                    timestamp: payload_timestamp(&point.payload),
                    ..Default::default()
                }
            })
            .collect();
        Ok(order_vectors(ids, vectors))
    }

    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error> {
        check_context(ctx)?;
        let before = timestamp.unix_milli() as f64;
        self.client
            .delete_points(
                qdrant::DeletePointsBuilder::new(collection)
                    .points(qdrant::Filter::must([qdrant::Condition::range(
                        PAYLOAD_TIMESTAMP,
                        qdrant::Range {
                            lt: Some(before),
                            ..Default::default()
                        },
                    )]))
                    .wait(true),
            )
            .await
            .map_err(other)?;
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
        let mut must = vec![qdrant::Condition::matches(PAYLOAD_HIDDEN, false)];
        for category in categories {
            must.push(qdrant::Condition::matches(
                PAYLOAD_CATEGORIES,
                (*category).to_owned(),
            ));
        }
        let query = if q.is_sparse() {
            let pairs: Vec<(u32, f32)> = q
                .indices
                .iter()
                .copied()
                .zip(q.values.iter().copied())
                .collect();
            qdrant::Query::new_nearest(pairs.as_slice())
        } else {
            qdrant::Query::new_nearest(q.float32_values().into_owned())
        };
        let response = self
            .client
            .query(
                qdrant::QueryPointsBuilder::new(collection)
                    .query(query)
                    .using(VECTOR_NAME)
                    .filter(qdrant::Filter::must(must))
                    .limit(top_k as u64)
                    .with_payload(true)
                    .with_vectors(qdrant::VectorsSelector {
                        names: vec![VECTOR_NAME.to_owned()],
                    }),
            )
            .await
            .map_err(other)?;
        Ok(response
            .result
            .iter()
            .map(|scored| {
                let (values, indices) = point_vector(scored.vectors.as_ref());
                ScoredVector {
                    vector: Vector {
                        id: payload_id(&scored.payload),
                        values,
                        indices,
                        is_hidden: payload_hidden(&scored.payload),
                        categories: payload_categories(&scored.payload),
                        ..Default::default()
                    },
                    score: scored.score,
                }
            })
            .collect())
    }
}

impl Qdrant {
    /// Go's `createCollection`: creates the collection, then its three field
    /// indexes. A failed index leaves the collection in place, as in Go.
    async fn create_collection(
        &self,
        name: &str,
        request: qdrant::CreateCollectionBuilder,
    ) -> Result<(), Error> {
        self.client
            .create_collection(request)
            .await
            .map_err(other)?;
        for (field, field_type) in FIELD_INDEXES {
            self.client
                .create_field_index(
                    qdrant::CreateFieldIndexCollectionBuilder::new(name, field, field_type)
                        .wait(true),
                )
                .await
                .map_err(other)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;
    use crate::vectors::{open, suite};
    use gorse_storage_core::options::Opt;

    /// Go's `qdrantUri`, from `QDRANT_URI`. Unset, the case fails rather than passes:
    /// these tests are `#[ignore]`d, so they run only when asked for, and then a
    /// missing server is an error.
    fn qdrant_dsn() -> String {
        std::env::var("QDRANT_URI")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("QDRANT_URI must name the Qdrant server; run with `-- --include-ignored`")
    }

    /// Qdrant has one collection namespace per server and the table prefix does
    /// not apply (Q-24), so the cases of this module run one at a time: each one
    /// purges the server before it starts, as Go's `SetupTest` does.
    static SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

    /// Runs one case on the server at `QDRANT_URI`, then closes the store.
    async fn run(case: impl for<'a> AsyncFnOnce(&'a Context, &'a dyn Database)) {
        let dsn = qdrant_dsn();
        let _serial = SERIAL.lock().await;
        let db = open(&dsn, "gorse_", &[] as &[Opt]).await.expect("open");
        db.init().await.expect("init");
        case(&Context::background(), db.as_ref()).await;
        db.close().await.expect("close");
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn purge() {
        run(suite::test_purge).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn collections() {
        run(suite::test_collections).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn vectors() {
        run(suite::test_vectors).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn half_vectors() {
        run(suite::test_half_vectors).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn get_vectors() {
        run(suite::test_get_vectors).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn sparse() {
        run(suite::test_sparse).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn hidden() {
        run(suite::test_hidden).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn dot() {
        run(suite::test_dot).await;
    }

    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn delete_vectors() {
        run(suite::test_delete_vectors).await;
    }

    /// Go's `TestQuantization` for Qdrant: every quantization the store accepts,
    /// with the bits it accepts.
    #[tokio::test]
    #[ignore = "needs a Qdrant server at QDRANT_URI; run with -- --include-ignored"]
    async fn quantization() {
        run(async |ctx, db| {
            suite::test_quantization(ctx, db, QuantizationType::None, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 1).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 2).await;
            suite::test_quantization(ctx, db, QuantizationType::Rq, 4).await;
            suite::test_quantization(ctx, db, QuantizationType::Sq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Sq, 8).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 0).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 1).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 2).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 4).await;
            suite::test_quantization(ctx, db, QuantizationType::Pq, 8).await;
        })
        .await;
    }
}
