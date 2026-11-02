//! The WP-4-6 acceptance test for the CLI across the two implementations. The
//! Go `gorse-cli` runs against the Rust master, and the Rust `gorse-cli` runs
//! against the Go master. Each runs the same commands, and each must print what
//! Go's CLI test expects.
//!
//! Each pairing is one cluster. The master starts, the test waits for its first
//! dataset load as Go's tests do, seeds the stores, and then runs the commands.
//! The test builds both CLIs itself, and fails rather than skips when a
//! toolchain is missing.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use gorse_client::AdminClient;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Month};
use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels, User};
use regex::Regex;

/// The admin API key the cluster's master checks, and that the CLIs are given.
const API_KEY: &str = "secret";

/// The config edit that sets the admin key of every master of a run.
const ADMIN_KEY_EDIT: [(&str, &str, &str); 1] = [("master", "admin_api_key", "\"secret\"")];

/// Go's `rfc3339Regexp`. Patterns write it as `<rfc3339>`.
const RFC3339: &str = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})";

/// What a command must print.
enum Expect {
    /// One line per pattern, in order, as Go's `requireCommandOutputLines` checks.
    Lines(&'static [&'static str]),
    /// Exactly this text.
    Exact(&'static str),
    /// Text that contains every `present` string and none of the `absent` ones.
    Contains {
        present: &'static [&'static str],
        absent: &'static [&'static str],
    },
}

/// The commands both CLIs run, with what each must print. They are the cases of
/// Go's `CLITestSuite` that need no recommendation run, so they hold on a master
/// that has only loaded its dataset.
const COMMANDS: [(&[&str], Expect); 9] = [
    (
        &["get", "categories"],
        Expect::Lines(&[r"^INDEX\s+VALUE$", r"^0\s+books$", r"^1\s+news$"]),
    ),
    (
        &["get", "user", "alice"],
        Expect::Exact("UserId: alice\nComment: \nLabels:\n{\n  \"role\": \"tester\"\n}\n"),
    ),
    (
        &["get", "users", "-n", "10"],
        Expect::Lines(&[
            r"^USER-ID\s+COMMENT\s+LABELS$",
            r#"^alice\s+\{"role":"tester"\}$"#,
            r"^bob\s*$",
            r"^neighbor-1\s*$",
        ]),
    ),
    (
        &["get", "item", "item-1"],
        Expect::Exact(
            r#"ItemId: item-1
Comment: raw item
Categories: ["news"]
IsHidden: false
Timestamp: 2026-01-01T01:00:00Z
Labels:
{
  "embedding": "[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, ...] (10 values)"
}
"#,
        ),
    ),
    (
        &["get", "items", "-n", "10"],
        Expect::Lines(&[
            r"^ITEM-ID\s+COMMENT\s+CATEGORIES\s+IS-HIDDEN\s+TIMESTAMP\s+LABELS$",
            r#"^item-1\s+raw item\s+\["news"\]\s+false\s+2026-01-01T01:00:00Z\s+\{"embedding":"\[0\.1, 0\.2, 0\.3, \.\.\.\] \(10 values\)"\}$"#,
            r#"^item-2\s+listed item\s+\["books"\]\s+false\s+2026-01-01T02:00:00Z\s+\{"description":"x{120}"\}$"#,
            r#"^latest-1\s+\["news","tech"\]\s+false\s+2026-01-02T00:00:00Z\s*$"#,
            r#"^popular-1\s+\["news"\]\s+false\s+2026-01-03T00:00:00Z\s*$"#,
            r#"^recommend-1\s+\["news"\]\s+false\s+2026-01-04T00:00:00Z\s*$"#,
            r#"^similar-1\s+\["news"\]\s+false\s+2026-01-05T00:00:00Z\s*$"#,
        ]),
    ),
    (
        &["get", "feedback", "-n", "1"],
        Expect::Lines(&[
            r"^FEEDBACK-TYPE\s+USER-ID\s+ITEM-ID\s+VALUE\s+TIMESTAMP\s+COMMENT\s+UPDATED\s+LABELS$",
            r"^click\s+alice\s+item-1\s+0\s+2026-01-01T03:00:00Z\s+\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\s*$",
        ]),
    ),
    (
        &["stats"],
        Expect::Lines(&[
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
        ]),
    ),
    (
        &["pipeline", "get"],
        Expect::Contains {
            present: &["cache_size:"],
            absent: &["recommend:"],
        },
    ),
    (
        &["pipeline", "schema"],
        Expect::Contains {
            present: &["$schema:", "$defs:", "RecommendConfig"],
            absent: &[],
        },
    ),
];

/// Go's `SetupTest` data for the commands in [`COMMANDS`]: the users, items,
/// feedback and categories the expected output names.
async fn seed(cluster: &Cluster) {
    let ctx = Context::background();
    let utc = gotime::utc();
    let at = |day, hour| gotime::date(2026, Month::JANUARY, day, hour, 0, 0, 0, &utc);
    let data = cluster.data_store().await.expect("the data store opens");
    data.batch_insert_users(
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
    data.batch_insert_items(
        &ctx,
        &[
            item("item-1", &["news"], at(1, 1), embedding(), "raw item"),
            item("item-2", &["books"], at(1, 2), description(), "listed item"),
            item("latest-1", &["news", "tech"], at(2, 0), Labels::Nil, ""),
            item("popular-1", &["news"], at(3, 0), Labels::Nil, ""),
            item("recommend-1", &["news"], at(4, 0), Labels::Nil, ""),
            item("similar-1", &["news"], at(5, 0), Labels::Nil, ""),
        ],
    )
    .await
    .expect("the items are stored");
    data.batch_insert_feedback(
        &ctx,
        &[Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "click".to_owned(),
                user_id: "alice".to_owned(),
                item_id: "item-1".to_owned(),
            },
            timestamp: at(1, 3),
            ..Feedback::default()
        }],
        true,
        true,
        true,
    )
    .await
    .expect("the feedback is stored");

    let cache = cluster.cache_store().await.expect("the cache store opens");
    cache
        .add_scores(
            &ctx,
            cache::ITEM_CATEGORIES,
            "",
            &[score("books", 2.0, &[]), score("news", 1.0, &[])],
        )
        .await
        .expect("the item categories are stored");
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

/// Go's `embedding` label: ten numbers, kept as the text Go's `json.Number` holds.
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

/// Waits until the master's health route answers, for a minute.
async fn wait_for_http(endpoint: &str) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("an HTTP client");
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(response) = http.get(format!("{endpoint}/api/health/live")).send().await
            && response.status() == reqwest::StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the master's HTTP port doesn't answer at {endpoint}");
}

/// Waits for the master's `Load Dataset` task to complete, as Go's tests do
/// before they seed.
async fn wait_for_initial_task(endpoint: &str) {
    let client = AdminClient::new(endpoint, API_KEY);
    let deadline = Instant::now() + Duration::from_secs(60);
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the master's initial load dataset task didn't complete");
}

/// Waits for the master, seeds the stores, and runs every command in
/// [`COMMANDS`] with `cli` against `endpoint`, checking each output.
async fn run_commands(cluster: &Cluster, cli: &Path, endpoint: &str) {
    wait_for_http(endpoint).await;
    wait_for_initial_task(endpoint).await;
    seed(cluster).await;
    for (args, expect) in COMMANDS.iter() {
        let output = Command::new(cli)
            .args(*args)
            .args(["--endpoint", endpoint, "--api-key", API_KEY])
            .output()
            .expect("the CLI runs");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "{} {args:?} failed ({}):\n{stdout}{stderr}",
            cli.display(),
            output.status
        );
        check(args, expect, &stdout);
    }
}

/// Checks one command's output against what it must print.
fn check(args: &[&str], expect: &Expect, out: &str) {
    match expect {
        Expect::Lines(patterns) => {
            assert!(
                !["┌", "│", "└"].iter().any(|mark| out.contains(mark)),
                "{args:?} printed box drawing:\n{out}"
            );
            let lines: Vec<&str> = out.trim_end_matches('\n').split('\n').collect();
            assert_eq!(lines.len(), patterns.len(), "{args:?} output:\n{out}");
            for (line, pattern) in lines.iter().zip(patterns.iter()) {
                let regex =
                    Regex::new(&pattern.replace("<rfc3339>", RFC3339)).expect("a valid pattern");
                assert!(
                    regex.is_match(line),
                    "{args:?}: {line:?} doesn't match {pattern}\noutput:\n{out}"
                );
            }
        }
        Expect::Exact(text) => assert_eq!(out, *text, "{args:?}"),
        Expect::Contains { present, absent } => {
            for text in present.iter() {
                assert!(
                    out.contains(text),
                    "{args:?} doesn't contain {text:?}\noutput:\n{out}"
                );
            }
            for text in absent.iter() {
                assert!(
                    !out.contains(text),
                    "{args:?} contains {text:?}\noutput:\n{out}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn go_cli_runs_against_the_rust_master() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let mut cluster = Cluster::new(go, &ADMIN_KEY_EDIT).expect("a run");
    cluster
        .start_rust_master_binary()
        .await
        .expect("the Rust master binary starts");
    let endpoint = format!("http://127.0.0.1:{}", cluster.ports.master_http);
    run_commands(&cluster, &go.binary("gorse-cli"), &endpoint).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_cli_runs_against_the_go_master() {
    let go = GoBuild::shared().expect("the Go nodes build");
    let rust = RustBuild::shared().expect("the Rust nodes build");
    let mut cluster = Cluster::new(go, &ADMIN_KEY_EDIT).expect("a run");
    cluster.start_master().await.expect("the Go master starts");
    let endpoint = format!("http://127.0.0.1:{}", cluster.ports.master_http);
    run_commands(&cluster, &rust.binary("gorse-cli"), &endpoint).await;
}
