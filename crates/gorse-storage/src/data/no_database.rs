// Ports: storage/data/no_database.go @ add21b5

//! [`NoDatabase`]: the data store of a node with no database configured.
//! Every method fails with `storage`'s `ErrNoDatabase`.

use async_trait::async_trait;
use futures_core::stream::BoxStream;
use futures_util::stream;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_config::config::SearchConfig;
use gorse_storage_core::errors::Error as StorageError;

use super::types::{Feedback, Item, ItemPatch, ScoredItem, User, UserPatch};
use super::{Database, Error, GetOptions, ScanOption};

/// Go's `data.NoDatabase`: a data store that stores nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoDatabase;

/// The error every `NoDatabase` method returns.
fn no_database() -> Error {
    Error::Storage(StorageError::NoDatabase)
}

/// The stream every `NoDatabase` stream method returns: one `ErrNoDatabase`,
/// as Go's error channel sends it.
fn no_database_stream<'a, T: Send + 'a>(
) -> BoxStream<'a, Result<Vec<T>, Error>> {
    Box::pin(stream::once(std::future::ready(Err(no_database()))))
}

#[async_trait]
impl Database for NoDatabase {
    async fn close(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn ping(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn init(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn reconcile(&self, _config: &SearchConfig) -> Result<(), Error> {
        Err(no_database())
    }

    async fn optimize(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn purge(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn batch_insert_items(&self, _ctx: &Context, _items: &[Item]) -> Result<(), Error> {
        Err(no_database())
    }

    async fn batch_get_items(
        &self,
        _ctx: &Context,
        _item_ids: &[String],
        _opts: &GetOptions,
    ) -> Result<Vec<Item>, Error> {
        Err(no_database())
    }

    async fn delete_item(&self, _ctx: &Context, _item_id: &str) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_item(&self, _ctx: &Context, _item_id: &str) -> Result<Item, Error> {
        Err(no_database())
    }

    async fn search_items(
        &self,
        _ctx: &Context,
        _query: &str,
        _n: i64,
    ) -> Result<Vec<ScoredItem>, Error> {
        Err(no_database())
    }

    async fn modify_item(
        &self,
        _ctx: &Context,
        _item_id: &str,
        _patch: &ItemPatch,
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_items(
        &self,
        _ctx: &Context,
        _cursor: &str,
        _n: i64,
        _begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error> {
        Err(no_database())
    }

    async fn get_latest_items(
        &self,
        _ctx: &Context,
        _n: i64,
        _categories: &[String],
        _after: Option<Time>,
    ) -> Result<Vec<Item>, Error> {
        Err(no_database())
    }

    async fn get_item_feedback(
        &self,
        _ctx: &Context,
        _item_id: &str,
        _feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        Err(no_database())
    }

    async fn batch_insert_users(&self, _ctx: &Context, _users: &[User]) -> Result<(), Error> {
        Err(no_database())
    }

    async fn delete_user(&self, _ctx: &Context, _user_id: &str) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_user(&self, _ctx: &Context, _user_id: &str) -> Result<User, Error> {
        Err(no_database())
    }

    async fn modify_user(
        &self,
        _ctx: &Context,
        _user_id: &str,
        _patch: &UserPatch,
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_users(
        &self,
        _ctx: &Context,
        _cursor: &str,
        _n: i64,
    ) -> Result<(String, Vec<User>), Error> {
        Err(no_database())
    }

    async fn get_user_feedback(
        &self,
        _ctx: &Context,
        _user_id: &str,
        _end_time: Option<Time>,
        _feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error> {
        Err(no_database())
    }

    async fn get_user_item_feedback(
        &self,
        _ctx: &Context,
        _user_id: &str,
        _item_id: &str,
        _feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        Err(no_database())
    }

    async fn delete_user_item_feedback(
        &self,
        _ctx: &Context,
        _user_id: &str,
        _item_id: &str,
        _feedback_types: &[String],
    ) -> Result<i64, Error> {
        Err(no_database())
    }

    async fn batch_insert_feedback(
        &self,
        _ctx: &Context,
        _feedback: &[Feedback],
        _insert_user: bool,
        _insert_item: bool,
        _overwrite: bool,
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_feedback(
        &self,
        _ctx: &Context,
        _cursor: &str,
        _n: i64,
        _begin_time: Option<Time>,
        _end_time: Option<Time>,
        _feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error> {
        Err(no_database())
    }

    fn get_user_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        _batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>> {
        no_database_stream()
    }

    fn get_item_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        _batch_size: i64,
        _time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>> {
        no_database_stream()
    }

    fn get_feedback_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        _batch_size: i64,
        _options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>> {
        no_database_stream()
    }

    async fn count_users(&self, _ctx: &Context) -> Result<i64, Error> {
        Err(no_database())
    }

    async fn count_items(&self, _ctx: &Context) -> Result<i64, Error> {
        Err(no_database())
    }

    async fn count_feedback(&self, _ctx: &Context) -> Result<i64, Error> {
        Err(no_database())
    }
}
