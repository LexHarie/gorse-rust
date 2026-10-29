// Ports: worker/worker.go @ add21b5

//! The worker node: the sync with the master, the pull of the models and the
//! user index, the recommendation loop, and the HTTP routes for metrics and
//! health. Go's `worker.go`.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use gorse_common::consistent::Consistent;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Duration as GoDuration;
use gorse_common::monitor;
use gorse_common::util::tls::TlsConfig;
use gorse_config::config::get_default_config;
use gorse_logics::cf::MatrixFactorizationUsers;
use gorse_model::ctr::model::unmarshal_model;
use gorse_protocol::master_client::MasterClient;
use gorse_protocol::{NodeInfo, NodeType};
use gorse_storage::blob;
use gorse_storage::cache;
use gorse_storage::data::types::User;
use gorse_storage::data::{self};
use gorse_storage::vectors;
use hyper_rustls::HttpsConnectorBuilder;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};
use tonic::codegen::Service;
use tonic::transport::{Channel, Endpoint, Uri};
use tracing::{error, info};

use crate::Error;
use crate::metrics;
use crate::pipeline::{Pipeline, ProgressFn};

/// The number of users read from the data store at a time (Go's `batchSize`).
const BATCH_SIZE: i64 = 10000;

/// The largest message the master may send (Go's `MaxCallRecvMsgSize`).
const MAX_RECV_MESSAGE_SIZE: usize = 512 * 1024 * 1024;

/// The prefix of a SQLite store, which the master serves (Go's `storage.SQLitePrefix`).
const SQLITE_PREFIX: &str = "sqlite://";

/// The prefix of an embedded vector store, which the master serves (Go's `storage.XvecPrefix`).
const XVEC_PREFIX: &str = "xvec://";

/// The version a worker reports to the master. Go reports its build's version;
/// this reports the crate's.
const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How a worker is started: where the master is, where this worker serves
/// HTTP, and how the recommendation runs.
#[derive(Debug, Clone)]
pub struct WorkerOptions {
    /// The master's gRPC host.
    pub master_host: String,
    /// The master's gRPC port.
    pub master_port: u16,
    /// The host this worker serves metrics and health on.
    pub http_host: String,
    /// The port this worker serves metrics and health on.
    pub http_port: u16,
    /// The number of users a recommendation run ranks at a time.
    pub jobs: usize,
    /// The TLS files of the master connection. Go accepts these; this port does
    /// not yet, so [`Worker::serve`] refuses them rather than connect without TLS.
    pub tls: Option<TlsConfig>,
    /// How often a recommendation run starts without a model update.
    pub tick_interval: Duration,
}

/// What the sync and the pull know about the master and the stores they have
/// connected. It is behind a lock, and no lock is held across an await.
#[derive(Default)]
struct SyncState {
    conn: Option<Channel>,
    master: Option<MasterClient<Channel>>,
    data_path: String,
    data_prefix: String,
    cache_path: String,
    cache_prefix: String,
    blob_config: String,
    blob_store: Option<Arc<dyn blob::Store>>,
    vector_path: String,
    vector_prefix: String,
    latest_collaborative_filtering_model_id: i64,
    latest_click_through_rate_model_id: i64,
    click_through_rate_model_id: i64,
    peers: Vec<String>,
    me: String,
}

/// Go's `Worker`: one worker node. It owns the pipeline and the channels that
/// connect the sync, the pull and the recommendation loop.
pub struct Worker {
    options: WorkerOptions,
    pipeline: Arc<Pipeline>,
    worker_name: String,
    state: Mutex<SyncState>,
    synced_tx: mpsc::Sender<()>,
    synced_rx: Mutex<Option<mpsc::Receiver<()>>>,
    pulled_tx: mpsc::Sender<()>,
    pulled_rx: Mutex<Option<mpsc::Receiver<()>>>,
    shutdown: watch::Sender<bool>,
}

impl Worker {
    /// Go's `NewWorker`: a worker with no stores, which the first sync
    /// connects. Its name is the MD5 of its host name and its HTTP address.
    ///
    /// # Errors
    ///
    /// Fails when the host name cannot be read.
    pub fn new(options: WorkerOptions) -> Result<Self, Error> {
        let worker_name = worker_name(&hostname()?, &options.http_host, options.http_port);
        metrics::register();
        let pipeline = Arc::new(Pipeline::new(
            get_default_config(),
            Arc::new(cache::no_database::NoDatabase),
            Arc::new(data::no_database::NoDatabase),
            Arc::new(vectors::NoDatabase),
            monitor::new_tracer(&worker_name),
            options.jobs,
        ));
        let (synced_tx, synced_rx) = mpsc::channel(1);
        let (pulled_tx, pulled_rx) = mpsc::channel(1);
        Ok(Self {
            options,
            pipeline,
            worker_name,
            state: Mutex::new(SyncState::default()),
            synced_tx,
            synced_rx: Mutex::new(Some(synced_rx)),
            pulled_tx,
            pulled_rx: Mutex::new(Some(pulled_rx)),
            shutdown: watch::channel(false).0,
        })
    }

    /// The pipeline the worker ranks with.
    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    /// The worker's name, which the master knows it by.
    pub fn worker_name(&self) -> &str {
        &self.worker_name
    }

    /// Go's `Sync`: keeps the worker's configuration, stores and model ids in
    /// step with the master, once per meta timeout, until shut down.
    pub async fn sync(&self) {
        let mut shutdown = self.shutdown.subscribe();
        info!("start meta sync");
        loop {
            self.sync_once().await;
            let meta_timeout = std_duration(self.pipeline.config().master.meta_timeout);
            tokio::select! {
                _ = tokio::time::sleep(meta_timeout) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            }
        }
    }

    /// One pass of Go's `Sync` loop: fetches the master's meta, and connects
    /// the stores and notes the models it names. A failed step is logged, and
    /// the rest of the pass is skipped.
    pub async fn sync_once(&self) {
        if let Err(err) = self.try_sync().await {
            error!(error = %err, "failed to sync with the master");
        }
    }

    /// The body of [`Worker::sync_once`], returning the first failure.
    async fn try_sync(&self) -> Result<(), Error> {
        let (master, conn) = {
            let state = self.state();
            (state.master.clone(), state.conn.clone())
        };
        let mut master = master.ok_or(Error::NotConnected)?;
        let meta = master
            .get_meta(NodeInfo {
                node_type: NodeType::Worker as i32,
                uuid: self.worker_name.clone(),
                binary_version: BINARY_VERSION.to_owned(),
                hostname: hostname()?,
            })
            .await?
            .into_inner();

        // Load the master's configuration over the worker's.
        let mut config = (*self.pipeline.config()).clone();
        config
            .unmarshal_json(&meta.config)
            .map_err(|err| Error::Config(err.to_string()))?;
        self.pipeline.set_config(config.clone());
        let database = &config.database;

        // Connect to the data store.
        let (data_path, data_prefix) = {
            let state = self.state();
            (state.data_path.clone(), state.data_prefix.clone())
        };
        if data_path != database.data_store || data_prefix != database.data_table_prefix {
            let data: Arc<dyn data::Database> = if database.data_store.starts_with(SQLITE_PREFIX) {
                info!("connect data store via master");
                Arc::new(data::proxy::ProxyClient::new(
                    conn.clone().ok_or(Error::NotConnected)?,
                ))
            } else {
                info!("connect data store");
                let mut registry = data::Registry::default();
                data::register_builtin(&mut registry);
                let options = database.storage_options(&database.data_store);
                Arc::from(
                    registry
                        .open(&database.data_store, &database.data_table_prefix, &options)
                        .await?,
                )
            };
            self.pipeline.set_data(data);
            let mut state = self.state();
            state.data_path = database.data_store.clone();
            state.data_prefix = database.data_table_prefix.clone();
        }

        // Connect to the cache store.
        let (cache_path, cache_prefix) = {
            let state = self.state();
            (state.cache_path.clone(), state.cache_prefix.clone())
        };
        if cache_path != database.cache_store || cache_prefix != database.cache_table_prefix {
            let cache: Arc<dyn cache::Database> = if database.cache_store.starts_with(SQLITE_PREFIX)
            {
                info!("connect cache store via master");
                Arc::new(cache::proxy::ProxyClient::new(
                    conn.clone().ok_or(Error::NotConnected)?,
                ))
            } else {
                info!("connect cache store");
                let mut registry = cache::Registry::default();
                cache::register_builtin(&mut registry);
                let options = database.storage_options(&database.cache_store);
                Arc::from(
                    registry
                        .open(
                            &database.cache_store,
                            &database.cache_table_prefix,
                            &options,
                        )
                        .await?,
                )
            };
            self.pipeline.set_cache(cache);
            let mut state = self.state();
            state.cache_path = database.cache_store.clone();
            state.cache_prefix = database.cache_table_prefix.clone();
        }

        // Connect to the blob store.
        let blob_config = self.state().blob_config.clone();
        if blob_config != config.blob.uri {
            let store: Arc<dyn blob::Store> =
                Arc::from(blob::new_store(&config.blob, conn.clone())?);
            let mut state = self.state();
            state.blob_store = Some(store);
            state.blob_config = config.blob.uri.clone();
        }

        // Connect to the vector store.
        let (vector_path, vector_prefix) = {
            let state = self.state();
            (state.vector_path.clone(), state.vector_prefix.clone())
        };
        if vector_path != database.vector_store || vector_prefix != database.vector_table_prefix {
            let vectors: Arc<dyn vectors::Database> =
                if database.vector_store.starts_with(XVEC_PREFIX) {
                    info!("connect vector store via master");
                    Arc::new(vectors::proxy::ProxyClient::new(
                        conn.clone().ok_or(Error::NotConnected)?,
                    ))
                } else {
                    Arc::from(
                        vectors::open(&database.vector_store, &database.vector_table_prefix, &[])
                            .await?,
                    )
                };
            self.pipeline.set_vectors(vectors);
            let mut state = self.state();
            state.vector_path = database.vector_store.clone();
            state.vector_prefix = database.vector_table_prefix.clone();
        }

        // Note the models the master names, and tell the pull when one is newer.
        let collaborative_filtering_model_id = meta.collaborative_filtering_model_id;
        let collaborative_filtering_is_newer =
            collaborative_filtering_model_id > self.pipeline.get_matrix_factorization_id();
        if collaborative_filtering_is_newer {
            info!(
                old_version = self.pipeline.get_matrix_factorization_id(),
                new_version = collaborative_filtering_model_id,
                "new ranking model found"
            );
            let _ = self.synced_tx.try_send(());
        }
        let click_through_rate_model_id = meta.click_through_rate_model_id;
        let click_through_rate_is_newer = {
            let mut state = self.state();
            state.latest_collaborative_filtering_model_id = collaborative_filtering_model_id;
            state.latest_click_through_rate_model_id = click_through_rate_model_id;
            click_through_rate_model_id > state.click_through_rate_model_id
        };
        if click_through_rate_is_newer {
            info!(
                old_version = self.state().click_through_rate_model_id,
                new_version = click_through_rate_model_id,
                "new click model found"
            );
            let _ = self.synced_tx.try_send(());
        }

        let mut state = self.state();
        state.peers = meta.workers;
        state.me = meta.me;
        Ok(())
    }

    /// Go's `Pull`: installs the models the sync named, each time the sync
    /// signals one. It stops when the sync channel closes.
    pub async fn pull(&self, mut synced_rx: mpsc::Receiver<()>) {
        while synced_rx.recv().await.is_some() {
            self.pull_once().await;
        }
    }

    /// One pass of Go's `Pull`: installs the collaborative filtering model and
    /// the click-through rate model when each is newer than the one installed.
    async fn pull_once(&self) {
        let mut pulled = false;
        let (collaborative_filtering_id, click_through_rate_id, installed_click_through_rate_id) = {
            let state = self.state();
            (
                state.latest_collaborative_filtering_model_id,
                state.latest_click_through_rate_model_id,
                state.click_through_rate_model_id,
            )
        };

        if collaborative_filtering_id > self.pipeline.get_matrix_factorization_id() {
            info!("start pull collaborative filtering model");
            match self
                .pull_collaborative_filtering(collaborative_filtering_id)
                .await
            {
                Ok(()) => {
                    info!(
                        id = self.pipeline.get_matrix_factorization_id(),
                        "synced collaborative filtering model"
                    );
                    pulled = true;
                }
                Err(err) => error!(error = %err, "failed to install collaborative filtering model"),
            }
        }

        if click_through_rate_id > installed_click_through_rate_id {
            info!("start pull click model");
            match self.pull_click_through_rate(click_through_rate_id).await {
                Ok(()) => {
                    info!(
                        version = click_through_rate_id,
                        "synced click-through rate model"
                    );
                    pulled = true;
                }
                Err(err) => error!(error = %err, "failed to install click-through rate model"),
            }
        }

        if pulled {
            let _ = self.pulled_tx.try_send(());
        }
    }

    /// Reads and installs the collaborative filtering model with the id.
    async fn pull_collaborative_filtering(&self, id: i64) -> Result<(), Error> {
        let bytes = self.read_blob(id).await?;
        let mut users = MatrixFactorizationUsers::new();
        users.unmarshal(&mut bytes.as_slice())?;
        self.pipeline
            .update_matrix_factorization(&Context::background(), id, users)
            .await
    }

    /// Reads and installs the click-through rate model with the id.
    async fn pull_click_through_rate(&self, id: i64) -> Result<(), Error> {
        let bytes = self.read_blob(id).await?;
        let model = unmarshal_model(&mut bytes.as_slice())?;
        self.pipeline
            .set_click_through_rate_model(Some(Arc::from(model)));
        self.state().click_through_rate_model_id = id;
        Ok(())
    }

    /// Reads the blob named by a model id from the blob store.
    async fn read_blob(&self, id: i64) -> Result<Vec<u8>, Error> {
        let store = self.state().blob_store.clone().ok_or(Error::NotConnected)?;
        let mut reader = store.open(&Context::background(), &id.to_string()).await?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(bytes)
    }

    /// Go's `Serve`: runs the worker until it is shut down. It connects to the
    /// master, starts the sync, the pull and the HTTP server, and runs a
    /// recommendation each time the tick comes or a model is pulled.
    ///
    /// # Errors
    ///
    /// Fails when the TLS files cannot be loaded, when the master address is
    /// invalid, or when the HTTP server cannot run.
    pub async fn serve(self: Arc<Self>) -> Result<(), Error> {
        let (Some(synced_rx), Some(pulled_rx)) = (
            self.synced_rx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
            self.pulled_rx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        ) else {
            return Err(Error::AlreadyServing);
        };

        // Connect to the master. The channel connects on first use, as Go's Dial.
        let address = join_host_port(&self.options.master_host, self.options.master_port);
        let channel = master_channel(&address, self.options.tls.as_ref())?;
        {
            let mut state = self.state();
            state.master = Some(
                MasterClient::new(channel.clone()).max_decoding_message_size(MAX_RECV_MESSAGE_SIZE),
            );
            state.conn = Some(channel);
        }

        let sync = {
            let worker = Arc::clone(&self);
            tokio::spawn(async move { worker.sync().await })
        };
        let pull = {
            let worker = Arc::clone(&self);
            tokio::spawn(async move { worker.pull(synced_rx).await })
        };
        let http = {
            let worker = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(err) = worker.serve_http().await {
                    error!(error = %err, "failed to start http server");
                }
            })
        };

        // The recommendation loop: a tick, or a pulled model, runs one pass.
        let interval = self.options.tick_interval;
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut pulled_rx = pulled_rx;
        let mut shutdown = self.shutdown.subscribe();
        loop {
            tokio::select! {
                // The watch guard is dropped inside this block, so the serve future stays `Send`.
                _ = async { let _ = shutdown.wait_for(|stopped| *stopped).await; } => break,
                tick = ticker.tick() => {
                    // A tick that fired long ago is skipped, as Go's check does.
                    if tick.elapsed() <= interval {
                        self.recommend_once().await;
                    }
                }
                Some(()) = pulled_rx.recv() => self.recommend_once().await,
            }
        }

        // Stop the sync and the HTTP server, then close the connections and stores.
        pull.abort();
        let _ = sync.await;
        let _ = http.await;
        self.state().conn = None;
        self.state().master = None;
        if let Err(err) = self.pipeline.data().close().await {
            error!(error = %err, "failed to close data database");
        }
        if let Err(err) = self.pipeline.cache().close().await {
            error!(error = %err, "failed to close cache database");
        }
        if let Err(err) = self.pipeline.vectors().close().await {
            error!(error = %err, "failed to close vector database");
        }
        Ok(())
    }

    /// One recommendation run for the users this worker is assigned, with the
    /// progress reported to the master.
    async fn recommend_once(&self) {
        let (peers, me, master) = {
            let state = self.state();
            (state.peers.clone(), state.me.clone(), state.master.clone())
        };
        let users = match pull_users(self.pipeline.data().as_ref(), &peers, &me).await {
            Ok(users) => users,
            Err(err) => {
                error!(error = %err, me = %me, workers = ?peers, "failed to split users");
                return;
            }
        };

        let pipeline = Arc::clone(&self.pipeline);
        let progress: ProgressFn = Box::new(move |completed, throughput| {
            info!(
                n_complete_users = completed,
                throughput, "ranking recommendation"
            );
            if let Some(master) = &master {
                let request = monitor::encode_progress(&pipeline.tracer().list());
                let mut master = master.clone();
                tokio::spawn(async move {
                    if let Err(err) = master.push_progress(request).await {
                        error!(error = %err, "failed to report update task");
                    }
                });
            }
        });
        Arc::clone(&self.pipeline)
            .recommend(Context::background(), users, Some(progress))
            .await;
    }

    /// Go's `ServeHTTP`: serves the metrics and the health routes until shut down.
    async fn serve_http(&self) -> Result<(), Error> {
        let listener = tokio::net::TcpListener::bind(join_host_port(
            &self.options.http_host,
            self.options.http_port,
        ))
        .await?;
        let mut stop = self.shutdown.subscribe();
        axum::serve(listener, router(Arc::clone(&self.pipeline)))
            .with_graceful_shutdown(async move {
                let _ = stop.wait_for(|stopped| *stopped).await;
            })
            .await?;
        Ok(())
    }

    /// Go's `Shutdown`: ends the sync, the recommendation loop and the HTTP
    /// server. It is safe to call more than once.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send_replace(true);
    }

    /// Locks the sync state, going on past a poisoned lock.
    fn state(&self) -> MutexGuard<'_, SyncState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Go's `WorkerName`: the MD5 of the host name, the HTTP host and the HTTP port,
/// in hex. Go hashes them in that order without sorting.
pub fn worker_name(hostname: &str, http_host: &str, http_port: u16) -> String {
    let mut hash = Md5::new();
    hash.update(hostname.as_bytes());
    hash.update(http_host.as_bytes());
    hash.update(http_port.to_string().as_bytes());
    hex::encode(hash.finalize())
}

/// The host name of this machine. On Linux, as Go's `os.Hostname` does, it is
/// the kernel's host name, with the `HOSTNAME` variable as a fallback.
fn hostname() -> Result<String, Error> {
    match std::fs::read_to_string("/proc/sys/kernel/hostname") {
        Ok(name) => Ok(name.trim_end().to_owned()),
        Err(err) => std::env::var("HOSTNAME").map_err(|_| Error::Hostname(err)),
    }
}

/// Go's `pullUsers`: the users that the consistent-hash ring of the peers
/// assigns to `me`. The user index is read in batches.
///
/// # Errors
///
/// Fails when `me` is not one of the peers, or the data store fails.
pub async fn pull_users(
    data: &dyn data::Database,
    peers: &[String],
    me: &str,
) -> Result<Vec<User>, Error> {
    let ctx = Context::background();
    if !peers.iter().any(|peer| peer == me) {
        return Err(Error::NotInWorkers);
    }
    let mut ring = Consistent::new();
    for peer in peers {
        ring.add(peer);
    }
    let mut users = Vec::new();
    let mut cursor = String::new();
    loop {
        let (next, batch) = data.get_users(&ctx, &cursor, BATCH_SIZE).await?;
        for user in batch {
            if ring.get(&user.user_id)? == me {
                users.push(user);
            }
        }
        if next.is_empty() {
            break;
        }
        cursor = next;
    }
    Ok(users)
}

/// Go's `HealthStatus`: whether the worker can serve, and what its stores said.
/// It marshals with Go's field names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HealthStatus {
    /// Whether both stores are connected.
    pub ready: bool,
    /// The data store's error, if its ping failed.
    pub data_store_error: Option<HealthError>,
    /// The cache store's error, if its ping failed.
    pub cache_store_error: Option<HealthError>,
    /// Whether the data store answered its ping.
    pub data_store_connected: bool,
    /// Whether the cache store answered its ping.
    pub cache_store_connected: bool,
}

/// An error in a [`HealthStatus`]. Go marshals an error as `{}`, since its
/// message is not an exported field, and so does this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthError {}

/// Go's `checkHealth`: pings both stores.
async fn check_health(pipeline: &Pipeline) -> HealthStatus {
    let data_error = pipeline.data().ping().await.err();
    let cache_error = pipeline.cache().ping().await.err();
    let data_store_connected = data_error.is_none();
    let cache_store_connected = cache_error.is_none();
    HealthStatus {
        ready: data_store_connected && cache_store_connected,
        data_store_error: data_error.map(|_| HealthError {}),
        cache_store_error: cache_error.map(|_| HealthError {}),
        data_store_connected,
        cache_store_connected,
    }
}

/// Go's `checkLive`: the health status, always with 200.
pub async fn check_live(State(pipeline): State<Arc<Pipeline>>) -> Response {
    let status = check_health(&pipeline).await;
    write_json(&status)
}

/// Go's `checkReady`: the health status with 200 when the stores are connected,
/// and the status as the error with 503 when they are not.
pub async fn check_ready(State(pipeline): State<Arc<Pipeline>>) -> Response {
    let status = check_health(&pipeline).await;
    if status.ready {
        write_json(&status)
    } else {
        match serde_json::to_string(&status) {
            Ok(reason) => write_error(&reason, StatusCode::SERVICE_UNAVAILABLE),
            Err(err) => write_error(&err.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
}

/// Go's `writeJSON`: a 200 response with the JSON. Go's `net/http` sniffs the
/// body as `text/plain`, so this sets that type.
fn write_json<T: Serialize>(content: &T) -> Response {
    match serde_json::to_string(content) {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(err) => write_error(&err.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Go's `writeError`: logs the error and answers with it as `http.Error` does,
/// with a trailing newline and `nosniff`.
fn write_error(error: &str, code: StatusCode) -> Response {
    error!(
        status = %code.canonical_reason().unwrap_or_default().to_ascii_lowercase(),
        error = %error,
        "request failed"
    );
    (
        code,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        format!("{error}\n"),
    )
        .into_response()
}

/// Go's `/metrics`: the default registry in the Prometheus text format.
async fn metrics_route() -> Response {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();
    match encoder.encode_to_string(&prometheus::gather()) {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, encoder.format_type().to_owned())],
            body,
        )
            .into_response(),
        Err(err) => write_error(&err.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// The HTTP routes of a worker. Go registers them on the default mux, which
/// serves any method on them.
fn router(pipeline: Arc<Pipeline>) -> Router {
    Router::new()
        .route("/metrics", any(metrics_route))
        .route("/api/health/live", any(check_live))
        .route("/api/health/ready", any(check_ready))
        .with_state(pipeline)
}

/// The Go `time.Duration` as a standard one. A negative duration is zero.
fn std_duration(duration: GoDuration) -> Duration {
    Duration::from_nanos(u64::try_from(duration.0).unwrap_or_default())
}

/// The channel to the master. Without TLS it is plain HTTP/2. With TLS it presents
/// this worker's certificate and checks the master's against the CA, without
/// comparing host names, as Go's `advancedtls` does. Either channel connects on
/// first use.
fn master_channel(address: &str, tls: Option<&TlsConfig>) -> Result<Channel, Error> {
    let Some(tls) = tls else {
        return Ok(Endpoint::from_shared(format!("http://{address}"))
            .map_err(|err| Error::MasterAddress(err.to_string()))?
            .connect_lazy());
    };
    let mut config = gorse_common::util::tls::new_client_config(tls)?;
    // The connector sets its own ALPN list, and refuses one that is already set.
    config.alpn_protocols.clear();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_only()
        .enable_http2()
        .build();
    // tonic wraps a custom connector: for an `https` endpoint it runs its own TLS
    // over the connector's I/O, and refuses when the endpoint has no TLS config.
    // The handshake already runs in `connector`, so the endpoint keeps the `http`
    // scheme and the connector dials the master's `https` URI.
    let target = Uri::try_from(format!("https://{address}"))
        .map_err(|err| Error::MasterAddress(err.to_string()))?;
    let endpoint = Endpoint::from_shared(format!("http://{address}"))
        .map_err(|err| Error::MasterAddress(err.to_string()))?;
    Ok(endpoint.connect_with_connector_lazy(MasterConnector { connector, target }))
}

/// The connector of a TLS channel to the master. It ignores the URI tonic passes
/// and dials `target`, so the inner connector runs the TLS handshake.
#[derive(Clone)]
struct MasterConnector<C> {
    connector: C,
    target: Uri,
}

impl<C: Service<Uri>> Service<Uri> for MasterConnector<C> {
    type Response = C::Response;
    type Error = C::Error;
    type Future = C::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.connector.poll_ready(cx)
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        self.connector.call(self.target.clone())
    }
}

/// Go's `net.JoinHostPort`: brackets an IPv6 host.
fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests;
