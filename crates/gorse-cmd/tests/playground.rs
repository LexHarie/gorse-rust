// Ports: cmd/gorse-in-one/main.go (`setup`, the playground branch) @ add21b5

//! The playground import: `gorse-in-one --playground=ml-100k` writes its config,
//! downloads the dump from `cdn.gorse.io`, restores it, and prints the welcome
//! text. The download host is blocked in this environment (O-1), so the ml-100k
//! test is ignored. It does not pass by being skipped: run it with `--ignored`
//! where the dataset can be reached.
//!
//! The GitHub playground test runs here. It serves the GitHub dump from a local
//! server, imports it through the same function the playground uses, and checks
//! the counts and that the node serves the imported data.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

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
    let app = Router::new().route(
        "/github.bin.gz",
        get(move || {
            let dump = dump.clone();
            async move { dump }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a local port for the dump server");
    let address = listener.local_addr().expect("a bound address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}/")
}
