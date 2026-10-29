// Ports: master/tasks.go @ add21b5 (removeOutOfDateModels, collectGarbage)

//! Removing the models the master no longer uses, from the blob store and the
//! vector store, and the scores of the users and the configurations that are
//! gone from the cache.

use std::collections::{HashMap, HashSet};
use std::sync::PoisonError;

use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_dataset::dataset::Dataset;
use gorse_dataset::index::NOT_ID;
use gorse_storage::cache::{self, ScoreCondition};
use gorse_storage::vectors;
use tracing::{error, info};

use crate::Error;
use crate::master::Master;

impl Master {
    /// Go's `removeOutOfDateModels`. It keeps the two newest complete
    /// collaborative filtering models and the models in use, and removes the
    /// other models from the blob store and their indexes from the vector store.
    pub(crate) async fn remove_out_of_date_models(&self, ctx: &Context) {
        let collaborative_id = self
            .collaborative_filtering
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .meta
            .id;
        let click_through_rate_id = self
            .click_through_rate
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .meta
            .id;
        let timestamp = collaborative_id.min(click_through_rate_id);

        let store = self.blob_store();
        let files = match store.list(ctx).await {
            Ok(files) => files,
            Err(err) => {
                error!(error = %err, "failed to list models in blob store");
                return;
            }
        };
        let mut models: HashMap<i64, String> = HashMap::new();
        for file in files {
            match file.parse::<i64>() {
                Ok(id) => {
                    models.insert(id, file);
                }
                Err(err) => {
                    info!(file = %file, error = %err, "failed to parse model id");
                }
            }
        }

        let vector_client = self.vectors();
        let collections = match vector_client.list_collections(ctx).await {
            Ok(collections) => collections,
            Err(err) => {
                error!(error = %err, "failed to list collections in vector store");
                return;
            }
        };
        let prefix = format!("{}_", vectors::COLLABORATIVE_FILTERING);
        let mut collaborative_collections: HashMap<i64, String> = HashMap::new();
        let mut complete: Vec<i64> = Vec::new();
        for collection in collections {
            let Some(suffix) = collection.strip_prefix(&prefix) else {
                continue;
            };
            let Ok(id) = suffix.parse::<i64>() else {
                continue;
            };
            if vectors::collaborative_filtering_collection(id) != collection {
                continue;
            }
            if models.contains_key(&id) {
                complete.push(id);
            }
            collaborative_collections.insert(id, collection);
        }
        complete.sort_unstable_by(|a, b| b.cmp(a));
        let mut keep: HashSet<i64> = HashSet::from([collaborative_id]);
        keep.extend(complete.iter().take(2).copied());

        for (id, collection) in &collaborative_collections {
            if keep.contains(id) {
                continue;
            }
            if let Some(file) = models.get(id).cloned()
                && *id != click_through_rate_id
            {
                if let Err(err) = store.remove(ctx, &file).await {
                    error!(id, error = %err, "failed to delete collaborative filtering model from blob store");
                    continue;
                }
                models.remove(id);
                info!(
                    id,
                    "deleted out-of-date collaborative filtering model from blob store"
                );
            }
            if let Err(err) = vector_client.delete_collection(ctx, collection).await {
                error!(id, error = %err, "failed to delete collaborative filtering index from vector store");
                continue;
            }
            info!(
                id,
                "deleted out-of-date collaborative filtering index from vector store"
            );
        }

        for (id, file) in &models {
            if *id < timestamp && !keep.contains(id) {
                if let Err(err) = store.remove(ctx, file).await {
                    error!(id, error = %err, "failed to delete model from blob store");
                } else {
                    info!(id, "deleted out-of-date model from blob store");
                }
            }
        }
    }

    /// Go's `collectGarbage`: deletes the cached scores of the non-personalized
    /// recommenders that are no longer configured, and of the collaborative
    /// filtering users who are no longer in the dataset.
    pub(crate) async fn collect_garbage(
        &self,
        parent: &Context,
        dataset: &Dataset,
    ) -> Result<(), Error> {
        let (ctx, span) = self.tracer.start(parent, "Collect Garbage in Cache", 1);
        let result = self.collect_garbage_in(&ctx, dataset).await;
        span.end();
        result
    }

    async fn collect_garbage_in(&self, ctx: &Context, dataset: &Dataset) -> Result<(), Error> {
        let cfg = self.config();
        let cache_client = self.cache();
        let configured: HashSet<String> = cfg
            .recommend
            .non_personalized
            .iter()
            .flatten()
            .map(|config| config.name.clone())
            .collect();

        // The scan's callback is synchronous, so the deletions are collected and
        // run once the scan has finished.
        let mut deletions: Vec<(String, ScoreCondition)> = Vec::new();
        let mut scan = |collection: &str,
                        _id: &str,
                        subset: &str,
                        _timestamp: Time|
         -> Result<(), gorse_storage::cache::Error> {
            match collection {
                cache::NON_PERSONALIZED => {
                    if !configured.contains(subset) {
                        deletions.push((
                            cache::NON_PERSONALIZED.to_owned(),
                            ScoreCondition {
                                subset: Some(subset.to_owned()),
                                ..Default::default()
                            },
                        ));
                    }
                }
                cache::COLLABORATIVE_FILTERING if dataset.get_user_dict().id(subset) == NOT_ID => {
                    deletions.push((
                        cache::COLLABORATIVE_FILTERING.to_owned(),
                        ScoreCondition {
                            subset: Some(subset.to_owned()),
                            before: Some(dataset.get_timestamp()),
                            ..Default::default()
                        },
                    ));
                }
                _ => {}
            }
            Ok(())
        };
        cache_client.scan_scores(ctx, &mut scan).await?;
        for (collection, condition) in deletions {
            cache_client
                .delete_scores(ctx, std::slice::from_ref(&collection), &condition)
                .await?;
        }
        Ok(())
    }
}
