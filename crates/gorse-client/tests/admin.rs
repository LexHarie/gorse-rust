// Ports: client/admin_test.go @ add21b5 (the unit part: TestGetCategories, TestGetTimeseries, TestGetUser)

//! The admin client against a master that runs in this process, on sqlite
//! stores in a temporary directory. The master serves its own router over HTTP,
//! as the Go test's `master.Serve` does.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gorse_client::AdminClient;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Month};
use gorse_config::config::defaults::get_default_config;
use gorse_master::Master;
use gorse_storage::cache::{self, Score, TimeSeriesPoint};
use gorse_storage::data::types::User;

/// The admin API key the test master checks.
const ADMIN_API_KEY: &str = "secret";

/// A master in this process, the endpoint it serves, and a client for it.
struct TestMaster {
    master: Arc<Master>,
    client: AdminClient,
    _dir: tempfile::TempDir,
}

/// A port that is free now, as Go's `freePort` picks one.
fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a free port");
    listener.local_addr().expect("the local address").port()
}

/// Starts a master over sqlite stores in a temporary directory, serves its
/// router, and waits until `/api/health/live` answers 200.
async fn start_master() -> TestMaster {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().display().to_string();
    let mut cfg = get_default_config();
    cfg.database.data_store = format!("sqlite://{path}/data.db");
    cfg.database.cache_store = format!("sqlite://{path}/cache.db");
    cfg.database.vector_store = format!("xvec://{path}/vectors");
    cfg.blob.uri = format!("{path}/blob");
    cfg.master.host = "127.0.0.1".to_owned();
    cfg.master.port = i64::from(free_port());
    cfg.master.http_host = "127.0.0.1".to_owned();
    cfg.master.http_port = i64::from(free_port());
    cfg.master.admin_api_key = ADMIN_API_KEY.to_owned();
    cfg.openai.auth_token = "test".to_owned();
    let http_port = cfg.master.http_port;

    let master = Master::new(cfg, &path, true, "");
    master.serve().await.expect("the master starts");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port as u16))
        .await
        .expect("bind the HTTP port");
    let router = gorse_master::rest::router(Arc::clone(&master));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let endpoint = format!("http://127.0.0.1:{http_port}");
    wait_for_master(&endpoint).await;
    let client = AdminClient::new(&endpoint, ADMIN_API_KEY);
    TestMaster {
        master,
        client,
        _dir: dir,
    }
}

/// Polls `/api/health/live` for ten seconds, as Go's `waitForMaster` does.
async fn wait_for_master(endpoint: &str) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("an HTTP client");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(response) = http.get(format!("{endpoint}/api/health/live")).send().await {
            if response.status() == reqwest::StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("master didn't become ready at {endpoint}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_categories_matches_go() {
    let test = start_master().await;
    let ctx = Context::background();
    test.master
        .cache()
        .add_scores(
            &ctx,
            cache::ITEM_CATEGORIES,
            "",
            &[score("news", 2.0), score("tech", 1.0)],
        )
        .await
        .expect("the categories are stored");

    let categories = test.client.get_categories().await.expect("the categories");
    assert_eq!(categories, vec!["news".to_owned(), "tech".to_owned()]);
    test.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_timeseries_matches_go() {
    let test = start_master().await;
    let ctx = Context::background();
    let timestamp = gotime::date(2026, Month::JANUARY, 1, 1, 0, 0, 0, &gotime::utc());
    test.master
        .cache()
        .add_time_series_points(
            &ctx,
            &[TimeSeriesPoint {
                name: "requests".to_owned(),
                timestamp,
                value: 1.0,
            }],
        )
        .await
        .expect("the point is stored");

    let points = test
        .client
        .get_timeseries(
            "requests",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            "1h",
        )
        .await
        .expect("the series");
    assert_eq!(points[0].name, "requests");
    assert_eq!(points[0].timestamp, "2026-01-01T01:00:00Z");
    assert_eq!(points[0].value, 1.0);
    test.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_user_matches_go() {
    let test = start_master().await;
    let ctx = Context::background();
    test.master
        .data()
        .batch_insert_users(
            &ctx,
            &[User {
                user_id: "alice".to_owned(),
                ..User::default()
            }],
        )
        .await
        .expect("the user is stored");

    let user = test.client.get_user("alice").await.expect("the user");
    assert_eq!(user.user_id, "alice");
    test.master.shutdown().await;
}

/// A score with no categories and the zero timestamp.
fn score(id: &str, score: f64) -> Score {
    Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: Vec::new(),
        timestamp: gotime::Time::default(),
    }
}
