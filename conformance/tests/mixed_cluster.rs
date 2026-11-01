// Ports: client/setup-test.sh and client/client_test.go @ add21b5 (WP-4-7).

//! The WP-4-7 acceptance test: the Go SDK suite (`client/client_test.go`, run as
//! `TestGorseClientTestSuite`) against four clusters, in this order: all Go (the
//! baseline), a Go master with Rust workers and Go servers, a Go master with Rust
//! workers and Rust servers, and an all-Rust cluster. Each leg is built, set up
//! and tested here, and it fails unless every suite test runs and passes.
//!
//! The setup follows `client/setup-test.sh`, which Go's CI runs: the config of the
//! setup (`client/config.toml`), the stores of the setup (SQLite), the restore of
//! the MovieLens 100k dump through `/api/restore`, a restart of the master, and the
//! suite against the server and the dashboard. One step differs. The setup
//! downloads the dump from `cdn.gorse.io`, which this environment blocks (O-1), so
//! the dump is read from the same archive in the project's datasets folder. The
//! archive's restore stats are checked (943 users, 1,682 items, 100,000 feedback).
//!
//! Nothing is skipped. A missing Go toolchain, a missing dump, a node that does not
//! start or a suite test that does not pass fails the leg.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};
use gorse_conformance::schema::{Result, fail, repo_root};
use serde_json::Value;

/// The archive of the MovieLens 100k dump, as the datasets folder keeps it. The
/// environment variable `GORSE_ML_100K_DUMP` names another copy.
const DUMP_PATH: &str = "/mnt/project-files/datasets/ml-100k.bin.gz";

/// The counts the restore of the MovieLens 100k dump must report.
const RESTORED: Counts = Counts {
    users: 943,
    items: 1682,
    feedback: 100_000,
};

/// The suite tests `TestGorseClientTestSuite` runs, in `client/client_test.go`.
const SUITE_TESTS: [&str; 6] = [
    "TestUsers",
    "TestItems",
    "TestFeedback",
    "TestLatest",
    "TestItemToItem",
    "TestRecommend",
];

/// How often a worker checks for work, in the Go and Rust worker alike. The setup
/// leaves it at the worker's default of one minute.
const WORKER_INTERVAL: Duration = Duration::from_secs(60);

/// The jobs of each worker, the worker's default.
const WORKER_JOBS: usize = 1;

/// The task that trains the collaborative filtering model, as the master names it.
const TRAINING_TASK: &str = "Train Collaborative Filtering Model";

/// How long the training may take after the restart. The Go master trains the
/// ml-100k dump in about ten seconds.
const TRAINING_TIMEOUT: Duration = Duration::from_secs(300);

/// How often the master's tasks are read while the training runs.
const TRAINING_POLL: Duration = Duration::from_millis(250);

/// Serialises the legs, so that one leg's nodes and suite never share the machine
/// with another's.
static LEG_TURN: Mutex<()> = Mutex::new(());

/// The binary that runs one node of a leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// The Go binary, built by [`GoBuild`].
    Go,
    /// The Rust binary, built by [`RustBuild`].
    Rust,
}

/// One mix of nodes: the master, the server and the worker of a leg.
#[derive(Debug, Clone, Copy)]
struct Mix {
    label: &'static str,
    master: Node,
    server: Node,
    worker: Node,
}

/// The legs, in the order of the strangler plan. The first is the baseline the
/// others are compared with, and the last is the all-Rust cluster.
const LEGS: [Mix; 4] = [
    Mix {
        label: "all Go (baseline)",
        master: Node::Go,
        server: Node::Go,
        worker: Node::Go,
    },
    Mix {
        label: "Go master, Rust workers, Go servers",
        master: Node::Go,
        server: Node::Go,
        worker: Node::Rust,
    },
    Mix {
        label: "Go master, Rust workers, Rust servers",
        master: Node::Go,
        server: Node::Rust,
        worker: Node::Rust,
    },
    Mix {
        label: "Rust master, Rust workers, Rust servers",
        master: Node::Rust,
        server: Node::Rust,
        worker: Node::Rust,
    },
];

/// The counts a restore reports, as `DumpStats` in `master/rest.go` encodes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    users: u64,
    items: u64,
    feedback: u64,
}

/// What `go test -json` reported for each test of the suite run, by name.
#[derive(Debug, Default, PartialEq, Eq)]
struct SuiteRun {
    actions: BTreeMap<String, String>,
}

/// The MovieLens 100k dump, decompressed, from the archive the datasets folder
/// keeps. A missing or unreadable archive is an error, not a skip.
fn dump() -> Result<Vec<u8>> {
    let path = std::env::var("GORSE_ML_100K_DUMP").unwrap_or_else(|_| DUMP_PATH.to_owned());
    let archive = std::fs::read(&path).map_err(|e| fail(format!("read {path}: {e}")))?;
    let mut bytes = Vec::new();
    GzDecoder::new(&archive[..])
        .read_to_end(&mut bytes)
        .map_err(|e| fail(format!("decompress {path}: {e}")))?;
    Ok(bytes)
}

/// Starts the master of a leg, and waits until its gRPC port listens.
async fn start_master(cluster: &mut Cluster, node: Node) -> Result<()> {
    match node {
        Node::Go => cluster.start_master().await,
        Node::Rust => cluster.start_rust_master_binary().await,
    }
}

/// Starts the server of a leg, and waits until it is ready.
async fn start_server(cluster: &mut Cluster, node: Node) -> Result<()> {
    match node {
        Node::Go => cluster.start_go_server().await,
        Node::Rust => cluster.start_rust_server_binary().await,
    }
}

/// Starts the worker of a leg, and waits until it is ready.
async fn start_worker(cluster: &mut Cluster, node: Node) -> Result<()> {
    match node {
        Node::Go => cluster.start_go_worker(WORKER_INTERVAL, WORKER_JOBS).await,
        Node::Rust => {
            cluster
                .start_rust_worker_binary(WORKER_INTERVAL, WORKER_JOBS)
                .await
        }
    }
}

/// Restores the MovieLens 100k dump through the master's `/api/restore`, as the
/// setup does with `curl --data-binary`, and checks the counts it reports.
async fn restore(master_http: u16, dump: Vec<u8>) -> Result<()> {
    let url = format!("http://127.0.0.1:{master_http}/api/restore");
    let response = reqwest::Client::new()
        .post(&url)
        .body(dump)
        .send()
        .await
        .map_err(|e| fail(format!("POST {url}: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| fail(format!("read the restore response: {e}")))?;
    if !status.is_success() {
        return Err(fail(format!("restore answered {status}: {body}")));
    }
    let stats: Value =
        serde_json::from_str(&body).map_err(|e| fail(format!("restore response {body}: {e}")))?;
    let counts = Counts {
        users: count(&stats, "Users")?,
        items: count(&stats, "Items")?,
        feedback: count(&stats, "Feedback")?,
    };
    if counts != RESTORED {
        return Err(fail(format!(
            "restore counts {counts:?}, expected {RESTORED:?}"
        )));
    }
    Ok(())
}

/// One count of a restore response.
fn count(stats: &Value, key: &str) -> Result<u64> {
    stats
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| fail(format!("restore response has no {key} count: {stats}")))
}

/// One string field of a task in the master's `/api/dashboard/tasks` list, or the
/// empty string when it is missing.
fn field<'a>(task: &'a Value, key: &str) -> &'a str {
    task.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Runs the Go SDK suite against the server and the dashboard of a leg, with
/// `go test -json`, and returns what each test reported.
fn run_suite(server_http: u16, master_http: u16) -> Result<SuiteRun> {
    let output = Command::new("go")
        .current_dir(repo_root())
        .args([
            "test",
            "./client",
            "-run",
            "^TestGorseClientTestSuite$",
            "-count=1",
            "-json",
        ])
        .env(
            "GORSE_SERVER_ENDPOINT",
            format!("http://127.0.0.1:{server_http}"),
        )
        .env(
            "GORSE_DASHBOARD_ENDPOINT",
            format!("http://127.0.0.1:{master_http}"),
        )
        .output()
        .map_err(|e| fail(format!("go test is needed by the SDK suite: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut run = SuiteRun::default();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let action = event.get("Action").and_then(Value::as_str);
        let test = event.get("Test").and_then(Value::as_str);
        if let (Some(test), Some(action @ ("pass" | "fail" | "skip"))) = (test, action) {
            run.actions.insert(test.to_owned(), action.to_owned());
        }
    }
    if run.actions.is_empty() {
        return Err(fail(format!(
            "go test reported no test events ({}):\n{}{}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(run)
}

/// Fails unless the suite ran every one of its tests and each one passed.
fn check_suite(label: &str, run: &SuiteRun) -> Result<()> {
    let mut problems = Vec::new();
    let names = std::iter::once("TestGorseClientTestSuite".to_owned()).chain(
        SUITE_TESTS
            .iter()
            .map(|t| format!("TestGorseClientTestSuite/{t}")),
    );
    for name in names {
        match run.actions.get(&name).map(String::as_str) {
            Some("pass") => {}
            Some(action) => problems.push(format!("{name}: {action}")),
            None => problems.push(format!("{name}: not run")),
        }
    }
    if problems.is_empty() {
        eprintln!(
            "{label}: the SDK suite ran {} tests and all passed",
            SUITE_TESTS.len()
        );
        Ok(())
    } else {
        Err(fail(format!(
            "{label}: the SDK suite did not pass:\n{}",
            problems.join("\n")
        )))
    }
}

/// Runs one leg: the nodes, the setup, and the suite. The cluster stops when it
/// drops, at the end of the leg.
async fn run_leg(mix: Mix) -> Result<SuiteRun> {
    let go = GoBuild::shared()?;
    RustBuild::shared()?;
    let config = repo_root().join("client/config.toml");
    let mut cluster = Cluster::new_from_config(go, &config, &[])?;
    eprintln!(
        "{}: master {:?}, server {:?}, worker {:?}",
        mix.label, mix.master, mix.server, mix.worker
    );

    start_master(&mut cluster, mix.master).await?;
    start_server(&mut cluster, mix.server).await?;
    start_worker(&mut cluster, mix.worker).await?;
    restore(cluster.ports.master_http, dump()?).await?;

    cluster.stop_master();
    start_master(&mut cluster, mix.master).await?;
    wait_for_training(cluster.ports.master_http, mix.label).await?;

    run_suite(cluster.ports.server_http, cluster.ports.master_http)
}

/// Waits until the master has trained its collaborative filtering model on the
/// restored dump and none of the master's own tasks is running. `TestItemToItem`
/// reads the trained model when it asks for the neighbours, so the suite must not
/// start before this. The setup does not wait for it: its last check only looks
/// for the task's name (`client/setup-test.sh`), which Go's CI can pass before
/// training runs. A training that fails is an error at once.
///
/// Worker progress is not waited for. The master lists it with the worker's name as
/// the tracer, and the worker's recommendation pass starts again with each model it
/// pulls. The suite does not read that output, and under load the pass can run for
/// minutes (O-89), so waiting on it timed out a leg whose training had finished.
async fn wait_for_training(master_http: u16, label: &str) -> Result<Vec<Value>> {
    let url = format!("http://127.0.0.1:{master_http}/api/dashboard/tasks");
    let client = reqwest::Client::new();
    let started = Instant::now();
    let deadline = started + TRAINING_TIMEOUT;
    loop {
        let tasks = match client.get(&url).send().await {
            Ok(response) => response.json::<Vec<Value>>().await.ok(),
            Err(_) => None,
        };
        if let Some(tasks) = tasks {
            if let Some(failed) = tasks.iter().find(|task| {
                field(task, "Name") == TRAINING_TASK && field(task, "Status") == "Failed"
            }) {
                return Err(fail(format!("training failed: {failed}")));
            }
            let trained = tasks.iter().any(|task| {
                field(task, "Name") == TRAINING_TASK && field(task, "Status") == "Complete"
            });
            let running = tasks.iter().any(|task| {
                field(task, "Tracer") == "master" && field(task, "Status") == "Running"
            });
            if trained && !running {
                eprintln!(
                    "{label}: the master's tasks finished {:.1} s after the restart:\n{}",
                    started.elapsed().as_secs_f64(),
                    task_timings(&tasks)
                );
                return Ok(tasks);
            }
        }
        if Instant::now() >= deadline {
            let tasks = match client.get(&url).send().await {
                Ok(response) => response.json::<Vec<Value>>().await.unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            return Err(fail(format!(
                "{label}: timed out after {:.1} s waiting for {TRAINING_TASK} to complete at {url}:\n{}",
                started.elapsed().as_secs_f64(),
                task_timings(&tasks)
            )));
        }
        tokio::time::sleep(TRAINING_POLL).await;
    }
}

/// One line per task of the master's `/api/dashboard/tasks` list: its tracer (the
/// master, or a worker's name), its name, its status, and its start and finish
/// times, as the master reports them.
fn task_timings(tasks: &[Value]) -> String {
    tasks
        .iter()
        .map(|task| {
            format!(
                "  [{}] {}: {} (start {}, finish {})",
                field(task, "Tracer"),
                field(task, "Name"),
                field(task, "Status"),
                field(task, "StartTime"),
                field(task, "FinishTime"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs one leg on a runtime of its own, under [`LEG_TURN`], and checks its suite.
fn leg(mix: Mix) {
    let _turn = LEG_TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    let run = runtime
        .block_on(run_leg(mix))
        .unwrap_or_else(|err| panic!("{}: {err}", mix.label));
    check_suite(mix.label, &run).unwrap_or_else(|err| panic!("{err}"));
}

/// How many times each master trains the model in the timing run.
const TIMING_ROUNDS: usize = 3;

/// Where the timing run writes one row per training, unless `GORSE_CF_TIMING_OUT`
/// names another file.
const TIMING_OUT: &str = "target/o89-cf-release-timing.tsv";

/// The times of one training run, in seconds.
#[derive(Debug, Clone, Copy)]
struct TrainingTime {
    /// The master's own time for the training task: its finish stamp minus its start stamp.
    task: f64,
    /// The time from the restarted master's answer until the training task reads as
    /// complete, polled every [`TRAINING_POLL`].
    observed: f64,
}

/// Restores the dump into a new cluster that holds one master of `node`, restarts
/// the master, and times the collaborative filtering training of the restarted master.
async fn time_training(node: Node) -> Result<TrainingTime> {
    let go = GoBuild::shared()?;
    RustBuild::shared()?;
    let config = repo_root().join("client/config.toml");
    let mut cluster = Cluster::new_from_config(go, &config, &[])?;
    start_master(&mut cluster, node).await?;
    restore(cluster.ports.master_http, dump()?).await?;
    cluster.stop_master();
    start_master(&mut cluster, node).await?;

    let began = Instant::now();
    let tasks = wait_for_training(cluster.ports.master_http, &format!("{node:?} master")).await?;
    let observed = began.elapsed().as_secs_f64();

    let trained: Vec<&Value> = tasks
        .iter()
        .filter(|task| field(task, "Name") == TRAINING_TASK && field(task, "Status") == "Complete")
        .collect();
    let [task] = trained.as_slice() else {
        return Err(fail(format!(
            "{node:?} master: expected one completed {TRAINING_TASK} task, found {}:\n{}",
            trained.len(),
            task_timings(&tasks)
        )));
    };
    let start = stamp_seconds(field(task, "StartTime"))?;
    let finish = stamp_seconds(field(task, "FinishTime"))?;
    Ok(TrainingTime {
        task: finish - start,
        observed,
    })
}

/// The seconds since the Unix epoch of a time stamp as the master writes it, such as
/// `2026-10-31T14:15:03.521483797Z`.
fn stamp_seconds(stamp: &str) -> Result<f64> {
    let bad = || fail(format!("unexpected time stamp {stamp:?}"));
    let body = stamp.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = body.split_once('T').ok_or_else(bad)?;
    let mut date_parts = date.split('-').map(str::parse::<i64>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day)), None) = (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) else {
        return Err(bad());
    };
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock_parts = clock.split(':').map(str::parse::<i64>);
    let (Some(Ok(hour)), Some(Ok(minute)), Some(Ok(second)), None) = (
        clock_parts.next(),
        clock_parts.next(),
        clock_parts.next(),
        clock_parts.next(),
    ) else {
        return Err(bad());
    };
    let fraction: f64 = format!("0.{fraction}").parse().map_err(|_| bad())?;
    let days = days_from_civil(year, month, day);
    Ok((days * 86_400 + hour * 3_600 + minute * 60 + second) as f64 + fraction)
}

/// The days from 1970-01-01 to the given civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The median of a non-empty list of times.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

/// The O-89 timing of the collaborative filtering training: the same dump, the same
/// config and the same master restart, for the Go master and the Rust master, both
/// built with `--release`. Each round times one training of each master, and the
/// order alternates between rounds. Every row goes to [`TIMING_OUT`], and the medians
/// are printed. The test fails only if a training does not complete; it states no
/// threshold, so the verdict on the ratio is left to the integrator.
///
/// Run it on a quiet machine:
/// `cargo test -p gorse-conformance --release --test mixed_cluster cf_training_release_timing -- --ignored --nocapture`
#[test]
#[ignore = "O-89: timing run, use cargo test --release -- --ignored"]
fn cf_training_release_timing() {
    let _turn = LEG_TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    let mut rows = String::from("round\tmaster\ttask_seconds\tobserved_seconds\n");
    let mut go_task = Vec::new();
    let mut rust_task = Vec::new();
    let mut go_observed = Vec::new();
    let mut rust_observed = Vec::new();
    for round in 0..TIMING_ROUNDS {
        let order = if round % 2 == 0 {
            [Node::Go, Node::Rust]
        } else {
            [Node::Rust, Node::Go]
        };
        for node in order {
            let time = runtime
                .block_on(time_training(node))
                .unwrap_or_else(|err| panic!("{node:?} master: {err}"));
            rows.push_str(&format!(
                "{}\t{node:?}\t{:.3}\t{:.3}\n",
                round + 1,
                time.task,
                time.observed
            ));
            eprintln!(
                "O-89 round {} {node:?} master: training {:.3} s (task stamps), {:.3} s (observed)",
                round + 1,
                time.task,
                time.observed
            );
            let (task, observed) = match node {
                Node::Go => (&mut go_task, &mut go_observed),
                Node::Rust => (&mut rust_task, &mut rust_observed),
            };
            task.push(time.task);
            observed.push(time.observed);
        }
    }

    let out = repo_root().join(TIMING_OUT);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("the timing folder");
    }
    std::fs::write(&out, &rows).unwrap_or_else(|err| panic!("write {}: {err}", out.display()));

    let go = median(go_task);
    let rust = median(rust_task);
    eprintln!(
        "O-89 medians over {TIMING_ROUNDS} rounds: Go training {go:.3} s, Rust training {rust:.3} s, Rust/Go {:.2}; observed Go {:.3} s, Rust {:.3} s (rows in {})",
        rust / go,
        median(go_observed),
        median(rust_observed),
        out.display()
    );
}

/// The master's time stamps read as seconds, including the fraction and the day change.
#[test]
fn stamp_seconds_reads_the_masters_stamps() {
    assert_eq!(stamp_seconds("1970-01-01T00:00:00Z").unwrap(), 0.0);
    assert_eq!(stamp_seconds("1970-01-02T00:00:01.5Z").unwrap(), 86_401.5);
    // 2000-03-01 follows the leap day of 2000.
    assert_eq!(
        stamp_seconds("2000-03-01T00:00:00Z").unwrap(),
        951_868_800.0
    );
    let start = stamp_seconds("2026-10-31T14:15:01.930804163Z").unwrap();
    let finish = stamp_seconds("2026-10-31T14:15:03.521483797Z").unwrap();
    assert!((finish - start - 1.590_679_634).abs() < 1e-6);
    assert!(stamp_seconds("2026-10-31 14:15:03Z").is_err());
}

#[test]
fn mixed_cluster_all_go_baseline() {
    leg(LEGS[0]);
}

#[test]
fn mixed_cluster_go_master_rust_workers_go_servers() {
    leg(LEGS[1]);
}

#[test]
fn mixed_cluster_go_master_rust_workers_rust_servers() {
    leg(LEGS[2]);
}

#[test]
fn mixed_cluster_all_rust() {
    leg(LEGS[3]);
}
