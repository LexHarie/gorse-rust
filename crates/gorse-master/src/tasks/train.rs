// Ports: master/tasks.go @ add21b5 (trainCollaborativeFiltering, newCollaborativeFilteringModel,
// trainClickThroughRatePrediction, optimizeCollaborativeFiltering, optimizeClickThroughRatePrediction)

//! Training and optimising the collaborative filtering model and the
//! click-through rate model, and saving what they produce: the model's users in
//! the blob store, the model's items in the vector store, and its metadata in
//! the meta store.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLockWriteGuard};

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_common::monitor;
use gorse_dataset::dataset::Dataset;
use gorse_logics::cf::MatrixFactorizationUsers;
use gorse_model::cf::model::{ALS, BPR, FitConfig as CfFitConfig, MatrixFactorization};
use gorse_model::cf::optimize::{ModelCreator as CfModelCreator, ModelSearch as CfModelSearch};
use gorse_model::ctr::data::Dataset as CtrDataset;
use gorse_model::ctr::fm::Afm;
use gorse_model::ctr::model::{
    FactorizationMachines as _, FitConfig as CtrFitConfig, marshal_model,
};
use gorse_model::ctr::optimize::{ModelCreator as CtrModelCreator, ModelSearch as CtrModelSearch};
use gorse_model::tpe::{Direction, Study, TpeSampler};
use gorse_storage::cache::{self, TimeSeriesPoint};
use gorse_storage::meta;
use gorse_storage::vectors::{self, Distance, QuantizationType, Vector, VectorConfig};
use tracing::{error, info};

use crate::Error;
use crate::master::{ClickThroughRateState, CollaborativeFilteringState, Master};
use crate::metrics;

use super::BATCH_SIZE;

impl Master {
    /// A copy of the collaborative filtering state, taken under the read lock.
    fn collaborative_filtering_snapshot(&self) -> CollaborativeFilteringState {
        self.collaborative_filtering
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The collaborative filtering state, for writing.
    fn collaborative_filtering_write(&self) -> RwLockWriteGuard<'_, CollaborativeFilteringState> {
        self.collaborative_filtering
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A copy of the click-through rate state, taken under the read lock.
    fn click_through_rate_snapshot(&self) -> ClickThroughRateState {
        self.click_through_rate
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The click-through rate state, for writing.
    fn click_through_rate_write(&self) -> RwLockWriteGuard<'_, ClickThroughRateState> {
        self.click_through_rate
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Go's `trainCollaborativeFiltering`: fits a collaborative filtering model on
    /// `train_set`, indexes its items, saves its users and its metadata, and
    /// removes the models that are out of date.
    pub(crate) async fn train_collaborative_filtering(
        &self,
        parent: &Context,
        train_set: &Dataset,
        test_set: &Dataset,
    ) -> Result<(), Error> {
        let (ctx, span) = self
            .tracer
            .start(parent, "Train Collaborative Filtering Model", 2);
        let result = self
            .train_collaborative_filtering_in(&ctx, &span, train_set, test_set)
            .await;
        span.end();
        result
    }

    async fn train_collaborative_filtering_in(
        &self,
        ctx: &Context,
        span: &Arc<monitor::Span>,
        train_set: &Dataset,
        test_set: &Dataset,
    ) -> Result<(), Error> {
        if train_set.count_users() == 0 {
            span.fail(&Error::Message("No user found.".to_owned()));
            return Ok(());
        } else if train_set.count_items() == 0 {
            span.fail(&Error::Message("No item found.".to_owned()));
            return Ok(());
        } else if train_set.count_feedback() == 0 {
            span.fail(&Error::Message("No feedback found.".to_owned()));
            return Ok(());
        }
        let state = self.collaborative_filtering_snapshot();
        if train_set.count_feedback() == state.train_set_size {
            info!("collaborative filtering dataset not changed");
            return Ok(());
        }

        // choose the model: the best one the optimisation found, when it is
        // a different model that scores better than the current one
        let mut model_type = state.meta.r#type.clone();
        let mut model_params = state.meta.params.clone();
        if state.target.score.ndcg > 0.0
            && !state.target.equal(&state.meta)
            && state.target.score.ndcg > state.meta.score.ndcg
        {
            model_type = state.target.r#type.clone();
            model_params = state.target.params.clone();
            info!(
                score = ?state.target.score,
                r#type = %model_type,
                params = ?model_params,
                "find better collaborative filtering model"
            );
        }

        let start_fit = gotime::now();
        let (fit_ctx, fit_span) = monitor::start(ctx, "Fit", 1);
        let mut model = new_collaborative_filtering_model(&model_type, model_params.clone());
        let config = CfFitConfig::new()
            .set_jobs(self.config().master.n_jobs.max(1) as usize)
            .set_patience(
                self.config()
                    .recommend
                    .collaborative
                    .early_stopping
                    .patience as usize,
            );
        let mut val_set = test_set.clone();
        let score = model.fit(&fit_ctx, train_set, &mut val_set, &config);
        metrics::COLLABORATIVE_FILTERING_FIT_SECONDS.set(gotime::since(&start_fit).seconds());
        span.add(1);
        fit_span.end();

        let model_id = gotime::now().unix_milli();
        let collection = vectors::collaborative_filtering_collection(model_id);
        let vector_client = self.vectors();
        let index_ctx;
        let index_span;
        (index_ctx, index_span) = monitor::start(ctx, "Index", train_set.count_items() as i64);
        let config = VectorConfig {
            quantization: QuantizationType::from(
                self.config().database.vector.quantization_type.as_str(),
            ),
            bits: self.config().database.vector.quantization_bits,
        };
        let dimension = model.get_item_factor(0).len();
        if let Err(err) = vector_client
            .add_collection(
                &index_ctx,
                &collection,
                dimension as i64,
                Distance::Dot,
                config,
            )
            .await
        {
            index_span.fail(&err);
            return Err(err.into());
        }
        let items = train_set.get_items();
        let item_count = train_set.count_items();
        for start in (0..item_count).step_by(BATCH_SIZE as usize) {
            let end = (start + BATCH_SIZE as usize).min(item_count);
            let mut item_vectors = Vec::with_capacity(end - start);
            for (i, item) in items.iter().enumerate().take(end).skip(start) {
                if model.is_item_predictable(i as i32) {
                    item_vectors.push(Vector {
                        id: item.item_id.clone(),
                        values: model.get_item_factor(i as i32).to_vec(),
                        is_hidden: item.is_hidden,
                        h_values: Vec::new(),
                        indices: Vec::new(),
                        categories: item.categories.clone().unwrap_or_default(),
                        timestamp: gotime::unix_milli(model_id),
                    });
                }
            }
            if item_vectors.is_empty() {
                index_span.add((end - start) as i64);
                continue;
            }
            if let Err(err) = vector_client
                .add_vectors(&index_ctx, &collection, &item_vectors)
                .await
            {
                index_span.fail(&err);
                return Err(err.into());
            }
            index_span.add((end - start) as i64);
        }
        if let Err(err) = vector_client.optimize(&index_ctx, &collection).await {
            index_span.fail(&err);
            return Err(err.into());
        }
        index_span.end();
        span.add(1);

        let mut matrix_factorization_users = MatrixFactorizationUsers::new();
        for i in 0..train_set.count_users() {
            if let Some(user_id) = train_set.get_user_dict().string(i as i32)
                && model.is_user_predictable(i as i32)
            {
                matrix_factorization_users.add(user_id, model.get_user_factor(i as i32).to_vec());
            }
        }

        info!(id = model_id, "fit collaborative filtering model completed");
        metrics::COLLABORATIVE_FILTERING_NDCG_10.set(f64::from(score.ndcg));
        metrics::COLLABORATIVE_FILTERING_RECALL_10.set(f64::from(score.recall));
        metrics::COLLABORATIVE_FILTERING_PRECISION_10.set(f64::from(score.precision));
        let cache_client = self.cache();
        if let Err(err) = cache_client
            .set(
                ctx,
                &[cache::time(
                    cache::key(&[cache::GLOBAL_META, cache::LAST_FIT_MATCHING_MODEL_TIME]),
                    gotime::now(),
                )],
            )
            .await
        {
            error!(error = %err, "failed to write meta");
        }

        // upload model
        let mut bytes = Vec::new();
        if let Err(err) = matrix_factorization_users.marshal(&mut bytes) {
            error!(id = model_id, error = %err, "failed to matrix factorization users");
            return Err(err.into());
        }
        self.write_blob(ctx, &model_id.to_string(), &bytes).await?;

        // update meta
        let meta_model = meta::Model {
            id: model_id,
            r#type: model_type.clone(),
            params: model_params.clone(),
            score,
        };
        let meta_json = match meta_model.to_json() {
            Ok(json) => json,
            Err(err) => return Err(err.into()),
        };
        if let Err(err) = self
            .meta_store()
            .put(meta::COLLABORATIVE_FILTERING_MODEL, &meta_json)
            .await
        {
            error!(error = %err, "failed to write collaborative filtering model meta");
            return Err(err.into());
        }
        {
            let mut state = self.collaborative_filtering_write();
            state.meta = meta_model;
            state.train_set_size = train_set.count_feedback();
        }
        info!(
            id = model_id,
            ndcg = score.ndcg,
            recall = score.recall,
            precision = score.precision,
            "write collaborative filtering model meta"
        );

        // update statistics
        let now = gotime::now();
        let points = [
            TimeSeriesPoint {
                name: cache::CF_NDCG.to_owned(),
                value: f64::from(score.ndcg),
                timestamp: now.clone(),
            },
            TimeSeriesPoint {
                name: cache::CF_PRECISION.to_owned(),
                value: f64::from(score.precision),
                timestamp: now.clone(),
            },
            TimeSeriesPoint {
                name: cache::CF_RECALL.to_owned(),
                value: f64::from(score.recall),
                timestamp: now,
            },
        ];
        if let Err(err) = cache_client.add_time_series_points(ctx, &points).await {
            error!(error = %err, "failed to write time series points");
            return Ok(());
        }

        self.remove_out_of_date_models(ctx).await;
        Ok(())
    }

    /// Go's `trainClickThroughRatePrediction`: fits a factorization machine on
    /// `train_set`, saves it to the blob store and its metadata to the meta
    /// store, and removes the models that are out of date.
    pub(crate) async fn train_click_through_rate_prediction(
        &self,
        parent: &Context,
        train_set: &CtrDataset,
        test_set: &CtrDataset,
    ) -> Result<(), Error> {
        let (ctx, span) = self
            .tracer
            .start(parent, "Train Click-Through Rate Prediction Model", 1);
        let result = self
            .train_click_through_rate_prediction_in(&ctx, &span, train_set, test_set)
            .await;
        span.end();
        result
    }

    async fn train_click_through_rate_prediction_in(
        &self,
        ctx: &Context,
        span: &Arc<monitor::Span>,
        train_set: &CtrDataset,
        test_set: &CtrDataset,
    ) -> Result<(), Error> {
        use gorse_dataset::dataset::CtrSplit as _;
        if train_set.count_users() == 0 {
            span.fail(&Error::Message("No user found.".to_owned()));
            return Ok(());
        } else if train_set.count_items() == 0 {
            span.fail(&Error::Message("No item found.".to_owned()));
            return Ok(());
        } else if train_set.count() == 0 {
            span.fail(&Error::Message("No feedback found.".to_owned()));
            return Ok(());
        }
        let state = self.click_through_rate_snapshot();
        if train_set.count() == state.train_set_size {
            info!("click dataset not changed");
            return Ok(());
        }

        let mut model_type = state.meta.r#type.clone();
        let mut model_params = state.meta.params.clone();
        if state.target.score.auc > 0.0
            && !state.target.equal(&state.meta)
            && state.target.score.auc > state.meta.score.auc
        {
            model_type = state.target.r#type.clone();
            model_params = state.target.params.clone();
            info!(
                precision = state.target.score.precision,
                recall = state.target.score.recall,
                params = ?model_params,
                "find better click model"
            );
        }
        let mut click_model = Afm::new(model_params.clone());
        let config = CtrFitConfig::new()
            .set_jobs(self.config().master.n_jobs.max(1) as usize)
            .set_patience(self.config().recommend.ranker.early_stopping.patience)
            .clone();

        let start_fit = gotime::now();
        let score = click_model.fit(ctx, train_set, test_set, &config);
        metrics::RANKING_FIT_SECONDS.set(gotime::since(&start_fit).seconds());

        // update match model
        let model_id = gotime::now().unix_milli();
        self.click_through_rate_write().train_set_size = train_set.count();
        info!(id = model_id, "fit click model complete");
        metrics::RANKING_PRECISION.set(f64::from(score.precision));
        metrics::RANKING_RECALL.set(f64::from(score.recall));
        metrics::RANKING_AUC.set(f64::from(score.auc));
        let cache_client = self.cache();
        if let Err(err) = cache_client
            .set(
                ctx,
                &[cache::time(
                    cache::key(&[cache::GLOBAL_META, cache::LAST_FIT_RANKING_MODEL_TIME]),
                    gotime::now(),
                )],
            )
            .await
        {
            error!(error = %err, "failed to write meta");
        }

        // upload model
        let mut bytes = Vec::new();
        if let Err(err) = marshal_model(&mut bytes, &click_model) {
            error!(id = model_id, error = %err, "failed to marshal click-through rate model");
            return Err(err.into());
        }
        self.write_blob(ctx, &model_id.to_string(), &bytes).await?;

        // update meta
        let meta_model = meta::Model {
            id: model_id,
            r#type: model_type,
            params: model_params.clone(),
            score,
        };
        let meta_json = meta_model.to_json()?;
        {
            let mut state = self.click_through_rate_write();
            state.meta = meta_model;
        }
        if let Err(err) = self
            .meta_store()
            .put(meta::CLICK_THROUGH_RATE_MODEL, &meta_json)
            .await
        {
            error!(error = %err, "failed to write click-through rate model meta");
            return Err(err.into());
        }
        info!(
            id = model_id,
            precision = score.precision,
            recall = score.recall,
            auc = score.auc,
            params = ?model_params,
            "write click-through rate model meta"
        );

        // update statistics
        let now = gotime::now();
        let points = [
            TimeSeriesPoint {
                name: cache::CTR_PRECISION.to_owned(),
                value: f64::from(score.precision),
                timestamp: now.clone(),
            },
            TimeSeriesPoint {
                name: cache::CTR_RECALL.to_owned(),
                value: f64::from(score.recall),
                timestamp: now.clone(),
            },
            TimeSeriesPoint {
                name: cache::CTR_AUC.to_owned(),
                value: f64::from(score.auc),
                timestamp: now,
            },
        ];
        if let Err(err) = cache_client.add_time_series_points(ctx, &points).await {
            error!(error = %err, "failed to write time series points");
            return Err(err.into());
        }

        self.remove_out_of_date_models(ctx).await;
        Ok(())
    }

    /// Go's `optimizeCollaborativeFiltering`: searches the collaborative
    /// filtering models and their parameters, and keeps the best one as the
    /// target for the next training.
    pub(crate) async fn optimize_collaborative_filtering(
        &self,
        parent: &Context,
        train_set: &Dataset,
        test_set: &Dataset,
    ) -> Result<(), Error> {
        let cfg = self.config();
        let trials = cfg.recommend.collaborative.optimize_trials as usize;
        let (ctx, span) = self.tracer.start(
            parent,
            "Optimize Collaborative Filtering Model",
            trials as i64,
        );
        let result = self
            .optimize_collaborative_filtering_in(&ctx, &span, train_set, test_set)
            .await;
        span.end();
        result
    }

    async fn optimize_collaborative_filtering_in(
        &self,
        ctx: &Context,
        span: &Arc<monitor::Span>,
        train_set: &Dataset,
        test_set: &Dataset,
    ) -> Result<(), Error> {
        let cfg = self.config();
        if train_set.count_users() == 0 {
            span.fail(&Error::Message("No user found.".to_owned()));
            return Ok(());
        } else if train_set.count_items() == 0 {
            span.fail(&Error::Message("No item found.".to_owned()));
            return Ok(());
        } else if train_set.count_feedback() == 0 {
            span.fail(&Error::Message("No feedback found.".to_owned()));
            return Ok(());
        }

        let mut creators: BTreeMap<String, CfModelCreator> = BTreeMap::new();
        creators.insert(
            "BPR".to_owned(),
            Box::new(|| Box::new(BPR::new(Default::default())) as Box<dyn MatrixFactorization>),
        );
        creators.insert(
            "ALS".to_owned(),
            Box::new(|| Box::new(ALS::new(Default::default())) as Box<dyn MatrixFactorization>),
        );
        let config = CfFitConfig::new()
            .set_jobs(cfg.master.n_jobs.max(1) as usize)
            .set_patience(cfg.recommend.collaborative.early_stopping.patience as usize);
        let mut search = CfModelSearch::new(
            creators,
            Box::new(train_set.clone()),
            Box::new(test_set.clone()),
            config,
        )
        .with_context(ctx.clone())
        .with_span(span.clone());

        let mut study = Study::new(Direction::Maximize, Box::new(TpeSampler::new()));
        study.with_context(ctx.clone());
        let trials = cfg.recommend.collaborative.optimize_trials as usize;
        if let Err(err) = study.optimize(|trial| search.objective(trial), trials) {
            return Err(err.into());
        }
        let result = search.result();
        {
            let mut state = self.collaborative_filtering_write();
            state.target = result;
        }
        let target = self.collaborative_filtering_snapshot().target;
        info!(
            score = ?target.score,
            r#type = %target.r#type,
            params = ?target.params,
            "optimize collaborative filtering model completed"
        );
        Ok(())
    }

    /// Go's `optimizeClickThroughRatePrediction`: searches the factorization
    /// machine's parameters, and keeps the best model as the target for the next
    /// training.
    pub(crate) async fn optimize_click_through_rate_prediction(
        &self,
        parent: &Context,
        train_set: &CtrDataset,
        test_set: &CtrDataset,
    ) -> Result<(), Error> {
        let cfg = self.config();
        let trials = cfg.recommend.ranker.optimize_trials as usize;
        let (ctx, span) = self.tracer.start(
            parent,
            "Optimize Click-Through Rate Prediction Model",
            trials as i64,
        );
        let result = self
            .optimize_click_through_rate_prediction_in(&ctx, &span, train_set, test_set)
            .await;
        span.end();
        result
    }

    async fn optimize_click_through_rate_prediction_in(
        &self,
        ctx: &Context,
        span: &Arc<monitor::Span>,
        train_set: &CtrDataset,
        test_set: &CtrDataset,
    ) -> Result<(), Error> {
        use gorse_dataset::dataset::CtrSplit as _;
        let cfg = self.config();
        if train_set.count_users() == 0 {
            span.fail(&Error::Message("No user found.".to_owned()));
            return Ok(());
        } else if train_set.count_items() == 0 {
            span.fail(&Error::Message("No item found.".to_owned()));
            return Ok(());
        } else if train_set.count() == 0 {
            span.fail(&Error::Message("No feedback found.".to_owned()));
            return Ok(());
        }

        let mut creators: BTreeMap<String, CtrModelCreator> = BTreeMap::new();
        creators.insert(
            "FM".to_owned(),
            Box::new(|| {
                Box::new(Afm::new(Default::default()))
                    as Box<dyn gorse_model::ctr::model::FactorizationMachines>
            }),
        );
        let config = CtrFitConfig::new()
            .set_jobs(cfg.master.n_jobs.max(1) as usize)
            .set_patience(cfg.recommend.ranker.early_stopping.patience)
            .clone();
        let mut search = CtrModelSearch::new(
            creators,
            Box::new(train_set.clone()),
            Box::new(test_set.clone()),
            config,
        )
        .with_context(ctx.clone())
        .with_span(span.clone());

        let mut study = Study::new(Direction::Maximize, Box::new(TpeSampler::new()));
        study.with_context(ctx.clone());
        let trials = cfg.recommend.ranker.optimize_trials as usize;
        if let Err(err) = study.optimize(|trial| search.objective(trial), trials) {
            return Err(err.into());
        }
        let result = search.result();
        {
            let mut state = self.click_through_rate_write();
            state.target = result;
        }
        let target = self.click_through_rate_snapshot().target;
        info!(
            score = ?target.score,
            r#type = %target.r#type,
            params = ?target.params,
            "optimize click-through rate model completed"
        );
        Ok(())
    }

    /// Saves `bytes` to the blob store under `name`, and waits until the
    /// store has finished the write (Go's `blobStore.Create`, `Write`, `Close`
    /// and `<-done`).
    async fn write_blob(&self, ctx: &Context, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let store = self.blob_store();
        let (mut writer, done) = store.create(ctx, name).await?;
        writer.write(ctx, bytes).await?;
        writer.close(ctx).await?;
        let _ = done.await;
        Ok(())
    }
}

/// Go's `newCollaborativeFilteringModel`: BPR and ALS, and BPR for any other type.
fn new_collaborative_filtering_model(
    model_type: &str,
    params: gorse_model_core::params::Params,
) -> Box<dyn MatrixFactorization> {
    match model_type {
        "ALS" => Box::new(ALS::new(params)),
        _ => Box::new(BPR::new(params)),
    }
}
