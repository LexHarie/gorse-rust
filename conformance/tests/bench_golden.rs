// Ports: cmd/gorse-bench/main.go (the golden comparison of the Go and Rust benchmark tools, WP-4-8 step 4) @ add21b5

//! The golden test of `gorse-bench`. It seeds a synthetic dataset from a fixed
//! seed into the MySQL database named by `MYSQL_URI`, builds the Go
//! `gorse-bench` with `go build`, and runs both tools on that database with the
//! same config. The dataset table must match exactly. The rankers' table, the
//! embedding table and the user-level CSV files must match within one unit of
//! the fourth decimal, which is the precision the tools print. That rounding is
//! within the replay tolerance of 1e-5 (D-195) on the values themselves.
//!
//! The rerankers call the mock of the reranker API in `gorse_common`, and the
//! decision ranker calls a mock of the decision API below. The embedding case
//! calls the OpenAI mock of `gorse_common`, on `openai.base_url`, so both tools
//! reach the same answers without a token or the internet.
//!
//! The test fails without `MYSQL_URI`. It does not skip.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_common::mock::OpenAiServer;
use gorse_common::reranker::MockServer as RerankerServer;
use gorse_conformance::cluster::RustBuild;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};
use gorse_storage::data::{self, Database};

/// The seed of the synthetic dataset. The same seed gives the same dataset on
/// every run, so both tools read the same rows.
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// The number of users, and of items.
const USERS: usize = 30;
const ITEMS: usize = 60;

/// The printed precision of a metric: one unit in the fourth decimal.
const PRINTED_TOLERANCE: f64 = 1e-4;

/// The config both tools load. It names the data store, the feedback types,
/// the rankers' APIs and the OpenAI API.
const CONFIG: &str = r#"
[database]
data_store = "{data_store}"
data_table_prefix = "{prefix}"

[recommend]
context_size = 100

[recommend.data_source]
positive_feedback_types = ["star"]
negative_feedback_types = ["dislike"]
read_feedback_types = ["read"]

[recommend.ranker.reranker_api]
auth_token = "golden"
model = "golden-reranker"
url = "{reranker_url}"
query_template = "query"
document_template = "document"

[recommend.ranker.decision_api]
auth_token = "golden"
model = "golden-decision"
url = "{decision_url}"
state_template = "state"
instructions_template = "instructions"

[openai]
auth_token = "golden"
base_url = "{openai_url}"
embedding_model = "golden-embedding"
"#;

/// The MySQL database of the golden run, named by `MYSQL_URI`. The test fails
/// without it, because the tools read their dataset from it.
fn mysql_uri() -> String {
    std::env::var("MYSQL_URI").unwrap_or_else(|_| {
        panic!(
            "MYSQL_URI must name the MySQL database the golden test seeds. The test fails \
             without it rather than skipping"
        )
    })
}

/// A table prefix no other run shares, so repeated runs on one database do not
/// read each other's rows.
fn unique_prefix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos();
    format!(
        "bench_golden_{}_{}_",
        std::process::id(),
        nanos % 1_000_000_000
    )
}

/// A small deterministic generator (xorshift64*), so the dataset depends only on
/// [`SEED`].
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Seeds the synthetic dataset. Every user has eight `star` (positive) items, four
/// `dislike` (negative) items and four `read` items, all distinct, in a shuffled
/// order over the user's own increasing timestamps. The user-time split of the
/// benchmark therefore holds both kinds in its test part. Each item has a comment
/// for the embedding case.
async fn seed(data: &dyn Database) {
    let ctx = Context::background();
    let items: Vec<Item> = (0..ITEMS)
        .map(|j| Item {
            item_id: format!("item-{j}"),
            categories: Some(vec![format!("c{}", j % 3)]),
            timestamp: gotime::unix(1_600_000_000 + j as i64 * 60, 0),
            comment: format!("item {j} about topic {} with words {}", j % 5, j % 7),
            ..Default::default()
        })
        .collect();
    data.batch_insert_items(&ctx, &items)
        .await
        .expect("insert the items");

    let users: Vec<User> = (0..USERS)
        .map(|u| User {
            user_id: format!("user-{u}"),
            ..Default::default()
        })
        .collect();
    data.batch_insert_users(&ctx, &users)
        .await
        .expect("insert the users");

    let mut rng = Rng(SEED);
    let mut feedback = Vec::new();
    for u in 0..USERS {
        let mut order: Vec<usize> = (0..ITEMS).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i + 1));
        }
        // The kinds are shuffled over the user's timeline, so each user's latest
        // feedback, which is the test split, holds both positive and negative
        // samples.
        let mut kinds: Vec<&str> = ["star"; 8]
            .into_iter()
            .chain(["dislike"; 4])
            .chain(["read"; 4])
            .collect();
        for i in (1..kinds.len()).rev() {
            kinds.swap(i, rng.below(i + 1));
        }
        for (k, (kind, item)) in kinds.into_iter().zip(order).enumerate() {
            feedback.push(Feedback {
                feedback_key: FeedbackKey {
                    feedback_type: kind.to_owned(),
                    user_id: format!("user-{u}"),
                    item_id: format!("item-{item}"),
                },
                value: 1.0,
                timestamp: gotime::unix(1_700_000_000 + (u * 32 + k) as i64 * 60, 0),
                ..Default::default()
            });
        }
    }
    data.batch_insert_feedback(&ctx, &feedback, true, true, false)
        .await
        .expect("insert the feedback");
}

/// Serves the decision API on a local port. It answers every question of a
/// request with a probability that depends only on the question's id, so both
/// tools get the same answers. The request is the JSON `{model, state,
/// questions}` that the decision client sends, and the answers are keyed by the
/// question ids.
fn serve_decision() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the decision mock");
    let url = format!(
        "http://{}/v1/decisions",
        listener.local_addr().expect("address")
    );
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let Some(body) = read_request(&mut stream) else {
                continue;
            };
            let request: serde_json::Value =
                serde_json::from_slice(&body).expect("the decision request is JSON");
            let answers: BTreeMap<String, serde_json::Value> = request["questions"]
                .as_object()
                .expect("the questions are an object")
                .keys()
                .map(|id| {
                    let probability = f64::from(fnv1a(id) % 1000) / 1000.0;
                    (
                        id.clone(),
                        serde_json::json!({"type": "noul", "noul": probability}),
                    )
                })
                .collect();
            let reply = serde_json::json!({"model": "golden-decision", "answers": answers});
            write_response(&mut stream, &reply.to_string());
        }
    });
    url
}

/// Reads one request's body, from its `Content-Length` header. `None` when the
/// request has no body to answer.
fn read_request(stream: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

/// Writes a `200 OK` JSON response and closes the connection.
fn write_response(stream: &mut std::net::TcpStream, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// The 64-bit FNV-1a hash of a string, to turn a question id into a probability.
fn fnv1a(text: &str) -> u32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % u64::from(u32::MAX)) as u32
}

/// The Go `gorse-bench`, built into `dir` with `go build` from the repository.
fn build_go(dir: &Path) -> PathBuf {
    let binary = dir.join("gorse-bench-go");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let output = Command::new("go")
        .current_dir(&root)
        .args(["build", "-o"])
        .arg(&binary)
        .arg("./cmd/gorse-bench")
        .output()
        .expect("go is needed to build the Go gorse-bench");
    assert!(
        output.status.success(),
        "go build ./cmd/gorse-bench failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

/// Runs one tool with the config and the arguments, in its own working directory,
/// and returns its standard output with that directory. A run that fails fails
/// the test. The directory is removed when the returned value is dropped.
fn run_tool(binary: &Path, config: &Path, args: &[&str]) -> (String, tempfile::TempDir) {
    let work = tempfile::tempdir().expect("a working directory");
    // Go seeds its global random source at random, which the AFM weights draw
    // from. `randautoseed=0` seeds it with 1, as the Rust bench reads it too, so
    // both tools fit the same model.
    let output = Command::new(binary)
        .current_dir(work.path())
        .env("GODEBUG", "randautoseed=0")
        .arg("-c")
        .arg(config)
        .args(args)
        .output()
        .expect("run the benchmark tool");
    assert!(
        output.status.success(),
        "{} {:?} failed ({}):\n{}",
        binary.display(),
        args,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    (String::from_utf8_lossy(&output.stdout).into_owned(), work)
}

/// The tables of an output, each as its lines, in the order they were printed.
fn tables(output: &str) -> Vec<Vec<String>> {
    let mut tables: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in output.lines() {
        if line.starts_with('┌') {
            current = Some(Vec::new());
        }
        if let Some(table) = current.as_mut() {
            table.push(line.to_owned());
            if line.starts_with('└') {
                tables.push(current.take().unwrap_or_default());
            }
        }
    }
    tables
}

/// The cells of a table row, trimmed. A row is `│ a │ b │`, so the cells are the
/// parts between the bars.
fn cells(line: &str) -> Vec<String> {
    let parts: Vec<&str> = line.split('│').collect();
    parts[1..parts.len() - 1]
        .iter()
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// Compares two tables. Every line must match exactly, except the body rows,
/// whose cells in `metric_columns` are printed metrics: those must match within
/// [`PRINTED_TOLERANCE`]. The header is the line after the top border, so it is
/// compared as text.
fn assert_tables_match(what: &str, go: &[String], rust: &[String], metric_columns: &[usize]) {
    assert_eq!(
        go.len(),
        rust.len(),
        "{what}: the tables have different sizes\ngo:\n{}\nrust:\n{}",
        go.join("\n"),
        rust.join("\n")
    );
    for (index, (go_line, rust_line)) in go.iter().zip(rust).enumerate() {
        let is_body = go_line.starts_with('│') && index > 1;
        if !is_body {
            assert_eq!(go_line, rust_line, "{what}: line {index}");
            continue;
        }
        let (go_cells, rust_cells) = (cells(go_line), cells(rust_line));
        assert_eq!(
            go_cells.len(),
            rust_cells.len(),
            "{what}: row {index} cells"
        );
        for (column, (a, b)) in go_cells.iter().zip(&rust_cells).enumerate() {
            if !metric_columns.contains(&column) {
                assert_eq!(a, b, "{what}: row {index} column {column}");
                continue;
            }
            let go_value: f64 = a
                .parse()
                .unwrap_or_else(|_| panic!("{what}: Go metric {a:?}"));
            let rust_value: f64 = b
                .parse()
                .unwrap_or_else(|_| panic!("{what}: Rust metric {b:?}"));
            assert!(
                (go_value - rust_value).abs() <= PRINTED_TOLERANCE,
                "{what}: row {index} column {column}: Go {go_value} and Rust {rust_value} differ by more than a printed unit"
            );
        }
    }
}

/// The rows of a CSV file, sorted, with the AUC parsed. The Go tool writes its
/// rows in map order, so the rows are compared as a set.
fn csv_rows(path: &Path) -> (String, Vec<(i64, i64, f64)>) {
    let text =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default().to_owned();
    let mut rows: Vec<(i64, i64, f64)> = lines
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            assert_eq!(fields.len(), 3, "{}: row {line:?}", path.display());
            (
                fields[0].parse().expect("the feedback count"),
                fields[1].parse().expect("the candidate count"),
                fields[2].parse().expect("the AUC"),
            )
        })
        .collect();
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
    (header, rows)
}

/// Compares the user-level CSV files of the two runs: the same files, the same
/// header, and the same rows within the printed precision of the AUC.
fn assert_csvs_match(go_dir: &Path, rust_dir: &Path) {
    let names = |dir: &Path| -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read the working directory")
            .map(|entry| {
                entry
                    .expect("a directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".csv"))
            .collect();
        names.sort();
        names
    };
    let go_names = names(go_dir);
    assert_eq!(go_names, names(rust_dir), "the CSV files of the two runs");
    assert!(!go_names.is_empty(), "the run wrote no user-level CSV file");
    for name in go_names {
        let (go_header, go_rows) = csv_rows(&go_dir.join(&name));
        let (rust_header, rust_rows) = csv_rows(&rust_dir.join(&name));
        assert_eq!(go_header, rust_header, "{name}: header");
        assert_eq!(go_rows.len(), rust_rows.len(), "{name}: row count");
        for (go_row, rust_row) in go_rows.iter().zip(&rust_rows) {
            assert_eq!(
                (go_row.0, go_row.1),
                (rust_row.0, rust_row.1),
                "{name}: counts"
            );
            assert!(
                (go_row.2 - rust_row.2).abs() <= PRINTED_TOLERANCE,
                "{name}: AUC {} against {}",
                go_row.2,
                rust_row.2
            );
        }
    }
}

/// Runs the golden comparison against the database at `uri`.
async fn compare(uri: &str) {
    let prefix = unique_prefix();
    let mut registry = data::Registry::default();
    data::register_builtin(&mut registry);
    let database: Box<dyn Database> = registry
        .open(uri, &prefix, &[])
        .await
        .expect("open the MySQL data store");
    database.init().await.expect("initialise the data tables");
    seed(database.as_ref()).await;

    let reranker = Arc::new(RerankerServer::new());
    let reranker_server = Arc::clone(&reranker);
    thread::spawn(move || {
        let _ = reranker_server.start();
    });
    let openai = Arc::new(OpenAiServer::new());
    let openai_server = Arc::clone(&openai);
    thread::spawn(move || {
        let _ = openai_server.start();
    });
    let decision_url = serve_decision();

    let dir = tempfile::tempdir().expect("a directory for the config and the binaries");
    let config = dir.path().join("config.toml");
    let text = CONFIG
        .replace("{data_store}", uri)
        .replace("{prefix}", &prefix)
        .replace("{reranker_url}", &reranker.url())
        .replace("{decision_url}", &decision_url)
        .replace("{openai_url}", &openai.base_url());
    fs::write(&config, text).expect("write the config");

    let go = build_go(dir.path());
    let rust = RustBuild::shared()
        .expect("build the Rust gorse-cmd binaries")
        .binary("gorse-bench");

    // The ranking benchmarks, with their user-level CSV files.
    for subcommand in ["reranker", "decision"] {
        let (go_out, go_work) = run_tool(&go, &config, &[subcommand, "--user-auc"]);
        let (rust_out, rust_work) = run_tool(&rust, &config, &[subcommand, "--user-auc"]);
        let go_tables = tables(&go_out);
        let rust_tables = tables(&rust_out);
        assert_eq!(
            go_tables.len(),
            2,
            "{subcommand}: Go prints two tables:\n{go_out}"
        );
        assert_eq!(
            rust_tables.len(),
            2,
            "{subcommand}: Rust prints two tables:\n{rust_out}"
        );
        assert_eq!(
            go_tables[0], rust_tables[0],
            "{subcommand}: the dataset table must match exactly"
        );
        assert_tables_match(
            &format!("{subcommand}: the GAUC table"),
            &sorted_body(&go_tables[1]),
            &sorted_body(&rust_tables[1]),
            &[1],
        );
        assert_csvs_match(go_work.path(), rust_work.path());
    }

    // The embedding benchmark, whose text goes to the OpenAI mock.
    let embedding_args = [
        "embedding",
        "-k",
        "5",
        "-s",
        "3",
        "--text-column",
        "item.Comment",
    ];
    let (go_out, _go_work) = run_tool(&go, &config, &embedding_args);
    let (rust_out, _rust_work) = run_tool(&rust, &config, &embedding_args);
    let go_tables = tables(&go_out);
    let rust_tables = tables(&rust_out);
    assert_eq!(
        go_tables.len(),
        2,
        "embedding: Go prints two tables:\n{go_out}"
    );
    assert_eq!(
        rust_tables.len(),
        2,
        "embedding: Rust prints two tables:\n{rust_out}"
    );
    assert_eq!(
        go_tables[0], rust_tables[0],
        "embedding: the size table must match exactly"
    );
    assert_tables_match(
        "embedding: the metrics table",
        &go_tables[1],
        &rust_tables[1],
        &[1, 2, 3],
    );

    reranker.close().expect("stop the reranker mock");
    openai.close().expect("stop the OpenAI mock");
}

/// The body lines of a table, sorted. Go lists the rankers in map order, so the
/// rows are compared as a set.
fn sorted_body(table: &[String]) -> Vec<String> {
    let mut lines = table.to_vec();
    let body_start = 3.min(lines.len());
    let body_end = lines.len().saturating_sub(1);
    if body_start < body_end {
        lines[body_start..body_end].sort();
    }
    lines
}

#[tokio::test(flavor = "multi_thread")]
async fn bench_golden_matches_go() {
    let uri = mysql_uri();
    compare(&uri).await;
}
