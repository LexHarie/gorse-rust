// Ports: cmd/gorse-in-one/main.go (`setup`, the playground branch) @ add21b5

//! The playground import: `gorse-in-one --playground` writes its config, downloads
//! a dump, restores it, and prints the welcome text. The download host is
//! `cdn.gorse.io`, which this environment blocks (O-1). The binary has no way to
//! point that download elsewhere: `cmd/gorse-in-one/main.go:151-152` and
//! `crates/gorse-cmd/src/in_one.rs:33` hard-code the URL.
//!
//! So the tests are split by what runs offline:
//! - The in-process tests serve the dumps from a local server, from the project's
//!   dataset folder, and import them through `restore_playground`, the function the
//!   playground uses. They check the counts and that the node serves the data.
//! - The binary config test runs the real `gorse-in-one` and checks the lines and
//!   the config file it writes before it touches the network.
//! - The binary import test runs the real binary through its download and checks
//!   the welcome text. It is ignored here, because the download cannot be served
//!   locally.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::routing::get;
use flate2::read::GzDecoder;
use gorse_cmd::in_one::restore_playground;
use gorse_cmd::master;
use gorse_common::gocontext::Context;
use gorse_config::config::get_default_config;
use gorse_master::Master;
use tempfile::TempDir;
use tokio::net::TcpListener;

/// The GitHub playground dump, as published in the project's dataset folder.
const GITHUB_DUMP: &str = "/mnt/project-files/datasets/github.bin.gz";

/// The MovieLens 100k playground dump, as the project's dataset folder keeps it.
const ML_100K_DUMP: &str = "/mnt/project-files/datasets/ml-100k.bin.gz";

#[tokio::test(flavor = "multi_thread")]
async fn playground_ml_100k_restores_from_a_local_dump_and_serves() {
    // The CDN serves the archive as it is on disk, so the local server does too.
    let archive = std::fs::read(ML_100K_DUMP)
        .unwrap_or_else(|err| panic!("read the playground dataset {ML_100K_DUMP}: {err}"));
    let base_url = serve_file("/ml-100k.bin.gz", Bytes::from(archive)).await;

    let data = TempDir::new().expect("a temporary data folder");
    let mut conf = get_default_config();
    conf.database.data_store = format!("sqlite://{}", data.path().join("data.sqlite").display());
    conf.database.cache_store = format!("sqlite://{}", data.path().join("cache.sqlite").display());
    // Ephemeral gRPC and HTTP ports, so this test never binds the default 8086
    // that the GitHub test's master binds when both run in one process.
    conf.master.host = "127.0.0.1".to_owned();
    conf.master.port = 0;
    conf.master.http_host = "127.0.0.1".to_owned();
    conf.master.http_port = 0;
    let cache_path = data.path().display().to_string();
    let master = Master::new(conf, &cache_path, true, "");
    master.serve().await.expect("start the master");

    restore_playground(&master, &base_url, "ml-100k")
        .await
        .expect("import the ml-100k playground dump");

    // The counts of the dump that client/setup-test.sh restores for the Go SDK suite.
    let ctx = Context::background();
    let store = master.data();
    assert_eq!(store.count_users(&ctx).await.expect("count users"), 943);
    assert_eq!(store.count_items(&ctx).await.expect("count items"), 1_682);
    assert_eq!(
        store.count_feedback(&ctx).await.expect("count feedback"),
        100_000
    );

    let listener = master::bind_http(&master).await;
    let address = listener.local_addr().expect("a bound address");
    let server = tokio::spawn(master::serve_http(master.clone(), listener));
    // The first three users, in the order the Go SDK suite expects (client/client_test.go, TestUsers).
    let cursor: UserCursor = reqwest::get(format!("http://{address}/api/users?n=3"))
        .await
        .expect("query the node")
        .error_for_status()
        .expect("the users route answers 200")
        .json()
        .await
        .expect("decode the users");
    let ids: Vec<&str> = cursor
        .users
        .iter()
        .map(|user| user.user_id.as_str())
        .collect();
    assert_eq!(ids, ["1", "10", "100"]);
    server.abort();
    master.shutdown().await;
}

/// The real `gorse-in-one --playground=ml-100k` writes its config and prints where
/// it wrote it, before its download. The test stops the binary once it has printed
/// the cache directory, so the download is never reached and no network is needed.
#[test]
fn playground_ml_100k_binary_writes_its_config() {
    let home = TempDir::new().expect("a temporary home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gorse-in-one"))
        .arg("--playground=ml-100k")
        .env("HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start gorse-in-one");
    let stdout = child.stdout.take().expect("a piped stdout");
    let (lines_tx, lines_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut printed = Vec::new();
    while let Ok(line) = lines_rx.recv_timeout(Duration::from_secs(60)) {
        let done = line.starts_with("Using cache directory:");
        printed.push(line);
        if done {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let config = home.path().join(".gorse/etc/config.toml");
    let generated = format!("Generated config file: {}", config.display());
    assert!(
        printed.contains(&generated),
        "the playground did not write its config: {printed:?}"
    );
    assert!(
        printed
            .iter()
            .any(|line| line.starts_with("Using cache directory:")),
        "the playground did not print its cache directory: {printed:?}"
    );
    // The ml-100k playground writes the client config (Go's `client.ConfigTOML`).
    let written = std::fs::read_to_string(&config).expect("read the playground config");
    assert_eq!(written, include_str!("../../../client/config.toml"));
}

/// The real `gorse-in-one --playground=ml-100k`, through its download and the
/// welcome text. It needs `cdn.gorse.io`, which this environment blocks (O-1), and
/// the binary cannot be pointed at a local server: `cmd/gorse-in-one/main.go:151-152`
/// and `crates/gorse-cmd/src/in_one.rs:33` hard-code the URL. It does not pass by
/// being skipped: run it with `--ignored` where the CDN can be reached.
#[test]
#[ignore = "O-1: dataset from cdn.gorse.io"]
fn playground_ml_100k_imports_the_dataset_and_serves() {
    let home = TempDir::new().expect("a temporary home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gorse-in-one"))
        .arg("--playground=ml-100k")
        .env("HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start gorse-in-one");
    let stdout = child.stdout.take().expect("a piped stdout");
    // The welcome text comes after the import, and the node serves from then on.
    let welcomed = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("Welcome to Gorse Playground"));
    let _ = child.kill();
    let _ = child.wait();
    assert!(welcomed, "the playground did not finish its import");
    assert!(home.path().join(".gorse/etc/config.toml").is_file());
}

/// The part of the master's users response that the ml-100k test reads.
#[derive(serde::Deserialize)]
struct UserCursor {
    #[serde(rename = "Users")]
    users: Vec<UserRow>,
}

/// One user of the master's users response, by its id.
#[derive(serde::Deserialize)]
struct UserRow {
    #[serde(rename = "UserId")]
    user_id: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn playground_github_restores_the_dump_and_serves() {
    // The CDN serves the archive as it is on disk, so the local server does too.
    let archive = std::fs::read(GITHUB_DUMP)
        .unwrap_or_else(|err| panic!("read the playground dump {GITHUB_DUMP}: {err}"));
    let mut dump = Vec::new();
    GzDecoder::new(&archive[..])
        .read_to_end(&mut dump)
        .expect("decompress the playground dump");
    assert_eq!(
        feedback_records(&dump),
        357_294,
        "feedback records in the dump"
    );
    let base_url = serve_dump(Bytes::from(archive)).await;

    let data = TempDir::new().expect("a temporary data folder");
    let mut conf = get_default_config();
    conf.database.data_store = format!("sqlite://{}", data.path().join("data.sqlite").display());
    conf.database.cache_store = format!("sqlite://{}", data.path().join("cache.sqlite").display());
    conf.master.http_host = "127.0.0.1".to_owned();
    conf.master.http_port = 0;
    let cache_path = data.path().display().to_string();
    let master = Master::new(conf, &cache_path, true, "");
    master.serve().await.expect("start the master");

    restore_playground(&master, &base_url, "default")
        .await
        .expect("import the GitHub playground dump");

    let ctx = Context::background();
    let store = master.data();
    assert_eq!(store.count_users(&ctx).await.expect("count users"), 2_079);
    assert_eq!(store.count_items(&ctx).await.expect("count items"), 22_320);
    // 25,393 of the dump's feedback records name a user that the dump does not
    // hold. The restore writes feedback without inserting users (Go's
    // `BatchInsertFeedback(ctx, feedback, false, false, true)`), so those rows
    // are dropped, as Go drops them.
    assert_eq!(
        store.count_feedback(&ctx).await.expect("count feedback"),
        331_901
    );

    let listener = master::bind_http(&master).await;
    let address = listener.local_addr().expect("a bound address");
    let server = tokio::spawn(master::serve_http(master.clone(), listener));
    let response = reqwest::get(format!("http://{address}/api/users?n=1"))
        .await
        .expect("query the node");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    server.abort();
    master.shutdown().await;
}

/// The number of feedback records in `dump`, read from its framing. A stream
/// starts with its flag (-1 users, -2 items, -3 feedback). Each record is its
/// length, then its bytes. A zero flag ends the dump.
fn feedback_records(dump: &[u8]) -> usize {
    let mut rest = dump;
    let mut stream = 0;
    let mut count = 0;
    loop {
        let (flag, tail) = rest.split_at(8);
        let flag = i64::from_le_bytes(flag.try_into().expect("eight bytes"));
        rest = tail;
        match flag {
            0 => return count,
            -3..=-1 => stream = flag,
            size => {
                let size = usize::try_from(size).expect("a positive record length");
                rest = &rest[size..];
                if stream == -3 {
                    count += 1;
                }
            }
        }
    }
}

/// Serves `dump` as the GitHub playground file on a local port, and returns the
/// base URL the playground import takes, with its trailing slash.
async fn serve_dump(dump: Bytes) -> String {
    serve_file("/github.bin.gz", dump).await
}

/// Serves `body` at `route` on a local port, and returns the base URL the
/// playground import takes, with its trailing slash.
async fn serve_file(route: &'static str, body: Bytes) -> String {
    let app = Router::new().route(
        route,
        get(move || {
            let body = body.clone();
            async move { body }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port for the dump server");
    let address = listener.local_addr().expect("a bound address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}/")
}
