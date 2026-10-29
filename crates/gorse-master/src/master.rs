// Ports: master/master.go @ add21b5

//! The master node: it opens the stores, keeps the cluster's metadata and
//! models, serves the gRPC services the workers and servers call, and runs the
//! training tasks on a timer and on each config change. Go's `master.go`.
//!
//! The REST server and the dashboard are WP-4-4's (`rest.rs`); [`Master::serve`]
//! starts everything that `Serve` starts before its `StartHttpServer` call.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration as StdDuration;

use gorse_common::gocontext::{CancelFunc, Context};
use gorse_common::gotime::Duration;
use gorse_common::monitor::{self, Monitor, Progress};
use gorse_common::util::tls::{TlsConfig, new_server_config};
use gorse_config::config::load::load_config;
use gorse_config::config::schema::Config;
use gorse_model::cf::model as cf;
use gorse_model::ctr::model as ctr;
use gorse_protocol::{
    blob_store_server::BlobStoreServer, cache_store_server::CacheStoreServer,
    data_store_server::DataStoreServer, master_server::MasterServer,
    vector_store_server::VectorStoreServer,
};
use gorse_storage::blob;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::meta;
use gorse_storage::vectors;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::rpc;
use crate::Error;

/// How long a config change waits for the writes to settle before it is read
/// (Go's `configReloadDebounce`).
const CONFIG_RELOAD_DEBOUNCE: StdDuration = StdDuration::from_secs(1);

/// The collaborative filtering model the master has: the model in use, the
/// size of the feedback it was trained on, and the best model the search found
/// (Go's `collaborativeFilteringMeta`, `collaborativeFilteringTrainSetSize` and
/// `collaborativeFilteringTarget`, under one lock).
#[derive(Debug, Default, Clone)]
pub struct CollaborativeFilteringState {
    /// The model in use (Go's `collaborativeFilteringMeta`).
    pub meta: meta::Model<cf::Score>,
    /// The number of feedback records the model was trained on.
    pub train_set_size: usize,
    /// The best model the optimisation found (Go's `collaborativeFilteringTarget`).
    pub target: meta::Model<cf::Score>,
}

/// The click-through rate model the master has, with the same three parts as
/// [`CollaborativeFilteringState`] (Go's `clickThroughRateMeta` and friends).
#[derive(Debug, Default, Clone)]
pub struct ClickThroughRateState {
    /// The model in use (Go's `clickThroughRateMeta`).
    pub meta: meta::Model<ctr::Score>,
    /// The number of feedback records the model was trained on.
    pub train_set_size: usize,
    /// The best model the optimisation found (Go's `clickThroughRateTarget`).
    pub target: meta::Model<ctr::Score>,
}

/// The master node (Go's `Master`).
pub struct Master {
    /// The configuration in force. Go's `ConfigMutex` guards it, and the
    /// config reload replaces it under the same lock.
    pub(crate) config: RwLock<Arc<Config>>,
    /// The config file the master watches (Go's `configPath`). Empty when none.
    pub(crate) config_path: String,
    /// The directory the meta database and the local blobs live in (Go's `cachePath`).
    pub(crate) cache_path: String,
    /// Whether the master also runs the recommendation itself (Go's `standalone`).
    pub(crate) standalone: bool,
    /// The task monitor (Go's `tracer`).
    pub(crate) tracer: Monitor,
    /// The progress each remote node last pushed, by tracer (Go's `remoteProgress`).
    pub(crate) remote_progress: Mutex<HashMap<String, Vec<Progress>>>,
    /// The data store the master reads and trains from (Go's `DataClient`).
    pub(crate) data: RwLock<Arc<dyn data::Database>>,
    /// The cache store the master writes results to (Go's `CacheClient`).
    pub(crate) cache: RwLock<Arc<dyn cache::Database>>,
    /// The vector store the master indexes items and users in (Go's `VectorClient`).
    pub(crate) vectors: RwLock<Arc<dyn vectors::Database>>,
    /// The meta database. `None` until [`Master::serve`] opens it (Go's `metaStore`).
    pub(crate) meta: RwLock<Option<Arc<dyn meta::Database>>>,
    /// The blob store the models are kept in. `None` until [`Master::serve`]
    /// opens it (Go's `blobStore`).
    pub(crate) blob: RwLock<Option<Arc<dyn blob::Store>>>,
    /// The collaborative filtering model (Go's `collaborativeFilteringMeta` and friends).
    pub(crate) collaborative_filtering: RwLock<CollaborativeFilteringState>,
    /// The click-through rate model (Go's `clickThroughRateMeta` and friends).
    pub(crate) click_through_rate: RwLock<ClickThroughRateState>,
    /// Whether a dataset reload is running. A second one that starts while it
    /// runs skips (Go's `reconciling`).
    pub(crate) reconciling: Arc<AtomicBool>,
    /// Asks the task loop to run now, with a buffer of one (Go's `scheduled`).
    pub(crate) scheduled: mpsc::Sender<()>,
    /// The receiving end of `scheduled`, taken by the task loop when it starts.
    scheduled_rx: Mutex<Option<mpsc::Receiver<()>>>,
    /// The period of the task loop's timer (Go's `ticker`).
    pub(crate) fit_period: Duration,
    /// Tells the background loops to stop (Go's `backgroundCancel` for loops).
    pub(crate) shutdown: watch::Sender<bool>,
    /// The context of the background tasks (Go's `backgroundContext`).
    pub(crate) background: Context,
    /// Cancels `background` (Go's `backgroundCancel`).
    pub(crate) background_cancel: CancelFunc,
    /// The background loops and servers Serve starts (Go's `backgroundWaitGroup`).
    pub(crate) background_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Stops the gRPC server when it is sent (Go's `grpcServer.GracefulStop`).
    grpc_shutdown: Mutex<Option<watch::Sender<bool>>>,
}

impl Master {
    /// Go's `NewMaster`: a master over `cfg` that keeps its state in
    /// `cache_folder`. The stores are empty until [`Master::serve`] opens them.
    pub fn new(cfg: Config, cache_folder: &str, standalone: bool, config_path: &str) -> Arc<Self> {
        // Go registers its metrics when the package starts. `register` forces
        // each one once, so repeated masters in one process share them.
        crate::metrics::register();
        let (background, background_cancel) = Context::background().with_cancel();
        let (scheduled, scheduled_rx) = mpsc::channel(1);
        let (shutdown, _) = watch::channel(false);
        let fit_period = cfg
            .recommend
            .collaborative
            .fit_period
            .min(cfg.recommend.ranker.fit_period);
        Arc::new(Self {
            config: RwLock::new(Arc::new(cfg)),
            config_path: config_path.to_owned(),
            cache_path: cache_folder.to_owned(),
            standalone,
            tracer: monitor::new_tracer("master"),
            remote_progress: Mutex::new(HashMap::new()),
            data: RwLock::new(Arc::new(data::no_database::NoDatabase)),
            cache: RwLock::new(Arc::new(cache::no_database::NoDatabase)),
            vectors: RwLock::new(Arc::new(vectors::NoDatabase)),
            meta: RwLock::new(None),
            blob: RwLock::new(None),
            collaborative_filtering: RwLock::new(CollaborativeFilteringState::default()),
            click_through_rate: RwLock::new(ClickThroughRateState::default()),
            reconciling: Arc::new(AtomicBool::new(false)),
            scheduled,
            scheduled_rx: Mutex::new(Some(scheduled_rx)),
            fit_period,
            shutdown,
            background,
            background_cancel,
            background_tasks: Mutex::new(Vec::new()),
            grpc_shutdown: Mutex::new(None),
        })
    }

    /// The configuration in force now.
    pub fn config(&self) -> Arc<Config> {
        self.config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the stores the master reads and writes. Go sets them in the
    /// struct before `Serve`, and the tests do the same.
    pub fn set_stores(
        &self,
        data: Arc<dyn data::Database>,
        cache: Arc<dyn cache::Database>,
        vectors: Arc<dyn vectors::Database>,
    ) {
        *self.data.write().unwrap_or_else(PoisonError::into_inner) = data;
        *self.cache.write().unwrap_or_else(PoisonError::into_inner) = cache;
        *self.vectors.write().unwrap_or_else(PoisonError::into_inner) = vectors;
    }

    /// The data store in use now.
    pub fn data(&self) -> Arc<dyn data::Database> {
        self.data
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The cache store in use now.
    pub fn cache(&self) -> Arc<dyn cache::Database> {
        self.cache
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The vector store in use now.
    pub fn vectors(&self) -> Arc<dyn vectors::Database> {
        self.vectors
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The meta database. Panics before [`Master::serve`] opens it, which Go's
    /// nil pointer would also do.
    pub fn meta_store(&self) -> Arc<dyn meta::Database> {
        self.meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("the meta database is opened by serve")
    }

    /// The blob store. Panics before [`Master::serve`] opens it.
    pub fn blob_store(&self) -> Arc<dyn blob::Store> {
        self.blob
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .expect("the blob store is opened by serve")
    }

    /// Go's `applyRecommendOverride`: replaces `cfg.recommend` with the
    /// recommendation config the meta database holds, when it holds one.
    pub(crate) async fn apply_recommend_override(&self, cfg: &mut Config) -> Result<(), Error> {
        let Some(meta_str) = self.meta_store().get(meta::RECOMMEND_CONFIG).await? else {
            return Ok(());
        };
        cfg.recommend.unmarshal_json(&meta_str).map_err(|err| Error::Config(err.to_string()))?;
        Ok(())
    }

    /// Go's `reloadConfigFromFile`: reads the config file, applies the
    /// recommendation override, validates the result, and swaps it in. Then it
    /// asks the task loop to run.
    pub(crate) async fn reload_config_from_file(&self) -> Result<(), Error> {
        let mut new_config = load_config(&self.config_path).map_err(|err| Error::Config(err.to_string()))?;
        self.apply_recommend_override(&mut new_config).await?;
        if let Err(err) = new_config.validate() {
            return Err(Error::Config(err.to_string()));
        }
        *self.config.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(new_config);
        let _ = self.scheduled.try_send(());
        Ok(())
    }

    /// Go's `watchConfigFile`: reloads the config file when it changes, after
    /// the writes settle. It stops when the master shuts down.
    async fn watch_config_file(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        if self.config_path.is_empty() {
            return;
        }
        let abs_path = match std::path::absolute(&self.config_path) {
            Ok(path) => path,
            Err(err) => {
                error!(path = %self.config_path, error = %err, "failed to resolve config path");
                return;
            }
        };
        if let Err(err) = std::fs::metadata(&abs_path) {
            tracing::warn!(path = %abs_path.display(), error = %err, "skip watching config file");
            return;
        }
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = event_tx.send(event);
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(err) => {
                error!(error = %err, "failed to create config watcher");
                return;
            }
        };
        let dir = abs_path.parent().map_or_else(|| PathBuf::from("."), PathBuf::from);
        if let Err(err) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            error!(path = %abs_path.display(), error = %err, "failed to watch config directory");
            return;
        }
        info!(path = %abs_path.display(), "watch config file");

        let mut reload_at: Option<tokio::time::Instant> = None;
        loop {
            let deadline = reload_at;
            tokio::select! {
                _ = shutdown.changed() => return,
                event = event_rx.recv() => {
                    let Some(event) = event else { return };
                    match event {
                        Ok(event) => {
                            let touches_config = event.paths.iter().any(|path| {
                                std::path::absolute(path).is_ok_and(|path| path == abs_path)
                            });
                            let changed = matches!(
                                event.kind,
                                EventKind::Create(_) | EventKind::Modify(_)
                            );
                            if touches_config && changed {
                                reload_at = Some(tokio::time::Instant::now() + CONFIG_RELOAD_DEBOUNCE);
                            }
                        }
                        Err(err) => error!(error = %err, "failed to watch config file"),
                    }
                }
                () = wait_until(deadline) => {
                    reload_at = None;
                    match self.reload_config_from_file().await {
                        Ok(()) => info!(path = %abs_path.display(), "reloaded config file"),
                        Err(err) => error!(
                            path = %abs_path.display(),
                            error = %err,
                            "failed to reload config file"
                        ),
                    }
                }
            }
        }
    }

    /// Go's `Serve` up to its `StartHttpServer` call. It connects the blob
    /// store, the meta database, the data, cache and vector stores, loads the
    /// models, and starts the config watcher, the task loop and the gRPC server.
    /// The HTTP server is started by the REST work package.
    pub async fn serve(self: &Arc<Self>) -> Result<(), Error> {
        let cfg = self.config();

        // connect blob store
        let blob_server = if !cfg.blob.uri.contains("://") {
            Some(Arc::new(blob::master::MasterStoreServer::new(&cfg.blob.uri)?))
        } else {
            None
        };
        let blob_store = blob::new_store(&cfg.blob, None)?;
        *self.blob.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::from(blob_store));

        // connect meta database
        let meta_path = format!("sqlite://{}/meta.sqlite3", self.cache_path);
        let meta_store = meta::open(&meta_path, cfg.master.meta_timeout).await?;
        meta_store.init().await?;
        let meta_store: Arc<dyn meta::Database> = Arc::from(meta_store);
        *self.meta.write().unwrap_or_else(PoisonError::into_inner) = Some(meta_store);

        // connect data database
        let data_opts = cfg.database.storage_options(&cfg.database.data_store);
        let mut data_registry = data::Registry::default();
        data::register_builtin(&mut data_registry);
        let data_client: Arc<dyn data::Database> = Arc::from(
            data_registry
                .open(
                    &cfg.database.data_store,
                    &cfg.database.data_table_prefix,
                    &data_opts,
                )
                .await?,
        );
        data_client.init().await?;

        // connect cache database
        let cache_opts = cfg.database.storage_options(&cfg.database.cache_store);
        let mut cache_registry = cache::Registry::default();
        cache::register_builtin(&mut cache_registry);
        let cache_client: Arc<dyn cache::Database> = Arc::from(
            cache_registry
                .open(
                    &cfg.database.cache_store,
                    &cfg.database.cache_table_prefix,
                    &cache_opts,
                )
                .await?,
        );
        cache_client.init().await?;

        // open vector store
        info!(path = %cfg.database.vector_store, "opening vector store");
        let vector_client: Arc<dyn vectors::Database> = Arc::from(
            vectors::open(
                &cfg.database.vector_store,
                &cfg.database.vector_table_prefix,
                &[
                    gorse_storage_core::options::with_num_jobs(cfg.master.n_jobs),
                    gorse_storage_core::options::with_skip_unindexed_segments(true),
                ],
            )
            .await?,
        );
        vector_client.init().await?;
        self.set_stores(data_client, cache_client, vector_client);

        // load config overrides
        let mut overridden = self.config().as_ref().clone();
        if let Err(err) = self.apply_recommend_override(&mut overridden).await {
            error!(error = %err, "failed to apply config overrides");
        } else {
            *self.config.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(overridden);
        }

        // load collective filtering model meta
        self.load_collaborative_filtering_meta().await;
        self.load_click_through_rate_meta().await;

        let config_master = Arc::clone(self);
        let config_shutdown = self.shutdown.subscribe();
        let watch_task = tokio::spawn(config_master.watch_config_file(config_shutdown));
        let loop_master = Arc::clone(self);
        let loop_task = tokio::spawn(async move {
            loop_master.run_tasks_loop().await;
        });
        let mut tasks = self
            .background_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tasks.push(watch_task);
        tasks.push(loop_task);
        drop(tasks);

        // start rpc server
        info!(
            host = %cfg.master.host,
            port = cfg.master.port,
            ssl_mode = cfg.master.ssl_mode,
            ssl_ca = %cfg.master.ssl_ca,
            ssl_cert = %cfg.master.ssl_cert,
            ssl_key = %cfg.master.ssl_key,
            "start rpc server"
        );
        let tls = if cfg.master.ssl_mode {
            Some(TlsConfig {
                ssl_ca: cfg.master.ssl_ca.clone(),
                ssl_cert: cfg.master.ssl_cert.clone(),
                ssl_key: cfg.master.ssl_key.clone(),
            })
        } else {
            None
        };
        self.serve_grpc(
            &format!("{}:{}", cfg.master.host, cfg.master.port),
            tls,
            blob_server,
        )
        .await?;
        Ok(())
    }

    /// Loads the collaborative filtering model meta. A failure is logged and the
    /// master carries on with no model, as Go does.
    async fn load_collaborative_filtering_meta(&self) {
        let value = match self
            .meta_store()
            .get(meta::COLLABORATIVE_FILTERING_MODEL)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                error!(error = %err, "failed to load collaborative filtering meta");
                return;
            }
        };
        let Some(value) = value else { return };
        let mut state = self
            .collaborative_filtering
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(err) = state.meta.from_json(&value) {
            error!(error = %err, "failed to unmarshal collaborative filtering meta");
        } else {
            info!(
                r#type = %state.meta.r#type,
                params = ?state.meta.params,
                score = ?state.meta.score,
                "loaded collaborative filtering model"
            );
        }
    }

    /// Loads the click-through rate model meta (Go's `Serve`, click-through rate half).
    async fn load_click_through_rate_meta(&self) {
        let value = match self.meta_store().get(meta::CLICK_THROUGH_RATE_MODEL).await {
            Ok(value) => value,
            Err(err) => {
                error!(error = %err, "failed to load click-through rate meta");
                return;
            }
        };
        let Some(value) = value else { return };
        let mut state = self
            .click_through_rate
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(err) = state.meta.from_json(&value) {
            error!(error = %err, "failed to unmarshal click-through rate meta");
        } else {
            info!(
                r#type = %state.meta.r#type,
                params = ?state.meta.params,
                score = ?state.meta.score,
                "loaded click-through rate model"
            );
        }
    }

    /// Go's `Shutdown`, without the HTTP server (see [`Master::serve`]). It stops
    /// the gRPC server, stops the background tasks before closing the databases,
    /// and closes them.
    pub async fn shutdown(self: &Arc<Self>) {
        if let Some(stop) = self
            .grpc_shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = stop.send(true);
        }
        let _ = self.shutdown.send(true);
        self.background_cancel.cancel();
        let tasks = std::mem::take(
            &mut *self
                .background_tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for task in tasks {
            let _ = task.await;
        }
        if let Some(meta_store) = self
            .meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            && let Err(err) = meta_store.close().await
        {
            error!(error = %err, "failed to close meta database");
        }
        if let Err(err) = self.data().close().await {
            error!(error = %err, "failed to close data database");
        }
        if let Err(err) = self.cache().close().await {
            error!(error = %err, "failed to close cache database");
        }
        if let Err(err) = self.vectors().close().await {
            error!(error = %err, "failed to close vector database");
        }
    }

    /// Go's `RunTasksLoop`: runs the dataset task on the timer and whenever it
    /// is scheduled, until the master shuts down.
    pub async fn run_tasks_loop(self: &Arc<Self>) {
        let Some(mut scheduled) = self
            .scheduled_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let _ = self.scheduled.try_send(());
        let mut shutdown = self.shutdown.subscribe();
        let mut ticker = tokio::time::interval(ticker_period(self.fit_period));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = ticker.tick() => {}
                _ = scheduled.recv() => {}
            }
            // download dataset
            let (ctx, cancel) = self.background.with_cancel();
            let result = self.run_load_dataset_task(&ctx).await;
            cancel.cancel();
            if self.background.err().is_some() {
                return;
            }
            if let Err(err) = result {
                error!(error = %err, "failed to load ranking dataset");
            }
        }
    }

    /// Records the progress a remote node pushed under its tracer name.
    pub(crate) fn store_remote_progress(&self, tracer: &str, progress: Vec<Progress>) {
        self.remote_progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tracer.to_owned(), progress);
    }

    /// Starts the gRPC server on `address`, with the TLS files when there are
    /// any. It returns once the listener is bound, and serves until
    /// [`Master::shutdown`].
    async fn serve_grpc(
        self: &Arc<Self>,
        address: &str,
        tls: Option<TlsConfig>,
        blob_server: Option<Arc<blob::master::MasterStoreServer>>,
    ) -> Result<(), Error> {
        let listener = tokio::net::TcpListener::bind(address).await.map_err(|err| {
            error!(error = %err, "failed to listen");
            err
        })?;
        let acceptor = match &tls {
            Some(tls) => Some(tokio_rustls::TlsAcceptor::from(Arc::new(
                new_server_config(tls)?,
            ))),
            None => None,
        };
        let (stop_tx, mut stop_rx) = watch::channel(false);
        *self
            .grpc_shutdown
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(stop_tx);

        let mut router = tonic::transport::Server::builder()
            .add_service(MasterServer::from_arc(Arc::clone(self)))
            .add_service(CacheStoreServer::new(cache::proxy::ProxyServer::new(
                self.cache(),
            )))
            .add_service(DataStoreServer::new(data::proxy::ProxyServer::new(self.data())))
            .add_service(VectorStoreServer::new(vectors::proxy::ProxyServer::new(
                self.vectors(),
            )));
        if let Some(blob_server) = blob_server {
            router = router.add_service(BlobStoreServer::from_arc(blob_server));
        }

        let incoming = rpc::incoming(listener, acceptor, stop_rx.clone());
        let task = tokio::spawn(async move {
            let shutdown = async move {
                let _ = stop_rx.wait_for(|stop| *stop).await;
            };
            if let Err(err) = router
                .serve_with_incoming_shutdown(incoming, shutdown)
                .await
            {
                error!(error = %err, "failed to start rpc server");
            }
        });
        self.background_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(task);
        Ok(())
    }
}

/// The period of a timer, as Go's `time.NewTicker` takes it. Go panics on a
/// period that is not positive, and so does this.
fn ticker_period(period: Duration) -> StdDuration {
    let nanos = period.nanoseconds();
    assert!(nanos > 0, "non-positive interval for NewTicker");
    StdDuration::from_nanos(nanos as u64)
}

/// Waits until `deadline`, or never when there is none.
async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
