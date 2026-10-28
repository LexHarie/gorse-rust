// Ports: logics/user_to_user.go @ add21b5

//! The user-to-user recommenders: each writes a similarity vector per user into
//! a collection of the vector store, and queries the collection for the users
//! nearest to a given one.
//!
//! As in [`crate::item_to_item`], Go's `UserToUser` interface is the
//! [`UserToUser`] enum, the column expressions run on the `gorse-expr` VM with
//! `user` typed as Go's `data.User`, and the users arrive with their labels
//! processed as the dataset gives them. The recommenders are not `Send` or
//! `Sync` either (Q-254, open).

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_config::config::schema::UserToUserConfig;
use gorse_dataset::dataset::DatasetUser;
use gorse_expr::ty::Ty;
use gorse_expr::value::{self, Value};
use gorse_expr::vm::{self, Program};
use gorse_storage::cache::Score;
use gorse_storage::vectors::{self, Database, Distance, Vector, VectorConfig};

use crate::item_to_item::{
    Error, compile_column, field, flatten, flatten_result, from_any, processed_value,
};
use crate::vector_writer::{
    VectorWriter, append_sparse_vector, new_similarity_vector_writer, new_sparse_vector,
};

/// Go's `UserToUserOptions`.
#[derive(Clone)]
pub struct UserToUserOptions {
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
    /// Go's `ItemsIDF`: the IDF of each item. `None` is Go's nil slice.
    pub items_idf: Option<Vec<f32>>,
}

/// Go's `UserToUser`: one of the four user-to-user recommenders.
pub enum UserToUser {
    /// Go's `embeddingUserToUser`.
    Embedding(EmbeddingUserToUser),
    /// Go's `tagsUserToUser`.
    Tags(TagsUserToUser),
    /// Go's `itemsUserToUser`.
    Items(ItemsUserToUser),
    /// Go's `autoUserToUser`.
    Auto(AutoUserToUser),
}

impl UserToUser {
    /// Go's `Add`: writes the vector of `user`, with `feedback` the indices of
    /// the items the user gave positive feedback. Users that do not suit the
    /// recommender are skipped, and the failures of its expression are logged,
    /// not returned.
    ///
    /// # Errors
    ///
    /// Fails when the vector store does.
    pub async fn add(&self, user: &DatasetUser, feedback: &[i32]) -> Result<(), Error> {
        match self {
            UserToUser::Embedding(recommender) => recommender.add(user, feedback).await,
            UserToUser::Tags(recommender) => recommender.add(user, feedback).await,
            UserToUser::Items(recommender) => recommender.add(user, feedback).await,
            UserToUser::Auto(recommender) => recommender.add(user, feedback).await,
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
            UserToUser::Embedding(recommender) => &recommender.writer,
            UserToUser::Tags(recommender) => &recommender.writer,
            UserToUser::Items(recommender) => &recommender.writer,
            UserToUser::Auto(recommender) => &recommender.writer,
        }
    }
}

/// Go's `embeddingUserToUser`: the vectors are the users' embeddings, which the
/// column expression gives.
pub struct EmbeddingUserToUser {
    writer: VectorWriter,
    timestamp: Time,
    column: Program,
}

impl EmbeddingUserToUser {
    /// Go's `Add`. The feedback is not used.
    async fn add(&self, user: &DatasetUser, _feedback: &[i32]) -> Result<(), Error> {
        let result = match vm::run(&self.column, &user_environment(user)) {
            Ok(result) => result,
            Err(err) => {
                tracing::error!(error = %err, user_id = %user.user_id, "failed to evaluate column expression");
                return Ok(());
            }
        };
        let embedding = match from_any(&result) {
            Some(embedding) if !embedding.is_empty() => embedding,
            _ => {
                tracing::error!(column = ?result, "invalid embedding column type");
                return Ok(());
            }
        };
        let vector = with_user_attributes(
            Vector {
                h_values: embedding,
                ..Vector::default()
            },
            user,
            &self.timestamp,
        );
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `tagsUserToUser`: the vectors are sparse, over the tags the column
/// expression gives, weighted by their IDF.
pub struct TagsUserToUser {
    writer: VectorWriter,
    timestamp: Time,
    column: Program,
    idf: Vec<f32>,
}

impl TagsUserToUser {
    /// Go's `Add`. The feedback is not used.
    async fn add(&self, user: &DatasetUser, _feedback: &[i32]) -> Result<(), Error> {
        let result = match vm::run(&self.column, &user_environment(user)) {
            Ok(result) => result,
            Err(err) => {
                tracing::error!(error = %err, user_id = %user.user_id, "failed to evaluate column expression");
                return Ok(());
            }
        };
        let mut tags = BTreeSet::new();
        flatten_result(&result, &mut tags);
        let ids: Vec<i32> = tags.into_iter().collect();
        let vector =
            with_user_attributes(new_sparse_vector(&ids, &self.idf, 0), user, &self.timestamp);
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `itemsUserToUser`: the vectors are sparse, over the items the user gave
/// positive feedback, weighted by their IDF.
pub struct ItemsUserToUser {
    writer: VectorWriter,
    timestamp: Time,
    idf: Vec<f32>,
}

impl ItemsUserToUser {
    /// Go's `Add`. Go sorts `feedback` in place; the vector is the same for any
    /// order, so this leaves the caller's slice as it is.
    async fn add(&self, user: &DatasetUser, feedback: &[i32]) -> Result<(), Error> {
        let vector = with_user_attributes(
            new_sparse_vector(feedback, &self.idf, 0),
            user,
            &self.timestamp,
        );
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `autoUserToUser`: the vectors are sparse, over the user's tags and the
/// items the user gave positive feedback, with the items' indices after the
/// tags'.
pub struct AutoUserToUser {
    writer: VectorWriter,
    timestamp: Time,
    tags_idf: Vec<f32>,
    items_idf: Vec<f32>,
}

impl AutoUserToUser {
    /// Go's `Add`. As in [`ItemsUserToUser::add`], the feedback is not sorted
    /// in place.
    async fn add(&self, user: &DatasetUser, feedback: &[i32]) -> Result<(), Error> {
        let mut tags = BTreeSet::new();
        flatten(&user.labels, &mut tags);
        let tag_ids: Vec<i32> = tags.into_iter().collect();
        let vector = append_sparse_vector(
            new_sparse_vector(&tag_ids, &self.tags_idf, 0),
            feedback,
            &self.items_idf,
            self.tags_idf.len() as u32,
        );
        let vector = with_user_attributes(vector, user, &self.timestamp);
        self.writer.add(vector).await?;
        Ok(())
    }
}

/// Go's `NewUserToUser`: the recommender of `cfg`, writing with the store and
/// options in `opts`, and with `timestamp` on its vectors.
///
/// # Errors
///
/// Fails when there is no vector store, when the IDF the type needs is missing,
/// when the type is unknown, when the type does not take a column and one is
/// given, or when the column does not compile.
pub fn new_user_to_user(
    cfg: &UserToUserConfig,
    timestamp: Time,
    opts: Option<&UserToUserOptions>,
) -> Result<UserToUser, Error> {
    let opts = match opts {
        Some(opts) if opts.vector_client.is_some() => opts,
        _ => {
            return Err(Error::Message(
                "vector database is required for user-to-user",
            ));
        }
    };
    match cfg.kind.as_str() {
        "embedding" => Ok(UserToUser::Embedding(EmbeddingUserToUser {
            writer: user_writer(cfg, &timestamp, opts, false),
            timestamp,
            column: compile_user_column(&cfg.column)?,
        })),
        "tags" => {
            let Some(idf) = opts.tags_idf.clone() else {
                return Err(Error::Message("tags IDF is required for tags user-to-user"));
            };
            Ok(UserToUser::Tags(TagsUserToUser {
                writer: user_writer(cfg, &timestamp, opts, true),
                timestamp,
                column: compile_user_column(&cfg.column)?,
                idf,
            }))
        }
        "items" => {
            let Some(idf) = opts.items_idf.clone() else {
                return Err(Error::Message(
                    "items IDF is required for items user-to-user",
                ));
            };
            if !cfg.column.is_empty() {
                return Err(Error::Message(
                    "column is not supported in items user-to-user",
                ));
            }
            Ok(UserToUser::Items(ItemsUserToUser {
                writer: user_writer(cfg, &timestamp, opts, true),
                timestamp,
                idf,
            }))
        }
        "auto" => {
            let (Some(tags_idf), Some(items_idf)) = (opts.tags_idf.clone(), opts.items_idf.clone())
            else {
                return Err(Error::Message(
                    "tags IDF and items IDF are required for auto user-to-user",
                ));
            };
            Ok(UserToUser::Auto(AutoUserToUser {
                writer: user_writer(cfg, &timestamp, opts, true),
                timestamp,
                tags_idf,
                items_idf,
            }))
        }
        _ => Err(Error::Message("unknown user-to-user method")),
    }
}

/// Go's `QueryUserToUser`: up to `n` users nearest to `user_id` in the
/// collection of `cfg`, nearest first. A user with no vector has no
/// neighbours. The scores are scaled as the recommender's type says, and a user
/// is never its own neighbour.
///
/// # Errors
///
/// Fails when the vector store does.
pub async fn query_user_to_user(
    ctx: &Context,
    client: &dyn Database,
    cfg: &UserToUserConfig,
    user_id: &str,
    n: usize,
) -> Result<Vec<Score>, Error> {
    let collection = vectors::user_to_user_collection(&cfg.name);
    let queries = client.get_vectors(ctx, &collection, &[user_id]).await?;
    let Some(query) = queries.first() else {
        return Ok(Vec::new());
    };
    let neighbors = client
        .query_vectors(ctx, &collection, query, &[], (n + 1) as i64)
        .await?;
    let distance = if cfg.kind == "embedding" {
        Distance::Euclidean
    } else {
        Distance::Dot
    };
    let score_scale = if cfg.kind == "auto" { 0.5 } else { 1.0 };
    let mut scores = Vec::with_capacity(n.min(neighbors.len()));
    for neighbor in neighbors {
        if neighbor.vector.id == user_id || distance == Distance::Dot && neighbor.score <= 0.0 {
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
            categories: Vec::new(),
            timestamp: Time::default(),
        });
        if scores.len() == n {
            break;
        }
    }
    Ok(scores)
}

/// Go's `newUserToUserVectorWriter`: the writer of the recommender's
/// collection. Sparse vectors rank by dot product, dense ones by Euclidean
/// distance.
fn user_writer(
    cfg: &UserToUserConfig,
    timestamp: &Time,
    opts: &UserToUserOptions,
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
        &vectors::user_to_user_collection(&cfg.name),
        distance,
        opts.vector_config.clone(),
        timestamp.clone(),
        opts.batch_size,
        sparse,
    )
}

/// The environment the user expressions run in: Go's `map[string]any{"user":
/// user}`.
fn user_environment(user: &DatasetUser) -> Value {
    Value::string_map(vec![(Value::str("user"), user_value(user))])
}

/// The user as the expressions see it: a `data.User`. The dataset does not keep
/// a user's `UpdateAt`, so it is the zero time, as Go's dataset users have it.
fn user_value(user: &DatasetUser) -> Value {
    Value::Struct(
        user_type(),
        Rc::new(vec![
            Value::str(&user.user_id),
            processed_value(&user.labels),
            Value::str(&user.comment),
            Value::Time(Time::default()),
        ]),
    )
}

/// Go's `data.User`, the type `user` has in the expressions.
fn user_type() -> Ty {
    Ty::named_struct(
        "data.User",
        vec![
            field("UserId", Ty::string()),
            field("Labels", Ty::any()),
            field("Comment", Ty::string()),
            field("UpdateAt", value::time_type()),
        ],
        Vec::new(),
    )
}

/// Compiles a user column under Go's `expr.Env(map[string]any{"user":
/// data.User{}})`.
fn compile_user_column(column: &str) -> Result<Program, Error> {
    compile_column(column, "user", &user_type())
}

/// Sets the attributes every user vector carries: its ID and the writer's time
/// stamp.
fn with_user_attributes(mut vector: Vector, user: &DatasetUser, timestamp: &Time) -> Vector {
    vector.id = user.user_id.clone();
    vector.timestamp = timestamp.clone();
    vector
}

#[cfg(test)]
mod tests;
