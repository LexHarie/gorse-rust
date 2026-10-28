// Ports: storage/data/database.go @ add21b5

//! The data store (`storage/data`): the record types, the [`Database`]
//! interface every backend implements, the scan options that narrow its
//! queries, and the registry that opens a store from its path.
//!
//! The backends are ported in their own work packages; [`no_database`] is the
//! store for a node with no database configured.
//!
//! Go's `chan []T` plus `chan error` pairs become one [`BoxStream`] of
//! `Result<Vec<T>, Error>`. The batches come first and an error, if any, ends
//! the stream, which is the order Go's consumers see on the two channels.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use futures_util::future::BoxFuture;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::{Context, ContextError};
use gorse_common::gotime::Time;
use gorse_config::config::SearchConfig;
use gorse_storage_core::errors::Error as StorageError;
use gorse_storage_core::options::Opt;

use types::{Feedback, Item, ItemPatch, ScoredItem, User, UserPatch};

#[cfg(feature = "mongodb")]
pub mod mongodb;
pub mod no_database;
pub mod types;

#[cfg(test)]
mod database_test;
#[cfg(test)]
mod no_database_test;

/// Go's errors from the data store. A sentinel from `storage` is
/// [`Error::Storage`]; [`Error::kind`] is Go's `errors.Is` for it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// A bare sentinel from `storage`, such as `ErrNotFound`.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// A context was cancelled (Go's `context canceled`).
    #[error(transparent)]
    Context(#[from] ContextError),
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
            Error::Storage(kind) => Some(*kind),
            _ => None,
        }
    }
}

/// Go's `ScanOptions`: the bounds and filters of a feedback or item scan.
/// Every field is unset until a [`ScanOption`] sets it.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// The first user id of the scan, included (`BeginUserId`).
    pub begin_user_id: Option<String>,
    /// The last user id of the scan, included (`EndUserId`).
    pub end_user_id: Option<String>,
    /// The first item id of the scan, included (`BeginItemId`).
    pub begin_item_id: Option<String>,
    /// The last item id of the scan, included (`EndItemId`).
    pub end_item_id: Option<String>,
    /// The earliest time stamp of the scan, included (`BeginTime`).
    pub begin_time: Option<Time>,
    /// The latest time stamp of the scan, included (`EndTime`).
    pub end_time: Option<Time>,
    /// The feedback types the scan keeps (`FeedbackTypes`).
    pub feedback_types: Vec<FeedbackTypeExpression>,
    /// Whether the scan orders by item id (`OrderByItemId`).
    pub order_by_item_id: bool,
}

/// Go's `ScanOption`: sets one field of a [`ScanOptions`]. Build one with the
/// `with_*` functions of this module.
pub struct ScanOption(Box<dyn Fn(&mut ScanOptions) + Send + Sync>);

impl ScanOption {
    fn new(set: impl Fn(&mut ScanOptions) + Send + Sync + 'static) -> Self {
        Self(Box::new(set))
    }

    /// Applies this option to `options`.
    pub fn apply(&self, options: &mut ScanOptions) {
        (self.0)(options)
    }
}

/// Go's `WithBeginUserId`: the begin user id, included in the result.
pub fn with_begin_user_id(user_id: impl Into<String>) -> ScanOption {
    let user_id = user_id.into();
    ScanOption::new(move |o| o.begin_user_id = Some(user_id.clone()))
}

/// Go's `WithEndUserId`: the end user id, included in the result.
pub fn with_end_user_id(user_id: impl Into<String>) -> ScanOption {
    let user_id = user_id.into();
    ScanOption::new(move |o| o.end_user_id = Some(user_id.clone()))
}

/// Go's `WithBeginItemId`: the beginning item id, included in the result.
pub fn with_begin_item_id(item_id: impl Into<String>) -> ScanOption {
    let item_id = item_id.into();
    ScanOption::new(move |o| o.begin_item_id = Some(item_id.clone()))
}

/// Go's `WithEndItemId`: the end item id, included in the result.
pub fn with_end_item_id(item_id: impl Into<String>) -> ScanOption {
    let item_id = item_id.into();
    ScanOption::new(move |o| o.end_item_id = Some(item_id.clone()))
}

/// Go's `WithBeginTime`: the begin time, included in the result.
pub fn with_begin_time(t: Time) -> ScanOption {
    ScanOption::new(move |o| o.begin_time = Some(t.clone()))
}

/// Go's `WithEndTime`: the end time, included in the result.
pub fn with_end_time(t: Time) -> ScanOption {
    ScanOption::new(move |o| o.end_time = Some(t.clone()))
}

/// Go's `WithFeedbackTypes`: the feedback types the scan keeps.
pub fn with_feedback_types(feedback_types: Vec<FeedbackTypeExpression>) -> ScanOption {
    ScanOption::new(move |o| o.feedback_types = feedback_types.clone())
}

/// Go's `WithOrderByItemId`: the scan orders by item id.
pub fn with_order_by_item_id() -> ScanOption {
    ScanOption::new(|o| o.order_by_item_id = true)
}

/// Go's `NewScanOptions`: applies `opts` in order to an empty
/// [`ScanOptions`]. Go skips nil options; a [`ScanOption`] cannot be nil, so
/// there is nothing to skip here.
pub fn new_scan_options(opts: &[ScanOption]) -> ScanOptions {
    let mut options = ScanOptions::default();
    for opt in opts {
        opt.apply(&mut options);
    }
    options
}

/// Go's `GetOptions`: how a batch get of items treats the items it finds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetOptions {
    /// Only items that have every one of these categories (`Categories`).
    pub categories: Vec<String>,
    /// Drops hidden items (`SkipHidden`).
    pub skip_hidden: bool,
    /// Returns only the item ids, with every other field empty (`ReturnId`).
    pub return_id: bool,
    /// Only items whose time stamp is after this time (`After`).
    pub after: Option<Time>,
}

/// Go's `data.Database`: the interface every data store backend implements.
///
/// Each method that Go gives a `ctx` takes it first. Methods without a `ctx`
/// are the store's lifecycle calls, which Go also gives none. The stream
/// methods do their work as the stream is polled, so dropping the stream
/// stops the scan.
#[async_trait]
pub trait Database: Send + Sync {
    /// Closes the connection.
    async fn close(&self) -> Result<(), Error>;
    /// Checks that the backend is reachable.
    async fn ping(&self) -> Result<(), Error>;
    /// Creates the tables or collections the store needs.
    async fn init(&self) -> Result<(), Error>;
    /// Brings the search index in line with the search columns of `config`.
    async fn reconcile(&self, config: &SearchConfig) -> Result<(), Error>;
    /// Makes the writes made so far visible to later reads.
    async fn optimize(&self) -> Result<(), Error>;
    /// Removes every record.
    async fn purge(&self) -> Result<(), Error>;
    /// Writes the items, overwriting those with the same id. An item already
    /// present is not an error.
    async fn batch_insert_items(&self, ctx: &Context, items: &[Item]) -> Result<(), Error>;
    /// Returns the items with the given ids that match `opts`.
    async fn batch_get_items(
        &self,
        ctx: &Context,
        item_ids: &[String],
        opts: &GetOptions,
    ) -> Result<Vec<Item>, Error>;
    /// Deletes an item and the feedback on it.
    async fn delete_item(&self, ctx: &Context, item_id: &str) -> Result<(), Error>;
    /// Returns an item, or `ErrNotFound`.
    async fn get_item(&self, ctx: &Context, item_id: &str) -> Result<Item, Error>;
    /// Returns up to `n` items that match `query`, best first.
    async fn search_items(
        &self,
        ctx: &Context,
        query: &str,
        n: i64,
    ) -> Result<Vec<ScoredItem>, Error>;
    /// Changes the fields of an item that `patch` sets.
    async fn modify_item(
        &self,
        ctx: &Context,
        item_id: &str,
        patch: &ItemPatch,
    ) -> Result<(), Error>;
    /// Returns up to `n` items after `cursor` and the cursor for the next
    /// page, which is empty at the end. Only items after `begin_time` are
    /// returned, when it is set.
    async fn get_items(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error>;
    /// Returns up to `n` of the latest items, newest first, that have every
    /// category in `categories` and a time stamp after `after`.
    async fn get_latest_items(
        &self,
        ctx: &Context,
        n: i64,
        categories: &[String],
        after: Option<Time>,
    ) -> Result<Vec<Item>, Error>;
    /// Returns the feedback on an item, of the given types when any are
    /// given.
    async fn get_item_feedback(
        &self,
        ctx: &Context,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error>;
    /// Writes the users, overwriting those with the same id.
    async fn batch_insert_users(&self, ctx: &Context, users: &[User]) -> Result<(), Error>;
    /// Deletes a user and the feedback from that user.
    async fn delete_user(&self, ctx: &Context, user_id: &str) -> Result<(), Error>;
    /// Returns a user, or `ErrNotFound`.
    async fn get_user(&self, ctx: &Context, user_id: &str) -> Result<User, Error>;
    /// Changes the fields of a user that `patch` sets.
    async fn modify_user(
        &self,
        ctx: &Context,
        user_id: &str,
        patch: &UserPatch,
    ) -> Result<(), Error>;
    /// Returns up to `n` users after `cursor` and the cursor for the next
    /// page, which is empty at the end.
    async fn get_users(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
    ) -> Result<(String, Vec<User>), Error>;
    /// Returns the feedback from a user up to `end_time`, of the feedback
    /// types that match `feedback_types` when any are given.
    async fn get_user_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        end_time: Option<Time>,
        feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error>;
    /// Returns the feedback from a user on an item, of the given types when
    /// any are given.
    async fn get_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error>;
    /// Deletes the feedback from a user on an item, of the given types when
    /// any are given, and returns how many records it deleted.
    async fn delete_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<i64, Error>;
    /// Writes the feedback. `insert_user` and `insert_item` add the users and
    /// items the feedback names when they are missing. `overwrite` replaces a
    /// feedback record that already exists; otherwise its value is added to.
    async fn batch_insert_feedback(
        &self,
        ctx: &Context,
        feedback: &[Feedback],
        insert_user: bool,
        insert_item: bool,
        overwrite: bool,
    ) -> Result<(), Error>;
    /// Returns up to `n` feedback records after `cursor`, between `begin_time`
    /// and `end_time` when they are set, and the cursor for the next page,
    /// which is empty at the end.
    async fn get_feedback(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
        end_time: Option<Time>,
        feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error>;
    /// Streams every user in batches of `batch_size`.
    fn get_user_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>>;
    /// Streams the items in batches of `batch_size`, skipping those
    /// stamped after `time_limit` when it is set.
    fn get_item_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>>;
    /// Streams the feedback that `options` select, in batches of
    /// `batch_size`.
    fn get_feedback_stream<'a>(
        &'a self,
        ctx: &'a Context,
        batch_size: i64,
        options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>>;
    /// Returns the number of users.
    async fn count_users(&self, ctx: &Context) -> Result<i64, Error>;
    /// Returns the number of items.
    async fn count_items(&self, ctx: &Context) -> Result<i64, Error>;
    /// Returns the number of feedback records.
    async fn count_feedback(&self, ctx: &Context) -> Result<i64, Error>;
}

/// Go's `data.Creator`: opens a store for a path and table prefix. Async, as
/// the cache registry is (D-151).
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
