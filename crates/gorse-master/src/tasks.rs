// Ports: master/tasks.go @ add21b5

//! The master's tasks: loading the dataset from the data store, updating the
//! item-to-item and user-to-user recommenders, training and optimising the
//! collaborative filtering and click-through rate models, and collecting the
//! garbage the models leave in the cache. Go's `tasks.go`, split by the part
//! of the work each file does.

mod gc;
mod load;
mod recommend;
#[cfg(test)]
mod tests;
mod train;

use std::mem::{size_of, size_of_val};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use gorse_common::event;
use gorse_common::expression::{FeedbackTypeExpression, match_feedback_type_expressions};
use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_dataset::dataset::{Dataset, DatasetItem, DatasetUser};
use gorse_logics::non_personalized::NonPersonalized;
use gorse_model::ctr::data as ctr_data;
use gorse_storage::cache::{self, Score, TimeSeriesPoint};
use gorse_storage::data::types::{Feedback, Item, Labels, User};
use tracing::{error, info};

use crate::Error;
use crate::master::Master;
use crate::metrics;

/// The number of rows a store streams at a time (Go's `batchSize`).
pub(crate) const BATCH_SIZE: i64 = 10000;

/// The datasets one load produces (Go's `Datasets`). The ranking splits feed
/// the collaborative filtering model and the click-through rate splits feed
/// the ranking model. Each training and optimisation takes its own copy of the
/// split it needs, so the splits here stay whole.
pub(crate) struct Datasets {
    /// The whole ranking dataset (Go's `rankingDataset`).
    pub(crate) ranking_dataset: Dataset,
    /// The training split of the ranking dataset (Go's `rankingTrainSet`).
    pub(crate) ranking_train_set: Dataset,
    /// The test split of the ranking dataset (Go's `rankingTestSet`).
    pub(crate) ranking_test_set: Dataset,
    /// The training split of the click-through rate dataset (Go's `clickTrainSet`).
    pub(crate) click_train_set: ctr_data::Dataset,
    /// The test split of the click-through rate dataset (Go's `clickTestSet`).
    pub(crate) click_test_set: ctr_data::Dataset,
}

/// Go's `reflect.Type.Size` of a `string`: its header.
const GO_STRING_SIZE: usize = 16;
/// Go's `reflect.Type.Size` of a slice: its header.
const GO_SLICE_SIZE: usize = 24;
/// Go's `reflect.Type.Size` of a `time.Time`.
const GO_TIME_SIZE: usize = 24;
/// Go's `reflect.Type.Size` of an `any`.
const GO_ANY_SIZE: usize = 16;

/// Go's `sizeof.DeepSize` of a `data.User` (`common/sizeof`). The walk adds the
/// contents of the struct's pointer and slice fields only, and a user has
/// neither, so the size is the struct's own: a string, an `any`, a string and a
/// `time.Time`.
pub(crate) fn user_size(_user: &User) -> usize {
    2 * GO_STRING_SIZE + GO_ANY_SIZE + GO_TIME_SIZE
}

/// Go's `sizeof.DeepSize` of a `data.Item`. Its categories are a slice, so the
/// walk adds the slice header and then each category, as `common/sizeof` does.
pub(crate) fn item_size(item: &Item) -> usize {
    let own = GO_STRING_SIZE // ItemId
        + 8 // IsHidden, padded to the slice's alignment
        + GO_SLICE_SIZE // Categories
        + 2 * GO_TIME_SIZE // Timestamp and UpdateAt
        + GO_ANY_SIZE // Labels
        + GO_STRING_SIZE; // Comment
    let categories = match &item.categories {
        Some(categories) => {
            let contents: usize = categories
                .iter()
                .map(|category| GO_STRING_SIZE + category.len())
                .sum();
            // The capacity beyond the length is counted as zeroed strings.
            let spare = GO_STRING_SIZE * (categories.capacity() - categories.len());
            GO_SLICE_SIZE + contents + spare
        }
        None => GO_SLICE_SIZE,
    };
    own + categories
}

/// Go's `sizeof.DeepSize` of a `data.Feedback`: its key, the value, two
/// timestamps, the `any` labels and the comment. It has no slice or pointer field.
pub(crate) fn feedback_size(_feedback: &Feedback) -> usize {
    3 * GO_STRING_SIZE // FeedbackKey: type, user and item
        + 8 // Value
        + 2 * GO_TIME_SIZE // Timestamp and Updated
        + GO_ANY_SIZE // Labels
        + GO_STRING_SIZE // Comment
}

/// The sum of the sizes of `values` under `size` (Go's `deepSize`).
pub(crate) fn deep_size_of<T>(values: &[T], size: fn(&T) -> usize) -> i64 {
    values.iter().map(|value| size(value) as i64).sum()
}

/// The heap bytes of a vector: its length times its element size. Capacity is
/// not counted, so the value follows the data and not the growth of the vector.
fn vec_heap<T>(values: &[T]) -> usize {
    size_of_val(values)
}

/// The heap bytes of a vector of vectors: the inner headers and their elements.
fn nested_heap<T>(values: &[Vec<T>]) -> usize {
    vec_heap(values) + values.iter().map(|value| vec_heap(value)).sum::<usize>()
}

/// The port's `DeepSize` of a ranking user: its struct and the bytes of its
/// ID and comment.
fn dataset_user_size(user: &DatasetUser) -> usize {
    size_of::<DatasetUser>() + user.user_id.len() + user.comment.len()
}

/// The port's `DeepSize` of a ranking item: its struct, its strings and its
/// categories.
fn dataset_item_size(item: &DatasetItem) -> usize {
    let categories = item.categories.as_ref().map_or(0, |categories| {
        vec_heap(categories) + categories.iter().map(String::len).sum::<usize>()
    });
    size_of::<DatasetItem>() + item.item_id.len() + item.comment.len() + categories
}

/// The port's `DeepSize` of a ranking dataset or one of its splits: its struct,
/// its users, its items and the feedback lists. Go's `sizeof.DeepSize` walks
/// Go's layout, so these values do not equal Go's. The index, the dictionaries
/// and the timestamp and negative lists are private to the dataset and are not
/// counted.
pub(crate) fn ranking_dataset_size(dataset: &Dataset) -> i64 {
    let users: usize = dataset.get_users().iter().map(dataset_user_size).sum();
    let items: usize = dataset.get_items().iter().map(dataset_item_size).sum();
    let size = size_of::<Dataset>()
        + users
        + items
        + nested_heap(dataset.get_user_feedback())
        + nested_heap(dataset.get_item_feedback());
    size as i64
}

/// The port's `DeepSize` of a click-through rate dataset or one of its splits:
/// its struct and the vectors of its samples, labels and embeddings. Its unified
/// index and embedding index are not counted (see [`ranking_dataset_size`]).
pub(crate) fn click_dataset_size(dataset: &ctr_data::Dataset) -> i64 {
    let embeddings = vec_heap(&dataset.item_embeddings)
        + dataset
            .item_embeddings
            .iter()
            .map(|item| nested_heap(item))
            .sum::<usize>();
    let context = dataset.context_labels.as_deref().map_or(0, nested_heap);
    let size = size_of::<ctr_data::Dataset>()
        + nested_heap(&dataset.user_labels)
        + nested_heap(&dataset.item_labels)
        + context
        + vec_heap(&dataset.users)
        + vec_heap(&dataset.items)
        + vec_heap(&dataset.target)
        + vec_heap(&dataset.timestamps)
        + embeddings
        + vec_heap(&dataset.item_embedding_dimension);
    size as i64
}

/// Counts the feedback that no excluded expression list matches, and their
/// sizes (Go's `countFeedback`).
pub(crate) fn count_feedback(
    feedback: &[Feedback],
    excluded: &[&[FeedbackTypeExpression]],
) -> (i64, i64) {
    let mut count = 0;
    let mut size = 0;
    for f in feedback {
        let duplicated = excluded.iter().any(|expressions| {
            match_feedback_type_expressions(expressions, &f.feedback_key.feedback_type, f.value)
        });
        if !duplicated {
            count += 1;
            size += feedback_size(f) as i64;
        }
    }
    (count, size)
}

/// The stored labels as the values the click-through rate package reads (Go
/// passes the labels as `any`, and `ctr.ConvertLabels` reads them).
pub(crate) fn plain_labels(labels: &Labels) -> ctr_data::Value {
    match labels {
        Labels::Nil => ctr_data::Value::Nil,
        Labels::Bool(value) => ctr_data::Value::Bool(*value),
        Labels::Float64(value) => ctr_data::Value::Float64(*value),
        Labels::Number(text) => ctr_data::Value::Number(text.clone()),
        Labels::String(text) => ctr_data::Value::String(text.clone()),
        Labels::Array(values) => ctr_data::Value::Slice(values.iter().map(plain_labels).collect()),
        Labels::Object(map) => ctr_data::Value::Map(
            map.iter()
                .map(|(key, value)| (key.clone(), plain_labels(value)))
                .collect(),
        ),
    }
}

impl Master {
    /// Go's `loadDataset`: loads the data from the store, saves the
    /// non-personalised recommenders and the statistics to the cache, and splits
    /// the ranking and click-through rate datasets.
    pub(crate) async fn load_dataset(&self, parent: &Context) -> Result<Datasets, Error> {
        let (ctx, span) = self.tracer.start(parent, "Load Dataset", 1);
        let result = self.load_dataset_in(&ctx).await;
        span.end();
        result
    }

    async fn load_dataset_in(&self, ctx: &Context) -> Result<Datasets, Error> {
        let cfg = self.config();
        let cache_client = self.cache();
        let data_client = self.data();

        // Reconcile the data store in the background. A reconciliation that
        // starts while one runs is skipped.
        let reconciling = Arc::clone(&self.reconciling);
        let search_config = cfg.recommend.search.clone();
        let reconcile_client = Arc::clone(&data_client);
        let task = tokio::spawn(async move {
            if reconciling
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                info!("skip reconciling data store since previous reconciliation is still running");
                return;
            }
            if let Err(err) = reconcile_client.reconcile(&search_config).await {
                error!(error = %err, "failed to reconcile data store");
            }
            reconciling.store(false, Ordering::SeqCst);
        });
        self.background_tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(task);

        // Build non-personalized recommenders
        let initial_start_time = gotime::now();
        let non_personalized_configs = cfg.recommend.non_personalized.clone().unwrap_or_default();
        let mut non_personalized_recommenders = Vec::new();
        for config in &non_personalized_configs {
            let recommender = NonPersonalized::new(
                config,
                cfg.recommend.cache_size as usize,
                initial_start_time.clone(),
            )?;
            non_personalized_recommenders.push(Mutex::new(recommender));
        }

        info!(
            positive_feedback_types = ?cfg.recommend.data_source.positive_feedback_types,
            negative_feedback_types = ?cfg.recommend.data_source.negative_feedback_types,
            read_feedback_types = ?cfg.recommend.data_source.read_feedback_types,
            item_ttl = cfg.recommend.data_source.item_ttl,
            feedback_ttl = cfg.recommend.data_source.positive_feedback_ttl,
            "load dataset"
        );
        let evaluator = Mutex::new(metrics::new_online_evaluator(
            cfg.recommend
                .data_source
                .positive_feedback_types
                .clone()
                .unwrap_or_default(),
            cfg.recommend
                .data_source
                .read_feedback_types
                .clone()
                .unwrap_or_default(),
        ));
        let sources = load::Sources {
            positive: cfg
                .recommend
                .data_source
                .positive_feedback_types
                .as_deref()
                .unwrap_or_default(),
            negative: cfg
                .recommend
                .data_source
                .negative_feedback_types
                .as_deref()
                .unwrap_or_default(),
            read: cfg
                .recommend
                .data_source
                .read_feedback_types
                .as_deref()
                .unwrap_or_default(),
            item_ttl: cfg.recommend.data_source.item_ttl,
            positive_feedback_ttl: cfg.recommend.data_source.positive_feedback_ttl,
        };
        let (click_dataset, ranking_dataset, snapshot) = self
            .load_data_from_database(
                ctx,
                &*data_client,
                &cfg,
                &sources,
                &evaluator,
                &non_personalized_recommenders,
            )
            .await?;
        let evaluator = evaluator
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        let emit_ctx = ctx.clone();
        tokio::task::spawn_blocking(move || event::emit(&emit_ctx, snapshot));

        // save non-personalized recommenders to cache
        for (i, recommender) in non_personalized_recommenders.into_iter().enumerate() {
            let mut recommender = recommender
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner);
            let scores = recommender.pop_all();
            if let Err(err) = cache_client
                .add_scores(ctx, cache::NON_PERSONALIZED, recommender.name(), &scores)
                .await
            {
                error!(error = %err, "failed to cache non-personalized recommenders");
            }
            if let Err(err) = cache_client
                .delete_scores(
                    ctx,
                    &[cache::NON_PERSONALIZED.to_owned()],
                    &cache::ScoreCondition {
                        subset: Some(recommender.name().to_owned()),
                        before: Some(recommender.timestamp().clone()),
                        ..Default::default()
                    },
                )
                .await
            {
                error!(error = %err, "failed to reclaim outdated items");
            }
            if let Err(err) = cache_client
                .set(
                    ctx,
                    &[
                        cache::time(
                            cache::key(&[cache::NON_PERSONALIZED_UPDATE_TIME, recommender.name()]),
                            recommender.timestamp().clone(),
                        ),
                        cache::string(
                            cache::key(&[cache::NON_PERSONALIZED_DIGEST, recommender.name()]),
                            non_personalized_configs[i].hash(),
                        ),
                    ],
                )
                .await
            {
                error!(error = %err, "failed to write meta");
            }
        }

        // write statistics to database
        let timestamp = ranking_dataset.get_timestamp();
        let series = [
            (cache::NUM_USERS, ranking_dataset.count_users() as f64),
            (cache::NUM_ITEMS, ranking_dataset.count_items() as f64),
            (cache::NUM_FEEDBACK, click_dataset.target.len() as f64),
            (
                cache::NUM_POS_FEEDBACKS,
                click_dataset.positive_count as f64,
            ),
            (
                cache::NUM_NEG_FEEDBACKS,
                click_dataset.negative_count as f64,
            ),
        ];
        let points: Vec<TimeSeriesPoint> = series
            .into_iter()
            .map(|(name, value)| TimeSeriesPoint {
                name: name.to_owned(),
                value,
                timestamp: timestamp.clone(),
            })
            .collect();
        if let Err(err) = cache_client.add_time_series_points(ctx, &points).await {
            error!(error = %err, "failed to write timeseries points");
        }

        let num_users = ranking_dataset.count_users() as i64;
        let num_items = ranking_dataset.count_items() as i64;
        let num_feedback = ranking_dataset.count_feedback() as i64;
        let user_labels = click_dataset
            .index
            .as_ref()
            .map_or(0, |index| i64::from(index.count_user_labels()));
        let item_labels = click_dataset
            .index
            .as_ref()
            .map_or(0, |index| i64::from(index.count_item_labels()));
        let positive_count = click_dataset.positive_count as i64;
        let negative_count = click_dataset.negative_count as i64;

        metrics::USERS_TOTAL.set(num_users as f64);
        metrics::ITEMS_TOTAL.set(num_items as f64);
        metrics::IMPLICIT_FEEDBACKS_TOTAL.set(num_feedback as f64);
        metrics::USER_LABELS_TOTAL.set(user_labels as f64);
        metrics::ITEM_LABELS_TOTAL.set(item_labels as f64);
        metrics::POSITIVE_FEEDBACKS_TOTAL.set(positive_count as f64);
        metrics::NEGATIVE_FEEDBACK_TOTAL.set(negative_count as f64);

        // The global counters, each with the message Go logs when its write fails.
        let globals = [
            (
                cache::NUM_USERS,
                num_users,
                "failed to write number of users",
            ),
            (
                cache::NUM_ITEMS,
                num_items,
                "failed to write number of items",
            ),
            (
                cache::NUM_TOTAL_POS_FEEDBACKS,
                num_feedback,
                "failed to write number of positive feedbacks",
            ),
            (
                cache::NUM_USER_LABELS,
                user_labels,
                "failed to write number of user labels",
            ),
            (
                cache::NUM_ITEM_LABELS,
                item_labels,
                "failed to write number of item labels",
            ),
            (
                cache::NUM_VALID_POS_FEEDBACKS,
                positive_count,
                "failed to write number of positive feedbacks",
            ),
            (
                cache::NUM_VALID_NEG_FEEDBACKS,
                negative_count,
                "failed to write number of negative feedbacks",
            ),
        ];
        for (name, value, message) in globals {
            let value = cache::integer(cache::key(&[cache::GLOBAL_META, name]), value);
            if let Err(err) = cache_client.set(ctx, &[value]).await {
                error!(error = %err, "{message}");
            }
        }

        // evaluate positive feedback rate
        let points = evaluator.evaluate();
        if let Err(err) = cache_client.add_time_series_points(ctx, &points).await {
            error!(error = %err, "failed to insert measurement");
        }

        // collect active users and items
        let user_feedback = ranking_dataset.get_user_feedback();
        let item_feedback = ranking_dataset.get_item_feedback();
        let active_users = user_feedback.iter().filter(|f| !f.is_empty()).count();
        let active_items = item_feedback.iter().filter(|f| !f.is_empty()).count();
        metrics::ACTIVE_USERS_TOTAL.set(active_users as f64);
        metrics::ACTIVE_ITEMS_TOTAL.set(active_items as f64);
        metrics::INACTIVE_USERS_TOTAL.set((user_feedback.len() - active_users) as f64);
        metrics::INACTIVE_ITEMS_TOTAL.set((item_feedback.len() - active_items) as f64);

        // write categories to cache
        let category_scores: Vec<Score> = ranking_dataset
            .get_categories()
            .iter()
            .map(|(category, count)| Score {
                id: category.clone(),
                score: *count as f64,
                timestamp: initial_start_time.clone(),
                is_hidden: false,
                categories: Vec::new(),
            })
            .collect();
        if let Err(err) = cache_client
            .add_scores(ctx, cache::ITEM_CATEGORIES, "", &category_scores)
            .await
        {
            error!(error = %err, "failed to write categories to cache");
        }

        // split ranking dataset
        let start = gotime::now();
        let (ranking_train_set, ranking_test_set) = ranking_dataset.split_cf(0, 0);
        metrics::LOAD_DATASET_STEP_SECONDS_VEC
            .with_label_values(&["split_ranking_dataset"])
            .set(gotime::since(&start).seconds());
        // The labels are Go's: the ranking splits feed the collaborative
        // filtering labels (master/tasks.go 234 and 235).
        metrics::MEMORY_IN_USE_BYTES_VEC
            .with_label_values(&["collaborative_filtering_train_set"])
            .set(ranking_dataset_size(&ranking_train_set) as f64);
        metrics::MEMORY_IN_USE_BYTES_VEC
            .with_label_values(&["collaborative_filtering_test_set"])
            .set(ranking_dataset_size(&ranking_test_set) as f64);

        // split click dataset
        let start = gotime::now();
        let (click_train_set, click_test_set) = click_dataset.split(0.2, 0);
        metrics::LOAD_DATASET_STEP_SECONDS_VEC
            .with_label_values(&["split_click_dataset"])
            .set(gotime::since(&start).seconds());
        // The click splits feed the ranking labels (master/tasks.go 244 and 245).
        metrics::MEMORY_IN_USE_BYTES_VEC
            .with_label_values(&["ranking_train_set"])
            .set(click_dataset_size(&click_train_set) as f64);
        metrics::MEMORY_IN_USE_BYTES_VEC
            .with_label_values(&["ranking_test_set"])
            .set(click_dataset_size(&click_test_set) as f64);

        metrics::LOAD_DATASET_TOTAL_SECONDS.set(gotime::since(&initial_start_time).seconds());
        Ok(Datasets {
            ranking_dataset,
            ranking_train_set,
            ranking_test_set,
            click_train_set,
            click_test_set,
        })
    }

    /// Go's `runLoadDatasetTask`: loads the datasets, updates the
    /// recommenders, and trains or optimises the models. A step that fails is
    /// logged, and the next one still runs.
    pub(crate) async fn run_load_dataset_task(&self, parent: &Context) -> Result<(), Error> {
        let datasets = self.load_dataset(parent).await?;
        let cfg = self.config();
        let use_collaborative_filtering_tasks = !cfg
            .recommend
            .collaborative
            .kind
            .eq_ignore_ascii_case("none");
        let use_click_through_rate_tasks = cfg.recommend.ranker.kind.eq_ignore_ascii_case("fm");
        if let Err(err) = self
            .update_user_to_user(parent, &datasets.ranking_dataset)
            .await
        {
            error!(error = %err, "failed to update user-to-user recommendation");
        }
        if let Err(err) = self
            .update_item_to_item(parent, &datasets.ranking_dataset)
            .await
        {
            error!(error = %err, "failed to update item-to-item recommendation");
        }
        if use_collaborative_filtering_tasks
            && let Err(err) = self
                .train_collaborative_filtering(
                    parent,
                    &datasets.ranking_train_set,
                    &datasets.ranking_test_set,
                )
                .await
        {
            error!(error = %err, "failed to train collaborative filtering model");
        }
        if use_click_through_rate_tasks
            && let Err(err) = self
                .train_click_through_rate_prediction(
                    parent,
                    &datasets.click_train_set,
                    &datasets.click_test_set,
                )
                .await
        {
            error!(error = %err, "failed to train click-through rate prediction model");
        }
        if self.standalone
            && let Err(err) = self.update_recommend(parent).await
        {
            error!(error = %err, "failed to update recommendation");
        }
        if let Err(err) = self
            .collect_garbage(parent, &datasets.ranking_dataset)
            .await
        {
            error!(error = %err, "failed to collect garbage in cache");
        }
        if use_collaborative_filtering_tasks
            && cfg.recommend.collaborative.optimize_period > gotime::Duration::default()
            && let Err(err) = self
                .optimize_collaborative_filtering(
                    parent,
                    &datasets.ranking_train_set,
                    &datasets.ranking_test_set,
                )
                .await
        {
            error!(error = %err, "failed to optimize collaborative filtering model");
        }
        if use_click_through_rate_tasks
            && cfg.recommend.ranker.optimize_period > gotime::Duration::default()
            && let Err(err) = self
                .optimize_click_through_rate_prediction(
                    parent,
                    &datasets.click_train_set,
                    &datasets.click_test_set,
                )
                .await
        {
            error!(error = %err, "failed to optimize click-through rate prediction model");
        }
        Ok(())
    }
}
