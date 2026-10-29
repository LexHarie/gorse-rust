//! The mixed-cluster harness (WP-4-1).
//!
//! A [`Cluster`] is one run: a temporary directory with a config written from
//! `contracts/config/config.toml`, one SQLite data store, one SQLite cache store
//! and one `xvec` vector store, and the nodes that use them. The Go nodes are
//! child processes, built once by [`GoBuild`] and started with the flags in
//! `contracts/cli/*.help.txt`. A Rust worker runs in this process through
//! [`Cluster::start_rust_worker`]. Every node stops when the cluster drops.
//!
//! Nothing here skips a run. A missing Go toolchain, a failed build, or a node
//! that does not become ready is an error, and the caller fails.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::future::Future;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use gorse_common::util::tls::TlsConfig;
use gorse_config::config::load::load_config;
use gorse_master::Master;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_worker::{Worker, WorkerOptions};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::schema::{Error, Result, fail, repo_root};

/// How long a node may take to become ready, or a condition may take to hold.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a condition is checked while a node starts.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long one readiness request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The Go packages a cluster needs, built into one directory.
const GO_PACKAGES: [&str; 3] = [
    "./cmd/gorse-master",
    "./cmd/gorse-server",
    "./cmd/gorse-worker",
];

/// The Go binaries of one build of the repository.
#[derive(Debug)]
pub struct GoBuild {
    dir: PathBuf,
}

impl GoBuild {
    /// Builds the Go nodes into `gorse-go-nodes` beside the test binary. Each
    /// process builds again, which replaces the last build; the first build
    /// downloads modules, so it can take minutes.
    ///
    /// # Errors
    ///
    /// Fails when `go` is not installed or the build fails. The mixed-cluster
    /// test needs the Go oracle, so it fails with this error rather than skip.
    pub fn build() -> Result<GoBuild> {
        let dir = build_dir()?;
        fs::create_dir_all(&dir).map_err(fail)?;
        let output = Command::new("go")
            .current_dir(repo_root())
            .args(["build", "-o"])
            .arg(&dir)
            .args(GO_PACKAGES)
            .output()
            .map_err(|e| fail(format!("go is needed by the mixed-cluster tests: {e}")))?;
        if !output.status.success() {
            return Err(fail(format!(
                "go build of the Go nodes failed ({}):\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(GoBuild { dir })
    }

    /// The build of this test process. It runs once, and every cluster shares it.
    ///
    /// # Errors
    ///
    /// Fails with the error of [`GoBuild::build`].
    pub fn shared() -> Result<&'static GoBuild> {
        static BUILD: OnceLock<Result<GoBuild>> = OnceLock::new();
        BUILD
            .get_or_init(GoBuild::build)
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The path of one built node, such as `gorse-master`.
    fn binary(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// The directory the Go nodes are built into: `gorse-go-nodes` in the profile
/// directory of the test binary (`target/<profile>`). The build of the last
/// process is replaced, so the nodes do not pile up in the temporary directory.
fn build_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(fail)?;
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| fail("the test binary is not under a target directory".to_owned()))?;
    Ok(profile.join("gorse-go-nodes"))
}

/// The ports a run listens on. Each is free when the run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// The master's gRPC port.
    pub master_grpc: u16,
    /// The master's HTTP port.
    pub master_http: u16,
    /// The server's HTTP port.
    pub server_http: u16,
    /// The worker's HTTP port, for health and metrics.
    pub worker_http: u16,
}

/// One key of the config to set: a section, a key, and the value as TOML text,
/// such as `("recommend.ranker", "type", "\"none\"")`.
pub type Edit<'a> = (&'a str, &'a str, &'a str);

/// The PEM files of a TLS run: the CA that signs every node's certificate, and
/// the certificate and key that every node presents.
#[derive(Debug)]
struct TlsFiles {
    ca: PathBuf,
    cert: PathBuf,
    key: PathBuf,
}

/// One run of a mixed cluster: its directory, its config and its nodes.
pub struct Cluster {
    dir: tempfile::TempDir,
    build: &'static GoBuild,
    config: PathBuf,
    /// The ports the nodes listen on.
    pub ports: Ports,
    data_uri: String,
    cache_uri: String,
    tls: Option<TlsFiles>,
    processes: Vec<(&'static str, Child)>,
}

impl Cluster {
    /// Starts a run: writes the config from `contracts/config/config.toml` with
    /// this run's stores and ports, applies `edits` in order, and starts no node.
    ///
    /// # Errors
    ///
    /// Fails when the config or the run's directories cannot be written.
    pub fn new(build: &'static GoBuild, edits: &[Edit<'_>]) -> Result<Cluster> {
        Self::create(build, edits, false, None)
    }

    /// Starts a run like [`Cluster::new`], with the data store at `data_uri`
    /// when it is given, and the run's SQLite file when it is not. The store
    /// is what the Go and the Rust nodes both open, so it must be shared
    /// between them only as the run's own.
    ///
    /// # Errors
    ///
    /// Fails when the config or the run's directories cannot be written.
    pub fn new_with_store(
        build: &'static GoBuild,
        edits: &[Edit<'_>],
        data_uri: Option<&str>,
    ) -> Result<Cluster> {
        Self::create(build, edits, false, data_uri)
    }

    /// Starts a run like [`Cluster::new`], with mutual TLS between the nodes. The
    /// master requires a client certificate signed by the run's CA, and every node
    /// presents the same certificate, which that CA signs.
    ///
    /// # Errors
    ///
    /// Fails when the certificates or the config cannot be written.
    pub fn new_tls(build: &'static GoBuild, edits: &[Edit<'_>]) -> Result<Cluster> {
        Self::create(build, edits, true, None)
    }

    /// Writes the run's directories, certificates and config. `secure` turns on
    /// mutual TLS in the master.
    fn create(
        build: &'static GoBuild,
        edits: &[Edit<'_>],
        secure: bool,
        data_uri: Option<&str>,
    ) -> Result<Cluster> {
        let dir = tempfile::tempdir().map_err(fail)?;
        let root = dir.path().to_path_buf();
        let ports = Ports {
            master_grpc: free_port()?,
            master_http: free_port()?,
            server_http: free_port()?,
            worker_http: free_port()?,
        };
        let data_uri = match data_uri {
            Some(uri) => uri.to_owned(),
            None => format!("sqlite://{}", root.join("data.db").display()),
        };
        let cache_uri = format!("sqlite://{}", root.join("cache.db").display());
        let vector_uri = format!("xvec://{}", root.join("vectors").display());
        let blob_dir = root.join("blob");
        fs::create_dir_all(&blob_dir).map_err(fail)?;
        fs::create_dir_all(root.join("master-cache")).map_err(fail)?;

        let mut config =
            fs::read_to_string(repo_root().join("contracts/config/config.toml")).map_err(fail)?;
        set_key(
            &mut config,
            "database",
            "cache_store",
            &toml_string(&cache_uri),
        );
        set_key(
            &mut config,
            "database",
            "data_store",
            &toml_string(&data_uri),
        );
        set_key(
            &mut config,
            "database",
            "vector_store",
            &toml_string(&vector_uri),
        );
        set_key(&mut config, "master", "host", "\"127.0.0.1\"");
        set_key(
            &mut config,
            "master",
            "port",
            &ports.master_grpc.to_string(),
        );
        set_key(&mut config, "master", "http_host", "\"127.0.0.1\"");
        set_key(
            &mut config,
            "master",
            "http_port",
            &ports.master_http.to_string(),
        );
        set_key(
            &mut config,
            "blob",
            "uri",
            &toml_string(&blob_dir.display().to_string()),
        );
        let tls = if secure {
            Some(write_certificates(&root)?)
        } else {
            None
        };
        if let Some(files) = &tls {
            set_key(&mut config, "master", "ssl_mode", "true");
            set_key(
                &mut config,
                "master",
                "ssl_ca",
                &toml_string(&path(&files.ca)),
            );
            set_key(
                &mut config,
                "master",
                "ssl_cert",
                &toml_string(&path(&files.cert)),
            );
            set_key(
                &mut config,
                "master",
                "ssl_key",
                &toml_string(&path(&files.key)),
            );
        }
        for &(section, key, value) in edits {
            set_key(&mut config, section, key, value);
        }
        let config_path = root.join("config.toml");
        fs::write(&config_path, config).map_err(fail)?;

        Ok(Cluster {
            dir,
            build,
            config: config_path,
            ports,
            data_uri,
            cache_uri,
            tls,
            processes: Vec::new(),
        })
    }

    /// Opens the run's data store, the SQLite file the master serves. A test
    /// seeds rows through it before the master starts, and closes it before then.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be opened.
    pub async fn data_store(&self) -> Result<Box<dyn data::Database>> {
        let mut registry = data::Registry::default();
        data::register_builtin(&mut registry);
        registry.open(&self.data_uri, "", &[]).await.map_err(fail)
    }

    /// Opens the run's cache store, the SQLite file the master serves. Call it
    /// once the master is running, so the master has created the tables.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be opened.
    pub async fn cache_store(&self) -> Result<Box<dyn cache::Database>> {
        let mut registry = cache::Registry::default();
        cache::register_builtin(&mut registry);
        registry.open(&self.cache_uri, "", &[]).await.map_err(fail)
    }

    /// Starts the Go `gorse-master` and waits until its gRPC port accepts
    /// connections.
    ///
    /// # Errors
    ///
    /// Fails when the master exits or is not listening within the timeout.
    pub async fn start_master(&mut self) -> Result<()> {
        let args = flags(&[
            ("config", path(&self.config)),
            ("cache-path", path(&self.dir.path().join("master-cache"))),
        ]);
        self.spawn("gorse-master", args)?;
        let port = self.ports.master_grpc;
        self.wait_for("the master gRPC port", move || port_open(port))
            .await
    }

    /// Starts the Go `gorse-server` against the master and waits until its
    /// `/api/health/ready` answers 200.
    ///
    /// # Errors
    ///
    /// Fails when the server exits or is not ready within the timeout.
    pub async fn start_go_server(&mut self) -> Result<()> {
        let args = flags(&[
            ("master-host", "127.0.0.1".to_owned()),
            ("master-port", self.ports.master_grpc.to_string()),
            ("http-host", "127.0.0.1".to_owned()),
            ("http-port", self.ports.server_http.to_string()),
            (
                "cache-path",
                path(&self.dir.path().join("server_cache.data")),
            ),
        ]);
        self.spawn("gorse-server", [args, self.tls_flags()].concat())?;
        let port = self.ports.server_http;
        self.wait_for("the Go server to be ready", move || http_ready(port))
            .await
    }

    /// Starts the Go `gorse-worker` against the master, with the tick interval
    /// and the number of jobs, and waits until its `/api/health/ready` answers 200.
    ///
    /// # Errors
    ///
    /// Fails when the worker exits or is not ready within the timeout.
    pub async fn start_go_worker(&mut self, interval: Duration, jobs: usize) -> Result<()> {
        let args = flags(&[
            ("master-host", "127.0.0.1".to_owned()),
            ("master-port", self.ports.master_grpc.to_string()),
            ("http-host", "127.0.0.1".to_owned()),
            ("http-port", self.ports.worker_http.to_string()),
            ("interval", go_duration(interval)),
            ("jobs", jobs.to_string()),
            (
                "cache-path",
                path(&self.dir.path().join("worker_cache.data")),
            ),
        ]);
        self.spawn("gorse-worker", [args, self.tls_flags()].concat())?;
        let port = self.ports.worker_http;
        self.wait_for("the Go worker to be ready", move || http_ready(port))
            .await
    }

    /// Runs the Rust `gorse-worker` in this process against the master, with the
    /// same settings as [`Cluster::start_go_worker`], and waits until its
    /// `/api/health/ready` answers 200.
    ///
    /// # Errors
    ///
    /// Fails when the worker cannot be created or is not ready within the timeout.
    /// The serve loop is stopped before the error returns.
    pub async fn start_rust_worker(
        &mut self,
        interval: Duration,
        jobs: usize,
    ) -> Result<RustWorker> {
        let options = WorkerOptions {
            master_host: "127.0.0.1".to_owned(),
            master_port: self.ports.master_grpc,
            http_host: "127.0.0.1".to_owned(),
            http_port: self.ports.worker_http,
            jobs,
            tls: self.tls.as_ref().map(|files| TlsConfig {
                ssl_ca: path(&files.ca),
                ssl_cert: path(&files.cert),
                ssl_key: path(&files.key),
            }),
            tick_interval: interval,
        };
        // The worker logs through `tracing`; the log lines are kept for a failure.
        captured();
        let worker = Arc::new(Worker::new(options).map_err(fail)?);
        let serving = Arc::clone(&worker);
        let task = tokio::spawn(async move { serving.serve().await });
        let rust = RustWorker {
            worker,
            task: Some(task),
        };
        let port = self.ports.worker_http;
        match self
            .wait_for("the Rust worker to be ready", move || http_ready(port))
            .await
        {
            Ok(()) => Ok(rust),
            Err(err) => {
                let logs = captured_tail();
                Err(match rust.stop().await {
                    Err(stopped) => fail(format!("{}\n{}\n{logs}", err.0, stopped.0)),
                    Ok(()) => fail(format!("{}\n{logs}", err.0)),
                })
            }
        }
    }

    /// The directory the run's blob store keeps its models in.
    pub fn blob_dir(&self) -> PathBuf {
        self.dir.path().join("blob")
    }

    /// Runs the Rust master in this process against the run's config, with the
    /// same settings as [`Cluster::start_master`], and waits until its gRPC port
    /// listens. The Rust master serves the gRPC services only; its REST routes
    /// are WP-4-4's.
    ///
    /// # Errors
    ///
    /// Fails when the config cannot be read or the master cannot start, or when
    /// the port is not listening within the timeout.
    pub async fn start_rust_master(&mut self) -> Result<RustMaster> {
        // The master logs through `tracing`, as the Rust workers do.
        captured();
        let cache_path = self.dir.path().join("master-cache");
        fs::create_dir_all(&cache_path).map_err(fail)?;
        let cfg = load_config(&path(&self.config)).map_err(fail)?;
        let master = Master::new(cfg, &path(&cache_path), false, &path(&self.config));
        if let Err(err) = master.serve().await {
            master.shutdown().await;
            return Err(fail(err));
        }
        let rust = RustMaster {
            master: Arc::clone(&master),
        };
        let port = self.ports.master_grpc;
        match self
            .wait_for("the Rust master gRPC port", move || port_open(port))
            .await
        {
            Ok(()) => Ok(rust),
            Err(err) => {
                rust.stop().await;
                Err(err)
            }
        }
    }

    /// Waits until `ready` holds. It fails with `what` when a Go node exits first,
    /// or when two minutes pass. Its error carries the tail of every node's log.
    ///
    /// # Errors
    ///
    /// Fails when `ready` does not hold in time, or a Go node exits.
    pub async fn wait_for<F, Fut>(&mut self, what: &str, mut ready: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if ready().await {
                return Ok(());
            }
            if let Some(name) = self.exited() {
                return Err(self.failure(&format!("{name} exited while waiting for {what}")));
            }
            if Instant::now() >= deadline {
                return Err(self.failure(&format!("timed out waiting for {what}")));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Stops every Go node this run started and waits for each to exit.
    pub fn stop(&mut self) {
        for (_, child) in &mut self.processes {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.processes.clear();
    }

    /// The `--ssl-*` flags of a Go node, when the run uses TLS.
    fn tls_flags(&self) -> Vec<String> {
        self.tls.as_ref().map_or_else(Vec::new, |files| {
            flags(&[
                ("ssl-ca", path(&files.ca)),
                ("ssl-cert", path(&files.cert)),
                ("ssl-key", path(&files.key)),
            ])
        })
    }

    /// Starts one Go node with `args`, with its output in `<name>.log` in the
    /// run's directory.
    fn spawn(&mut self, name: &'static str, args: Vec<String>) -> Result<()> {
        let log = File::create(self.dir.path().join(format!("{name}.log"))).map_err(fail)?;
        let stderr = log.try_clone().map_err(fail)?;
        let child = Command::new(self.build.binary(name))
            .args(&args)
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(stderr)
            .spawn()
            .map_err(|e| fail(format!("start {name}: {e}")))?;
        self.processes.push((name, child));
        Ok(())
    }

    /// The name of a Go node that has exited, if one has.
    fn exited(&mut self) -> Option<&'static str> {
        self.processes
            .iter_mut()
            .find_map(|(name, child)| match child.try_wait() {
                Ok(Some(_)) => Some(*name),
                _ => None,
            })
    }

    /// An error with `message` and the tail of every node's log.
    fn failure(&self, message: &str) -> Error {
        fail(format!("{message}\n{}", self.logs()))
    }

    /// The last lines of every node's log.
    fn logs(&self) -> String {
        let mut out = String::new();
        for (name, _) in &self.processes {
            let text =
                fs::read_to_string(self.dir.path().join(format!("{name}.log"))).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let _ = writeln!(out, "--- {name} ---");
            for line in &lines[lines.len().saturating_sub(20)..] {
                let _ = writeln!(out, "{line}");
            }
        }
        out
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A Rust worker running in this process. [`RustWorker::stop`] shuts it down and
/// waits for its serve loop to end. Dropping it shuts it down without waiting.
pub struct RustWorker {
    worker: Arc<Worker>,
    task: Option<JoinHandle<std::result::Result<(), gorse_worker::Error>>>,
}

/// The `tracing` output of this test process, which the Rust workers log to.
type Captured = Arc<Mutex<Vec<u8>>>;

/// The process's `tracing` output, kept in memory. The subscriber is installed
/// once and never replaced: swapping subscribers while spans are open makes
/// `tracing-subscriber` panic when an event reaches a span that the new one did
/// not record. The output of every test in the process goes to the same buffer.
fn captured() -> &'static Captured {
    static CAPTURED: OnceLock<Captured> = OnceLock::new();
    CAPTURED.get_or_init(|| {
        let buffer = Captured::default();
        let writer = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || Capture(Arc::clone(&writer)))
            .finish();
        // Another subscriber may already be set; then the lines are not kept.
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    })
}

/// The writer of [`captured`]: every write is appended to the buffer.
struct Capture(Captured);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every `tracing` line that the Rust workers of this process have written.
pub fn worker_logs() -> String {
    let buffer = captured().lock().unwrap_or_else(PoisonError::into_inner);
    String::from_utf8_lossy(&buffer).into_owned()
}

/// The last lines of [`worker_logs`], for a failure message.
fn captured_tail() -> String {
    let text = worker_logs();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(60)..].join("\n")
}

impl RustWorker {
    /// Shuts the worker down and waits until its serve loop returns.
    ///
    /// # Errors
    ///
    /// Fails when the serve loop returns an error or panicked.
    pub async fn stop(mut self) -> Result<()> {
        self.worker.shutdown();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await.map_err(fail)?.map_err(fail)
    }
}

impl Drop for RustWorker {
    fn drop(&mut self) {
        self.worker.shutdown();
    }
}

/// A Rust master running in this process. [`RustMaster::stop`] shuts its gRPC
/// server, its background tasks and its stores down.
pub struct RustMaster {
    master: Arc<Master>,
}

impl RustMaster {
    /// The master's REST router, which the REST test serves over HTTP.
    pub fn router(&self) -> axum::Router {
        gorse_master::rest::router(Arc::clone(&self.master))
    }

    /// Shuts the master down and waits for its background tasks to end.
    pub async fn stop(self) {
        self.master.shutdown().await;
    }
}

/// Picks a TCP port that is free now. Another process can take it before a node
/// binds it; a run that loses that race fails when the node does not start.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(fail)?;
    Ok(listener.local_addr().map_err(fail)?.port())
}

/// Whether something accepts connections on `port`.
async fn port_open(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).await.is_ok()
}

/// Whether `GET /api/health/ready` on `port` answers 200.
async fn http_ready(port: u16) -> bool {
    http_status(port, "/api/health/ready").await == Some(200)
}

/// The status code `GET path` answers on `port`, or `None` when the request
/// fails or takes longer than [`REQUEST_TIMEOUT`].
async fn http_status(port: u16, path: &str) -> Option<u16> {
    tokio::time::timeout(REQUEST_TIMEOUT, request_status(port, path))
        .await
        .ok()
        .flatten()
}

/// The status code of one `GET path` request on `port`.
async fn request_status(port: u16, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;
    response.split_whitespace().nth(1)?.parse().ok()
}

/// The command-line flags `--name value` of `pairs`, in order.
fn flags(pairs: &[(&str, String)]) -> Vec<String> {
    pairs
        .iter()
        .flat_map(|(name, value)| [format!("--{name}"), value.clone()])
        .collect()
}

/// Writes a CA and one node certificate into `dir`, and returns their paths. The
/// node certificate allows server and client authentication, so the same files
/// serve the master and every node that dials it. Its names are `localhost` and
/// `127.0.0.1`. Neither side checks host names, as Go's `advancedtls` does, so
/// the names only keep the certificate well formed.
fn write_certificates(dir: &Path) -> Result<TlsFiles> {
    let ca_key = KeyPair::generate().map_err(fail)?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).map_err(fail)?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "gorse conformance ca");
    let ca_cert = ca_params.self_signed(&ca_key).map_err(fail)?;
    let issuer = Issuer::from_params(&ca_params, &ca_key);

    let node_key = KeyPair::generate().map_err(fail)?;
    let mut node_params =
        CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])
            .map_err(fail)?;
    node_params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    node_params
        .distinguished_name
        .push(DnType::CommonName, "gorse conformance node");
    let node_cert = node_params.signed_by(&node_key, &issuer).map_err(fail)?;

    let files = TlsFiles {
        ca: dir.join("ca.pem"),
        cert: dir.join("node.pem"),
        key: dir.join("node-key.pem"),
    };
    fs::write(&files.ca, ca_cert.pem()).map_err(fail)?;
    fs::write(&files.cert, node_cert.pem()).map_err(fail)?;
    fs::write(&files.key, node_key.serialize_pem()).map_err(fail)?;
    Ok(files)
}

/// A path as the text a flag takes.
fn path(path: &Path) -> String {
    path.display().to_string()
}

/// A duration in the form the Go flags take, such as `500ms`.
fn go_duration(duration: Duration) -> String {
    format!("{}ms", duration.as_millis())
}

/// A TOML basic string of `text`.
fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Sets `key` in `[section]` of a TOML document, in place. The key's line is
/// replaced when the section has the key. Otherwise the key is added at the end
/// of the section, and a section the document lacks is appended. Only the named
/// section changes, so a key that several sections share, such as `type`, is set
/// in that one. A multi-line value cannot be replaced: only its first line is.
fn set_key(document: &mut String, section: &str, key: &str, value: &str) {
    let mut lines: Vec<String> = document.lines().map(str::to_owned).collect();
    let mut current = String::new();
    let mut in_multiline = false;
    let mut replace = None;
    let mut insert_at = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if in_multiline {
            in_multiline = !toggles_multiline(trimmed);
            continue;
        }
        if trimmed.starts_with('[') {
            if current == section {
                insert_at = Some(index);
            }
            current = trimmed.trim_matches(['[', ']']).trim().to_owned();
            continue;
        }
        if current == section {
            insert_at = Some(index + 1);
            let is_key = trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key);
            if replace.is_none() && is_key {
                replace = Some(index);
            }
        }
        in_multiline = toggles_multiline(trimmed);
    }
    if current == section {
        insert_at = Some(lines.len());
    }
    let line = format!("{key} = {value}");
    if let Some(index) = replace {
        lines[index] = line;
    } else if let Some(index) = insert_at {
        lines.insert(index, line);
    } else {
        lines.push(String::new());
        lines.push(format!("[{section}]"));
        lines.push(line);
    }
    *document = join(&lines);
}

/// Whether a line opens or closes a multi-line string, which is an odd number of
/// triple quotes.
fn toggles_multiline(line: &str) -> bool {
    line.matches("\"\"\"").count() % 2 == 1 || line.matches("'''").count() % 2 == 1
}

/// Joins lines with newlines, and ends the text with one.
fn join(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_key_replaces_only_the_named_section() {
        let mut document = "[a]\ntype = \"x\"\n[b]\ntype = \"y\"\n".to_owned();
        set_key(&mut document, "b", "type", "\"z\"");
        assert_eq!(document, "[a]\ntype = \"x\"\n[b]\ntype = \"z\"\n");
    }

    #[test]
    fn set_key_adds_a_missing_key_to_its_section() {
        let mut document = "[a]\nx = 1\n[b]\ny = 2\n".to_owned();
        set_key(&mut document, "a", "z", "3");
        assert_eq!(document, "[a]\nx = 1\nz = 3\n[b]\ny = 2\n");
    }

    #[test]
    fn set_key_adds_a_key_to_the_last_section_of_the_document() {
        let mut document = "[a]\nx = 1\n[b]\ny = 2".to_owned();
        set_key(&mut document, "b", "z", "3");
        assert_eq!(document, "[a]\nx = 1\n[b]\ny = 2\nz = 3\n");
    }

    #[test]
    fn set_key_appends_a_missing_section() {
        let mut document = "[a]\nx = 1\n".to_owned();
        set_key(&mut document, "b", "y", "2");
        assert_eq!(document, "[a]\nx = 1\n\n[b]\ny = 2\n");
    }

    #[test]
    fn set_key_skips_the_inside_of_multiline_strings() {
        let mut document = "[a]\nq = \"\"\"\n[b]\ntype = 1\n\"\"\"\n[b]\ntype = 2\n".to_owned();
        set_key(&mut document, "b", "type", "3");
        assert_eq!(
            document,
            "[a]\nq = \"\"\"\n[b]\ntype = 1\n\"\"\"\n[b]\ntype = 3\n"
        );
    }

    #[test]
    fn toml_string_escapes_quotes_and_backslashes() {
        assert_eq!(toml_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn go_duration_uses_milliseconds() {
        assert_eq!(go_duration(Duration::from_millis(500)), "500ms");
    }

    #[test]
    fn flags_pair_each_name_with_its_value() {
        let args = flags(&[("master-port", "8086".to_owned()), ("jobs", "1".to_owned())]);
        assert_eq!(args, ["--master-port", "8086", "--jobs", "1"]);
    }

    #[test]
    fn the_contract_config_has_every_key_the_harness_sets() {
        let base = fs::read_to_string(repo_root().join("contracts/config/config.toml"))
            .expect("read the config contract");
        let keys = [
            ("database", "cache_store"),
            ("database", "data_store"),
            ("database", "vector_store"),
            ("master", "host"),
            ("master", "port"),
            ("master", "http_host"),
            ("master", "http_port"),
            ("blob", "uri"),
            ("recommend.collaborative", "type"),
            ("recommend.ranker", "type"),
            ("recommend.ranker", "recommenders"),
            ("recommend.fallback", "recommenders"),
        ];
        for (section, key) in keys {
            let mut edited = base.clone();
            set_key(&mut edited, section, key, "\"probe\"");
            assert_eq!(
                edited.lines().count(),
                base.lines().count(),
                "[{section}] {key} is not in the contract config, so setting it adds a line"
            );
        }
    }
}
