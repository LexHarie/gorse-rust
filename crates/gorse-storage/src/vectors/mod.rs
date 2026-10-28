// Ports: storage/vectors/database.go, storage/vectors/no_database.go @ add21b5

//! The vector store API of Go's `storage/vectors` package: collections of
//! dense (FP16) or sparse (FP32) vectors, each with categories, a hidden flag
//! and a timestamp, and the [`Database`] trait every store implements.
//!
//! The `xvec://` store is ported as Rust's own file format ([`xvec`], see
//! `PORTING.md` D-6). Qdrant, Milvus and Weaviate arrive with WP-2-9.

pub mod xvec;

#[cfg(test)]
mod suite;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;

use async_trait::async_trait;
use gorse_common::floats;
use gorse_common::gocontext::{Context, ContextError};
use gorse_common::gotime::Time;
use gorse_storage_core::errors::Error as StorageError;
use gorse_storage_core::options::Opt;
use gorse_storage_core::scheme::XVEC_PREFIX;

/// Go's `Distance`: how similarity between two vectors is measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Distance {
    /// Go's `Cosine`.
    #[default]
    Cosine,
    /// Go's `Euclidean`.
    Euclidean,
    /// Go's `Dot`.
    Dot,
}

/// Go's `QuantizationType`, a string type. Gorse writes `""`, `"sq"`, `"pq"` or
/// `"rq"`, but config values are passed through unchecked, so any other string
/// is kept as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum QuantizationType {
    /// Go's `QuantizationNone`, the empty string: no quantization.
    #[default]
    None,
    /// Go's `QuantizationSQ`: scalar quantization.
    Sq,
    /// Go's `QuantizationPQ`: product quantization.
    Pq,
    /// Go's `QuantizationRQ`: rotational quantization.
    Rq,
    /// Any other string, as written. Go's `String()` prints it as `unknown`.
    Other(String),
}

impl From<&str> for QuantizationType {
    fn from(value: &str) -> Self {
        match value {
            "" => QuantizationType::None,
            "sq" => QuantizationType::Sq,
            "pq" => QuantizationType::Pq,
            "rq" => QuantizationType::Rq,
            other => QuantizationType::Other(other.to_owned()),
        }
    }
}

impl QuantizationType {
    /// The string Go holds for this type.
    pub fn as_str(&self) -> &str {
        match self {
            QuantizationType::None => "",
            QuantizationType::Sq => "sq",
            QuantizationType::Pq => "pq",
            QuantizationType::Rq => "rq",
            QuantizationType::Other(value) => value,
        }
    }
}

impl fmt::Display for QuantizationType {
    /// Go's `String()`: the name of the type, `unknown` for an unlisted string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuantizationType::Other(_) => f.write_str("unknown"),
            known => f.write_str(known.as_str()),
        }
    }
}

/// Go's `VectorConfig`: how a collection stores its vectors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VectorConfig {
    /// Go's `Type`: the quantization type.
    pub quantization: QuantizationType,
    /// Go's `Bits`: the quantization bits. 0 uses the backend default.
    pub bits: i64,
}

/// Go's `CollectionInfo`, returned by [`Database::describe_collection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionInfo {
    /// The collection's name without the table prefix.
    pub name: String,
    /// The number of dimensions. 0 for a sparse collection.
    pub dimension: i64,
    /// The distance the collection ranks by.
    pub distance: Distance,
    /// Go's embedded `VectorConfig`.
    pub config: VectorConfig,
}

/// Go's `Vector`: one vector and its attributes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vector {
    /// The vector's id, unique within its collection.
    pub id: String,
    /// Dense values in FP32.
    pub values: Vec<f32>,
    /// Dense values as IEEE 754 FP16 bits. Takes precedence over `values` when
    /// non-empty.
    pub h_values: Vec<u16>,
    /// The dimensions of a sparse vector; `values` holds their values.
    pub indices: Vec<u32>,
    /// Go's `IsHidden`: hidden vectors are never returned by a query.
    pub is_hidden: bool,
    /// The categories a query can filter on.
    pub categories: Vec<String>,
    /// When the vector was written. Deletes remove vectors older than a cutoff.
    pub timestamp: Time,
}

impl Vector {
    /// Go's `Float32Values`: the dense values in FP32.
    pub fn float32_values(&self) -> Cow<'_, [f32]> {
        if self.h_values.is_empty() {
            Cow::Borrowed(&self.values)
        } else {
            Cow::Owned(floats::to_float32(&self.h_values))
        }
    }

    /// Go's `Float16Values`: the dense values as IEEE 754 FP16 bits.
    pub fn float16_values(&self) -> Cow<'_, [u16]> {
        if self.h_values.is_empty() {
            Cow::Owned(floats::from_float32(&self.values))
        } else {
            Cow::Borrowed(&self.h_values)
        }
    }

    /// Go's `IsSparse`: the vector has indices and no dense values.
    pub fn is_sparse(&self) -> bool {
        self.h_values.is_empty() && !self.indices.is_empty()
    }

    /// Go's `Len`: the number of dense dimensions.
    pub fn len(&self) -> usize {
        if self.h_values.is_empty() {
            self.values.len()
        } else {
            self.h_values.len()
        }
    }

    /// Whether [`Vector::len`] is 0.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Go's `ScoredVector`: a vector and its similarity to the query. Higher
/// scores are more similar.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredVector {
    /// Go's embedded `Vector`.
    pub vector: Vector,
    /// The similarity score.
    pub score: f32,
}

/// The errors of the vector stores. Callers test the storage sentinels with
/// [`Error::kind`], as Go callers use `errors.Is`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// One of storage's sentinel errors, with the text Go wraps around it.
    #[error("{message}")]
    Storage {
        /// The sentinel, from `storage/errors.go`.
        kind: StorageError,
        /// Go's error text.
        message: String,
    },
    /// The caller's context was cancelled.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// Any other failure: Go's `errors.New`, or an I/O error with `errors.WithStack`.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// The storage sentinel this error wraps, if any. Go's `errors.Is`.
    pub fn kind(&self) -> Option<StorageError> {
        match self {
            Error::Storage { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

/// A storage sentinel returned bare, as Go returns `storage.ErrNoDatabase`.
pub(crate) fn storage_error(kind: StorageError) -> Error {
    Error::Storage {
        kind,
        message: kind.to_string(),
    }
}

/// Go's `storage.ErrNotFound` wrapped as `collection <name>: not found`.
pub(crate) fn not_found(name: &str) -> Error {
    Error::Storage {
        kind: StorageError::NotFound,
        message: format!("collection {name}: {}", StorageError::NotFound),
    }
}

/// Go's `storage.ErrAlreadyExists` wrapped as `collection <name> already exists`.
pub(crate) fn already_exists(name: &str) -> Error {
    Error::Storage {
        kind: StorageError::AlreadyExists,
        message: format!("collection {name} {}", StorageError::AlreadyExists),
    }
}

/// Go's `context.Context` error check, as the stores make it at the start of a call.
pub(crate) fn check_context(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}

/// Go's `CollaborativeFiltering` prefix.
pub const COLLABORATIVE_FILTERING: &str = "collaborative_filtering";
/// Go's `ItemToItemPrefix`.
pub const ITEM_TO_ITEM_PREFIX: &str = "item_to_item";
/// Go's `UserToUserPrefix`.
pub const USER_TO_USER_PREFIX: &str = "user_to_user";

/// Go's `CollaborativeFilteringCollection`: the collection of a CF model.
pub fn collaborative_filtering_collection(id: i64) -> String {
    format!("{COLLABORATIVE_FILTERING}_{id}")
}

/// Go's `ItemToItemCollection`.
pub fn item_to_item_collection(name: &str) -> String {
    format!("{ITEM_TO_ITEM_PREFIX}_{name}")
}

/// Go's `UserToUserCollection`.
pub fn user_to_user_collection(name: &str) -> String {
    format!("{USER_TO_USER_PREFIX}_{name}")
}

/// Go's `Database`: the vector store every backend implements. Every method
/// takes the caller's context first, as the Go methods do. The methods are
/// async, so a store can await the network or run file IO on a blocking pool.
#[async_trait]
pub trait Database: Send + Sync {
    /// Go's `Init`: opens the collections the store already holds.
    async fn init(&self) -> Result<(), Error>;
    /// Go's `Optimize`: rebuilds what the store keeps for a collection.
    async fn optimize(&self, ctx: &Context, name: &str) -> Result<(), Error>;
    /// Go's `Close`.
    async fn close(&self) -> Result<(), Error>;
    /// Go's `ListCollections`, sorted by name.
    async fn list_collections(&self, ctx: &Context) -> Result<Vec<String>, Error>;
    /// Go's `DescribeCollection`.
    async fn describe_collection(&self, ctx: &Context, name: &str)
    -> Result<CollectionInfo, Error>;
    /// Go's `AddCollection`. A dimension of 0 makes a sparse collection, which
    /// ranks by [`Distance::Dot`] only.
    async fn add_collection(
        &self,
        ctx: &Context,
        name: &str,
        dimensions: i64,
        distance: Distance,
        config: VectorConfig,
    ) -> Result<(), Error>;
    /// Go's `DeleteCollection`.
    async fn delete_collection(&self, ctx: &Context, name: &str) -> Result<(), Error>;
    /// Go's `CountVectors`.
    async fn count_vectors(&self, ctx: &Context, collection: &str) -> Result<i64, Error>;
    /// Go's `AddVectors`: writes each vector, replacing any with the same id.
    async fn add_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        vectors: &[Vector],
    ) -> Result<(), Error>;
    /// Go's `GetVectors`: the vectors with the given ids, in the order of `ids`.
    /// Missing ids and repeated ids are skipped.
    async fn get_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<Vector>, Error>;
    /// Go's `DeleteVectors`: deletes the vectors written before `timestamp`.
    async fn delete_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        timestamp: &Time,
    ) -> Result<(), Error>;
    /// Go's `QueryVectors`: up to `top_k` visible vectors that have all of
    /// `categories`, best score first.
    async fn query_vectors(
        &self,
        ctx: &Context,
        collection: &str,
        q: &Vector,
        categories: &[&str],
        top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error>;
}

/// Go's `Purge`: deletes all collections from a vector database.
pub async fn purge(ctx: &Context, database: &dyn Database) -> Result<(), Error> {
    for collection in database.list_collections(ctx).await? {
        database.delete_collection(ctx, &collection).await?;
    }
    Ok(())
}

/// Go's `orderVectors`: the vectors in the order of `ids`, each id once, with
/// the ids that have no vector left out.
pub(crate) fn order_vectors(ids: &[&str], vectors: Vec<Vector>) -> Vec<Vector> {
    let mut by_id: HashMap<String, Vector> = HashMap::with_capacity(vectors.len());
    for vector in vectors {
        by_id.insert(vector.id.clone(), vector);
    }
    let mut seen: HashSet<&str> = HashSet::with_capacity(ids.len());
    let mut ordered = Vec::with_capacity(by_id.len());
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        if let Some(vector) = by_id.remove(*id) {
            ordered.push(vector);
        }
    }
    ordered
}

/// Go's `Float16Bytes`: the little-endian bytes of FP16 bits.
///
/// Go reinterprets the slice's memory, so the bytes share it. This port
/// copies (`PORTING_NOTES.md` X-WP28-1).
pub fn float16_bytes(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// Go's `Float16Values` on bytes: FP16 bits read little-endian from `values`.
/// An odd trailing byte is dropped, as Go drops it. Copies, see
/// [`float16_bytes`].
pub fn float16_from_bytes(values: &[u8]) -> Vec<u16> {
    let (pairs, _) = values.as_chunks::<2>();
    pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect()
}

/// Go's `Open`: opens the store that `path` names. `xvec://` is the only store
/// so far; WP-2-9 adds the network stores. Async, as D-151 rules for every
/// registry. The xvec store's directory reads run on tokio's blocking pool.
///
/// Go fills a registry from `init` functions, which Rust has no equivalent of
/// without a link-time crate. The backends are listed here instead, which
/// gives the same lookup by prefix.
pub async fn open(
    path: &str,
    table_prefix: &str,
    opts: &[Opt],
) -> Result<Box<dyn Database>, Error> {
    // The xvec store takes no options: exact search has no job count and no
    // index to skip (`PORTING_NOTES.md` X-WP28-2).
    let _ = opts;
    match path.strip_prefix(XVEC_PREFIX) {
        Some(root) => {
            let (root, table_prefix) = (root.to_owned(), table_prefix.to_owned());
            let store = xvec::blocking(move || xvec::Xvec::new(&root, &table_prefix)).await?;
            Ok(Box::new(store))
        }
        None => Err(Error::Other(format!("Unknown database: {path}"))),
    }
}

/// Go's `NoDatabase`: the store used when no vector store is configured. Every
/// call returns `storage.ErrNoDatabase`, except [`Database::optimize`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoDatabase;

#[async_trait]
impl Database for NoDatabase {
    async fn init(&self) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn optimize(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    async fn close(&self) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn list_collections(&self, _ctx: &Context) -> Result<Vec<String>, Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn describe_collection(
        &self,
        _ctx: &Context,
        _name: &str,
    ) -> Result<CollectionInfo, Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn add_collection(
        &self,
        _ctx: &Context,
        _name: &str,
        _dimensions: i64,
        _distance: Distance,
        _config: VectorConfig,
    ) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn delete_collection(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn count_vectors(&self, _ctx: &Context, _collection: &str) -> Result<i64, Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn add_vectors(
        &self,
        _ctx: &Context,
        _collection: &str,
        _vectors: &[Vector],
    ) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn get_vectors(
        &self,
        _ctx: &Context,
        _collection: &str,
        _ids: &[&str],
    ) -> Result<Vec<Vector>, Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn delete_vectors(
        &self,
        _ctx: &Context,
        _collection: &str,
        _timestamp: &Time,
    ) -> Result<(), Error> {
        Err(storage_error(StorageError::NoDatabase))
    }

    async fn query_vectors(
        &self,
        _ctx: &Context,
        _collection: &str,
        _q: &Vector,
        _categories: &[&str],
        _top_k: i64,
    ) -> Result<Vec<ScoredVector>, Error> {
        Err(storage_error(StorageError::NoDatabase))
    }
}

#[cfg(test)]
mod database {
    use super::*;

    // Ports: storage/vectors/database_test.go TestHalfBytes @ add21b5
    // The Go test also checks that the byte and FP16 slices share memory and
    // that the conversions allocate nothing. Those are Go's zero-copy
    // reinterpretation, which this port does not do (X-WP28-1), so they have
    // no counterpart here.
    #[test]
    fn half_bytes() {
        let values = [0x0001, 0x3c00, 0x7e01, 0xffff];
        let encoded = float16_bytes(&values);
        assert_eq!(hex::encode(&encoded), "0100003c017effff");
        let decoded = float16_from_bytes(&encoded);
        assert_eq!(decoded, values);
        // An odd trailing byte is dropped.
        assert_eq!(float16_from_bytes(&encoded[..7]), values[..3]);
    }
}

#[cfg(test)]
mod no_database {
    use super::*;
    use gorse_common::gotime;

    // Ports: storage/vectors/no_database_test.go @ add21b5

    fn store() -> NoDatabase {
        NoDatabase
    }

    #[tokio::test]
    async fn lifecycle() {
        let ctx = Context::background();
        let database = store();
        assert_eq!(
            database.init().await.unwrap_err().kind(),
            Some(StorageError::NoDatabase)
        );
        assert!(database.optimize(&ctx, "test").await.is_ok());
        assert_eq!(
            database.close().await.unwrap_err().kind(),
            Some(StorageError::NoDatabase)
        );
    }

    #[tokio::test]
    async fn collections() {
        let ctx = Context::background();
        let database = store();
        assert_eq!(
            database.list_collections(&ctx).await.unwrap_err().kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .describe_collection(&ctx, "test")
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        let config = VectorConfig::default();
        assert_eq!(
            database
                .add_collection(&ctx, "test", 4, Distance::Cosine, config.clone())
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .add_collection(&ctx, "test", 4, Distance::Dot, config)
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .delete_collection(&ctx, "test")
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .delete_collection(&ctx, "missing")
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
    }

    #[tokio::test]
    async fn vectors() {
        let ctx = Context::background();
        let database = store();
        assert_eq!(
            database
                .count_vectors(&ctx, "test")
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );

        let vector = Vector {
            id: "a".to_owned(),
            values: vec![1.0, 0.0, 0.0, 0.0],
            categories: vec!["cat-a".to_owned()],
            ..Default::default()
        };
        assert_eq!(
            database
                .add_vectors(&ctx, "test", std::slice::from_ref(&vector))
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .add_vectors(&ctx, "test", &[])
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .get_vectors(&ctx, "test", &["a"])
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .delete_vectors(&ctx, "test", &gotime::now())
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .delete_vectors(&ctx, "missing", &Time::default())
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );

        let query = Vector {
            values: vec![1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        assert_eq!(
            database
                .query_vectors(&ctx, "test", &query, &["cat-a"], 10)
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
        assert_eq!(
            database
                .query_vectors(&ctx, "missing", &Vector::default(), &[], 0)
                .await
                .unwrap_err()
                .kind(),
            Some(StorageError::NoDatabase)
        );
    }
}
