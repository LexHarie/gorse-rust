// Ports: storage/cache/database.go @ add21b5

//! The cache store: the names Gorse caches under, the value and score
//! document types, the [`Database`] interface every backend implements, and
//! the registry that opens a store from its path.
//!
//! Three kinds of data live in a cache store: string values, score documents
//! (sorted, filterable recommendation lists) and time series points. The
//! backends are ported in their own work packages; [`no_database`] is the
//! store for a node with no database configured.
//!
//! Strings are Rust `String`s. Every backend but Redis stores text as UTF-8,
//! and Go's JSON and BSON encoders cannot store other bytes either, so a
//! Redis value that is not UTF-8 is an error here where Go returns its bytes.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use gorse_common::dateparse;
use gorse_common::gocontext::{Context, ContextError};
use gorse_common::gosort;
use gorse_common::gostrconv;
use gorse_common::gotime::{self, Duration, Time};
use gorse_storage_core::errors::Error as StorageError;
use gorse_storage_core::options::Opt;
#[cfg(feature = "mongodb")]
use gorse_storage_core::scheme::{MONGO_PREFIX, MONGO_SRV_PREFIX};

#[cfg(feature = "mongodb")]
pub mod mongodb;
pub mod no_database;
#[cfg(any(feature = "mysql", feature = "postgres", feature = "sqlite"))]
pub mod sql;

#[cfg(test)]
mod database_test;
#[cfg(test)]
mod no_database_test;

/// Recommendation results of non-personalized recommenders, one per name.
pub const NON_PERSONALIZED: &str = "non-personalized";
/// The digest of a non-personalized recommender's configuration.
pub const NON_PERSONALIZED_DIGEST: &str = "non-personalized_digest";
/// When a non-personalized recommender was last updated.
pub const NON_PERSONALIZED_UPDATE_TIME: &str = "non-personalized_update_time";
/// Item-to-item recommendations.
pub const ITEM_TO_ITEM: &str = "item-to-item";
/// The digest of the item-to-item configuration.
pub const ITEM_TO_ITEM_DIGEST: &str = "item-to-item_digest";
/// When item-to-item recommendations were last updated.
pub const ITEM_TO_ITEM_UPDATE_TIME: &str = "item-to-item_update_time";
/// User-to-user recommendations.
pub const USER_TO_USER: &str = "user-to-user";
/// The digest of the user-to-user configuration.
pub const USER_TO_USER_DIGEST: &str = "user-to-user_digest";
/// When user-to-user recommendations were last updated.
pub const USER_TO_USER_UPDATE_TIME: &str = "user-to-user_update_time";
/// Collaborative filtering recommendations.
pub const COLLABORATIVE_FILTERING: &str = "collaborative-filtering";
/// The digest of the collaborative filtering configuration.
pub const COLLABORATIVE_FILTERING_DIGEST: &str = "collaborative-filtering_digest";
/// When collaborative filtering recommendations were last updated.
pub const COLLABORATIVE_FILTERING_UPDATE_TIME: &str = "collaborative-filtering_update_time";
/// Personalized recommendations.
pub const RECOMMEND: &str = "recommend";
/// The digest of the personalized recommendation configuration.
pub const RECOMMEND_DIGEST: &str = "recommend_digest";
/// When personalized recommendations were last updated.
pub const RECOMMEND_UPDATE_TIME: &str = "recommend_update_time";

/// The set of item categories (`item_categories`, global).
pub const ITEM_CATEGORIES: &str = "item_categories";

/// The latest timestamp that a user related data was modified.
pub const LAST_MODIFY_ITEM_TIME: &str = "last_modify_item_time";
/// The latest timestamp that an item related data was modified.
pub const LAST_MODIFY_USER_TIME: &str = "last_modify_user_time";

/// Global meta information.
pub const GLOBAL_META: &str = "global_meta";
/// The number of users.
pub const NUM_USERS: &str = "num_users";
/// The number of items.
pub const NUM_ITEMS: &str = "num_items";
/// The number of feedback records.
pub const NUM_FEEDBACK: &str = "num_feedback";
/// The number of positive feedback records.
pub const NUM_POS_FEEDBACKS: &str = "num_pos_feedbacks";
/// The number of negative feedback records.
pub const NUM_NEG_FEEDBACKS: &str = "num_neg_feedbacks";
/// The number of user labels.
pub const NUM_USER_LABELS: &str = "num_user_labels";
/// The number of item labels.
pub const NUM_ITEM_LABELS: &str = "num_item_labels";
/// The number of positive feedback records in total.
pub const NUM_TOTAL_POS_FEEDBACKS: &str = "num_total_pos_feedbacks";
/// The number of valid positive feedback records.
pub const NUM_VALID_POS_FEEDBACKS: &str = "num_valid_pos_feedbacks";
/// The number of valid negative feedback records.
pub const NUM_VALID_NEG_FEEDBACKS: &str = "num_valid_neg_feedbacks";
/// When the matching model was last fitted.
pub const LAST_FIT_MATCHING_MODEL_TIME: &str = "last_fit_matching_model_time";
/// When the ranking model was last fitted.
pub const LAST_FIT_RANKING_MODEL_TIME: &str = "last_fit_ranking_model_time";
/// The latest timestamp that latest items were updated.
pub const LAST_UPDATE_LATEST_ITEMS_TIME: &str = "last_update_latest_items_time";
/// The latest timestamp that popular items were updated.
pub const LAST_UPDATE_POPULAR_ITEMS_TIME: &str = "last_update_popular_items_time";
/// The NDCG of collaborative filtering.
pub const CF_NDCG: &str = "cf_ndcg";
/// The precision of collaborative filtering.
pub const CF_PRECISION: &str = "cf_precision";
/// The recall of collaborative filtering.
pub const CF_RECALL: &str = "cf_recall";
/// The precision of the click-through rate model.
pub const CTR_PRECISION: &str = "ctr_precision";
/// The recall of the click-through rate model.
pub const CTR_RECALL: &str = "ctr_recall";
/// The AUC of the click-through rate model.
pub const CTR_AUC: &str = "ctr_auc";
/// The ratio of positive feedback.
pub const POSITIVE_FEEDBACK_RATIO: &str = "positive_feedback_ratio";

/// The collections the server patches when an item is hidden, re-categorised
/// or deleted (Go's `ItemCache`).
pub const ITEM_CACHE: [&str; 3] = [NON_PERSONALIZED, ITEM_TO_ITEM, RECOMMEND];

/// Go's `cache.Key`: joins the keys with `/`. Empty keys after the first are
/// skipped, so `key(&["a", "", "b"])` is `"a/b"`. With no keys the result is
/// empty.
pub fn key(keys: &[&str]) -> String {
    let Some((first, rest)) = keys.split_first() else {
        return String::new();
    };
    let mut s = String::from(*first);
    for k in rest.iter().filter(|k| !k.is_empty()) {
        s.push('/');
        s.push_str(k);
    }
    s
}

/// Go's `cache.Value`: a name and the string the backend stores for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    name: String,
    value: String,
}

impl Value {
    /// Returns the name of the value.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the raw string representation of the value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Go's `cache.String`: a string value, stored as is.
pub fn string(name: impl Into<String>, value: impl Into<String>) -> Value {
    Value {
        name: name.into(),
        value: value.into(),
    }
}

/// Go's `cache.Integer`: an integer value, stored in decimal (`strconv.Itoa`).
pub fn integer(name: impl Into<String>, value: i64) -> Value {
    Value {
        name: name.into(),
        value: value.to_string(),
    }
}

/// Go's `cache.Time`: a time value, stored as `Time.String()`, which includes
/// the monotonic clock reading when the time came from `now()`
/// (`PORTING_NOTES.md` Q-1).
pub fn time(name: impl Into<String>, value: Time) -> Value {
    Value {
        name: name.into(),
        value: value.string(),
    }
}

/// Go's `cache.ReturnValue`: what a backend answered for one [`Database::get`],
/// with a typed reader for each encoding. A missing key reads as the zero
/// value with no error.
#[derive(Debug, Clone)]
pub struct ReturnValue {
    value: String,
    err: Option<Error>,
    exists: bool,
}

/// Go's `NewReturnValue`: the raw value and whether the key exists.
pub fn new_return_value(value: impl Into<String>, exists: bool) -> ReturnValue {
    ReturnValue {
        value: value.into(),
        err: None,
        exists,
    }
}

/// Go's `NewReturnError`: a return value that carries an error.
pub fn new_return_error(err: Error) -> ReturnValue {
    ReturnValue {
        value: String::new(),
        err: Some(err),
        exists: false,
    }
}

impl ReturnValue {
    /// Go's `String`: the value as is, or "" when the key does not exist.
    pub fn string(&self) -> Result<String, Error> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if !self.exists {
            return Ok(String::new());
        }
        Ok(self.value.clone())
    }

    /// Go's `Integer`: the value parsed as a decimal integer, or 0 when the
    /// key does not exist.
    pub fn integer(&self) -> Result<i64, Error> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if !self.exists {
            return Ok(0);
        }
        let (n, err) = gostrconv::atoi(&self.value);
        match err {
            Some(err) => Err(Error::Atoi(err)),
            None => Ok(n),
        }
    }

    /// Go's `Time`: the value parsed with `dateparse.ParseAny` and converted
    /// to UTC, or the zero time when the key does not exist.
    pub fn time(&self) -> Result<Time, Error> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if !self.exists {
            return Ok(Time::default());
        }
        let t = dateparse::parse_any(&self.value, &[])?;
        Ok(t.in_location(&gotime::utc()))
    }

    /// Go's `Exists`: whether the key exists.
    pub fn exists(&self) -> bool {
        self.exists
    }
}

/// Go's `cache.Score`: one document of a score collection.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// The document's id (an item id for recommendations).
    pub id: String,
    /// The score the list is sorted by.
    pub score: f64,
    /// Whether the document is hidden from searches.
    pub is_hidden: bool,
    /// The categories the document belongs to.
    pub categories: Vec<String>,
    /// The time the document was added or last changed.
    pub timestamp: Time,
}

/// Go's `cache.SortDocuments`: sorts by score, highest first, with Go's
/// `sort.Slice`. The sort is not stable, so documents with equal scores come
/// out in the order Go's pattern-defeating quicksort gives, not in input order.
/// `gorse_common::gosort` ports `sort.Sort`, which runs the same algorithm as
/// `sort.Slice` behind a different interface, so the comparisons and swaps are
/// the same.
pub fn sort_documents(documents: &mut [Score]) {
    gosort::sort(&mut ByScore(documents));
}

/// The documents as the `sort.Interface` that `gosort` sorts: `less` compares
/// scores, as the closure does in Go's `SortDocuments`.
struct ByScore<'a>(&'a mut [Score]);

impl gosort::Interface for ByScore<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.0[i].score > self.0[j].score
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}

/// Go's `cache.ConvertDocumentsToValues`: the ids of the documents, in order.
pub fn convert_documents_to_values(documents: &[Score]) -> Vec<String> {
    documents.iter().map(|d| d.id.clone()).collect()
}

/// Go's `cache.ScoreCondition`: which documents a delete removes. Each field
/// that is set must match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScoreCondition {
    /// Only documents in this subset.
    pub subset: Option<String>,
    /// Only the document with this id.
    pub id: Option<String>,
    /// Only documents timestamped before this time.
    pub before: Option<Time>,
}

impl ScoreCondition {
    /// Go's `Check`: a condition with no field set would match everything,
    /// so it is an error.
    pub fn check(&self) -> Result<(), Error> {
        if self.id.is_none() && self.before.is_none() && self.subset.is_none() {
            return Err(Error::DocumentCondition(StorageError::InvalidArgument));
        }
        Ok(())
    }
}

/// Go's `cache.ScorePatch`: the fields an update changes. A `None` field is
/// left as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScorePatch {
    /// The new hidden flag.
    pub is_hidden: Option<bool>,
    /// The new categories. `Some` with an empty list clears them; Go's nil
    /// slice is `None`.
    pub categories: Option<Vec<String>>,
    /// The new score.
    pub score: Option<f64>,
}

/// Go's `cache.TimeSeriesPoint`: one value of a named series at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeSeriesPoint {
    /// The series name.
    pub name: String,
    /// When the value was recorded.
    pub timestamp: Time,
    /// The value.
    pub value: f64,
}

/// Errors from the cache store. Go's `errors.Is` checks become
/// [`Error::kind`], which reports the `storage` sentinel an error wraps.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// A bare sentinel from `storage`, such as `ErrNoDatabase`.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Go's `fmt.Errorf("document condition: %w", ...)`.
    #[error("document condition: {0}")]
    DocumentCondition(StorageError),
    /// A context was cancelled (Go's `context canceled`).
    #[error(transparent)]
    Context(#[from] ContextError),
    /// `strconv.Atoi` failed on a stored value.
    #[error(transparent)]
    Atoi(#[from] gostrconv::NumError),
    /// `dateparse.ParseAny` failed on a stored value.
    #[error(transparent)]
    DateParse(#[from] dateparse::Error),
    /// No registered creator matches the database path (Go's
    /// `Unknown database`).
    #[error("Unknown database: {0}")]
    UnknownDatabase(String),
    /// A failure from the backend's own driver.
    #[error("{0}")]
    Backend(Arc<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// The `storage` sentinel this error wraps, as Go's `errors.Is` sees it.
    pub fn kind(&self) -> Option<StorageError> {
        match self {
            Error::Storage(kind) | Error::DocumentCondition(kind) => Some(*kind),
            _ => None,
        }
    }
}

/// Go's `cache.Database`: the interface every cache backend implements.
#[async_trait]
pub trait Database: Send + Sync {
    /// Closes the connection.
    async fn close(&self) -> Result<(), Error>;
    /// Checks that the backend is reachable.
    async fn ping(&self) -> Result<(), Error>;
    /// Creates the tables or collections the store needs.
    async fn init(&self) -> Result<(), Error>;
    /// Calls `work` with every key. A callback error stops the scan and is
    /// returned.
    async fn scan(
        &self,
        work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error>;
    /// Removes everything in the store.
    async fn purge(&self) -> Result<(), Error>;

    /// Writes the values.
    async fn set(&self, ctx: &Context, values: &[Value]) -> Result<(), Error>;
    /// Reads one value. Errors are carried in the returned [`ReturnValue`].
    async fn get(&self, ctx: &Context, name: &str) -> ReturnValue;
    /// Removes one value.
    async fn delete(&self, ctx: &Context, name: &str) -> Result<(), Error>;

    /// Adds score documents to a collection and subset.
    async fn add_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        documents: &[Score],
    ) -> Result<(), Error>;
    /// Returns the visible documents of one subset that contain every
    /// category in `query`, highest score first, from `begin` to `end`.
    async fn search_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        query: &[String],
        begin: i64,
        end: i64,
    ) -> Result<Vec<Score>, Error>;
    /// Removes the documents that match the condition from the collections.
    async fn delete_scores(
        &self,
        ctx: &Context,
        collection: &[String],
        condition: &ScoreCondition,
    ) -> Result<(), Error>;
    /// Changes the matching documents in the collections. `subset` is `None`
    /// to match every subset.
    async fn update_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        subset: Option<&str>,
        id: &str,
        patch: &ScorePatch,
    ) -> Result<(), Error>;
    /// Calls `callback` with the collection, id, subset and timestamp of every
    /// score document.
    async fn scan_scores(
        &self,
        ctx: &Context,
        callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error>;

    /// Adds time series points.
    async fn add_time_series_points(
        &self,
        ctx: &Context,
        points: &[TimeSeriesPoint],
    ) -> Result<(), Error>;
    /// Returns the points of a series between `begin` and `end`, one per
    /// `duration` bucket, each the last value in its bucket.
    async fn get_time_series_points(
        &self,
        ctx: &Context,
        name: &str,
        begin: Time,
        end: Time,
        duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error>;
}

/// Go's `cache.Creator`: opens a store for a path and table prefix. Async,
/// because the network stores open their clients asynchronously (D-151).
pub type Creator =
    for<'a> fn(&'a str, &'a str, &'a [Opt]) -> BoxFuture<'a, Result<Box<dyn Database>, Error>>;

/// Go's package-level `creators` map and its `Register` and `Open` functions,
/// as a value a node builds at start-up. Go registers creators in `init`.
#[derive(Default)]
pub struct Registry {
    creators: HashMap<String, Creator>,
}

impl Registry {
    /// Go's `Register`: makes `creator` handle every path that starts with one
    /// of the prefixes. A later registration of a prefix replaces the earlier
    /// one.
    pub fn register(&mut self, prefixes: &[&str], creator: Creator) {
        for p in prefixes {
            self.creators.insert((*p).to_owned(), creator);
        }
    }

    /// Go's `Open`: opens the store whose registered prefix matches `path`.
    /// Go picks among matching prefixes in map order; this takes the longest,
    /// which is the only one a path can match unambiguously.
    pub async fn open(
        &self,
        path: &str,
        table_prefix: &str,
        opts: &[Opt],
    ) -> Result<Box<dyn Database>, Error> {
        let creator = self
            .creators
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, creator)| *creator);
        match creator {
            Some(creator) => creator(path, table_prefix, opts).await,
            None => Err(Error::UnknownDatabase(path.to_owned())),
        }
    }
}

/// Registers the cache backends this build was compiled with. Go registers them
/// in `init`; a node calls this once on the registry it builds (D-126).
pub fn register_builtin(registry: &mut Registry) {
    #[cfg(feature = "mongodb")]
    registry.register(&[MONGO_PREFIX, MONGO_SRV_PREFIX], mongodb::creator);
    #[cfg(any(feature = "mysql", feature = "postgres", feature = "sqlite"))]
    sql::register(registry);
    #[cfg(not(feature = "mongodb"))]
    let _ = registry;
}

#[cfg(test)]
mod tests {
    use gorse_common::gotime::{self, Month};

    use super::*;

    /// The values Go writes for `cache.Time`: `Time.String()` of the times
    /// below, printed by Go 1.27.0.
    #[test]
    fn times_are_written_as_go_writes_them() {
        let utc = gotime::utc();
        let t = gotime::date(2026, Month(1), 2, 3, 4, 5, 678_901_000, &utc);
        assert_eq!(
            time("last_modify_user_time/u1", t).value(),
            "2026-01-02 03:04:05.678901 +0000 UTC"
        );
        let t = gotime::date(1996, Month::APRIL, 8, 0, 0, 0, 0, &utc);
        assert_eq!(time("meta/1", t).value(), "1996-04-08 00:00:00 +0000 UTC");
        let t = gotime::date(2023, Month::JANUARY, 1, 0, 0, 0, 500, &utc);
        assert_eq!(
            time("x", t).value(),
            "2023-01-01 00:00:00.0000005 +0000 UTC"
        );
    }

    /// Go's `ReturnValue.Time` reads what Go writes, including the monotonic
    /// reading Go adds to times from `now()`, and returns UTC.
    #[test]
    fn times_are_read_back_in_utc() {
        let read = |s: &str| new_return_value(s, true).time().unwrap();
        let want = gotime::date(2026, Month(10), 17, 2, 58, 44, 902_094_534, &gotime::utc());
        let got = read("2026-10-17 02:58:44.902094534 +0000 UTC m=+39.779705272");
        assert!(got.equal(&want));
        assert_eq!(got.year(), 2026);
        // A time in another zone names the same instant in UTC.
        let got = read("2026-10-17 02:58:44.902094534 +0200 CEST");
        assert!(got.equal(&gotime::date(
            2026,
            Month(10),
            17,
            0,
            58,
            44,
            902_094_534,
            &gotime::utc()
        )));
    }

    /// A missing key reads as the zero value with no error.
    #[test]
    fn missing_keys_read_as_zero_values() {
        let ret = new_return_value("ignored", false);
        assert!(!ret.exists());
        assert_eq!(ret.string().unwrap(), "");
        assert_eq!(ret.integer().unwrap(), 0);
        assert!(ret.time().unwrap().is_zero());
    }

    #[test]
    fn integers_round_trip_and_report_go_errors() {
        assert_eq!(integer("n", -5).value(), "-5");
        assert_eq!(new_return_value("-5", true).integer().unwrap(), -5);
        let err = new_return_value("abc", true).integer().unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"strconv.Atoi: parsing "abc": invalid syntax"#
        );
    }

    #[test]
    fn score_condition_needs_one_field() {
        assert_eq!(
            ScoreCondition::default().check().unwrap_err().kind(),
            Some(StorageError::InvalidArgument)
        );
        let condition = ScoreCondition {
            subset: Some("s".to_owned()),
            ..Default::default()
        };
        assert!(condition.check().is_ok());
    }

    /// Stands in for a backend's creator: it reports the arguments it was
    /// given, in its error text.
    fn short<'a>(
        path: &'a str,
        table_prefix: &'a str,
        _: &'a [Opt],
    ) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
        Box::pin(async move {
            Err(Error::Backend(Arc::new(std::io::Error::other(format!(
                "short {table_prefix}{path}"
            )))))
        })
    }

    fn long<'a>(
        path: &'a str,
        table_prefix: &'a str,
        _: &'a [Opt],
    ) -> BoxFuture<'a, Result<Box<dyn Database>, Error>> {
        Box::pin(async move {
            Err(Error::Backend(Arc::new(std::io::Error::other(format!(
                "long {table_prefix}{path}"
            )))))
        })
    }

    /// Go's `Open` hands the path to the creator whose prefix matches. With
    /// two matches this takes the longer prefix.
    #[tokio::test]
    async fn registry_opens_by_prefix() {
        async fn message(registry: &Registry, path: &str) -> String {
            match registry.open(path, "t_", &[]).await {
                Ok(_) => panic!("{path} must not open"),
                Err(err) => err.to_string(),
            }
        }
        let mut registry = Registry::default();
        registry.register(&["x://"], short);
        registry.register(&["x://y/"], long);
        assert_eq!(message(&registry, "x://q").await, "short t_x://q");
        assert_eq!(message(&registry, "x://y/z").await, "long t_x://y/z");
        assert_eq!(message(&registry, "w://").await, "Unknown database: w://");
    }

    /// Score documents whose ids are their input positions, so the order
    /// `sort_documents` gives shows in the ids.
    fn scored(scores: &[u8]) -> Vec<Score> {
        scores
            .iter()
            .enumerate()
            .map(|(i, s)| Score {
                id: i.to_string(),
                score: f64::from(*s),
                is_hidden: false,
                categories: Vec::new(),
                timestamp: Time::default(),
            })
            .collect()
    }

    /// The input positions of the documents, in the order they are in.
    fn positions(documents: &[Score]) -> Vec<usize> {
        documents.iter().map(|d| d.id.parse().unwrap()).collect()
    }

    #[test]
    fn sort_documents_sorts_highest_first() {
        let scores: Vec<u8> = (0..200u32).map(|i| ((i * 37) % 101) as u8).collect();
        let mut documents = scored(&scores);
        sort_documents(&mut documents);
        assert!(documents.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn sort_documents_handles_short_and_empty_slices() {
        let mut documents = scored(&[]);
        sort_documents(&mut documents);
        assert!(documents.is_empty());
        let mut documents = scored(&[3]);
        sort_documents(&mut documents);
        assert_eq!(positions(&documents), [0]);
        let mut documents = scored(&[1, 2]);
        sort_documents(&mut documents);
        assert_eq!(positions(&documents), [1, 0]);
    }

    /// `sort.Slice` output (Go 1.27.0) for the 40 scores `(i * 7) % 5`: the
    /// input positions, highest score first.
    #[test]
    fn sort_documents_breaks_ties_as_go_does_for_40_items() {
        let scores: Vec<u8> = (0..40u32).map(|i| ((i * 7) % 5) as u8).collect();
        let mut documents = scored(&scores);
        sort_documents(&mut documents);
        assert_eq!(positions(&documents), GO_40);
    }

    /// `sort.Slice` output (Go 1.27.0) for the 200 scores `(i * 37) % 101`,
    /// which are mostly ties. It takes the pattern-breaking and heap paths
    /// that the 40-item input does not.
    #[test]
    fn sort_documents_breaks_ties_as_go_does_for_200_items() {
        let scores: Vec<u8> = (0..200u32).map(|i| ((i * 37) % 101) as u8).collect();
        let mut documents = scored(&scores);
        sort_documents(&mut documents);
        assert_eq!(positions(&documents), GO_200);
    }

    const GO_40: [usize; 40] = [
        22, 37, 2, 27, 32, 17, 12, 7, 4, 9, 24, 19, 34, 39, 14, 29, 16, 26, 31, 11, 36, 21, 1, 6,
        8, 23, 13, 3, 28, 38, 33, 18, 20, 30, 25, 35, 0, 15, 10, 5,
    ];

    const GO_200: [usize; 200] = [
        131, 30, 161, 60, 90, 191, 120, 19, 150, 49, 180, 79, 8, 109, 38, 139, 68, 169, 98, 199,
        128, 27, 57, 158, 188, 87, 16, 117, 46, 147, 76, 177, 5, 106, 136, 35, 65, 166, 196, 95,
        125, 24, 54, 155, 185, 84, 114, 13, 144, 43, 174, 73, 103, 2, 133, 32, 163, 62, 92, 193,
        21, 122, 51, 152, 81, 182, 10, 111, 141, 40, 70, 171, 100, 130, 29, 160, 59, 89, 190, 18,
        119, 149, 48, 179, 78, 7, 108, 138, 37, 168, 67, 198, 97, 26, 127, 56, 157, 86, 187, 116,
        15, 146, 45, 176, 75, 105, 4, 135, 34, 64, 165, 94, 195, 124, 23, 154, 53, 184, 83, 113,
        12, 42, 143, 173, 72, 102, 1, 132, 31, 61, 162, 192, 91, 121, 20, 151, 50, 181, 80, 9, 110,
        140, 39, 170, 69, 99, 28, 129, 58, 159, 88, 189, 118, 17, 47, 148, 77, 178, 6, 107, 36,
        137, 66, 167, 96, 197, 25, 126, 55, 156, 186, 85, 115, 14, 145, 44, 74, 175, 104, 3, 33,
        134, 164, 63, 93, 194, 123, 22, 52, 153, 82, 183, 11, 112, 142, 41, 71, 172, 0, 101,
    ];
}
