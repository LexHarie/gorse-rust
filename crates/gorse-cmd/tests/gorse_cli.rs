// Ports: cmd/gorse-cli/main_test.go @ add21b5 (CLITestSuite: SetupTest and each test case)

//! The `gorse-cli` commands against a master that runs in this process. Each
//! test starts the master and seeds it as Go's `SetupTest` does, runs the
//! command, and checks the output as Go's test does: line by line against
//! regular expressions, or exactly.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gorse_client::AdminClient;
use gorse_cmd::cli_context::MemoryKeyring;
use gorse_cmd::gorse_cli::{Environment, execute};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Month};
use gorse_config::config::defaults::get_default_config;
use gorse_config::config::schema::{ItemToItemConfig, SearchConfig, UserToUserConfig};
use gorse_master::Master;
use gorse_protocol::master_server::Master as MasterRpc;
use gorse_protocol::{NodeInfo, NodeType};
use gorse_storage::cache::{self, Score, TimeSeriesPoint};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels, User};
use gorse_storage::vectors::{self, Distance, Vector, VectorConfig};
use regex::Regex;
use tonic::Request;
use yaml_rust2::{Yaml, YamlLoader};

/// The admin API key the test master checks, and that the commands are given.
const API_KEY: &str = "secret";

/// Go's `rfc3339Regexp`. Patterns write it as `<rfc3339>`, which is replaced
/// before the pattern is compiled.
const RFC3339: &str = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})";

/// A master in this process, the endpoint it serves, and the directory its
/// stores are in, which lives as long as the suite.
struct Suite {
    master: Arc<Master>,
    endpoint: String,
    _dir: tempfile::TempDir,
}

impl Suite {
    /// Runs `gorse-cli` with `args`, and the test endpoint and key appended, as
    /// Go's `execute` does.
    async fn execute(&self, args: &[&str]) -> Result<String, String> {
        self.execute_with_input("", args).await
    }

    /// As [`Suite::execute`], with `input` as stdin.
    async fn execute_with_input(&self, input: &str, args: &[&str]) -> Result<String, String> {
        let mut argv = to_argv(args);
        argv.extend([
            "--endpoint".to_owned(),
            self.endpoint.clone(),
            "--api-key".to_owned(),
            API_KEY.to_owned(),
        ]);
        run(input, &argv).await
    }
}

/// Runs `gorse-cli` with `args` and no endpoint or key, as Go's
/// `executeRawCommand` does for the commands that fail before they connect.
async fn execute_raw(args: &[&str]) -> Result<String, String> {
    run("", &to_argv(args)).await
}

/// Runs `gorse-cli` in process with `input` as stdin. The result is the output
/// when the command succeeds. When it fails, the result is the output with the
/// failure's text after it, since cobra writes its errors to the same buffer
/// that Go's test reads.
async fn run(input: &str, argv: &[String]) -> Result<String, String> {
    let env = Environment {
        keyring: Box::new(MemoryKeyring::default()),
        endpoint: String::new(),
        api_key: String::new(),
        interactive: false,
    };
    let mut stdin = input.as_bytes();
    let mut out = Vec::new();
    let result = execute(argv, &env, &mut stdin, &mut out).await;
    let mut text = String::from_utf8(out).expect("UTF-8 output");
    match result {
        Ok(()) => Ok(text),
        Err(failure) => {
            text.push_str(&failure.render());
            Err(text)
        }
    }
}

fn to_argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// A port that is free now, as Go's `freePort` picks one.
fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a free port");
    listener.local_addr().expect("the local address").port()
}

/// Polls `/api/health/live` for ten seconds, as Go's `waitForMaster` does.
async fn wait_for_master(endpoint: &str) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("an HTTP client");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(response) = http.get(format!("{endpoint}/api/health/live")).send().await
            && response.status() == reqwest::StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("master didn't become ready at {endpoint}");
}

/// Polls the tasks for ten seconds until `Load Dataset` completes, as Go's
/// `waitForInitialTask` does.
async fn wait_for_initial_task(endpoint: &str) {
    let client = AdminClient::new(endpoint, API_KEY);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(tasks) = client.get_tasks().await {
            for task in tasks.iter().filter(|task| task.name == "Load Dataset") {
                match task.status.as_str() {
                    "Complete" => return,
                    "Failed" => panic!("initial load dataset task failed: {}", task.error),
                    _ => {}
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("initial load dataset task didn't complete");
}

/// Serializes suite setup. A suite picks its ports and binds them afterwards, so
/// two suites set up at once could pick the same port. Go's suite runs its tests
/// one at a time, and setup does the same here.
static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Starts a master over sqlite stores in a temporary directory, as Go's
/// `newTestMaster` does, and then seeds it as `SetupTest` does.
async fn setup() -> Suite {
    let _setup = SETUP.lock().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    // Bound now and kept, so no other port pick can return this port.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind the HTTP port");
    let http_port = listener.local_addr().expect("the local address").port();
    let path = dir.path().display().to_string();
    let mut cfg = get_default_config();
    cfg.database.data_store = format!("sqlite://{path}/data.db");
    cfg.database.cache_store = format!("sqlite://{path}/cache.db");
    cfg.database.vector_store = format!("xvec://{path}/vectors");
    cfg.blob.uri = format!("{path}/blob");
    cfg.master.host = "127.0.0.1".to_owned();
    cfg.master.port = i64::from(free_port());
    cfg.master.http_host = "127.0.0.1".to_owned();
    cfg.master.http_port = i64::from(http_port);
    cfg.master.admin_api_key = API_KEY.to_owned();
    cfg.recommend.search.columns = Some(vec!["item.Comment".to_owned()]);
    cfg.recommend.item_to_item = Some(vec![ItemToItemConfig {
        name: "neighbors".to_owned(),
        kind: "tags".to_owned(),
        column: "item.Labels".to_owned(),
    }]);
    cfg.recommend.user_to_user = Some(vec![UserToUserConfig {
        name: "neighbors".to_owned(),
        kind: "items".to_owned(),
        column: String::new(),
    }]);
    cfg.openai.auth_token = "test".to_owned();

    let master = Master::new(cfg, &path, true, "");
    master.serve().await.expect("the master starts");
    let router = gorse_master::rest::router(Arc::clone(&master));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let endpoint = format!("http://127.0.0.1:{http_port}");
    wait_for_master(&endpoint).await;
    wait_for_initial_task(&endpoint).await;
    seed(&master).await;
    Suite {
        master,
        endpoint,
        _dir: dir,
    }
}

/// The data Go's `SetupTest` writes: the server node, users, items, feedback,
/// cache scores and a time series.
async fn seed(master: &Master) {
    let ctx = Context::background();
    master
        .get_meta(Request::new(NodeInfo {
            node_type: NodeType::Server as i32,
            uuid: "server-node".to_owned(),
            hostname: "localhost".to_owned(),
            binary_version: "v1".to_owned(),
        }))
        .await
        .expect("the server node registers");
    master
        .data()
        .batch_insert_users(
            &ctx,
            &[
                User {
                    user_id: "alice".to_owned(),
                    labels: Labels::Object(
                        [("role".to_owned(), Labels::String("tester".to_owned()))]
                            .into_iter()
                            .collect(),
                    ),
                    ..User::default()
                },
                User {
                    user_id: "bob".to_owned(),
                    ..User::default()
                },
                User {
                    user_id: "neighbor-1".to_owned(),
                    ..User::default()
                },
            ],
        )
        .await
        .expect("the users are stored");
    master
        .data()
        .batch_insert_items(
            &ctx,
            &[
                item(
                    "item-1",
                    &["news"],
                    at(2026, Month::JANUARY, 1, 1),
                    embedding(),
                    "raw item",
                ),
                item(
                    "item-2",
                    &["books"],
                    at(2026, Month::JANUARY, 1, 2),
                    description(),
                    "listed item",
                ),
                item(
                    "latest-1",
                    &["news", "tech"],
                    at(2026, Month::JANUARY, 2, 0),
                    Labels::Nil,
                    "",
                ),
                item(
                    "popular-1",
                    &["news"],
                    at(2026, Month::JANUARY, 3, 0),
                    Labels::Nil,
                    "",
                ),
                item(
                    "recommend-1",
                    &["news"],
                    at(2026, Month::JANUARY, 4, 0),
                    Labels::Nil,
                    "",
                ),
                item(
                    "similar-1",
                    &["news"],
                    at(2026, Month::JANUARY, 5, 0),
                    Labels::Nil,
                    "",
                ),
            ],
        )
        .await
        .expect("the items are stored");
    master
        .data()
        .reconcile(&SearchConfig {
            columns: Some(vec!["item.Comment".to_owned()]),
        })
        .await
        .expect("the search columns are reconciled");
    master
        .data()
        .optimize()
        .await
        .expect("the data is optimized");
    master
        .data()
        .batch_insert_feedback(
            &ctx,
            &[Feedback {
                feedback_key: FeedbackKey {
                    feedback_type: "click".to_owned(),
                    user_id: "alice".to_owned(),
                    item_id: "item-1".to_owned(),
                },
                timestamp: at(2026, Month::JANUARY, 1, 3),
                ..Feedback::default()
            }],
            true,
            true,
            true,
        )
        .await
        .expect("the feedback is stored");

    let cache = master.cache();
    cache
        .add_scores(
            &ctx,
            cache::ITEM_CATEGORIES,
            "",
            &[score("books", 2.0, &[]), score("news", 1.0, &[])],
        )
        .await
        .expect("the item categories are stored");
    cache
        .add_scores(
            &ctx,
            cache::NON_PERSONALIZED,
            "popular",
            &[
                score("popular-1", 2.0, &["news"]),
                score("recommend-1", 1.0, &["news"]),
            ],
        )
        .await
        .expect("the popular items are stored");
    cache
        .set(
            &ctx,
            &[cache::string(
                cache::key(&[cache::NON_PERSONALIZED_DIGEST, "popular"]),
                "digest",
            )],
        )
        .await
        .expect("the digest is stored");
    cache
        .add_time_series_points(
            &ctx,
            &[TimeSeriesPoint {
                name: "requests".to_owned(),
                timestamp: at(2026, Month::JANUARY, 1, 0),
                value: 1.0,
            }],
        )
        .await
        .expect("the time series is stored");
}

/// A time in UTC.
fn at(year: i64, month: Month, day: i64, hour: i64) -> gotime::Time {
    gotime::date(year, month, day, hour, 0, 0, 0, &gotime::utc())
}

fn item(
    id: &str,
    categories: &[&str],
    timestamp: gotime::Time,
    labels: Labels,
    comment: &str,
) -> Item {
    Item {
        item_id: id.to_owned(),
        categories: Some(categories.iter().map(|c| (*c).to_owned()).collect()),
        timestamp,
        labels,
        comment: comment.to_owned(),
        ..Item::default()
    }
}

fn score(id: &str, score: f64, categories: &[&str]) -> Score {
    Score {
        id: id.to_owned(),
        score,
        is_hidden: false,
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        timestamp: gotime::Time::default(),
    }
}

/// Go's `embedding` label: ten numbers, kept as the text Go's `json.Number`
/// holds.
fn embedding() -> Labels {
    let numbers = [
        "0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.7", "0.8", "0.9", "1.0",
    ];
    let values = numbers
        .iter()
        .map(|number| Labels::Number((*number).to_owned()))
        .collect();
    Labels::Object(
        [("embedding".to_owned(), Labels::Array(values))]
            .into_iter()
            .collect(),
    )
}

/// Go's `description` label: 120 `x`s.
fn description() -> Labels {
    Labels::Object(
        [("description".to_owned(), Labels::String("x".repeat(120)))]
            .into_iter()
            .collect(),
    )
}

/// Compiles a pattern, with `<rfc3339>` replaced by Go's `rfc3339Regexp`.
fn compile(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("<rfc3339>", RFC3339)).expect("a valid pattern")
}

/// The lines of an output, without the newline that ends it.
fn output_lines(out: &str) -> Vec<&str> {
    out.trim_end_matches('\n').split('\n').collect()
}

fn has_box_drawing(out: &str) -> bool {
    ["┌", "│", "└"].iter().any(|mark| out.contains(mark))
}

/// Asserts that `text` matches `pattern`, as Go's `Require().Regexp` does.
fn assert_matches(text: &str, pattern: &str) {
    assert!(
        compile(pattern).is_match(text),
        "{text:?} doesn't match {pattern}"
    );
}

/// Asserts that the output has one line per pattern, in order, and that each
/// line matches its pattern, as Go's `requireCommandOutputLines` does.
fn assert_lines(out: &str, patterns: &[&str]) {
    assert!(!has_box_drawing(out), "box drawing in output:\n{out}");
    let lines = output_lines(out);
    assert_eq!(lines.len(), patterns.len(), "output:\n{out}");
    for (index, (line, pattern)) in lines.iter().zip(patterns).enumerate() {
        assert!(
            compile(pattern).is_match(line),
            "line {} {line:?} doesn't match {pattern}\noutput:\n{out}",
            index + 1
        );
    }
}

/// Runs `args` and checks the output has one line per pattern.
async fn require_lines(suite: &Suite, args: &[&str], patterns: &[&str]) -> String {
    let out = suite.execute(args).await.expect("the command succeeds");
    assert_lines(&out, patterns);
    out
}

/// Runs a recommend command and checks the scored-items table: the header, then
/// one row per pattern, as Go's `requireScoredItemsOutput` does.
async fn require_scored_items(suite: &Suite, args: &[&str], rows: &[&str]) {
    let mut patterns =
        vec![r"^ITEM-ID\s+COMMENT\s+CATEGORIES\s+IS-HIDDEN\s+TIMESTAMP\s+LABELS\s+SCORE$"];
    patterns.extend_from_slice(rows);
    require_lines(suite, args, &patterns).await;
}

/// The patterns that no line of `out` matches.
fn missing_patterns<'a>(out: &str, patterns: &[&'a str]) -> Vec<&'a str> {
    let lines = output_lines(out);
    patterns
        .iter()
        .copied()
        .filter(|pattern| {
            let regex = compile(pattern);
            !lines.iter().any(|line| regex.is_match(line))
        })
        .collect()
}

/// Runs `args` until the output has a line for every pattern or `timeout` passes,
/// as Go's `requireCommandOutputEventuallyContainsLines` does.
async fn eventually_contains_lines(
    suite: &Suite,
    args: &[&str],
    timeout: Duration,
    patterns: &[&str],
) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let result = suite.execute(args).await;
        if let Ok(out) = &result
            && !has_box_drawing(out)
            && missing_patterns(out, patterns).is_empty()
        {
            return out.clone();
        }
        if Instant::now() > deadline {
            let out = result.unwrap_or_else(|out| panic!("the command failed:\n{out}"));
            assert!(!has_box_drawing(&out), "box drawing in output:\n{out}");
            assert_eq!(
                missing_patterns(&out, patterns),
                Vec::<&str>::new(),
                "output:\n{out}"
            );
            return out;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The keys of the top-level mapping of a YAML document, which Go's test reads
/// with `yaml.Unmarshal`.
fn top_level_keys(text: &str) -> Vec<String> {
    let documents = YamlLoader::load_from_str(text).expect("the output is valid YAML");
    let mapping = documents
        .first()
        .and_then(Yaml::as_hash)
        .expect("a top-level mapping");
    mapping
        .keys()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect()
}

/// The `get items` and `get items raw item` rows for the seeded items, as Go's
/// `TestGetItems` and `TestSearchItems` expect them.
fn item_rows() -> [String; 2] {
    [
        r#"^item-1\s+raw item\s+\["news"\]\s+false\s+2026-01-01T01:00:00Z\s+\{"embedding":"\[0\.1, 0\.2, 0\.3, \.\.\.\] \(10 values\)"\}$"#.to_owned(),
        format!(
            r#"^item-2\s+listed item\s+\["books"\]\s+false\s+2026-01-01T02:00:00Z\s+\{{"description":"{}"\}}$"#,
            "x".repeat(120)
        ),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_cluster_info() {
    let suite = setup().await;
    require_lines(
        &suite,
        &["cluster-info"],
        &[
            r"^HOSTNAME\s+TYPE\s+UUID\s+UPDATE-TIME\s+VERSION$",
            r"^localhost\s+Server\s+server-node\s+<rfc3339>\s+v1$",
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_categories() {
    let suite = setup().await;
    require_lines(
        &suite,
        &["get", "categories"],
        &[r"^INDEX\s+VALUE$", r"^0\s+books$", r"^1\s+news$"],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_ps() {
    let suite = setup().await;
    eventually_contains_lines(
        &suite,
        &["ps"],
        Duration::from_secs(10),
        &[
            r"^TRACER\s+NAME\s+STATUS\s+PROGRESS\s+ERROR\s+START-TIME\s+FINISH-TIME$",
            r"^master\s+Load Dataset\s+Complete\s+\[[#-]{20}\]\s+\d+%\s+<rfc3339>\s+<rfc3339>$",
            r"^master\s+Generate recommendation\s+Complete\s+\[[#-]{20}\]\s+\d+%\s+<rfc3339>\s+<rfc3339>$",
            r"^master\s+Collect Garbage in Cache\s+Complete\s+\[[#-]{20}\]\s+\d+%\s+<rfc3339>\s+<rfc3339>$",
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_stats() {
    let suite = setup().await;
    require_lines(
        &suite,
        &["stats"],
        &[
            r"^BinaryVersion: .+$",
            r"^LatestItemsUpdateTime: <rfc3339>$",
            r"^MatchingModelFitTime: <rfc3339>$",
            r"^MatchingModelScore:$",
            r"^\{$",
            r#"^  "NDCG": 0,$"#,
            r#"^  "Precision": 0,$"#,
            r#"^  "Recall": 0$"#,
            r"^\}$",
            r"^NumItemLabels: \d+$",
            r"^NumItems: \d+$",
            r"^NumServers: \d+$",
            r"^NumTotalPosFeedback: \d+$",
            r"^NumUserLabels: \d+$",
            r"^NumUsers: \d+$",
            r"^NumValidNegFeedback: \d+$",
            r"^NumValidPosFeedback: \d+$",
            r"^NumWorkers: \d+$",
            r"^PopularItemsUpdateTime: <rfc3339>$",
            r"^RankingModelFitTime: <rfc3339>$",
            r"^RankingModelScore:$",
            r"^\{$",
            r#"^  "AUC": 0,$"#,
            r#"^  "Accuracy": 0,$"#,
            r#"^  "Precision": 0,$"#,
            r#"^  "RMSE": 0,$"#,
            r#"^  "Recall": 0$"#,
            r"^\}$",
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_recommend_latest() {
    let suite = setup().await;
    require_scored_items(
        &suite,
        &["recommend", "latest", "-n", "3", "--category", "tech"],
        &[r#"^latest-1\s+\["news","tech"\]\s+false\s+2026-01-02T00:00:00Z\s+1767312000$"#],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_recommend_non_personalized() {
    let suite = setup().await;
    require_scored_items(
        &suite,
        &[
            "recommend",
            "non-personalized",
            "popular",
            "-n",
            "3",
            "--user-id",
            "alice",
            "--category",
            "news",
        ],
        &[
            r#"^popular-1\s+\["news"\]\s+false\s+2026-01-03T00:00:00Z\s+2$"#,
            r#"^recommend-1\s+\["news"\]\s+false\s+2026-01-04T00:00:00Z\s+1$"#,
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_recommend_item_to_user() {
    let suite = setup().await;
    require_scored_items(
        &suite,
        &[
            "recommend",
            "item-to-user",
            "alice",
            "non-personalized",
            "popular",
            "-n",
            "3",
            "--category",
            "news",
        ],
        &[
            r#"^popular-1\s+\["news"\]\s+false\s+2026-01-03T00:00:00Z\s+2$"#,
            r#"^recommend-1\s+\["news"\]\s+false\s+2026-01-04T00:00:00Z\s+1$"#,
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_recommend_item_to_item() {
    let suite = setup().await;
    let ctx = Context::background();
    let collection = vectors::item_to_item_collection("neighbors");
    suite
        .master
        .vectors()
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("the item-to-item collection is created");
    suite
        .master
        .vectors()
        .add_vectors(
            &ctx,
            &collection,
            &[
                Vector {
                    id: "item-1".to_owned(),
                    indices: vec![0],
                    values: vec![1.0],
                    ..Vector::default()
                },
                Vector {
                    id: "similar-1".to_owned(),
                    indices: vec![0],
                    values: vec![1.0],
                    categories: vec!["news".to_owned()],
                    ..Vector::default()
                },
            ],
        )
        .await
        .expect("the vectors are stored");
    require_scored_items(
        &suite,
        &[
            "recommend",
            "item-to-item",
            "neighbors",
            "item-1",
            "-n",
            "3",
            "--category",
            "news",
        ],
        &[r#"^similar-1\s+\["news"\]\s+false\s+2026-01-05T00:00:00Z\s+1$"#],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_recommend_user_to_user() {
    let suite = setup().await;
    let ctx = Context::background();
    let collection = vectors::user_to_user_collection("neighbors");
    suite
        .master
        .vectors()
        .add_collection(&ctx, &collection, 0, Distance::Dot, VectorConfig::default())
        .await
        .expect("the user-to-user collection is created");
    suite
        .master
        .vectors()
        .add_vectors(
            &ctx,
            &collection,
            &[
                Vector {
                    id: "alice".to_owned(),
                    indices: vec![0],
                    values: vec![1.0],
                    ..Vector::default()
                },
                Vector {
                    id: "neighbor-1".to_owned(),
                    indices: vec![0],
                    values: vec![1.0],
                    ..Vector::default()
                },
            ],
        )
        .await
        .expect("the vectors are stored");
    require_lines(
        &suite,
        &["recommend", "user-to-user", "neighbors", "alice", "-n", "3"],
        &[r"^USER-ID\s+COMMENT\s+LABELS\s+SCORE$", r"^neighbor-1\s+1$"],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_user() {
    let suite = setup().await;
    let out = suite
        .execute(&["get", "user", "alice"])
        .await
        .expect("the user prints");
    assert_eq!(
        out,
        "UserId: alice\nComment: \nLabels:\n{\n  \"role\": \"tester\"\n}\n"
    );
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_users() {
    let suite = setup().await;
    let out = suite
        .execute(&["get", "users", "-n", "10"])
        .await
        .expect("the users print");
    let lines: Vec<&str> = out.trim().split('\n').collect();
    assert_eq!(lines.len(), 4, "output:\n{out}");
    assert_matches(lines[0], r"^USER-ID\s+COMMENT\s+LABELS$");
    assert_matches(lines[1], r#"^alice\s+\{"role":"tester"\}$"#);
    assert_matches(lines[2], r"^bob\s*$");
    assert_matches(lines[3], r"^neighbor-1\s*$");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_item() {
    let suite = setup().await;
    let out = suite
        .execute(&["get", "item", "item-1"])
        .await
        .expect("the item prints");
    assert_eq!(
        out,
        r#"ItemId: item-1
Comment: raw item
Categories: ["news"]
IsHidden: false
Timestamp: 2026-01-01T01:00:00Z
Labels:
{
  "embedding": "[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, ...] (10 values)"
}
"#
    );
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_feedback() {
    let suite = setup().await;
    let commands: [&[&str]; 4] = [
        &["get", "feedback", "-n", "1"],
        &[
            "get", "feedback", "--type", "click", "--user", "alice", "-n", "1",
        ],
        &[
            "get", "feedback", "--type", "click", "--item", "item-1", "-n", "1",
        ],
        &[
            "get", "feedback", "--type", "click", "--user", "alice", "--item", "item-1", "-n", "1",
        ],
    ];
    for args in commands {
        let out = suite.execute(args).await.expect("the feedback prints");
        let lines: Vec<&str> = out.trim().split('\n').collect();
        assert_eq!(lines.len(), 2, "output:\n{out}");
        assert_matches(
            lines[0],
            r"^FEEDBACK-TYPE\s+USER-ID\s+ITEM-ID\s+VALUE\s+TIMESTAMP\s+COMMENT\s+UPDATED\s+LABELS$",
        );
        assert_matches(
            lines[1],
            r"^click\s+alice\s+item-1\s+0\s+2026-01-01T03:00:00Z\s+\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\s*$",
        );
        assert!(!out.contains("Cursor"), "output:\n{out}");
        assert!(!has_box_drawing(&out), "box drawing in output:\n{out}");
    }

    let out = execute_raw(&["get", "feedback", "click"])
        .await
        .expect_err("an unknown command fails");
    assert!(out.contains(r#"unknown command "click""#), "output:\n{out}");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_get_items() {
    let suite = setup().await;
    let [item_1, item_2] = item_rows();
    let out = require_lines(
        &suite,
        &["get", "items", "-n", "10"],
        &[
            r"^ITEM-ID\s+COMMENT\s+CATEGORIES\s+IS-HIDDEN\s+TIMESTAMP\s+LABELS$",
            &item_1,
            &item_2,
            r#"^latest-1\s+\["news","tech"\]\s+false\s+2026-01-02T00:00:00Z\s*$"#,
            r#"^popular-1\s+\["news"\]\s+false\s+2026-01-03T00:00:00Z\s*$"#,
            r#"^recommend-1\s+\["news"\]\s+false\s+2026-01-04T00:00:00Z\s*$"#,
            r#"^similar-1\s+\["news"\]\s+false\s+2026-01-05T00:00:00Z\s*$"#,
        ],
    )
    .await;
    assert!(!out.contains("LABELS-EMBEDDING"), "output:\n{out}");
    assert!(!out.contains("0.4"), "output:\n{out}");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_search_items() {
    let suite = setup().await;
    let [item_1, item_2] = item_rows();
    require_lines(
        &suite,
        &["get", "items", "raw", "item", "-n", "10"],
        &[
            r"^ITEM-ID\s+COMMENT\s+CATEGORIES\s+IS-HIDDEN\s+TIMESTAMP\s+LABELS$",
            &item_1,
            &item_2,
        ],
    )
    .await;
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_pipeline_get() {
    let suite = setup().await;
    let out = suite
        .execute(&["pipeline", "get"])
        .await
        .expect("the pipeline prints");
    let keys = top_level_keys(&out);
    assert!(keys.iter().any(|key| key == "cache_size"), "keys: {keys:?}");
    assert!(!keys.iter().any(|key| key == "recommend"), "keys: {keys:?}");
    assert!(out.contains("cache_size:"), "output:\n{out}");
    assert!(!out.contains("recommend:"), "output:\n{out}");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_pipeline_schema() {
    let suite = setup().await;
    let out = suite
        .execute(&["pipeline", "schema"])
        .await
        .expect("the schema prints");
    let keys = top_level_keys(&out);
    assert!(keys.iter().any(|key| key == "$schema"), "keys: {keys:?}");
    assert!(keys.iter().any(|key| key == "$defs"), "keys: {keys:?}");
    assert!(out.contains("$schema:"), "output:\n{out}");
    assert!(out.contains("$defs:"), "output:\n{out}");
    assert!(out.contains("RecommendConfig"), "output:\n{out}");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_pipeline_patch() {
    let suite = setup().await;
    let patch = r#"[{"op":"replace","path":"/cache_size","value":100},{"op":"replace","path":"/ranker/type","value":"reranker"}]"#;
    let out = suite
        .execute(&["pipeline", "patch", patch])
        .await
        .expect("the patch applies");
    assert!(out.contains("CacheSize"), "output:\n{out}");
    assert!(out.contains("100"), "output:\n{out}");
    assert!(out.contains("reranker"), "output:\n{out}");

    let set_out = execute_raw(&["pipeline", "set", "recommend.cache_size=100"])
        .await
        .expect_err("an unknown command fails");
    assert!(
        set_out.contains(r#"unknown command "set""#),
        "output:\n{set_out}"
    );
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_pipeline_reset() {
    let suite = setup().await;
    let cancel_out = suite
        .execute_with_input("N\n", &["pipeline", "reset"])
        .await
        .expect("the reset is canceled");
    assert!(
        cancel_out.contains("Confirm [y/N]"),
        "output:\n{cancel_out}"
    );
    assert!(
        cancel_out.contains("Pipeline reset canceled"),
        "output:\n{cancel_out}"
    );
    assert!(
        !cancel_out.contains("Pipeline reset to defaults"),
        "output:\n{cancel_out}"
    );

    let reset_out = suite
        .execute_with_input("y\n", &["pipeline", "reset"])
        .await
        .expect("the reset succeeds");
    assert!(reset_out.contains("Confirm [y/N]"), "output:\n{reset_out}");
    assert!(reset_out.contains("Pipeline reset"), "output:\n{reset_out}");
    suite.master.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_dump_restore() {
    let suite = setup().await;
    let ctx = Context::background();
    let dir = tempfile::tempdir().expect("a temporary directory");
    let output_path = dir.path().join("backup.bin").display().to_string();

    let dump_out = suite
        .execute(&["dump", output_path.as_str()])
        .await
        .expect("the dump succeeds");
    assert!(
        dump_out.contains(&format!("Data dumped to {output_path}")),
        "output:\n{dump_out}"
    );
    let content = std::fs::read(&output_path).expect("the dump file exists");
    assert!(!content.is_empty(), "the dump file is empty");
    suite
        .master
        .data()
        .purge()
        .await
        .expect("the data is purged");

    let cancel_out = suite
        .execute_with_input("N\n", &["restore", output_path.as_str()])
        .await
        .expect("the restore is canceled");
    assert!(
        cancel_out.contains("Confirm [y/N]"),
        "output:\n{cancel_out}"
    );
    assert!(
        cancel_out.contains("Restore canceled"),
        "output:\n{cancel_out}"
    );
    let (_, users) = suite
        .master
        .data()
        .get_users(&ctx, "", 10)
        .await
        .expect("the users list");
    assert!(
        users.is_empty(),
        "users after the canceled restore: {users:?}"
    );

    let restore_out = suite
        .execute_with_input("y\n", &["restore", output_path.as_str()])
        .await
        .expect("the restore succeeds");
    assert!(
        restore_out.contains("Confirm [y/N]"),
        "output:\n{restore_out}"
    );
    assert!(
        restore_out.contains("Restored 3 users, 6 items, 1 feedback in "),
        "output:\n{restore_out}"
    );
    assert!(!restore_out.contains('│'), "output:\n{restore_out}");
    assert_eq!(
        restore_out.matches('\n').count(),
        1,
        "output:\n{restore_out}"
    );

    suite
        .master
        .data()
        .get_user(&ctx, "alice")
        .await
        .expect("alice is restored");
    suite
        .master
        .data()
        .get_item(&ctx, "item-1")
        .await
        .expect("item-1 is restored");
    let end_time = at(2026, Month::JANUARY, 2, 0);
    let (_, feedback) = suite
        .master
        .data()
        .get_feedback(&ctx, "", 10, None, Some(end_time), &[])
        .await
        .expect("the feedback lists");
    assert_eq!(feedback.len(), 1);
    assert_eq!(
        feedback[0].feedback_key,
        FeedbackKey {
            feedback_type: "click".to_owned(),
            user_id: "alice".to_owned(),
            item_id: "item-1".to_owned(),
        }
    );
    suite.master.shutdown().await;
}
