// Ports: config/config.go (Config.Validate and its validator rules) @ add21b5

//! `Config.Validate`. Go runs the struct tags through go-playground/validator
//! and returns the first error it reports, translated with the English
//! messages. Here each tag of the Go struct is a check in the same place, in
//! the same field order, so the first failing check is the error Go returns.
//! Each check names its tag in a comment; `tests/data/config_golden.json`
//! holds the messages.

use std::collections::HashSet;

use gorse_common::gotime::Duration;
use gorse_expr::parser;
use gorse_storage_core::scheme::{
    MILVUS_PREFIX, MONGO_PREFIX, MONGO_SRV_PREFIX, MYSQL_PREFIX, POSTGRES_PREFIX,
    POSTGRESQL_PREFIX, QDRANT_PREFIX, REDIS_CLUSTER_PREFIX, REDIS_PREFIX, REDISS_CLUSTER_PREFIX,
    REDISS_PREFIX, SQLITE_PREFIX, WEAVIATE_PREFIX, WEAVIATES_PREFIX, XVEC_PREFIX,
};

use super::schema::{
    Config, ItemToItemConfig, NonPersonalizedConfig, RecommendConfig, UserToUserConfig,
};

/// An error from [`Config::validate`], with the message Go's `Validate`
/// returns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidateError(pub String);

type Check = Result<(), ValidateError>;

fn fail(message: impl Into<String>) -> Check {
    Err(ValidateError(message.into()))
}

/// `validate:"required"` on a string.
fn required(value: &str, field: &str) -> Check {
    if value.is_empty() {
        return fail(format!("{field} is a required field"));
    }
    Ok(())
}

/// `validate:"gte=0"` on an integer.
fn gte_zero(value: i64, field: &str) -> Check {
    if value < 0 {
        return fail(format!("{field} must be 0 or greater"));
    }
    Ok(())
}

/// `validate:"gt=0"` on an integer.
fn gt_zero(value: i64, field: &str) -> Check {
    if value <= 0 {
        return fail(format!("{field} must be greater than 0"));
    }
    Ok(())
}

/// `validate:"gte=0"` on a `time.Duration`.
fn duration_gte_zero(value: Duration, field: &str) -> Check {
    gte_zero(value.0, field)
}

/// `validate:"gt=0"` on a `time.Duration`.
fn duration_gt_zero(value: Duration, field: &str) -> Check {
    gt_zero(value.0, field)
}

/// `validate:"gt=0"` on a float64; NaN fails the comparison.
fn float_gt_zero(value: f64, field: &str) -> Check {
    if value.is_nan() || value <= 0.0 {
        return fail(format!("{field} must be greater than 0"));
    }
    Ok(())
}

/// `validate:"oneof=..."`.
fn one_of(value: &str, field: &str, options: &[&str]) -> Check {
    if !options.contains(&value) {
        return fail(format!("{field} must be one of [{}]", options.join(" ")));
    }
    Ok(())
}

/// `validate:"item_expr"`: an empty expression is legal; any other must parse.
fn item_expr(value: &str) -> Check {
    if !value.is_empty() && parser::parse(value).is_err() {
        return fail("invalid item expression");
    }
    Ok(())
}

/// `validate:"required,item_expr"`.
fn required_item_expr(value: &str, field: &str) -> Check {
    required(value, field)?;
    item_expr(value)
}

/// `validate:"required,<store>"`: the value must start with one of the
/// backend's prefixes.
fn store(value: &str, field: &str, prefixes: &[&str], unsupported: &str) -> Check {
    required(value, field)?;
    if !prefixes.iter().any(|p| value.starts_with(p)) {
        return fail(unsupported);
    }
    Ok(())
}

const DATA_STORE_PREFIXES: &[&str] = &[
    MONGO_PREFIX,
    MONGO_SRV_PREFIX,
    MYSQL_PREFIX,
    POSTGRES_PREFIX,
    POSTGRESQL_PREFIX,
    SQLITE_PREFIX,
];

const CACHE_STORE_PREFIXES: &[&str] = &[
    REDIS_PREFIX,
    REDISS_PREFIX,
    REDIS_CLUSTER_PREFIX,
    REDISS_CLUSTER_PREFIX,
    MONGO_PREFIX,
    MONGO_SRV_PREFIX,
    MYSQL_PREFIX,
    POSTGRES_PREFIX,
    POSTGRESQL_PREFIX,
    SQLITE_PREFIX,
];

const VECTOR_STORE_PREFIXES: &[&str] = &[
    XVEC_PREFIX,
    QDRANT_PREFIX,
    WEAVIATE_PREFIX,
    WEAVIATES_PREFIX,
    MILVUS_PREFIX,
];

impl Config {
    /// Validate: the checks Go makes on a config, the first failure wins.
    pub fn validate(&self) -> Check {
        if self.recommend.ranker.kind == "decision" {
            if self.recommend.ranker.decision_api.url.trim().is_empty() {
                return fail("recommend.ranker.decision_api.url must not be empty");
            }
            if self.recommend.ranker.decision_api.model.trim().is_empty() {
                return fail("recommend.ranker.decision_api.model must not be empty");
            }
        }
        check_duplicates(
            self.recommend
                .non_personalized
                .iter()
                .flatten()
                .map(|c| c.name.as_str()),
            "non-personalized",
        )?;
        check_duplicates(
            self.recommend
                .item_to_item
                .iter()
                .flatten()
                .map(|c| c.name.as_str()),
            "item-to-item",
        )?;

        self.validate_fields()?;

        let ranker = &self.recommend.ranker;
        let recommenders = ranker.recommenders.as_deref().unwrap_or_default();
        if recommenders.is_empty() {
            return fail("ranker.recommenders must not be empty");
        }
        if ranker.kind == "none" && recommenders.len() > 1 {
            return fail(
                "ranker.recommenders must contain at most one recommender when ranker.type is none",
            );
        }
        check_recommenders(&self.recommend, recommenders)?;
        check_recommenders(
            &self.recommend,
            self.recommend
                .fallback
                .recommenders
                .as_deref()
                .unwrap_or_default(),
        )
    }

    /// The struct tags, in the order the validator visits the fields.
    fn validate_fields(&self) -> Check {
        let db = &self.database;
        store(
            &db.data_store,
            "data_store",
            DATA_STORE_PREFIXES,
            "unsupported data storage backend",
        )?;
        store(
            &db.cache_store,
            "cache_store",
            CACHE_STORE_PREFIXES,
            "unsupported cache storage backend",
        )?;
        store(
            &db.vector_store,
            "vector_store",
            VECTOR_STORE_PREFIXES,
            "unsupported vector storage backend",
        )?;
        // [database.mysql]
        one_of(
            &db.mysql.isolation_level,
            "isolation_level",
            &[
                "READ-UNCOMMITTED",
                "READ-COMMITTED",
                "REPEATABLE-READ",
                "SERIALIZABLE",
            ],
        )?;
        gte_zero(db.mysql.max_open_conns, "max_open_conns")?;
        gte_zero(db.mysql.max_idle_conns, "max_idle_conns")?;
        duration_gte_zero(db.mysql.conn_max_lifetime, "conn_max_lifetime")?;
        // [database.postgres]
        gte_zero(db.postgres.max_open_conns, "max_open_conns")?;
        gte_zero(db.postgres.max_idle_conns, "max_idle_conns")?;
        duration_gte_zero(db.postgres.conn_max_lifetime, "conn_max_lifetime")?;
        // [database.redis]
        gt_zero(db.redis.max_search_results, "max_search_results")?;
        // [database.vector]
        if !db.vector.quantization_type.is_empty() {
            one_of(
                &db.vector.quantization_type,
                "quantization_type",
                &["sq", "pq", "rq"],
            )?;
        }
        gte_zero(db.vector.quantization_bits, "quantization_bits")?;

        // [master]
        let master = &self.master;
        gte_zero(master.port, "port")?;
        gte_zero(master.http_port, "http_port")?;
        gt_zero(master.n_jobs, "n_jobs")?;
        duration_gt_zero(master.meta_timeout, "meta_timeout")?;

        // [server]
        let server = &self.server;
        gt_zero(server.default_n, "default_n")?;
        duration_gte_zero(server.clock_error, "clock_error")?;
        duration_gt_zero(server.cache_expire, "cache_expire")?;

        self.recommend.validate_fields()?;

        // [tracing]
        one_of(
            &self.tracing.exporter,
            "exporter",
            &["zipkin", "otlp", "otlphttp"],
        )?;

        // [oidc]
        if !self.oidc.redirect_url.is_empty()
            && !self.oidc.redirect_url.ends_with("/callback/oauth2")
        {
            return fail(
                "Key: 'Config.oidc.redirect_url' Error:Field validation for 'redirect_url' failed on the 'endswith' tag",
            );
        }

        // [blob]
        required(&self.blob.uri, "uri")?;

        // [quota]
        let quota = &self.quota;
        gte_zero(quota.max_users_count, "max_users_count")?;
        gte_zero(quota.max_items_count, "max_items_count")?;
        gte_zero(quota.max_labels_size, "max_labels_size")?;
        gte_zero(quota.max_comment_size, "max_comment_size")?;
        gte_zero(quota.max_categories_count, "max_categories_count")?;
        gte_zero(quota.max_categories_size, "max_categories_size")
    }
}

impl RecommendConfig {
    /// The struct tags of the recommendation setup, in field order.
    fn validate_fields(&self) -> Check {
        gt_zero(self.cache_size, "cache_size")?;
        duration_gt_zero(self.cache_expire, "cache_expire")?;
        gt_zero(self.context_size, "context_size")?;
        gte_zero(self.active_user_ttl, "active_user_ttl")?;
        // [recommend.data_source]: `gte=0` on the unsigned TTLs always holds.
        // [recommend.search]
        for item in self.search.columns.iter().flatten() {
            item_expr(item)?;
        }
        for item in self.non_personalized.iter().flatten() {
            validate_non_personalized(item)?;
        }
        for item in self.item_to_item.iter().flatten() {
            validate_item_to_item(item)?;
        }
        for item in self.user_to_user.iter().flatten() {
            validate_user_to_user(item)?;
        }
        // [recommend.collaborative]
        let c = &self.collaborative;
        one_of(&c.kind, "type", &["none", "mf"])?;
        duration_gt_zero(c.fit_period, "fit_period")?;
        gt_zero(c.fit_epoch, "fit_epoch")?;
        duration_gte_zero(c.optimize_period, "optimize_period")?;
        gt_zero(c.optimize_trials, "optimize_trials")?;
        // [recommend.external]: `ExternalConfig` has no tags.
        // [recommend.replacement]
        float_gt_zero(
            self.replacement.positive_replacement_decay,
            "positive_replacement_decay",
        )?;
        float_gt_zero(
            self.replacement.read_replacement_decay,
            "read_replacement_decay",
        )?;
        // [recommend.ranker]
        let r = &self.ranker;
        one_of(&r.kind, "type", &["none", "fm", "reranker", "decision"])?;
        duration_gt_zero(r.cache_expire, "cache_expire")?;
        duration_gt_zero(r.fit_period, "fit_period")?;
        gt_zero(r.fit_epoch, "fit_epoch")?;
        duration_gte_zero(r.optimize_period, "optimize_period")?;
        gt_zero(r.optimize_trials, "optimize_trials")
    }
}

fn validate_non_personalized(item: &NonPersonalizedConfig) -> Check {
    required_item_expr(&item.score, "score")?;
    item_expr(&item.filter)
}

fn validate_item_to_item(item: &ItemToItemConfig) -> Check {
    one_of(&item.kind, "type", &["embedding", "tags", "users", "auto"])?;
    item_expr(&item.column)
}

fn validate_user_to_user(item: &UserToUserConfig) -> Check {
    one_of(&item.kind, "type", &["embedding", "tags", "items", "auto"])?;
    item_expr(&item.column)
}

/// Go's duplicate checks over the recommenders of one kind.
fn check_duplicates<'a>(names: impl Iterator<Item = &'a str>, kind: &str) -> Check {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return fail(format!("{kind} recommender {name} is duplicated"));
        }
    }
    Ok(())
}

/// `checkRecommenders`: every name is a recommender the config defines, and
/// the collaborative one is only named when it is enabled.
fn check_recommenders(config: &RecommendConfig, recommenders: &[String]) -> Check {
    let mut available: HashSet<String> = HashSet::new();
    available.extend(
        config
            .non_personalized
            .iter()
            .flatten()
            .map(|r| r.full_name()),
    );
    available.extend(config.item_to_item.iter().flatten().map(|r| r.full_name()));
    available.extend(config.user_to_user.iter().flatten().map(|r| r.full_name()));
    available.extend(config.external.iter().flatten().map(|r| r.full_name()));
    available.insert("latest".to_owned());
    if config.collaborative.kind != "none" {
        available.insert(config.collaborative.full_name());
    }
    for recommender in recommenders {
        if *recommender == config.collaborative.full_name() && config.collaborative.kind == "none" {
            return fail("collaborative recommender is disabled");
        }
        if !available.contains(recommender) {
            return fail(format!("recommender {recommender} doesn't exist"));
        }
    }
    Ok(())
}
