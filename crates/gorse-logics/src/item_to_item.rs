// Ports: logics/item_to_item.go @ add21b5

//! The item-to-item recommenders: each writes a similarity vector per item into
//! a collection of the vector store, and queries the collection for the items
//! nearest to a given one.
//!
//! Go's `ItemToItem` interface is the [`ItemToItem`] enum, whose variants are
//! the four recommenders. The column expressions run on the `gorse-expr` VM,
//! with `item` typed as Go's `data.Item`. The items arrive with their labels
//! processed, as the dataset gives them (`dataset.AddItem`), so an expression
//! sees the same values Go's does: a label string is its ID, and a numeric list
//! is FP16 bits. `item_value` builds that view of an item.
//!
//! The recommenders hold compiled programs, whose payloads are `Arc`s, so they
//! are `Send` and `Sync` (Q-254, closed by D-232), which a test checks.

use std::collections::BTreeSet;
use std::sync::Arc;

use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_config::config::schema::ItemToItemConfig;
use gorse_dataset::dataset::{DatasetItem, Value as ProcessedLabels};
use gorse_expr::conf::Config;
use gorse_expr::file;
use gorse_expr::nature;
use gorse_expr::ty::{Field, Kind, Ty};
use gorse_expr::value::{self, Value};
use gorse_expr::vm::{self, Program};
use gorse_storage::cache::Score;
use gorse_storage::data::types::Labels;
use gorse_storage::vectors::{self, Database, Distance, Vector, VectorConfig};

use crate::vector_writer::{
    VectorWriter, append_sparse_vector, new_similarity_vector_writer, new_sparse_vector,
};

/// An error from building or running an item-to-item recommender.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A fixed message, as Go's `errors.New` gives.
    #[error("{0}")]
    Message(&'static str),
    /// A column expression that does not compile. Go returns the compiler's
    /// error as it is.
    #[error(transparent)]
    Expr(#[from] file::Error),
    /// A call to the vector store failed.
    #[error(transparent)]
    Vectors(#[from] vectors::Error),
}

/// Go's `ItemToItemOptions`.
#[derive(Clone)]
pub struct ItemToItemOptions {
    /// Go's `Context`. `None` is the background context.
    pub ctx: Option<Context>,
    /// Go's `VectorClient`. A recommender needs one.
    pub vector_client: Option<Arc<dyn Database>>,
    /// Go's `VectorConfig`: how the collection stores its vectors.
    pub vector_config: VectorConfig,
    /// Go's `BatchSize`. 0 or less is the default batch size.
    pub batch_size: i64,
    /// Go's `TagsIDF`: the IDF of each tag. `None` is Go's nil slice.
    pub tags_idf: Option<Vec<f32>>,
    /// Go's `UsersIDF`: the IDF of each user. `None` is Go's nil slice.
    pub users_idf: Option<Vec<f32>>,
}

/// Go's `ItemToItem`: one of the four item-to-item recommenders.
pub enum ItemToItem {
    /// Go's `embeddingItemToItem`.
    Embedding(EmbeddingItemToItem),
    /// Go's `tagsItemToItem`.
    Tags(TagsItemToItem),
    /// Go's `usersItemToItem`.
    Users(UsersItemToItem),
    /// Go's `autoItemToItem`.
    Auto(AutoItemToItem),
}

impl ItemToItem {
    /// Go's `Add`: writes the vector of `item`, with `feedback` the indices of
    /// the users who gave the item positive feedback. Items that do not suit
    /// the recommender are skipped, and the failures of its expression are
    /// logged, not returned.
    ///
    /// # Errors
    ///
    /// Fails when the vector store does.
    pub async fn add(&self, item: &DatasetItem, feedback: &[i32]) -> Result<(), Error> {
        match self {
            ItemToItem::Embedding(recommender) => recommender.add(item, feedback).await,
            ItemToItem::Tags(recommender) => recommender.add(item, feedback).await,
            ItemToItem::Users(recommender) => recommender.add(item, feedback).await,
            ItemToItem::Auto(recommender) => recommender.add(item, feedback).await,
        }
    }

    /// Go's `Clean`: writes what is buffered, deletes the vectors written
    /// before the recommender's timestamp, and optimizes the collection.
    ///
    /// # Errors
    ///
    /// Fails when the vector store does.
    pub async fn clean(&self) -> Result<(), Error> {
        Ok(self.writer().clean().await?)
    }

    /// The writer of the recommender's collection, which Go's variants embed.
    fn writer(&self) -> &VectorWriter {
        match self {
            ItemToItem::Embedding(recommender) => &recommender.writer,
            ItemToItem::Tags(recommender) => &recommender.writer,
            ItemToItem::Users(recommender) => &recommender.writer,
            ItemToItem::Auto(recommender) => &recommender.writer,
        }
    }
}

/// Go's `embeddingItemToItem`: the vectors are the items' embeddings, which the
/// column expression gives.
pub struct EmbeddingItemToItem {
    writer: VectorWriter,
    timestamp: Time,
    column: Program,
}

impl EmbeddingItemToItem {
    /// Go's `Add`. The feedback is not used.
    async fn add(&self, item: &DatasetItem, _feedback: &[i32]) -> Result<(), Error> {
        let embedding = match extract_item_embedding(item, &self.column) {
            Some(embedding) if !embedding.is_empty() => embedding,
            _ => return Ok(()),
        };
        let vector = with_item_attributes(
            Vector {
                h_values: embedding,
                ..Vector::default()
            },
            item,
            &self.timestamp,
        );
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `tagsItemToItem`: the vectors are sparse, over the tags the column
/// expression gives, weighted by their IDF.
pub struct TagsItemToItem {
    writer: VectorWriter,
    timestamp: Time,
    column: Program,
    idf: Vec<f32>,
}

impl TagsItemToItem {
    /// Go's `Add`. The feedback is not used.
    async fn add(&self, item: &DatasetItem, _feedback: &[i32]) -> Result<(), Error> {
        let env = item_environment(item);
        let result = match vm::run(&self.column, &env) {
            Ok(result) => result,
            Err(err) => {
                tracing::error!(error = %err, item_id = %item.item_id, "failed to evaluate column expression");
                return Ok(());
            }
        };
        let mut tags = BTreeSet::new();
        flatten_result(&result, &mut tags);
        let ids: Vec<i32> = tags.into_iter().collect();
        let vector =
            with_item_attributes(new_sparse_vector(&ids, &self.idf, 0), item, &self.timestamp);
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `usersItemToItem`: the vectors are sparse, over the users who gave the
/// item positive feedback, weighted by their IDF.
pub struct UsersItemToItem {
    writer: VectorWriter,
    timestamp: Time,
    idf: Vec<f32>,
}

impl UsersItemToItem {
    /// Go's `Add`. Go sorts `feedback` in place; the vector is the same for any
    /// order, so this leaves the caller's slice as it is.
    async fn add(&self, item: &DatasetItem, feedback: &[i32]) -> Result<(), Error> {
        let vector = with_item_attributes(
            new_sparse_vector(feedback, &self.idf, 0),
            item,
            &self.timestamp,
        );
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `autoItemToItem`: the vectors are sparse, over the item's tags and the
/// users who gave it positive feedback, with the users' indices after the tags'.
pub struct AutoItemToItem {
    writer: VectorWriter,
    timestamp: Time,
    tags_idf: Vec<f32>,
    users_idf: Vec<f32>,
}

impl AutoItemToItem {
    /// Go's `Add`. As in [`UsersItemToItem::add`], the feedback is not sorted
    /// in place.
    async fn add(&self, item: &DatasetItem, feedback: &[i32]) -> Result<(), Error> {
        let mut tags = BTreeSet::new();
        flatten(&item.labels, &mut tags);
        let tag_ids: Vec<i32> = tags.into_iter().collect();
        let vector = append_sparse_vector(
            new_sparse_vector(&tag_ids, &self.tags_idf, 0),
            feedback,
            &self.users_idf,
            self.tags_idf.len() as u32,
        );
        let vector = with_item_attributes(vector, item, &self.timestamp);
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `NewItemToItem`: the recommender of `cfg`, writing with the store and
/// options in `opts`, and with `timestamp` on its vectors.
///
/// # Errors
///
/// Fails when there is no vector store, when the IDF the type needs is missing,
/// when the type is unknown, or when the column does not compile.
pub fn new_item_to_item(
    cfg: &ItemToItemConfig,
    timestamp: Time,
    opts: Option<&ItemToItemOptions>,
) -> Result<ItemToItem, Error> {
    let opts = match opts {
        Some(opts) if opts.vector_client.is_some() => opts,
        _ => {
            return Err(Error::Message(
                "vector database is required for item-to-item",
            ));
        }
    };
    match cfg.kind.as_str() {
        "embedding" => Ok(ItemToItem::Embedding(EmbeddingItemToItem {
            writer: item_writer(cfg, &timestamp, opts, false),
            timestamp,
            column: compile_item_column(&cfg.column)?,
        })),
        "tags" => {
            let Some(idf) = opts.tags_idf.clone() else {
                return Err(Error::Message("tags IDF is required for tags item-to-item"));
            };
            Ok(ItemToItem::Tags(TagsItemToItem {
                writer: item_writer(cfg, &timestamp, opts, true),
                timestamp,
                column: compile_item_column(&cfg.column)?,
                idf,
            }))
        }
        "users" => {
            let Some(idf) = opts.users_idf.clone() else {
                return Err(Error::Message(
                    "users IDF is required for users item-to-item",
                ));
            };
            if !cfg.column.is_empty() {
                return Err(Error::Message(
                    "column is not supported in users item-to-item",
                ));
            }
            Ok(ItemToItem::Users(UsersItemToItem {
                writer: item_writer(cfg, &timestamp, opts, true),
                timestamp,
                idf,
            }))
        }
        "auto" => {
            let (Some(tags_idf), Some(users_idf)) = (opts.tags_idf.clone(), opts.users_idf.clone())
            else {
                return Err(Error::Message(
                    "tags and users IDF are required for auto item-to-item",
                ));
            };
            Ok(ItemToItem::Auto(AutoItemToItem {
                writer: item_writer(cfg, &timestamp, opts, true),
                timestamp,
                tags_idf,
                users_idf,
            }))
        }
        _ => Err(Error::Message("invalid item-to-item type")),
    }
}

/// Go's `QueryItemToItem`: up to `n` items nearest to `item_id` in the
/// collection of `cfg`, among those that have all of `categories`, nearest
/// first. An item with no vector has no neighbours. The scores are scaled as
/// the recommender's type says, and an item is never its own neighbour.
///
/// # Errors
///
/// Fails when the vector store does.
pub async fn query_item_to_item(
    ctx: &Context,
    client: &dyn Database,
    cfg: &ItemToItemConfig,
    item_id: &str,
    categories: &[&str],
    n: usize,
) -> Result<Vec<Score>, Error> {
    let collection = vectors::item_to_item_collection(&cfg.name);
    let queries = client.get_vectors(ctx, &collection, &[item_id]).await?;
    let Some(query) = queries.first() else {
        return Ok(Vec::new());
    };
    let neighbors = client
        .query_vectors(ctx, &collection, query, categories, (n + 1) as i64)
        .await?;
    let distance = if cfg.kind == "embedding" {
        Distance::Euclidean
    } else {
        Distance::Dot
    };
    let score_scale = if cfg.kind == "auto" { 0.5 } else { 1.0 };
    let mut scores = Vec::with_capacity(n.min(neighbors.len()));
    for neighbor in neighbors {
        if neighbor.vector.id == item_id || distance == Distance::Dot && neighbor.score <= 0.0 {
            continue;
        }
        let mut score = f64::from(neighbor.score) * score_scale;
        if distance == Distance::Euclidean {
            score = 1.0 / (1.0 - score);
        }
        scores.push(Score {
            id: neighbor.vector.id,
            score,
            is_hidden: false,
            categories: neighbor.vector.categories,
            timestamp: Time::default(),
        });
        if scores.len() == n {
            break;
        }
    }
    Ok(scores)
}

/// Go's `newItemToItemVectorWriter`: the writer of the recommender's
/// collection. Sparse vectors rank by dot product, dense ones by Euclidean
/// distance.
fn item_writer(
    cfg: &ItemToItemConfig,
    timestamp: &Time,
    opts: &ItemToItemOptions,
    sparse: bool,
) -> VectorWriter {
    let distance = if sparse {
        Distance::Dot
    } else {
        Distance::Euclidean
    };
    new_similarity_vector_writer(
        opts.ctx.clone(),
        opts.vector_client.clone(),
        &vectors::item_to_item_collection(&cfg.name),
        distance,
        opts.vector_config.clone(),
        timestamp.clone(),
        opts.batch_size,
        sparse,
    )
}

/// Go's `ExtractItemEmbedding`: the FP16 bits of the item's embedding, which
/// `column` gives. `None` when the expression fails or its value is not a
/// number list, and the failure is logged.
pub fn extract_item_embedding(item: &DatasetItem, column: &Program) -> Option<Vec<u16>> {
    let result = match vm::run(column, &item_environment(item)) {
        Ok(result) => result,
        Err(err) => {
            tracing::error!(error = %err, item_id = %item.item_id, "failed to evaluate column expression");
            return None;
        }
    };
    let embedding = from_any(&result);
    if embedding.is_none() {
        tracing::error!(column = ?result, "failed to convert column to FP16 slice");
    }
    embedding
}

/// The environment the item expressions run in: Go's `map[string]any{"item":
/// item}`.
fn item_environment(item: &DatasetItem) -> Value {
    Value::string_map(vec![(Value::str("item"), item_value(item))])
}

/// Compiles an item column under Go's `expr.Env(map[string]any{"item":
/// data.Item{}})`.
fn compile_item_column(column: &str) -> Result<Program, Error> {
    compile_column(column, "item", &item_type())
}

/// Compiles `column` with `name` as a variable of the Go type `ty`, as
/// `expr.Compile(column, expr.Env(...))` does.
pub(crate) fn compile_column(column: &str, name: &str, ty: &Ty) -> Result<Program, Error> {
    let mut config = Config::create_new();
    config.with_env(nature::map_env_of([(
        name.to_owned(),
        nature::from_type(ty),
    )]));
    Ok(gorse_expr::compile(column, &config)?)
}

/// Go's `data.Item`, the type `item` has in the expressions.
fn item_type() -> Ty {
    Ty::named_struct(
        "data.Item",
        vec![
            field("ItemId", Ty::string()),
            field("IsHidden", Ty::bool()),
            field("Categories", Ty::slice(Ty::string())),
            field("Timestamp", value::time_type()),
            field("UpdateAt", value::time_type()),
            field("Labels", Ty::any()),
            field("Comment", Ty::string()),
        ],
        Vec::new(),
    )
}

/// A field of a struct type that is not embedded.
pub(crate) fn field(name: &str, ty: Ty) -> Field {
    Field {
        name: name.to_owned(),
        ty,
        embedded: false,
    }
}

/// The item as the expressions see it: a `data.Item`. The dataset does not
/// keep an item's `UpdateAt`, so it is the zero time, as Go's dataset items
/// have it.
pub(crate) fn item_value(item: &DatasetItem) -> Value {
    Value::Struct(
        item_type(),
        Arc::new(vec![
            Value::str(&item.item_id),
            Value::Bool(item.is_hidden),
            string_slice(item.categories.as_deref()),
            Value::Time(item.timestamp.clone()),
            Value::Time(Time::default()),
            processed_value(&item.labels),
            Value::str(&item.comment),
        ]),
    )
}

/// A `[]string` as an expression value. Go's nil slice is an empty one here,
/// which has the same length and ranges the same way.
pub(crate) fn string_slice(items: Option<&[String]>) -> Value {
    let items = items.unwrap_or(&[]);
    Value::Array(
        Ty::slice(Ty::string()),
        Arc::new(items.iter().map(|item| Value::str(item)).collect()),
    )
}

/// Processed labels as the `any` values Go's expression sees: a label ID is a
/// `dataset.ID`, a list of IDs a `[]dataset.ID`, and FP16 bits a `[]uint16`.
pub(crate) fn processed_value(labels: &ProcessedLabels) -> Value {
    match labels {
        ProcessedLabels::Plain(labels) => labels_value(labels),
        ProcessedLabels::Object(entries) => Value::Map(
            Ty::map(Ty::string(), Ty::any()),
            Arc::new(
                entries
                    .iter()
                    .map(|(name, value)| (Value::str(name), processed_value(value)))
                    .collect(),
            ),
        ),
        ProcessedLabels::Floats(bits) => Value::Array(
            Ty::slice(Ty::basic(Kind::Uint16)),
            Arc::new(
                bits.iter()
                    .map(|&bits| Value::Integer(Ty::basic(Kind::Uint16), i64::from(bits)))
                    .collect(),
            ),
        ),
        ProcessedLabels::Ids(ids) => Value::Array(
            Ty::slice(id_type()),
            Arc::new(
                ids.iter()
                    .map(|id| Value::Integer(id_type(), i64::from(id.0)))
                    .collect(),
            ),
        ),
        ProcessedLabels::Id(id) => Value::Integer(id_type(), i64::from(id.0)),
    }
}

/// Go's `dataset.ID`, a named `int32`.
pub(crate) fn id_type() -> Ty {
    Ty::named_basic("dataset.ID", Kind::Int32, Vec::new())
}

/// Labels as the `any` values Go's expression sees: a `json.Number` becomes a
/// float, since expr-lang has no type for it (Q-255, D-228). As in
/// `non_personalized.rs`.
fn labels_value(labels: &Labels) -> Value {
    match labels {
        Labels::Nil => Value::Nil,
        Labels::Bool(value) => Value::Bool(*value),
        Labels::Float64(value) => Value::Float(*value),
        Labels::Number(text) => {
            Value::Float(text.parse().expect("a json.Number is a valid number"))
        }
        Labels::String(text) => Value::str(text),
        Labels::Array(items) => Value::any_array(items.iter().map(labels_value).collect()),
        Labels::Object(entries) => Value::Map(
            Ty::map(Ty::string(), Ty::any()),
            Arc::new(
                entries
                    .iter()
                    .map(|(name, value)| (Value::str(name), labels_value(value)))
                    .collect(),
            ),
        ),
    }
}

/// Sets the attributes every item vector carries: its ID, whether it is hidden,
/// its categories and the writer's time stamp.
pub(crate) fn with_item_attributes(
    mut vector: Vector,
    item: &DatasetItem,
    timestamp: &Time,
) -> Vector {
    vector.id = item.item_id.clone();
    vector.is_hidden = item.is_hidden;
    vector.categories = item.categories.clone().unwrap_or_default();
    vector.timestamp = timestamp.clone();
    vector
}

/// Go's `flatten` over processed labels: adds the label IDs in `labels` to
/// `tags`. Go's `case` on `dataset.ID` and `[]dataset.ID` matches the IDs, and
/// a map is searched through its values.
pub(crate) fn flatten(labels: &ProcessedLabels, tags: &mut BTreeSet<i32>) {
    match labels {
        ProcessedLabels::Id(id) => {
            tags.insert(id.0);
        }
        ProcessedLabels::Ids(ids) => tags.extend(ids.iter().map(|id| id.0)),
        ProcessedLabels::Object(entries) => {
            for value in entries.values() {
                flatten(value, tags);
            }
        }
        _ => {}
    }
}

/// Go's `flatten` over the value of an expression. The types are the ones
/// [`processed_value`] gives the labels, so only a `dataset.ID`, a
/// `[]dataset.ID` and a `map[string]any` are searched, as Go's type switch
/// searches exact types.
pub(crate) fn flatten_result(result: &Value, tags: &mut BTreeSet<i32>) {
    match result {
        Value::Integer(ty, id) if *ty == id_type() => {
            tags.insert(*id as i32);
        }
        Value::Array(ty, items) if *ty == Ty::slice(id_type()) => {
            for item in items.iter() {
                if let Value::Integer(_, id) = item {
                    tags.insert(*id as i32);
                }
            }
        }
        Value::Map(ty, entries) if *ty == Ty::map(Ty::string(), Ty::any()) => {
            for (_, child) in entries.iter() {
                flatten_result(child, tags);
            }
        }
        _ => {}
    }
}

/// Go's `floats.FromAny`: the FP16 bits of a numeric slice, or `None` when
/// `value` is not one. A `[]uint16` is taken as it is. Any other slice is
/// converted element by element, and an element that is not a number of Go's
/// basic types fails the whole slice.
pub(crate) fn from_any(value: &Value) -> Option<Vec<u16>> {
    match value {
        Value::Array(ty, items) => {
            if *ty == Ty::slice(Ty::basic(Kind::Uint16)) {
                return items
                    .iter()
                    .map(|item| match item {
                        Value::Integer(_, bits) => u16::try_from(*bits).ok(),
                        _ => None,
                    })
                    .collect();
            }
            let values = items.iter().map(any_to_f32).collect::<Option<Vec<f32>>>()?;
            Some(floats::from_float32(&values))
        }
        Value::Bytes(bytes) => {
            let values: Vec<f32> = bytes.iter().map(|&byte| f32::from(byte)).collect();
            Some(floats::from_float32(&values))
        }
        _ => None,
    }
}

/// Go's `anyToFloat32`: the number a value holds, for the types it converts. A
/// named type such as `dataset.ID` does not convert, as Go's type switch
/// matches the exact types only.
fn any_to_f32(value: &Value) -> Option<f32> {
    match value {
        Value::Float(number) => Some(*number as f32),
        Value::Int(number) => Some(*number as f32),
        Value::Integer(ty, number) if !ty.is_named() => match ty.kind() {
            Kind::Int8
            | Kind::Int16
            | Kind::Int32
            | Kind::Int64
            | Kind::Uint8
            | Kind::Uint16
            | Kind::Uint32 => Some(*number as f32),
            Kind::Uint | Kind::Uint64 => Some(*number as u64 as f32),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests;
