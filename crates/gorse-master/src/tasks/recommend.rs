// Ports: master/tasks.go @ add21b5 (updateItemToItem, updateUserToUser, updateRecommend, pullAllUsers)

//! The item-to-item and user-to-user recommenders, which the master keeps
//! up to date from the processed dataset, and in standalone mode the
//! recommendation of every user.

use std::sync::Arc;

use futures_util::StreamExt;
use gorse_common::gocontext::Context;
use gorse_common::monitor;
use gorse_dataset::dataset::Dataset;
use gorse_logics::cf::MatrixFactorizationUsers;
use gorse_logics::item_to_item::{ItemToItemOptions, new_item_to_item};
use gorse_logics::user_to_user::{UserToUserOptions, new_user_to_user};
use gorse_model::ctr::model::unmarshal_model;
use gorse_storage::data::types::User;
use gorse_storage::vectors::{QuantizationType, VectorConfig};
use gorse_worker::Pipeline;
use tokio::io::AsyncReadExt;
use tracing::{error, info};

use super::BATCH_SIZE;
use crate::Error;
use crate::master::Master;

impl Master {
    /// The vector configuration the item-to-item and user-to-user recommenders
    /// index with (Go's `vectorConfig` in each update).
    fn recommender_vector_config(&self) -> VectorConfig {
        let cfg = self.config();
        VectorConfig {
            quantization: QuantizationType::from(cfg.database.vector.quantization_type.as_str()),
            bits: cfg.database.vector.quantization_bits,
        }
    }

    /// Go's `updateItemToItem`: builds the item-to-item recommenders over the
    /// processed items, and adds every item to them.
    pub(crate) async fn update_item_to_item(
        &self,
        parent: &Context,
        dataset: &Dataset,
    ) -> Result<(), Error> {
        let cfg = self.config();
        let Some(configs) = cfg.recommend.item_to_item.clone().filter(|c| !c.is_empty()) else {
            return Ok(());
        };
        let total = dataset.get_items().len() * configs.len();
        let (ctx, span) =
            self.tracer
                .start(parent, "Generate item-to-item recommendation", total as i64);
        let result = self
            .update_item_to_item_in(&ctx, &span, dataset, &configs)
            .await;
        span.end();
        result
    }

    async fn update_item_to_item_in(
        &self,
        ctx: &Context,
        span: &monitor::Span,
        dataset: &Dataset,
        configs: &[gorse_config::config::schema::ItemToItemConfig],
    ) -> Result<(), Error> {
        let cfg = self.config();
        let options = ItemToItemOptions {
            ctx: Some(ctx.clone()),
            vector_client: Some(self.vectors()),
            vector_config: self.recommender_vector_config(),
            batch_size: BATCH_SIZE,
            tags_idf: Some(dataset.get_item_column_values_idf()),
            users_idf: Some(dataset.get_user_idf()),
        };
        let mut recommenders = Vec::with_capacity(configs.len());
        for config in configs {
            recommenders.push(new_item_to_item(
                config,
                dataset.get_timestamp(),
                Some(&options),
            )?);
        }

        // Add items to item-to-item recommenders
        let feedback = dataset.get_item_feedback();
        let jobs = cfg.master.n_jobs.max(1) as usize;
        futures_util::stream::iter(dataset.get_items().iter().enumerate())
            .for_each_concurrent(jobs, |(i, item)| {
                let recommenders = &recommenders;
                async move {
                    for recommender in recommenders {
                        if let Err(err) = recommender.add(item, &feedback[i]).await {
                            error!(
                                item_id = %item.item_id,
                                error = %err,
                                "failed to add item to item-to-item recommender"
                            );
                        }
                        span.add(1);
                    }
                }
            })
            .await;
        for recommender in &recommenders {
            if let Err(err) = recommender.clean().await {
                span.fail(&err);
                return Err(err.into());
            }
        }
        Ok(())
    }

    /// Go's `updateUserToUser`: builds the user-to-user recommenders over the
    /// processed users, and adds every user to them.
    pub(crate) async fn update_user_to_user(
        &self,
        parent: &Context,
        dataset: &Dataset,
    ) -> Result<(), Error> {
        let cfg = self.config();
        let Some(configs) = cfg.recommend.user_to_user.clone().filter(|c| !c.is_empty()) else {
            return Ok(());
        };
        let total = dataset.get_users().len() * configs.len();
        let (ctx, span) =
            self.tracer
                .start(parent, "Generate user-to-user recommendation", total as i64);
        let result = self
            .update_user_to_user_in(&ctx, &span, dataset, &configs)
            .await;
        span.end();
        result
    }

    async fn update_user_to_user_in(
        &self,
        ctx: &Context,
        span: &monitor::Span,
        dataset: &Dataset,
        configs: &[gorse_config::config::schema::UserToUserConfig],
    ) -> Result<(), Error> {
        let cfg = self.config();
        let options = UserToUserOptions {
            ctx: Some(ctx.clone()),
            vector_client: Some(self.vectors()),
            vector_config: self.recommender_vector_config(),
            batch_size: BATCH_SIZE,
            tags_idf: Some(dataset.get_user_column_values_idf()),
            items_idf: Some(dataset.get_item_idf()),
        };
        let mut recommenders = Vec::with_capacity(configs.len());
        for config in configs {
            recommenders.push(new_user_to_user(
                config,
                dataset.get_timestamp(),
                Some(&options),
            )?);
        }

        // Add users to user-to-user recommender
        let feedback = dataset.get_user_feedback();
        let jobs = cfg.master.n_jobs.max(1) as usize;
        futures_util::stream::iter(dataset.get_users().iter().enumerate())
            .for_each_concurrent(jobs, |(i, user)| {
                let recommenders = &recommenders;
                async move {
                    for recommender in recommenders {
                        if let Err(err) = recommender.add(user, &feedback[i]).await {
                            error!(
                                user_id = %user.user_id,
                                error = %err,
                                "failed to add user to user-to-user recommender"
                            );
                        }
                        span.add(1);
                    }
                }
            })
            .await;
        for recommender in &recommenders {
            if let Err(err) = recommender.clean().await {
                span.fail(&err);
                return Err(err.into());
            }
        }
        Ok(())
    }

    /// Go's `updateRecommend`: in standalone mode, loads the models the master
    /// holds and recommends for every user.
    pub(crate) async fn update_recommend(&self, parent: &Context) -> Result<(), Error> {
        let cfg = self.config();
        let pipeline = Arc::new(Pipeline::new(
            (*cfg).clone(),
            self.cache(),
            self.data(),
            self.vectors(),
            // Go shares the master's tracer, so the pipeline's spans show in `ps`.
            Arc::clone(&self.tracer),
            cfg.master.n_jobs.max(1) as usize,
        ));
        let store = self.blob_store();

        // load matrix factorization model
        let collaborative = self
            .collaborative_filtering
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .meta
            .id;
        if collaborative > 0 {
            let mut reader = match store.open(parent, &collaborative.to_string()).await {
                Ok(reader) => reader,
                Err(err) => {
                    error!(id = collaborative, error = %err, "failed to load collaborative filtering model from blob store");
                    return Err(err.into());
                }
            };
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            let mut users = MatrixFactorizationUsers::new();
            if let Err(err) = users.unmarshal(&mut bytes.as_slice()) {
                error!(error = %err, "failed to unmarshal matrix factorization users");
                return Err(err.into());
            }
            pipeline
                .update_matrix_factorization(parent, collaborative, users)
                .await?;
        }

        // load click-through rate model when FM ranker is enabled
        let click_through_rate = self
            .click_through_rate
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .meta
            .id;
        if cfg.recommend.ranker.kind.eq_ignore_ascii_case("fm") && click_through_rate > 0 {
            let mut reader = match store.open(parent, &click_through_rate.to_string()).await {
                Ok(reader) => reader,
                Err(err) => {
                    error!(error = %err, "failed to open click-through rate model");
                    return Err(err.into());
                }
            };
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            match unmarshal_model(&mut bytes.as_slice()) {
                Ok(model) => pipeline.set_click_through_rate_model(Some(Arc::from(model))),
                Err(err) => {
                    error!(error = %err, "failed to unmarshal click-through rate model");
                    return Err(err.into());
                }
            }
        }

        // Pull all users from database
        let users = self.pull_all_users(parent).await.inspect_err(|err| {
            error!(error = %err, "failed to pull users");
        })?;

        pipeline
            .recommend(
                parent.clone(),
                users,
                Some(Box::new(|completed, throughput| {
                    info!(
                        n_complete_users = completed,
                        throughput, "ranking recommendation"
                    );
                })),
            )
            .await;
        Ok(())
    }

    /// Go's `pullAllUsers`: every user in the data store.
    pub(crate) async fn pull_all_users(&self, ctx: &Context) -> Result<Vec<User>, Error> {
        let data = self.data();
        let mut users = Vec::new();
        let mut stream = data.get_user_stream(ctx, BATCH_SIZE);
        while let Some(batch) = stream.next().await {
            users.extend(batch?);
        }
        Ok(users)
    }
}
