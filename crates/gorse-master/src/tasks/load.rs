// Ports: master/tasks.go @ add21b5 (LoadDataFromDatabase, lines 297 to 786)

//! Loading the dataset from the data store: the users, the items with their
//! labels and embeddings, the explicit negative feedback, the positive
//! feedback, the read feedback, and the click-through rate dataset built from
//! them.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, PoisonError};

use futures_util::{StreamExt, TryStreamExt};
use gorse_common::event::Snapshot;
use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::monitor::{self, Span};
use gorse_common::parallel;
use gorse_config::config::schema::Config;
use gorse_dataset::dataset::Dataset;
use gorse_dataset::index::{Index, NOT_ID};
use gorse_dataset::unified_index::UnifiedMapIndexBuilder;
use gorse_logics::non_personalized::NonPersonalized;
use gorse_model::ctr::data::{self as ctr_data, Label};
use gorse_storage::data::types::{Feedback, Item};
use gorse_storage::data::{self, Database, ScanOption};
use tracing::debug;

use super::{
    BATCH_SIZE, count_feedback, deep_size_of, item_size, plain_labels, user_size,
};
use crate::Error;
use crate::master::Master;
use crate::metrics::{self, OnlineEvaluator};

/// The feedback types and time limits a load reads with (Go's
/// `posFeedbackTypes`, `negFeedbackTypes`, `readTypes`, `itemTTL` and
/// `positiveFeedbackTTL`). `loadDataset` takes them from the config.
pub(crate) struct Sources<'a> {
    /// The types that count as positive feedback.
    pub(crate) positive: &'a [FeedbackTypeExpression],
    /// The types that count as explicit negative feedback.
    pub(crate) negative: &'a [FeedbackTypeExpression],
    /// The types that count as read feedback.
    pub(crate) read: &'a [FeedbackTypeExpression],
    /// The days an item stays in the dataset after it is stamped, 0 for no limit.
    pub(crate) item_ttl: u64,
    /// The days a feedback stays in the dataset, 0 for no limit.
    pub(crate) positive_feedback_ttl: u64,
}

/// The labels of one kind of entity, numbered in the order they were first
/// seen (Go's `labelCount`, `labelFirst` and `labelIndex`). A label seen once
/// is only remembered. Its second occurrence indexes it and gives the first
/// holder the label, and every occurrence after that goes to the entity it is
/// on.
#[derive(Default)]
struct LabelLoader {
    /// How many times each label has occurred.
    count: HashMap<String, usize>,
    /// The first holder of each label.
    first: HashMap<String, i32>,
    /// The number of each label that occurs more than once.
    index: Index,
}

impl LabelLoader {
    /// Adds the labels `features` of the entity `entity` to `labels`, the
    /// per-entity lists indexed by the entity's index.
    fn add(&mut self, labels: &mut [Vec<(i32, f32)>], entity: i32, features: &[Label]) {
        for feature in features {
            let count = self.count.entry(feature.name.clone()).or_insert(0);
            *count += 1;
            let count = *count;
            // Memorize the first occurrence.
            if count == 1 {
                self.first.insert(feature.name.clone(), entity);
            }
            // Add the label to the index in second occurrence.
            if count == 2 {
                self.index.add(&feature.name);
                let first = self.first[&feature.name] as usize;
                labels[first].push((self.index.to_number(&feature.name), feature.value));
            }
            // Add the label to the entity.
            if count > 1 {
                labels[entity as usize].push((self.index.to_number(&feature.name), feature.value));
            }
        }
    }
}

/// The state the feedback steps share. Go guards the same maps with one `mu`
/// and the dataset takes its own lock, so one lock here does the same work.
struct Shared {
    /// The dataset, which the positive feedback step grows.
    dataset: Dataset,
    /// The items each user has given explicit negative feedback on. `None`
    /// when there are no negative feedback types.
    negative_set: Option<Vec<HashSet<i32>>>,
    /// The latest timestamp of each explicit negative feedback, by user.
    negative_timestamps: Vec<HashMap<i32, Time>>,
    /// The latest timestamp of each positive feedback, by user.
    positive_feedback: Vec<HashMap<i32, Time>>,
    /// The items each user has read and given no other feedback on.
    read_set: Vec<HashSet<i32>>,
    /// The latest timestamp of each read feedback, by user.
    read_timestamps: Vec<HashMap<i32, Time>>,
    /// The number of explicit negative feedback records.
    explicit_negative_count: usize,
    /// The number of positive feedback records.
    positive_count: usize,
    /// The number of read feedback records.
    read_count: usize,
}

/// Stores `timestamp` for `item` when none is stored yet, or when it is later
/// than the stored one (Go's `timestamp, ok := m[i]; !ok || f.Timestamp.After(timestamp)`).
fn keep_latest(timestamps: &mut HashMap<i32, Time>, item: i32, timestamp: &Time) {
    match timestamps.get(&item) {
        Some(stored) if !timestamp.after(stored) => {}
        _ => {
            timestamps.insert(item, timestamp.clone());
        }
    }
}

/// The scan options of one feedback stream over the items `begin` to `end`.
/// Go builds the same list in each of its three feedback steps.
fn feedback_options(
    begin: &str,
    end: &str,
    time_limit: Option<&Time>,
    end_time: &Time,
    types: &[FeedbackTypeExpression],
    order_by_item: bool,
) -> Vec<ScanOption> {
    let mut options = vec![
        data::with_begin_item_id(begin),
        data::with_end_item_id(end),
        data::with_end_time(end_time.clone()),
        data::with_feedback_types(types.to_vec()),
    ];
    if let Some(time_limit) = time_limit {
        options.push(data::with_begin_time(time_limit.clone()));
    }
    if order_by_item {
        options.push(data::with_order_by_item_id());
    }
    options
}

/// Splits `items` into `n` owned groups in the order `parallel::split` cuts
/// them (Go's `parallel.Split`, which returns views).
fn split_owned<T>(mut items: Vec<T>, n: usize) -> Vec<Vec<T>> {
    let sizes: Vec<usize> = parallel::split(&items, n)
        .iter()
        .map(|chunk| chunk.len())
        .collect();
    let mut groups = Vec::with_capacity(sizes.len());
    for size in sizes {
        let rest = items.split_off(size);
        groups.push(std::mem::replace(&mut items, rest));
    }
    groups
}

impl Master {
    /// Go's `LoadDataFromDatabase`. It reads the users, the items, the explicit
    /// negative feedback, the positive feedback and the read feedback, and
    /// builds the click-through rate dataset. Positive feedback is pushed to the
    /// non-personalized recommenders as it is read.
    pub(crate) async fn load_data_from_database(
        &self,
        parent: &Context,
        database: &dyn Database,
        cfg: &Config,
        sources: &Sources<'_>,
        evaluator: &Mutex<OnlineEvaluator>,
        non_personalized: &[Mutex<NonPersonalized>],
    ) -> Result<(ctr_data::Dataset, Dataset, Snapshot), Error> {
        // Estimate the number of users, items, and feedbacks
        let estimated_users = database.count_users(parent).await?;
        let estimated_items = database.count_items(parent).await?;
        let estimated_feedbacks = database.count_feedback(parent).await?;
        let (ctx, span) = monitor::start(
            parent,
            "LoadDataFromDatabase",
            estimated_users + estimated_items + estimated_feedbacks,
        );
        let estimates = Estimates {
            users: estimated_users as usize,
            items: estimated_items as usize,
            feedbacks: estimated_feedbacks as usize,
        };
        let result = load(
            &ctx,
            &span,
            database,
            cfg,
            sources,
            evaluator,
            non_personalized,
            &estimates,
        )
        .await;
        span.end();
        result
    }
}

/// The counts the store reported before the load, which size the buffers.
struct Estimates {
    /// The number of users.
    users: usize,
    /// The number of items.
    items: usize,
    /// The number of feedback records.
    feedbacks: usize,
}

/// The body of [`Master::load_data_from_database`], run under its span.
#[allow(clippy::too_many_arguments)]
async fn load(
    ctx: &Context,
    span: &Span,
    database: &dyn Database,
    cfg: &Config,
    sources: &Sources<'_>,
    evaluator: &Mutex<OnlineEvaluator>,
    non_personalized: &[Mutex<NonPersonalized>],
    estimates: &Estimates,
) -> Result<(ctr_data::Dataset, Dataset, Snapshot), Error> {
    let jobs = cfg.master.n_jobs.max(1) as usize;
    let mut dataset = Dataset::new(gotime::now(), estimates.users, estimates.items);
    let mut snapshot = Snapshot {
        user_count: 0,
        user_bytes: 0,
        item_count: 0,
        item_bytes: 0,
        feedback_count: 0,
        feedback_bytes: 0,
        timestamp: gotime::now(),
    };

    // setup time limit. Go takes these from time.Now(), not the config clock.
    let now = gotime::now();
    let item_time_limit = (sources.item_ttl > 0)
        .then(|| now.add_date(0, 0, -(sources.item_ttl as i64)));
    let feedback_time_limit = (sources.positive_feedback_ttl > 0)
        .then(|| now.add_date(0, 0, -(sources.positive_feedback_ttl as i64)));

    // STEP 1: pull users
    let mut user_loader = LabelLoader::default();
    let mut user_labels: Vec<Vec<(i32, f32)>> = Vec::with_capacity(estimates.users);
    let start = gotime::now();
    {
        let mut users = database.get_user_stream(ctx, BATCH_SIZE);
        while let Some(batch) = users.next().await {
            let batch = batch?;
            let batch_len = batch.len();
            snapshot.user_count += batch_len as i64;
            snapshot.user_bytes += deep_size_of(&batch, user_size);
            for user in batch {
                let features = ctr_data::convert_labels(&plain_labels(&user.labels));
                let user_id = user.user_id.clone();
                dataset.add_user(user);
                let user_index = dataset.get_user_dict().id(&user_id);
                if user_labels.len() == user_index as usize {
                    user_labels.push(Vec::new());
                }
                user_labels[user_index as usize] = Vec::with_capacity(features.len());
                user_loader.add(&mut user_labels, user_index, &features);
            }
            span.add(batch_len as i64);
        }
    }
    debug!(
        n_users = dataset.count_users(),
        n_user_labels = user_loader.index.len(),
        used_time = ?gotime::since(&start),
        "pulled users from database"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["load_users"])
        .set(gotime::since(&start).seconds());

    // STEP 2: pull items
    let mut items: Vec<Item> = Vec::with_capacity(estimates.items);
    let mut item_loader = LabelLoader::default();
    let mut item_labels: Vec<Vec<(i32, f32)>> = Vec::with_capacity(estimates.items);
    let mut item_embedding_indexer = Index::new();
    let mut item_embedding_dimension: Vec<HashMap<usize, usize>> = Vec::new();
    let mut item_embeddings: Vec<Vec<Vec<u16>>> = Vec::with_capacity(estimates.items);
    let start = gotime::now();
    {
        let mut stream = database.get_item_stream(ctx, BATCH_SIZE, item_time_limit.clone());
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let batch_len = batch.len();
            snapshot.item_count += batch_len as i64;
            snapshot.item_bytes += deep_size_of(&batch, item_size);
            for item in batch {
                let labels = ctr_data::convert_labels(&plain_labels(&item.labels));
                // Go replaces the item's embedding labels with their FP16 bits
                // before it reads the embeddings. The dataset takes the stored
                // labels, since `Labels` has no FP16 slice to hold them.
                let embeddings = ctr_data::convert_embeddings(
                    &ctr_data::convert_embedding_labels(plain_labels(&item.labels)),
                );
                let item_id = item.item_id.clone();
                dataset.add_item(item.clone());
                let item_index = dataset.get_item_dict().id(&item_id);
                if item_labels.len() == item_index as usize {
                    item_labels.push(Vec::new());
                }
                if item_embeddings.len() == item_index as usize {
                    item_embeddings.push(Vec::new());
                }
                // load labels
                item_labels[item_index as usize] = Vec::with_capacity(labels.len());
                item_loader.add(&mut item_labels, item_index, &labels);
                // load embeddings
                item_embeddings[item_index as usize] = Vec::with_capacity(embeddings.len());
                for embedding in embeddings {
                    item_embedding_indexer.add(&embedding.name);
                    let embedding_index =
                        item_embedding_indexer.to_number(&embedding.name) as usize;
                    let row = &mut item_embeddings[item_index as usize];
                    while row.len() <= embedding_index {
                        row.push(Vec::new());
                    }
                    row[embedding_index] = embedding.value;
                    while item_embedding_dimension.len() <= embedding_index {
                        item_embedding_dimension.push(HashMap::new());
                    }
                    *item_embedding_dimension[embedding_index]
                        .entry(row[embedding_index].len())
                        .or_insert(0) += 1;
                }
                items.push(item);
            }
            span.add(batch_len as i64);
        }
    }
    debug!(
        n_items = dataset.count_items(),
        n_item_labels = item_loader.index.len(),
        used_time = ?gotime::since(&start),
        "pulled items from database"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["load_items"])
        .set(gotime::since(&start).seconds());

    // split item groups. Go sorts by item ID, and the feedback streams read
    // each group between its first and last item ID.
    items.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    let item_groups: Vec<Vec<Item>> = split_owned(items, jobs);
    // SQL collation can differ from Go's item ID ordering. Locate feedback
    // items directly instead of scanning the group or assuming matching order.
    let mut item_positions = vec![0usize; dataset.count_items()];
    for group in &item_groups {
        for (index, item) in group.iter().enumerate() {
            item_positions[dataset.get_item_dict().id(&item.item_id) as usize] = index;
        }
    }
    let bounds: Vec<(String, String)> = item_groups
        .iter()
        .map(|group| {
            (
                group[0].item_id.clone(),
                group[group.len() - 1].item_id.clone(),
            )
        })
        .collect();
    let item_groups = Mutex::new(item_groups);

    let num_users = dataset.count_users();
    let shared = Mutex::new(Shared {
        dataset,
        negative_set: (!sources.negative.is_empty()).then(|| vec![HashSet::new(); num_users]),
        negative_timestamps: vec![HashMap::new(); num_users],
        positive_feedback: vec![HashMap::new(); num_users],
        read_set: vec![HashSet::new(); num_users],
        read_timestamps: vec![HashMap::new(); num_users],
        explicit_negative_count: 0,
        positive_count: 0,
        read_count: 0,
    });
    let feedback_count = AtomicI64::new(0);
    let feedback_bytes = AtomicI64::new(0);

    // STEP 3: pull explicit negative feedback (highest priority)
    let start = gotime::now();
    if !sources.negative.is_empty() {
        futures_util::stream::iter(0..bounds.len())
            .map(Ok::<usize, Error>)
            .try_for_each_concurrent(jobs, |i| {
                let feedback_time_limit = &feedback_time_limit;
                let (begin, end) = &bounds[i];
                let (feedback_count, feedback_bytes) = (&feedback_count, &feedback_bytes);
                let (shared, span) = (&shared, span);
                async move {
                    let options = feedback_options(
                        begin,
                        end,
                        feedback_time_limit.as_ref(),
                        &cfg.now(),
                        sources.negative,
                        true,
                    );
                    let mut stream = database.get_feedback_stream(ctx, BATCH_SIZE, &options);
                    while let Some(feedback) = stream.next().await {
                        let feedback = feedback?;
                        let (count, size) = count_feedback(&feedback, &[]);
                        feedback_count.fetch_add(count, Ordering::SeqCst);
                        feedback_bytes.fetch_add(size, Ordering::SeqCst);
                        let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        let state = &mut *guard;
                        for f in &feedback {
                            let user_index = state.dataset.get_user_dict().id(&f.feedback_key.user_id);
                            if user_index == NOT_ID {
                                continue;
                            }
                            let item_index = state.dataset.get_item_dict().id(&f.feedback_key.item_id);
                            if item_index == NOT_ID {
                                continue;
                            }
                            let user = user_index as usize;
                            let negative_set = state
                                .negative_set
                                .as_mut()
                                .expect("the negative set exists when there are negative types");
                            negative_set[user].insert(item_index);
                            keep_latest(&mut state.negative_timestamps[user], item_index, &f.timestamp);
                            state.explicit_negative_count += 1;
                            evaluator
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .add(
                                    &f.feedback_key.feedback_type,
                                    f.value,
                                    user_index,
                                    item_index,
                                    f.timestamp.clone(),
                                );
                        }
                        drop(guard);
                        span.add(feedback.len() as i64);
                    }
                    Ok(())
                }
            })
            .await?;
    }
    let explicit_negative_count = shared
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .explicit_negative_count;
    debug!(
        n_explicit_negative_feedback = explicit_negative_count,
        used_time = ?gotime::since(&start),
        "pulled explicit negative feedback from database"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["load_explicit_negative_feedback"])
        .set(gotime::since(&start).seconds());

    // STEP 4: pull positive feedback
    let start = gotime::now();
    futures_util::stream::iter(0..bounds.len())
        .map(Ok::<usize, Error>)
        .try_for_each_concurrent(jobs, |i| {
            let feedback_time_limit = &feedback_time_limit;
            let (begin, end) = &bounds[i];
            let (feedback_count, feedback_bytes) = (&feedback_count, &feedback_bytes);
            let (shared, item_groups, item_positions) = (&shared, &item_groups, &item_positions);
            let (evaluator, span) = (evaluator, span);
            async move {
                let mut group = {
                    let mut groups = item_groups.lock().unwrap_or_else(PoisonError::into_inner);
                    std::mem::take(&mut groups[i])
                };
                let mut item_feedback: Vec<Feedback> = Vec::new();
                let mut item_group_index = 0usize;
                let mut item_has_feedback = vec![false; group.len()];
                let options = feedback_options(
                    begin,
                    end,
                    feedback_time_limit.as_ref(),
                    &cfg.now(),
                    sources.positive,
                    true,
                );
                let mut stream = database.get_feedback_stream(ctx, BATCH_SIZE, &options);
                while let Some(feedback) = stream.next().await {
                    let feedback = feedback?;
                    let (count, size) = count_feedback(&feedback, &[sources.negative]);
                    feedback_count.fetch_add(count, Ordering::SeqCst);
                    feedback_bytes.fetch_add(size, Ordering::SeqCst);
                    for f in feedback.iter() {
                        let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        let state = &mut *guard;
                        // convert user and item id to index
                        let user_index = state.dataset.get_user_dict().id(&f.feedback_key.user_id);
                        if user_index == NOT_ID {
                            continue;
                        }
                        let item_index = state.dataset.get_item_dict().id(&f.feedback_key.item_id);
                        if item_index == NOT_ID {
                            continue;
                        }
                        let user = user_index as usize;
                        // skip if already in negative feedback set (negative feedback has highest priority)
                        if state
                            .negative_set
                            .as_ref()
                            .is_some_and(|set| set[user].contains(&item_index))
                        {
                            continue;
                        }
                        // insert feedback to positive map, and to the evaluator
                        keep_latest(&mut state.positive_feedback[user], item_index, &f.timestamp);
                        state.positive_count += 1;
                        evaluator
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .add(
                                &f.feedback_key.feedback_type,
                                f.value,
                                user_index,
                                item_index,
                                f.timestamp.clone(),
                            );

                        // append item feedback
                        let same_item = item_feedback
                            .last()
                            .is_some_and(|last| last.feedback_key.item_id == f.feedback_key.item_id);
                        if item_feedback.is_empty() || same_item {
                            item_feedback.push(f.clone());
                        } else {
                            // add item to non-personalized recommenders
                            item_has_feedback[item_group_index] = true;
                            for recommender in non_personalized {
                                recommender
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .push(&group[item_group_index], &item_feedback);
                            }
                            group[item_group_index] = Item {
                                item_id: group[item_group_index].item_id.clone(),
                                ..Default::default()
                            }; // release memory
                            item_feedback.clear();
                            item_feedback.push(f.clone());
                        }
                        item_group_index = item_positions[item_index as usize];
                        state
                            .dataset
                            .add_feedback(&f.feedback_key.user_id, &f.feedback_key.item_id, f.timestamp.clone());
                    }
                    span.add(feedback.len() as i64);
                }

                // add item to non-personalized recommenders
                if !item_feedback.is_empty() {
                    item_has_feedback[item_group_index] = true;
                    for recommender in non_personalized {
                        recommender
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(&group[item_group_index], &item_feedback);
                    }
                    group[item_group_index] = Item {
                        item_id: group[item_group_index].item_id.clone(),
                        ..Default::default()
                    }; // release memory
                }
                for (index, has_feedback) in item_has_feedback.iter().enumerate() {
                    if !*has_feedback {
                        for recommender in non_personalized {
                            recommender
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(&group[index], &[]);
                        }
                        group[index] = Item {
                            item_id: group[index].item_id.clone(),
                            ..Default::default()
                        }; // release memory
                    }
                }
                Ok(())
            }
        })
        .await?;
    let positive_count = shared
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .positive_count;
    debug!(
        n_positive_feedback = positive_count,
        used_time = ?gotime::since(&start),
        "pulled positive feedback from database"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["load_positive_feedback"])
        .set(gotime::since(&start).seconds());

    // STEP 5: pull read feedback (implicit negative feedback)
    let start = gotime::now();
    futures_util::stream::iter(0..bounds.len())
        .map(Ok::<usize, Error>)
        .try_for_each_concurrent(jobs, |i| {
            let feedback_time_limit = &feedback_time_limit;
            let (begin, end) = &bounds[i];
            let (feedback_count, feedback_bytes) = (&feedback_count, &feedback_bytes);
            let shared = &shared;
            let evaluator = evaluator;
            let span = span;
            async move {
                let options = feedback_options(
                    begin,
                    end,
                    feedback_time_limit.as_ref(),
                    &cfg.now(),
                    sources.read,
                    false,
                );
                let mut stream = database.get_feedback_stream(ctx, BATCH_SIZE, &options);
                while let Some(feedback) = stream.next().await {
                    let feedback = feedback?;
                    let (count, size) =
                        count_feedback(&feedback, &[sources.negative, sources.positive]);
                    feedback_count.fetch_add(count, Ordering::SeqCst);
                    feedback_bytes.fetch_add(size, Ordering::SeqCst);
                    for f in &feedback {
                        let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        let state = &mut *guard;
                        let user_index = state.dataset.get_user_dict().id(&f.feedback_key.user_id);
                        if user_index == NOT_ID {
                            continue;
                        }
                        let item_index = state.dataset.get_item_dict().id(&f.feedback_key.item_id);
                        if item_index == NOT_ID {
                            continue;
                        }
                        let user = user_index as usize;
                        // skip if already in negative feedback set or positive set
                        let negated = state
                            .negative_set
                            .as_ref()
                            .is_some_and(|set| set[user].contains(&item_index));
                        if negated || state.positive_feedback[user].contains_key(&item_index) {
                            continue;
                        }
                        state.read_set[user].insert(item_index);
                        keep_latest(&mut state.read_timestamps[user], item_index, &f.timestamp);
                        state.read_count += 1;
                        evaluator
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .add(
                                &f.feedback_key.feedback_type,
                                f.value,
                                user_index,
                                item_index,
                                f.timestamp.clone(),
                            );
                    }
                    span.add(feedback.len() as i64);
                }
                Ok(())
            }
        })
        .await?;
    let read_count = shared
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .read_count;
    debug!(
        n_read_feedback = read_count,
        used_time = ?gotime::since(&start),
        "pulled read feedback from database"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["load_read_feedback"])
        .set(gotime::since(&start).seconds());

    // STEP 6: create click-through rate dataset
    let start = gotime::now();
    let state = shared.into_inner().unwrap_or_else(PoisonError::into_inner);
    let Shared {
        dataset,
        negative_set,
        negative_timestamps,
        positive_feedback,
        read_set,
        read_timestamps,
        ..
    } = state;
    let mut unified_index = UnifiedMapIndexBuilder::default();
    unified_index.item_index = dataset.get_item_dict().to_index();
    unified_index.user_index = dataset.get_user_dict().to_index();
    unified_index.item_label_index = item_loader.index.clone();
    unified_index.user_label_index = user_loader.index.clone();
    let mut click = ctr_data::Dataset {
        index: Some(unified_index.build()),
        user_labels,
        item_labels,
        context_labels: None,
        users: Vec::with_capacity(estimates.feedbacks),
        items: Vec::with_capacity(estimates.feedbacks),
        target: Vec::with_capacity(estimates.feedbacks),
        timestamps: Vec::new(),
        item_embeddings: Vec::new(),
        item_embedding_dimension: Vec::new(),
        item_embedding_index: Some(item_embedding_indexer),
        positive_count: 0,
        negative_count: 0,
    };

    // The dimension of each embedding is the most common dimension its
    // values have. Go reads the map in its own order; ties go to the smaller
    // dimension here, so the result does not depend on the map's order.
    let mut dimensions = Vec::with_capacity(item_embedding_dimension.len());
    for dimension in &item_embedding_dimension {
        let mut best = 0usize;
        let mut counted: Vec<(&usize, &usize)> = dimension.iter().collect();
        counted.sort();
        for (dim, cnt) in counted {
            if *cnt > dimension.get(&best).copied().unwrap_or(0) {
                best = *dim;
            }
        }
        dimensions.push(best);
    }
    for embeddings in &mut item_embeddings {
        for (j, embedding) in embeddings.iter_mut().enumerate() {
            if embedding.len() != dimensions[j] {
                *embedding = Vec::new();
            }
        }
    }
    click.item_embedding_dimension = dimensions;
    click.item_embeddings = item_embeddings;

    for user in 0..num_users {
        // insert explicit negative feedback (highest priority)
        if let Some(negative_set) = &negative_set {
            let mut items: Vec<i32> = negative_set[user].iter().copied().collect();
            items.sort_unstable();
            for item in items {
                click.users.push(user as i32);
                click.items.push(item);
                click.target.push(-1.0);
                click
                    .timestamps
                    .push(negative_timestamps[user].get(&item).cloned().unwrap_or_default());
                click.negative_count += 1;
            }
        }
        // insert positive feedback
        let mut positives: Vec<(i32, &Time)> = positive_feedback[user]
            .iter()
            .map(|(item, timestamp)| (*item, timestamp))
            .collect();
        positives.sort_by_key(|(item, _)| *item);
        for (item, timestamp) in positives {
            click.users.push(user as i32);
            click.items.push(item);
            click.target.push(1.0);
            click.timestamps.push(timestamp.clone());
            click.positive_count += 1;
        }
        // insert read feedback (implicit negative)
        let mut reads: Vec<i32> = read_set[user].iter().copied().collect();
        reads.sort_unstable();
        for item in reads {
            click.users.push(user as i32);
            click.items.push(item);
            click.target.push(-1.0);
            click
                .timestamps
                .push(read_timestamps[user].get(&item).cloned().unwrap_or_default());
            click.negative_count += 1;
        }
    }
    debug!(
        n_valid_positive = click.positive_count,
        n_valid_negative = click.negative_count,
        used_time = ?gotime::since(&start),
        "created ranking dataset"
    );
    metrics::LOAD_DATASET_STEP_SECONDS_VEC
        .with_label_values(&["create_ranking_dataset"])
        .set(gotime::since(&start).seconds());
    snapshot.feedback_count = feedback_count.load(Ordering::SeqCst);
    snapshot.feedback_bytes = feedback_bytes.load(Ordering::SeqCst);
    snapshot.timestamp = gotime::now();
    Ok((click, dataset, snapshot))
}
