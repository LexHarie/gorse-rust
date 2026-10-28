// Ports: storage/cache/proxy.go @ add21b5 (ProxyServer, ProxyClient)

//! The cache store over gRPC (`protocol.CacheStore`, `protocol/cache_store.proto`).
//! [`ProxyServer`] serves one backend, and [`ProxyClient`] is the store a
//! server or worker uses when `database.cache_store` starts with `sqlite://`.
//!
//! Values travel as the strings `cache-keys.md` describes. `Init`, `Scan` and
//! `Purge` are refused by the client, as in Go. A scan of the score documents
//! streams them as the backend finds them.

use std::sync::Arc;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{Duration, Time};
use gorse_protocol as pb;
use gorse_protocol::cache_store_client::CacheStoreClient;
use gorse_protocol::cache_store_server::{CacheStore, CacheStoreServer};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use super::{
    Database, Error, ReturnValue, Score, ScoreCondition, ScorePatch, TimeSeriesPoint,
    new_return_error, new_return_value, string,
};
use crate::data::proxy::{optional_strings, rpc_error, time_from_pb, time_to_pb, unknown};

/// Go's `ProxyServer`: serves one [`Database`] as the `CacheStore` gRPC service.
pub struct ProxyServer {
    database: Arc<dyn Database>,
}

impl ProxyServer {
    /// Go's `NewProxyServer`.
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }

    /// The service to add to a tonic server (Go's `RegisterCacheStoreServer`).
    pub fn into_service(self) -> CacheStoreServer<Self> {
        CacheStoreServer::new(self)
    }
}

fn score_to_pb(score: &Score) -> pb::Score {
    pb::Score {
        id: score.id.clone(),
        score: score.score,
        is_hidden: score.is_hidden,
        categories: score.categories.clone(),
        timestamp: Some(time_to_pb(&score.timestamp)),
    }
}

fn score_from_pb(score: pb::Score) -> Score {
    Score {
        id: score.id,
        score: score.score,
        is_hidden: score.is_hidden,
        categories: score.categories,
        timestamp: time_from_pb(score.timestamp.as_ref()),
    }
}

fn time_series_point_to_pb(point: &TimeSeriesPoint) -> pb::TimeSeriesPoint {
    pb::TimeSeriesPoint {
        name: point.name.clone(),
        timestamp: Some(time_to_pb(&point.timestamp)),
        value: point.value,
    }
}

fn time_series_point_from_pb(point: pb::TimeSeriesPoint) -> TimeSeriesPoint {
    TimeSeriesPoint {
        name: point.name,
        timestamp: time_from_pb(point.timestamp.as_ref()),
        value: point.value,
    }
}

/// A gRPC failure seen by a client.
fn backend(status: Status) -> Error {
    Error::Backend(Arc::new(rpc_error(status)))
}

/// The error of a client method that Go refuses, as `errors.Errorf` makes it.
fn refused(message: &str) -> Error {
    Error::Backend(Arc::new(std::io::Error::other(message.to_owned())))
}

#[tonic::async_trait]
impl CacheStore for ProxyServer {
    type ScanScoresStream =
        futures_core::stream::BoxStream<'static, Result<pb::ScanScoresResponse, Status>>;

    async fn ping(
        &self,
        _request: Request<pb::PingRequest>,
    ) -> Result<Response<pb::PingResponse>, Status> {
        self.database.ping().await.map_err(unknown)?;
        Ok(Response::new(pb::PingResponse {}))
    }

    async fn get(
        &self,
        request: Request<pb::GetRequest>,
    ) -> Result<Response<pb::GetResponse>, Status> {
        let name = request.into_inner().name;
        let value = self.database.get(&Context::background(), &name).await;
        // Go answers a missing key with no value and no error, even when the
        // backend reported an error along with it.
        if !value.exists() {
            return Ok(Response::new(pb::GetResponse { value: None }));
        }
        let value = value.string().map_err(unknown)?;
        Ok(Response::new(pb::GetResponse { value: Some(value) }))
    }

    async fn set(
        &self,
        request: Request<pb::SetRequest>,
    ) -> Result<Response<pb::SetResponse>, Status> {
        let values: Vec<_> = request
            .into_inner()
            .values
            .into_iter()
            .map(|value| string(value.name, value.value))
            .collect();
        self.database
            .set(&Context::background(), &values)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::SetResponse {}))
    }

    async fn delete(
        &self,
        request: Request<pb::DeleteRequest>,
    ) -> Result<Response<pb::DeleteResponse>, Status> {
        self.database
            .delete(&Context::background(), &request.into_inner().name)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteResponse {}))
    }

    async fn add_scores(
        &self,
        request: Request<pb::AddScoresRequest>,
    ) -> Result<Response<pb::AddScoresResponse>, Status> {
        let request = request.into_inner();
        let scores: Vec<Score> = request.documents.into_iter().map(score_from_pb).collect();
        self.database
            .add_scores(
                &Context::background(),
                &request.collection,
                &request.subset,
                &scores,
            )
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::AddScoresResponse {}))
    }

    async fn search_scores(
        &self,
        request: Request<pb::SearchScoresRequest>,
    ) -> Result<Response<pb::SearchScoresResponse>, Status> {
        let request = request.into_inner();
        let scores = self
            .database
            .search_scores(
                &Context::background(),
                &request.collection,
                &request.subset,
                &request.query,
                i64::from(request.begin),
                i64::from(request.end),
            )
            .await
            .map_err(unknown)?;
        let documents = scores.iter().map(score_to_pb).collect();
        Ok(Response::new(pb::SearchScoresResponse { documents }))
    }

    async fn delete_scores(
        &self,
        request: Request<pb::DeleteScoresRequest>,
    ) -> Result<Response<pb::DeleteScoresResponse>, Status> {
        let request = request.into_inner();
        // Go reads `Condition.Before` without a nil check and panics on a
        // request without a condition. Here the missing condition is empty,
        // and the backend refuses it.
        let condition = request.condition.unwrap_or_default();
        let condition = ScoreCondition {
            subset: condition.subset,
            id: condition.id,
            before: condition.before.as_ref().map(|t| time_from_pb(Some(t))),
        };
        self.database
            .delete_scores(&Context::background(), &request.collection, &condition)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::DeleteScoresResponse {}))
    }

    async fn update_scores(
        &self,
        request: Request<pb::UpdateScoresRequest>,
    ) -> Result<Response<pb::UpdateScoresResponse>, Status> {
        let request = request.into_inner();
        let patch = request.patch.unwrap_or_default();
        let patch = ScorePatch {
            is_hidden: patch.is_hidden,
            categories: optional_strings(patch.categories),
            score: patch.score,
        };
        self.database
            .update_scores(
                &Context::background(),
                &request.collection,
                request.subset.as_deref(),
                &request.id,
                &patch,
            )
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::UpdateScoresResponse {}))
    }

    async fn scan_scores(
        &self,
        _request: Request<pb::ScanScoresRequest>,
    ) -> Result<Response<Self::ScanScoresStream>, Status> {
        // The backend's scan callback is synchronous, so the documents go into
        // an unbounded channel. A slow reader therefore buffers the whole scan
        // here, where Go's sender waits for gRPC flow control (`PORTING_NOTES.md`).
        let database = Arc::clone(&self.database);
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let ctx = Context::background();
            let result = database
                .scan_scores(
                    &ctx,
                    &mut |collection: &str, id: &str, subset: &str, timestamp: Time| {
                        tx.send(Ok(pb::ScanScoresResponse {
                            collection: collection.to_owned(),
                            id: id.to_owned(),
                            subset: subset.to_owned(),
                            timestamp: Some(time_to_pb(&timestamp)),
                        }))
                        .map_err(|_| {
                            Error::Backend(Arc::new(std::io::Error::other("client disconnected")))
                        })
                    },
                )
                .await;
            if let Err(err) = result {
                let _ = tx.send(Err(Status::internal(format!(
                    "failed to scan scores: {err}"
                ))));
            }
        });
        Ok(Response::new(Box::pin(UnboundedReceiverStream::new(rx))))
    }

    async fn add_time_series_points(
        &self,
        request: Request<pb::AddTimeSeriesPointsRequest>,
    ) -> Result<Response<pb::AddTimeSeriesPointsResponse>, Status> {
        let points: Vec<TimeSeriesPoint> = request
            .into_inner()
            .points
            .into_iter()
            .map(time_series_point_from_pb)
            .collect();
        self.database
            .add_time_series_points(&Context::background(), &points)
            .await
            .map_err(unknown)?;
        Ok(Response::new(pb::AddTimeSeriesPointsResponse {}))
    }

    async fn get_time_series_points(
        &self,
        request: Request<pb::GetTimeSeriesPointsRequest>,
    ) -> Result<Response<pb::GetTimeSeriesPointsResponse>, Status> {
        let request = request.into_inner();
        let points = self
            .database
            .get_time_series_points(
                &Context::background(),
                &request.name,
                time_from_pb(request.begin.as_ref()),
                time_from_pb(request.end.as_ref()),
                Duration(request.duration),
            )
            .await
            .map_err(unknown)?;
        let points = points.iter().map(time_series_point_to_pb).collect();
        Ok(Response::new(pb::GetTimeSeriesPointsResponse { points }))
    }
}

/// Go's `ProxyClient`: a [`Database`] that calls a remote [`ProxyServer`].
///
/// `Init`, `Scan` and `Purge` are refused, as in Go. `Close` does nothing.
pub struct ProxyClient {
    client: CacheStoreClient<Channel>,
    /// The backend that the suite's `SetupTest` and `TearDownTest` reach
    /// directly, as Go's proxy suite does. Only the suite's tests set it.
    #[cfg(all(test, feature = "sqlite"))]
    suite_backend: Option<Arc<dyn Database>>,
}

impl ProxyClient {
    /// Go's `NewProxyClient`.
    pub fn new(channel: Channel) -> Self {
        Self {
            client: CacheStoreClient::new(channel),
            #[cfg(all(test, feature = "sqlite"))]
            suite_backend: None,
        }
    }

    /// A client for the proxy suite: `ping` and `purge` reach `backend` directly,
    /// as Go's `ProxyTestSuite.SetupTest` does (see the data store's client).
    #[cfg(all(test, feature = "sqlite"))]
    pub(crate) fn for_suite(channel: Channel, backend: Arc<dyn Database>) -> Self {
        Self {
            client: CacheStoreClient::new(channel),
            suite_backend: Some(backend),
        }
    }

    /// A handle to the gRPC client for one call (see the data store's client).
    fn grpc(&self) -> CacheStoreClient<Channel> {
        self.client.clone()
    }
}

#[async_trait]
impl Database for ProxyClient {
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        #[cfg(all(test, feature = "sqlite"))]
        if let Some(backend) = &self.suite_backend {
            return backend.ping().await;
        }
        self.grpc()
            .ping(pb::PingRequest {})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn init(&self) -> Result<(), Error> {
        Err(refused("init is not allowed in proxy client"))
    }

    async fn scan(
        &self,
        _work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error> {
        Err(refused("scan is not allowed in proxy client"))
    }

    async fn purge(&self) -> Result<(), Error> {
        #[cfg(all(test, feature = "sqlite"))]
        if let Some(backend) = &self.suite_backend {
            return backend.purge().await;
        }
        Err(refused("purge is not allowed in proxy client"))
    }

    async fn set(&self, _ctx: &Context, values: &[super::Value]) -> Result<(), Error> {
        let values = values
            .iter()
            .map(|value| pb::Value {
                name: value.name().to_owned(),
                value: value.value().to_owned(),
            })
            .collect();
        self.grpc()
            .set(pb::SetRequest { values })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get(&self, _ctx: &Context, name: &str) -> ReturnValue {
        match self
            .grpc()
            .get(pb::GetRequest {
                name: name.to_owned(),
            })
            .await
        {
            Err(status) => new_return_error(backend(status)),
            Ok(response) => match response.into_inner().value {
                None => new_return_value("", false),
                Some(value) => new_return_value(value, true),
            },
        }
    }

    async fn delete(&self, _ctx: &Context, name: &str) -> Result<(), Error> {
        self.grpc()
            .delete(pb::DeleteRequest {
                name: name.to_owned(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn add_scores(
        &self,
        _ctx: &Context,
        collection: &str,
        subset: &str,
        documents: &[Score],
    ) -> Result<(), Error> {
        self.grpc()
            .add_scores(pb::AddScoresRequest {
                collection: collection.to_owned(),
                subset: subset.to_owned(),
                documents: documents.iter().map(score_to_pb).collect(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn search_scores(
        &self,
        _ctx: &Context,
        collection: &str,
        subset: &str,
        query: &[String],
        begin: i64,
        end: i64,
    ) -> Result<Vec<Score>, Error> {
        let response = self
            .grpc()
            .search_scores(pb::SearchScoresRequest {
                collection: collection.to_owned(),
                subset: subset.to_owned(),
                query: query.to_vec(),
                begin: begin as i32,
                end: end as i32,
            })
            .await
            .map_err(backend)?
            .into_inner();
        Ok(response.documents.into_iter().map(score_from_pb).collect())
    }

    async fn delete_scores(
        &self,
        _ctx: &Context,
        collection: &[String],
        condition: &ScoreCondition,
    ) -> Result<(), Error> {
        // Go checks the condition before it sends anything.
        condition.check()?;
        self.grpc()
            .delete_scores(pb::DeleteScoresRequest {
                collection: collection.to_vec(),
                condition: Some(pb::ScoreCondition {
                    subset: condition.subset.clone(),
                    id: condition.id.clone(),
                    before: condition.before.as_ref().map(time_to_pb),
                }),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn update_scores(
        &self,
        _ctx: &Context,
        collections: &[String],
        subset: Option<&str>,
        id: &str,
        patch: &ScorePatch,
    ) -> Result<(), Error> {
        self.grpc()
            .update_scores(pb::UpdateScoresRequest {
                collection: collections.to_vec(),
                subset: subset.map(str::to_owned),
                id: id.to_owned(),
                patch: Some(pb::ScorePatch {
                    is_hidden: patch.is_hidden,
                    categories: patch.categories.clone().unwrap_or_default(),
                    score: patch.score,
                }),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn scan_scores(
        &self,
        ctx: &Context,
        callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error> {
        let mut stream = self
            .grpc()
            .scan_scores(pb::ScanScoresRequest {})
            .await
            .map_err(backend)?
            .into_inner();
        loop {
            // Go checks the context before each message.
            if let Some(err) = ctx.err() {
                return Err(err.into());
            }
            let Some(response) = stream.message().await.map_err(backend)? else {
                return Ok(());
            };
            callback(
                &response.collection,
                &response.id,
                &response.subset,
                time_from_pb(response.timestamp.as_ref()),
            )?;
        }
    }

    async fn add_time_series_points(
        &self,
        _ctx: &Context,
        points: &[TimeSeriesPoint],
    ) -> Result<(), Error> {
        self.grpc()
            .add_time_series_points(pb::AddTimeSeriesPointsRequest {
                points: points.iter().map(time_series_point_to_pb).collect(),
            })
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_time_series_points(
        &self,
        _ctx: &Context,
        name: &str,
        begin: Time,
        end: Time,
        duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error> {
        let response = self
            .grpc()
            .get_time_series_points(pb::GetTimeSeriesPointsRequest {
                name: name.to_owned(),
                begin: Some(time_to_pb(&begin)),
                end: Some(time_to_pb(&end)),
                duration: duration.0,
            })
            .await
            .map_err(backend)?
            .into_inner();
        Ok(response
            .points
            .into_iter()
            .map(time_series_point_from_pb)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tonic::transport::{Channel, Server};

    use super::*;
    use crate::cache::no_database::NoDatabase;
    use crate::data::proxy::support::connect;

    /// A client of `database` served through the proxy, with every call on the wire.
    async fn wire_client_of(database: Arc<dyn Database>) -> ProxyClient {
        let router = Server::builder().add_service(ProxyServer::new(database).into_service());
        ProxyClient::new(connect(router).await)
    }

    /// A client of `database` served through the proxy, as the proxy suite has it.
    #[cfg(all(test, feature = "sqlite"))]
    async fn client_of(database: Arc<dyn Database>) -> ProxyClient {
        let router =
            Server::builder().add_service(ProxyServer::new(database.clone()).into_service());
        ProxyClient::for_suite(connect(router).await, database)
    }

    /// Go's proxy client refuses `Init`, `Scan` and `Purge` without a call.
    #[tokio::test]
    async fn refuses_init_scan_and_purge() {
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let client = ProxyClient::new(channel);
        let err = client.init().await.unwrap_err();
        assert_eq!(err.to_string(), "init is not allowed in proxy client");
        let err = client.purge().await.unwrap_err();
        assert_eq!(err.to_string(), "purge is not allowed in proxy client");
        let mut keys = |_: &str| -> Result<(), Error> { Ok(()) };
        let err = client.scan(&mut keys).await.unwrap_err();
        assert_eq!(err.to_string(), "scan is not allowed in proxy client");
    }

    /// A store with no database behind it errors through the proxy with the
    /// text Go's client shows.
    #[tokio::test]
    async fn no_database_errors_cross_the_wire() {
        let client = wire_client_of(Arc::new(NoDatabase)).await;
        let ctx = Context::background();
        let text = "rpc error: code = Unknown desc = database not assigned";
        assert_eq!(client.ping().await.unwrap_err().to_string(), text);
        let err = client.set(&ctx, &[string("a", "b")]).await.unwrap_err();
        assert_eq!(err.to_string(), text);
    }

    /// Go's proxy server answers a missing key with no value and no error, even
    /// when the backend reported an error with it, so the client sees a
    /// missing key.
    #[tokio::test]
    async fn a_failing_get_reads_as_a_missing_key_through_the_proxy() {
        let client = wire_client_of(Arc::new(NoDatabase)).await;
        let value = client.get(&Context::background(), "key").await;
        assert!(!value.exists());
        assert_eq!(value.string().unwrap(), "");
    }

    /// The suite of Go's proxy test, on SQLite, through the proxy.
    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn suite_through_the_proxy_on_sqlite() {
        use crate::cache::database_test::run_through_proxy;
        use crate::cache::{Registry, register_builtin};

        let dir = tempfile::tempdir().expect("temp dir");
        let mut registry = Registry::default();
        register_builtin(&mut registry);
        let path = format!("sqlite://{}/sqlite.db", dir.path().display());
        let backend: Arc<dyn Database> =
            Arc::from(registry.open(&path, "gorse_", &[]).await.expect("open"));
        backend.init().await.expect("init");
        let client = client_of(backend).await;
        run_through_proxy(&client).await;
    }
}
