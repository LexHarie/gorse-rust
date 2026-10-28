// Ports: storage/cache/mongodb.go, storage/cache/mongodb_test.go @ add21b5

//! The MongoDB cache store, [`MongoDB`].
//!
//! The store keeps three collections, named from the table prefix: `values`
//! holds one document per string value (`_id` is the name), `documents` one
//! per score document, and `time_series_points` one per point.
//! [`Database::init`] creates them with the indexes Go creates, so a database
//! made by either implementation serves the other.
//!
//! The driver's `bulk_write` needs MongoDB 8.0, and this store runs on 6.0 as
//! well. So each write is sent on its own, in order, and the first error stops
//! the batch, as Go's ordered bulk write does.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, Time};
use gorse_storage_core::options::Opt;
use gorse_storage_core::scheme::TablePrefix;
use mongodb::bson::{self, DateTime, Document, doc};
use mongodb::{Client, Collection, IndexModel};

use super::{
    Database, Error, ReturnValue, Score, ScoreCondition, ScorePatch, TimeSeriesPoint, Value,
    new_return_error, new_return_value,
};

/// Go's `cache.MongoDB`: a cache store in one MongoDB database.
pub struct MongoDB {
    client: Client,
    db_name: String,
    values_table: String,
    document_table: String,
    points_table: String,
}

impl MongoDB {
    /// Go's `Open` creator for `mongodb://` and `mongodb+srv://` paths: makes
    /// a client for the cluster `path` names and uses the database the path
    /// names. Go's `mongo.Connect` is lazy, and so is this: a server that is
    /// down shows on the first operation, not here.
    pub async fn open(path: &str, table_prefix: &str) -> Result<MongoDB, Error> {
        let options = mongodb::options::ClientOptions::parse(driver_uri(path))
            .await
            .map_err(backend)?;
        let db_name = options.default_database.clone().unwrap_or_default();
        let client = Client::with_options(options).map_err(backend)?;
        let prefix = TablePrefix::from(table_prefix);
        Ok(MongoDB {
            client,
            db_name,
            values_table: collection_name(prefix.values_table())?,
            document_table: collection_name(prefix.document_table())?,
            points_table: collection_name(prefix.points_table())?,
        })
    }

    /// The database every collection is in.
    fn db(&self) -> mongodb::Database {
        self.client.database(&self.db_name)
    }

    /// The collection of string values.
    fn values(&self) -> Collection<Document> {
        self.db().collection(&self.values_table)
    }

    /// The collection of score documents.
    fn documents(&self) -> Collection<Document> {
        self.db().collection(&self.document_table)
    }

    /// The collection of time series points.
    fn points(&self) -> Collection<Document> {
        self.db().collection(&self.points_table)
    }
}

/// The URI as the Rust driver reads it. Go's connection string parser takes
/// the legacy option `connect`, and the Gorse configs use `connect=direct`,
/// which the Rust driver rejects. As Go reads it, `connect=direct` is
/// `directConnection=true`, and `connect=automatic` is the default, so it is
/// dropped. Other options pass through unchanged.
fn driver_uri(path: &str) -> String {
    let Some((base, query)) = path.split_once('?') else {
        return path.to_owned();
    };
    let options: Vec<&str> = query
        .split('&')
        .filter_map(|option| match option {
            "connect=direct" => Some("directConnection=true"),
            "connect=automatic" => None,
            other => Some(other),
        })
        .collect();
    if options.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", options.join("&"))
    }
}

/// Wraps a driver or BSON error as the store's backend error.
fn backend<E>(err: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::Backend(Arc::new(err))
}

/// The name of a collection, which MongoDB needs as UTF-8.
fn collection_name(name: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(name).map_err(backend)
}

/// The error the driver gives for a write with no models (Go's
/// `ErrEmptySlice`).
#[derive(Debug, thiserror::Error)]
#[error("must provide at least one element in input slice")]
struct EmptySlice;

/// The error the server gives for a negative `skip`. The driver's skip is
/// unsigned, so the store refuses the value itself, with the server's text.
#[derive(Debug, thiserror::Error)]
#[error("(Location51024) BSON field 'skip' value must be >= 0, actual value '{0}'")]
struct NegativeSkip(i64);

/// Go's driver returns the context's error without sending a request when the
/// context is already done. Operations that are under way are not cancelled.
fn check_ctx(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}

/// Go's `primitive.NewDateTimeFromTime`: the milliseconds since the Unix
/// epoch, computed as `t.Unix()*1e3 + t.Nanosecond()/1e6`. A time's
/// sub-millisecond part is cut off within its second, and before 1970 this
/// is not the same as cutting off the whole nanosecond count.
fn to_datetime(t: &Time) -> DateTime {
    DateTime::from_millis(t.unix() * 1000 + t.nanosecond() / 1_000_000)
}

/// Go's driver reads a BSON date as `time.Unix(ms/1e3, ms%1e3*1e6).UTC()`.
fn from_datetime(d: DateTime) -> Time {
    let ms = d.timestamp_millis();
    gotime::unix(ms / 1000, ms % 1000 * 1_000_000).in_location(&gotime::utc())
}

fn str_field<'a>(doc: &'a Document, key: &str) -> Result<&'a str, Error> {
    doc.get_str(key).map_err(backend)
}

fn f64_field(doc: &Document, key: &str) -> Result<f64, Error> {
    doc.get_f64(key).map_err(backend)
}

fn datetime_field(doc: &Document, key: &str) -> Result<DateTime, Error> {
    doc.get_datetime(key).copied().map_err(backend)
}

/// The categories of a score document. Go writes a nil slice as BSON null,
/// which reads back as no categories, the same as an empty array does.
fn categories_field(doc: &Document, key: &str) -> Result<Vec<String>, Error> {
    match doc.get(key) {
        None | Some(bson::Bson::Null) => Ok(Vec::new()),
        Some(_) => doc
            .get_array(key)
            .map_err(backend)?
            .iter()
            .map(|item| match item.as_str() {
                Some(category) => Ok(category.to_owned()),
                None => Err(backend(bson::document::ValueAccessError::UnexpectedType)),
            })
            .collect(),
    }
}

/// Reads a score document as Go's `cache.Score` decodes it. Go's decoder
/// reads the key `ishidden`, which this store never writes, so `IsHidden` is
/// always false. The search only returns documents that are not hidden, so
/// the result is the same either way.
fn score_from_document(doc: &Document) -> Result<Score, Error> {
    Ok(Score {
        id: str_field(doc, "id")?.to_owned(),
        score: f64_field(doc, "score")?,
        is_hidden: false,
        categories: categories_field(doc, "categories")?,
        timestamp: from_datetime(datetime_field(doc, "timestamp")?),
    })
}

#[async_trait]
impl Database for MongoDB {
    async fn close(&self) -> Result<(), Error> {
        self.client.clone().shutdown().await;
        Ok(())
    }

    async fn ping(&self) -> Result<(), Error> {
        self.client
            .database("admin")
            .run_command(doc! {"ping": 1})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn init(&self) -> Result<(), Error> {
        let db = self.db();
        let collections = db.list_collection_names().await.map_err(backend)?;
        if !collections.contains(&self.values_table) {
            db.create_collection(&self.values_table)
                .await
                .map_err(backend)?;
        }
        self.documents()
            .create_indexes(vec![
                IndexModel::builder()
                    .keys(doc! {"collection": 1, "subset": 1, "id": 1})
                    .build(),
                IndexModel::builder()
                    .keys(doc! {
                        "collection": 1,
                        "subset": 1,
                        "categories": 1,
                        "is_hidden": 1,
                        "score": -1,
                    })
                    .build(),
            ])
            .await
            .map_err(backend)?;
        self.points()
            .create_indexes(vec![
                IndexModel::builder()
                    .keys(doc! {"name": 1, "timestamp": 1})
                    .build(),
            ])
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn scan(
        &self,
        work: &mut (dyn for<'a> FnMut(&'a str) -> Result<(), Error> + Send),
    ) -> Result<(), Error> {
        let mut cursor = self.values().find(doc! {}).await.map_err(backend)?;
        // Go's loop ends at a cursor error without returning it, since Go
        // never checks the cursor's error. The port does the same.
        while let Ok(true) = cursor.advance().await {
            let row = cursor.deserialize_current().map_err(backend)?;
            work(str_field(&row, "_id")?)?;
        }
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        // As in Go, the time series points are not purged.
        self.values().delete_many(doc! {}).await.map_err(backend)?;
        self.documents()
            .delete_many(doc! {})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn set(&self, ctx: &Context, values: &[Value]) -> Result<(), Error> {
        if values.is_empty() {
            return Ok(());
        }
        check_ctx(ctx)?;
        let collection = self.values();
        for value in values {
            collection
                .update_one(
                    doc! {"_id": value.name()},
                    doc! {"$set": {"_id": value.name(), "value": value.value()}},
                )
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn get(&self, ctx: &Context, name: &str) -> ReturnValue {
        if let Err(err) = check_ctx(ctx) {
            return new_return_error(err);
        }
        match self.values().find_one(doc! {"_id": name}).await {
            Ok(Some(row)) => match str_field(&row, "value") {
                Ok(value) => new_return_value(value, true),
                Err(err) => new_return_error(err),
            },
            Ok(None) => new_return_value("", false),
            Err(err) => new_return_error(backend(err)),
        }
    }

    async fn delete(&self, ctx: &Context, name: &str) -> Result<(), Error> {
        check_ctx(ctx)?;
        self.values()
            .delete_one(doc! {"_id": name})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn add_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        documents: &[Score],
    ) -> Result<(), Error> {
        if documents.is_empty() {
            return Ok(());
        }
        check_ctx(ctx)?;
        let table = self.documents();
        for document in documents {
            table
                .update_one(
                    doc! {
                        "collection": collection,
                        "subset": subset,
                        "id": document.id.as_str(),
                    },
                    doc! {"$set": {
                        "score": document.score,
                        "is_hidden": document.is_hidden,
                        "categories": document.categories.clone(),
                        "timestamp": to_datetime(&document.timestamp),
                    }},
                )
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn search_scores(
        &self,
        ctx: &Context,
        collection: &str,
        subset: &str,
        query: &[String],
        begin: i64,
        end: i64,
    ) -> Result<Vec<Score>, Error> {
        check_ctx(ctx)?;
        let skip = u64::try_from(begin).map_err(|_| backend(NegativeSkip(begin)))?;
        let mut filter = doc! {
            "collection": collection,
            "subset": subset,
            "is_hidden": false,
        };
        if !query.is_empty() {
            filter.insert("categories", doc! {"$all": query.to_vec()});
        }
        let table = self.documents();
        let mut find = table.find(filter).skip(skip).sort(doc! {"score": -1});
        if end != -1 {
            find = find.limit(end - begin);
        }
        let mut cursor = find.await.map_err(backend)?;
        let mut documents = Vec::new();
        // As in Scan: a cursor error ends the loop without being returned.
        while let Ok(true) = cursor.advance().await {
            let row = cursor.deserialize_current().map_err(backend)?;
            documents.push(score_from_document(&row)?);
        }
        Ok(documents)
    }

    async fn delete_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        condition: &ScoreCondition,
    ) -> Result<(), Error> {
        condition.check()?;
        check_ctx(ctx)?;
        let mut filter = doc! {"collection": {"$in": collections.to_vec()}};
        if let Some(subset) = &condition.subset {
            filter.insert("subset", subset.as_str());
        }
        if let Some(id) = &condition.id {
            filter.insert("id", id.as_str());
        }
        if let Some(before) = &condition.before {
            filter.insert("timestamp", doc! {"$lt": to_datetime(before)});
        }
        self.documents()
            .delete_many(filter)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn update_scores(
        &self,
        ctx: &Context,
        collections: &[String],
        subset: Option<&str>,
        id: &str,
        patch: &ScorePatch,
    ) -> Result<(), Error> {
        if collections.is_empty() {
            return Ok(());
        }
        if patch.is_hidden.is_none() && patch.categories.is_none() && patch.score.is_none() {
            return Ok(());
        }
        check_ctx(ctx)?;
        let mut filter = doc! {
            "collection": {"$in": collections.to_vec()},
            "id": id,
        };
        if let Some(subset) = subset {
            filter.insert("subset", subset);
        }
        // Go sends one `$set` for each patched field. The server applies them
        // as the one `$set` below does, so the result is the same.
        let mut set = Document::new();
        if let Some(is_hidden) = patch.is_hidden {
            set.insert("is_hidden", is_hidden);
        }
        if let Some(categories) = &patch.categories {
            set.insert("categories", categories.clone());
        }
        if let Some(score) = patch.score {
            set.insert("score", score);
        }
        self.documents()
            .update_many(filter, doc! {"$set": set})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn scan_scores(
        &self,
        ctx: &Context,
        callback: &mut (
                 dyn for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time) -> Result<(), Error>
                     + Send
             ),
    ) -> Result<(), Error> {
        let mut cursor = self.documents().find(doc! {}).await.map_err(backend)?;
        // As in Scan: a cursor error ends the loop without being returned.
        while let Ok(true) = cursor.advance().await {
            check_ctx(ctx)?;
            let row = cursor.deserialize_current().map_err(backend)?;
            let collection = str_field(&row, "collection")?;
            let subset = str_field(&row, "subset")?;
            let id = str_field(&row, "id")?;
            let timestamp = from_datetime(datetime_field(&row, "timestamp")?);
            callback(collection, id, subset, timestamp)?;
        }
        Ok(())
    }

    async fn add_time_series_points(
        &self,
        ctx: &Context,
        points: &[TimeSeriesPoint],
    ) -> Result<(), Error> {
        // Unlike the other writes, Go's bulk write refuses an empty list.
        if points.is_empty() {
            return Err(backend(EmptySlice));
        }
        check_ctx(ctx)?;
        let table = self.points();
        for point in points {
            table
                .update_one(
                    doc! {
                        "name": point.name.as_str(),
                        "timestamp": to_datetime(&point.timestamp),
                    },
                    doc! {"$set": {"value": point.value}},
                )
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn get_time_series_points(
        &self,
        ctx: &Context,
        name: &str,
        begin: Time,
        end: Time,
        duration: Duration,
    ) -> Result<Vec<TimeSeriesPoint>, Error> {
        check_ctx(ctx)?;
        let ms = duration.milliseconds();
        let mut cursor = self
            .points()
            .aggregate(vec![
                doc! {"$match": {
                    "name": name,
                    "timestamp": {
                        "$gte": to_datetime(&begin),
                        "$lte": to_datetime(&end),
                    },
                }},
                doc! {"$group": {
                    "_id": {"$multiply": [
                        {"$floor": {"$divide": [{"$toLong": "$timestamp"}, ms]}},
                        ms,
                    ]},
                    "name": {"$top": {"sortBy": {"timestamp": -1}, "output": "$name"}},
                    "value": {"$top": {"sortBy": {"timestamp": -1}, "output": "$value"}},
                }},
                doc! {"$project": {
                    "_id": 0,
                    "timestamp": {"$toDate": "$_id"},
                    "name": 1,
                    "value": 1,
                }},
                doc! {"$sort": {"timestamp": 1}},
            ])
            .await
            .map_err(backend)?;
        let mut points = Vec::new();
        while cursor.advance().await.map_err(backend)? {
            let row = cursor.deserialize_current().map_err(backend)?;
            points.push(TimeSeriesPoint {
                name: str_field(&row, "name")?.to_owned(),
                timestamp: from_datetime(datetime_field(&row, "timestamp")?),
                value: f64_field(&row, "value")?,
            });
        }
        Ok(points)
    }
}

/// The future a [`creator`] returns.
pub type OpenFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn Database>, Error>> + Send + 'a>>;

/// The creator for `mongodb://` and `mongodb+srv://` paths, in the form the
/// cache registry takes: it opens the store [`MongoDB::open`] makes. Go's
/// creator takes options and ignores them, and so does this.
pub fn creator<'a>(path: &'a str, table_prefix: &'a str, _opts: &'a [Opt]) -> OpenFuture<'a> {
    Box::pin(async move {
        let db = MongoDB::open(path, table_prefix).await?;
        Ok(Box::new(db) as Box<dyn Database>)
    })
}

#[cfg(test)]
mod tests {
    use crate::cache::{Database, database_test};

    use super::MongoDB;

    /// The creator opens the same store as [`MongoDB::open`], as a boxed
    /// [`Database`]. Skipped without `MONGO_URI`, as [`mongo`] is.
    #[tokio::test]
    async fn creator_opens_the_store() {
        let Ok(uri) = std::env::var("MONGO_URI") else {
            eprintln!("MONGO_URI is not set, skipping MongoDB creator test");
            return;
        };
        let db = super::creator(
            &format!("{uri}gorse_cache_test?authSource=admin&connect=direct"),
            "gorse_",
            &[],
        )
        .await
        .unwrap();
        db.ping().await.unwrap();
        db.close().await.unwrap();
    }

    /// Go's `TestMongo`: runs the shared suite against a real MongoDB, in the
    /// database `gorse_cache_test`, which is dropped first. Without
    /// `MONGO_URI` the test is skipped, as Go's `t.Skip` does.
    #[tokio::test]
    async fn mongo() {
        let Ok(uri) = std::env::var("MONGO_URI") else {
            eprintln!("MONGO_URI is not set, skipping MongoDB test");
            return;
        };
        let db_name = "gorse_cache_test";
        // Drop the database, as Go's SetupSuite does before it creates the schema.
        let db = MongoDB::open(&uri, "gorse_").await.unwrap();
        db.client.database(db_name).drop().await.unwrap();
        db.close().await.unwrap();
        // Create the schema in the test database.
        let db = MongoDB::open(
            &format!("{uri}{db_name}?authSource=admin&connect=direct"),
            "gorse_",
        )
        .await
        .unwrap();
        db.init().await.unwrap();
        database_test::run(&db).await;
        db.close().await.unwrap();
    }
}
