// Ports: cmd/gorse-in-one/main.go (`setup`, the playground branch) @ add21b5

//! The playground import: `gorse-in-one --playground` downloads a dump, restores
//! it, and serves it. The CDN is blocked in this environment (O-1), so both
//! tests serve the dumps from a local server, from the project's dataset folder.
//! They import through `restore_playground`, the function the playground uses,
//! and check the counts and that the node serves the imported data.

use std::io::Read;

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
async fn playground_ml_100k_imports_the_dataset_and_serves() {
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
