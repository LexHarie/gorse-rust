// Ports: storage/vectors/proxy.go @ add21b5 (ProxyServer, ProxyClient, and the
// distance and config conversions)

//! The vector store over gRPC (`protocol.VectorStore`,
//! `protocol/vector_store.proto`). [`ProxyServer`] serves one backend, and
//! [`ProxyClient`] is the store a server or worker uses when
//! `database.vector_store` starts with `xvec://` and the master holds it.
//!
//! The wire `Distance` enum starts at `Unknown = 0`, and Go's in-memory enum
//! starts at `Cosine = 0`, so both directions convert (`contracts/grpc.md`).

use std::sync::Arc;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_protocol as pb;
use gorse_protocol::vector_store_client::VectorStoreClient;
use gorse_protocol::vector_store_server::{VectorStore, VectorStoreServer};
use gorse_storage_core::errors::Error as StorageError;
use tonic::transport::Channel;
use tonic::{Code, Request, Response, Status};

use super::{
    CollectionInfo, Database, Distance, Error, QuantizationType, ScoredVector, Vector,
    VectorConfig, float16_bytes, float16_from_bytes, not_found,
};
use crate::data::proxy::{optional_time_from_pb, rpc_error, time_to_pb, unknown};

/// Go's `ProxyServer`: serves one [`Database`] as the `VectorStore` gRPC service.
pub struct ProxyServer {
    database: Arc<dyn Database>,
}

impl ProxyServer {
    /// Go's `NewProxyServer`.
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }

    /// The service to add to a tonic server (Go's `RegisterVectorStoreServer`).
    pub fn into_service(self) -> VectorStoreServer<Self> {
        VectorStoreServer::new(self)
    }
}

/// Go's `distanceToProtoDistance`.
fn distance_to_pb(distance: Distance) -> pb::Distance {
    match distance {
        Distance::Cosine => pb::Distance::Cosine,
        Distance::Euclidean => pb::Distance::Euclidean,
        Distance::Dot => pb::Distance::Dot,
    }
}

/// Go's `protoDistanceToDistance`. `Unknown` and any number outside the enum
/// are refused with `distance method not supported`.
fn distance_from_pb(distance: i32) -> Result<Distance, String> {
    match pb::Distance::try_from(distance) {
        Ok(pb::Distance::Cosine) => Ok(Distance::Cosine),
        Ok(pb::Distance::Euclidean) => Ok(Distance::Euclidean),
        Ok(pb::Distance::Dot) => Ok(Distance::Dot),
        Ok(pb::Distance::Unknown) | Err(_) => {
            Err(format!("distance method {}", StorageError::NotSupported))
        }
    }
}

/// Go's `vectorConfigToProtoVectorConfig`.
fn config_to_pb(config: &VectorConfig) -> pb::VectorConfig {
    pb::VectorConfig {
        quantization_type: config.quantization.as_str().to_owned(),
        quantization_bits: config.bits as i32,
    }
}

/// Go's `protoVectorConfigToVectorConfig`: a missing config is the zero config.
fn config_from_pb(config: Option<pb::VectorConfig>) -> VectorConfig {
    match config {
        None => VectorConfig::default(),
        Some(config) => VectorConfig {
            quantization: QuantizationType::from(config.quantization_type.as_str()),
            bits: i64::from(config.quantization_bits),
        },
    }
}

/// A vector as the server sends it, with its time stamp.
fn vector_to_pb(vector: &Vector) -> pb::Vector {
    pb::Vector {
        id: vector.id.clone(),
        values: vector.values.clone(),
        h_values: float16_bytes(&vector.h_values),
        indices: vector.indices.clone(),
        is_hidden: vector.is_hidden,
        categories: vector.categories.clone(),
        timestamp: Some(time_to_pb(&vector.timestamp)),
    }
}

/// A vector as the server reads it. A missing time stamp is the zero time.
fn vector_from_pb(vector: pb::Vector) -> Vector {
    Vector {
        id: vector.id,
        values: vector.values,
        h_values: float16_from_bytes(&vector.h_values),
        indices: vector.indices,
        is_hidden: vector.is_hidden,
        categories: vector.categories,
        timestamp: optional_time_from_pb(vector.timestamp.as_ref()),
    }
}

/// The query vector as the server reads it: no id, no time stamp.
fn query_from_pb(query: pb::Vector) -> Vector {
    Vector {
        values: query.values,
        h_values: float16_from_bytes(&query.h_values),
        indices: query.indices,
        ..Default::default()
    }
}

/// A gRPC failure seen by a client, as Go's client returns it.
fn other(status: Status) -> Error {
    Error::Other(rpc_error(status).to_string())
}

#[tonic::async_trait]
impl VectorStore for ProxyServer {
    async fn list_collections(
        &self,
        _request: Request<pb::ListCollectionsRequest>,
    ) -> Result<Response<pb::ListCollectionsResponse>, Status> {
        let collections = self
            .database
            .list_collections(&Context::background())
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::ListCollectionsResponse { collections }))
    }

    async fn describe_collection(
        &self,
        request: Request<pb::DescribeCollectionRequest>,
    ) -> Result<Response<pb::DescribeCollectionResponse>, Status> {
        let name = request.into_inner().name;
        let info = match self
            .database
            .describe_collection(&Context::background(), &name)
            .await
        {
            Ok(info) => info,
            // Go returns this code, which the client maps back to ErrNotFound.
            Err(err) if err.kind() == Some(StorageError::NotFound) => {
                return Err(Status::not_found(err.to_string()));
            }
            Err(err) => return Err(unknown(err)),
        };
        Ok(Response::new(pb::DescribeCollectionResponse {
            name: info.name,
            dimensions: info.dimension as i32,
            distance: distance_to_pb(info.distance).into(),
            config: Some(config_to_pb(&info.config)),
        }))
    }

    async fn add_collection(
        &self,
        request: Request<pb::AddCollectionRequest>,
    ) -> Result<Response<pb::AddCollectionResponse>, Status> {
        let request = request.into_inner();
        let distance = distance_from_pb(request.distance).map_err(Status::unknown)?;
        let config = config_from_pb(request.config);
        self.database
            .add_collection(
                &Context::background(),
                &request.name,
                i64::from(request.dimensions),
                distance,
                config,
            )
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::AddCollectionResponse {}))
    }

    async fn delete_collection(
        &self,
        request: Request<pb::DeleteCollectionRequest>,
    ) -> Result<Response<pb::DeleteCollectionResponse>, Status> {
        self.database
            .delete_collection(&Context::background(), &request.into_inner().name)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteCollectionResponse {}))
    }

    async fn count_vectors(
        &self,
        request: Request<pb::CountVectorsRequest>,
    ) -> Result<Response<pb::CountVectorsResponse>, Status> {
        let count = self
            .database
            .count_vectors(&Context::background(), &request.into_inner().collection)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::CountVectorsResponse { count }))
    }

    async fn add_vectors(
        &self,
        request: Request<pb::AddVectorsRequest>,
    ) -> Result<Response<pb::AddVectorsResponse>, Status> {
        let request = request.into_inner();
        let vectors: Vec<Vector> = request.vectors.into_iter().map(vector_from_pb).collect();
        self.database
            .add_vectors(&Context::background(), &request.collection, &vectors)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::AddVectorsResponse {}))
    }

    async fn get_vectors(
        &self,
        request: Request<pb::GetVectorsRequest>,
    ) -> Result<Response<pb::GetVectorsResponse>, Status> {
        let request = request.into_inner();
        let ids: Vec<&str> = request.ids.iter().map(String::as_str).collect();
        let vectors = self
            .database
            .get_vectors(&Context::background(), &request.collection, &ids)
            .await
            .map_err(unknown)?;
        let vectors = vectors.iter().map(vector_to_pb).collect();
        Ok(Response::new(pb::GetVectorsResponse { vectors }))
    }

    async fn delete_vectors(
        &self,
        request: Request<pb::DeleteVectorsRequest>,
    ) -> Result<Response<pb::DeleteVectorsResponse>, Status> {
        let request = request.into_inner();
        let timestamp = optional_time_from_pb(request.timestamp.as_ref());
        self.database
            .delete_vectors(&Context::background(), &request.collection, &timestamp)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteVectorsResponse {}))
    }

    async fn query_vectors(
        &self,
        request: Request<pb::QueryVectorsRequest>,
    ) -> Result<Response<pb::QueryVectorsResponse>, Status> {
        let request = request.into_inner();
        let query = query_from_pb(request.query.unwrap_or_default());
        let categories: Vec<&str> = request.categories.iter().map(String::as_str).collect();
        let results = self
            .database
            .query_vectors(
                &Context::background(),
                &request.collection,
                &query,
                &categories,
                i64::from(request.top_k),
            )
            .await
            .map_err(unknown)?;
        // Go's QueryVectors reply carries no time stamp.
        let vectors = results
            .into_iter()
            .map(|result| pb::ScoredVector {
                vector: Some(pb::Vector {
                    id: result.vector.id,
                    values: result.vector.values,
                    h_values: float16_bytes(&result.vector.h_values),
                    indices: result.vector.indices,
                    is_hidden: result.vector.is_hidden,
                    categories: result.vector.categories,
                    timestamp: None,
                }),
                score: result.score,
            })
            .collect();
        Ok(Response::new(pb::QueryVectorsResponse { vectors }))
    }
}

/// Go's `ProxyClient`: a [`Database`] that calls a remote [`ProxyServer`].
///
/// `Init`, `Optimize` and `Close` do nothing, as in Go.
pub struct ProxyClient {
    client: VectorStoreClient<Channel>,
}

impl ProxyClient {
    /// Go's `NewProxyClient`.
    pub fn new(channel: Channel) -> Self {
        Self {
            client: VectorStoreClient::new(channel),
        }
    }

    /// A handle to the gRPC client for one call (see the data store's client).
    fn grpc(&self) -> VectorStoreClient<Channel> {
        self.client.clone()
    }
}

#[async_trait]
impl Database for ProxyClient {
    async fn init(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn optimize(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn list_collections(&self, _ctx: &Context) -> Result<Vec<String>, Error> {
        let response = self
            .grpc()
            .list_collections(pb::ListCollectionsRequest {})
            .await
            .map_err(other)?
            .into_inner();
        Ok(response.collections)
    }

    async fn describe_collection(
        &self,
        _ctx: &Context,
        name: &str,
    ) -> Result<CollectionInfo, Error> {
        let response = match self
            .grpc()
            .describe_collection(pb::DescribeCollectionRequest {
                name: name.to_owned(),
            })
            .await
        {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == Code::NotFound => return Err(not_found(name)),
            Err(status) => return Err(other(status)),
        };
        let distance = distance_from_pb(response.distance).map_err(Error::Other)?;
        Ok(CollectionInfo {
            name: response.name,
            dimension: i64::from(response.dimensions),
            distance,
            config: config_from_pb(response.config),
        })
    }

    async fn add_collection(
        &self,
        _ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), Error> {
        self.grpc()
            .add_collection(pb::AddCollectionRequest {
                name: name.to_owned(),
                dimensions: dimensions as i32,
                distance: distance_to_pb(distance).into(),
                config: Some(config_to_pb(&config)),
            })
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn delete_collection(&self, _ctx: &Context, name: &str) -> Result<(), Error> {
        self.grpc()
            .delete_collection(pb::DeleteCollectionRequest {
                name: name.to_owned(),
            })
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn count_vectors(&self, _ctx: &Context, collection: &str) -> Result<i64, Error> {
        let response = self
            .grpc()
            .count_vectors(pb::CountVectorsRequest {
                collection: collection.to_owned(),
            })
            .await
            .map_err(other)?
            .into_inner();
        Ok(response.count)
    }

    async fn add_vectors(
        &self,
        _ctx: &Context,
        collection: &str,
        vectors: &[Vector],
    ) -> Result<(), Error> {
        self.grpc()
            .add_vectors(pb::AddVectorsRequest {
                collection: collection.to_owned(),
                vectors: vectors.iter().map(vector_to_pb).collect(),
            })
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn get_vectors(
        &self,
        _ctx: &Context,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<Vector>, Error> {
        let response = self
            .grpc()
            .get_vectors(pb::GetVectorsRequest {
                collection: collection.to_owned(),
                ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            })
            .await
            .map_err(other)?
            .into_inner();
        Ok(response.vectors.into_iter().map(vector_from_pb).collect())
    }

    async fn delete_vectors(
        &self,
        _ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error> {
        self.grpc()
            .delete_vectors(pb::DeleteVectorsRequest {
                collection: collection.to_owned(),
                timestamp: Some(time_to_pb(timestamp)),
            })
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn query_vectors(
        &self,
        _ctx: &Context,
        collection: &str,
        q: &Vector,
        categories: &[&str],
        top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error> {
        let response = self
            .grpc()
            .query_vectors(pb::QueryVectorsRequest {
                collection: collection.to_owned(),
                query: Some(pb::Vector {
                    values: q.values.clone(),
                    h_values: float16_bytes(&q.h_values),
                    indices: q.indices.clone(),
                    ..Default::default()
                }),
                categories: categories.iter().map(|c| (*c).to_owned()).collect(),
                top_k: top_k as i32,
            })
            .await
            .map_err(other)?
            .into_inner();
        Ok(response
            .vectors
            .into_iter()
            .map(|scored| {
                let vector = scored.vector.unwrap_or_default();
                ScoredVector {
                    vector: Vector {
                        id: vector.id,
                        values: vector.values,
                        h_values: float16_from_bytes(&vector.h_values),
                        indices: vector.indices,
                        is_hidden: vector.is_hidden,
                        categories: vector.categories,
                        timestamp: Time::default(),
                    },
                    score: scored.score,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tonic::transport::Server;

    use super::*;
    use crate::data::proxy::support::connect;
    use crate::vectors::{open, suite};
    use gorse_storage_core::options::Opt;
    use gorse_storage_core::scheme::XVEC_PREFIX;

    /// A client of an xvec store in `dir`, served through the proxy. The store
    /// is initialised, as Go's `ProxyTestSuite.SetupSuite` does.
    async fn client_of_xvec(dir: &tempfile::TempDir) -> ProxyClient {
        let backend = open(
            &format!("{XVEC_PREFIX}{}", dir.path().display()),
            "gorse_",
            &[] as &[Opt],
        )
        .await
        .expect("open");
        backend.init().await.expect("init");
        let router =
            Server::builder().add_service(ProxyServer::new(Arc::from(backend)).into_service());
        ProxyClient::new(connect(router).await)
    }

    /// Runs one case of Go's vectors suite through the proxy, on a new store.
    async fn run(case: impl for<'a> AsyncFnOnce(&'a Context, &'a dyn Database)) {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = client_of_xvec(&dir).await;
        let db: &dyn Database = &client;
        case(&Context::background(), db).await;
    }

    // The cases of Go's `ProxyTestSuite`, which embeds `vectorsTestSuite`.

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

    /// Go's client maps the NotFound code back to `ErrNotFound` with its own
    /// text, `collection <name>: not found`.
    #[tokio::test]
    async fn missing_collection_is_not_found_with_go_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = client_of_xvec(&dir).await;
        let err = client
            .describe_collection(&Context::background(), "missing")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotFound));
        assert_eq!(err.to_string(), "collection missing: not found");
    }

    /// The server refuses a distance outside the enum, as Go does, with the
    /// error as a plain `Unknown` status.
    #[tokio::test]
    async fn unknown_distance_is_refused_as_go_refuses_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = client_of_xvec(&dir).await;
        let status = client
            .grpc()
            .add_collection(pb::AddCollectionRequest {
                name: "c".to_owned(),
                dimensions: 4,
                distance: 0,
                config: None,
            })
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::Unknown);
        assert_eq!(status.message(), "distance method not supported");
    }

    #[test]
    fn distances_convert_both_ways_and_refuse_unknown() {
        for distance in [Distance::Cosine, Distance::Euclidean, Distance::Dot] {
            assert_eq!(
                distance_from_pb(distance_to_pb(distance).into()),
                Ok(distance)
            );
        }
        assert_eq!(
            distance_from_pb(0),
            Err("distance method not supported".to_owned())
        );
        assert_eq!(
            distance_from_pb(9),
            Err("distance method not supported".to_owned())
        );
    }

    #[test]
    fn a_missing_config_is_the_zero_config() {
        assert_eq!(config_from_pb(None), VectorConfig::default());
    }
}
