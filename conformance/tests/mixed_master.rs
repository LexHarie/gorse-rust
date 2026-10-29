//! The WP-4-3 acceptance test: a master, a Go server and one Go worker over the
//! same SQLite stores. The master is the Go `gorse-master` in one run and the
//! Rust master in the other, on the same seeded data. Once every user has a
//! recommendation, each user's recommend rows and digest must match between the
//! two runs.
//!
//! The Go worker and the Go server are the same in both runs, so a difference
//! comes from the master: its dataset load, its tasks and its gRPC answers.
//!
//! The test fails, and does not skip, when the Go toolchain is missing or a node
//! does not start.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Duration;

use gorse_common::encoding;
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_conformance::cluster::{Cluster, Edit, GoBuild, worker_logs};
use gorse_storage::blob::{self, Store as _};
use gorse_storage::cache;
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, User};

/// The users of a run. Each likes two items.
const USERS: usize = 10;

/// The items of a run, one minute apart, in the category `i % 3`.
const ITEMS: usize = 30;

/// How often the worker checks for work. The first check runs after one interval.
const TICK: Duration = Duration::from_millis(500);

/// Which master a run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MasterKind {
    /// The Go `gorse-master`, as a process.
    Go,
    /// The Rust master, in this process.
    Rust,
}

/// Which configuration a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Round {
    /// No collaborative model and no ranker: the latest items are the only candidates.
    Latest,
    /// The contract's collaborative filtering model is trained and used, with
    /// no ranker. The master stores the model in its blob store.
    Training,
}

/// One recommend row: the item id, its score bits, and its categories.
type Row = (String, u64, Vec<String>);

/// The user embeddings a run stored, keyed by user id. Go writes them in map
/// order, which is random, so the blob's bytes differ between runs even when the
/// embeddings do not. The blob is read into this map to compare its contents.
type Embeddings = BTreeMap<String, Vec<f32>>;

/// What a run leaves in the cache for each user: the recommend rows, sorted, and
/// the recommend digest. Sorting makes the comparison one of sets of rows, which
/// ignores the order of ties.
#[derive(Debug, Default, PartialEq)]
struct Outcome {
    rows: BTreeMap<String, Vec<Row>>,
    digests: BTreeMap<String, String>,
    /// The user embeddings of the model the master stored, when the round trains one.
    model: Option<Embeddings>,
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

/// The user embeddings a run stored, read from the model blob. A run stores one
/// collaborative filtering model, and the blob's name is the model's time stamp,
/// which differs between runs. The blob is `MatrixFactorizationUsers.Marshal`:
/// the user count, then each user's id and embedding.
async fn read_model(cluster: &Cluster) -> Option<Embeddings> {
    let store = blob::posix::Posix::new(cluster.blob_dir());
    let ctx = Context::background();
    let names = store.list(&ctx).await.expect("list the blobs");
    if names.is_empty() {
        return None;
    }
    let [name] = names.as_slice() else {
        panic!("a run stores one model blob, found {}", names.len());
    };
    let mut reader = store.open(&ctx, name).await.expect("open the model blob");
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
        .await
        .expect("read the model blob");
    Some(decode_embeddings(&bytes))
}

/// Reads the blob [`read_model`] reads into a map, and checks that the blob has
/// no bytes left over.
fn decode_embeddings(bytes: &[u8]) -> Embeddings {
    let mut r = bytes;
    let count: i64 = encoding::read_gob(&mut r).expect("read the user count");
    let mut embeddings = Embeddings::new();
    for _ in 0..count {
        let user: String = encoding::read_string(&mut r).expect("read a user id");
        let embedding: Vec<f32> = encoding::read_slice(&mut r).expect("read a user embedding");
        assert!(
            embeddings.insert(user.clone(), embedding).is_none(),
            "user {user} is stored twice"
        );
    }
    assert!(r.is_empty(), "the model blob has {} unread bytes", r.len());
    embeddings
}

/// Runs one cluster with the master of `kind` until every user has a
/// recommendation, then reads what the cache and the blob store hold. The Go
/// server and the Go worker are the same in both runs.
async fn run(build: &'static GoBuild, kind: MasterKind, round: Round) -> Outcome {
    let edits: Vec<Edit> = match round {
        // No collaborative model and no ranker, so the only candidates are the
        // latest items. Both are deterministic, so the two runs can match exactly.
        Round::Latest => vec![
            ("recommend.collaborative", "type", "\"none\""),
            ("recommend.ranker", "type", "\"none\""),
            ("recommend.ranker", "recommenders", "[\"latest\"]"),
            ("recommend.fallback", "recommenders", "[\"latest\"]"),
        ],
        Round::Training => vec![
            ("recommend.ranker", "type", "\"none\""),
            ("recommend.ranker", "recommenders", "[\"latest\"]"),
            ("recommend.fallback", "recommenders", "[\"latest\"]"),
        ],
    };
    let mut cluster = Cluster::new(build, &edits).expect("write the cluster config");
    let (users, items, feedback) = seed();
    seed_stores(&cluster, &users, &items, &feedback).await;
    let rust = match kind {
        MasterKind::Go => {
            cluster.start_master().await.expect("start the Go master");
            None
        }
        MasterKind::Rust => Some(
            cluster
                .start_rust_master()
                .await
                .expect("start the Rust master"),
        ),
    };
    cluster
        .start_go_server()
        .await
        .expect("start the Go server");
    cluster
        .start_go_worker(TICK, 1)
        .await
        .expect("start the Go worker");

    let cache = cluster.cache_store().await.expect("open the cache store");
    let ids: Vec<String> = users.iter().map(|user| user.user_id.clone()).collect();
    let (cache_ref, ids_ref) = (&*cache, ids.as_slice());
    cluster
        .wait_for("every user to have a recommendation", move || async move {
            all_recommended(cache_ref, ids_ref).await
        })
        .await
        .expect("the master and the worker recommend for every user");
    let mut outcome = read_outcome(cache_ref, &ids).await;
    outcome.model = read_model(&cluster).await;

    cluster.stop();
    if let Some(rust) = rust {
        rust.stop().await;
    }
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

/// Runs a Go master and a Rust master over the same data, and checks that the
/// cache holds the same recommend rows and digests after each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_master_matches_go_master_in_a_mixed_cluster() {
    let build =
        GoBuild::shared().expect("the mixed-cluster test needs the Go toolchain; install it");
    let go = run(build, MasterKind::Go, Round::Latest).await;
    let rust = run(build, MasterKind::Rust, Round::Latest).await;

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
        "the Rust master's recommendations differ from the Go master's:\n{}",
        diff(&go, &rust)
    );
    // The Rust master logs its failures in this process. A run whose master
    // failed a task, for example to load the dataset, can still match on the
    // rows that the Go worker left behind, so the errors are checked too. The
    // column expression errors are not failures of a task: the contract config
    // has an embedding recommender, and the items here have no embedding, so
    // Go logs the same error for each item (logics/item_to_item.go).
    let logs = worker_logs();
    let failures: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("ERROR"))
        .filter(|line| !line.contains("failed to evaluate column expression"))
        .collect();
    assert!(
        failures.is_empty(),
        "the Rust master logged errors:\n{}",
        failures.join("\n")
    );
}

/// The training round: the Go master and the Rust master each train the
/// collaborative filtering model on the same data, and the Go worker recommends
/// from it. The stored embeddings must match exactly, and so must the rows. The
/// Go master runs twice first: its embeddings must match between its own runs,
/// or no comparison with it can be exact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_master_trains_the_go_model_in_a_mixed_cluster() {
    let build =
        GoBuild::shared().expect("the mixed-cluster test needs the Go toolchain; install it");
    let go = run(build, MasterKind::Go, Round::Training).await;
    let go_again = run(build, MasterKind::Go, Round::Training).await;
    let go_model = go.model.as_ref().expect("the Go master stores one model");
    assert!(
        !go_model.is_empty(),
        "the Go master stores an empty model, so the comparison proves nothing"
    );
    assert_eq!(
        go.model, go_again.model,
        "the Go master stores different embeddings on each run"
    );
    assert_eq!(
        go.rows, go_again.rows,
        "the Go runs recommend different rows"
    );

    let rust = run(build, MasterKind::Rust, Round::Training).await;
    assert_eq!(
        go.model, rust.model,
        "the Rust master's embeddings differ from the Go master's"
    );
    assert_eq!(
        go,
        rust,
        "the Rust master's run differs from the Go master's:\n{}",
        diff(&go, &rust)
    );
    let logs = worker_logs();
    let failures: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("ERROR"))
        .filter(|line| !line.contains("failed to evaluate column expression"))
        .collect();
    assert!(
        failures.is_empty(),
        "the Rust master logged errors:\n{}",
        failures.join("\n")
    );
}
