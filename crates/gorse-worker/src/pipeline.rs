// Ports: worker/pipeline.go @ add21b5

//! The recommendation pipeline of a worker: the per-user ranking of Go's
//! `worker/pipeline.go`, with the item cache and the rankers.
//!
//! Go runs each user on a detachable goroutine and its rankers call their
//! HTTP APIs with blocking clients. Here each user runs on a thread from
//! [`parallel::detachable`] inside `spawn_blocking`, and the async store calls
//! are driven with `Handle::block_on`. So a ranker's blocking call never sits
//! on a runtime worker.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use gorse_common::floats::from_float32;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration as GoDuration, Time};
use gorse_common::{expression, monitor, parallel};
use gorse_config::config::schema::Config;
use gorse_logics::cf::MatrixFactorizationUsers;
use gorse_logics::recommend::Recommender;
use gorse_logics::reranker::{Decision, FeedbackItem, Reranker};
use gorse_model::ctr::data::{Embedding, Label, Value, convert_embeddings, convert_labels};
use gorse_model::ctr::model::FactorizationMachines;
use gorse_storage::cache::{self, Score, ScoreCondition};
use gorse_storage::data::types::{Feedback, Item, Labels, User, sort_feedbacks};
use gorse_storage::data::{self, GetOptions};
use gorse_storage::vectors::{self, Vector};
use tracing::{error, info};

use crate::Error;
use crate::metrics::{
    MEMORY_INUSE_BYTES_VEC, OFFLINE_RECOMMEND_STEP_SECONDS_VEC, OFFLINE_RECOMMEND_TOTAL_SECONDS,
    UPDATE_USER_RECOMMEND_TOTAL,
};

/// How often the progress callback runs (Go's `time.NewTicker(10 * time.Second)`).
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// The steps Go reports a duration for in `OfflineRecommendStepSecondsVec`.
const STEPS: [&str; 5] = [
    "collaborative_recommend",
    "item_based_recommend",
    "user_based_recommend",
    "latest_recommend",
    "popular_recommend",
];

/// The callback Go's `Recommend` gives its progress: the users completed so
/// far, and the users completed since the last call.
pub type ProgressFn = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Go's `compressLabelsEmbeddings`: compresses the embedding vectors in labels
/// to FP16 bits, and leaves every other value as it is. Go aligns the strings
/// in a `strutil` pool; that only shares memory, so this keeps the strings as
/// they are.
pub(crate) fn compress_labels_embeddings(labels: &Value) -> Value {
    match labels {
        Value::Map(map) => Value::Map(
            map.iter()
                .map(|(key, value)| (key.clone(), compress_labels_embeddings(value)))
                .collect(),
        ),
        Value::Slice(values) => match embedding_bits(values) {
            Some(bits) => Value::Uint16s(bits),
            None => Value::Slice(values.iter().map(compress_labels_embeddings).collect()),
        },
        Value::Float32s(values) => Value::Uint16s(from_float32(values)),
        Value::Float64s(values) => {
            let floats: Vec<f32> = values.iter().map(|value| *value as f32).collect();
            Value::Uint16s(from_float32(&floats))
        }
        other => other.clone(),
    }
}

/// Go's `floats.FromAny` over a list: the FP16 bits of the list when every
/// element is a number, and `None` otherwise. An empty list gives no bits, as
/// Go's gives an empty `[]uint16`.
fn embedding_bits(values: &[Value]) -> Option<Vec<u16>> {
    let floats = values
        .iter()
        .map(number_as_f32)
        .collect::<Option<Vec<f32>>>()?;
    Some(from_float32(&floats))
}

/// Go's `anyToFloat32` for the value types the labels hold.
fn number_as_f32(value: &Value) -> Option<f32> {
    match value {
        Value::Float32(number) => Some(*number),
        Value::Float64(number) => Some(*number as f32),
        Value::Int(number) => Some(*number as f32),
        _ => None,
    }
}

/// The labels a stored item or user holds, as the pipeline's values. Go's
/// labels are `any` values decoded from JSON; this keeps their shape.
pub(crate) fn plain_labels(labels: &Labels) -> Value {
    match labels {
        Labels::Nil => Value::Nil,
        Labels::Bool(value) => Value::Bool(*value),
        Labels::Float64(value) => Value::Float64(*value),
        Labels::Number(text) => Value::Number(text.clone()),
        Labels::String(text) => Value::String(text.clone()),
        Labels::Array(values) => Value::Slice(values.iter().map(plain_labels).collect()),
        Labels::Object(map) => Value::Map(
            map.iter()
                .map(|(key, value)| (key.clone(), plain_labels(value)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// The labels a reranker template sees for compressed labels. Go's template
/// renders an embedding's `[]uint16` as a list of its FP16 bits, so an
/// embedding becomes a list of numbers that are those bits.
fn labels_view(value: &Value) -> Labels {
    match value {
        Value::Nil => Labels::Nil,
        Value::Bool(value) => Labels::Bool(*value),
        Value::Number(text) => Labels::Number(text.clone()),
        Value::String(text) => Labels::String(text.clone()),
        Value::Int(number) => Labels::Number(number.to_string()),
        Value::Float32(number) => Labels::Float64(f64::from(*number)),
        Value::Float64(number) => Labels::Float64(*number),
        Value::Slice(values) => Labels::Array(values.iter().map(labels_view).collect()),
        Value::Float32s(values) => Labels::Array(
            values
                .iter()
                .map(|value| Labels::Float64(f64::from(*value)))
                .collect(),
        ),
        Value::Float64s(values) => {
            Labels::Array(values.iter().map(|value| Labels::Float64(*value)).collect())
        }
        Value::Uint16s(bits) => Labels::Array(
            bits.iter()
                .map(|bit| Labels::Float64(f64::from(*bit)))
                .collect(),
        ),
        Value::Map(map) => Labels::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), labels_view(value)))
                .collect(),
        ),
    }
}

/// An item as the pipeline holds it: its labels are compressed, as Go's
/// `ItemCache` stores them.
#[derive(Debug, Clone)]
pub struct CachedItem {
    /// The item's id.
    pub item_id: String,
    /// Whether the item is hidden. Hidden items never reach a ranking.
    pub is_hidden: bool,
    /// The item's categories.
    pub categories: Option<Vec<String>>,
    /// The time the item was last changed.
    pub timestamp: Time,
    /// The item's comment.
    pub comment: String,
    /// The item's labels, with each embedding compressed to FP16 bits.
    pub labels: Value,
}

impl CachedItem {
    /// Caches an item, compressing its embeddings.
    pub fn from_item(item: Item) -> Self {
        Self {
            labels: compress_labels_embeddings(&plain_labels(&item.labels)),
            item_id: item.item_id,
            is_hidden: item.is_hidden,
            categories: item.categories,
            timestamp: item.timestamp,
            comment: item.comment,
        }
    }

    /// The item as a ranker sees it: the same item, with its compressed labels
    /// shown as the FP16 bits of each embedding.
    pub fn to_item(&self) -> Item {
        Item {
            item_id: self.item_id.clone(),
            is_hidden: self.is_hidden,
            categories: self.categories.clone(),
            timestamp: self.timestamp.clone(),
            comment: self.comment.clone(),
            labels: labels_view(&self.labels),
            ..Default::default()
        }
    }
}

/// Go's `ItemCache`: the items a recommendation run has downloaded. It is
/// shared by the users of one run, so each item is downloaded once.
pub struct ItemCache {
    client: Arc<dyn data::Database>,
    items: Mutex<HashMap<String, Arc<CachedItem>>>,
}

impl ItemCache {
    /// Go's `NewItemCache`.
    pub fn new(client: Arc<dyn data::Database>) -> Self {
        Self {
            client,
            items: Mutex::new(HashMap::new()),
        }
    }

    /// Go's `GetSlice`: the items with the ids, in the order of the ids. Items
    /// not yet cached are downloaded, and hidden items are left out.
    ///
    /// # Errors
    ///
    /// Fails when the data store does.
    pub async fn get_slice(
        &self,
        ctx: &Context,
        item_ids: &[String],
    ) -> Result<Vec<Arc<CachedItem>>, Error> {
        let requests: Vec<String> = {
            let cached = lock(&self.items);
            item_ids
                .iter()
                .filter(|id| !cached.contains_key(*id))
                .cloned()
                .collect()
        };
        let response = self
            .client
            .batch_get_items(ctx, &requests, &GetOptions::default())
            .await?;
        {
            let mut cached = lock(&self.items);
            for item in response {
                let item = CachedItem::from_item(item);
                cached.insert(item.item_id.clone(), Arc::new(item));
            }
        }
        let cached = lock(&self.items);
        Ok(item_ids
            .iter()
            .filter_map(|id| cached.get(id))
            .filter(|item| !item.is_hidden)
            .cloned()
            .collect())
    }

    /// Go's `GetMap`: [`ItemCache::get_slice`] keyed by item id.
    ///
    /// # Errors
    ///
    /// Fails when the data store does.
    pub async fn get_map(
        &self,
        ctx: &Context,
        item_ids: &[String],
    ) -> Result<HashMap<String, Arc<CachedItem>>, Error> {
        let items = self.get_slice(ctx, item_ids).await?;
        Ok(items
            .into_iter()
            .map(|item| (item.item_id.clone(), item))
            .collect())
    }
}

/// The matrix factorisation users a worker has pulled, with their model's id.
#[derive(Default)]
struct MatrixFactorization {
    users: Option<Arc<MatrixFactorizationUsers>>,
    id: i64,
}

/// Go's `Pipeline`: the stores, the models and the configuration a worker
/// ranks with. It is shared between the sync, pull and recommendation tasks,
/// so each field that changes is behind a lock.
pub struct Pipeline {
    config: RwLock<Arc<Config>>,
    cache: RwLock<Arc<dyn cache::Database>>,
    data: RwLock<Arc<dyn data::Database>>,
    vectors: RwLock<Arc<dyn vectors::Database>>,
    tracer: Arc<monitor::Monitor>,
    jobs: usize,
    matrix_factorization: RwLock<MatrixFactorization>,
    click_through_rate_model: RwLock<Option<Arc<dyn FactorizationMachines>>>,
    /// Go's `dontskipColdStartUsers`: when set, users without feedback and
    /// users without embeddings are still ranked.
    pub dont_skip_cold_start_users: bool,
}

impl Pipeline {
    /// Creates a pipeline on the stores, which run `jobs` users at a time.
    pub fn new(
        config: Config,
        cache: Arc<dyn cache::Database>,
        data: Arc<dyn data::Database>,
        vectors: Arc<dyn vectors::Database>,
        tracer: Arc<monitor::Monitor>,
        jobs: usize,
    ) -> Self {
        Self {
            config: RwLock::new(Arc::new(config)),
            cache: RwLock::new(cache),
            data: RwLock::new(data),
            vectors: RwLock::new(vectors),
            tracer,
            jobs,
            matrix_factorization: RwLock::new(MatrixFactorization::default()),
            click_through_rate_model: RwLock::new(None),
            dont_skip_cold_start_users: false,
        }
    }

    /// The configuration the pipeline runs with.
    pub fn config(&self) -> Arc<Config> {
        read(&self.config).clone()
    }

    /// Replaces the configuration, as Go's sync does when the master's
    /// configuration changes.
    pub fn set_config(&self, config: Config) {
        *write(&self.config) = Arc::new(config);
    }

    /// The data store.
    pub fn data(&self) -> Arc<dyn data::Database> {
        read(&self.data).clone()
    }

    /// Replaces the data store.
    pub fn set_data(&self, data: Arc<dyn data::Database>) {
        *write(&self.data) = data;
    }

    /// The cache store.
    pub fn cache(&self) -> Arc<dyn cache::Database> {
        read(&self.cache).clone()
    }

    /// Replaces the cache store.
    pub fn set_cache(&self, cache: Arc<dyn cache::Database>) {
        *write(&self.cache) = cache;
    }

    /// The vector store.
    pub fn vectors(&self) -> Arc<dyn vectors::Database> {
        read(&self.vectors).clone()
    }

    /// Replaces the vector store.
    pub fn set_vectors(&self, vectors: Arc<dyn vectors::Database>) {
        *write(&self.vectors) = vectors;
    }

    /// The tracer that reports the progress of a recommendation run.
    pub fn tracer(&self) -> &monitor::Monitor {
        &self.tracer
    }

    /// Go's `UpdateMatrixFactorization`: installs the users of a matrix
    /// factorisation model, after checking its vector collection exists.
    ///
    /// # Errors
    ///
    /// Fails when the vector store does not have the collection.
    pub async fn update_matrix_factorization(
        &self,
        ctx: &Context,
        id: i64,
        users: MatrixFactorizationUsers,
    ) -> Result<(), Error> {
        self.vectors()
            .describe_collection(ctx, &vectors::collaborative_filtering_collection(id))
            .await?;
        self.set_matrix_factorization(id, Some(users));
        Ok(())
    }

    /// Go's assignment of the matrix factorisation users and their id.
    pub fn set_matrix_factorization(&self, id: i64, users: Option<MatrixFactorizationUsers>) {
        *write(&self.matrix_factorization) = MatrixFactorization {
            users: users.map(Arc::new),
            id,
        };
    }

    /// Go's `GetMatrixFactorizationId`.
    pub fn get_matrix_factorization_id(&self) -> i64 {
        read(&self.matrix_factorization).id
    }

    /// Go's `GetMatrixFactorization`: the users and their model's id, or no
    /// users and id 0 when none are installed.
    pub fn get_matrix_factorization(&self) -> (Option<Arc<MatrixFactorizationUsers>>, i64) {
        let matrix_factorization = read(&self.matrix_factorization);
        match &matrix_factorization.users {
            None => (None, 0),
            Some(users) => (Some(Arc::clone(users)), matrix_factorization.id),
        }
    }

    /// The click-through rate model, if one is installed.
    pub fn click_through_rate_model(&self) -> Option<Arc<dyn FactorizationMachines>> {
        read(&self.click_through_rate_model).clone()
    }

    /// Installs the click-through rate model, or removes it.
    pub fn set_click_through_rate_model(&self, model: Option<Arc<dyn FactorizationMachines>>) {
        *write(&self.click_through_rate_model) = model;
    }

    /// Go's `Recommend`: ranks the items for each of the users and caches the
    /// results. Users whose cache is fresh, or who are inactive, are skipped.
    /// `progress` is called about every ten seconds while the run goes on.
    pub async fn recommend(
        self: Arc<Self>,
        ctx: Context,
        users: Vec<User>,
        progress: Option<ProgressFn>,
    ) {
        let start_recommend = Instant::now();
        let item_cache = Arc::new(ItemCache::new(self.data()));
        let jobs = self.jobs;
        info!(
            n_working_users = users.len(),
            n_jobs = jobs,
            cache_size = self.config().recommend.cache_size,
            "ranking recommendation"
        );

        let (_, span) = self
            .tracer
            .start(&ctx, "Generate recommendation", users.len() as i64);

        // The progress tracker: the workers report each completed user, and
        // every interval the progress callback and the span see the count.
        let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let reporter = {
            let span = Arc::clone(&span);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval_at(
                    tokio::time::Instant::now() + PROGRESS_INTERVAL,
                    PROGRESS_INTERVAL,
                );
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let (mut completed_count, mut previous_count) = (0usize, 0usize);
                loop {
                    tokio::select! {
                        message = completed_rx.recv() => match message {
                            Some(()) => completed_count += 1,
                            None => break,
                        },
                        _ = ticker.tick() => {
                            let throughput = completed_count - previous_count;
                            span.add(throughput as i64);
                            if let Some(progress) = &progress {
                                progress(completed_count, throughput);
                            }
                            previous_count = completed_count;
                        }
                    }
                }
            })
        };

        let counted = Arc::new(AtomicUsize::new(0));
        let users = Arc::new(users);
        let handle = tokio::runtime::Handle::current();
        let pipeline = Arc::clone(&self);
        let blocking = {
            let (counted, users, item_cache) = (
                Arc::clone(&counted),
                Arc::clone(&users),
                Arc::clone(&item_cache),
            );
            tokio::task::spawn_blocking(move || {
                let run = parallel::detachable(&ctx, users.len(), jobs, jobs, |pctx, job| {
                    let ran = handle.block_on(pipeline.recommend_user(
                        &ctx,
                        pctx,
                        &users[job],
                        &item_cache,
                    ));
                    if ran {
                        counted.fetch_add(1, Ordering::Relaxed);
                    }
                    // The reporter stops once every sender is gone, and the
                    // receiver is only dropped after the last job.
                    let _ = completed_tx.send(());
                });
                if let Err(err) = run {
                    error!(error = %err, "recommendation was cancelled");
                }
            })
        };
        if let Err(err) = blocking.await {
            error!(error = %err, "recommendation task failed");
        }
        let _ = reporter.await;

        info!("complete ranking recommendation");
        UPDATE_USER_RECOMMEND_TOTAL.set(counted.load(Ordering::Relaxed) as f64);
        OFFLINE_RECOMMEND_TOTAL_SECONDS.set(start_recommend.elapsed().as_secs_f64());
        // Go's per-step durations are never added to, so they are always zero.
        for step in STEPS {
            OFFLINE_RECOMMEND_STEP_SECONDS_VEC
                .with_label_values(&[step])
                .set(0.0);
        }
        MEMORY_INUSE_BYTES_VEC
            .with_label_values(&["user_feedback_cache"])
            .set(0.0);
        span.end();
    }

    /// The body of Go's per-user job. It returns whether the user was counted:
    /// Go counts a user once the skip checks pass.
    async fn recommend_user(
        &self,
        ctx: &Context,
        pctx: &mut parallel::Context,
        user: &User,
        item_cache: &ItemCache,
    ) -> bool {
        let user_id = user.user_id.as_str();
        // Skip inactive users, and users whose recommendation is still fresh.
        if !self.check_user_active_time(ctx, user_id).await
            || !self.check_recommend_cache_out_of_date(ctx, user_id).await
        {
            return false;
        }

        let config = self.config();
        let recommend_time = gotime::now();
        let mut recommender = match Recommender::new(
            config.recommend.clone(),
            self.cache(),
            self.data(),
            self.vectors(),
            false,
            user_id.to_owned(),
            Vec::new(),
        )
        .await
        {
            Ok(recommender) => recommender,
            Err(err) => {
                error!(user_id, error = %err, "failed to create recommender");
                return true;
            }
        };
        if !self.dont_skip_cold_start_users && recommender.is_cold_start() {
            // Skip cold-start users without any positive feedback.
            return true;
        }

        // Update the collaborative filtering recommendation.
        let (matrix_factorization_users, matrix_factorization_id) = self.get_matrix_factorization();
        if !config
            .recommend
            .collaborative
            .kind
            .eq_ignore_ascii_case("none")
            && let Some(users) = matrix_factorization_users
        {
            if let Some(embedding) = users.get(user_id) {
                if let Err(err) = self
                    .update_collaborative_recommend(
                        ctx,
                        matrix_factorization_id,
                        user_id,
                        embedding,
                        recommender.exclude_set(),
                    )
                    .await
                {
                    error!(user_id, error = %err, "failed to recommend by collaborative filtering");
                    return true;
                }
            } else if !self.dont_skip_cold_start_users {
                // Skip users without collaborative filtering embeddings.
                return true;
            }
        }

        // Generate recommendation from the recommenders.
        let names = match &config.recommend.ranker.recommenders {
            Some(names) if !names.is_empty() => names.clone(),
            _ => config.recommend.list_recommenders(),
        };
        let (scores, digest) = match recommender
            .recommend_sequential(ctx, Vec::new(), 0, &names)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                error!(user_id, error = %err, "failed to recommend items");
                return true;
            }
        };

        let score_ids: Vec<String> = scores.iter().map(|score| score.id.clone()).collect();
        let items = match item_cache.get_map(ctx, &score_ids).await {
            Ok(items) => items,
            Err(err) => {
                error!(user_id, error = %err, "failed to download items");
                return true;
            }
        };
        let mut candidates = Vec::with_capacity(scores.len());
        let mut candidate_set = HashSet::new();
        for score in scores {
            if items.contains_key(&score.id) {
                candidate_set.insert(score.id.clone());
                candidates.push(Score {
                    timestamp: recommend_time.clone(),
                    ..score
                });
            }
        }

        // Insert replacement items into the candidate set before ranking, so
        // that every ranker can order them.
        let replacement = config.recommend.replacement.enable_replacement
            && config.recommend.ranker.kind != "none";
        let (mut positive_items, mut negative_items) = (HashSet::new(), HashSet::new());
        if replacement {
            match self
                .add_replacement_candidates(
                    ctx,
                    candidates,
                    &mut candidate_set,
                    recommender.user_feedback(),
                    item_cache,
                    recommend_time.clone(),
                )
                .await
            {
                Ok((replaced, positive, negative)) => {
                    candidates = replaced;
                    positive_items = positive;
                    negative_items = negative;
                }
                Err(err) => {
                    error!(error = %err, "failed to prepare replacement candidates");
                    return true;
                }
            }
        }

        // Rank by click-through rate, or by the reranker or the decision API.
        // A model that is missing or invalid leaves the candidates as they are.
        let ctr_model = match config.recommend.ranker.kind.as_str() {
            "fm" => self
                .click_through_rate_model()
                .filter(|model| !model.invalid()),
            _ => None,
        };
        let ranked = match (config.recommend.ranker.kind.as_str(), ctr_model) {
            ("fm", Some(model)) => {
                self.rank_by_click_through_rate(
                    ctx,
                    model.as_ref(),
                    user,
                    &candidates,
                    item_cache,
                    recommend_time.clone(),
                )
                .await
            }
            ("reranker", _) => match Reranker::new(&config.recommend.ranker.reranker_api) {
                Ok(ranker) => {
                    self.rank_by_reranker(
                        ctx,
                        Some(pctx),
                        &ranker,
                        user,
                        recommender.user_feedback(),
                        &candidates,
                        item_cache,
                        recommend_time.clone(),
                    )
                    .await
                }
                Err(err) => {
                    error!(error = %err, "failed to create reranker");
                    return true;
                }
            },
            ("decision", _) => match Decision::new(&config.recommend.ranker.decision_api) {
                Ok(ranker) => {
                    self.rank_by_decision(
                        ctx,
                        Some(pctx),
                        &ranker,
                        user,
                        recommender.user_feedback(),
                        &candidates,
                        item_cache,
                        recommend_time.clone(),
                    )
                    .await
                }
                Err(err) => {
                    error!(error = %err, "failed to create decision ranker");
                    return true;
                }
            },
            _ => Ok(candidates),
        };
        let mut results = match ranked {
            Ok(results) => results,
            Err(err) => {
                error!(user_id, error = %err, "failed to rank items");
                return true;
            }
        };

        // Apply the replacement decay after ranking, so the weights do not
        // bypass the ranker's order.
        if replacement {
            results = apply_replacement_decay(
                config.recommend.replacement.positive_replacement_decay,
                config.recommend.replacement.read_replacement_decay,
                results,
                &positive_items,
                &negative_items,
            );
        }

        // Cache the recommendation.
        let cache = self.cache();
        if let Err(err) = cache
            .add_scores(ctx, cache::RECOMMEND, user_id, &results)
            .await
        {
            error!(user_id, error = %err, "failed to cache recommendation");
            return true;
        }
        if let Err(err) = cache
            .delete_scores(
                ctx,
                &[cache::RECOMMEND.to_owned()],
                &ScoreCondition {
                    before: Some(recommend_time.clone()),
                    subset: Some(user_id.to_owned()),
                    ..Default::default()
                },
            )
            .await
        {
            error!(user_id, error = %err, "failed to delete stale recommendation");
            return true;
        }
        if let Err(err) = cache
            .set(
                ctx,
                &[
                    cache::time(
                        cache::key(&[cache::RECOMMEND_UPDATE_TIME, user_id]),
                        recommend_time.clone(),
                    ),
                    cache::string(cache::key(&[cache::RECOMMEND_DIGEST, user_id]), digest),
                ],
            )
            .await
        {
            error!(user_id, error = %err, "failed to cache recommendation time");
        }
        true
    }

    /// Go's `checkUserActiveTime`: whether the user was active within the
    /// active-user TTL. An inactive user's recommendations are removed.
    pub async fn check_user_active_time(&self, ctx: &Context, user_id: &str) -> bool {
        let config = self.config();
        if config.recommend.active_user_ttl == 0 {
            return true;
        }
        // Read the active time. A missing time keeps the user active.
        let active_time = match self
            .cache()
            .get(ctx, &cache::key(&[cache::LAST_MODIFY_USER_TIME, user_id]))
            .await
            .time()
        {
            Ok(time) => time,
            Err(err) => {
                error!(user_id, error = %err, "failed to read last modify user time");
                return true;
            }
        };
        if active_time.is_zero() {
            return true;
        }
        let ttl = GoDuration(config.recommend.active_user_ttl * 24 * gotime::HOUR.0);
        if gotime::since(&active_time) < ttl {
            return true;
        }
        // Remove the recommendations of the inactive user.
        if let Err(err) = self
            .cache()
            .delete_scores(
                ctx,
                &[cache::RECOMMEND.to_owned()],
                &ScoreCondition {
                    subset: Some(user_id.to_owned()),
                    ..Default::default()
                },
            )
            .await
        {
            error!(user_id, error = %err, "failed to delete recommend cache");
        }
        false
    }

    /// Go's `checkRecommendCacheOutOfDate`: whether the user's cached
    /// recommendation is stale, and must be recomputed.
    pub async fn check_recommend_cache_out_of_date(&self, ctx: &Context, user_id: &str) -> bool {
        let cache = self.cache();
        let config = self.config();

        // 1. An empty cache is stale.
        match cache
            .search_scores(ctx, cache::RECOMMEND, user_id, &[], 0, -1)
            .await
        {
            Ok(items) if items.is_empty() => return true,
            Ok(_) => {}
            Err(err) => {
                error!(user_id, error = %err, "failed to load offline recommendation");
                return true;
            }
        }

        // 2. A missing digest, or one that does not match the config, is stale.
        let digest = match cache
            .get(ctx, &cache::key(&[cache::RECOMMEND_DIGEST, user_id]))
            .await
            .string()
        {
            Ok(digest) => digest,
            Err(err) => {
                error!(user_id, error = %err, "failed to read offline recommendation digest");
                return true;
            }
        };
        if digest.is_empty() || digest != config.recommend.hash() {
            return true;
        }

        // The active time: an error here leaves it zero, as Go's does.
        let active_time = cache
            .get(ctx, &cache::key(&[cache::LAST_MODIFY_USER_TIME, user_id]))
            .await
            .time()
            .unwrap_or_default();

        // 3. A missing update time is stale.
        let recommend_time = match cache
            .get(ctx, &cache::key(&[cache::RECOMMEND_UPDATE_TIME, user_id]))
            .await
            .time()
        {
            Ok(time) => time,
            Err(err) => {
                error!(error = %err, "failed to read last update user recommend time");
                return true;
            }
        };

        // 4. An update time older than the cache expiry is stale.
        let now = gotime::now();
        if recommend_time.before(&now.add(GoDuration(-config.recommend.cache_expire.0))) {
            return true;
        }

        // 5. A user active since the update is stale once the ranker's cache
        // expiry has passed.
        if active_time.before(&recommend_time.clone()) {
            let timeout_time = recommend_time.add(config.recommend.ranker.cache_expire);
            return timeout_time.before(&now);
        }
        true
    }

    /// Go's `updateCollaborativeRecommend`: caches the vector store's nearest
    /// items to the user's embedding, leaving out the items this run excludes.
    async fn update_collaborative_recommend(
        &self,
        ctx: &Context,
        matrix_factorization_id: i64,
        user_id: &str,
        user_embedding: &[f32],
        exclude_set: &HashSet<String>,
    ) -> Result<(), Error> {
        let config = self.config();
        let cache = self.cache();
        let local_start_time = gotime::now();
        let scored_vectors = self
            .vectors()
            .query_vectors(
                ctx,
                &vectors::collaborative_filtering_collection(matrix_factorization_id),
                &Vector {
                    values: user_embedding.to_vec(),
                    ..Default::default()
                },
                &[],
                config.recommend.cache_size + exclude_set.len() as i64,
            )
            .await?;
        let recommend: Vec<Score> = scored_vectors
            .into_iter()
            .filter(|scored| !exclude_set.contains(&scored.vector.id))
            .map(|scored| Score {
                id: scored.vector.id,
                score: f64::from(scored.score),
                is_hidden: false,
                categories: scored.vector.categories,
                timestamp: local_start_time.clone(),
            })
            .collect();
        if let Err(err) = cache
            .add_scores(ctx, cache::COLLABORATIVE_FILTERING, user_id, &recommend)
            .await
        {
            error!(user_id, error = %err, "failed to cache collaborative filtering recommendation result");
            return Err(err.into());
        }
        if let Err(err) = cache
            .set(
                ctx,
                &[
                    cache::time(
                        cache::key(&[cache::COLLABORATIVE_FILTERING_UPDATE_TIME, user_id]),
                        local_start_time.clone(),
                    ),
                    cache::string(
                        cache::key(&[cache::COLLABORATIVE_FILTERING_DIGEST, user_id]),
                        config.recommend.collaborative.hash(&config.recommend),
                    ),
                ],
            )
            .await
        {
            error!(user_id, error = %err, "failed to cache collaborative filtering recommendation time");
            return Err(err.into());
        }
        if let Err(err) = cache
            .delete_scores(
                ctx,
                &[cache::COLLABORATIVE_FILTERING.to_owned()],
                &ScoreCondition {
                    before: Some(local_start_time.clone()),
                    subset: Some(user_id.to_owned()),
                    ..Default::default()
                },
            )
            .await
        {
            error!(user_id, error = %err, "failed to delete stale collaborative filtering recommendation result");
            return Err(err.into());
        }
        Ok(())
    }

    /// Go's `rankByClickTroughRate`: ranks the candidates by the predicted
    /// click-through rate, batching the predictions when the model can.
    pub async fn rank_by_click_through_rate(
        &self,
        ctx: &Context,
        predictor: &dyn FactorizationMachines,
        user: &User,
        candidates: &[Score],
        item_cache: &ItemCache,
        recommend_time: Time,
    ) -> Result<Vec<Score>, Error> {
        let candidate_ids: Vec<String> = candidates.iter().map(|score| score.id.clone()).collect();
        let items = item_cache.get_slice(ctx, &candidate_ids).await?;
        let user_labels: Vec<Label> = convert_labels(&plain_labels(&user.labels));
        let mut top_items = Vec::with_capacity(items.len());
        if let Some(batch) = predictor.as_batch_inference() {
            let inputs: Vec<(String, String, Vec<Label>, Vec<Label>)> = items
                .iter()
                .map(|item| {
                    (
                        user.user_id.clone(),
                        item.item_id.clone(),
                        user_labels.clone(),
                        convert_labels(&item.labels),
                    )
                })
                .collect();
            let embeddings: Vec<Vec<Embedding>> = items
                .iter()
                .map(|item| convert_embeddings(&item.labels))
                .collect();
            let output = batch.batch_predict(&inputs, &embeddings, self.jobs);
            for (item, score) in items.iter().zip(output) {
                top_items.push(Score {
                    id: item.item_id.clone(),
                    score: f64::from(score),
                    is_hidden: false,
                    categories: item.categories.clone().unwrap_or_default(),
                    timestamp: recommend_time.clone(),
                });
            }
        } else {
            for item in &items {
                let score = predictor.predict(
                    &user.user_id,
                    &item.item_id,
                    &user_labels,
                    &convert_labels(&item.labels),
                );
                top_items.push(Score {
                    id: item.item_id.clone(),
                    score: f64::from(score),
                    is_hidden: false,
                    categories: item.categories.clone().unwrap_or_default(),
                    timestamp: recommend_time.clone(),
                });
            }
        }
        cache::sort_documents(&mut top_items);
        Ok(top_items)
    }

    /// Go's `rankByReranker`: ranks the candidates with the reranker API.
    #[allow(clippy::too_many_arguments)] // Go's signature.
    pub async fn rank_by_reranker(
        &self,
        ctx: &Context,
        pctx: Option<&mut parallel::Context>,
        ranker: &Reranker,
        user: &User,
        feedback: &[Feedback],
        candidates: &[Score],
        item_cache: &ItemCache,
        recommend_time: Time,
    ) -> Result<Vec<Score>, Error> {
        self.rank_by_api(
            ctx,
            pctx,
            feedback,
            candidates,
            item_cache,
            recommend_time.clone(),
            |feedback_items, items| ranker.rank(ctx, Some(user), feedback_items, items),
        )
        .await
    }

    /// Go's `rankByDecision`: ranks the candidates with the decision API.
    #[allow(clippy::too_many_arguments)] // Go's signature.
    pub async fn rank_by_decision(
        &self,
        ctx: &Context,
        pctx: Option<&mut parallel::Context>,
        ranker: &Decision,
        user: &User,
        feedback: &[Feedback],
        candidates: &[Score],
        item_cache: &ItemCache,
        recommend_time: Time,
    ) -> Result<Vec<Score>, Error> {
        self.rank_by_api(
            ctx,
            pctx,
            feedback,
            candidates,
            item_cache,
            recommend_time.clone(),
            |feedback_items, items| ranker.rank(ctx, Some(user), feedback_items, items),
        )
        .await
    }

    /// Go's `rankByAPI`, shared by the reranker and the decision API. The
    /// context feedback is the user's latest positive feedback, up to the
    /// context size. The worker's detachable slot is released while the API
    /// call runs, as Go's `Detach` does.
    #[allow(clippy::too_many_arguments)] // Go's signature.
    async fn rank_by_api(
        &self,
        ctx: &Context,
        mut pctx: Option<&mut parallel::Context>,
        feedback: &[Feedback],
        candidates: &[Score],
        item_cache: &ItemCache,
        recommend_time: Time,
        rank: impl FnOnce(
            &[FeedbackItem],
            &[Item],
        ) -> Result<Vec<Score>, gorse_logics::reranker::RankError>,
    ) -> Result<Vec<Score>, Error> {
        let config = self.config();
        let candidate_ids: Vec<String> = candidates.iter().map(|score| score.id.clone()).collect();
        let items = item_cache.get_slice(ctx, &candidate_ids).await?;

        // Convert the feedback: the latest positive feedback up to the context size.
        let mut sorted = feedback.to_vec();
        sort_feedbacks(&mut sorted);
        let positive_types = config
            .recommend
            .data_source
            .positive_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let mut context_feedback = Vec::new();
        for item in &sorted {
            if config.recommend.context_size <= context_feedback.len() as i64 {
                break;
            }
            if expression::match_feedback_type_expressions(
                positive_types,
                &item.feedback_key.feedback_type,
                item.value,
            ) {
                context_feedback.push(item.clone());
            }
        }
        let context_item_ids: Vec<String> = context_feedback
            .iter()
            .map(|feedback| feedback.feedback_key.item_id.clone())
            .collect();
        let item_map = item_cache.get_map(ctx, &context_item_ids).await?;
        let feedback_items: Vec<FeedbackItem> = context_feedback
            .iter()
            .filter_map(|feedback| {
                item_map
                    .get(&feedback.feedback_key.item_id)
                    .map(|item| FeedbackItem {
                        feedback_type: feedback.feedback_key.feedback_type.clone(),
                        item: item.to_item(),
                    })
            })
            .collect();
        let ranker_items: Vec<Item> = items.iter().map(|item| item.to_item()).collect();

        // Rank by the API, with the worker slot released for the call.
        if let Some(pctx) = pctx.as_mut() {
            pctx.detach();
        }
        let parsed = rank(&feedback_items, &ranker_items);
        if let Some(pctx) = pctx.as_mut() {
            pctx.attach();
        }
        let parsed = parsed?;

        // Build the scores, with the categories of the items they are for.
        let categories: HashMap<&str, Vec<String>> = items
            .iter()
            .map(|item| {
                (
                    item.item_id.as_str(),
                    item.categories.clone().unwrap_or_default(),
                )
            })
            .collect();
        Ok(parsed
            .into_iter()
            .map(|score| Score {
                categories: categories
                    .get(score.id.as_str())
                    .cloned()
                    .unwrap_or_default(),
                id: score.id,
                score: score.score,
                is_hidden: false,
                timestamp: recommend_time.clone(),
            })
            .collect())
    }

    /// Go's `addReplacementCandidates`: adds the items the user gave feedback
    /// on back into the candidates. It returns the candidates, the positive
    /// items and the negative items that exist in the item cache.
    async fn add_replacement_candidates(
        &self,
        ctx: &Context,
        mut candidates: Vec<Score>,
        candidate_set: &mut HashSet<String>,
        feedbacks: &[Feedback],
        item_cache: &ItemCache,
        recommend_time: Time,
    ) -> Result<(Vec<Score>, HashSet<String>, HashSet<String>), Error> {
        let config = self.config();
        let data_source = &config.recommend.data_source;
        let positive_types = data_source
            .positive_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let read_types = data_source.read_feedback_types.as_deref().unwrap_or(&[]);
        let mut positive_items = HashSet::new();
        let mut distinct_items = HashSet::new();
        for feedback in feedbacks {
            if expression::match_feedback_type_expressions(
                positive_types,
                &feedback.feedback_key.feedback_type,
                feedback.value,
            ) {
                positive_items.insert(feedback.feedback_key.item_id.clone());
                distinct_items.insert(feedback.feedback_key.item_id.clone());
            } else if expression::match_feedback_type_expressions(
                read_types,
                &feedback.feedback_key.feedback_type,
                feedback.value,
            ) {
                distinct_items.insert(feedback.feedback_key.item_id.clone());
            }
        }
        if distinct_items.is_empty() {
            return Ok((candidates, positive_items, HashSet::new()));
        }

        let distinct_ids: Vec<String> = distinct_items.iter().cloned().collect();
        let items = item_cache.get_slice(ctx, &distinct_ids).await?;
        // Keep only the items that exist and are not hidden in the cache.
        for item in &items {
            if candidate_set.contains(&item.item_id) {
                continue;
            }
            candidates.push(Score {
                id: item.item_id.clone(),
                score: 0.0,
                is_hidden: false,
                categories: item.categories.clone().unwrap_or_default(),
                timestamp: recommend_time.clone(),
            });
            candidate_set.insert(item.item_id.clone());
        }

        // The negative items are the distinct items that are not positive, and exist.
        let existing: HashSet<String> = items.iter().map(|item| item.item_id.clone()).collect();
        let positive_existing = positive_items.intersection(&existing).cloned().collect();
        let negative_items = distinct_items
            .difference(&positive_items)
            .filter(|id| existing.contains(*id))
            .cloned()
            .collect();
        Ok((candidates, positive_existing, negative_items))
    }
}

/// Go's `applyReplacementDecay`: scales the scores of the positive and the
/// negative replacement items, and sorts the results again when any changed.
fn apply_replacement_decay(
    positive_decay: f64,
    read_decay: f64,
    mut results: Vec<Score>,
    positive_items: &HashSet<String>,
    negative_items: &HashSet<String>,
) -> Vec<Score> {
    if positive_items.is_empty() && negative_items.is_empty() {
        return results;
    }
    let mut changed = false;
    for result in &mut results {
        if positive_items.contains(&result.id) {
            result.score *= positive_decay;
            changed = true;
        } else if negative_items.contains(&result.id) {
            result.score *= read_decay;
            changed = true;
        }
    }
    if changed {
        cache::sort_documents(&mut results);
    }
    results
}

/// Locks a mutex, going on past a poisoned lock: the cached items stay usable
/// after a job panics.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads a lock, going on past a poisoned lock.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Writes a lock, going on past a poisoned lock.
fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
