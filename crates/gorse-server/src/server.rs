// Ports: server/server.go @ add21b5

//! The server node's life cycle (`server/server.go`): it connects to the
//! master, keeps its configuration and stores in step with the master's meta,
//! and serves the REST API until it is shut down.

use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use gorse_common::util::tls::TlsConfig;
use gorse_config::config::defaults::get_default_config;
use gorse_config::config::load::unmarshal;
use gorse_config::formats::json;
use gorse_protocol::{Meta, NodeInfo, NodeType, master_client::MasterClient};
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::vectors;
use tokio::sync::watch;
use tonic::transport::Channel;

use crate::rest::{self, RestServer};

/// Compile-time check (Q-254): the server is shared between the HTTP task and
/// the sync task, so it must be `Send` and `Sync`.
const fn assert_send_sync<T: Send + Sync>() {}
const _: () = assert_send_sync::<Server>();

/// The OpenAPI document the server serves at `/apidocs.json`. It is the frozen
/// document the Go server serves, not one generated here (`PORTING.md`).
const OPENAPI_DOCUMENT: &str = include_str!("../../../contracts/rest/server.openapi.json");

/// The stores a server node has open, and the paths and table prefixes they
/// were opened with, so that `sync` reopens a store only when its settings
/// change, as Go's `Sync` does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OpenedPaths {
    pub(crate) data_path: String,
    pub(crate) data_prefix: String,
    pub(crate) cache_path: String,
    pub(crate) cache_prefix: String,
    pub(crate) vector_path: String,
    pub(crate) vector_prefix: String,
}

/// A server node: the REST API plus the link to the master.
pub struct Server {
    rest: Arc<RestServer>,
    master_host: String,
    master_port: u16,
    http_host: String,
    http_port: u16,
    tls: Option<TlsConfig>,
    server_name: String,
    data_registry: data::Registry,
    cache_registry: cache::Registry,
    shutdown: watch::Sender<bool>,
}

/// Go's `NewServer`: a node with the default configuration and no stores. The
/// stores come with the first meta the master sends.
///
/// # Errors
///
/// Fails when the host name cannot be read, which Go's `ServerName` reports.
pub fn new_server(
    master_host: &str,
    master_port: u16,
    server_host: &str,
    server_port: u16,
    tls: Option<TlsConfig>,
) -> Result<Server, ServerError> {
    let server_name = server_name(server_host, server_port)?;
    let mut data_registry = data::Registry::default();
    data::register_builtin(&mut data_registry);
    let mut cache_registry = cache::Registry::default();
    cache::register_builtin(&mut cache_registry);
    let (shutdown, _) = watch::channel(false);
    let rest = RestServer::new(
        get_default_config(),
        Arc::new(cache::no_database::NoDatabase),
        Arc::new(data::no_database::NoDatabase),
        Arc::new(vectors::NoDatabase),
    );
    Ok(Server {
        rest: Arc::new(rest),
        master_host: master_host.to_owned(),
        master_port,
        http_host: server_host.to_owned(),
        http_port: server_port,
        tls,
        server_name,
        data_registry,
        cache_registry,
        shutdown,
    })
}

/// Errors of the server's life cycle.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Go's `ServerName` failed, which it does when the host name is unknown.
    #[error("failed to get server name: {0}")]
    ServerName(String),
    /// The link to the master could not be made.
    #[error("failed to connect master: {0}")]
    Master(String),
    /// The HTTP listener could not be bound or served.
    #[error("failed to start http server: {0}")]
    Http(String),
}

/// Go's `ServerName`: the MD5 of the host name, the HTTP host and the HTTP
/// port, in hex. A cluster uses it to tell the servers apart.
///
/// # Errors
///
/// Fails when the host name cannot be read.
pub fn server_name(http_host: &str, http_port: u16) -> Result<String, ServerError> {
    use md5::{Digest, Md5};
    let hostname = read_hostname()?;
    let mut hash = Md5::new();
    hash.update(hostname.as_bytes());
    hash.update(http_host.as_bytes());
    hash.update(http_port.to_string().as_bytes());
    Ok(hex::encode(hash.finalize()))
}

/// The host name, as Go's `os.Hostname` gives it on Linux: the kernel's name.
fn read_hostname() -> Result<String, ServerError> {
    match std::fs::read_to_string("/proc/sys/kernel/hostname") {
        Ok(name) => Ok(name.trim_end().to_owned()),
        Err(err) => std::env::var("HOSTNAME").map_err(|_| ServerError::ServerName(err.to_string())),
    }
}

impl Server {
    /// The REST state, shared with the router.
    pub fn rest(&self) -> Arc<RestServer> {
        self.rest.clone()
    }

    /// The name this node reports to the master.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Go's `Serve`: connects to the master, starts the meta sync, and serves
    /// the REST API until [`Server::shutdown`] is called.
    ///
    /// # Errors
    ///
    /// Fails when the master link cannot be made or the HTTP listener cannot
    /// be bound.
    pub async fn serve(&self) -> Result<(), ServerError> {
        tracing::info!(
            server_name = %self.server_name,
            server_host = %self.http_host,
            server_port = self.http_port,
            master_host = %self.master_host,
            master_port = self.master_port,
            "start server"
        );
        let channel = self.connect_master().await?;
        let listener =
            tokio::net::TcpListener::bind(format!("{}:{}", self.http_host, self.http_port))
                .await
                .map_err(|err| ServerError::Http(err.to_string()))?;
        let mut stop = self.shutdown.subscribe();
        let served = axum::serve(listener, self.app())
            .with_graceful_shutdown(async move {
                let _ = stop.wait_for(|stopped| *stopped).await;
            })
            .into_future();
        let (served, ()) = tokio::join!(served, self.sync_loop(channel));
        served.map_err(|err| ServerError::Http(err.to_string()))
    }

    /// Go's `Shutdown`: stops the HTTP server and the meta sync.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// The application router: the REST API, the OpenAPI document and the
    /// metrics, on one listener as in Go's `StartHttpServer`.
    pub fn app(&self) -> Router {
        rest::router(self.rest.clone())
            .route("/apidocs.json", get(openapi_document))
            .route("/metrics", get(metrics))
    }

    async fn connect_master(&self) -> Result<Channel, ServerError> {
        if self.tls.is_some() {
            return Err(ServerError::Master(
                "TLS to the master is not ported yet (WP-4-3 with gorse_common::util::tls)"
                    .to_owned(),
            ));
        }
        let uri = format!("http://{}:{}", self.master_host, self.master_port);
        let endpoint = tonic::transport::Endpoint::from_shared(uri)
            .map_err(|err| ServerError::Master(err.to_string()))?;
        Ok(endpoint.connect_lazy())
    }

    /// Go's `Sync`: asks the master for its meta every `master.meta_timeout`,
    /// until the server shuts down.
    async fn sync_loop(&self, channel: Channel) {
        let mut client = MasterClient::new(channel.clone());
        let mut opened = OpenedPaths::default();
        let mut stop = self.shutdown.subscribe();
        tracing::info!(
            meta_timeout = ?self.rest.config().master.meta_timeout,
            "start meta sync"
        );
        loop {
            self.sync_once(&mut client, &channel, &mut opened).await;
            let timeout = StdDuration::from_nanos(
                u64::try_from(self.rest.config().master.meta_timeout.nanoseconds()).unwrap_or(0),
            );
            tokio::select! {
                () = tokio::time::sleep(timeout) => {}
                _ = stop.wait_for(|stopped| *stopped) => return,
            }
        }
    }

    /// One pass of Go's `Sync`: fetches the meta, applies its configuration and
    /// reopens whichever stores changed. A failure is logged, and the next
    /// pass tries again.
    pub(crate) async fn sync_once(
        &self,
        client: &mut MasterClient<Channel>,
        channel: &Channel,
        opened: &mut OpenedPaths,
    ) {
        let info = NodeInfo {
            node_type: NodeType::Server as i32,
            uuid: self.server_name.clone(),
            binary_version: env!("CARGO_PKG_VERSION").to_owned(),
            hostname: read_hostname().unwrap_or_default(),
        };
        let meta: Meta = match client.get_meta(info).await {
            Ok(response) => response.into_inner(),
            Err(err) => {
                tracing::error!(error = %err, "failed to get meta");
                return;
            }
        };
        let config =
            match json::decode(meta.config.as_bytes()).and_then(|settings| unmarshal(&settings)) {
                Ok(config) => config,
                Err(err) => {
                    tracing::error!(error = %err, "failed to parse master config");
                    return;
                }
            };
        self.rest.set_config(config.clone());
        let database = &config.database;
        if opened.data_path != database.data_store
            || opened.data_prefix != database.data_table_prefix
        {
            if database
                .data_store
                .starts_with(gorse_storage_core::scheme::SQLITE_PREFIX)
            {
                tracing::info!("connect cache store via master");
                self.rest
                    .set_data(Arc::new(data::proxy::ProxyClient::new(channel.clone())));
            } else {
                tracing::info!("connect data store");
                let opts = database.storage_options(&database.data_store);
                match self
                    .data_registry
                    .open(&database.data_store, &database.data_table_prefix, &opts)
                    .await
                {
                    Ok(store) => self.rest.set_data(Arc::from(store)),
                    Err(err) => {
                        tracing::error!(error = %err, "failed to connect data store");
                        return;
                    }
                }
            }
            opened.data_path = database.data_store.clone();
            opened.data_prefix = database.data_table_prefix.clone();
        }
        if opened.cache_path != database.cache_store
            || opened.cache_prefix != database.cache_table_prefix
        {
            if database
                .cache_store
                .starts_with(gorse_storage_core::scheme::SQLITE_PREFIX)
            {
                tracing::info!("connect cache store via master");
                self.rest
                    .set_cache(Arc::new(cache::proxy::ProxyClient::new(channel.clone())));
            } else {
                tracing::info!("connect cache store");
                let opts = database.storage_options(&database.cache_store);
                match self
                    .cache_registry
                    .open(&database.cache_store, &database.cache_table_prefix, &opts)
                    .await
                {
                    Ok(store) => self.rest.set_cache(Arc::from(store)),
                    Err(err) => {
                        tracing::error!(error = %err, "failed to connect cache store");
                        return;
                    }
                }
            }
            opened.cache_path = database.cache_store.clone();
            opened.cache_prefix = database.cache_table_prefix.clone();
        }
        if opened.vector_path != database.vector_store
            || opened.vector_prefix != database.vector_table_prefix
        {
            if database
                .vector_store
                .starts_with(gorse_storage_core::scheme::XVEC_PREFIX)
            {
                tracing::info!("connect vector store via master");
                self.rest
                    .set_vectors(Arc::new(vectors::proxy::ProxyClient::new(channel.clone())));
            } else {
                match vectors::open(&database.vector_store, &database.vector_table_prefix, &[])
                    .await
                {
                    Ok(store) => self.rest.set_vectors(Arc::from(store)),
                    Err(err) => {
                        tracing::error!(error = %err, "failed to connect vector store");
                        return;
                    }
                }
            }
            opened.vector_path = database.vector_store.clone();
            opened.vector_prefix = database.vector_table_prefix.clone();
        }
    }
}

/// Go's `/apidocs.json`: the OpenAPI document.
async fn openapi_document() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        OPENAPI_DOCUMENT,
    )
}

/// Go's `/metrics`: the Prometheus exposition of the default registry.
async fn metrics() -> impl IntoResponse {
    use prometheus::Encoder;
    let mut buffer = Vec::new();
    let families = prometheus::gather();
    let encoder = prometheus::TextEncoder::new();
    let _ = encoder.encode(&families, &mut buffer);
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_owned())],
        buffer,
    )
}

#[cfg(test)]
mod tests;
