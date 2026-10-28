// Ports: logics/vector_writer.go @ add21b5

//! The writer that batches vectors into one collection of a vector store for
//! the item-to-item and user-to-user recommenders (Go `VectorWriter`), and the
//! sparse vectors those recommenders build from feedback and tags
//! ([`new_sparse_vector`], [`append_sparse_vector`]).
//!
//! Go's `container/heap` and `sort.Slice` are ported step for step, so that
//! entries the pruner ties come out in Go's order, including NaN values, which
//! break the heap's ordering.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gorse_common::gocontext::Context;
use gorse_common::gosort;
use gorse_common::gotime::Time;
use gorse_storage::vectors::{self, Database, Distance, Vector, VectorConfig};
use gorse_storage_core::errors::Error as StorageError;
use tokio::sync::Mutex;

/// Go's `defaultSimilarityVectorBatchSize`.
const DEFAULT_BATCH_SIZE: usize = 1024;

/// Go's `maxSparseVectorNNZ`: the most entries a sparse vector keeps.
pub const MAX_SPARSE_VECTOR_NNZ: usize = 1024;

/// Go's `sparseVectorEntry`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SparseEntry {
    index: u32,
    value: f32,
}

/// Go's `sparseVectorPruner`: keeps the `max_nnz` entries with the highest
/// values, and of equal values the lowest indices. The entries sit in a
/// min-heap, so the weakest kept entry is at the root.
struct SparseVectorPruner {
    entries: Vec<SparseEntry>,
    max_nnz: usize,
}

impl SparseVectorPruner {
    /// Go's `newSparseVectorPruner`.
    fn new(max_nnz: usize) -> Self {
        Self {
            entries: Vec::with_capacity(max_nnz),
            max_nnz,
        }
    }

    /// Go's `sparseVectorHeap.Less`: a smaller value first, and of equal
    /// values the larger index first.
    fn less(&self, i: usize, j: usize) -> bool {
        let (a, b) = (self.entries[i], self.entries[j]);
        if a.value == b.value {
            a.index > b.index
        } else {
            a.value < b.value
        }
    }

    /// Go's `container/heap` `up`: sifts the entry at `j` towards the root.
    fn up(&mut self, mut j: usize) {
        while j > 0 {
            let i = (j - 1) / 2;
            if !self.less(j, i) {
                break;
            }
            self.entries.swap(i, j);
            j = i;
        }
    }

    /// Go's `container/heap` `down`: sifts the entry at `i0` down within the
    /// first `n` entries. Reports whether the entry moved.
    fn down(&mut self, i0: usize, n: usize) -> bool {
        let mut i = i0;
        loop {
            let j1 = 2 * i + 1;
            if j1 >= n {
                break;
            }
            let mut j = j1;
            let j2 = j1 + 1;
            if j2 < n && self.less(j2, j1) {
                j = j2;
            }
            if !self.less(j, i) {
                break;
            }
            self.entries.swap(i, j);
            i = j;
        }
        i > i0
    }

    /// Go's `Add`.
    fn add(&mut self, index: u32, value: f32) {
        if self.max_nnz == 0 {
            return;
        }
        let entry = SparseEntry { index, value };
        if self.entries.len() < self.max_nnz {
            // Go's `heap.Push`.
            self.entries.push(entry);
            let n = self.entries.len() - 1;
            self.up(n);
            return;
        }
        let root = self.entries[0];
        if entry.value > root.value || (entry.value == root.value && entry.index < root.index) {
            // Go's `heap.Fix(&p.entries, 0)` after the root is replaced.
            self.entries[0] = entry;
            let n = self.entries.len();
            if !self.down(0, n) {
                self.up(0);
            }
        }
    }

    /// Go's `Result`: the kept indices in ascending order, with their values.
    fn result(&self) -> (Vec<u32>, Vec<f32>) {
        let mut entries = ByIndex(self.entries.clone());
        gosort::sort(&mut entries);
        entries.0.iter().map(|e| (e.index, e.value)).unzip()
    }
}

/// The entries ordered by index, for [`gosort::sort`] (Go's `sort.Slice` with
/// the index comparison).
struct ByIndex(Vec<SparseEntry>);

impl gosort::Interface for ByIndex {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.0[i].index < self.0[j].index
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}

/// Go's `newSparseVector`: a sparse vector of the ids whose idf is positive,
/// with each id's index at `offset` plus the id.
pub fn new_sparse_vector(ids: &[i32], idf: &[f32], offset: u32) -> Vector {
    append_sparse_vector(Vector::default(), ids, idf, offset)
}

/// Go's `appendSparseVector`: adds to `vector`'s entries the ids whose idf is
/// positive and whose index is not taken yet, then keeps the
/// [`MAX_SPARSE_VECTOR_NNZ`] entries with the highest values. An id's entry
/// has the square root of its idf as its value.
///
/// # Panics
///
/// When `vector` holds more indices than values, as Go indexes past the end
/// of its values.
pub fn append_sparse_vector(mut vector: Vector, ids: &[i32], idf: &[f32], offset: u32) -> Vector {
    let mut pruner = SparseVectorPruner::new(MAX_SPARSE_VECTOR_NNZ);
    let mut seen: HashSet<u32> = vector.indices.iter().copied().collect();
    for (i, &index) in vector.indices.iter().enumerate() {
        pruner.add(index, vector.values[i]);
    }
    for &id in ids {
        let Ok(position) = usize::try_from(id) else {
            continue;
        };
        let Some(&weight) = idf.get(position) else {
            continue;
        };
        if weight <= 0.0 {
            continue;
        }
        let index = offset.wrapping_add(id as u32);
        if !seen.insert(index) {
            continue;
        }
        pruner.add(index, (weight as f64).sqrt() as f32);
    }
    let (indices, values) = pruner.result();
    vector.indices = indices;
    vector.values = values;
    vector
}

/// Go's `VectorWriter`: writes vectors into one collection of a vector store,
/// in batches. Safe to share between tasks; each call takes the lock, as Go's
/// methods do.
pub struct VectorWriter {
    ctx: Context,
    client: Option<Arc<dyn Database>>,
    collection: String,
    distance: Distance,
    vector_config: VectorConfig,
    timestamp: Time,
    batch_size: usize,
    sparse: bool,
    state: Mutex<WriterState>,
}

/// What [`VectorWriter`] guards with its lock.
struct WriterState {
    /// The dimension of a dense collection, fixed by the first flush; the
    /// dimension of a sparse collection is 0.
    dimension: Option<i64>,
    collection_exists: bool,
    buffer: Vec<Vector>,
}

/// Go's `newSimilarityVectorWriter`. A `None` context is the background
/// context, and a batch size of 0 or less is the default batch size. A sparse
/// writer takes the default vector configuration whatever `vector_config` is.
#[allow(clippy::too_many_arguments)] // Go's signature.
pub fn new_similarity_vector_writer(
    ctx: Option<Context>,
    client: Option<Arc<dyn Database>>,
    collection: &str,
    distance: Distance,
    vector_config: VectorConfig,
    timestamp: Time,
    batch_size: i64,
    sparse: bool,
) -> VectorWriter {
    let batch_size = usize::try_from(batch_size)
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_BATCH_SIZE);
    let (dimension, vector_config) = if sparse {
        (Some(0), VectorConfig::default())
    } else {
        (None, vector_config)
    };
    VectorWriter {
        ctx: ctx.unwrap_or_else(Context::background),
        client,
        collection: collection.to_owned(),
        distance,
        vector_config,
        timestamp,
        batch_size,
        sparse,
        state: Mutex::new(WriterState {
            dimension,
            collection_exists: false,
            buffer: Vec::new(),
        }),
    }
}

/// Go's `fmt.Errorf("vector database %w", storage.ErrNoDatabase)`.
fn no_database() -> vectors::Error {
    vectors::Error::Storage {
        kind: StorageError::NoDatabase,
        message: format!("vector database {}", StorageError::NoDatabase),
    }
}

impl VectorWriter {
    /// Go's `Add`: buffers the vector when it suits the writer (a sparse
    /// writer takes sparse vectors, a dense one takes dense vectors with no
    /// indices), and flushes once the buffer holds a batch.
    ///
    /// # Errors
    ///
    /// Fails when a flush fails.
    pub async fn add(&self, vector: Vector) -> Result<(), vectors::Error> {
        let mut state = self.state.lock().await;
        if !vector.h_values.is_empty() {
            if self.sparse {
                return Ok(());
            }
        } else if self.sparse {
            if vector.indices.is_empty() || vector.indices.len() != vector.values.len() {
                return Ok(());
            }
        } else if !vector.indices.is_empty() || vector.is_empty() {
            return Ok(());
        }
        state.buffer.push(vector);
        if state.buffer.len() >= self.batch_size {
            return self.flush_locked(&mut state).await;
        }
        Ok(())
    }

    /// Go's `Clean`: flushes the buffer, deletes the vectors written before
    /// the writer's timestamp, and optimizes the collection. A collection
    /// that does not exist has nothing to delete.
    ///
    /// # Errors
    ///
    /// Fails when a flush, the delete or the optimize fails, or when the
    /// writer has no store (Go panics there, `PORTING_NOTES.md` X-WP34-3).
    pub async fn clean(&self) -> Result<(), vectors::Error> {
        let mut state = self.state.lock().await;
        self.flush_locked(&mut state).await?;
        let client = self.client.as_deref().ok_or_else(no_database)?;
        match client
            .delete_vectors(&self.ctx, &self.collection, &self.timestamp)
            .await
        {
            Ok(()) => {}
            Err(err) if err.kind() == Some(StorageError::NotFound) => return Ok(()),
            Err(err) => return Err(err),
        }
        client.optimize(&self.ctx, &self.collection).await
    }

    /// Go's `ensureCollectionLocked`: creates the collection with the flushed
    /// dimension, or recreates it when its dimension, distance or
    /// configuration differ.
    async fn ensure_collection_locked(
        &self,
        state: &mut WriterState,
    ) -> Result<(), vectors::Error> {
        if state.collection_exists {
            return Ok(());
        }
        let client = self.client.as_deref().ok_or_else(no_database)?;
        let dimension = state
            .dimension
            .expect("the dimension is set before the collection is ensured");
        let mut info = match client
            .describe_collection(&self.ctx, &self.collection)
            .await
        {
            Ok(info) => Some(info),
            Err(err) if err.kind() == Some(StorageError::NotFound) => None,
            Err(err) => return Err(err),
        };
        let stale = info.as_ref().is_some_and(|info| {
            info.dimension != dimension
                || info.distance != self.distance
                || info.config.quantization != self.vector_config.quantization
                || info.config.bits != self.vector_config.bits
        });
        if stale {
            tracing::warn!(
                collection = %self.collection,
                dimension,
                distance = self.distance as i64,
                "recreating similarity vector collection"
            );
            match client.delete_collection(&self.ctx, &self.collection).await {
                Ok(()) => {}
                Err(err) if err.kind() == Some(StorageError::NotFound) => {}
                Err(err) => return Err(err),
            }
            info = None;
        }
        if info.is_none() {
            client
                .add_collection(
                    &self.ctx,
                    &self.collection,
                    dimension,
                    self.distance,
                    self.vector_config.clone(),
                )
                .await?;
        }
        state.collection_exists = true;
        Ok(())
    }

    /// Go's `flushLocked`. A dense buffer first keeps the vectors of the
    /// dimension that most vectors have (the first flush fixes it), and logs
    /// and drops the rest. A failed write leaves the buffer as it is.
    async fn flush_locked(&self, state: &mut WriterState) -> Result<(), vectors::Error> {
        if state.buffer.is_empty() {
            return Ok(());
        }
        if !self.sparse {
            let dimension = match state.dimension {
                Some(dimension) => dimension,
                None => {
                    let dimension = mode_dimension(&state.buffer);
                    state.dimension = Some(dimension);
                    dimension
                }
            };
            state.buffer.retain(|vector| {
                if vector.len() as i64 == dimension {
                    return true;
                }
                tracing::error!(
                    collection = %self.collection,
                    id = %vector.id,
                    dimension = vector.len(),
                    expected_dimension = dimension,
                    "invalid similarity vector dimension"
                );
                false
            });
        }
        self.ensure_collection_locked(state).await?;
        let client = self.client.as_deref().ok_or_else(no_database)?;
        client
            .add_vectors(&self.ctx, &self.collection, &state.buffer)
            .await?;
        state.buffer.clear();
        Ok(())
    }
}

/// The dimension that most vectors in `buffer` have. Ties go to the one
/// reached first in the buffer, as Go's loop over the buffer chooses it.
fn mode_dimension(buffer: &[Vector]) -> i64 {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for vector in buffer {
        *counts.entry(vector.len()).or_default() += 1;
    }
    let mut dimension = buffer[0].len();
    for vector in buffer {
        let candidate = vector.len();
        if counts[&candidate] > counts[&dimension] {
            dimension = candidate;
        }
    }
    dimension as i64
}

#[cfg(test)]
mod tests {
    // Ports: logics/vector_writer_test.go @ add21b5
    use super::*;

    #[test]
    fn test_sparse_vector_pruner_keeps_highest_values() {
        let mut pruner = SparseVectorPruner::new(MAX_SPARSE_VECTOR_NNZ);
        for i in 0..=MAX_SPARSE_VECTOR_NNZ {
            pruner.add(i as u32, (i + 1) as f32);
        }

        let (indices, values) = pruner.result();
        assert_eq!(indices.len(), MAX_SPARSE_VECTOR_NNZ, "indices");
        assert_eq!(values.len(), MAX_SPARSE_VECTOR_NNZ, "values");
        for i in 0..indices.len() {
            assert_eq!(indices[i], (i + 1) as u32, "index {i}");
            assert_eq!(values[i], (i + 2) as f32, "value {i}");
        }
    }

    #[test]
    fn test_sparse_vector_pruner_breaks_ties_by_index() {
        let mut pruner = SparseVectorPruner::new(2);
        pruner.add(3, 1.0);
        pruner.add(2, 1.0);
        pruner.add(1, 1.0);

        let (indices, values) = pruner.result();
        assert_eq!(indices, vec![1, 2]);
        assert_eq!(values, vec![1.0, 1.0]);
    }

    #[test]
    fn test_new_sparse_vector_prunes_to_max_nnz() {
        let ids: Vec<i32> = (0..=MAX_SPARSE_VECTOR_NNZ as i32).collect();
        let idf: Vec<f32> = (0..ids.len()).map(|i| (i + 1) as f32).collect();

        let vector = new_sparse_vector(&ids, &idf, 0);

        assert_eq!(vector.indices.len(), MAX_SPARSE_VECTOR_NNZ, "indices");
        assert_eq!(vector.values.len(), MAX_SPARSE_VECTOR_NNZ, "values");
        for i in 0..vector.indices.len() {
            assert_eq!(vector.indices[i], (i + 1) as u32, "index {i}");
            assert!(
                (f64::from(vector.values[i]) - ((i + 2) as f64).sqrt()).abs() <= 1e-6,
                "value {i}: expected sqrt({}), got {}",
                i + 2,
                vector.values[i]
            );
        }
    }

    #[test]
    fn test_append_sparse_vector_prunes_combined_vector_to_max_nnz() {
        let first_ids: Vec<i32> = (0..MAX_SPARSE_VECTOR_NNZ as i32).collect();
        let first_idf = vec![1.0; first_ids.len()];
        let second_ids = first_ids.clone();
        let second_idf = vec![2.0; second_ids.len()];

        let vector = new_sparse_vector(&first_ids, &first_idf, 0);
        let vector = append_sparse_vector(vector, &second_ids, &second_idf, first_idf.len() as u32);

        assert_eq!(vector.indices.len(), MAX_SPARSE_VECTOR_NNZ, "indices");
        for i in 0..vector.indices.len() {
            let expected = (first_idf.len() + i) as u32;
            assert_eq!(vector.indices[i], expected, "index {i}");
            assert!(
                (f64::from(vector.values[i]) - 2f64.sqrt()).abs() <= 1e-6,
                "value {i}: expected sqrt(2), got {}",
                vector.values[i]
            );
        }
    }

    /// One case of `testdata/sparse_oracle.json`: 400 pruner cases and 20
    /// `appendSparseVector` cases, made by Go on seeded random inputs that
    /// include NaN, infinities, ties and duplicate indices. Values are FP32
    /// bits, so NaN payloads compare exactly. The generator is
    /// `contracts/tools/logicsgolden/sparse_oracle_test.go.txt`.
    #[derive(serde::Deserialize)]
    struct OracleCase {
        kind: String,
        #[serde(default)]
        max_nnz: usize,
        #[serde(default)]
        adds: Vec<OracleAdd>,
        #[serde(default)]
        in_indices: Vec<u32>,
        #[serde(default)]
        in_values: Vec<u32>,
        #[serde(default)]
        ids: Vec<i32>,
        #[serde(default)]
        idf: Vec<u32>,
        #[serde(default)]
        offset: u32,
        indices: Vec<u32>,
        values: Vec<u32>,
    }

    #[derive(serde::Deserialize)]
    struct OracleAdd {
        index: u32,
        value: u32,
    }

    fn floats(bits: &[u32]) -> Vec<f32> {
        bits.iter().map(|&b| f32::from_bits(b)).collect()
    }

    /// The pruner and `appendSparseVector` agree with Go bit for bit on every
    /// case, including the order of ties and the handling of NaN.
    #[test]
    fn test_sparse_vectors_match_go() {
        let cases: Vec<OracleCase> =
            serde_json::from_str(include_str!("../testdata/sparse_oracle.json"))
                .expect("sparse_oracle.json");
        assert_eq!(cases.len(), 420, "cases");
        for (n, case) in cases.iter().enumerate() {
            let (indices, values) = match case.kind.as_str() {
                "pruner" => {
                    let mut pruner = SparseVectorPruner::new(case.max_nnz);
                    for add in &case.adds {
                        pruner.add(add.index, f32::from_bits(add.value));
                    }
                    pruner.result()
                }
                "append" => {
                    let vector = Vector {
                        indices: case.in_indices.clone(),
                        values: floats(&case.in_values),
                        ..Vector::default()
                    };
                    let out =
                        append_sparse_vector(vector, &case.ids, &floats(&case.idf), case.offset);
                    (out.indices, out.values)
                }
                other => panic!("case {n}: unknown kind {other}"),
            };
            let bits: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
            assert_eq!(indices, case.indices, "case {n} indices");
            assert_eq!(bits, case.values, "case {n} values");
        }
    }

    /// The writer on the real `xvec://` store: a batch flushes when full, the
    /// first flush fixes the dimension and creates the collection, vectors of
    /// another dimension are dropped, a writer of another dimension recreates
    /// the collection, and `clean` deletes what was written before its
    /// timestamp.
    #[tokio::test]
    async fn test_vector_writer_batches_and_cleans() {
        let dir = tempfile::tempdir().expect("temp dir");
        let uri = format!("xvec://{}", dir.path().display());
        let db: Arc<dyn Database> =
            Arc::from(vectors::open(&uri, "", &[]).await.expect("open the store"));
        let ctx = Context::background();
        let dense = |id: &str, dim: usize| Vector {
            id: id.to_owned(),
            values: vec![1.0; dim],
            ..Vector::default()
        };
        let writer = |batch_size: i64, timestamp: Time| {
            new_similarity_vector_writer(
                None,
                Some(db.clone()),
                "items",
                Distance::Dot,
                VectorConfig::default(),
                timestamp,
                batch_size,
                false,
            )
        };

        let batched = writer(2, Time::default());
        // Nothing is written until the batch is full.
        batched.add(dense("a", 3)).await.expect("add a");
        batched.add(dense("b", 3)).await.expect("add b");
        assert_eq!(db.count_vectors(&ctx, "items").await.expect("count"), 2);
        batched.add(dense("c", 3)).await.expect("add c");
        // The batch holds c and d; d has the wrong dimension and is dropped.
        batched.add(dense("d", 2)).await.expect("add d");
        assert_eq!(db.count_vectors(&ctx, "items").await.expect("count"), 3);

        // A writer of dimension 4 recreates the collection of dimension 3.
        writer(1, Time::default())
            .add(dense("e", 4))
            .await
            .expect("add e");
        assert_eq!(db.count_vectors(&ctx, "items").await.expect("count"), 1);

        // Clean deletes the vectors written before its timestamp, which the
        // zero time of these vectors is.
        writer(1024, gorse_common::gotime::now())
            .clean()
            .await
            .expect("clean");
        assert_eq!(db.count_vectors(&ctx, "items").await.expect("count"), 0);

        // A collection that does not exist has nothing to delete.
        new_similarity_vector_writer(
            None,
            Some(db.clone()),
            "missing",
            Distance::Dot,
            VectorConfig::default(),
            gorse_common::gotime::now(),
            1024,
            false,
        )
        .clean()
        .await
        .expect("clean a collection that does not exist");
    }
}
