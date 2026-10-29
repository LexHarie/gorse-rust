// Ports: the fixtures of worker/worker_test.go and worker/pipeline_test.go @ add21b5

//! Fixtures shared by the worker and pipeline tests.
//!
//! Go's suites open one set of SQLite and `xvec` stores in `SetupSuite` and
//! purge them in `SetupTest`. Each test here opens its own temporary stores,
//! so no test sees another's rows. The expectations are Go's.

use std::future::Future;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use gorse_common::expression::{FeedbackTypeExpression, must_parse_feedback_type_expression};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::monitor;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use gorse_storage::data::{self};
use gorse_storage::vectors::{self, Distance, VectorConfig};

use crate::pipeline::Pipeline;

/// The stores one test reads and writes. The directory keeps the SQLite files
/// and the vector store alive for the test.
pub(crate) struct Fixture {
    _dir: tempfile::TempDir,
    pub(crate) data: Arc<dyn data::Database>,
    pub(crate) cache: Arc<dyn cache::Database>,
    pub(crate) vectors: Arc<dyn vectors::Database>,
}

/// Opens and initialises a fresh data, cache and vector store, and adds the
/// collaborative filtering collection the suite's `SetupTest` adds, with its
/// two dimensions.
pub(crate) async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut data_registry = data::Registry::default();
    data::sql::register(&mut data_registry);
    let data: Arc<dyn data::Database> = Arc::from(
        data_registry
            .open(
                &format!("sqlite://{}/data.db", dir.path().display()),
                "",
                &[],
            )
            .await
            .expect("open the data store"),
    );
    let mut cache_registry = cache::Registry::default();
    cache::sql::register(&mut cache_registry);
    let cache: Arc<dyn cache::Database> = Arc::from(
        cache_registry
            .open(
                &format!("sqlite://{}/cache.db", dir.path().display()),
                "",
                &[],
            )
            .await
            .expect("open the cache store"),
    );
    let vectors: Arc<dyn vectors::Database> = Arc::from(
        vectors::open(&format!("xvec://{}/vectors", dir.path().display()), "", &[])
            .await
            .expect("open the vector store"),
    );
    let ctx = Context::background();
    data.init().await.expect("init the data store");
    cache.init().await.expect("init the cache store");
    vectors.init().await.expect("init the vector store");
    vectors
        .add_collection(
            &ctx,
            &vectors::collaborative_filtering_collection(1),
            2,
            Distance::Dot,
            VectorConfig::default(),
        )
        .await
        .expect("add the collaborative filtering collection");
    Fixture {
        _dir: dir,
        data,
        cache,
        vectors,
    }
}

/// Go's `SetupTest` configuration: the matrix factorisation collaborative
/// filter, and the latest recommender as the fallback.
pub(crate) fn default_config() -> Config {
    let mut config = gorse_config::config::get_default_config();
    config.recommend.collaborative.kind = "mf".to_owned();
    config.recommend.fallback.recommenders = Some(vec!["latest".to_owned()]);
    config
}

/// A pipeline on the fixture's stores with one job, cold-start users kept, and
/// the matrix factorisation model at version 1, as Go's `SetupTest` sets it.
pub(crate) fn new_pipeline(fixture: &Fixture, config: Config) -> Pipeline {
    let mut pipeline = Pipeline::new(
        config,
        Arc::clone(&fixture.cache),
        Arc::clone(&fixture.data),
        Arc::clone(&fixture.vectors),
        monitor::new_tracer("test"),
        1,
    );
    pipeline.dont_skip_cold_start_users = true;
    pipeline.set_matrix_factorization(1, None);
    pipeline
}

/// Go's `storage.ErrNoDatabase` user stores are the no-database stores; this
/// returns the fixture's pipeline with its stores replaced by them.
pub(crate) fn no_database_pipeline() -> Arc<Pipeline> {
    let config = default_config();
    Arc::new(Pipeline::new(
        config,
        Arc::new(cache::no_database::NoDatabase),
        Arc::new(data::no_database::NoDatabase),
        Arc::new(vectors::NoDatabase),
        monitor::new_tracer("test"),
        1,
    ))
}

/// A user with the given id and no labels.
pub(crate) fn user(id: &str) -> User {
    User {
        user_id: id.to_owned(),
        ..Default::default()
    }
}

/// An item with the given id and no categories.
pub(crate) fn item(id: &str) -> Item {
    Item {
        item_id: id.to_owned(),
        ..Default::default()
    }
}

/// An item with the given id, timestamp and categories.
pub(crate) fn item_with(id: &str, categories: &[&str], timestamp: Time) -> Item {
    Item {
        item_id: id.to_owned(),
        categories: Some(categories.iter().map(|c| (*c).to_owned()).collect()),
        timestamp,
        ..Default::default()
    }
}

/// A feedback of the given type from the user on the item, at the zero time.
pub(crate) fn feedback(feedback_type: &str, user_id: &str, item_id: &str) -> Feedback {
    Feedback {
        feedback_key: FeedbackKey {
            feedback_type: feedback_type.to_owned(),
            user_id: user_id.to_owned(),
            item_id: item_id.to_owned(),
        },
        ..Default::default()
    }
}

/// A feedback of the given type from the user on the item, at `timestamp`.
pub(crate) fn feedback_at(
    feedback_type: &str,
    user_id: &str,
    item_id: &str,
    timestamp: Time,
) -> Feedback {
    Feedback {
        timestamp,
        ..feedback(feedback_type, user_id, item_id)
    }
}

/// Go's `expression.MustParseFeedbackTypeExpression` over a list of names.
pub(crate) fn expressions(names: &[&str]) -> Vec<FeedbackTypeExpression> {
    names
        .iter()
        .map(|name| must_parse_feedback_type_expression(name))
        .collect()
}

/// A score of the given id, score and categories, at the zero time.
pub(crate) fn score(id: &str, score: f64, categories: &[&str]) -> cache::Score {
    cache::Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        timestamp: Time::default(),
    }
}

/// The time `hours` hours from now (negative for the past), as Go's
/// `time.Now().Add(d)`.
pub(crate) fn hours_from_now(hours: i64) -> Time {
    gotime::now().add(gotime::Duration(hours * gotime::HOUR.0))
}

/// Runs a future to completion on a blocking-pool thread, where the blocking
/// rankers may run. The rankers call their HTTP APIs with a blocking client,
/// as Go's do, so a test must not call them on a runtime worker.
pub(crate) async fn on_worker_thread<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || tokio::runtime::Handle::current().block_on(future))
        .await
        .expect("the blocking test thread did not panic")
}

/// A request a [`ScriptedServer`] received.
pub(crate) struct Received {
    /// The request body, as sent.
    pub(crate) body: String,
}

/// A one-shot HTTP server that answers the next request with a fixed body and
/// sends the request body back to the test (the `httptest.NewServer` of Go's
/// `TestRankByDecision`).
pub(crate) struct ScriptedServer {
    /// The base URL of the server.
    pub(crate) url: String,
    /// The request bodies the server received, in order.
    pub(crate) received: Receiver<Received>,
}

impl ScriptedServer {
    /// Starts a server on a local port that answers each request with `reply`.
    pub(crate) fn start(reply: &'static str) -> ScriptedServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        let (sender, received) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let body = read_request_body(&stream);
                if sender.send(Received { body }).is_err() {
                    return;
                }
                write_response(stream, reply);
            }
        });
        ScriptedServer { url, received }
    }
}

/// Reads the headers and the `Content-Length` body of one HTTP request.
fn read_request_body(stream: &TcpStream) -> String {
    let mut reader = std::io::BufReader::new(stream);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
            break;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    String::from_utf8_lossy(&body).into_owned()
}

/// Writes a `200 OK` JSON response and closes the connection.
fn write_response(mut stream: TcpStream, reply: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.len(),
        reply
    );
    let _ = stream.flush();
}
