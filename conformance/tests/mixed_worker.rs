//! The WP-4-1 acceptance test: a Go master, a Go server and one worker over the
//! same SQLite stores. The worker is the Go `gorse-worker` in one run and the
//! Rust `gorse-worker` in the other, on the same seeded data. Once every user has
//! a recommendation, each user's recommend rows and digest must match between
//! the two runs.
//!
//! The test fails, and does not skip, when the Go toolchain is missing or a node
//! does not start.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Duration;

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_conformance::cluster::{Cluster, Edit, GoBuild, worker_logs};
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};

/// The users of a run. Each likes two items.
const USERS: usize = 10;

/// The items of a run, one minute apart, in the category `i % 3`.
const ITEMS: usize = 30;

/// How often the worker checks for work. The first check runs after one interval.
const TICK: Duration = Duration::from_millis(500);

/// Which worker a run starts after the Go master and server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerKind {
    Go,
    Rust,
}

/// How the nodes of a run talk to the master.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    /// Plain gRPC, as the contract config sets it.
    Plain,
    /// Mutual TLS: the master requires a client certificate, and every node
    /// presents one that the same CA signs.
    MutualTls,
}

/// One recommend row: the item id, its score bits, and its categories.
type Row = (String, u64, Vec<String>);

/// What a run leaves in the cache for each user: the recommend rows, sorted, and
/// the recommend digest. Sorting makes the comparison one of sets of rows, which
/// ignores the order of ties.
#[derive(Debug, Default, PartialEq, Eq)]
struct Outcome {
    rows: BTreeMap<String, Vec<Row>>,
    digests: BTreeMap<String, String>,
}

/// The rows both runs start from. The timestamps are fixed, so the latest
/// recommender orders the items the same way in both runs.
fn seed() -> (Vec<User>, Vec<Item>, Vec<Feedback>) {
    const BASE: i64 = 1_700_000_000;
    let users = (0..USERS)
        .map(|u| User {
            user_id: format!("user-{u}"),
            ..Default::default()
        })
        .collect();
    let items = (0..ITEMS)
        .map(|i| Item {
            item_id: format!("item-{i}"),
            categories: Some(vec![format!("category-{}", i % 3)]),
            timestamp: gotime::unix(BASE + 60 * i as i64, 0),
            ..Default::default()
        })
        .collect();
    let feedback = (0..USERS)
        .flat_map(|u| {
            (0..2).map(move |k| {
                let item = (3 * u + 7 * k) % ITEMS;
                Feedback {
                    feedback_key: FeedbackKey {
                        feedback_type: "like".to_owned(),
                        user_id: format!("user-{u}"),
                        item_id: format!("item-{item}"),
                    },
                    value: 1.0,
                    timestamp: gotime::unix(BASE + 3_600 + 60 * (2 * u + k) as i64, 0),
                    ..Default::default()
                }
            })
        })
        .collect();
    (users, items, feedback)
}

/// Writes the rows of a run into its data store, then closes the store so the
/// master can open the file.
async fn seed_stores(cluster: &Cluster, users: &[User], items: &[Item], feedback: &[Feedback]) {
    let data = cluster.data_store().await.expect("open the data store");
    let ctx = Context::background();
    data.init().await.expect("init the data store");
    data.batch_insert_users(&ctx, users)
        .await
        .expect("insert the users");
    data.batch_insert_items(&ctx, items)
        .await
        .expect("insert the items");
    data.batch_insert_feedback(&ctx, feedback, false, false, false)
        .await
        .expect("insert the feedback");
    data.close().await.expect("close the data store");
}

/// Whether every user has a recommendation update time in the cache.
async fn all_recommended(cache: &dyn cache::Database, ids: &[String]) -> bool {
    let ctx = Context::background();
    for id in ids {
        let key = cache::key(&[cache::RECOMMEND_UPDATE_TIME, id.as_str()]);
        if !cache.get(&ctx, &key).await.exists() {
            return false;
        }
    }
    true
}

/// What the cache holds for each user once the run has recommended for them.
async fn read_outcome(cache: &dyn cache::Database, ids: &[String]) -> Outcome {
    let ctx = Context::background();
    let mut outcome = Outcome::default();
    for id in ids {
        let scores = cache
            .search_scores(&ctx, cache::RECOMMEND, id, &[], 0, -1)
            .await
            .expect("search the recommendations");
        let mut rows: Vec<Row> = scores
            .into_iter()
            .map(|score| (score.id, score.score.to_bits(), score.categories))
            .collect();
        rows.sort();
        outcome.rows.insert(id.clone(), rows);
        let digest = cache
            .get(&ctx, &cache::key(&[cache::RECOMMEND_DIGEST, id.as_str()]))
            .await
            .string()
            .expect("read the recommend digest");
        outcome.digests.insert(id.clone(), digest);
    }
    outcome
}

/// Runs one cluster with the worker of `kind` over `transport` until every user
/// has a recommendation, then reads what the cache holds.
async fn run(build: &'static GoBuild, kind: WorkerKind, transport: Transport) -> Outcome {
    // No collaborative model and no ranker, so the only candidates are the latest
    // items. Both are deterministic, so the two runs can match exactly.
    let edits: [Edit; 4] = [
        ("recommend.collaborative", "type", "\"none\""),
        ("recommend.ranker", "type", "\"none\""),
        ("recommend.ranker", "recommenders", "[\"latest\"]"),
        ("recommend.fallback", "recommenders", "[\"latest\"]"),
    ];
    let mut cluster = match transport {
        Transport::Plain => Cluster::new(build, &edits),
        Transport::MutualTls => Cluster::new_tls(build, &edits),
    }
    .expect("write the cluster config");
    let (users, items, feedback) = seed();
    seed_stores(&cluster, &users, &items, &feedback).await;
    cluster.start_master().await.expect("start the Go master");
    cluster
        .start_go_server()
        .await
        .expect("start the Go server");
    let rust = match kind {
        WorkerKind::Go => {
            cluster
                .start_go_worker(TICK, 1)
                .await
                .expect("start the Go worker");
            None
        }
        WorkerKind::Rust => Some(
            cluster
                .start_rust_worker(TICK, 1)
                .await
                .expect("start the Rust worker"),
        ),
    };

    let cache = cluster.cache_store().await.expect("open the cache store");
    let ids: Vec<String> = users.iter().map(|user| user.user_id.clone()).collect();
    let (cache_ref, ids_ref) = (&*cache, ids.as_slice());
    cluster
        .wait_for("every user to have a recommendation", move || async move {
            all_recommended(cache_ref, ids_ref).await
        })
        .await
        .expect("the worker recommends for every user");
    let outcome = read_outcome(cache_ref, &ids).await;

    if let Some(rust) = rust {
        rust.stop().await.expect("stop the Rust worker");
    }
    cluster.stop();
    outcome
}

/// The rows and digests that differ between the two runs, user by user.
fn diff(go: &Outcome, rust: &Outcome) -> String {
    let no_rows: Vec<Row> = Vec::new();
    let users: BTreeSet<&String> = go.rows.keys().chain(rust.rows.keys()).collect();
    let mut out = String::new();
    for user in users {
        let go_rows = go.rows.get(user).unwrap_or(&no_rows);
        let rust_rows = rust.rows.get(user).unwrap_or(&no_rows);
        let go_only: Vec<&Row> = go_rows
            .iter()
            .filter(|row| !rust_rows.contains(row))
            .collect();
        let rust_only: Vec<&Row> = rust_rows
            .iter()
            .filter(|row| !go_rows.contains(row))
            .collect();
        let digests = (go.digests.get(user), rust.digests.get(user));
        if go_only.is_empty() && rust_only.is_empty() && digests.0 == digests.1 {
            continue;
        }
        let _ = writeln!(
            out,
            "{user}: only in go {go_only:?}, only in rust {rust_only:?}, digests go {:?} rust {:?}",
            digests.0, digests.1
        );
    }
    out
}

/// Runs a Go worker and a Rust worker over the same data and transport, and
/// checks that they wrote the same recommend rows and digests.
async fn assert_rust_worker_matches_go_worker(transport: Transport) {
    let build =
        GoBuild::shared().expect("the mixed-cluster test needs the Go toolchain; install it");
    let go = run(build, WorkerKind::Go, transport).await;
    let rust = run(build, WorkerKind::Rust, transport).await;

    assert_eq!(go.rows.len(), USERS, "the Go run recommends for every user");
    // The latest recommender skips the items a user has liked: 30 items less 2.
    assert!(
        go.rows.values().all(|rows| rows.len() == ITEMS - 2),
        "the Go run gives every user the latest items they have not liked"
    );
    assert!(
        go.digests.values().all(|digest| !digest.is_empty()),
        "the Go run writes a digest for every user"
    );
    assert!(
        go == rust,
        "the Rust worker's recommendations differ from the Go worker's:\n{}",
        diff(&go, &rust)
    );
    // A Rust worker that cannot reach the master logs these. Over TLS that means
    // the handshake failed, so the run would otherwise pass without the master.
    let logs = worker_logs();
    let failures: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.contains("failed to sync with the master")
                || line.contains("failed to report update task")
        })
        .collect();
    assert!(
        failures.is_empty(),
        "the Rust worker failed to reach the master:\n{}",
        failures.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_worker_matches_go_worker_in_a_mixed_cluster() {
    assert_rust_worker_matches_go_worker(Transport::Plain).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_worker_matches_go_worker_over_mutual_tls() {
    assert_rust_worker_matches_go_worker(Transport::MutualTls).await;
}
