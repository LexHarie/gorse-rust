// Ports: cmd/gorse-in-one/main.go (the playground branch of the command, and `setup`) @ add21b5,
// and the CI job `playground` in .github/workflows/build_test.yml @ add21b5.

//! The playground end to end (WP-5-3). `gorse-in-one --playground=ml-100k` runs once
//! as the Go build and once as the Rust build. Each run writes its config under a
//! temporary `HOME`, downloads and imports the ML-100k dump, prints its welcome
//! block, and then answers the dashboard paths and the users route.
//!
//! The Go CI job checks that the dashboard answers after the import. This test does
//! that for both builds, and compares them: the same config file, the same lines
//! before the server starts, the same status at the same dashboard paths, and the
//! same users after the import.
//!
//! The playground downloads `https://cdn.gorse.io/example/ml-100k.bin.gz`. This
//! environment blocks that host (O-1), and neither binary can be pointed at another
//! host. So each run gets a stand-in for the CDN. The binary reaches it through
//! `HTTPS_PROXY`: the proxy answers `CONNECT cdn.gorse.io:443` with a TLS session,
//! presents a certificate for that host, and serves the project's dataset file over
//! it. The binary trusts the test CA through `SSL_CERT_FILE`. Both binaries run
//! unchanged, and their own download code is what runs.
//!
//! The two builds share one port set, so the runs happen one at a time, and each
//! build runs once per test process.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use gorse_conformance::cluster::{GoBuild, RustBuild};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tempfile::TempDir;

/// The ML-100k playground dump, as the project's dataset folder keeps it.
const ML_100K_DUMP: &str = "/mnt/project-files/datasets/ml-100k.bin.gz";

/// The host and path the playground downloads (`cmd/gorse-in-one/main.go`).
const CDN_HOST: &str = "cdn.gorse.io";
const CDN_ARCHIVE: &str = "/example/ml-100k.bin.gz";

/// The config the ML-100k playground writes (`client/config.toml`). Its `http_port`
/// is 8088, the port the dashboard checks use.
const CLIENT_CONFIG: &str = include_str!("../../client/config.toml");
const HTTP_PORT: u16 = 8088;

/// The dashboard paths. `/` is the CI check. `/overview` and `/apidocs` are the two
/// the welcome block prints, and `/apidocs/` is where `/apidocs` redirects.
const DASHBOARD_PATHS: [&str; 4] = ["/", "/overview", "/apidocs", "/apidocs/"];

/// The users route the playground's ML-100k data answers. Its first three users
/// are the ones the Go SDK suite expects (`client/client_test.go`, `TestUsers`).
const USERS_PATH: &str = "/api/users?n=3";
const FIRST_USERS: [&str; 3] = ["1", "10", "100"];

/// The line that ends the import, and the block the binary prints around it.
const WELCOME: &str = "Welcome to Gorse Playground";
const BLOCK_END: &str = "Documentation: https://gorse.io";

/// How long one binary may take to import the dump and print its welcome block.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(900);

/// How long the HTTP server may take to answer after the welcome block.
const SERVE_TIMEOUT: Duration = Duration::from_secs(120);

/// The playground runs bind the playground's fixed ports, so one run at a time.
static PORTS: Mutex<()> = Mutex::new(());

/// The Go playground run, shared by the tests of this file.
static GO_RUN: OnceLock<Run> = OnceLock::new();

/// The Rust playground run, shared by the tests of this file.
static RUST_RUN: OnceLock<Run> = OnceLock::new();

#[test]
fn playground_ml_100k_go_dashboard_answers() {
    go_run().check("the Go playground");
}

#[test]
fn playground_ml_100k_rust_dashboard_answers() {
    rust_run().check("the Rust playground");
}

#[test]
fn playground_ml_100k_rust_matches_go() {
    let go = go_run();
    let rust = rust_run();
    assert_eq!(
        comparable(&go.printed),
        comparable(&rust.printed),
        "the lines the playground prints before it serves"
    );
    assert_eq!(
        go.config, rust.config,
        "the config file the playground writes"
    );
    assert_eq!(
        go.downloads, rust.downloads,
        "the CDN requests of the import"
    );
    assert_eq!(go.dashboard, rust.dashboard, "the dashboard answers");
    assert_eq!(
        go.dashboard_bodies, rust.dashboard_bodies,
        "the dashboard pages"
    );
    assert_eq!(go.users, rust.users, "the users route after the import");
    assert_eq!(
        go.user_ids, rust.user_ids,
        "the first users after the import"
    );
}

/// The Go playground run, made once per test process.
fn go_run() -> &'static Run {
    GO_RUN.get_or_init(|| {
        let build = GoBuild::shared().expect("the Go build of the playground");
        playground(&build.binary("gorse-in-one"))
    })
}

/// The Rust playground run, made once per test process.
fn rust_run() -> &'static Run {
    RUST_RUN.get_or_init(|| {
        let build = RustBuild::shared().expect("the Rust build of the playground");
        playground(&build.binary("gorse-in-one"))
    })
}

/// What one playground run printed, wrote and answered.
#[derive(Debug)]
struct Run {
    /// The lines the binary printed up to the end of its welcome block, with the
    /// run's home folder written as `<home>`.
    printed: Vec<String>,
    /// The config file the playground wrote, as bytes.
    config: Vec<u8>,
    /// The requests the CDN stand-in served: each target and the status it sent.
    downloads: Vec<(String, u16)>,
    /// The answer of each dashboard path.
    dashboard: Vec<Answer>,
    /// The body of each dashboard path, in the order of `DASHBOARD_PATHS`.
    dashboard_bodies: Vec<String>,
    /// The answer of the users route.
    users: Answer,
    /// The ids of the users the route returned, in order.
    user_ids: Vec<String>,
}

/// One HTTP answer: the path asked, the status, and the content type.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    path: String,
    status: u16,
    content_type: Option<String>,
}

impl Run {
    /// Checks one run on its own, as the Go CI check does: the welcome block and the
    /// config are right, the dataset is downloaded once, the dashboard answers, and
    /// the import reached the users route.
    fn check(&self, label: &str) {
        assert_eq!(
            self.downloads,
            [(CDN_ARCHIVE.to_owned(), 200)],
            "{label} downloads the dump once"
        );
        assert!(
            self.printed.iter().any(|line| line == WELCOME),
            "{label} prints its welcome block: {:?}",
            self.printed
        );
        assert_eq!(
            self.config,
            CLIENT_CONFIG.as_bytes(),
            "{label} writes client/config.toml"
        );
        for path in ["/", "/overview", "/apidocs/"] {
            let answer = self
                .dashboard
                .iter()
                .find(|answer| answer.path == path)
                .unwrap_or_else(|| panic!("{label} has no answer for {path}"));
            assert_eq!(
                answer.status, 200,
                "{label} answers the dashboard at {path}"
            );
        }
        assert_eq!(self.users.status, 200, "{label} answers {USERS_PATH}");
        assert_eq!(
            self.user_ids, FIRST_USERS,
            "{label} has the first users after the import"
        );
    }
}

/// Runs `gorse-in-one --playground=ml-100k` from `binary` and collects its run.
fn playground(binary: &Path) -> Run {
    let _ports = PORTS.lock().unwrap_or_else(PoisonError::into_inner);
    let cdn = Cdn::start();
    let home = TempDir::new().expect("a temporary home");
    let stderr_path = home.path().join("stderr.log");
    let stderr = File::create(&stderr_path).expect("a file for the binary's stderr");
    let child = Command::new(binary)
        .arg("--playground=ml-100k")
        .env("HOME", home.path())
        .env("HTTPS_PROXY", &cdn.proxy)
        .env("https_proxy", &cdn.proxy)
        .env("SSL_CERT_FILE", &cdn.ca)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .unwrap_or_else(|err| panic!("start {}: {err}", binary.display()));
    let mut running = Running { child };
    let stdout = running.child.stdout.take().expect("a piped stdout");
    let lines = lines_of(stdout);
    let home_text = home.path().display().to_string();

    let import_deadline = Instant::now() + IMPORT_TIMEOUT;
    let mut printed = Vec::new();
    loop {
        let wait = import_deadline.saturating_duration_since(Instant::now());
        let line = lines.recv_timeout(wait).unwrap_or_else(|err| {
            panic!(
                "{} printed no welcome block ({err}); its stderr:\n{}",
                binary.display(),
                fs::read_to_string(&stderr_path).unwrap_or_default()
            )
        });
        // A JSON line is a log record, not the playground's output (see `is_log_record`).
        if is_log_record(&line) {
            continue;
        }
        let ends = line.trim_start().starts_with(BLOCK_END);
        printed.push(line.replace(&home_text, "<home>"));
        if ends {
            break;
        }
    }
    // The block ends with a blank line after its last line.
    if let Ok(blank) = lines.recv_timeout(Duration::from_secs(5)) {
        printed.push(blank.replace(&home_text, "<home>"));
    }

    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .expect("an HTTP client");
    wait_until_answering(&client, Instant::now() + SERVE_TIMEOUT);
    let mut dashboard = Vec::new();
    let mut dashboard_bodies = Vec::new();
    for path in DASHBOARD_PATHS {
        let (answer, body) = get(&client, path);
        dashboard.push(answer);
        dashboard_bodies.push(body);
    }
    let (users, users_body) = get(&client, USERS_PATH);
    let user_ids = user_ids(&users_body);
    let config = fs::read(home.path().join(".gorse/etc/config.toml"))
        .expect("the config file the playground wrote");
    let downloads = cdn.served();

    drop(running);
    Run {
        printed,
        config,
        downloads,
        dashboard,
        dashboard_bodies,
        users,
        user_ids,
    }
}

/// A playground process, stopped when the run ends, also when a check panics.
struct Running {
    child: Child,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The lines of a process's stdout, read on a thread, so the run can wait with a
/// timeout.
fn lines_of(stdout: ChildStdout) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

/// Whether a line is a log record. Both builds write their logs as JSON lines on
/// stdout. The Go build logs after its welcome block, and the Rust build logs
/// while its stores open, before the import (`porting/notes/WP-5-3.md`), so the
/// block is compared without them.
fn is_log_record(line: &str) -> bool {
    line.trim_start().starts_with('{')
}

/// Waits until the dashboard's root answers with a success status.
fn wait_until_answering(client: &Client, deadline: Instant) {
    loop {
        if let Ok(response) = client.get(url("/")).send()
            && response.status().is_success()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the dashboard did not answer within {SERVE_TIMEOUT:?}"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

/// The local URL of `path` on the playground's HTTP port.
fn url(path: &str) -> String {
    format!("http://127.0.0.1:{HTTP_PORT}{path}")
}

/// Asks the playground for `path`, and returns the answer and the body.
fn get(client: &Client, path: &str) -> (Answer, String) {
    let response = client
        .get(url(path))
        .send()
        .unwrap_or_else(|err| panic!("request {path}: {err}"));
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .unwrap_or_else(|err| panic!("read the body of {path}: {err}"));
    let answer = Answer {
        path: path.to_owned(),
        status,
        content_type,
    };
    (answer, body)
}

/// The ids of the users in a users response, in order. The response is the
/// master's cursor (`Users`, each with `UserId`), as the Go and Rust masters write it.
fn user_ids(body: &str) -> Vec<String> {
    let value: serde_json::Value =
        serde_json::from_str(body).unwrap_or_else(|err| panic!("decode the users: {err}"));
    value["Users"]
        .as_array()
        .map(|users| {
            users
                .iter()
                .filter_map(|user| user["UserId"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A printed line as the comparison reads it. Go's core line names the CPU brand
/// (`Using 4 CPU cores: Intel(R) ...`, from cpuid), and the Rust line does not. The
/// brand is left out of both lines. The difference is logged in
/// `porting/notes/WP-5-3.md`.
fn comparable(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line.find(" CPU cores") {
            Some(at) if line.starts_with("Using ") => line[..at + " CPU cores".len()].to_owned(),
            _ => line.clone(),
        })
        .collect()
}

/// The stand-in for `cdn.gorse.io`: a loopback proxy, the test CA that signs its
/// certificate, and the record of what it served.
struct Cdn {
    /// The proxy URL to put in `HTTPS_PROXY`.
    proxy: String,
    /// The CA certificate, as a PEM file for `SSL_CERT_FILE`.
    ca: PathBuf,
    /// Every request target the proxy answered, with its status.
    served: Arc<Mutex<Vec<(String, u16)>>>,
    /// The folder that holds the CA file.
    _dir: TempDir,
}

impl Cdn {
    /// Starts the proxy on a free loopback port. It serves the ML-100k dump from the
    /// project's dataset folder, the bytes the CDN serves for that path.
    fn start() -> Cdn {
        let dir = TempDir::new().expect("a temporary folder for the CDN stand-in");
        let (ca_pem, tls) = test_tls();
        let ca = dir.path().join("ca.pem");
        fs::write(&ca, ca_pem).expect("write the test CA");
        let archive = fs::read(ML_100K_DUMP)
            .unwrap_or_else(|err| panic!("read the playground dataset {ML_100K_DUMP}: {err}"));
        let served = Arc::new(Mutex::new(Vec::new()));
        let stand_in = Arc::new(Stand {
            tls,
            archive,
            served: Arc::clone(&served),
        });
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("a loopback port for the CDN stand-in");
        let port = listener
            .local_addr()
            .expect("a bound address for the CDN stand-in")
            .port();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let stand_in = Arc::clone(&stand_in);
                thread::spawn(move || stand_in.answer(stream));
            }
        });
        Cdn {
            proxy: format!("http://127.0.0.1:{port}"),
            ca,
            served,
            _dir: dir,
        }
    }

    /// The requests served so far.
    fn served(&self) -> Vec<(String, u16)> {
        self.served
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What the proxy serves: the TLS identity of `cdn.gorse.io`, the dump, and the record.
struct Stand {
    tls: Arc<ServerConfig>,
    archive: Vec<u8>,
    served: Arc<Mutex<Vec<(String, u16)>>>,
}

impl Stand {
    /// Answers one client connection: a CONNECT for the CDN host, then one request
    /// over the TLS session it opens. Any other CONNECT is refused.
    fn answer(&self, mut stream: TcpStream) {
        let Some(head) = read_head(&mut stream) else {
            return;
        };
        let (method, target) = request_line(&head);
        if method != "CONNECT" || target != format!("{CDN_HOST}:443") {
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
        if stream
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
        {
            return;
        }
        let Ok(conn) = ServerConnection::new(Arc::clone(&self.tls)) else {
            return;
        };
        let mut tls = StreamOwned::new(conn, stream);
        let Some(head) = read_head(&mut tls) else {
            return;
        };
        let (method, target) = request_line(&head);
        let found = method == "GET" && target == CDN_ARCHIVE;
        let (status, reason, body): (u16, &str, &[u8]) = if found {
            (200, "OK", self.archive.as_slice())
        } else {
            (404, "Not Found", &[])
        };
        self.served
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((target.to_owned(), status));
        let reply = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = tls.write_all(reply.as_bytes());
        let _ = tls.write_all(body);
        let _ = tls.flush();
        tls.conn.send_close_notify();
        let _ = tls.flush();
    }
}

/// Reads one HTTP head, up to its blank line, one byte at a time, so no byte past it
/// is taken from the stream.
fn read_head(reader: &mut impl Read) -> Option<String> {
    let mut data = Vec::new();
    let mut byte = [0_u8; 1];
    while !data.ends_with(b"\r\n\r\n") {
        if reader.read(&mut byte).ok()? == 0 {
            return None;
        }
        data.push(byte[0]);
    }
    String::from_utf8(data).ok()
}

/// The method and target of an HTTP request head.
fn request_line(head: &str) -> (&str, &str) {
    let line = head.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

/// A test CA, and a server certificate for the CDN host that it signs. It returns the
/// CA as PEM, and the server configuration that presents the certificate.
fn test_tls() -> (String, Arc<ServerConfig>) {
    let ca_key = KeyPair::generate().expect("a CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "gorse playground cdn stand-in ca");
    let ca_cert = ca_params.self_signed(&ca_key).expect("a CA certificate");
    let issuer = Issuer::from_params(&ca_params, &ca_key);

    let server_key = KeyPair::generate().expect("a server key");
    let mut server_params =
        CertificateParams::new(vec![CDN_HOST.to_owned()]).expect("server parameters");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params
        .distinguished_name
        .push(DnType::CommonName, CDN_HOST);
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("a server certificate");

    let chain = vec![server_cert.der().clone(), ca_cert.der().clone()];
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(server_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default TLS protocol versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("a TLS server configuration");
    (ca_cert.pem(), Arc::new(config))
}
