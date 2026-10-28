// Ports: storage/vectors/xvec.go @ add21b5

//! The `xvec://` vector store.
//!
//! Go keeps each collection in a directory of the embedded
//! `github.com/gorse-io/xvec` engine. This port keeps the same API on Rust's
//! own file format (`PORTING.md` D-6; `PORTING_NOTES.md` X-WP28-2 and
//! X-WP28-3).
//!
//! Layout: the store's root directory holds one directory per collection,
//! named `<table prefix><collection name>`. Each collection directory holds a
//! `gorse.vectors` log: a 17-byte header (the magic `GVECTORS`, the format
//! version, the dimension and the distance code, all little-endian), then one
//! record per change. A record is a `u32` length and a payload whose first
//! byte is the operation:
//!
//! - `1` upsert: the id, the timestamp in milliseconds, the hidden flag, the
//!   categories, then the kind byte. Dense: the dimension and FP16 bits. Sparse:
//!   the count, the indices and the FP32 values.
//! - `2` delete before: the cutoff timestamp in milliseconds.
//!
//! Opening a collection replays the log. An incomplete record at the end (a
//! crash during a write) is dropped; any other damage is an error. Every write
//! is appended and synced before it is applied, so a failed write changes
//! nothing. `Optimize` rewrites the log with one upsert per live vector.
//!
//! Queries scan every live vector. The scores are xvec's: cosine as the negated
//! cosine distance `1 - cos`, Euclidean as the negated squared distance, and
//! dot as the inner product. Higher scores are better, as in Go.
//!
//! The async trait runs each call on tokio's blocking pool, because every call
//! does file IO or scans the vectors. The locks are std locks, and none is held
//! across an `.await`.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_storage_core::errors::Error as StorageError;

use super::{
    CollectionInfo, Database, Distance, Error, QuantizationType, ScoredVector, Vector,
    VectorConfig, already_exists, check_context, not_found, order_vectors,
};

/// The file each collection directory holds.
const LOG_FILE: &str = "gorse.vectors";
/// The first bytes of every log.
const MAGIC: &[u8; 8] = b"GVECTORS";
/// The version of the log layout above.
const FORMAT_VERSION: u32 = 1;
/// Magic, version, dimension (`u32`) and distance code (`u8`).
const HEADER_LEN: usize = 8 + 4 + 4 + 1;

/// A record that upserts one vector.
const OP_UPSERT: u8 = 1;
/// A record that deletes the vectors written before a cutoff.
const OP_DELETE_BEFORE: u8 = 2;
/// The kind byte of a dense vector.
const KIND_DENSE: u8 = 0;
/// The kind byte of a sparse vector.
const KIND_SPARSE: u8 = 1;

/// How many vectors a loop handles between two checks of the caller's context.
const CONTEXT_CHECK_INTERVAL: usize = 1024;

/// Go's `Xvec`: the `xvec://` store.
pub struct Xvec {
    store: Arc<Store>,
}

/// The store's state. Its methods block: they do file IO and scan vectors.
struct Store {
    root: PathBuf,
    table_prefix: String,
    inner: RwLock<Inner>,
}

/// What the store's lock guards: whether it is closed, and its collections.
struct Inner {
    closed: bool,
    collections: BTreeMap<String, Arc<Collection>>,
}

/// One collection: its schema and its vectors, which are also on disk.
struct Collection {
    dir: PathBuf,
    info: CollectionInfo,
    state: RwLock<State>,
}

/// What a collection's lock guards: the open log and the live vectors.
struct State {
    file: File,
    /// The log's length in bytes. A failed append truncates back to it.
    len: u64,
    /// The live vectors, by id. A `BTreeMap` keeps ids sorted, so query ties
    /// and rewrites are deterministic.
    docs: BTreeMap<String, Doc>,
}

/// A stored vector and its attributes.
#[derive(Debug, Clone, PartialEq)]
struct Doc {
    /// Milliseconds since the Unix epoch.
    timestamp: i64,
    hidden: bool,
    categories: Vec<String>,
    values: Values,
}

/// The values of a vector. A collection holds one kind only.
#[derive(Debug, Clone, PartialEq)]
enum Values {
    /// Dense values, already rounded to FP16 when they were written.
    Dense(Vec<f32>),
    /// Sparse values: the value at each index.
    Sparse { indices: Vec<u32>, values: Vec<f32> },
}

/// A query vector, prepared once per query.
enum Query {
    Dense(Vec<f32>),
    Sparse(HashMap<u32, f32>),
}

/// Runs `task` on tokio's blocking pool and returns its result. A panic in the
/// task propagates to the caller, as a panic in Go's store would crash the
/// process. Must be awaited inside a tokio runtime.
pub(super) async fn blocking<T, F>(task: F) -> Result<T, Error>
where
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(err) => match err.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(err) => Err(Error::Other(err.to_string())),
        },
    }
}

impl Xvec {
    /// Go's `Open` for an `xvec://` path. `root` is the path without the
    /// prefix. Nothing is read until [`Database::init`].
    pub fn new(root: &str, table_prefix: &str) -> Result<Xvec, Error> {
        if root.is_empty() {
            return Err(Error::Other("xvec path is empty".to_owned()));
        }
        Ok(Xvec {
            store: Arc::new(Store {
                root: PathBuf::from(root),
                table_prefix: table_prefix.to_owned(),
                inner: RwLock::new(Inner {
                    closed: false,
                    collections: BTreeMap::new(),
                }),
            }),
        })
    }
}

impl Store {
    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        // A panic while the lock was held leaves the maps intact: each write
        // is applied only after it is on disk. So a poisoned lock is still safe.
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Go's `collection`: the collection named `name`, if the store is open and holds it.
    fn collection(&self, name: &str) -> Result<Arc<Collection>, Error> {
        let inner = self.read();
        if inner.closed {
            return Err(closed());
        }
        inner
            .collections
            .get(name)
            .cloned()
            .ok_or_else(|| not_found(name))
    }

    fn init(&self) -> Result<(), Error> {
        fs::create_dir_all(&self.root).map_err(io_error)?;
        let entries = fs::read_dir(&self.root).map_err(io_error)?;
        {
            let inner = self.read();
            if inner.closed {
                return Err(closed());
            }
            if !inner.collections.is_empty() {
                return Ok(());
            }
        }
        let mut opened = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = dir_name.strip_prefix(self.table_prefix.as_str()) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            match Collection::open(&entry.path(), name)? {
                Some(collection) => opened.push((name.to_owned(), collection)),
                None => tracing::warn!(
                    collection = name,
                    "skipping a directory without a vector log of this format"
                ),
            }
        }
        let mut inner = self.write();
        if inner.closed {
            return Err(closed());
        }
        for (name, collection) in opened {
            inner
                .collections
                .entry(name)
                .or_insert_with(|| Arc::new(collection));
        }
        Ok(())
    }

    fn optimize(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        let collection = self.collection(name)?;
        check_context(ctx)?;
        let mut state = collection
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        compact(ctx, &collection, &mut state)
    }

    fn close(&self) -> Result<(), Error> {
        let mut inner = self.write();
        if inner.closed {
            return Ok(());
        }
        inner.closed = true;
        // The collections close with their last handle: every write was synced.
        inner.collections.clear();
        Ok(())
    }

    fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error> {
        check_context(ctx)?;
        let inner = self.read();
        if inner.closed {
            return Err(closed());
        }
        Ok(inner.collections.keys().cloned().collect())
    }

    fn describe_collection(&self, ctx: &Context, name: &str) -> Result<CollectionInfo, Error> {
        check_context(ctx)?;
        Ok(self.collection(name)?.info.clone())
    }

    fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: &VectorConfig,
    ) -> Result<(), Error> {
        // Go's `collectionSchema`, which runs before the store is looked at.
        check_context(ctx)?;
        if dimensions < 0 || dimensions > i64::from(u32::MAX) {
            return Err(Error::Other(format!(
                "invalid vector dimension {dimensions}"
            )));
        }
        if config.quantization != QuantizationType::None {
            return Err(Error::Storage {
                kind: StorageError::NotSupported,
                message: format!(
                    "quantization type {} for xvec {}",
                    config.quantization,
                    StorageError::NotSupported
                ),
            });
        }
        if dimensions == 0 && distance != Distance::Dot {
            return Err(Error::Storage {
                kind: StorageError::NotSupported,
                message: format!(
                    "distance method for sparse vector {}",
                    StorageError::NotSupported
                ),
            });
        }
        check_name(name)?;

        let mut inner = self.write();
        if inner.closed {
            return Err(closed());
        }
        if inner.collections.contains_key(name) {
            return Err(already_exists(name));
        }
        let dir = self.root.join(format!("{}{name}", self.table_prefix));
        if dir.exists() {
            if holds_collection(&dir) {
                return Err(Error::Other(format!(
                    "directory {} already holds a collection",
                    dir.display()
                )));
            }
            // A directory of another store under this name. Its contents are
            // derived data, so it is replaced (`PORTING_NOTES.md` X-WP28-3).
            tracing::warn!(
                collection = name,
                directory = %dir.display(),
                "replacing a directory without a vector log of this format"
            );
            fs::remove_dir_all(&dir).map_err(io_error)?;
        }
        let info = CollectionInfo {
            name: name.to_owned(),
            dimension: dimensions,
            distance,
            config: VectorConfig::default(),
        };
        let collection = Collection::create(&dir, info)?;
        inner
            .collections
            .insert(name.to_owned(), Arc::new(collection));
        Ok(())
    }

    fn delete_collection(&self, name: &str) -> Result<(), Error> {
        let mut inner = self.write();
        if inner.closed {
            return Err(closed());
        }
        let Some(collection) = inner.collections.remove(name) else {
            return Err(not_found(name));
        };
        if let Err(err) = fs::remove_dir_all(&collection.dir) {
            // Go puts the collection back when its destroy fails.
            inner.collections.insert(name.to_owned(), collection);
            return Err(io_error(err));
        }
        Ok(())
    }

    fn count_vectors(&self, ctx: &Context, name: &str) -> Result<i64, Error> {
        check_context(ctx)?;
        let collection = self.collection(name)?;
        let state = collection
            .state
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(state.docs.len() as i64)
    }

    fn add_vectors(&self, ctx: &Context, name: &str, vectors: &[Vector]) -> Result<(), Error> {
        if vectors.is_empty() {
            return Ok(());
        }
        let collection = self.collection(name)?;
        check_context(ctx)?;
        let docs = vectors
            .iter()
            .map(|vector| Ok((vector.id.as_str(), collection.encode(vector)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        check_context(ctx)?;
        let mut state = collection
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let payloads: Vec<Vec<u8>> = docs
            .iter()
            .map(|(id, doc)| upsert_payload(id, doc))
            .collect();
        append(&mut state, &payloads)?;
        for (id, doc) in docs {
            state.docs.insert(id.to_owned(), doc);
        }
        Ok(())
    }

    fn get_vectors(&self, ctx: &Context, name: &str, ids: &[String]) -> Result<Vec<Vector>, Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let collection = self.collection(name)?;
        check_context(ctx)?;
        let state = collection
            .state
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let mut vectors = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            if index % CONTEXT_CHECK_INTERVAL == 0 {
                check_context(ctx)?;
            }
            if let Some(doc) = state.docs.get(id) {
                // Go's `GetVectors` returns the timestamp in UTC.
                let timestamp = gotime::unix_milli(doc.timestamp).utc();
                vectors.push(vector_from(id, doc, timestamp));
            }
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        Ok(order_vectors(&refs, vectors))
    }

    fn delete_vectors(&self, ctx: &Context, name: &str, timestamp: &Time) -> Result<(), Error> {
        let collection = self.collection(name)?;
        check_context(ctx)?;
        let cutoff = timestamp.unix_milli();
        let mut state = collection
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        append(&mut state, &[delete_payload(cutoff)])?;
        state.docs.retain(|_, doc| doc.timestamp >= cutoff);
        Ok(())
    }

    fn query_vectors(
        &self,
        ctx: &Context,
        name: &str,
        q: &Vector,
        categories: &[String],
        top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error> {
        if top_k <= 0 {
            return Ok(Vec::new());
        }
        let collection = self.collection(name)?;
        check_context(ctx)?;
        let query = if q.is_sparse() {
            if collection.info.dimension != 0 {
                return Err(Error::Other(format!(
                    "query is sparse but collection {} is dense",
                    collection.info.name
                )));
            }
            if q.indices.len() != q.values.len() {
                return Err(Error::Other(format!(
                    "query has {} indices and {} values",
                    q.indices.len(),
                    q.values.len()
                )));
            }
            Query::Sparse(
                q.indices
                    .iter()
                    .copied()
                    .zip(q.values.iter().copied())
                    .collect(),
            )
        } else {
            if collection.info.dimension == 0 {
                return Err(Error::Other(format!(
                    "query is dense but collection {} is sparse",
                    collection.info.name
                )));
            }
            let bits = q.float16_values();
            if bits.len() as i64 != collection.info.dimension {
                return Err(Error::Other(format!(
                    "query has {} dimensions, collection {} has {}",
                    bits.len(),
                    collection.info.name,
                    collection.info.dimension
                )));
            }
            Query::Dense(floats::to_float32(&bits))
        };

        let state = collection
            .state
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let mut hits: Vec<(f32, &str, &Doc)> = Vec::new();
        for (index, (id, doc)) in state.docs.iter().enumerate() {
            if index % CONTEXT_CHECK_INTERVAL == 0 {
                check_context(ctx)?;
            }
            if doc.hidden || !has_categories(&doc.categories, categories) {
                continue;
            }
            let score = match (&doc.values, &query) {
                (Values::Dense(values), Query::Dense(query)) => {
                    dense_score(collection.info.distance, query, values)
                }
                (Values::Sparse { indices, values }, Query::Sparse(query)) => {
                    let score = sparse_dot(query, indices, values);
                    // Go skips the zero scores of a sparse query: no shared index.
                    if score == 0.0 {
                        continue;
                    }
                    score
                }
                // A collection holds one kind of vector, and the query was
                // checked against it above.
                _ => continue,
            };
            hits.push((score, id, doc));
        }
        // Best score first. Ties go to the smaller id, so the order is total.
        let by_score = |a: &(f32, &str, &Doc), b: &(f32, &str, &Doc)| {
            b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1))
        };
        let top_k = usize::try_from(top_k).unwrap_or(usize::MAX);
        if hits.len() > top_k {
            hits.select_nth_unstable_by(top_k - 1, by_score);
            hits.truncate(top_k);
        }
        hits.sort_by(by_score);
        Ok(hits
            .into_iter()
            .map(|(score, id, doc)| ScoredVector {
                // Go's QueryVectors does not convert the timestamp to UTC.
                vector: vector_from(id, doc, gotime::unix_milli(doc.timestamp)),
                score,
            })
            .collect())
    }
}

#[async_trait]
impl Database for Xvec {
    async fn init(&self) -> Result<(), Error> {
        let store = Arc::clone(&self.store);
        blocking(move || store.init()).await
    }

    async fn optimize(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), name.to_owned());
        blocking(move || store.optimize(&ctx, &name)).await
    }

    async fn close(&self) -> Result<(), Error> {
        let store = Arc::clone(&self.store);
        blocking(move || store.close()).await
    }

    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error> {
        let (store, ctx) = (Arc::clone(&self.store), ctx.clone());
        blocking(move || store.list_collections(&ctx)).await
    }

    async fn describe_collection(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<CollectionInfo, Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), name.to_owned());
        blocking(move || store.describe_collection(&ctx, &name)).await
    }

    async fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), name.to_owned());
        blocking(move || store.add_collection(&ctx, &name, dimensions, distance, &config)).await
    }

    async fn delete_collection(&self, _ctx: &Context, name: &str) -> Result<(), Error> {
        let (store, name) = (Arc::clone(&self.store), name.to_owned());
        blocking(move || store.delete_collection(&name)).await
    }

    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), collection.to_owned());
        blocking(move || store.count_vectors(&ctx, &name)).await
    }

    async fn add_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        vectors: &[Vector],
    ) -> Result<(), Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), collection.to_owned());
        let vectors = vectors.to_vec();
        blocking(move || store.add_vectors(&ctx, &name, &vectors)).await
    }

    async fn get_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<Vector>, Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), collection.to_owned());
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        blocking(move || store.get_vectors(&ctx, &name, &ids)).await
    }

    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), collection.to_owned());
        let timestamp = timestamp.clone();
        blocking(move || store.delete_vectors(&ctx, &name, &timestamp)).await
    }

    async fn query_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        q: &Vector,
        categories: &[&str],
        top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error> {
        let (store, ctx, name) = (Arc::clone(&self.store), ctx.clone(), collection.to_owned());
        let (q, categories) = (
            q.clone(),
            categories
                .iter()
                .map(|c| (*c).to_owned())
                .collect::<Vec<_>>(),
        );
        blocking(move || store.query_vectors(&ctx, &name, &q, &categories, top_k)).await
    }
}

fn closed() -> Error {
    Error::Other("xvec database is closed".to_owned())
}

fn io_error(err: io::Error) -> Error {
    Error::Other(err.to_string())
}

fn corrupt(name: &str, reason: &str) -> Error {
    Error::Other(format!("xvec collection {name} is corrupt: {reason}"))
}

/// Go's `distanceToXvec`, as the code the header stores.
fn distance_code(distance: Distance) -> u8 {
    match distance {
        Distance::Cosine => 0,
        Distance::Euclidean => 1,
        Distance::Dot => 2,
    }
}

fn distance_from_code(code: u8) -> Option<Distance> {
    match code {
        0 => Some(Distance::Cosine),
        1 => Some(Distance::Euclidean),
        2 => Some(Distance::Dot),
        _ => None,
    }
}

/// Rejects names that would not stay one directory under the store's root.
/// `PORTING_NOTES.md` X-WP28-4.
fn check_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::Other(format!("invalid collection name {name:?}")));
    }
    Ok(())
}

fn header(dimension: u32, distance: Distance) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN);
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&dimension.to_le_bytes());
    buf.push(distance_code(distance));
    buf
}

fn put_u32(buf: &mut Vec<u8>, value: usize) {
    // The sizes written here are bounded by the request that carries them,
    // far below `u32::MAX` for any batch that fits in memory.
    buf.extend_from_slice(&(value as u32).to_le_bytes());
}

fn put_str(buf: &mut Vec<u8>, value: &str) {
    put_u32(buf, value.len());
    buf.extend_from_slice(value.as_bytes());
}

/// The payload of an upsert record.
fn upsert_payload(id: &str, doc: &Doc) -> Vec<u8> {
    let mut buf = vec![OP_UPSERT];
    put_str(&mut buf, id);
    buf.extend_from_slice(&doc.timestamp.to_le_bytes());
    buf.push(u8::from(doc.hidden));
    put_u32(&mut buf, doc.categories.len());
    for category in &doc.categories {
        put_str(&mut buf, category);
    }
    match &doc.values {
        Values::Dense(values) => {
            buf.push(KIND_DENSE);
            put_u32(&mut buf, values.len());
            for bits in floats::from_float32(values) {
                buf.extend_from_slice(&bits.to_le_bytes());
            }
        }
        Values::Sparse { indices, values } => {
            buf.push(KIND_SPARSE);
            put_u32(&mut buf, indices.len());
            for index in indices {
                buf.extend_from_slice(&index.to_le_bytes());
            }
            for value in values {
                buf.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
    }
    buf
}

/// The payload of a delete-before record.
fn delete_payload(cutoff: i64) -> Vec<u8> {
    let mut buf = vec![OP_DELETE_BEFORE];
    buf.extend_from_slice(&cutoff.to_le_bytes());
    buf
}

/// Reads the fields of one record, failing when the payload is too short.
struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if self.buf.len() < len {
            return None;
        }
        let (head, tail) = self.buf.split_at(len);
        self.buf = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i64(&mut self) -> Option<i64> {
        self.take(8).map(|bytes| {
            i64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        })
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }

    fn finished(&self) -> bool {
        self.buf.is_empty()
    }
}

/// Applies one record to the vectors read so far. A reason is returned when
/// the record does not parse as the layout above.
fn apply(
    docs: &mut BTreeMap<String, Doc>,
    dimension: i64,
    payload: &[u8],
) -> Result<(), &'static str> {
    let mut reader = Reader { buf: payload };
    match reader.u8().ok_or("empty record")? {
        OP_UPSERT => {
            let id = reader.string().ok_or("bad id")?;
            let timestamp = reader.i64().ok_or("bad timestamp")?;
            let hidden = match reader.u8().ok_or("bad hidden flag")? {
                0 => false,
                1 => true,
                _ => return Err("bad hidden flag"),
            };
            let categories = (0..reader.u32().ok_or("bad category count")?)
                .map(|_| reader.string().ok_or("bad category"))
                .collect::<Result<Vec<_>, _>>()?;
            let values = match reader.u8().ok_or("bad kind")? {
                KIND_DENSE => {
                    let count = reader.u32().ok_or("bad dimension")? as i64;
                    if count != dimension {
                        return Err("dense vector of the wrong dimension");
                    }
                    let bits = (0..count)
                        .map(|_| reader.u16().ok_or("truncated vector"))
                        .collect::<Result<Vec<_>, _>>()?;
                    Values::Dense(floats::to_float32(&bits))
                }
                KIND_SPARSE => {
                    let count = reader.u32().ok_or("bad sparse count")? as usize;
                    let indices = (0..count)
                        .map(|_| reader.u32().ok_or("truncated indices"))
                        .collect::<Result<Vec<_>, _>>()?;
                    let values = (0..count)
                        .map(|_| reader.u32().map(f32::from_bits).ok_or("truncated values"))
                        .collect::<Result<Vec<_>, _>>()?;
                    Values::Sparse { indices, values }
                }
                _ => return Err("unknown kind"),
            };
            docs.insert(
                id,
                Doc {
                    timestamp,
                    hidden,
                    categories,
                    values,
                },
            );
        }
        OP_DELETE_BEFORE => {
            let cutoff = reader.i64().ok_or("bad cutoff")?;
            docs.retain(|_, doc| doc.timestamp >= cutoff);
        }
        _ => return Err("unknown operation"),
    }
    if !reader.finished() {
        return Err("trailing bytes");
    }
    Ok(())
}

/// Reads the header. `None` means the directory holds no log of this format,
/// as a directory from another store does.
fn parse_header(bytes: &[u8], name: &str) -> Result<Option<CollectionInfo>, Error> {
    if bytes.len() < HEADER_LEN || bytes[..MAGIC.len()] != MAGIC[..] {
        return Ok(None);
    }
    let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if version != FORMAT_VERSION {
        return Err(Error::Other(format!(
            "xvec collection {name} has format version {version}, this build reads {FORMAT_VERSION}"
        )));
    }
    let dimension = i64::from(u32::from_le_bytes([
        bytes[12], bytes[13], bytes[14], bytes[15],
    ]));
    let distance =
        distance_from_code(bytes[16]).ok_or_else(|| corrupt(name, "unknown distance"))?;
    Ok(Some(CollectionInfo {
        name: name.to_owned(),
        dimension,
        distance,
        config: VectorConfig::default(),
    }))
}

/// Whether `dir` holds a log of this format.
fn holds_collection(dir: &Path) -> bool {
    match fs::read(dir.join(LOG_FILE)) {
        Ok(bytes) => bytes.len() >= MAGIC.len() && bytes[..MAGIC.len()] == MAGIC[..],
        Err(_) => false,
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

impl Collection {
    /// Creates the directory and an empty log.
    fn create(dir: &Path, info: CollectionInfo) -> Result<Collection, Error> {
        fs::create_dir_all(dir).map_err(io_error)?;
        let path = dir.join(LOG_FILE);
        let header = header(info.dimension as u32, info.distance);
        let mut file = File::create(&path).map_err(io_error)?;
        file.write_all(&header).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io_error)?;
        sync_dir(dir).map_err(io_error)?;
        Ok(Collection {
            dir: dir.to_path_buf(),
            info,
            state: RwLock::new(State {
                file,
                len: HEADER_LEN as u64,
                docs: BTreeMap::new(),
            }),
        })
    }

    /// Opens the collection in `dir` by replaying its log. `None` means `dir`
    /// does not hold a log of this format.
    fn open(dir: &Path, name: &str) -> Result<Option<Collection>, Error> {
        let path = dir.join(LOG_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(io_error(err)),
        };
        let Some(info) = parse_header(&bytes, name)? else {
            return Ok(None);
        };
        let mut docs = BTreeMap::new();
        let mut offset = HEADER_LEN;
        while offset < bytes.len() {
            let Some(prefix) = bytes.get(offset..offset + 4) else {
                break;
            };
            let len = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
            let Some(payload) = bytes.get(offset + 4..offset + 4 + len) else {
                break;
            };
            apply(&mut docs, info.dimension, payload).map_err(|reason| corrupt(name, reason))?;
            offset += 4 + len;
        }
        if offset < bytes.len() {
            tracing::warn!(
                collection = name,
                dropped_bytes = bytes.len() - offset,
                "dropping an incomplete record at the end of the vector log"
            );
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(io_error)?;
            file.set_len(offset as u64).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
        }
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io_error)?;
        Ok(Some(Collection {
            dir: dir.to_path_buf(),
            info,
            state: RwLock::new(State {
                file,
                len: offset as u64,
                docs,
            }),
        }))
    }

    /// The stored form of `vector`, if it fits the collection. The values are
    /// rounded to FP16 here, as Go's `Upsert` rounds them.
    fn encode(&self, vector: &Vector) -> Result<Doc, Error> {
        let values = if vector.is_sparse() {
            if self.info.dimension != 0 {
                return Err(Error::Other(format!(
                    "vector {} is sparse but collection {} is dense",
                    vector.id, self.info.name
                )));
            }
            if vector.indices.len() != vector.values.len() {
                return Err(Error::Other(format!(
                    "vector {} has {} indices and {} values",
                    vector.id,
                    vector.indices.len(),
                    vector.values.len()
                )));
            }
            Values::Sparse {
                indices: vector.indices.clone(),
                values: vector.values.clone(),
            }
        } else {
            if self.info.dimension == 0 {
                return Err(Error::Other(format!(
                    "vector {} is dense but collection {} is sparse",
                    vector.id, self.info.name
                )));
            }
            let bits = vector.float16_values();
            if bits.len() as i64 != self.info.dimension {
                return Err(Error::Other(format!(
                    "vector {} has {} dimensions, collection {} has {}",
                    vector.id,
                    bits.len(),
                    self.info.name,
                    self.info.dimension
                )));
            }
            Values::Dense(floats::to_float32(&bits))
        };
        Ok(Doc {
            timestamp: vector.timestamp.unix_milli(),
            hidden: vector.is_hidden,
            categories: vector.categories.clone(),
            values,
        })
    }
}

/// Appends records to the log, syncs them, and only then returns. On a failed
/// write the log is cut back to its length before the write.
fn append(state: &mut State, payloads: &[Vec<u8>]) -> Result<(), Error> {
    let mut buf = Vec::new();
    for payload in payloads {
        put_u32(&mut buf, payload.len());
        buf.extend_from_slice(payload);
    }
    let written = state
        .file
        .write_all(&buf)
        .and_then(|()| state.file.sync_data());
    match written {
        Ok(()) => {
            state.len += buf.len() as u64;
            Ok(())
        }
        Err(err) => {
            // Ignored: the write error is the one to report. A failed cut
            // leaves a torn record that the next open drops.
            let _ = state.file.set_len(state.len);
            Err(io_error(err))
        }
    }
}

/// Rewrites the log with one upsert per live vector, and reopens it.
fn compact(ctx: &Context, collection: &Collection, state: &mut State) -> Result<(), Error> {
    let mut out = header(collection.info.dimension as u32, collection.info.distance);
    for (index, (id, doc)) in state.docs.iter().enumerate() {
        if index % CONTEXT_CHECK_INTERVAL == 0 {
            check_context(ctx)?;
        }
        let payload = upsert_payload(id, doc);
        put_u32(&mut out, payload.len());
        out.extend_from_slice(&payload);
    }
    check_context(ctx)?;
    let path = collection.dir.join(LOG_FILE);
    let tmp = collection.dir.join(format!("{LOG_FILE}.tmp"));
    let result = (|| -> io::Result<File> {
        let mut file = File::create(&tmp)?;
        file.write_all(&out)?;
        file.sync_all()?;
        // The handle is opened before the rename, so the log stays writable
        // even if the reopen would fail after it.
        let file = OpenOptions::new().append(true).open(&tmp)?;
        fs::rename(&tmp, &path)?;
        sync_dir(&collection.dir)?;
        Ok(file)
    })();
    match result {
        Ok(file) => {
            state.file = file;
            state.len = out.len() as u64;
            Ok(())
        }
        Err(err) => {
            // Ignored: the log is still the old one, so the leftover is only clutter.
            let _ = fs::remove_file(&tmp);
            Err(io_error(err))
        }
    }
}

/// Go's `hasCategories`: the document has every category asked for.
fn has_categories(doc_categories: &[String], wanted: &[String]) -> bool {
    wanted
        .iter()
        .all(|wanted| doc_categories.iter().any(|category| category == wanted))
}

/// xvec's cosine distance, `1 - cos`, with its rules for zero vectors.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut inner = 0.0_f32;
    let mut a_norm = 0.0_f32;
    let mut b_norm = 0.0_f32;
    for (x, y) in a.iter().zip(b) {
        inner += x * y;
        a_norm += x * x;
        b_norm += y * y;
    }
    if a_norm == 0.0 && b_norm == 0.0 {
        return 0.0;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        return 1.0;
    }
    let cosine = inner / (a_norm.sqrt() * b_norm.sqrt());
    1.0 - cosine.clamp(-1.0, 1.0)
}

/// The score of a dense vector against the query, higher is better.
fn dense_score(distance: Distance, query: &[f32], values: &[f32]) -> f32 {
    match distance {
        Distance::Cosine => -cosine_distance(query, values),
        Distance::Euclidean => -query
            .iter()
            .zip(values)
            .map(|(x, y)| {
                let d = x - y;
                d * d
            })
            .sum::<f32>(),
        Distance::Dot => query.iter().zip(values).map(|(x, y)| x * y).sum(),
    }
}

/// The inner product of a sparse vector with the query.
fn sparse_dot(query: &HashMap<u32, f32>, indices: &[u32], values: &[f32]) -> f32 {
    indices
        .iter()
        .zip(values)
        .map(|(index, value)| query.get(index).map_or(0.0, |q| q * value))
        .sum()
}

/// The vector a document returns as. The timestamp is the caller's choice,
/// because Go's `GetVectors` and `QueryVectors` set it differently.
fn vector_from(id: &str, doc: &Doc, timestamp: Time) -> Vector {
    let mut vector = Vector {
        id: id.to_owned(),
        is_hidden: doc.hidden,
        categories: doc.categories.clone(),
        timestamp,
        ..Default::default()
    };
    match &doc.values {
        Values::Dense(values) => vector.values = values.clone(),
        Values::Sparse { indices, values } => {
            vector.indices = indices.clone();
            vector.values = values.clone();
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::{open, suite};
    use gorse_common::gotime::{HOUR, MILLISECOND};
    use gorse_storage_core::options::Opt;
    use gorse_storage_core::scheme::XVEC_PREFIX;

    /// Go's `XvecTestSuite`: each case runs on a new store with table prefix `gorse_`.
    async fn run(case: impl for<'a> AsyncFnOnce(&'a Context, &'a dyn Database)) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = open(
            &format!("{XVEC_PREFIX}{}", dir.path().display()),
            "gorse_",
            &[] as &[Opt],
        )
        .await
        .expect("open");
        db.init().await.expect("init");
        case(&Context::background(), db.as_ref()).await;
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn purge() {
        run(suite::test_purge).await;
    }

    #[tokio::test]
    async fn collections() {
        run(suite::test_collections).await;
    }

    #[tokio::test]
    async fn vectors() {
        run(suite::test_vectors).await;
    }

    #[tokio::test]
    async fn half_vectors() {
        run(suite::test_half_vectors).await;
    }

    #[tokio::test]
    async fn get_vectors() {
        run(suite::test_get_vectors).await;
    }

    #[tokio::test]
    async fn sparse() {
        run(suite::test_sparse).await;
    }

    #[tokio::test]
    async fn hidden() {
        run(suite::test_hidden).await;
    }

    #[tokio::test]
    async fn dot() {
        run(suite::test_dot).await;
    }

    #[tokio::test]
    async fn delete_vectors() {
        run(suite::test_delete_vectors).await;
    }

    /// The suite's quantization case with the one quantization xvec accepts.
    #[tokio::test]
    async fn quantization_none() {
        run(async |ctx, db| suite::test_quantization(ctx, db, QuantizationType::None, 0).await)
            .await;
    }

    /// A store on `dir`, initialised.
    async fn store_at(dir: &Path) -> Box<dyn Database> {
        let db = open(
            &format!("{XVEC_PREFIX}{}", dir.display()),
            "gorse_",
            &[] as &[Opt],
        )
        .await
        .expect("open");
        db.init().await.expect("init");
        db
    }

    fn dense(id: &str, values: &[f32], timestamp: Time) -> Vector {
        Vector {
            id: id.to_owned(),
            values: values.to_vec(),
            categories: vec!["common".to_owned()],
            timestamp,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn reopen_keeps_collections_and_vectors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let timestamp = gotime::now().utc().truncate(MILLISECOND);
        let before = {
            let db = store_at(dir.path()).await;
            db.add_collection(&ctx, "test", 4, Distance::Cosine, VectorConfig::default())
                .await
                .expect("add");
            db.add_vectors(
                &ctx,
                "test",
                &[
                    dense("a", &[1.0, 0.0, 0.0, 0.0], timestamp.clone()),
                    dense("b", &[0.9, 0.1, 0.0, 0.0], timestamp.clone()),
                ],
            )
            .await
            .expect("add vectors");
            let results = db
                .query_vectors(
                    &ctx,
                    "test",
                    &dense("", &[1.0, 0.0, 0.0, 0.0], timestamp.clone()),
                    &[],
                    10,
                )
                .await
                .expect("query");
            db.close().await.expect("close");
            results
        };

        let db = store_at(dir.path()).await;
        assert_eq!(db.list_collections(&ctx).await.expect("list"), ["test"]);
        assert_eq!(db.count_vectors(&ctx, "test").await.expect("count"), 2);
        let after = db
            .query_vectors(
                &ctx,
                "test",
                &dense("", &[1.0, 0.0, 0.0, 0.0], timestamp.clone()),
                &[],
                10,
            )
            .await
            .expect("query");
        assert_eq!(after, before);
        let stored = db.get_vectors(&ctx, "test", &["a"]).await.expect("get");
        assert_eq!(stored[0].timestamp, timestamp);
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn incomplete_record_at_the_end_is_dropped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let timestamp = gotime::now().utc().truncate(MILLISECOND);
        let db = store_at(dir.path()).await;
        db.add_collection(&ctx, "test", 4, Distance::Cosine, VectorConfig::default())
            .await
            .expect("add");
        db.add_vectors(
            &ctx,
            "test",
            &[dense("a", &[1.0, 0.0, 0.0, 0.0], timestamp.clone())],
        )
        .await
        .expect("add");
        db.close().await.expect("close");

        let log = dir.path().join("gorse_test").join(LOG_FILE);
        let intact = fs::metadata(&log).expect("metadata").len();
        // A record that claims 100 bytes and has 3 of them.
        let mut torn = 100u32.to_le_bytes().to_vec();
        torn.extend_from_slice(&[1, 2, 3]);
        OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open")
            .write_all(&torn)
            .expect("write");

        let db = store_at(dir.path()).await;
        assert_eq!(db.count_vectors(&ctx, "test").await.expect("count"), 1);
        assert_eq!(fs::metadata(&log).expect("metadata").len(), intact);
        db.add_vectors(
            &ctx,
            "test",
            &[dense("b", &[0.0, 1.0, 0.0, 0.0], timestamp)],
        )
        .await
        .expect("add");
        db.close().await.expect("close");

        let db = store_at(dir.path()).await;
        assert_eq!(db.count_vectors(&ctx, "test").await.expect("count"), 2);
        db.close().await.expect("close");
    }

    /// A delete is replayed from the log when the store is opened again.
    #[tokio::test]
    async fn delete_survives_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let cutoff = gotime::now().utc().truncate(MILLISECOND);
        let db = store_at(dir.path()).await;
        db.add_collection(&ctx, "test", 4, Distance::Cosine, VectorConfig::default())
            .await
            .expect("add");
        db.add_vectors(
            &ctx,
            "test",
            &[
                dense(
                    "old",
                    &[1.0, 0.0, 0.0, 0.0],
                    cutoff.add(gotime::Duration(-HOUR.0)),
                ),
                dense("at", &[0.0, 1.0, 0.0, 0.0], cutoff.clone()),
                dense(
                    "new",
                    &[0.0, 0.0, 1.0, 0.0],
                    cutoff.add(gotime::Duration(HOUR.0)),
                ),
            ],
        )
        .await
        .expect("add vectors");
        db.delete_vectors(&ctx, "test", &cutoff)
            .await
            .expect("delete");
        db.close().await.expect("close");

        let db = store_at(dir.path()).await;
        assert_eq!(db.count_vectors(&ctx, "test").await.expect("count"), 2);
        let ids: Vec<String> = db
            .get_vectors(&ctx, "test", &["old", "at", "new"])
            .await
            .expect("get")
            .into_iter()
            .map(|vector| vector.id)
            .collect();
        assert_eq!(ids, ["at", "new"]);
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn optimize_rewrites_the_log_and_keeps_the_vectors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let cutoff = gotime::now().utc().truncate(MILLISECOND);
        let db = store_at(dir.path()).await;
        db.add_collection(&ctx, "test", 4, Distance::Dot, VectorConfig::default())
            .await
            .expect("add");
        db.add_vectors(
            &ctx,
            "test",
            &[
                dense(
                    "old",
                    &[1.0, 0.0, 0.0, 0.0],
                    cutoff.add(gotime::Duration(-HOUR.0)),
                ),
                dense("new", &[0.0, 1.0, 0.0, 0.0], cutoff.clone()),
                dense(
                    "newer",
                    &[0.0, 0.0, 1.0, 0.0],
                    cutoff.add(gotime::Duration(MILLISECOND.0)),
                ),
            ],
        )
        .await
        .expect("add vectors");
        db.delete_vectors(&ctx, "test", &cutoff)
            .await
            .expect("delete");
        let log = dir.path().join("gorse_test").join(LOG_FILE);
        let before = fs::metadata(&log).expect("metadata").len();

        db.optimize(&ctx, "test").await.expect("optimize");
        assert!(fs::metadata(&log).expect("metadata").len() < before);
        assert!(
            !dir.path()
                .join("gorse_test")
                .join(format!("{LOG_FILE}.tmp"))
                .exists()
        );
        db.close().await.expect("close");

        let db = store_at(dir.path()).await;
        assert_eq!(db.count_vectors(&ctx, "test").await.expect("count"), 2);
        let ids: Vec<String> = db
            .get_vectors(&ctx, "test", &["new", "newer", "old"])
            .await
            .expect("get")
            .into_iter()
            .map(|vector| vector.id)
            .collect();
        assert_eq!(ids, ["new", "newer"]);
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn scores_follow_xvec_metrics() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let timestamp = Time::default();
        let db = store_at(dir.path()).await;
        db.add_collection(
            &ctx,
            "euclidean",
            4,
            Distance::Euclidean,
            VectorConfig::default(),
        )
        .await
        .expect("add");
        db.add_collection(&ctx, "cosine", 4, Distance::Cosine, VectorConfig::default())
            .await
            .expect("add");
        db.add_collection(&ctx, "dot", 4, Distance::Dot, VectorConfig::default())
            .await
            .expect("add");
        db.add_vectors(
            &ctx,
            "euclidean",
            &[dense("a", &[3.0, 4.0, 0.0, 0.0], timestamp.clone())],
        )
        .await
        .expect("add");
        db.add_vectors(
            &ctx,
            "cosine",
            &[dense("a", &[1.0, 1.0, 0.0, 0.0], timestamp.clone())],
        )
        .await
        .expect("add");
        db.add_vectors(
            &ctx,
            "dot",
            &[dense("a", &[1.0, 1.0, 0.0, 0.0], timestamp.clone())],
        )
        .await
        .expect("add");

        let zero = dense("", &[0.0; 4], timestamp.clone());
        let score = db
            .query_vectors(&ctx, "euclidean", &zero, &[], 1)
            .await
            .expect("query")[0]
            .score;
        assert_eq!(score, -25.0);

        let unit = dense("", &[1.0, 0.0, 0.0, 0.0], timestamp.clone());
        let score = db
            .query_vectors(&ctx, "cosine", &unit, &[], 1)
            .await
            .expect("query")[0]
            .score;
        assert!(
            (score - (std::f32::consts::FRAC_1_SQRT_2 - 1.0)).abs() < 1e-6,
            "{score}"
        );

        let two_three = dense("", &[2.0, 3.0, 0.0, 0.0], timestamp);
        let score = db
            .query_vectors(&ctx, "dot", &two_three, &[], 1)
            .await
            .expect("query")[0]
            .score;
        assert_eq!(score, 5.0);
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn unsupported_collections_are_rejected() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let db = store_at(dir.path()).await;

        let sq = VectorConfig {
            quantization: QuantizationType::Sq,
            bits: 0,
        };
        let err = db
            .add_collection(&ctx, "sq", 4, Distance::Cosine, sq)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotSupported));
        assert_eq!(
            err.to_string(),
            "quantization type sq for xvec not supported"
        );

        let err = db
            .add_collection(&ctx, "sparse", 0, Distance::Cosine, VectorConfig::default())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::NotSupported));

        assert!(
            db.add_collection(&ctx, "negative", -1, Distance::Dot, VectorConfig::default())
                .await
                .is_err()
        );
        for name in ["", ".", "..", "a/b", "a\\b"] {
            assert!(
                db.add_collection(&ctx, name, 4, Distance::Dot, VectorConfig::default())
                    .await
                    .is_err(),
                "{name:?}"
            );
        }
        assert_eq!(
            db.list_collections(&ctx).await.expect("list"),
            Vec::<String>::new()
        );
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn wrong_kind_or_dimension_is_rejected_without_writing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let timestamp = Time::default();
        let db = store_at(dir.path()).await;
        db.add_collection(&ctx, "dense", 4, Distance::Cosine, VectorConfig::default())
            .await
            .expect("add");
        db.add_collection(&ctx, "sparse", 0, Distance::Dot, VectorConfig::default())
            .await
            .expect("add");

        let short = dense("short", &[1.0, 0.0, 0.0], timestamp.clone());
        assert!(
            db.add_vectors(
                &ctx,
                "dense",
                &[dense("ok", &[1.0; 4], timestamp.clone()), short]
            )
            .await
            .is_err()
        );
        assert_eq!(
            db.count_vectors(&ctx, "dense").await.expect("count"),
            0,
            "a failed batch writes nothing"
        );

        let sparse = Vector {
            id: "s".to_owned(),
            indices: vec![1],
            values: vec![1.0],
            ..Default::default()
        };
        assert!(
            db.add_vectors(&ctx, "dense", std::slice::from_ref(&sparse))
                .await
                .is_err()
        );
        assert!(
            db.add_vectors(&ctx, "sparse", &[dense("d", &[1.0; 4], timestamp.clone())])
                .await
                .is_err()
        );
        assert!(
            db.query_vectors(&ctx, "dense", &sparse, &[], 1)
                .await
                .is_err()
        );
        assert!(
            db.query_vectors(&ctx, "sparse", &dense("", &[1.0; 4], timestamp), &[], 1)
                .await
                .is_err()
        );
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn foreign_directories_are_skipped_and_replaced() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let foreign = dir.path().join("gorse_test");
        fs::create_dir_all(&foreign).expect("create");
        fs::write(foreign.join("CURRENT"), b"not ours").expect("write");

        let db = store_at(dir.path()).await;
        assert!(db.list_collections(&ctx).await.expect("list").is_empty());
        db.add_collection(&ctx, "test", 4, Distance::Cosine, VectorConfig::default())
            .await
            .expect("replaces the foreign directory");
        assert!(!foreign.join("CURRENT").exists());
        let err = db
            .add_collection(&ctx, "test", 4, Distance::Cosine, VectorConfig::default())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), Some(StorageError::AlreadyExists));
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn empty_requests_do_not_need_a_collection() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let db = store_at(dir.path()).await;
        assert!(db.add_vectors(&ctx, "missing", &[]).await.is_ok());
        assert!(
            db.get_vectors(&ctx, "missing", &[])
                .await
                .expect("get")
                .is_empty()
        );
        assert!(
            db.query_vectors(&ctx, "missing", &Vector::default(), &[], 0)
                .await
                .expect("query")
                .is_empty()
        );
        assert_eq!(
            db.count_vectors(&ctx, "missing").await.unwrap_err().kind(),
            Some(StorageError::NotFound)
        );
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn closed_store_refuses_calls_and_closes_again() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Context::background();
        let db = store_at(dir.path()).await;
        db.close().await.expect("close");
        db.close().await.expect("a second close succeeds");
        assert_eq!(
            db.list_collections(&ctx).await.unwrap_err().to_string(),
            "xvec database is closed"
        );
        assert_eq!(
            db.init().await.unwrap_err().to_string(),
            "xvec database is closed"
        );
    }

    #[tokio::test]
    async fn cancelled_context_stops_calls() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = store_at(dir.path()).await;
        let (ctx, cancel) = Context::background().with_cancel();
        cancel.cancel();
        assert_eq!(
            db.list_collections(&ctx).await.unwrap_err(),
            Error::Context(gorse_common::gocontext::ContextError::Canceled)
        );
        db.close().await.expect("close");
    }

    #[tokio::test]
    async fn empty_root_is_rejected() {
        assert!(open(XVEC_PREFIX, "", &[] as &[Opt]).await.is_err());
    }
}
