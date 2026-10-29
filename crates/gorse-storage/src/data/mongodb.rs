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
use mongodb::bson::raw::{RawBsonRef, RawDocumentBuf};
use mongodb::bson::{self, Bson, DateTime, Document, doc};
use mongodb::error::ErrorKind;
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

    /// The users collection. Its documents are read raw, so that their labels
    /// are never a BSON tree; see [`split_stored`].
    fn users(&self) -> Collection<RawDocumentBuf> {
        self.db().collection(&self.users_table)
    }

    /// The items collection, read raw as [`MongoDB::users`] is.
    fn items(&self) -> Collection<RawDocumentBuf> {
        self.db().collection(&self.items_table)
    }

    /// The feedback collection, read raw as [`MongoDB::users`] is.
    fn feedback(&self) -> Collection<RawDocumentBuf> {
        self.db().collection(&self.feedback_table)
    }

    /// Applies one update, whose update document `update` is raw BSON, to the
    /// documents `filter` matches in the collection named `collection`. The
    /// update is sent as a raw `update` command; see [`update_command`].
    async fn update_raw(
        &self,
        collection: &str,
        filter: &Document,
        update: &RawDocumentBuf,
        upsert: bool,
    ) -> Result<(), Error> {
        let command = update_command(collection, filter, update, upsert)?;
        let reply = self.db().run_raw_command(command).await.map_err(refused)?;
        write_refused(&reply)
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

/// A list of strings as a BSON array.
fn strings_bson(values: &[String]) -> Bson {
    Bson::Array(values.iter().map(|v| Bson::String(v.clone())).collect())
}

/// The error for a field name with a zero byte, which BSON cannot hold. The
/// text is the one the BSON encoder gives.
#[derive(Debug, thiserror::Error)]
#[error("cstrings cannot contain null bytes: {0:?}")]
struct InvalidName(String);

/// The error for a write or command the server refused, worded as Go's
/// driver words it, since the labels of a document travel in it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Refused(String);

/// A BSON document written into one buffer, as the driver encodes it. The
/// driver walks a `Document` recursively, so a document that carries labels
/// is written here instead, field by field, and the labels are written by an
/// explicit stack. Each document or array has its length written as a
/// placeholder that is set when it closes, so every byte is written once,
/// however deep the labels are nested.
struct Writer {
    bytes: Vec<u8>,
    /// Where each open document or array starts, so that its length can be
    /// set when it closes.
    open: Vec<usize>,
}

impl Writer {
    /// A writer for a document, which [`Writer::finish`] closes.
    fn new() -> Writer {
        let mut writer = Writer {
            bytes: Vec::new(),
            open: Vec::new(),
        };
        writer.open_length();
        writer
    }

    /// Writes the type and the name of a field.
    fn element(&mut self, kind: u8, name: &str) -> Result<(), Error> {
        if name.contains('\0') {
            return Err(backend(InvalidName(name.to_owned())));
        }
        self.bytes.push(kind);
        self.bytes.extend_from_slice(name.as_bytes());
        self.bytes.push(0);
        Ok(())
    }

    /// Writes a placeholder length for the document or array that starts here.
    fn open_length(&mut self) {
        self.open.push(self.bytes.len());
        self.bytes.extend_from_slice(&[0; 4]);
    }

    /// Opens an embedded document (`kind` 0x03) or an array (0x04) as the
    /// field `name`. It is closed by [`Writer::close`].
    fn open(&mut self, kind: u8, name: &str) -> Result<(), Error> {
        self.element(kind, name)?;
        self.open_length();
        Ok(())
    }

    /// Closes the document or array that was opened last.
    fn close(&mut self) -> Result<(), Error> {
        self.bytes.push(0);
        let start = self.open.pop().expect("a document is open to close");
        let length = i32::try_from(self.bytes.len() - start).map_err(backend)?;
        self.bytes[start..start + 4].copy_from_slice(&length.to_le_bytes());
        Ok(())
    }

    /// Closes the top-level document, which [`Writer::new`] opened, and returns
    /// its bytes as a document.
    fn finish(mut self) -> Result<RawDocumentBuf, Error> {
        debug_assert_eq!(self.open.len(), 1, "only the top-level document is open");
        self.close()?;
        RawDocumentBuf::from_bytes(self.bytes).map_err(backend)
    }

    /// Writes a BSON string: its length with the zero that ends it, the bytes
    /// and the zero.
    fn text(&mut self, value: &str) -> Result<(), Error> {
        let length = i32::try_from(value.len() + 1).map_err(backend)?;
        self.bytes.extend_from_slice(&length.to_le_bytes());
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        Ok(())
    }

    fn string(&mut self, name: &str, value: &str) -> Result<(), Error> {
        self.element(0x02, name)?;
        self.text(value)
    }

    /// An embedded document, written as the driver writes a `Document`.
    fn document(&mut self, name: &str, document: &Document) -> Result<(), Error> {
        let raw = RawDocumentBuf::from_document(document).map_err(backend)?;
        self.raw_document(name, &raw)
    }

    /// An embedded document that is already written.
    fn raw_document(&mut self, name: &str, document: &RawDocumentBuf) -> Result<(), Error> {
        self.element(0x03, name)?;
        self.bytes.extend_from_slice(document.as_bytes());
        Ok(())
    }

    /// A list of strings as a BSON array, with the indexes as its keys.
    fn strings(&mut self, name: &str, values: &[String]) -> Result<(), Error> {
        self.open(0x04, name)?;
        for (index, value) in values.iter().enumerate() {
            self.string(&index.to_string(), value)?;
        }
        self.close()
    }

    fn boolean(&mut self, name: &str, value: bool) -> Result<(), Error> {
        self.element(0x08, name)?;
        self.bytes.push(u8::from(value));
        Ok(())
    }

    fn null(&mut self, name: &str) -> Result<(), Error> {
        self.element(0x0A, name)
    }

    fn double(&mut self, name: &str, value: f64) -> Result<(), Error> {
        self.element(0x01, name)?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn int64(&mut self, name: &str, value: i64) -> Result<(), Error> {
        self.element(0x12, name)?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn datetime(&mut self, name: &str, value: DateTime) -> Result<(), Error> {
        self.element(0x09, name)?;
        self.bytes
            .extend_from_slice(&value.timestamp_millis().to_le_bytes());
        Ok(())
    }

    /// Go's `json.Number` as BSON. The Go driver's encoder writes it as an
    /// int64 when its text is an int64, and as a double otherwise
    /// (`JSONNumberEncodeValue`). Text that is neither cannot come from JSON;
    /// the port writes it as a string, where Go's encoder returns an error.
    fn number(&mut self, name: &str, text: &str) -> Result<(), Error> {
        if let Ok(value) = text.parse::<i64>() {
            self.int64(name, value)
        } else if let Ok(value) = text.parse::<f64>() {
            self.double(name, value)
        } else {
            self.string(name, text)
        }
    }

    /// Writes one label as the field `name`. A scalar is written at once. A
    /// list or a map is opened, and its children are written by the loop in
    /// [`Writer::labels`], which takes the frame from `frames`.
    fn label<'a>(
        &mut self,
        name: &str,
        label: &'a Labels,
        frames: &mut Vec<WriteFrame<'a>>,
    ) -> Result<(), Error> {
        match label {
            Labels::Nil => self.null(name),
            Labels::Bool(value) => self.boolean(name, *value),
            Labels::Float64(value) => self.double(name, *value),
            Labels::Number(text) => self.number(name, text),
            Labels::String(text) => self.string(name, text),
            Labels::Array(values) => {
                self.open(0x04, name)?;
                frames.push(WriteFrame::List(values.iter(), 0));
                Ok(())
            }
            Labels::Object(entries) => {
                self.open(0x03, name)?;
                frames.push(WriteFrame::Map(entries.iter()));
                Ok(())
            }
        }
    }

    /// Go's `any` labels, written as the field `name`. The lists and maps that
    /// the walk is inside are kept on a stack, not by recursion, so labels
    /// nested at any depth are written in full.
    fn labels(&mut self, name: &str, labels: &Labels) -> Result<(), Error> {
        let mut frames = Vec::new();
        self.label(name, labels, &mut frames)?;
        while let Some(frame) = frames.last_mut() {
            let next = match frame {
                WriteFrame::List(children, index) => children.next().map(|child| {
                    let name = index.to_string();
                    *index += 1;
                    (name, child)
                }),
                WriteFrame::Map(children) => {
                    children.next().map(|(key, child)| (key.clone(), child))
                }
            };
            match next {
                Some((name, child)) => self.label(&name, child, &mut frames)?,
                None => {
                    frames.pop();
                    self.close()?;
                }
            }
        }
        Ok(())
    }
}

/// A list or a map whose children are being written: the children it has yet
/// to write, and for a list the index of the next one.
enum WriteFrame<'a> {
    List(std::slice::Iter<'a, Labels>, usize),
    Map(indexmap::map::Iter<'a, String, Labels>),
}

/// Go's `unpack`: a BSON value as the `any` Go reads it. A document becomes a
/// map and an array becomes a slice. Go does not unpack an array's elements,
/// but Gorse's labels validation keeps arrays to strings and numbers, where
/// that makes no difference, so the elements are unpacked here too. The lists
/// and documents being unpacked are kept on a stack, so a value nested at any
/// depth is unpacked in full.
fn labels_from_raw(value: RawBsonRef<'_>) -> Result<Labels, Error> {
    /// A list or a document being unpacked: the children it has yet to take,
    /// what it has so far, and for a document the name of the child in progress.
    enum Open<'a> {
        List(std::vec::IntoIter<RawBsonRef<'a>>, Vec<Labels>),
        Document(
            std::vec::IntoIter<(&'a str, RawBsonRef<'a>)>,
            IndexMap<String, Labels>,
            Option<&'a str>,
        ),
    }
    let mut open: Vec<Open<'_>> = Vec::new();
    let mut child = value;
    loop {
        let mut finished = match child {
            RawBsonRef::Null => Some(Labels::Nil),
            RawBsonRef::Boolean(value) => Some(Labels::Bool(value)),
            RawBsonRef::Double(value) => Some(Labels::Float64(value)),
            // Go keeps an int32 or int64 as it is. Its JSON text is the same as a
            // `json.Number`'s, which is how Gorse's labels hold a number.
            RawBsonRef::Int32(value) => Some(Labels::Number(value.to_string())),
            RawBsonRef::Int64(value) => Some(Labels::Number(value.to_string())),
            RawBsonRef::String(text) => Some(Labels::String(text.to_owned())),
            RawBsonRef::Array(values) => {
                let children = values
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(backend)?;
                open.push(Open::List(children.into_iter(), Vec::new()));
                None
            }
            RawBsonRef::Document(entries) => {
                let children = entries
                    .iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(backend)?;
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
                        entries.insert(name.to_owned(), value);
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

/// A stored document as the server sends it, read into its labels and its
/// other fields. The labels are unpacked from the raw bytes by
/// [`labels_from_raw`], and the other fields are shallow, so they are read
/// into a `Document`.
fn split_stored(raw: &RawDocumentBuf) -> Result<(Document, Labels), Error> {
    let mut fields = Document::new();
    let mut labels = Labels::default();
    for entry in raw.iter() {
        let (key, value) = entry.map_err(backend)?;
        if key == "labels" {
            labels = labels_from_raw(value)?;
        } else {
            fields.insert(key, Bson::try_from(value).map_err(backend)?);
        }
    }
    Ok((fields, labels))
}

/// Decodes a stored document with the decoder of its type.
fn decode_stored<T>(
    raw: &RawDocumentBuf,
    decode: fn(&Document, Labels) -> Result<T, Error>,
) -> Result<T, Error> {
    let (doc, labels) = split_stored(raw)?;
    decode(&doc, labels)
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

/// Go's `GetItem` and `BatchGetItems` decoding: an `Item` from its document,
/// and its labels, which [`split_stored`] reads.
fn item_from_document(doc: &Document, labels: Labels) -> Result<Item, Error> {
    Ok(Item {
        item_id: string_field(doc, "itemid")?,
        is_hidden: bool_field(doc, "ishidden")?,
        categories: categories_field(doc, "categories")?,
        timestamp: time_field(doc, "timestamp")?,
        update_at: time_field(doc, "updateat")?,
        labels,
        comment: string_field(doc, "comment")?,
    })
}

/// Go's `ScoredItem` decoding: an item with its `score`.
fn scored_item_from_document(doc: &Document, labels: Labels) -> Result<ScoredItem, Error> {
    Ok(ScoredItem {
        item: item_from_document(doc, labels)?,
        score: f64_field(doc, "score")?,
    })
}

/// Go's `User` decoding.
fn user_from_document(doc: &Document, labels: Labels) -> Result<User, Error> {
    Ok(User {
        user_id: string_field(doc, "userid")?,
        labels,
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
fn feedback_from_document(doc: &Document, labels: Labels) -> Result<Feedback, Error> {
    Ok(Feedback {
        feedback_key: feedback_key_from_document(doc)?,
        value: f64_field(doc, "value")?,
        timestamp: time_field(doc, "timestamp")?,
        updated: time_field(doc, "updated")?,
        labels,
        comment: string_field(doc, "comment")?,
    })
}

/// Writes the fields of an item, in the order Go writes them, with `updateat`
/// set to `update_at`.
fn write_item(writer: &mut Writer, item: &Item, update_at: &Time) -> Result<(), Error> {
    writer.string("itemid", &item.item_id)?;
    writer.boolean("ishidden", item.is_hidden)?;
    match &item.categories {
        None => writer.null("categories")?,
        Some(categories) => writer.strings("categories", categories)?,
    }
    writer.datetime("timestamp", to_datetime(&item.timestamp))?;
    writer.datetime("updateat", to_datetime(update_at))?;
    writer.labels("labels", &item.labels)?;
    writer.string("comment", &item.comment)
}

/// Writes the fields of a user, in the order Go writes them, with `updateat`
/// set to `update_at`.
fn write_user(writer: &mut Writer, user: &User, update_at: &Time) -> Result<(), Error> {
    writer.string("userid", &user.user_id)?;
    writer.labels("labels", &user.labels)?;
    writer.string("comment", &user.comment)?;
    writer.datetime("updateat", to_datetime(update_at))
}

/// Writes the fields of a feedback record, in the order Go writes them.
fn write_feedback(writer: &mut Writer, feedback: &Feedback) -> Result<(), Error> {
    writer.document(
        "feedbackkey",
        &feedback_key_document(&feedback.feedback_key),
    )?;
    writer.double("value", feedback.value)?;
    writer.datetime("timestamp", to_datetime(&feedback.timestamp))?;
    writer.datetime("updated", to_datetime(&feedback.updated))?;
    writer.labels("labels", &feedback.labels)?;
    writer.string("comment", &feedback.comment)
}

/// An update that sets the fields `set` writes, as Go's `$set` does.
fn set_update(set: impl FnOnce(&mut Writer) -> Result<(), Error>) -> Result<RawDocumentBuf, Error> {
    operator_update("$set", set)
}

/// An update with one operator, whose fields `fields` writes.
fn operator_update(
    operator: &str,
    fields: impl FnOnce(&mut Writer) -> Result<(), Error>,
) -> Result<RawDocumentBuf, Error> {
    let mut update = Writer::new();
    update.open(0x03, operator)?;
    fields(&mut update)?;
    update.close()?;
    update.finish()
}

/// The update Go applies for a feedback record. With `overwrite` the record is
/// set as it is. Otherwise the key is set on insert, the value and the times
/// are merged in, and the labels and comment are set.
fn feedback_update(feedback: &Feedback, overwrite: bool) -> Result<RawDocumentBuf, Error> {
    if overwrite {
        return set_update(|set| write_feedback(set, feedback));
    }
    let mut update = Writer::new();
    update.document(
        "$setOnInsert",
        &doc! {"feedbackkey": feedback_key_document(&feedback.feedback_key)},
    )?;
    update.document("$inc", &doc! {"value": feedback.value})?;
    update.document(
        "$min",
        &doc! {"timestamp": to_datetime(&feedback.timestamp)},
    )?;
    update.document("$max", &doc! {"updated": to_datetime(&feedback.updated)})?;
    update.open(0x03, "$set")?;
    update.labels("labels", &feedback.labels)?;
    update.string("comment", &feedback.comment)?;
    update.close()?;
    update.finish()
}

/// The `update` command for one update of the documents `filter` matches in
/// `collection`. The driver takes an update as a `Document`, which can hold
/// labels only as a BSON tree, so the command is built as raw BSON and the
/// driver sends its bytes as they are.
fn update_command(
    collection: &str,
    filter: &Document,
    update: &RawDocumentBuf,
    upsert: bool,
) -> Result<RawDocumentBuf, Error> {
    let mut statement = Writer::new();
    statement.document("q", filter)?;
    statement.raw_document("u", update)?;
    statement.boolean("upsert", upsert)?;
    statement.boolean("multi", false)?;
    let statement = statement.finish()?;
    let mut command = Writer::new();
    command.string("update", collection)?;
    command.open(0x04, "updates")?;
    command.raw_document("0", &statement)?;
    command.close()?;
    command.finish()
}

/// The error for a command the server refused, with the code name in brackets
/// as Go's driver gives it. Other errors keep the driver's own text.
fn refused(err: mongodb::error::Error) -> Error {
    match err.kind.as_ref() {
        ErrorKind::Command(command) if command.code_name.is_empty() => {
            backend(Refused(command.message.clone()))
        }
        ErrorKind::Command(command) => backend(Refused(format!(
            "({}) {}",
            command.code_name, command.message
        ))),
        _ => backend(err),
    }
}

/// The error for the write errors in the reply to an update, worded as Go's
/// `WriteException` is: each error's message, in a list.
fn write_refused(reply: &Document) -> Result<(), Error> {
    let Ok(errors) = reply.get_array("writeErrors") else {
        return Ok(());
    };
    if errors.is_empty() {
        return Ok(());
    }
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(Bson::as_document)
        .map(|error| error.get_str("errmsg").unwrap_or_default())
        .collect();
    Err(backend(Refused(format!(
        "write exception: write errors: [{}]",
        messages.join(", ")
    ))))
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
    mut cursor: Cursor<RawDocumentBuf>,
    decode: fn(&Document, Labels) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let mut rows = Vec::new();
    while let Ok(true) = cursor.advance().await {
        let raw: RawDocumentBuf = cursor.deserialize_current().map_err(backend)?;
        rows.push(decode_stored(&raw, decode)?);
    }
    Ok(rows)
}

/// The batches of a Go stream. Go's goroutine sends a full batch as soon as it
/// has `batch_size` rows, then the rest at the end, and an error ends the
/// stream without the partial batch. The query runs when the stream is first
/// polled. A `batch_size` that is not positive never fills a batch, so the
/// whole scan comes as one batch, as it does in Go.
fn batches<'a, T>(
    find: impl std::future::Future<Output = Result<Cursor<RawDocumentBuf>, Error>> + Send + 'a,
    decode: fn(&Document, Labels) -> Result<T, Error>,
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
                let raw: RawDocumentBuf = cursor.deserialize_current().map_err(backend)?;
                batch.push(decode_stored(&raw, decode)?);
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
        for item in items {
            let update = set_update(|set| write_item(set, item, &now))?;
            self.update_raw(
                &self.items_table,
                &doc! {"itemid": {"$eq": item.item_id.as_str()}},
                &update,
                true,
            )
            .await?;
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
            Some(raw) => decode_stored(&raw, item_from_document),
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
            let raw: RawDocumentBuf = cursor.deserialize_current().map_err(backend)?;
            items.push(decode_stored(&raw, scored_item_from_document)?);
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
        let sets_something = patch.is_hidden.is_some()
            || patch.categories.is_some()
            || patch.comment.is_some()
            || patch.labels != Labels::Nil
            || patch.timestamp.is_some();
        if !sets_something {
            return Ok(());
        }
        let update = set_update(|set| {
            if let Some(is_hidden) = patch.is_hidden {
                set.boolean("ishidden", is_hidden)?;
            }
            if let Some(categories) = &patch.categories {
                set.strings("categories", categories)?;
            }
            if let Some(comment) = &patch.comment {
                set.string("comment", comment)?;
            }
            if patch.labels != Labels::Nil {
                set.labels("labels", &patch.labels)?;
            }
            if let Some(timestamp) = &patch.timestamp {
                set.datetime("timestamp", to_datetime(timestamp))?;
            }
            set.datetime("updateat", to_datetime(&gotime::now()))
        })?;
        self.update_raw(
            &self.items_table,
            &doc! {"itemid": {"$eq": item_id}},
            &update,
            false,
        )
        .await
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
        for user in users {
            let update = set_update(|set| write_user(set, user, &now))?;
            self.update_raw(
                &self.users_table,
                &doc! {"userid": {"$eq": user.user_id.as_str()}},
                &update,
                true,
            )
            .await?;
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
            Some(raw) => decode_stored(&raw, user_from_document),
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
        if patch.labels == Labels::Nil && patch.comment.is_none() {
            return Ok(());
        }
        let update = set_update(|set| {
            if patch.labels != Labels::Nil {
                set.labels("labels", &patch.labels)?;
            }
            if let Some(comment) = &patch.comment {
                set.string("comment", comment)?;
            }
            set.datetime("updateat", to_datetime(&gotime::now()))
        })?;
        self.update_raw(
            &self.users_table,
            &doc! {"userid": {"$eq": user_id}},
            &update,
            false,
        )
        .await
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
            for user_id in &users {
                let user = User {
                    user_id: user_id.clone(),
                    ..User::default()
                };
                let update = operator_update("$setOnInsert", |set| write_user(set, &user, &now))?;
                self.update_raw(
                    &self.users_table,
                    &doc! {"userid": {"$eq": user_id.as_str()}},
                    &update,
                    true,
                )
                .await?;
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
            for item_id in &items {
                let item = Item {
                    item_id: item_id.clone(),
                    ..Item::default()
                };
                let update = operator_update("$setOnInsert", |set| write_item(set, &item, &now))?;
                self.update_raw(
                    &self.items_table,
                    &doc! {"itemid": {"$eq": item_id.as_str()}},
                    &update,
                    true,
                )
                .await?;
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
        for f in feedback {
            if !users.contains(&f.feedback_key.user_id) || !items.contains(&f.feedback_key.item_id)
            {
                continue;
            }
            let mut f = f.clone();
            f.updated = f.timestamp.clone();
            let filter = doc! {"feedbackkey": feedback_key_document(&f.feedback_key)};
            let update = feedback_update(&f, overwrite)?;
            self.update_raw(&self.feedback_table, &filter, &update, true)
                .await?;
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
    use super::super::types::{labels_json, nested_labels};
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

    /// Writes labels nested `depth` levels as the field `labels` of a document,
    /// then reads the document back as the driver reads a cursor's documents,
    /// and compares the text. Go's BSON has no nesting limit, unlike its JSON,
    /// so these convert in full. What MongoDB accepts is decided by the live
    /// conformance run.
    fn convert_labels_both_ways(depth: usize) {
        let (labels, text) = nested_labels(depth);
        let mut written = Writer::new();
        written.labels("labels", &labels).expect("write");
        let bytes = written.finish().expect("document");
        let raw: RawDocumentBuf =
            bson::from_slice(bytes.as_bytes()).expect("read the document as a cursor does");
        let (_, read) = split_stored(&raw).expect("unpack");
        assert!(
            labels_json(&read).expect("write") == text,
            "labels changed through BSON"
        );
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
        let mut written = Writer::new();
        written.labels("labels", &labels).expect("write");
        let raw = written.finish().expect("document");
        let (_, read) = split_stored(&raw).expect("unpack");
        assert_eq!(read, labels);
    }

    /// The bytes of labels are the bytes the driver writes for the same labels
    /// as a `Document`, so the stored documents do not change.
    #[test]
    fn labels_are_written_as_the_driver_writes_them() {
        let labels = Labels::Object(
            [
                ("title".to_owned(), Labels::String("Hello".to_owned())),
                ("count".to_owned(), Labels::Number("3".to_owned())),
                ("ratio".to_owned(), Labels::Number("1.5".to_owned())),
                ("nothing".to_owned(), Labels::Nil),
                ("flag".to_owned(), Labels::Bool(true)),
                (
                    "embedding".to_owned(),
                    Labels::Array(vec![
                        Labels::Float64(0.25),
                        Labels::Number("-7".to_owned()),
                        Labels::String("x".to_owned()),
                    ]),
                ),
                (
                    "nested".to_owned(),
                    Labels::Object(
                        [("deep".to_owned(), Labels::Array(Vec::new()))]
                            .into_iter()
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        );
        let mut written = Writer::new();
        written.labels("labels", &labels).expect("write");
        let written = written.finish().expect("document");
        let expected = doc! {"labels": {
            "title": "Hello",
            "count": 3_i64,
            "ratio": 1.5_f64,
            "nothing": null,
            "flag": true,
            "embedding": [0.25_f64, -7_i64, "x"],
            "nested": {"deep": []},
        }};
        let expected = bson::to_raw_document_buf(&expected).expect("encode");
        assert_eq!(written.as_bytes(), expected.as_bytes());
    }

    /// The fields of an item and of a feedback record are written in Go's order,
    /// with the types the port has always stored, and a nil categories is null.
    #[test]
    fn records_are_written_as_go_writes_them() {
        let now = Time::default();
        let item = Item {
            item_id: "i".to_owned(),
            is_hidden: true,
            categories: Some(vec!["a".to_owned()]),
            labels: Labels::Number("3".to_owned()),
            comment: "c".to_owned(),
            ..Item::default()
        };
        let mut written = Writer::new();
        write_item(&mut written, &item, &now).expect("write");
        let written = written.finish().expect("document");
        let expected = doc! {
            "itemid": "i",
            "ishidden": true,
            "categories": ["a"],
            "timestamp": to_datetime(&item.timestamp),
            "updateat": to_datetime(&now),
            "labels": 3_i64,
            "comment": "c",
        };
        let expected = bson::to_raw_document_buf(&expected).expect("encode");
        assert_eq!(written.as_bytes(), expected.as_bytes());

        let feedback = Feedback {
            feedback_key: FeedbackKey {
                feedback_type: "like".to_owned(),
                user_id: "u".to_owned(),
                item_id: "i".to_owned(),
            },
            value: 1.5,
            ..Feedback::default()
        };
        let mut written = Writer::new();
        write_feedback(&mut written, &feedback).expect("write");
        let written = written.finish().expect("document");
        let expected = doc! {
            "feedbackkey": {"feedbacktype": "like", "userid": "u", "itemid": "i"},
            "value": 1.5_f64,
            "timestamp": to_datetime(&feedback.timestamp),
            "updated": to_datetime(&feedback.updated),
            "labels": null,
            "comment": "",
        };
        let expected = bson::to_raw_document_buf(&expected).expect("encode");
        assert_eq!(written.as_bytes(), expected.as_bytes());

        let mut items = Writer::new();
        write_item(
            &mut items,
            &Item {
                categories: None,
                ..Item::default()
            },
            &now,
        )
        .expect("write");
        let items = items.finish().expect("document");
        assert_eq!(
            items.get("categories").expect("a field"),
            Some(RawBsonRef::Null)
        );
    }

    /// The stored types of JSON numbers, as the Go driver stores them (probed
    /// against MongoDB 6 with the Go store: `n: 3` came back as an int64).
    #[test]
    fn json_numbers_are_stored_as_go_stores_them() {
        let mut written = Writer::new();
        for (name, text) in [("n", "3"), ("d", "3.0"), ("f", "1.5")] {
            written
                .labels(name, &Labels::Number(text.to_owned()))
                .expect("write");
        }
        let raw = written.finish().expect("document");
        assert_eq!(raw.get_i64("n").expect("an int64"), 3);
        assert_eq!(raw.get_f64("d").expect("a double"), 3.0);
        assert_eq!(raw.get_f64("f").expect("a double"), 1.5);
        assert_eq!(
            labels_from_raw(raw.get("n").expect("a field").expect("a value")).unwrap(),
            Labels::Number("3".to_owned())
        );
    }

    /// A field name with a zero byte is an error, as the BSON encoder makes it.
    #[test]
    fn field_names_with_a_zero_byte_are_an_error() {
        let mut written = Writer::new();
        assert!(written.labels("a\0b", &Labels::Nil).is_err());
    }

    /// The `update` command is the one the driver would send for the update, with
    /// the update's document as its `u`.
    #[test]
    fn update_command_is_the_update_the_driver_sends() {
        let labels = Labels::Object(
            [("k".to_owned(), Labels::Number("3".to_owned()))]
                .into_iter()
                .collect(),
        );
        let update = set_update(|set| set.labels("labels", &labels)).expect("update");
        let command =
            update_command("items", &doc! {"itemid": "a"}, &update, true).expect("command");
        let expected = doc! {
            "update": "items",
            "updates": [{
                "q": {"itemid": "a"},
                "u": {"$set": {"labels": {"k": 3_i64}}},
                "upsert": true,
                "multi": false,
            }],
        };
        let expected = bson::to_raw_document_buf(&expected).expect("encode");
        assert_eq!(command.as_bytes(), expected.as_bytes());
    }

    /// Labels nested 100,000 levels travel in an update command, and come out of
    /// its `$set` as they went in.
    #[test]
    fn update_command_carries_labels_at_any_depth() {
        let (labels, text) = nested_labels(100_000);
        let update = set_update(|set| set.labels("labels", &labels)).expect("update");
        let command =
            update_command("items", &doc! {"itemid": "a"}, &update, true).expect("command");
        let statement = command
            .get_array("updates")
            .expect("updates")
            .get_document(0)
            .expect("a statement");
        let set = statement
            .get_document("u")
            .expect("u")
            .get_document("$set")
            .expect("$set");
        let read =
            labels_from_raw(set.get("labels").expect("a field").expect("a value")).expect("unpack");
        assert!(labels_json(&read).expect("write") == text);
    }

    /// A write the server refuses is worded as Go's `WriteException` is.
    #[test]
    fn refused_writes_are_worded_as_go_words_them() {
        let reply = doc! {
            "ok": 1,
            "n": 0,
            "writeErrors": [{"index": 0, "code": 2, "errmsg": "too deep"}],
        };
        let err = write_refused(&reply).expect_err("a write error");
        assert_eq!(err.to_string(), "write exception: write errors: [too deep]");
        assert!(write_refused(&doc! {"ok": 1, "n": 1}).is_ok());
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
