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

use std::sync::PoisonError;
use std::time::Instant;

use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};
use gorse_conformance::schema::{Result, fail, repo_root};
use serde_json::Value;

mod common;

use common::sdk_suite::{
    LEG_TURN, LEGS, Node, TRAINING_TASK, dump, field, leg, restore, start_master, task_timings,
    wait_for_training,
};

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
    /// complete, polled as [`wait_for_training`] polls.
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
