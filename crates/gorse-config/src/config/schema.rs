// Ports: config/config.go (the Config types, their hashes and helpers) @ add21b5

//! The configuration types. Each struct's fields are the Go struct's fields
//! in declaration order, with the `mapstructure` tag of each field in
//! [`Decode`] and the Go field name in the JSON form ([`super::json`]).
//! Go's `time.Duration` is [`Duration`], Go's `uint` is `u64` and Go's `int`
//! is `i64`.

use std::collections::HashSet;

use gorse_common::expression::FeedbackTypeExpression;
use gorse_common::gotime::{self, Duration, Time};
use gorse_storage_core::options::{
    Opt, with_conn_max_lifetime, with_isolation_level, with_max_idle_conns, with_max_open_conns,
    with_max_search_results, with_redis_client_name,
};
use gorse_storage_core::scheme::{
    MYSQL_PREFIX, POSTGRES_PREFIX, POSTGRESQL_PREFIX, REDIS_CLUSTER_PREFIX, REDIS_PREFIX,
    REDISS_CLUSTER_PREFIX, REDISS_PREFIX,
};
use md5::{Digest as _, Md5};

use crate::mapstructure::{Decode, Errors, Fields, decode_struct};
use crate::value::Value;

/// Config is the configuration for the engine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// The data, cache and vector stores.
    pub database: DatabaseConfig,
    /// The master node's settings.
    pub master: MasterConfig,
    /// The server node's settings.
    pub server: ServerConfig,
    /// The recommendation settings.
    pub recommend: RecommendConfig,
    /// OpenTelemetry tracing.
    pub tracing: TracingConfig,
    /// OpenID Connect login.
    pub oidc: OIDCConfig,
    /// The OpenAI API client.
    pub openai: OpenAIConfig,
    /// Blob storage for models and data.
    pub blob: BlobConfig,
    /// Resource quotas.
    pub quota: QuotaConfig,
}

/// DatabaseConfig is the configuration for the database.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DatabaseConfig {
    /// database for data store
    pub data_store: String,
    /// database for cache store
    pub cache_store: String,
    /// database for vector indices
    pub vector_store: String,
    /// table prefix of every store
    pub table_prefix: String,
    /// table prefix of the data store
    pub data_table_prefix: String,
    /// table prefix of the cache store
    pub cache_table_prefix: String,
    /// client name the cache store reports to Redis
    pub cache_client_name: String,
    /// table prefix of the vector store
    pub vector_table_prefix: String,
    /// MySQL connection settings.
    pub mysql: MySQLConfig,
    /// PostgreSQL connection settings.
    pub postgres: SQLConfig,
    /// Redis search settings.
    pub redis: RedisConfig,
    /// Vector quantization settings.
    pub vector: VectorConfig,
}

/// The MySQL connection settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MySQLConfig {
    /// the transaction isolation level
    pub isolation_level: String,
    /// the maximum number of open connections, 0 for no limit
    pub max_open_conns: i64,
    /// the maximum number of idle connections
    pub max_idle_conns: i64,
    /// the longest time a connection is reused, 0 for no limit
    pub conn_max_lifetime: Duration,
}

/// The PostgreSQL connection settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SQLConfig {
    /// the maximum number of open connections, 0 for no limit
    pub max_open_conns: i64,
    /// the maximum number of idle connections
    pub max_idle_conns: i64,
    /// the longest time a connection is reused, 0 for no limit
    pub conn_max_lifetime: Duration,
}

/// The Redis search settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RedisConfig {
    /// the most results a search returns
    pub max_search_results: i64,
}

/// The vector store settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorConfig {
    /// the quantization type, empty for none
    pub quantization_type: String,
    /// the bits per quantized value
    pub quantization_bits: i64,
}

/// MasterConfig is the configuration for the master.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MasterConfig {
    /// master port
    pub port: i64,
    /// master host
    pub host: String,
    /// enable SSL mode
    pub ssl_mode: bool,
    /// SSL CA file
    pub ssl_ca: String,
    /// SSL certificate file
    pub ssl_cert: String,
    /// SSL key file
    pub ssl_key: String,
    /// HTTP port
    pub http_port: i64,
    /// HTTP host
    pub http_host: String,
    /// add allowed cors domains
    pub http_cors_domains: Option<Vec<String>>,
    /// add allowed cors methods
    pub http_cors_methods: Option<Vec<String>>,
    /// number of working jobs
    pub n_jobs: i64,
    /// cluster meta timeout (second)
    pub meta_timeout: Duration,
    /// dashboard user name
    pub dashboard_user_name: String,
    /// dashboard password
    pub dashboard_password: String,
    /// hide the secrets the dashboard shows
    pub dashboard_redacted: bool,
    /// the key the admin API accepts
    pub admin_api_key: String,
}

/// ServerConfig is the configuration for the server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerConfig {
    /// secret key for RESTful APIs (SSL required)
    pub api_key: String,
    /// default number of returned items
    pub default_n: i64,
    /// clock error in the cluster in seconds
    pub clock_error: Duration,
    /// insert new users while inserting feedback
    pub auto_insert_user: bool,
    /// insert new items while inserting feedback
    pub auto_insert_item: bool,
    /// server-side cache expire time
    pub cache_expire: Duration,
}

/// RecommendConfig is the configuration of recommendation setup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecommendConfig {
    /// the number of recommended items cached per user
    pub cache_size: i64,
    /// how long a cached recommendation lives
    pub cache_expire: Duration,
    /// the number of recent items that feed a recommendation
    pub context_size: i64,
    /// how many days a user stays active
    pub active_user_ttl: i64,
    /// which feedback the recommenders read
    pub data_source: DataSourceConfig,
    /// the search columns
    pub search: SearchConfig,
    /// the non-personalized recommenders
    pub non_personalized: Option<Vec<NonPersonalizedConfig>>,
    /// the item-to-item recommenders
    pub item_to_item: Option<Vec<ItemToItemConfig>>,
    /// the user-to-user recommenders
    pub user_to_user: Option<Vec<UserToUserConfig>>,
    /// the collaborative filtering recommender
    pub collaborative: CollaborativeConfig,
    /// the external recommenders
    pub external: Option<Vec<ExternalConfig>>,
    /// how feedback replaces recommended items
    pub replacement: ReplacementConfig,
    /// the ranker that orders the recommenders' items
    pub ranker: RankerConfig,
    /// the recommenders used when the others return nothing
    pub fallback: FallbackConfig,
}

impl RecommendConfig {
    /// ListRecommenders
    pub fn list_recommenders(&self) -> Vec<String> {
        let mut recommenders = Vec::new();
        for rec in self.non_personalized.iter().flatten() {
            recommenders.push(rec.full_name());
        }
        for rec in self.item_to_item.iter().flatten() {
            recommenders.push(rec.full_name());
        }
        for rec in self.user_to_user.iter().flatten() {
            recommenders.push(rec.full_name());
        }
        for rec in self.external.iter().flatten() {
            recommenders.push(rec.full_name());
        }
        recommenders.push(self.collaborative.full_name());
        recommenders.push("latest".to_owned());
        recommenders
    }

    /// GetItemToItemConfig: the first item-to-item recommender with `name`.
    pub fn get_item_to_item_config(&self, name: &str) -> Option<&ItemToItemConfig> {
        self.item_to_item.iter().flatten().find(|c| c.name == name)
    }

    /// GetUserToUserConfig: the first user-to-user recommender with `name`.
    pub fn get_user_to_user_config(&self, name: &str) -> Option<&UserToUserConfig> {
        self.user_to_user.iter().flatten().find(|c| c.name == name)
    }

    /// Hash: the MD5 of the hashes of the recommenders the ranker uses, or of
    /// all of them when the ranker names none.
    pub fn hash(&self) -> String {
        let mut recommenders: HashSet<String> =
            self.ranker.recommenders.iter().flatten().cloned().collect();
        if recommenders.is_empty() {
            recommenders.extend(self.list_recommenders());
        }
        let mut digests = Vec::new();
        for rec in self.non_personalized.iter().flatten() {
            if recommenders.contains(&rec.full_name()) {
                digests.push(rec.hash());
            }
        }
        for rec in self.item_to_item.iter().flatten() {
            if recommenders.contains(&rec.full_name()) {
                digests.push(rec.hash(Some(self)));
            }
        }
        for rec in self.user_to_user.iter().flatten() {
            if recommenders.contains(&rec.full_name()) {
                digests.push(rec.hash(Some(self)));
            }
        }
        for rec in self.external.iter().flatten() {
            if recommenders.contains(&rec.full_name()) {
                digests.push(rec.hash());
            }
        }
        if recommenders.contains(&self.collaborative.full_name()) {
            digests.push(self.collaborative.hash(self));
        }
        if recommenders.contains("latest") {
            digests.push("latest".to_owned());
        }
        md5_of_sorted(&digests)
    }
}

/// The feedback the recommenders read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataSourceConfig {
    /// positive feedback type
    pub positive_feedback_types: Option<Vec<FeedbackTypeExpression>>,
    /// negative feedback type (highest priority)
    pub negative_feedback_types: Option<Vec<FeedbackTypeExpression>>,
    /// feedback type for read event
    pub read_feedback_types: Option<Vec<FeedbackTypeExpression>>,
    /// time-to-live of positive feedbacks
    pub positive_feedback_ttl: u64,
    /// time-to-live of items
    pub item_ttl: u64,
}

/// The search settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchConfig {
    /// the item columns the search covers
    pub columns: Option<Vec<String>>,
}

/// A non-personalized recommender: the items a score orders, with a filter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NonPersonalizedConfig {
    /// the recommender's name
    pub name: String,
    /// the expression that scores each item
    pub score: String,
    /// the expression that keeps an item
    pub filter: String,
}

impl NonPersonalizedConfig {
    /// The name the recommender is listed under.
    pub fn full_name(&self) -> String {
        format!("non-personalized/{}", self.name)
    }

    /// Hash: the MD5 of the name, score and filter.
    pub fn hash(&self) -> String {
        md5_stream(&[&self.name, &self.score, &self.filter])
    }
}

/// An item-to-item recommender.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemToItemConfig {
    /// the recommender's name
    pub name: String,
    /// Go's `Type`
    pub kind: String,
    /// the item column the similarity is taken over
    pub column: String,
}

impl ItemToItemConfig {
    /// The name the recommender is listed under.
    pub fn full_name(&self) -> String {
        format!("item-to-item/{}", self.name)
    }

    /// Hash: the name, type and column, and for type `users` the feedback
    /// types. `cfg` is only read for type `users`.
    pub fn hash(&self, cfg: Option<&RecommendConfig>) -> String {
        let mut parts = vec![self.name.as_str(), self.kind.as_str(), self.column.as_str()];
        let mut feedback = Vec::new();
        if self.kind == "users"
            && let Some(cfg) = cfg
        {
            feedback = feedback_strings(&cfg.data_source);
        }
        parts.extend(feedback.iter().map(String::as_str));
        md5_stream(&parts)
    }
}

/// A user-to-user recommender.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserToUserConfig {
    /// the recommender's name
    pub name: String,
    /// Go's `Type`
    pub kind: String,
    /// the user column the similarity is taken over
    pub column: String,
}

impl UserToUserConfig {
    /// The name the recommender is listed under.
    pub fn full_name(&self) -> String {
        format!("user-to-user/{}", self.name)
    }

    /// Hash: the name, type and column, and for type `items` the feedback
    /// types. `cfg` is only read for type `items`.
    pub fn hash(&self, cfg: Option<&RecommendConfig>) -> String {
        let mut parts = vec![self.name.as_str(), self.kind.as_str(), self.column.as_str()];
        let mut feedback = Vec::new();
        if self.kind == "items"
            && let Some(cfg) = cfg
        {
            feedback = feedback_strings(&cfg.data_source);
        }
        parts.extend(feedback.iter().map(String::as_str));
        md5_stream(&parts)
    }
}

/// The collaborative filtering recommender.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollaborativeConfig {
    /// Go's `Type`
    pub kind: String,
    /// how often the model is fitted
    pub fit_period: Duration,
    /// the number of epochs each fit runs
    pub fit_epoch: i64,
    /// how often the hyperparameters are optimized
    pub optimize_period: Duration,
    /// the number of optimization trials
    pub optimize_trials: i64,
    /// when fitting stops early
    pub early_stopping: EarlyStoppingConfig,
}

impl CollaborativeConfig {
    /// The name the recommender is listed under.
    pub fn full_name(&self) -> String {
        "collaborative".to_owned()
    }

    /// Hash: the positive and negative feedback types.
    pub fn hash(&self, cfg: &RecommendConfig) -> String {
        let parts = feedback_strings(&cfg.data_source);
        md5_stream(&parts.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

/// When a model's fitting stops early.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EarlyStoppingConfig {
    /// the number of epochs without improvement that stop the fit
    pub patience: i64,
}

/// An external recommender, run by a script.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExternalConfig {
    /// the recommender's name
    pub name: String,
    /// the script that produces the items
    pub script: String,
}

impl ExternalConfig {
    /// The name the recommender is listed under.
    pub fn full_name(&self) -> String {
        format!("external/{}", self.name)
    }

    /// Hash: the MD5 of the name and script.
    pub fn hash(&self) -> String {
        md5_stream(&[&self.name, &self.script])
    }
}

/// How feedback replaces the items already recommended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplacementConfig {
    /// whether feedback replaces recommended items
    pub enable_replacement: bool,
    /// the decay of a positive feedback's replacement
    pub positive_replacement_decay: f64,
    /// the decay of a read feedback's replacement
    pub read_replacement_decay: f64,
}

/// The ranker that orders the recommenders' items.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RankerConfig {
    /// Go's `Type`
    pub kind: String,
    /// the recommenders the ranker reads
    pub recommenders: Option<Vec<String>>,
    /// how long a ranked result lives
    pub cache_expire: Duration,
    /// how often the ranker is fitted
    pub fit_period: Duration,
    /// the number of epochs each fit runs
    pub fit_epoch: i64,
    /// how often the hyperparameters are optimized
    pub optimize_period: Duration,
    /// the number of optimization trials
    pub optimize_trials: i64,
    /// when fitting stops early
    pub early_stopping: EarlyStoppingConfig,
    /// the reranker API settings
    pub reranker_api: RerankerAPIConfig,
    /// the decision API settings
    pub decision_api: DecisionAPIConfig,
}

/// The recommenders used when the others return nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FallbackConfig {
    /// the fallback recommenders, by name
    pub recommenders: Option<Vec<String>>,
}

/// TracingConfig is the configuration of OpenTelemetry tracing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TracingConfig {
    /// whether traces are exported
    pub enable_tracing: bool,
    /// the exporter the traces are sent with
    pub exporter: String,
    /// the endpoint the exporter sends to
    pub collector_endpoint: String,
    /// the sampler that decides which traces are kept
    pub sampler: String,
    /// the fraction of traces kept, for the ratio sampler
    pub ratio: f64,
}

impl TracingConfig {
    /// Equal: whether the two configs would build the same tracer. Go's
    /// receiver may be nil, which returns false; a Rust reference is never nil.
    pub fn equal(&self, other: &TracingConfig) -> bool {
        self.enable_tracing == other.enable_tracing
            && self.exporter == other.exporter
            && self.collector_endpoint == other.collector_endpoint
            && self.sampler == other.sampler
            && self.ratio == other.ratio
    }
}

/// OpenID Connect login.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OIDCConfig {
    /// whether login goes through the identity provider
    pub enable: bool,
    /// the identity provider's issuer URL
    pub issuer: String,
    /// the client ID registered with the provider
    pub client_id: String,
    /// the client secret registered with the provider
    pub client_secret: String,
    /// the URL the provider redirects back to
    pub redirect_url: String,
}

/// The decision API settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionAPIConfig {
    /// the template of the state sent to the API
    pub state_template: String,
    /// the template of the instructions sent to the API
    pub instructions_template: String,
    /// the token that authorizes the API calls
    pub auth_token: String,
    /// the model the API runs
    pub model: String,
    /// the URL of the API
    pub url: String,
}

/// The reranker API settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RerankerAPIConfig {
    /// the template of the query sent to the API
    pub query_template: String,
    /// the template of each document sent to the API
    pub document_template: String,
    /// the token that authorizes the API calls
    pub auth_token: String,
    /// the model the API runs
    pub model: String,
    /// the URL of the API
    pub url: String,
}

/// The OpenAI API client.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenAIConfig {
    /// the base URL of the API
    pub base_url: String,
    /// the token that authorizes the API calls
    pub auth_token: String,
    /// the model for chat completions
    pub chat_completion_model: String,
    /// the requests-per-minute limit for chat completions
    pub chat_completion_rpm: i64,
    /// the tokens-per-minute limit for chat completions
    pub chat_completion_tpm: i64,
    /// the model for embeddings
    pub embedding_model: String,
    /// the dimensions of each embedding
    pub embedding_dimensions: i64,
    /// the requests-per-minute limit for embeddings
    pub embedding_rpm: i64,
    /// the tokens-per-minute limit for embeddings
    pub embedding_tpm: i64,
    /// the file the API calls are logged to
    pub log_file: String,
}

/// Blob storage for models and data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlobConfig {
    /// the URI of the blob store
    pub uri: String,
    /// the S3 settings
    pub s3: S3Config,
    /// the Google Cloud Storage settings
    pub gcs: GCSConfig,
    /// the Azure Blob settings
    pub azure: AzureBlobConfig,
}

/// The S3 settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct S3Config {
    /// the endpoint of the S3 service
    pub endpoint: String,
    /// the access key ID
    pub access_key_id: String,
    /// the secret access key
    pub secret_access_key: String,
}

/// The Google Cloud Storage settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GCSConfig {
    /// the file of the service-account credentials
    pub credentials_file: String,
}

/// The Azure Blob settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AzureBlobConfig {
    /// the endpoint of the Azure Blob service
    pub endpoint: String,
    /// the storage account name
    pub account_name: String,
    /// the storage account key
    pub account_key: String,
    /// the connection string of the account
    pub connection_string: String,
}

/// QuotaConfig is the configuration for resource quotas.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuotaConfig {
    /// Max total number of Users (0 = no limit)
    pub max_users_count: i64,
    /// Max total number of Items (0 = no limit)
    pub max_items_count: i64,
    /// Max size of Labels in bytes (0 = no limit)
    pub max_labels_size: i64,
    /// Max size of Comment in bytes (0 = no limit)
    pub max_comment_size: i64,
    /// Max number of Categories per item (0 = no limit)
    pub max_categories_count: i64,
    /// Max size of Categories in bytes (0 = no limit)
    pub max_categories_size: i64,
}

impl DatabaseConfig {
    /// StorageOptions: the options the store at `path` takes from this
    /// config, or none for a backend without options.
    pub fn storage_options(&self, path: &str) -> Vec<Opt> {
        if path.starts_with(MYSQL_PREFIX) {
            return vec![
                with_isolation_level(self.mysql.isolation_level.clone()),
                with_max_open_conns(self.mysql.max_open_conns),
                with_max_idle_conns(self.mysql.max_idle_conns),
                with_conn_max_lifetime(self.mysql.conn_max_lifetime),
            ];
        }
        if path.starts_with(POSTGRES_PREFIX) || path.starts_with(POSTGRESQL_PREFIX) {
            return vec![
                with_max_open_conns(self.postgres.max_open_conns),
                with_max_idle_conns(self.postgres.max_idle_conns),
                with_conn_max_lifetime(self.postgres.conn_max_lifetime),
            ];
        }
        if path.starts_with(REDIS_PREFIX)
            || path.starts_with(REDISS_PREFIX)
            || path.starts_with(REDIS_CLUSTER_PREFIX)
            || path.starts_with(REDISS_CLUSTER_PREFIX)
        {
            return vec![
                with_max_search_results(self.redis.max_search_results),
                with_redis_client_name(self.cache_client_name.clone()),
            ];
        }
        Vec::new()
    }
}

impl Config {
    /// Now: the time as the cluster's clocks may disagree on it, ahead by the
    /// clock error.
    pub fn now(&self) -> Time {
        gotime::now().add(self.server.clock_error)
    }
}

/// The feedback type expressions of `data_source` as Go's hashes write them:
/// the positive ones, then the negative ones.
fn feedback_strings(data_source: &DataSourceConfig) -> Vec<String> {
    data_source
        .positive_feedback_types
        .iter()
        .flatten()
        .chain(data_source.negative_feedback_types.iter().flatten())
        .map(ToString::to_string)
        .collect()
}

/// `md5.New()` written with each part in turn, then hex-encoded.
fn md5_stream(parts: &[&str]) -> String {
    let mut hash = Md5::new();
    for part in parts {
        hash.update(part.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// `util.MD5(s...)`: the digests sorted byte-wise, then hashed as one stream
/// (`gorse_common::util::md5`).
fn md5_of_sorted(digests: &[String]) -> String {
    gorse_common::util::md5(digests)
}

// The mapstructure decoders. Each struct lists its fields by their
// `mapstructure` tags in declaration order, as `decodeStructFromMap` visits
// them, so errors come out in the same order.

impl Decode for Config {
    fn go_type() -> String {
        "config.Config".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(self, name, "config.Config", input, errs, |out, f, errs| {
            f.decode("database", &mut out.database, errs);
            f.decode("master", &mut out.master, errs);
            f.decode("server", &mut out.server, errs);
            f.decode("recommend", &mut out.recommend, errs);
            f.decode("tracing", &mut out.tracing, errs);
            f.decode("oidc", &mut out.oidc, errs);
            f.decode("openai", &mut out.openai, errs);
            f.decode("blob", &mut out.blob, errs);
            f.decode("quota", &mut out.quota, errs);
        });
    }
}

impl Decode for DatabaseConfig {
    fn go_type() -> String {
        "config.DatabaseConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.DatabaseConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("data_store", &mut out.data_store, errs);
                f.decode("cache_store", &mut out.cache_store, errs);
                f.decode("vector_store", &mut out.vector_store, errs);
                f.decode("table_prefix", &mut out.table_prefix, errs);
                f.decode("data_table_prefix", &mut out.data_table_prefix, errs);
                f.decode("cache_table_prefix", &mut out.cache_table_prefix, errs);
                f.decode("cache_client_name", &mut out.cache_client_name, errs);
                f.decode("vector_table_prefix", &mut out.vector_table_prefix, errs);
                f.decode("mysql", &mut out.mysql, errs);
                f.decode("postgres", &mut out.postgres, errs);
                f.decode("redis", &mut out.redis, errs);
                f.decode("vector", &mut out.vector, errs);
            },
        );
    }
}

impl Decode for MySQLConfig {
    fn go_type() -> String {
        "config.MySQLConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.MySQLConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("isolation_level", &mut out.isolation_level, errs);
                f.decode("max_open_conns", &mut out.max_open_conns, errs);
                f.decode("max_idle_conns", &mut out.max_idle_conns, errs);
                f.decode("conn_max_lifetime", &mut out.conn_max_lifetime, errs);
            },
        );
    }
}

impl Decode for SQLConfig {
    fn go_type() -> String {
        "config.SQLConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.SQLConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("max_open_conns", &mut out.max_open_conns, errs);
                f.decode("max_idle_conns", &mut out.max_idle_conns, errs);
                f.decode("conn_max_lifetime", &mut out.conn_max_lifetime, errs);
            },
        );
    }
}

impl Decode for RedisConfig {
    fn go_type() -> String {
        "config.RedisConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.RedisConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("max_search_results", &mut out.max_search_results, errs);
            },
        );
    }
}

impl Decode for VectorConfig {
    fn go_type() -> String {
        "config.VectorConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.VectorConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("quantization_type", &mut out.quantization_type, errs);
                f.decode("quantization_bits", &mut out.quantization_bits, errs);
            },
        );
    }
}

impl Decode for MasterConfig {
    fn go_type() -> String {
        "config.MasterConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.MasterConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("port", &mut out.port, errs);
                f.decode("host", &mut out.host, errs);
                f.decode("ssl_mode", &mut out.ssl_mode, errs);
                f.decode("ssl_ca", &mut out.ssl_ca, errs);
                f.decode("ssl_cert", &mut out.ssl_cert, errs);
                f.decode("ssl_key", &mut out.ssl_key, errs);
                f.decode("http_port", &mut out.http_port, errs);
                f.decode("http_host", &mut out.http_host, errs);
                f.decode("http_cors_domains", &mut out.http_cors_domains, errs);
                f.decode("http_cors_methods", &mut out.http_cors_methods, errs);
                f.decode("n_jobs", &mut out.n_jobs, errs);
                f.decode("meta_timeout", &mut out.meta_timeout, errs);
                f.decode("dashboard_user_name", &mut out.dashboard_user_name, errs);
                f.decode("dashboard_password", &mut out.dashboard_password, errs);
                f.decode("dashboard_redacted", &mut out.dashboard_redacted, errs);
                f.decode("admin_api_key", &mut out.admin_api_key, errs);
            },
        );
    }
}

impl Decode for ServerConfig {
    fn go_type() -> String {
        "config.ServerConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.ServerConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("api_key", &mut out.api_key, errs);
                f.decode("default_n", &mut out.default_n, errs);
                f.decode("clock_error", &mut out.clock_error, errs);
                f.decode("auto_insert_user", &mut out.auto_insert_user, errs);
                f.decode("auto_insert_item", &mut out.auto_insert_item, errs);
                f.decode("cache_expire", &mut out.cache_expire, errs);
            },
        );
    }
}

impl Decode for RecommendConfig {
    fn go_type() -> String {
        "config.RecommendConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.RecommendConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("cache_size", &mut out.cache_size, errs);
                f.decode("cache_expire", &mut out.cache_expire, errs);
                f.decode("context_size", &mut out.context_size, errs);
                f.decode("active_user_ttl", &mut out.active_user_ttl, errs);
                f.decode("data_source", &mut out.data_source, errs);
                f.decode("search", &mut out.search, errs);
                f.decode("non-personalized", &mut out.non_personalized, errs);
                f.decode("item-to-item", &mut out.item_to_item, errs);
                f.decode("user-to-user", &mut out.user_to_user, errs);
                f.decode("collaborative", &mut out.collaborative, errs);
                f.decode("external", &mut out.external, errs);
                f.decode("replacement", &mut out.replacement, errs);
                f.decode("ranker", &mut out.ranker, errs);
                f.decode("fallback", &mut out.fallback, errs);
            },
        );
    }
}

impl Decode for DataSourceConfig {
    fn go_type() -> String {
        "config.DataSourceConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.DataSourceConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode(
                    "positive_feedback_types",
                    &mut out.positive_feedback_types,
                    errs,
                );
                f.decode(
                    "negative_feedback_types",
                    &mut out.negative_feedback_types,
                    errs,
                );
                f.decode("read_feedback_types", &mut out.read_feedback_types, errs);
                f.decode(
                    "positive_feedback_ttl",
                    &mut out.positive_feedback_ttl,
                    errs,
                );
                f.decode("item_ttl", &mut out.item_ttl, errs);
            },
        );
    }
}

impl Decode for SearchConfig {
    fn go_type() -> String {
        "config.SearchConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.SearchConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("columns", &mut out.columns, errs);
            },
        );
    }
}

impl Decode for NonPersonalizedConfig {
    fn go_type() -> String {
        "config.NonPersonalizedConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.NonPersonalizedConfig",
            input,
            errs,
            decode_non_personalized,
        );
    }
}

fn decode_non_personalized(out: &mut NonPersonalizedConfig, f: &Fields<'_>, errs: &mut Errors) {
    f.decode("name", &mut out.name, errs);
    f.decode("score", &mut out.score, errs);
    f.decode("filter", &mut out.filter, errs);
}

impl Decode for ItemToItemConfig {
    fn go_type() -> String {
        "config.ItemToItemConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.ItemToItemConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("name", &mut out.name, errs);
                f.decode("type", &mut out.kind, errs);
                f.decode("column", &mut out.column, errs);
            },
        );
    }
}

impl Decode for UserToUserConfig {
    fn go_type() -> String {
        "config.UserToUserConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.UserToUserConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("name", &mut out.name, errs);
                f.decode("type", &mut out.kind, errs);
                f.decode("column", &mut out.column, errs);
            },
        );
    }
}

impl Decode for CollaborativeConfig {
    fn go_type() -> String {
        "config.CollaborativeConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.CollaborativeConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("type", &mut out.kind, errs);
                f.decode("fit_period", &mut out.fit_period, errs);
                f.decode("fit_epoch", &mut out.fit_epoch, errs);
                f.decode("optimize_period", &mut out.optimize_period, errs);
                f.decode("optimize_trials", &mut out.optimize_trials, errs);
                f.decode("early_stopping", &mut out.early_stopping, errs);
            },
        );
    }
}

impl Decode for EarlyStoppingConfig {
    fn go_type() -> String {
        "config.EarlyStoppingConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.EarlyStoppingConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("patience", &mut out.patience, errs);
            },
        );
    }
}

impl Decode for ExternalConfig {
    fn go_type() -> String {
        "config.ExternalConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.ExternalConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("name", &mut out.name, errs);
                f.decode("script", &mut out.script, errs);
            },
        );
    }
}

impl Decode for ReplacementConfig {
    fn go_type() -> String {
        "config.ReplacementConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.ReplacementConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("enable_replacement", &mut out.enable_replacement, errs);
                f.decode(
                    "positive_replacement_decay",
                    &mut out.positive_replacement_decay,
                    errs,
                );
                f.decode(
                    "read_replacement_decay",
                    &mut out.read_replacement_decay,
                    errs,
                );
            },
        );
    }
}

impl Decode for RankerConfig {
    fn go_type() -> String {
        "config.RankerConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.RankerConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("type", &mut out.kind, errs);
                f.decode("recommenders", &mut out.recommenders, errs);
                f.decode("cache_expire", &mut out.cache_expire, errs);
                f.decode("fit_period", &mut out.fit_period, errs);
                f.decode("fit_epoch", &mut out.fit_epoch, errs);
                f.decode("optimize_period", &mut out.optimize_period, errs);
                f.decode("optimize_trials", &mut out.optimize_trials, errs);
                f.decode("early_stopping", &mut out.early_stopping, errs);
                f.decode("reranker_api", &mut out.reranker_api, errs);
                f.decode("decision_api", &mut out.decision_api, errs);
            },
        );
    }
}

impl Decode for FallbackConfig {
    fn go_type() -> String {
        "config.FallbackConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.FallbackConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("recommenders", &mut out.recommenders, errs);
            },
        );
    }
}

impl Decode for TracingConfig {
    fn go_type() -> String {
        "config.TracingConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.TracingConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("enable_tracing", &mut out.enable_tracing, errs);
                f.decode("exporter", &mut out.exporter, errs);
                f.decode("collector_endpoint", &mut out.collector_endpoint, errs);
                f.decode("sampler", &mut out.sampler, errs);
                f.decode("ratio", &mut out.ratio, errs);
            },
        );
    }
}

impl Decode for OIDCConfig {
    fn go_type() -> String {
        "config.OIDCConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.OIDCConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("enable", &mut out.enable, errs);
                f.decode("issuer", &mut out.issuer, errs);
                f.decode("client_id", &mut out.client_id, errs);
                f.decode("client_secret", &mut out.client_secret, errs);
                f.decode("redirect_url", &mut out.redirect_url, errs);
            },
        );
    }
}

impl Decode for DecisionAPIConfig {
    fn go_type() -> String {
        "config.DecisionAPIConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.DecisionAPIConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("state_template", &mut out.state_template, errs);
                f.decode(
                    "instructions_template",
                    &mut out.instructions_template,
                    errs,
                );
                f.decode("auth_token", &mut out.auth_token, errs);
                f.decode("model", &mut out.model, errs);
                f.decode("url", &mut out.url, errs);
            },
        );
    }
}

impl Decode for RerankerAPIConfig {
    fn go_type() -> String {
        "config.RerankerAPIConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.RerankerAPIConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("query_template", &mut out.query_template, errs);
                f.decode("document_template", &mut out.document_template, errs);
                f.decode("auth_token", &mut out.auth_token, errs);
                f.decode("model", &mut out.model, errs);
                f.decode("url", &mut out.url, errs);
            },
        );
    }
}

impl Decode for OpenAIConfig {
    fn go_type() -> String {
        "config.OpenAIConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.OpenAIConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("base_url", &mut out.base_url, errs);
                f.decode("auth_token", &mut out.auth_token, errs);
                f.decode(
                    "chat_completion_model",
                    &mut out.chat_completion_model,
                    errs,
                );
                f.decode("chat_completion_rpm", &mut out.chat_completion_rpm, errs);
                f.decode("chat_completion_tpm", &mut out.chat_completion_tpm, errs);
                f.decode("embedding_model", &mut out.embedding_model, errs);
                f.decode("embedding_dimensions", &mut out.embedding_dimensions, errs);
                f.decode("embedding_rpm", &mut out.embedding_rpm, errs);
                f.decode("embedding_tpm", &mut out.embedding_tpm, errs);
                f.decode("log_file", &mut out.log_file, errs);
            },
        );
    }
}

impl Decode for BlobConfig {
    fn go_type() -> String {
        "config.BlobConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.BlobConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("uri", &mut out.uri, errs);
                f.decode("s3", &mut out.s3, errs);
                f.decode("gcs", &mut out.gcs, errs);
                f.decode("azure", &mut out.azure, errs);
            },
        );
    }
}

impl Decode for S3Config {
    fn go_type() -> String {
        "config.S3Config".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.S3Config",
            input,
            errs,
            |out, f, errs| {
                f.decode("endpoint", &mut out.endpoint, errs);
                f.decode("access_key_id", &mut out.access_key_id, errs);
                f.decode("secret_access_key", &mut out.secret_access_key, errs);
            },
        );
    }
}

impl Decode for GCSConfig {
    fn go_type() -> String {
        "config.GCSConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.GCSConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("credentials_file", &mut out.credentials_file, errs);
            },
        );
    }
}

impl Decode for AzureBlobConfig {
    fn go_type() -> String {
        "config.AzureBlobConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.AzureBlobConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("endpoint", &mut out.endpoint, errs);
                f.decode("account_name", &mut out.account_name, errs);
                f.decode("account_key", &mut out.account_key, errs);
                f.decode("connection_string", &mut out.connection_string, errs);
            },
        );
    }
}

impl Decode for QuotaConfig {
    fn go_type() -> String {
        "config.QuotaConfig".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_struct(
            self,
            name,
            "config.QuotaConfig",
            input,
            errs,
            |out, f, errs| {
                f.decode("max_users_count", &mut out.max_users_count, errs);
                f.decode("max_items_count", &mut out.max_items_count, errs);
                f.decode("max_labels_size", &mut out.max_labels_size, errs);
                f.decode("max_comment_size", &mut out.max_comment_size, errs);
                f.decode("max_categories_count", &mut out.max_categories_count, errs);
                f.decode("max_categories_size", &mut out.max_categories_size, errs);
            },
        );
    }
}
