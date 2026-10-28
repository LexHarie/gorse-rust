// Ports: storage/cache/no_database.go @ add21b5

//! [`NoDatabase`]: the cache store of a node with no database configured.
//! Every method fails with `storage`'s `ErrNoDatabase`.

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{Duration, Time};
use gorse_storage_core::errors::Error as StorageError;

use super::{
    Database, Error, ReturnValue, Score, ScoreCondition, ScorePatch, TimeSeriesPoint, Value,
    new_return_error,
};

/// Go's `cache.NoDatabase`: a cache store that stores nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoDatabase;

/// The error every `NoDatabase` method returns.
fn no_database() -> Error {
    Error::Storage(StorageError::NoDatabase)
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

    async fn scan(
        &self,
        _work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn purge(&self) -> Result<(), Error> {
        Err(no_database())
    }

    async fn set(&self, _ctx: &Context, _values: &[Value]) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get(&self, _ctx: &Context, _name: &str) -> ReturnValue {
        new_return_error(no_database())
    }

    async fn delete(&self, _ctx: &Context, _name: &str) -> Result<(), Error> {
        Err(no_database())
    }

    async fn add_scores(
        &self,
        _ctx: &Context,
        _collection: &str,
        _subset: &str,
        _documents: &[Score],
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn search_scores(
        &self,
        _ctx: &Context,
        _collection: &str,
        _subset: &str,
        _query: &[String],
        _begin: i64,
        _end: i64,
    ) -> Result<Vec<Score>, Error> {
        Err(no_database())
    }

    async fn delete_scores(
        &self,
        _ctx: &Context,
        _collection: &[String],
        _condition: &ScoreCondition,
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn update_scores(
        &self,
        _ctx: &Context,
        _collections: &[String],
        _subset: Option<&str>,
        _id: &str,
        _patch: &ScorePatch,
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn scan_scores(
        &self,
        _ctx: &Context,
        _callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn add_time_series_points(
        &self,
        _ctx: &Context,
        _points: &[TimeSeriesPoint],
    ) -> Result<(), Error> {
        Err(no_database())
    }

    async fn get_time_series_points(
        &self,
        _ctx: &Context,
        _name: &str,
        _begin: Time,
        _end: Time,
        _duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error> {
        Err(no_database())
    }
}
