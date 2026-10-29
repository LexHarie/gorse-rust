// Ports: storage/data/mongodb.go, storage/data/mongodb_test.go @ add21b5 (TestMongo; the
// benchmark BenchmarkMongo_CountItems is not ported, since `go test` does not run it. The
// cases TestMongo runs are the shared suite in `database_test.rs`.)

//! The MongoDB data store, [`MongoDB`]: the users, items and feedback
//! collections, with the indexes Go creates, so a database made by either
//! implementation serves the other.
//!
//! The documents keep Go's BSON field names: the Go field names lower-cased
//! by the BSON codec (`userid`, `itemid`, `ishidden`, `updateat`, and the
//! nested `feedbackkey.{feedbacktype,userid,itemid}`). A nil slice or a nil
//! label is BSON null, as Go writes it.
//!
//! The driver's `bulk_write` needs MongoDB 8.0, and this store runs on 6.0 as
//! well. So each write is sent on its own, in order, and the first error stops
//! the batch, as Go's ordered bulk write does.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::alphabet;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
use futures_core::stream::BoxStream;
use futures_util::future::BoxFuture;
use futures_util::{TryStreamExt, stream};
use gorse_common::expression::{ExprType, FeedbackTypeExpression, go_json_quote};
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_config::config::SearchConfig;
use gorse_storage_core::errors::Error as StorageError;
use gorse_storage_core::options::Opt;
use gorse_storage_core::scheme::{MONGO_PREFIX, MONGO_SRV_PREFIX, TablePrefix};
use indexmap::{IndexMap, IndexSet};
use mongodb::bson::{self, Bson, DateTime, Document, doc};
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, Cursor, IndexModel};

use super::types::{Feedback, FeedbackKey, Item, ItemPatch, Labels, ScoredItem, User, UserPatch};
use super::{Database, Error, GetOptions, Registry, ScanOption, ScanOptions, new_scan_options};

/// The name of the text index that `reconcile` keeps (Go's `searchIndexName`).
const SEARCH_INDEX_NAME: &str = "gorse_search_index";

/// Go's `base64.StdEncoding`, which the paged scans' cursors use. The crate's
/// `STANDARD` engine rejects the non-zero trailing bits that Go decodes, so
/// this engine accepts them.
const GO_STANDARD: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
);

/// Go's `data.MongoDB`: a data store in one MongoDB database.
pub struct MongoDB {
    client: Client,
    db_name: String,
    users_table: String,
    items_table: String,
    feedback_table: String,
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
            users_table: collection_name(prefix.users_table())?,
            items_table: collection_name(prefix.items_table())?,
            feedback_table: collection_name(prefix.feedback_table())?,
        })
    }

    /// The database every collection is in.
    fn db(&self) -> mongodb::Database {
        self.client.database(&self.db_name)
    }

    /// The users collection.
    fn users(&self) -> Collection<Document> {
        self.db().collection(&self.users_table)
    }

    /// The items collection.
    fn items(&self) -> Collection<Document> {
        self.db().collection(&self.items_table)
    }

    /// The feedback collection.
    fn feedback(&self) -> Collection<Document> {
        self.db().collection(&self.feedback_table)
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

/// Go's driver returns the context's error without sending a request when the
/// context is already done. Operations that are under way are not cancelled.
fn check_ctx(ctx: &Context) -> Result<(), Error> {
    match ctx.err() {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}

/// The error for a BSON value a label cannot hold. Gorse's labels come from
/// JSON, so only a document written by another program can hold one.
#[derive(Debug, thiserror::Error)]
#[error("unsupported BSON type in labels: {0:?}")]
struct UnsupportedLabel(bson::spec::ElementType);

/// Go's `primitive.NewDateTimeFromTime`: the milliseconds since the Unix
/// epoch, computed as `t.Unix()*1e3 + t.Nanosecond()/1e6`.
fn to_datetime(t: &Time) -> DateTime {
    DateTime::from_millis(t.unix() * 1000 + t.nanosecond() / 1_000_000)
}

/// Go's driver reads a BSON date as `time.Unix(ms/1e3, ms%1e3*1e6).UTC()`.
fn from_datetime(d: DateTime) -> Time {
    let ms = d.timestamp_millis();
    gotime::unix(ms / 1000, ms % 1000 * 1_000_000).in_location(&gotime::utc())
}

/// Go's `FeedbackKey` as a BSON document, with its fields in struct order.
fn feedback_key_document(key: &FeedbackKey) -> Document {
    doc! {
        "feedbacktype": key.feedback_type.as_str(),
        "userid": key.user_id.as_str(),
        "itemid": key.item_id.as_str(),
    }
}

/// Go's `[]string` as a BSON array, or BSON null for a nil slice.
fn categories_bson(categories: Option<&[String]>) -> Bson {
    match categories {
        None => Bson::Null,
        Some(categories) => strings_bson(categories),
    }
}

/// A list of strings as a BSON array.
fn strings_bson(values: &[String]) -> Bson {
    Bson::Array(values.iter().map(|v| Bson::String(v.clone())).collect())
}

/// Go's `any` labels as BSON. The lists and maps that the walk is inside are
/// kept on a stack, not by recursion, so labels nested at any depth are
/// converted in full.
fn labels_to_bson(labels: &Labels) -> Bson {
    /// A list or a map whose children are being converted: the children it has
    /// yet to take, what it has so far, and for a map the name of the child in
    /// progress.
    enum Open<'a> {
        List(std::slice::Iter<'a, Labels>, Vec<Bson>),
        Map(
            indexmap::map::Iter<'a, String, Labels>,
            Document,
            Option<&'a String>,
        ),
    }
    let mut open: Vec<Open<'_>> = Vec::new();
    let mut child = labels;
    loop {
        // A scalar is converted at once. A list or a map opens a frame, and its
        // first child is converted next.
        let mut finished = match child {
            Labels::Nil => Some(Bson::Null),
            Labels::Bool(value) => Some(Bson::Boolean(*value)),
            Labels::Float64(value) => Some(Bson::Double(*value)),
            Labels::Number(text) => Some(number_to_bson(text)),
            Labels::String(text) => Some(Bson::String(text.clone())),
            Labels::Array(values) => {
                open.push(Open::List(values.iter(), Vec::with_capacity(values.len())));
                None
            }
            Labels::Object(entries) => {
                open.push(Open::Map(entries.iter(), Document::new(), None));
                None
            }
        };
        // A finished value goes to the frame around it. A frame with no children
        // left is finished in turn, until a frame has a child to convert.
        child = loop {
            if let Some(value) = finished.take() {
                match open.last_mut() {
                    None => return value,
                    Some(Open::List(_, values)) => values.push(value),
                    Some(Open::Map(_, entries, name)) => {
                        let name = name.take().expect("a map's child has its name");
                        entries.insert(name.clone(), value);
                    }
                }
            }
            let frame = open
                .last_mut()
                .expect("a frame is open while its child converts");
            match frame {
                Open::List(children, _) => {
                    if let Some(next) = children.next() {
                        break next;
                    }
                }
                Open::Map(children, _, name) => {
                    if let Some((key, next)) = children.next() {
                        *name = Some(key);
                        break next;
                    }
                }
            }
            finished = Some(match open.pop().expect("the frame is open") {
                Open::List(_, values) => Bson::Array(values),
                Open::Map(_, entries, _) => Bson::Document(entries),
            });
        };
    }
}

/// A `json.Number` as BSON. The Go driver's encoder writes it as an int64 when
/// its text is an int64, and as a double otherwise (`JSONNumberEncodeValue`).
/// Text that is neither cannot come from JSON; the port writes it as a string,
/// where Go's encoder returns an error.
fn number_to_bson(text: &str) -> Bson {
    if let Ok(value) = text.parse::<i64>() {
        Bson::Int64(value)
    } else if let Ok(value) = text.parse::<f64>() {
        Bson::Double(value)
    } else {
        Bson::String(text.to_owned())
    }
}

/// Go's `unpack`: a BSON value as the `any` Go reads it. A document becomes a
/// map and an array becomes a slice. Go does not unpack an array's elements,
/// but Gorse's labels validation keeps arrays to strings and numbers, where
/// that makes no difference, so the elements are unpacked here too. The lists
/// and documents being unpacked are kept on a stack, so a value nested at any
/// depth is unpacked in full.
fn labels_from_bson(value: &Bson) -> Result<Labels, Error> {
    /// A list or a document being unpacked, as in [`labels_to_bson`].
    enum Open<'a> {
        List(std::slice::Iter<'a, Bson>, Vec<Labels>),
        Document(
            std::vec::IntoIter<(&'a String, &'a Bson)>,
            IndexMap<String, Labels>,
            Option<&'a String>,
        ),
    }
    let mut open: Vec<Open<'_>> = Vec::new();
    let mut child = value;
    loop {
        let mut finished = match child {
            Bson::Null => Some(Labels::Nil),
            Bson::Boolean(value) => Some(Labels::Bool(*value)),
            Bson::Double(value) => Some(Labels::Float64(*value)),
            // Go keeps an int32 or int64 as it is. Its JSON text is the same as a
            // `json.Number`'s, which is how Gorse's labels hold a number.
            Bson::Int32(value) => Some(Labels::Number(value.to_string())),
            Bson::Int64(value) => Some(Labels::Number(value.to_string())),
            Bson::String(text) => Some(Labels::String(text.clone())),
            Bson::Array(values) => {
                open.push(Open::List(values.iter(), Vec::with_capacity(values.len())));
                None
            }
            Bson::Document(entries) => {
                let children: Vec<_> = entries.iter().collect();
                open.push(Open::Document(children.into_iter(), IndexMap::new(), None));
                None
            }
            other => return Err(backend(UnsupportedLabel(other.element_type()))),
        };
        child = loop {
            if let Some(value) = finished.take() {
                match open.last_mut() {
                    None => return Ok(value),
                    Some(Open::List(_, values)) => values.push(value),
                    Some(Open::Document(_, entries, name)) => {
                        let name = name.take().expect("a document's child has its name");
                        entries.insert(name.clone(), value);
                    }
                }
            }
            let frame = open
                .last_mut()
                .expect("a frame is open while its child unpacks");
            match frame {
                Open::List(children, _) => {
                    if let Some(next) = children.next() {
                        break next;
                    }
                }
                Open::Document(children, _, name) => {
                    if let Some((key, next)) = children.next() {
                        *name = Some(key);
                        break next;
                    }
                }
            }
            finished = Some(match open.pop().expect("the frame is open") {
                Open::List(_, values) => Labels::Array(values),
                Open::Document(_, entries, _) => Labels::Object(entries),
            });
        };
    }
}

/// Reads a string field. A missing or null field is the zero value, as Go's
/// decoder leaves it.
fn string_field(doc: &Document, key: &str) -> Result<String, Error> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(String::new()),
        Some(value) => Ok(string_value(value, key)?.to_owned()),
    }
}

/// A BSON string, or the error for a value of another type.
fn string_value<'a>(value: &'a Bson, key: &str) -> Result<&'a str, Error> {
    value.as_str().ok_or_else(|| backend(bson_type_error(key)))
}

/// Reads a boolean field, false when it is missing or null.
fn bool_field(doc: &Document, key: &str) -> Result<bool, Error> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| backend(bson_type_error(key))),
    }
}

/// Reads a number field. Go decodes an int32 or an int64 into a float64.
fn f64_field(doc: &Document, key: &str) -> Result<f64, Error> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(0.0),
        Some(Bson::Double(value)) => Ok(*value),
        Some(Bson::Int32(value)) => Ok(f64::from(*value)),
        #[allow(
            clippy::cast_precision_loss,
            reason = "Go converts an int64 to a float64 the same way"
        )]
        Some(Bson::Int64(value)) => Ok(*value as f64),
        Some(_) => Err(backend(bson_type_error(key))),
    }
}

/// Reads a date field. A missing or null field is the zero time.
fn time_field(doc: &Document, key: &str) -> Result<Time, Error> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(Time::default()),
        Some(Bson::DateTime(value)) => Ok(from_datetime(*value)),
        Some(_) => Err(backend(bson_type_error(key))),
    }
}

/// Reads a labels field. A missing field is nil.
fn labels_field(doc: &Document, key: &str) -> Result<Labels, Error> {
    match doc.get(key) {
        None => Ok(Labels::Nil),
        Some(value) => labels_from_bson(value),
    }
}

/// Reads the categories of an item or a user. A missing or null field is a
/// nil slice, which is `None`.
fn categories_field(doc: &Document, key: &str) -> Result<Option<Vec<String>>, Error> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Array(values)) => values
            .iter()
            .map(|value| Ok(string_value(value, key)?.to_owned()))
            .collect::<Result<Vec<_>, Error>>()
            .map(Some),
        Some(_) => Err(backend(bson_type_error(key))),
    }
}

/// The error for a field of the wrong BSON type.
#[derive(Debug, thiserror::Error)]
#[error("unexpected BSON type for field {0}")]
struct FieldType(String);

fn bson_type_error(key: &str) -> FieldType {
    FieldType(key.to_owned())
}

/// Go's `GetItem` and `BatchGetItems` decoding: an `Item` from its document.
fn item_from_document(doc: &Document) -> Result<Item, Error> {
    Ok(Item {
        item_id: string_field(doc, "itemid")?,
        is_hidden: bool_field(doc, "ishidden")?,
        categories: categories_field(doc, "categories")?,
        timestamp: time_field(doc, "timestamp")?,
        update_at: time_field(doc, "updateat")?,
        labels: labels_field(doc, "labels")?,
        comment: string_field(doc, "comment")?,
    })
}

/// Go's `ScoredItem` decoding: an item with its `score`.
fn scored_item_from_document(doc: &Document) -> Result<ScoredItem, Error> {
    Ok(ScoredItem {
        item: item_from_document(doc)?,
        score: f64_field(doc, "score")?,
    })
}

/// Go's `User` decoding.
fn user_from_document(doc: &Document) -> Result<User, Error> {
    Ok(User {
        user_id: string_field(doc, "userid")?,
        labels: labels_field(doc, "labels")?,
        comment: string_field(doc, "comment")?,
        update_at: time_field(doc, "updateat")?,
    })
}

/// Go's `FeedbackKey` decoding from the nested `feedbackkey` document. A
/// missing key is the zero key.
fn feedback_key_from_document(doc: &Document) -> Result<FeedbackKey, Error> {
    match doc.get("feedbackkey") {
        None | Some(Bson::Null) => Ok(FeedbackKey::default()),
        Some(Bson::Document(key)) => Ok(FeedbackKey {
            feedback_type: string_field(key, "feedbacktype")?,
            user_id: string_field(key, "userid")?,
            item_id: string_field(key, "itemid")?,
        }),
        Some(_) => Err(backend(bson_type_error("feedbackkey"))),
    }
}

/// Go's `Feedback` decoding.
fn feedback_from_document(doc: &Document) -> Result<Feedback, Error> {
    Ok(Feedback {
        feedback_key: feedback_key_from_document(doc)?,
        value: f64_field(doc, "value")?,
        timestamp: time_field(doc, "timestamp")?,
        updated: time_field(doc, "updated")?,
        labels: labels_field(doc, "labels")?,
        comment: string_field(doc, "comment")?,
    })
}

/// The document Go writes for an item: its struct fields in order, with
/// `updateat` set to `update_at`.
fn item_document(item: &Item, update_at: &Time) -> Document {
    doc! {
        "itemid": item.item_id.as_str(),
        "ishidden": item.is_hidden,
        "categories": categories_bson(item.categories.as_deref()),
        "timestamp": to_datetime(&item.timestamp),
        "updateat": to_datetime(update_at),
        "labels": labels_to_bson(&item.labels),
        "comment": item.comment.as_str(),
    }
}

/// The document Go writes for a user, with `updateat` set to `update_at`.
fn user_document(user: &User, update_at: &Time) -> Document {
    doc! {
        "userid": user.user_id.as_str(),
        "labels": labels_to_bson(&user.labels),
        "comment": user.comment.as_str(),
        "updateat": to_datetime(update_at),
    }
}

/// The document Go writes for a feedback record.
fn feedback_document(feedback: &Feedback) -> Document {
    doc! {
        "feedbackkey": feedback_key_document(&feedback.feedback_key),
        "value": feedback.value,
        "timestamp": to_datetime(&feedback.timestamp),
        "updated": to_datetime(&feedback.updated),
        "labels": labels_to_bson(&feedback.labels),
        "comment": feedback.comment.as_str(),
    }
}

/// Go's `FeedbackTypeExpressionToMongo`: the filter one feedback type
/// expression matches.
fn feedback_type_expression_to_mongo(e: &FeedbackTypeExpression) -> Document {
    let mut filter = doc! {"feedbackkey.feedbacktype": e.feedback_type.as_str()};
    let operator = match e.expr_type {
        ExprType::None => None,
        ExprType::Less => Some("$lt"),
        ExprType::LessOrEqual => Some("$lte"),
        ExprType::Greater => Some("$gt"),
        ExprType::GreaterOrEqual => Some("$gte"),
    };
    if let Some(operator) = operator {
        let mut condition = Document::new();
        condition.insert(operator, e.value);
        filter.insert("value", condition);
    }
    filter
}

/// The `$or` of a list of conditions, as an array.
fn or_of(conditions: Vec<Document>) -> Bson {
    Bson::Array(conditions.into_iter().map(Bson::Document).collect())
}

/// The conditions that match any of the feedback types, one
/// `feedbackkey.feedbacktype` equality per type.
fn feedback_type_conditions(feedback_types: &[String]) -> Vec<Document> {
    feedback_types
        .iter()
        .map(|feedback_type| doc! {"feedbackkey.feedbacktype": {"$eq": feedback_type.as_str()}})
        .collect()
}

/// Go's `GetItems` and `GetUsers` cursor: the base64 of the last id of the
/// page, as Go's `StdEncoding` writes it.
fn decode_cursor(cursor: &str) -> Result<String, Error> {
    let bytes = GO_STANDARD.decode(cursor).map_err(backend)?;
    String::from_utf8(bytes).map_err(backend)
}

/// The cursor for a page that ends with `last`.
fn encode_cursor(last: &str) -> String {
    GO_STANDARD.encode(last)
}

/// Go's `json.Marshal` of a `FeedbackKey`, which the feedback cursor holds.
/// The struct has no JSON tags, so the field names are the Go names.
fn feedback_key_json(key: &FeedbackKey) -> String {
    format!(
        "{{\"FeedbackType\":{},\"UserId\":{},\"ItemId\":{}}}",
        go_json_quote(&key.feedback_type),
        go_json_quote(&key.user_id),
        go_json_quote(&key.item_id),
    )
}

/// Go's `feedbackKeyFromString`: a `FeedbackKey` from the JSON of a cursor.
fn feedback_key_from_json(text: &[u8]) -> Result<FeedbackKey, Error> {
    serde_json::from_slice(text).map_err(backend)
}

/// Go's `mongoSearchColumnToField`: the document field a search column
/// indexes, or `None` for a column the text index does not cover.
fn mongo_search_column_to_field(column: &str) -> Option<String> {
    match column {
        "item.ItemId" => Some("itemid".to_owned()),
        "item.Categories" => Some("categories".to_owned()),
        "item.Comment" => Some("comment".to_owned()),
        "item.Labels" => Some("labels".to_owned()),
        _ => column
            .strip_prefix("item.Labels.")
            .filter(|rest| !rest.is_empty())
            .map(|rest| format!("labels.{rest}")),
    }
}

/// Go's `mongoSearchIndexKeysMatch`: whether an index's keys are exactly
/// `expected`, with the same count and the same values.
fn search_index_keys_match(existing: &Document, expected: &[(String, Bson)]) -> bool {
    if existing.len() != expected.len() {
        return false;
    }
    expected
        .iter()
        .all(|(key, value)| existing.get(key) == Some(value))
}

/// Go's `decodeAndCollect` loops: the rows of a cursor, decoded. Go's loops
/// end at a cursor error without returning it, since they never check
/// `r.Err()`, so the port does the same.
async fn decode_all<T>(
    mut cursor: Cursor<Document>,
    decode: fn(&Document) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let mut rows = Vec::new();
    while let Ok(true) = cursor.advance().await {
        let doc: Document = cursor.deserialize_current().map_err(backend)?;
        rows.push(decode(&doc)?);
    }
    Ok(rows)
}

/// The batches of a Go stream. Go's goroutine sends a full batch as soon as it
/// has `batch_size` rows, then the rest at the end, and an error ends the
/// stream without the partial batch. The query runs when the stream is first
/// polled. A `batch_size` that is not positive never fills a batch, so the
/// whole scan comes as one batch, as it does in Go.
fn batches<'a, T>(
    find: impl std::future::Future<Output = Result<Cursor<Document>, Error>> + Send + 'a,
    decode: fn(&Document) -> Result<T, Error>,
    batch_size: i64,
) -> BoxStream<'a, Result<Vec<T>, Error>>
where
    T: Send + 'a,
{
    let limit = usize::try_from(batch_size)
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(usize::MAX);
    let stream = stream::once(find).map_ok(move |cursor| {
        stream::try_unfold(cursor, move |mut cursor| async move {
            let mut batch = Vec::new();
            while batch.len() < limit {
                // As Go's loop does, a cursor error ends the scan without an error.
                if !matches!(cursor.advance().await, Ok(true)) {
                    break;
                }
                let doc: Document = cursor.deserialize_current().map_err(backend)?;
                batch.push(decode(&doc)?);
            }
            Ok(if batch.is_empty() {
                None
            } else {
                Some((batch, cursor))
            })
        })
    });
    Box::pin(stream.try_flatten())
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
        for name in [&self.users_table, &self.items_table, &self.feedback_table] {
            if !collections.contains(name) {
                db.create_collection(name.as_str()).await.map_err(backend)?;
            }
        }
        let unique = || IndexOptions::builder().unique(true).build();
        self.users()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"userid": 1})
                    .options(unique())
                    .build(),
            )
            .await
            .map_err(backend)?;
        self.items()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"itemid": 1})
                    .options(unique())
                    .build(),
            )
            .await
            .map_err(backend)?;
        self.feedback()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"feedbackkey": 1})
                    .options(unique())
                    .build(),
            )
            .await
            .map_err(backend)?;
        self.feedback()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"feedbackkey.userid": 1})
                    .build(),
            )
            .await
            .map_err(backend)?;
        self.feedback()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"feedbackkey.itemid": 1})
                    .build(),
            )
            .await
            .map_err(backend)?;
        self.items()
            .create_index(IndexModel::builder().keys(doc! {"timestamp": 1}).build())
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn reconcile(&self, search_config: &SearchConfig) -> Result<(), Error> {
        let columns = match &search_config.columns {
            Some(columns) if !columns.is_empty() => columns,
            _ => return Ok(()),
        };
        let mut keys: Vec<(String, Bson)> = columns
            .iter()
            .filter_map(|column| mongo_search_column_to_field(column))
            .map(|field| (field, Bson::String("text".to_owned())))
            .collect();
        if keys.is_empty() {
            return Ok(());
        }
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        let items = self.items();
        let mut indexes = items.list_indexes().await.map_err(backend)?;
        while let Some(index) = indexes.try_next().await.map_err(backend)? {
            let name = index.options.as_ref().and_then(|o| o.name.as_deref());
            if name == Some(SEARCH_INDEX_NAME) {
                if search_index_keys_match(&index.keys, &keys) {
                    return Ok(());
                }
                items.drop_index(SEARCH_INDEX_NAME).await.map_err(backend)?;
                break;
            }
        }
        let keys: Document = keys.into_iter().collect();
        items
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(
                        IndexOptions::builder()
                            .name(SEARCH_INDEX_NAME.to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn optimize(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn purge(&self) -> Result<(), Error> {
        for collection in [self.items(), self.feedback(), self.users()] {
            collection.delete_many(doc! {}).await.map_err(backend)?;
        }
        Ok(())
    }

    async fn batch_insert_items(&self, ctx: &Context, items: &[Item]) -> Result<(), Error> {
        check_ctx(ctx)?;
        let now = gotime::now();
        let collection = self.items();
        for item in items {
            collection
                .update_one(
                    doc! {"itemid": {"$eq": item.item_id.as_str()}},
                    doc! {"$set": item_document(item, &now)},
                )
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn batch_get_items(
        &self,
        ctx: &Context,
        item_ids: &[String],
        opts: &GetOptions,
    ) -> Result<Vec<Item>, Error> {
        check_ctx(ctx)?;
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut filter = doc! {"itemid": {"$in": strings_bson(item_ids)}};
        if !opts.categories.is_empty() {
            filter.insert("categories", doc! {"$all": strings_bson(&opts.categories)});
        }
        if opts.skip_hidden {
            filter.insert("ishidden", false);
        }
        if let Some(after) = &opts.after {
            filter.insert("timestamp", doc! {"$gt": to_datetime(after)});
        }
        let items = self.items();
        let mut find = items.find(filter);
        if opts.return_id {
            find = find.projection(doc! {"itemid": 1});
        }
        let cursor = find.await.map_err(backend)?;
        decode_all(cursor, item_from_document).await
    }

    async fn delete_item(&self, ctx: &Context, item_id: &str) -> Result<(), Error> {
        check_ctx(ctx)?;
        self.items()
            .delete_one(doc! {"itemid": item_id})
            .await
            .map_err(backend)?;
        self.feedback()
            .delete_many(doc! {"feedbackkey.itemid": {"$eq": item_id}})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_item(&self, ctx: &Context, item_id: &str) -> Result<Item, Error> {
        check_ctx(ctx)?;
        match self
            .items()
            .find_one(doc! {"itemid": item_id})
            .await
            .map_err(backend)?
        {
            Some(doc) => item_from_document(&doc),
            None => Err(StorageError::NotFound.into()),
        }
    }

    async fn search_items(
        &self,
        ctx: &Context,
        query: &str,
        n: i64,
    ) -> Result<Vec<ScoredItem>, Error> {
        check_ctx(ctx)?;
        if n <= 0 {
            return Ok(Vec::new());
        }
        let mut cursor = self
            .items()
            .find(doc! {"$text": {"$search": query}})
            .sort(doc! {"score": {"$meta": "textScore"}})
            .projection(doc! {"score": {"$meta": "textScore"}})
            .limit(n)
            .await
            .map_err(backend)?;
        // Unlike the other loops, the search returns the cursor's error, as Go does.
        let mut items = Vec::new();
        while cursor.advance().await.map_err(backend)? {
            let doc: Document = cursor.deserialize_current().map_err(backend)?;
            items.push(scored_item_from_document(&doc)?);
        }
        Ok(items)
    }

    async fn modify_item(
        &self,
        ctx: &Context,
        item_id: &str,
        patch: &ItemPatch,
    ) -> Result<(), Error> {
        check_ctx(ctx)?;
        let mut update = Document::new();
        if let Some(is_hidden) = patch.is_hidden {
            update.insert("ishidden", is_hidden);
        }
        if let Some(categories) = &patch.categories {
            update.insert("categories", strings_bson(categories));
        }
        if let Some(comment) = &patch.comment {
            update.insert("comment", comment.as_str());
        }
        if patch.labels != Labels::Nil {
            update.insert("labels", labels_to_bson(&patch.labels));
        }
        if let Some(timestamp) = &patch.timestamp {
            update.insert("timestamp", to_datetime(timestamp));
        }
        if update.is_empty() {
            return Ok(());
        }
        update.insert("updateat", to_datetime(&gotime::now()));
        self.items()
            .update_one(doc! {"itemid": {"$eq": item_id}}, doc! {"$set": update})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_items(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
    ) -> Result<(String, Vec<Item>), Error> {
        check_ctx(ctx)?;
        let cursor_item = decode_cursor(cursor)?;
        let mut filter = doc! {"itemid": {"$gt": cursor_item.as_str()}};
        if let Some(begin_time) = &begin_time {
            filter.insert("timestamp", doc! {"$gt": to_datetime(begin_time)});
        }
        let rows = self
            .items()
            .find(filter)
            .sort(doc! {"itemid": 1})
            .limit(n)
            .await
            .map_err(backend)?;
        let items = decode_all(rows, item_from_document).await?;
        // Go reads `items[n-1]` when the page is full, which panics for n = 0.
        // The port takes the empty cursor instead.
        let next = match items.last() {
            Some(last) if items.len() as i64 == n => last.item_id.as_str(),
            _ => "",
        };
        Ok((encode_cursor(next), items))
    }

    async fn get_latest_items(
        &self,
        ctx: &Context,
        n: i64,
        categories: &[String],
        after: Option<Time>,
    ) -> Result<Vec<Item>, Error> {
        check_ctx(ctx)?;
        let mut filter = doc! {"ishidden": {"$ne": true}};
        if !categories.is_empty() {
            filter.insert("categories", doc! {"$all": strings_bson(categories)});
        }
        if let Some(after) = &after {
            filter.insert("timestamp", doc! {"$gt": to_datetime(after)});
        }
        let rows = self
            .items()
            .find(filter)
            .sort(doc! {"timestamp": -1})
            .limit(n)
            .await
            .map_err(backend)?;
        decode_all(rows, item_from_document).await
    }

    fn get_item_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
        time_limit: Option<Time>,
    ) -> BoxStream<'a, Result<Vec<Item>, Error>> {
        // Go's stream runs on `context.Background()`, so the context does not stop it.
        let items = self.items();
        let find = async move {
            let mut filter = Document::new();
            if let Some(time_limit) = &time_limit {
                filter.insert("timestamp", doc! {"$gt": to_datetime(time_limit)});
            }
            items.find(filter).await.map_err(backend)
        };
        batches(find, item_from_document, batch_size)
    }

    async fn get_item_feedback(
        &self,
        ctx: &Context,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        check_ctx(ctx)?;
        let mut filter = doc! {
            "feedbackkey.itemid": {"$eq": item_id},
            "timestamp": {"$lte": to_datetime(&gotime::now())},
        };
        if !feedback_types.is_empty() {
            filter.insert("$or", or_of(feedback_type_conditions(feedback_types)));
        }
        let rows = self.feedback().find(filter).await.map_err(backend)?;
        decode_all(rows, feedback_from_document).await
    }

    async fn batch_insert_users(&self, ctx: &Context, users: &[User]) -> Result<(), Error> {
        check_ctx(ctx)?;
        let now = gotime::now();
        let collection = self.users();
        for user in users {
            collection
                .update_one(
                    doc! {"userid": {"$eq": user.user_id.as_str()}},
                    doc! {"$set": user_document(user, &now)},
                )
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn delete_user(&self, ctx: &Context, user_id: &str) -> Result<(), Error> {
        check_ctx(ctx)?;
        self.users()
            .delete_one(doc! {"userid": user_id})
            .await
            .map_err(backend)?;
        self.feedback()
            .delete_many(doc! {"feedbackkey.userid": {"$eq": user_id}})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_user(&self, ctx: &Context, user_id: &str) -> Result<User, Error> {
        check_ctx(ctx)?;
        match self
            .users()
            .find_one(doc! {"userid": user_id})
            .await
            .map_err(backend)?
        {
            Some(doc) => user_from_document(&doc),
            None => Err(StorageError::NotFound.into()),
        }
    }

    async fn modify_user(
        &self,
        ctx: &Context,
        user_id: &str,
        patch: &UserPatch,
    ) -> Result<(), Error> {
        check_ctx(ctx)?;
        let mut update = Document::new();
        if patch.labels != Labels::Nil {
            update.insert("labels", labels_to_bson(&patch.labels));
        }
        if let Some(comment) = &patch.comment {
            update.insert("comment", comment.as_str());
        }
        if update.is_empty() {
            return Ok(());
        }
        update.insert("updateat", to_datetime(&gotime::now()));
        self.users()
            .update_one(doc! {"userid": {"$eq": user_id}}, doc! {"$set": update})
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn get_users(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
    ) -> Result<(String, Vec<User>), Error> {
        check_ctx(ctx)?;
        let cursor_user = decode_cursor(cursor)?;
        let rows = self
            .users()
            .find(doc! {"userid": {"$gt": cursor_user.as_str()}})
            .sort(doc! {"userid": 1})
            .limit(n)
            .await
            .map_err(backend)?;
        let users = decode_all(rows, user_from_document).await?;
        let next = match users.last() {
            Some(last) if users.len() as i64 == n => last.user_id.as_str(),
            _ => "",
        };
        Ok((encode_cursor(next), users))
    }

    fn get_user_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
    ) -> BoxStream<'a, Result<Vec<User>, Error>> {
        // Go's stream runs on `context.Background()`, so the context does not stop it.
        let users = self.users();
        let find = async move { users.find(doc! {}).await.map_err(backend) };
        batches(find, user_from_document, batch_size)
    }

    async fn get_user_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        end_time: Option<Time>,
        feedback_types: &[FeedbackTypeExpression],
    ) -> Result<Vec<Feedback>, Error> {
        check_ctx(ctx)?;
        let mut filter = doc! {"feedbackkey.userid": {"$eq": user_id}};
        if let Some(end_time) = &end_time {
            filter.insert("timestamp", doc! {"$lte": to_datetime(end_time)});
        }
        if !feedback_types.is_empty() {
            let conditions = feedback_types
                .iter()
                .map(feedback_type_expression_to_mongo)
                .collect();
            filter.insert("$or", or_of(conditions));
        }
        let rows = self.feedback().find(filter).await.map_err(backend)?;
        decode_all(rows, feedback_from_document).await
    }

    async fn get_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<Vec<Feedback>, Error> {
        check_ctx(ctx)?;
        let mut filter = doc! {
            "feedbackkey.userid": {"$eq": user_id},
            "feedbackkey.itemid": {"$eq": item_id},
        };
        if !feedback_types.is_empty() {
            filter.insert("$or", or_of(feedback_type_conditions(feedback_types)));
        }
        let rows = self.feedback().find(filter).await.map_err(backend)?;
        decode_all(rows, feedback_from_document).await
    }

    async fn delete_user_item_feedback(
        &self,
        ctx: &Context,
        user_id: &str,
        item_id: &str,
        feedback_types: &[String],
    ) -> Result<i64, Error> {
        check_ctx(ctx)?;
        let mut filter = doc! {
            "feedbackkey.userid": {"$eq": user_id},
            "feedbackkey.itemid": {"$eq": item_id},
        };
        if !feedback_types.is_empty() {
            filter.insert(
                "feedbackkey.feedbacktype",
                doc! {"$in": strings_bson(feedback_types)},
            );
        }
        let result = self.feedback().delete_many(filter).await.map_err(backend)?;
        Ok(result.deleted_count as i64)
    }

    async fn batch_insert_feedback(
        &self,
        ctx: &Context,
        feedback: &[Feedback],
        insert_user: bool,
        insert_item: bool,
        overwrite: bool,
    ) -> Result<(), Error> {
        check_ctx(ctx)?;
        if feedback.is_empty() {
            return Ok(());
        }
        // Go's sets, whose order does not matter here, so an insertion order is kept.
        let mut users: IndexSet<String> = IndexSet::new();
        let mut items: IndexSet<String> = IndexSet::new();
        for f in feedback {
            users.insert(f.feedback_key.user_id.clone());
            items.insert(f.feedback_key.item_id.clone());
        }
        let now = gotime::now();
        if insert_user {
            let collection = self.users();
            for user_id in &users {
                let user = User {
                    user_id: user_id.clone(),
                    ..User::default()
                };
                collection
                    .update_one(
                        doc! {"userid": {"$eq": user_id.as_str()}},
                        doc! {"$setOnInsert": user_document(&user, &now)},
                    )
                    .upsert(true)
                    .await
                    .map_err(backend)?;
            }
        } else {
            for user_id in users.clone() {
                match self.get_user(ctx, &user_id).await {
                    Ok(_) => {}
                    Err(err) if err.kind() == Some(StorageError::NotFound) => {
                        users.swap_remove(&user_id);
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        if insert_item {
            let collection = self.items();
            for item_id in &items {
                let item = Item {
                    item_id: item_id.clone(),
                    ..Item::default()
                };
                collection
                    .update_one(
                        doc! {"itemid": {"$eq": item_id.as_str()}},
                        doc! {"$setOnInsert": item_document(&item, &now)},
                    )
                    .upsert(true)
                    .await
                    .map_err(backend)?;
            }
        } else {
            for item_id in items.clone() {
                match self.get_item(ctx, &item_id).await {
                    Ok(_) => {}
                    Err(err) if err.kind() == Some(StorageError::NotFound) => {
                        items.swap_remove(&item_id);
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        let collection = self.feedback();
        for f in feedback {
            if !users.contains(&f.feedback_key.user_id) || !items.contains(&f.feedback_key.item_id)
            {
                continue;
            }
            let mut f = f.clone();
            f.updated = f.timestamp.clone();
            let filter = doc! {"feedbackkey": feedback_key_document(&f.feedback_key)};
            let update = if overwrite {
                doc! {"$set": feedback_document(&f)}
            } else {
                doc! {
                    "$setOnInsert": {"feedbackkey": feedback_key_document(&f.feedback_key)},
                    "$inc": {"value": f.value},
                    "$min": {"timestamp": to_datetime(&f.timestamp)},
                    "$max": {"updated": to_datetime(&f.updated)},
                    "$set": {
                        "labels": labels_to_bson(&f.labels),
                        "comment": f.comment.as_str(),
                    },
                }
            };
            collection
                .update_one(filter, update)
                .upsert(true)
                .await
                .map_err(backend)?;
        }
        Ok(())
    }

    async fn get_feedback(
        &self,
        ctx: &Context,
        cursor: &str,
        n: i64,
        begin_time: Option<Time>,
        end_time: Option<Time>,
        feedback_types: &[String],
    ) -> Result<(String, Vec<Feedback>), Error> {
        check_ctx(ctx)?;
        let buf = GO_STANDARD.decode(cursor).map_err(backend)?;
        let mut filter = Document::new();
        // A cursor that is not empty names the last feedback of the page before.
        if !buf.is_empty() {
            let key = feedback_key_from_json(&buf)?;
            filter.insert("feedbackkey", doc! {"$gt": feedback_key_document(&key)});
        }
        if !feedback_types.is_empty() {
            filter.insert("$or", or_of(feedback_type_conditions(feedback_types)));
        }
        // Go always sets the `timestamp` condition, even when it is empty.
        // An empty document is an exact match for an empty document, so a
        // query with neither bound finds nothing. The port reproduces that.
        let mut timestamp = Document::new();
        if let Some(begin_time) = &begin_time {
            timestamp.insert("$gt", to_datetime(begin_time));
        }
        if let Some(end_time) = &end_time {
            timestamp.insert("$lte", to_datetime(end_time));
        }
        filter.insert("timestamp", timestamp);
        let rows = self
            .feedback()
            .find(filter)
            .sort(doc! {"feedbackkey": 1})
            .limit(n)
            .await
            .map_err(backend)?;
        let feedback = decode_all(rows, feedback_from_document).await?;
        let next = match feedback.last() {
            Some(last) if feedback.len() as i64 == n => feedback_key_json(&last.feedback_key),
            _ => String::new(),
        };
        Ok((encode_cursor(&next), feedback))
    }

    fn get_feedback_stream<'a>(
        &'a self,
        _ctx: &'a Context,
        batch_size: i64,
        options: &[ScanOption],
    ) -> BoxStream<'a, Result<Vec<Feedback>, Error>> {
        // Go's stream runs on `context.Background()`, so the context does not stop it.
        let scan: ScanOptions = new_scan_options(options);
        let collection = self.feedback();
        let find = async move {
            let mut filter = Document::new();
            if !scan.feedback_types.is_empty() {
                let conditions = scan
                    .feedback_types
                    .iter()
                    .map(feedback_type_expression_to_mongo)
                    .collect();
                filter.insert("$or", or_of(conditions));
            }
            if scan.begin_time.is_some() || scan.end_time.is_some() {
                let mut timestamp = Document::new();
                if let Some(begin_time) = &scan.begin_time {
                    timestamp.insert("$gt", to_datetime(begin_time));
                }
                if let Some(end_time) = &scan.end_time {
                    timestamp.insert("$lte", to_datetime(end_time));
                }
                filter.insert("timestamp", timestamp);
            }
            if scan.begin_user_id.is_some() || scan.end_user_id.is_some() {
                let mut user_id = Document::new();
                if let Some(begin) = &scan.begin_user_id {
                    user_id.insert("$gte", begin.as_str());
                }
                if let Some(end) = &scan.end_user_id {
                    user_id.insert("$lte", end.as_str());
                }
                filter.insert("feedbackkey.userid", user_id);
            }
            if scan.begin_item_id.is_some() || scan.end_item_id.is_some() {
                let mut item_id = Document::new();
                if let Some(begin) = &scan.begin_item_id {
                    item_id.insert("$gte", begin.as_str());
                }
                if let Some(end) = &scan.end_item_id {
                    item_id.insert("$lte", end.as_str());
                }
                filter.insert("feedbackkey.itemid", item_id);
            }
            let mut find = collection.find(filter);
            if scan.order_by_item_id {
                find = find.sort(doc! {"feedbackkey.itemid": 1});
            }
            find.await.map_err(backend)
        };
        batches(find, feedback_from_document, batch_size)
    }

    async fn count_users(&self, ctx: &Context) -> Result<i64, Error> {
        check_ctx(ctx)?;
        let n = self
            .users()
            .estimated_document_count()
            .await
            .map_err(backend)?;
        Ok(n as i64)
    }

    async fn count_items(&self, ctx: &Context) -> Result<i64, Error> {
        check_ctx(ctx)?;
        let n = self
            .items()
            .estimated_document_count()
            .await
            .map_err(backend)?;
        Ok(n as i64)
    }

    async fn count_feedback(&self, ctx: &Context) -> Result<i64, Error> {
        check_ctx(ctx)?;
        let n = self
            .feedback()
            .estimated_document_count()
            .await
            .map_err(backend)?;
        Ok(n as i64)
    }
}

/// The future a [`creator`] returns.
pub type OpenFuture<'a> = BoxFuture<'a, Result<Box<dyn Database>, Error>>;

/// The creator for `mongodb://` and `mongodb+srv://` paths, in the form the
/// data registry takes (D-157): it opens the store [`MongoDB::open`] makes.
/// Go's creator takes options and ignores them, and so does this.
pub fn creator<'a>(path: &'a str, table_prefix: &'a str, _opts: &'a [Opt]) -> OpenFuture<'a> {
    Box::pin(async move {
        let db = MongoDB::open(path, table_prefix).await?;
        Ok(Box::new(db) as Box<dyn Database>)
    })
}

/// Registers [`creator`] for both MongoDB prefixes. Go registers the creator in
/// `init`; a node calls this once on the registry it builds.
pub fn register(registry: &mut Registry) {
    registry.register(&[MONGO_PREFIX, MONGO_SRV_PREFIX], creator);
}

#[cfg(test)]
mod tests {
    use super::super::database_test::{self, Backend, TimeStamps};
    use super::super::types::nested_labels;
    use super::*;

    impl Backend for MongoDB {
        /// MongoDB keeps time stamps to the millisecond.
        fn time_stamps(&self) -> TimeStamps {
            TimeStamps::Milliseconds
        }
    }

    /// The creator opens the same store as [`MongoDB::open`], as a boxed
    /// [`Database`]. Skipped without `MONGO_URI`, as Go's `TestMongo` is.
    #[tokio::test]
    async fn creator_opens_the_store() {
        let Ok(uri) = std::env::var("MONGO_URI") else {
            panic!("MONGO_URI is not set");
        };
        let db = creator(
            &format!("{uri}gorse_data_creator_test?authSource=admin&connect=direct"),
            "gorse_",
            &[],
        )
        .await
        .unwrap();
        db.ping().await.unwrap();
        db.close().await.unwrap();
    }

    /// Go's `TestMongo`: runs the shared suite against a real MongoDB, in the
    /// database `gorse_data_test`, which is dropped first, and creates the
    /// schema there. Without `MONGO_URI` the test is skipped, as Go's
    /// `t.Skip` does.
    #[tokio::test]
    async fn mongo() {
        let Ok(uri) = std::env::var("MONGO_URI") else {
            panic!("MONGO_URI is not set");
        };
        let db_name = "gorse_data_test";
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

    #[test]
    fn driver_uri_maps_go_connect_options() {
        assert_eq!(
            driver_uri("mongodb://h:27017/d?authSource=admin&connect=direct"),
            "mongodb://h:27017/d?authSource=admin&directConnection=true"
        );
        assert_eq!(
            driver_uri("mongodb://h:27017/d?connect=automatic"),
            "mongodb://h:27017/d"
        );
        assert_eq!(driver_uri("mongodb://h:27017/"), "mongodb://h:27017/");
    }

    #[test]
    fn search_columns_map_to_fields_as_go_does() {
        assert_eq!(
            mongo_search_column_to_field("item.ItemId").as_deref(),
            Some("itemid")
        );
        assert_eq!(
            mongo_search_column_to_field("item.Categories").as_deref(),
            Some("categories")
        );
        assert_eq!(
            mongo_search_column_to_field("item.Comment").as_deref(),
            Some("comment")
        );
        assert_eq!(
            mongo_search_column_to_field("item.Labels").as_deref(),
            Some("labels")
        );
        assert_eq!(
            mongo_search_column_to_field("item.Labels.title").as_deref(),
            Some("labels.title")
        );
        assert_eq!(mongo_search_column_to_field("item.Labels."), None);
        assert_eq!(mongo_search_column_to_field("user.UserId"), None);
    }

    #[test]
    fn labels_nested_ten_thousand_levels_convert_both_ways() {
        convert_labels_both_ways(10_000);
    }

    #[test]
    fn labels_nested_one_hundred_thousand_levels_convert_both_ways() {
        convert_labels_both_ways(100_000);
    }

    /// Converts labels nested `depth` levels to BSON and back, and compares the
    /// text. The BSON is dropped by [`drop_bson_iteratively`].
    fn convert_labels_both_ways(depth: usize) {
        let (labels, text) = nested_labels(depth);
        let bson = labels_to_bson(&labels);
        let read = labels_from_bson(&bson).expect("unpack");
        let written = String::from_utf8(gorse_common::jsonutil::marshal(&read).expect("marshal"))
            .expect("utf-8");
        assert!(written == text, "labels changed through BSON");
        drop_bson_iteratively(bson);
    }

    /// Drops a BSON value without recursing. The `bson` crate drops nested
    /// values recursively, which overflows the stack at these depths.
    fn drop_bson_iteratively(value: Bson) {
        let mut pending = vec![value];
        while let Some(mut value) = pending.pop() {
            match &mut value {
                Bson::Array(values) => pending.append(values),
                Bson::Document(entries) => {
                    pending.extend(std::mem::take(entries).into_iter().map(|(_, value)| value));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn labels_survive_a_round_trip() {
        let labels = Labels::Object(
            [
                ("title".to_owned(), Labels::String("Hello".to_owned())),
                (
                    "embedding".to_owned(),
                    Labels::Array(vec![Labels::Float64(0.25), Labels::Float64(0.5)]),
                ),
                ("nothing".to_owned(), Labels::Nil),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(labels_from_bson(&labels_to_bson(&labels)).unwrap(), labels);
    }

    /// The stored types of JSON numbers, as the Go driver stores them (probed
    /// against MongoDB 6 with the Go store: `n: 3` came back as an int64).
    #[test]
    fn json_numbers_are_stored_as_go_stores_them() {
        assert_eq!(number_to_bson("3"), Bson::Int64(3));
        assert_eq!(number_to_bson("3.0"), Bson::Double(3.0));
        assert_eq!(number_to_bson("1.5"), Bson::Double(1.5));
        assert_eq!(
            labels_from_bson(&number_to_bson("3")).unwrap(),
            Labels::Number("3".to_owned())
        );
    }

    /// Go decodes "QR==" to "A", with non-zero trailing bits, and so does the port.
    #[test]
    fn cursors_decode_as_go_decodes_them() {
        assert_eq!(decode_cursor("QR==").unwrap(), "A");
        assert_eq!(decode_cursor("").unwrap(), "");
        assert_eq!(encode_cursor("i1"), "aTE=");
    }

    #[test]
    fn feedback_key_cursor_matches_go_json() {
        let key = FeedbackKey {
            feedback_type: "like".to_owned(),
            user_id: "<u&1>".to_owned(),
            item_id: "i\"1".to_owned(),
        };
        let text = feedback_key_json(&key);
        assert_eq!(
            text,
            r#"{"FeedbackType":"like","UserId":"\u003cu\u00261\u003e","ItemId":"i\"1"}"#
        );
        assert_eq!(feedback_key_from_json(text.as_bytes()).unwrap(), key);
    }
}
