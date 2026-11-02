//! O-91 probe: captures the master's raw gRPC replies for the golden's two requests.
// Ports: none. An O-91 probe (WP-4-3 follow-up): the master's gRPC replies for the golden's
// item-to-item and latest-item requests, captured as raw bytes from a Go cluster and a Rust
// cluster seeded the way the WP-5-1 golden seeds them.
//
// Run with: cargo test -p gorse-conformance --test o91_probe -- --ignored --nocapture

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Buf;
use flate2::read::GzDecoder;
use gorse_common::gocontext::Context;
use gorse_conformance::cluster::{Cluster, GoBuild, RustBuild};
use gorse_conformance::schema::{Result, fail, repo_root};
use gorse_protocol::{GetLatestItemsRequest, GetVectorsRequest, QueryVectorsRequest};
use gorse_storage::cache;
use prost::Message;
use serde_json::Value;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::transport::Channel;
use tonic::{Request, Status};

/// The archive of the MovieLens 100k dump, as the datasets folder keeps it. The
/// environment variable `GORSE_ML_100K_DUMP` names another copy. The same archive
/// seeds the SDK suite (`tests/mixed_cluster.rs`).
const DUMP_PATH: &str = "/mnt/project-files/datasets/ml-100k.bin.gz";

/// The counts the restore must report for the MovieLens 100k dump.
const RESTORED_USERS: u64 = 943;

const RESTORED_ITEMS: u64 = 1682;

const RESTORED_FEEDBACK: u64 = 100_000;

/// The task that trains the collaborative filtering model, as the master names it.
const TRAINING_TASK: &str = "Train Collaborative Filtering Model";

/// How long the seeding of one cluster may take: the restore, the training and
/// every user's recommendation. Go's worker needs minutes for the 943 users. The
/// Rust worker took about fifteen minutes for them in the first probe run (942 of 943
/// users were done when the former 900 s limit expired), so the limit is an hour.
const SEED_TIMEOUT: Duration = Duration::from_secs(3600);

/// How long the set of recommended users must stay the same before the requests
/// are sent: two worker intervals, so the worker has passed over every user at
/// least twice since its last change.
const SETTLE: Duration = Duration::from_secs(120);

/// How often the master's tasks and the cache are read while a cluster seeds.
const POLL: Duration = Duration::from_millis(250);

/// How often a worker checks for work. The setup leaves it at the worker's default.
const WORKER_INTERVAL: Duration = Duration::from_secs(60);

/// The jobs of each worker, the worker's default.
const WORKER_JOBS: usize = 1;

/// The users the worker must recommend for before a run's requests are sent. The
/// dump's users are `1` to `943`.
const USER_COUNT: u64 = 943;

fn dump() -> Result<Vec<u8>> {
    let path = std::env::var("GORSE_ML_100K_DUMP").unwrap_or_else(|_| DUMP_PATH.to_owned());
    let archive = fs::read(&path).map_err(|e| fail(format!("read {path}: {e}")))?;
    let mut bytes = Vec::new();
    GzDecoder::new(&archive[..])
        .read_to_end(&mut bytes)
        .map_err(|e| fail(format!("decompress {path}: {e}")))?;
    Ok(bytes)
}

/// Which implementation a side runs: the Go binaries or the Rust ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Impl {
    Go,
    Rust,
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
    let count = |key: &str| {
        stats
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| fail(format!("restore response has no {key} count: {body}")))
    };
    let counts = (count("Users")?, count("Items")?, count("Feedback")?);
    if counts != (RESTORED_USERS, RESTORED_ITEMS, RESTORED_FEEDBACK) {
        return Err(fail(format!("restore counts {counts:?}")));
    }
    Ok(())
}

/// Starts the master of a side, and waits until its gRPC port listens.
async fn start_master(cluster: &mut Cluster, build: Impl) -> Result<()> {
    match build {
        Impl::Go => cluster.start_master().await,
        Impl::Rust => cluster.start_rust_master_binary().await,
    }
}

/// Waits until the master has trained its collaborative filtering model on the
/// restored dump and none of its own tasks is running. A training that fails is an
/// error at once.
async fn wait_for_training(master_http: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{master_http}/api/dashboard/tasks");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + SEED_TIMEOUT;
    loop {
        let tasks: Option<Vec<Value>> = match client.get(&url).send().await {
            Ok(response) => response.json().await.ok(),
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
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(fail(format!(
                "timed out waiting for {TRAINING_TASK} at {url}"
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// One string field of a task, or the empty string.
fn field<'a>(task: &'a Value, key: &str) -> &'a str {
    task.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The users of the dump that have a recommendation update time in the cache.
async fn recommended_users(cache: &dyn cache::Database) -> BTreeSet<u64> {
    let ctx = Context::background();
    let mut recommended = BTreeSet::new();
    for id in 1..=USER_COUNT {
        let text = id.to_string();
        let key = cache::key(&[cache::RECOMMEND_UPDATE_TIME, text.as_str()]);
        if cache.get(&ctx, &key).await.exists() {
            recommended.insert(id);
        }
    }
    recommended
}

/// The last lines of the logs of the nodes that run in `run_dir`, for a failure.
fn node_logs(run_dir: &str) -> String {
    let mut out = String::new();
    for name in ["gorse-master", "gorse-server", "gorse-worker"] {
        let text =
            fs::read_to_string(Path::new(run_dir).join(format!("{name}.log"))).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        out.push_str(&format!("--- {name} ---\n"));
        for line in &lines[lines.len().saturating_sub(30)..] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Seeds one cluster the way `client/setup-test.sh` does: the master, the server
/// and the worker start, the dump is restored, the master restarts, and the
/// master's training and the worker's recommendations are waited for.
async fn seed(build: &'static GoBuild, side: Impl) -> Result<Cluster> {
    let config = repo_root().join("client/config.toml");
    let mut cluster = Cluster::new_from_config(build, &config, &[])?;
    start_master(&mut cluster, side).await?;
    match side {
        Impl::Go => cluster.start_go_server().await?,
        Impl::Rust => cluster.start_rust_server_binary().await?,
    }
    match side {
        Impl::Go => {
            cluster
                .start_go_worker(WORKER_INTERVAL, WORKER_JOBS)
                .await?
        }
        Impl::Rust => {
            cluster
                .start_rust_worker_binary(WORKER_INTERVAL, WORKER_JOBS)
                .await?
        }
    }
    restore(cluster.ports.master_http, dump()?).await?;
    cluster.stop_master();
    start_master(&mut cluster, side).await?;
    wait_for_training(cluster.ports.master_http).await?;

    // The worker needs about two minutes for the 943 users of the dump (the Go
    // worker does about seven users a second). It writes no update time for a user
    // it skips: a cold-start user, which has no positive feedback, returns before
    // the write (`worker/pipeline.go`, `IsColdStart`). So the wait ends when the set
    // of recommended users has stayed the same for `SETTLE`, not when it is every
    // user. `Cluster::wait_for` gives up after two minutes, which is too early here.
    let cache = cluster.cache_store().await?;
    let deadline = Instant::now() + SEED_TIMEOUT;
    let mut previous = recommended_users(&*cache).await;
    let mut unchanged_since = Instant::now();
    loop {
        if Instant::now() >= deadline {
            return Err(fail(format!(
                "timed out after {SEED_TIMEOUT:?} waiting for the recommendations to settle: {} users have one\n{}",
                previous.len(),
                node_logs(&run_dir(&cluster))
            )));
        }
        let current = recommended_users(&*cache).await;
        if current != previous {
            previous = current;
            unchanged_since = Instant::now();
        } else if !previous.is_empty() && unchanged_since.elapsed() >= SETTLE {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(cluster)
}

fn run_dir(cluster: &Cluster) -> String {
    cluster
        .blob_dir()
        .parent()
        .map_or_else(String::new, |dir| dir.display().to_string())
}

/// A codec that encodes the request as protobuf and returns the reply's bytes as
/// they came off the wire, so the reply's size and its fields can be read exactly.
struct RawCodec<E>(PhantomData<fn() -> E>);

impl<E> Default for RawCodec<E> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E: Message + Send + 'static> Codec for RawCodec<E> {
    type Encode = E;
    type Decode = Vec<u8>;
    type Encoder = RawEncoder<E>;
    type Decoder = RawDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder(PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}

struct RawEncoder<E>(PhantomData<fn() -> E>);

impl<E: Message> Encoder for RawEncoder<E> {
    type Item = E;
    type Error = Status;

    fn encode(&mut self, item: E, buf: &mut EncodeBuf<'_>) -> std::result::Result<(), Status> {
        item.encode(buf)
            .map_err(|err| Status::internal(err.to_string()))
    }
}

struct RawDecoder;

impl Decoder for RawDecoder {
    type Item = Vec<u8>;
    type Error = Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> std::result::Result<Option<Vec<u8>>, Status> {
        let mut bytes = vec![0; buf.remaining()];
        buf.copy_to_slice(&mut bytes);
        Ok(Some(bytes))
    }
}

/// Calls `method` on the master's gRPC port with `request`, and returns the reply's
/// bytes. The receive limit is lifted, since the replies are larger than the 4 MiB
/// default that the server node keeps.
async fn raw_call<E: Message + Send + 'static>(
    channel: Channel,
    method: &'static str,
    request: E,
) -> Result<Vec<u8>> {
    let mut grpc = tonic::client::Grpc::new(channel).max_decoding_message_size(usize::MAX);
    grpc.ready()
        .await
        .map_err(|err| fail(format!("{method}: the master is not ready: {err}")))?;
    let path = tonic::codegen::http::uri::PathAndQuery::from_static(method);
    let response = grpc
        .unary(Request::new(request), path, RawCodec::<E>::default())
        .await
        .map_err(|err| fail(format!("{method} failed: {err}")))?;
    Ok(response.into_inner())
}

/// The two replies the server node asks the master for in the golden's requests.
struct Replies {
    /// The `QueryVectors` reply behind `GET /api/item-to-item/neighbors/1` with n = 5.
    query: Vec<u8>,
    /// The `BatchGetItems` reply that the item routes make for their neighbours
    /// (`FilterVisibleItemsByCategories`, `server/rest.go`).
    items: Vec<u8>,
    /// The `GetLatestItems` reply behind `GET /api/latest` with n = 5 and user-id = 1.
    latest: Vec<u8>,
}

/// Captures the two replies from a running master, with the arguments the server
/// sends for the golden's requests (logics/item_to_item.go, server/rest.go).
async fn capture(cluster: &Cluster) -> Result<Replies> {
    let address = format!("http://127.0.0.1:{}", cluster.ports.master_grpc);
    let channel = Channel::from_shared(address)
        .map_err(|err| fail(format!("master address: {err}")))?
        .connect()
        .await
        .map_err(|err| fail(format!("connect to the master: {err}")))?;
    let collection = gorse_storage::vectors::item_to_item_collection("neighbors");
    // The server reads the item's own vector first, then asks for n + 1 neighbours.
    let vector_reply = raw_call(
        channel.clone(),
        "/protocol.VectorStore/GetVectors",
        GetVectorsRequest {
            collection: collection.clone(),
            ids: vec!["1".to_owned()],
        },
    )
    .await?;
    let vector = gorse_protocol::GetVectorsResponse::decode(vector_reply.as_slice())
        .map_err(|err| fail(format!("decode the item's vector: {err}")))?
        .vectors
        .into_iter()
        .next()
        .ok_or_else(|| fail("the master has no vector for item 1".to_owned()))?;
    // The golden sends `n = 5` and `user-id = 1`. The server counts the items that user 1
    // has feedback on (`read`), asks for `max(offset + n + read, cache_size)` neighbours,
    // and asks `QueryVectors` for one more (`logics/item_to_item.go`, `SearchItemToItem`).
    const N: usize = 5;
    const CACHE_SIZE: usize = 100; // client/config.toml, `[recommend] cache_size`
    let feedback_reply = raw_call(
        channel.clone(),
        "/protocol.DataStore/GetUserFeedback",
        gorse_protocol::GetUserFeedbackRequest {
            user_id: "1".to_owned(),
            // The server passes the current time. 2100-01-01 is after every feedback of the dump.
            end_time: Some(prost_types::Timestamp {
                seconds: 4_102_444_800,
                nanos: 0,
            }),
            feedback_types: Vec::new(),
        },
    )
    .await?;
    let feedback = gorse_protocol::GetFeedbackResponse::decode(feedback_reply.as_slice())
        .map_err(|err| fail(format!("decode the feedback of user 1: {err}")))?
        .feedback;
    let read = feedback
        .iter()
        .map(|f| f.item_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let query_n = (N + read).max(CACHE_SIZE);
    let query = raw_call(
        channel.clone(),
        "/protocol.VectorStore/QueryVectors",
        QueryVectorsRequest {
            collection,
            query: Some(vector),
            categories: Vec::new(),
            top_k: i32::try_from(query_n + 1).map_err(|err| fail(format!("top_k: {err}")))?,
        },
    )
    .await?;
    // `logics.QueryItemToItem` skips the item itself and keeps the first `n` others.
    let neighbours = gorse_protocol::QueryVectorsResponse::decode(query.as_slice())
        .map_err(|err| fail(format!("decode the neighbours: {err}")))?
        .vectors;
    let ids: Vec<String> = neighbours
        .iter()
        .filter_map(|scored| scored.vector.as_ref().map(|v| v.id.clone()))
        .filter(|id| id != "1")
        .take(query_n)
        .collect();
    let items = raw_call(
        channel.clone(),
        "/protocol.DataStore/BatchGetItems",
        gorse_protocol::BatchGetItemsRequest {
            item_ids: ids,
            get_options: None,
        },
    )
    .await?;
    // `/api/latest` asks for `offset + n + read` items (`server/rest.go`).
    let latest = raw_call(
        channel,
        "/protocol.DataStore/GetLatestItems",
        GetLatestItemsRequest {
            n: i32::try_from(N + read).map_err(|err| fail(format!("limit: {err}")))?,
            categories: Vec::new(),
            after: None,
        },
    )
    .await?;
    Ok(Replies {
        query,
        items,
        latest,
    })
}

/// One field of a protobuf message as it is on the wire: its number, its wire type
/// and its payload. A varint keeps its own bytes, and a length-delimited field its
/// contents.
#[derive(Debug, PartialEq, Eq)]
struct Field {
    number: u64,
    wire: u8,
    payload: Vec<u8>,
}

fn read_varint(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn fields(bytes: &[u8]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let key = read_varint(bytes, &mut at);
        let number = key >> 3;
        let wire = (key & 7) as u8;
        let payload = match wire {
            0 => {
                let start = at;
                read_varint(bytes, &mut at);
                bytes[start..at].to_vec()
            }
            1 => {
                let payload = bytes[at..at + 8].to_vec();
                at += 8;
                payload
            }
            2 => {
                let len = read_varint(bytes, &mut at) as usize;
                let payload = bytes[at..at + len].to_vec();
                at += len;
                payload
            }
            5 => {
                let payload = bytes[at..at + 4].to_vec();
                at += 4;
                payload
            }
            other => panic!("wire type {other} in a reply"),
        };
        out.push(Field {
            number,
            wire,
            payload,
        });
    }
    out
}

/// The messages of a repeated message field, in order.
fn elements(message: &[u8], number: u64) -> Vec<Vec<u8>> {
    fields(message)
        .into_iter()
        .filter(|field| field.number == number && field.wire == 2)
        .map(|field| field.payload)
        .collect()
}

/// A summary of a message's fields: each field's number, and its size or value.
fn describe(message: &[u8]) -> String {
    fields(message)
        .iter()
        .map(|field| match field.wire {
            0 => format!(
                "#{}=varint({})",
                field.number,
                read_varint(&field.payload, &mut 0)
            ),
            5 => format!("#{}=fixed32", field.number),
            _ => format!("#{}=len({})", field.number, field.payload.len()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first field that differs between two messages, with its size or its bytes.
fn first_difference(go: &[u8], rust: &[u8]) -> String {
    let (go, rust) = (fields(go), fields(rust));
    for index in 0..go.len().max(rust.len()) {
        match (go.get(index), rust.get(index)) {
            (Some(g), Some(r)) if g == r => continue,
            (g, r) => {
                let show = |f: Option<&Field>| match f {
                    None => "absent".to_owned(),
                    Some(f) if f.payload.len() <= 24 => {
                        format!("#{} wire {} {:02x?}", f.number, f.wire, f.payload)
                    }
                    Some(f) => format!("#{} wire {} len {}", f.number, f.wire, f.payload.len()),
                };
                return format!("field {index}: go {} | rust {}", show(g), show(r));
            }
        }
    }
    "none".to_owned()
}

/// Compares the paired elements of two replies: the element counts, and for the
/// first few differing elements, their fields. `nested` means each element wraps
/// its message in field 1 (a ScoredVector wraps a Vector).
fn compare(name: &str, go: &[Vec<u8>], rust: &[Vec<u8>], nested: bool) {
    eprintln!(
        "{name}: go has {} elements, rust has {}",
        go.len(),
        rust.len()
    );
    let inner = |message: &[u8]| {
        if nested {
            elements(message, 1).into_iter().next().unwrap_or_default()
        } else {
            message.to_vec()
        }
    };
    let mut differing = 0;
    for (index, (g, r)) in go.iter().zip(rust).enumerate() {
        if g == r {
            continue;
        }
        differing += 1;
        if differing > 3 {
            continue;
        }
        let (gi, ri) = (inner(g), inner(r));
        eprintln!(
            "{name}[{index}]: go {} bytes, rust {} bytes; first difference: {}",
            g.len(),
            r.len(),
            first_difference(&gi, &ri)
        );
        eprintln!("  go   {}", describe(&gi));
        eprintln!("  rust {}", describe(&ri));
    }
    eprintln!(
        "{name}: {differing} of {} paired elements differ",
        go.len().min(rust.len())
    );
}

/// Seeds a Go cluster and a Rust cluster the way the golden does, captures the two
/// replies from each master, and compares them field by field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "seeds two clusters, about half an hour: run with --ignored"]
async fn o91_probe_master_replies() {
    let go_build = GoBuild::shared().expect("build the Go nodes");
    RustBuild::shared().expect("build the Rust nodes");
    let out =
        PathBuf::from(std::env::var("O91_OUT").unwrap_or_else(|_| "/tmp/claude/o91".to_owned()));
    fs::create_dir_all(&out).expect("create the output directory");

    // `O91_REUSE_GO=1` reads the Go captures already in the output folder instead of seeding
    // the Go cluster again. Go's bytes do not depend on this change, so they can be kept.
    let reuse_go = std::env::var_os("O91_REUSE_GO").is_some();
    let mut captured = Vec::new();
    for (name, side) in [("go", Impl::Go), ("rust", Impl::Rust)] {
        let replies = if name == "go" && reuse_go {
            Replies {
                query: fs::read(out.join("go-query.bin")).expect("read the saved Go query reply"),
                items: fs::read(out.join("go-items.bin")).expect("read the saved Go items reply"),
                latest: fs::read(out.join("go-latest.bin"))
                    .expect("read the saved Go latest reply"),
            }
        } else {
            let mut cluster = seed(go_build, side).await.expect("seed the cluster");
            let replies = capture(&cluster)
                .await
                .expect("capture the master's replies");
            cluster.stop();
            replies
        };
        fs::write(out.join(format!("{name}-query.bin")), &replies.query)
            .expect("write the query reply");
        fs::write(out.join(format!("{name}-items.bin")), &replies.items)
            .expect("write the items reply");
        fs::write(out.join(format!("{name}-latest.bin")), &replies.latest)
            .expect("write the latest reply");
        eprintln!(
            "{name}: QueryVectors {} bytes, BatchGetItems {} bytes, GetLatestItems {} bytes",
            replies.query.len(),
            replies.items.len(),
            replies.latest.len()
        );
        captured.push((name, replies));
    }

    let (_, go) = &captured[0];
    let (_, rust) = &captured[1];
    eprintln!(
        "sizes: query rust-go = {}, items rust-go = {}, latest rust-go = {}",
        rust.query.len() as i64 - go.query.len() as i64,
        rust.items.len() as i64 - go.items.len() as i64,
        rust.latest.len() as i64 - go.latest.len() as i64
    );
    compare(
        "query",
        &elements(&go.query, 1),
        &elements(&rust.query, 1),
        true,
    );
    compare(
        "items",
        &elements(&go.items, 1),
        &elements(&rust.items, 1),
        false,
    );
    compare(
        "latest",
        &elements(&go.latest, 1),
        &elements(&rust.latest, 1),
        false,
    );
}
