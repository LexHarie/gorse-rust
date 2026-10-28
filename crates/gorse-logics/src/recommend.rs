// Ports: logics/recommend.go @ add21b5

//! The recommendation pipeline of one user: the feedback the user has given,
//! the items the user has already seen, and the recommenders the configuration
//! names, read in order until enough items are found.
//!
//! Go's `Recommender` is [`Recommender`]. Go's `parse` returns a closure for
//! each recommender name; here it returns a `Source`, which
//! [`Recommender::recommend_sequential`] runs. The digest a run returns is the
//! hash of the recommenders' configs, which a caller compares to decide whether
//! a cached list is stale.
//!
//! Go walks a map of candidate scores in random order before it takes the top
//! k. Ties therefore come out in an order Go does not fix. This port walks the
//! candidates in ID order, so a tie is always broken the same way. Go can also
//! return a nil list where this port returns an empty one, which the REST work
//! must check (`PORTING_NOTES.md` O-76).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gorse_common::expression::match_feedback_type_expressions;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Time};
use gorse_common::heap::TopKFilter;
use gorse_common::util;
use gorse_config::config::schema::RecommendConfig;
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Feedback, Item, sort_feedbacks};
use gorse_storage::data::{self, GetOptions};
use gorse_storage::vectors;
use gorse_storage_core::errors::Error as StorageError;

use crate::external::{External, PullError};
use crate::item_to_item::{self, query_item_to_item};
use crate::user_to_user::query_user_to_user;

/// Go's `LatestRecommender`: the newest items.
pub const LATEST_RECOMMENDER: &str = "latest";
/// Go's `NonPersonalizedRecommender`: the prefix of a non-personalised name.
pub const NON_PERSONALIZED_RECOMMENDER: &str = "non-personalized/";
/// Go's `ItemToItemRecommender`: the prefix of an item-to-item name.
pub const ITEM_TO_ITEM_RECOMMENDER: &str = "item-to-item/";
/// Go's `UserToUserRecommender`: the prefix of a user-to-user name.
pub const USER_TO_USER_RECOMMENDER: &str = "user-to-user/";
/// Go's `ExternalRecommender`: the prefix of an external name.
pub const EXTERNAL_RECOMMENDER: &str = "external/";
/// Go's `CollaborativeRecommender`: the collaborative filtering recommender.
pub const COLLABORATIVE_RECOMMENDER: &str = "collaborative";

/// An error from a recommendation run, with Go's texts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The name is none of the recommender kinds (Go's `parse`).
    #[error("unknown recommender: {0}")]
    UnknownRecommender(String),
    /// The configuration has no item-to-item recommender with the name. Go
    /// wraps `storage.ErrNotFound` here.
    #[error("item-to-item recommender {name} {source}")]
    ItemToItemNotFound {
        /// The name that was asked for.
        name: String,
        /// Always `NotFound`.
        #[source]
        source: StorageError,
    },
    /// The configuration has no user-to-user recommender with the name. Go
    /// wraps `storage.ErrNotFound` here.
    #[error("user-to-user recommender {name} {source}")]
    UserToUserNotFound {
        /// The name that was asked for.
        name: String,
        /// Always `NotFound`.
        #[source]
        source: StorageError,
    },
    /// A data store call failed.
    #[error(transparent)]
    Data(#[from] data::Error),
    /// A cache call failed.
    #[error(transparent)]
    Cache(#[from] cache::Error),
    /// A vector store call failed.
    #[error(transparent)]
    Vectors(#[from] vectors::Error),
    /// The item-to-item or user-to-user query failed. Go's two query functions
    /// return one error type, and so does this port.
    #[error(transparent)]
    Query(#[from] item_to_item::Error),
    /// The external script did not start or did not compile.
    #[error(transparent)]
    External(#[from] rquickjs::Error),
    /// The external script's pull failed.
    #[error(transparent)]
    Pull(#[from] PullError),
    /// The external pull ran on a blocking thread, which did not finish.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

/// Which recommender a name in the configuration names. Go's `parse` returns a
/// closure; this is the closure's identity, which `Recommender::run` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Latest,
    Collaborative,
    NonPersonalized(String),
    ItemToItem(String),
    UserToUser(String),
    External(String),
}

/// Go's `parse`: the recommender a full name names. The exact names are
/// checked first, then the prefixes in the order Go checks them.
fn parse(fullname: &str) -> Result<Source, Error> {
    if fullname == COLLABORATIVE_RECOMMENDER {
        Ok(Source::Collaborative)
    } else if fullname == LATEST_RECOMMENDER {
        Ok(Source::Latest)
    } else if let Some(name) = fullname.strip_prefix(NON_PERSONALIZED_RECOMMENDER) {
        Ok(Source::NonPersonalized(name.to_owned()))
    } else if let Some(name) = fullname.strip_prefix(ITEM_TO_ITEM_RECOMMENDER) {
        Ok(Source::ItemToItem(name.to_owned()))
    } else if let Some(name) = fullname.strip_prefix(USER_TO_USER_RECOMMENDER) {
        Ok(Source::UserToUser(name.to_owned()))
    } else if let Some(name) = fullname.strip_prefix(EXTERNAL_RECOMMENDER) {
        Ok(Source::External(name.to_owned()))
    } else {
        Err(Error::UnknownRecommender(fullname.to_owned()))
    }
}

/// Go's `int` count, converted for a store or a heap. The configuration
/// validates a cache size as positive, so the conversion only ever sees a
/// positive count; a negative one, which Go's `make` would panic on, gives 0.
fn count(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// Go's `Recommender`: the state of one user's recommendation run.
pub struct Recommender {
    config: RecommendConfig,
    cache: Arc<dyn cache::Database>,
    data: Arc<dyn data::Database>,
    vectors: Arc<dyn vectors::Database>,
    online: bool,
    coldstart: bool,
    user_id: String,
    user_feedback: Vec<Feedback>,
    categories: Vec<String>,
    exclude_set: HashSet<String>,
}

impl Recommender {
    /// Go's `NewRecommender`. It reads the user's feedback, and excludes every
    /// item the user has given feedback on, except that a replacement config
    /// keeps the items the user only read, while the recommender is online.
    /// Negative feedback is always excluded. The user is cold-started until
    /// one feedback matches a positive type.
    ///
    /// # Errors
    ///
    /// Fails when the data store does.
    pub async fn new(
        config: RecommendConfig,
        cache: Arc<dyn cache::Database>,
        data: Arc<dyn data::Database>,
        vectors: Arc<dyn vectors::Database>,
        online: bool,
        user_id: String,
        categories: Vec<String>,
    ) -> Result<Self, Error> {
        // Go reads with `context.Background()` here, as this constructor has no
        // context of its own.
        let user_feedback = data
            .get_user_feedback(&Context::background(), &user_id, Some(gotime::now()), &[])
            .await?;
        let negative = config
            .data_source
            .negative_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let positive = config
            .data_source
            .positive_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let mut exclude_set = HashSet::new();
        let mut coldstart = true;
        for feedback in &user_feedback {
            let key = &feedback.feedback_key;
            // Negative feedback items are always excluded (highest priority).
            if match_feedback_type_expressions(negative, &key.feedback_type, feedback.value) {
                exclude_set.insert(key.item_id.clone());
            } else if !config.replacement.enable_replacement || !online {
                // Other feedback items are excluded unless replacement is enabled.
                exclude_set.insert(key.item_id.clone());
            }
            if match_feedback_type_expressions(positive, &key.feedback_type, feedback.value) {
                coldstart = false;
            }
        }
        Ok(Self {
            config,
            cache,
            data,
            vectors,
            online,
            coldstart,
            user_id,
            user_feedback,
            categories,
            exclude_set,
        })
    }

    /// Go's `ExcludeSet`: the items this run must not recommend. A run adds
    /// every item it returns, so a later recommender does not repeat one.
    pub fn exclude_set(&self) -> &HashSet<String> {
        &self.exclude_set
    }

    /// Go's `UserFeedback`: the user's feedback. An item-to-item run sorts it in
    /// place, as Go's does.
    pub fn user_feedback(&self) -> &[Feedback] {
        &self.user_feedback
    }

    /// Go's `IsColdStart`: whether no feedback of the user matches a positive
    /// type.
    pub fn is_cold_start(&self) -> bool {
        self.coldstart
    }

    /// Go's `Recommend`: up to `limit` items for the user. A `limit` of 0 or
    /// less asks for every item the recommenders give. The ranker's cached list
    /// is used when the ranker names a type other than `none`, and the
    /// fallback recommenders fill the rest.
    ///
    /// # Errors
    ///
    /// Fails when a store or a named recommender does.
    pub async fn recommend(&mut self, ctx: &Context, limit: i64) -> Result<Vec<Score>, Error> {
        let mut result = if !self.config.ranker.kind.eq_ignore_ascii_case("none") {
            let scores = self
                .cache
                .search_scores(
                    ctx,
                    cache::RECOMMEND,
                    &self.user_id,
                    &self.categories,
                    0,
                    self.config.cache_size,
                )
                .await?;
            let mut result = Vec::with_capacity(scores.len());
            for score in scores {
                if !self.exclude_set.contains(&score.id) {
                    self.exclude_set.insert(score.id.clone());
                    result.push(score);
                }
            }
            result
        } else {
            let names = self.config.ranker.recommenders.clone().unwrap_or_default();
            let cache_size = self.config.cache_size;
            self.recommend_sequential(ctx, Vec::new(), cache_size, &names)
                .await?
                .0
        };
        if limit > 0 && result.len() as i64 >= limit {
            result.truncate(limit as usize);
            return Ok(result);
        }
        let names = self
            .config
            .fallback
            .recommenders
            .clone()
            .unwrap_or_default();
        Ok(self
            .recommend_sequential(ctx, result, limit, &names)
            .await?
            .0)
    }

    /// Go's `RecommendSequential`: runs the named recommenders in order, and
    /// stops once `result` holds `limit` items. A `limit` of 0 or less runs
    /// them all. The digest is the MD5 of the digests of the recommenders run.
    ///
    /// # Errors
    ///
    /// Fails on the first name that is unknown or whose run fails. The items
    /// of the recommenders run before it are already in the exclude set.
    pub async fn recommend_sequential(
        &mut self,
        ctx: &Context,
        mut result: Vec<Score>,
        limit: i64,
        names: &[String],
    ) -> Result<(Vec<Score>, String), Error> {
        let mut digests = Vec::new();
        for name in names {
            let source = parse(name)?;
            let (scores, digest) = self.run(ctx, &source).await?;
            for score in &scores {
                self.exclude_set.insert(score.id.clone());
            }
            result.extend(scores);
            digests.push(digest);
            if limit > 0 && result.len() as i64 >= limit {
                result.truncate(limit as usize);
                return Ok((result, util::md5(&digests[..])));
            }
        }
        Ok((result, util::md5(&digests[..])))
    }

    /// Runs one recommender, which Go does through the closure `parse` returns.
    async fn run(&mut self, ctx: &Context, source: &Source) -> Result<(Vec<Score>, String), Error> {
        match source {
            Source::Latest => self.recommend_latest(ctx).await,
            Source::Collaborative => self.recommend_collaborative(ctx).await,
            Source::NonPersonalized(name) => self.recommend_non_personalized(ctx, name).await,
            Source::ItemToItem(name) => self.recommend_item_to_item(ctx, name).await,
            Source::UserToUser(name) => self.recommend_user_to_user(ctx, name).await,
            Source::External(name) => self.recommend_external(ctx, name).await,
        }
    }

    /// Go's `item.Timestamp` cut-off: an item older than the item TTL, in days,
    /// is not recommended. `None` when the TTL is not set.
    fn item_after(&self) -> Option<Time> {
        let ttl = self.config.data_source.item_ttl;
        (ttl > 0).then(|| gotime::now().add_date(0, 0, -(ttl as i64)))
    }

    /// The scores that have no excluded item.
    fn without_excluded(&self, items: Vec<Score>) -> Vec<Score> {
        items
            .into_iter()
            .filter(|item| !self.exclude_set.contains(&item.id))
            .collect()
    }

    /// Go's `recommendLatest`: the newest items of the data store, scored by
    /// their timestamp in Unix seconds.
    async fn recommend_latest(&self, ctx: &Context) -> Result<(Vec<Score>, String), Error> {
        let items = self
            .data
            .get_latest_items(
                ctx,
                self.config.cache_size,
                &self.categories,
                self.item_after(),
            )
            .await?;
        let scores = items
            .into_iter()
            .filter(|item| !self.exclude_set.contains(&item.item_id))
            .map(|item| Score {
                score: item.timestamp.unix() as f64,
                categories: item.categories.unwrap_or_default(),
                id: item.item_id,
                is_hidden: false,
                timestamp: Time::default(),
            })
            .collect();
        Ok((scores, LATEST_RECOMMENDER.to_owned()))
    }

    /// Go's `recommendNonPersonalized`: the cached list of a non-personalised
    /// recommender. With no category, the list is read under the empty
    /// category, which is how Go stores an uncategorised score.
    async fn recommend_non_personalized(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Vec<Score>, String), Error> {
        let categories = if self.categories.is_empty() {
            vec![String::new()]
        } else {
            self.categories.clone()
        };
        let items = self
            .cache
            .search_scores(
                ctx,
                cache::NON_PERSONALIZED,
                name,
                &categories,
                0,
                self.config.cache_size,
            )
            .await?;
        let digest = self
            .cache
            .get(ctx, &cache::key(&[cache::NON_PERSONALIZED_DIGEST, name]))
            .await
            .string()?;
        Ok((self.without_excluded(items), digest))
    }

    /// Go's `recommendCollaborative`: the cached list of collaborative
    /// filtering for the user.
    async fn recommend_collaborative(&self, ctx: &Context) -> Result<(Vec<Score>, String), Error> {
        let items = self
            .cache
            .search_scores(
                ctx,
                cache::COLLABORATIVE_FILTERING,
                &self.user_id,
                &self.categories,
                0,
                self.config.cache_size,
            )
            .await?;
        let digest = self
            .cache
            .get(
                ctx,
                &cache::key(&[cache::COLLABORATIVE_FILTERING_DIGEST, &self.user_id]),
            )
            .await
            .string()?;
        Ok((self.without_excluded(items), digest))
    }

    /// Go's `recommendItemToItem`: the neighbours of the user's positive
    /// feedback items, summed over the feedback, then the top ones.
    ///
    /// Go sorts the user's feedback in place, so this takes `&mut self` and
    /// leaves the sorted feedback in `user_feedback`, as Go's does.
    async fn recommend_item_to_item(
        &mut self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Vec<Score>, String), Error> {
        let Some(cfg) = self.config.get_item_to_item_config(name).cloned() else {
            return Err(Error::ItemToItemNotFound {
                name: name.to_owned(),
                source: StorageError::NotFound,
            });
        };
        sort_feedbacks(&mut self.user_feedback);
        let positive = self
            .config
            .data_source
            .positive_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let mut seeds = Vec::new();
        for feedback in &self.user_feedback {
            if match_feedback_type_expressions(
                positive,
                &feedback.feedback_key.feedback_type,
                feedback.value,
            ) {
                seeds.push(feedback.feedback_key.item_id.clone());
                if self.online && self.config.context_size <= seeds.len() as i64 {
                    break;
                }
            }
        }
        let categories: Vec<&str> = self.categories.iter().map(String::as_str).collect();
        let n = count(self.config.cache_size);
        let mut scores: BTreeMap<String, f64> = BTreeMap::new();
        let mut item_categories: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for seed in &seeds {
            let neighbors =
                query_item_to_item(ctx, self.vectors.as_ref(), &cfg, seed, &categories, n).await?;
            for neighbor in neighbors {
                if !self.exclude_set.contains(&neighbor.id) {
                    *scores.entry(neighbor.id.clone()).or_default() += neighbor.score;
                    item_categories.insert(neighbor.id, neighbor.categories);
                }
            }
        }
        let mut filter = TopKFilter::new(n);
        for (id, score) in scores {
            filter.push(id, score);
        }
        let results = filter
            .pop_all()
            .into_iter()
            .map(|elem| Score {
                categories: item_categories
                    .get(&elem.value)
                    .cloned()
                    .unwrap_or_default(),
                id: elem.value,
                score: elem.weight,
                is_hidden: false,
                timestamp: Time::default(),
            })
            .collect();
        Ok((results, cfg.hash(Some(&self.config))))
    }

    /// Go's `recommendUserToUser`: the positive feedback items of the users
    /// nearest to this one, weighted by how near they are. Hidden items and
    /// items older than the item TTL are dropped, and so is an item outside the
    /// requested categories.
    async fn recommend_user_to_user(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<(Vec<Score>, String), Error> {
        let Some(cfg) = self.config.get_user_to_user_config(name).cloned() else {
            return Err(Error::UserToUserNotFound {
                name: name.to_owned(),
                source: StorageError::NotFound,
            });
        };
        let similar_users = query_user_to_user(
            ctx,
            self.vectors.as_ref(),
            &cfg,
            &self.user_id,
            count(self.config.cache_size),
        )
        .await?;
        let positive = self
            .config
            .data_source
            .positive_feedback_types
            .as_deref()
            .unwrap_or(&[]);
        let mut scores: BTreeMap<String, f64> = BTreeMap::new();
        for user in similar_users {
            let feedbacks = self
                .data
                .get_user_feedback(ctx, &user.id, Some(gotime::now()), positive)
                .await?;
            for feedback in feedbacks {
                let item_id = feedback.feedback_key.item_id;
                if !self.exclude_set.contains(&item_id) {
                    *scores.entry(item_id).or_default() += user.score;
                }
            }
        }
        let n = count(self.config.cache_size);
        let mut filter = TopKFilter::new(n);
        for (id, score) in scores {
            filter.push(id, score);
        }
        let elems = filter.pop_all();
        let ids: Vec<String> = elems.iter().map(|elem| elem.value.clone()).collect();
        let items = self
            .data
            .batch_get_items(
                ctx,
                &ids,
                &GetOptions {
                    skip_hidden: true,
                    after: self.item_after(),
                    ..Default::default()
                },
            )
            .await?;
        let items: HashMap<String, Item> = items
            .into_iter()
            .map(|item| (item.item_id.clone(), item))
            .collect();
        let mut results = Vec::with_capacity(elems.len());
        for elem in elems {
            let Some(item) = items.get(&elem.value) else {
                continue;
            };
            // Go's `lo.Every(item.Categories, r.categories)`. The first argument
            // is the collection and the second the subset, so the item must
            // carry every requested category.
            let item_categories = item.categories.as_deref().unwrap_or(&[]);
            let within = self
                .categories
                .iter()
                .all(|category| item_categories.contains(category));
            if within {
                results.push(Score {
                    id: item.item_id.clone(),
                    score: elem.weight,
                    is_hidden: false,
                    categories: item.categories.clone().unwrap_or_default(),
                    timestamp: Time::default(),
                });
            }
        }
        Ok((results, cfg.hash(Some(&self.config))))
    }

    /// Go's `recommendExternal`: the items an external script pulls for the
    /// user. An external recommender takes no categories, so with a category
    /// it gives nothing. The script runs on a blocking thread, since its
    /// `fetch` blocks.
    async fn recommend_external(
        &self,
        _ctx: &Context,
        name: &str,
    ) -> Result<(Vec<Score>, String), Error> {
        let external_config = self
            .config
            .external
            .iter()
            .flatten()
            .find(|config| config.name == name)
            .cloned()
            .unwrap_or_default();
        if !self.categories.is_empty() {
            // External recommenders do not support categories.
            return Ok((Vec::new(), external_config.hash()));
        }
        let user_id = self.user_id.clone();
        let script_config = external_config.clone();
        let items = tokio::task::spawn_blocking(move || -> Result<Vec<String>, Error> {
            let external = External::new(&script_config)?;
            Ok(external.pull(&user_id)?)
        })
        .await??;
        let scores = items
            .into_iter()
            .filter(|id| !self.exclude_set.contains(id))
            .map(|id| Score {
                id,
                score: 0.0,
                is_hidden: false,
                categories: Vec::new(),
                timestamp: Time::default(),
            })
            .collect();
        Ok((scores, external_config.hash()))
    }
}

#[cfg(test)]
mod tests;
