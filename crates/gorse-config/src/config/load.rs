// Ports: config/config.go (LoadConfig, the bindings table, Viper's Unmarshal call) @ add21b5

//! `LoadConfig`: the defaults, the environment variables, the configuration
//! file, the decode into [`Config`], validation and the table prefixes.

use std::fs;
use std::io;

use super::defaults::set_default;
use super::schema::Config;
use super::validate::ValidateError;
use crate::log;
use crate::mapstructure::{self, Errors};
use crate::value::{Map, Value};
use crate::viper::{ReadError, Viper, path_error};

/// `bindings`: each viper key and the environment variable bound to it.
pub(crate) const BINDINGS: &[(&str, &str)] = &[
    ("database.cache_store", "GORSE_CACHE_STORE"),
    ("database.data_store", "GORSE_DATA_STORE"),
    ("database.vector_store", "GORSE_VECTOR_STORE"),
    ("database.table_prefix", "GORSE_TABLE_PREFIX"),
    ("database.cache_table_prefix", "GORSE_CACHE_TABLE_PREFIX"),
    ("database.cache_client_name", "GORSE_CACHE_CLIENT_NAME"),
    ("database.data_table_prefix", "GORSE_DATA_TABLE_PREFIX"),
    ("database.vector_table_prefix", "GORSE_VECTOR_TABLE_PREFIX"),
    ("database.vector.quantization_type", "GORSE_VECTOR_QUANTIZATION_TYPE"),
    ("database.vector.quantization_bits", "GORSE_VECTOR_QUANTIZATION_BITS"),
    ("master.port", "GORSE_MASTER_PORT"),
    ("master.host", "GORSE_MASTER_HOST"),
    ("master.ssl_mode", "GORSE_MASTER_SSL_MODE"),
    ("master.ssl_ca", "GORSE_MASTER_SSL_CA"),
    ("master.ssl_cert", "GORSE_MASTER_SSL_CERT"),
    ("master.ssl_key", "GORSE_MASTER_SSL_KEY"),
    ("master.http_port", "GORSE_MASTER_HTTP_PORT"),
    ("master.http_host", "GORSE_MASTER_HTTP_HOST"),
    ("master.n_jobs", "GORSE_MASTER_JOBS"),
    ("master.dashboard_user_name", "GORSE_DASHBOARD_USER_NAME"),
    ("master.dashboard_password", "GORSE_DASHBOARD_PASSWORD"),
    ("master.dashboard_auth_server", "GORSE_DASHBOARD_AUTH_SERVER"),
    ("master.dashboard_redacted", "GORSE_DASHBOARD_REDACTED"),
    ("master.admin_api_key", "GORSE_ADMIN_API_KEY"),
    ("server.api_key", "GORSE_SERVER_API_KEY"),
    ("oidc.enable", "GORSE_OIDC_ENABLE"),
    ("oidc.issuer", "GORSE_OIDC_ISSUER"),
    ("oidc.client_id", "GORSE_OIDC_CLIENT_ID"),
    ("oidc.client_secret", "GORSE_OIDC_CLIENT_SECRET"),
    ("oidc.redirect_url", "GORSE_OIDC_REDIRECT_URL"),
    ("blob.uri", "GORSE_BLOB_URI"),
    ("blob.s3.endpoint", "S3_ENDPOINT"),
    ("blob.s3.access_key_id", "S3_ACCESS_KEY_ID"),
    ("blob.s3.secret_access_key", "S3_SECRET_ACCESS_KEY"),
    ("blob.gcs.credentials_file", "GCS_CREDENTIALS_FILE"),
    ("blob.azure.endpoint", "AZURE_STORAGE_ENDPOINT"),
    ("blob.azure.account_name", "AZURE_STORAGE_ACCOUNT_NAME"),
    ("blob.azure.account_key", "AZURE_STORAGE_ACCOUNT_KEY"),
    ("blob.azure.connection_string", "AZURE_STORAGE_CONNECTION_STRING"),
    ("openai.base_url", "OPENAI_BASE_URL"),
    ("openai.auth_token", "OPENAI_AUTH_TOKEN"),
    ("openai.chat_completion_model", "OPENAI_CHAT_COMPLETION_MODEL"),
    ("recommend.ranker.reranker_api.url", "RERANKER_URL"),
    ("recommend.ranker.reranker_api.model", "RERANKER_MODEL"),
    ("recommend.ranker.reranker_api.auth_token", "RERANKER_AUTH_TOKEN"),
    ("recommend.ranker.decision_api.url", "DECISION_URL"),
    ("recommend.ranker.decision_api.model", "DECISION_MODEL"),
    ("recommend.ranker.decision_api.auth_token", "DECISION_AUTH_TOKEN"),
    ("quota.max_users_count", "GORSE_QUOTA_MAX_USERS_COUNT"),
    ("quota.max_items_count", "GORSE_QUOTA_MAX_ITEMS_COUNT"),
    ("quota.max_labels_size", "GORSE_QUOTA_MAX_LABELS_SIZE"),
    ("quota.max_comment_size", "GORSE_QUOTA_MAX_COMMENT_SIZE"),
    ("quota.max_categories_count", "GORSE_QUOTA_MAX_CATEGORIES_COUNT"),
    ("quota.max_categories_size", "GORSE_QUOTA_MAX_CATEGORIES_SIZE"),
];

/// An error from [`load_config`].
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// `os.Stat` failed for a reason other than a missing file, with Go's
    /// `*os.PathError` text.
    #[error("{0}")]
    Stat(String),
    /// viper could not read the file: an unsupported extension, a read error
    /// or a decoder's parse error.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// `viper.Unmarshal` failed.
    #[error("{0}")]
    Unmarshal(String),
    /// [`Config::validate`] failed.
    #[error(transparent)]
    Validate(#[from] ValidateError),
}

/// LoadConfig loads the configuration from a TOML, YAML, JSON or dotenv
/// file, with the `GORSE_*` environment variables over it. An empty `path`
/// loads the defaults and the environment; a missing file loads them too.
pub fn load_config(path: &str) -> Result<Config, LoadError> {
    load_config_with_env(path, &|key| {
        std::env::var_os(key).map(|v| v.to_string_lossy().into_owned())
    })
}

/// [`load_config`] with the environment variables read by `lookup_env` (Go's
/// `os.LookupEnv`), so a caller can give its own.
pub fn load_config_with_env(
    path: &str,
    lookup_env: &dyn Fn(&str) -> Option<String>,
) -> Result<Config, LoadError> {
    let mut viper = Viper::new(lookup_env);
    // set default config
    set_default(&mut viper);

    // bind environment bindings
    for (key, env) in BINDINGS {
        viper.bind_env(key, env);
    }

    // load config file if provided
    if path.is_empty() {
        log::info("no config file provided, use defaults and environment variables");
    } else {
        match fs::metadata(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                log::warn(&format!("config file not found, use default config path={path}"));
            }
            Err(err) => return Err(LoadError::Stat(path_error("stat", path, &err))),
            Ok(_) => viper.read_in_config(path)?,
        }
    }

    // unmarshal config file
    let mut conf = unmarshal(&viper.all_settings()).map_err(LoadError::Unmarshal)?;

    // validate config file
    conf.validate()?;

    // apply table prefix
    if conf.database.cache_table_prefix.is_empty() {
        conf.database.cache_table_prefix.clone_from(&conf.database.table_prefix);
    }
    if conf.database.data_table_prefix.is_empty() {
        conf.database.data_table_prefix.clone_from(&conf.database.table_prefix);
    }
    Ok(conf)
}

/// `viper.Unmarshal` into a [`Config`]: the settings decoded with
/// mapstructure's weakly typed rules. The error text is mapstructure's.
pub fn unmarshal(settings: &Map) -> Result<Config, String> {
    let mut conf = Config::default();
    let mut errs: Errors = Vec::new();
    mapstructure::decode("", &Value::Map(settings.clone()), &mut conf, &mut errs);
    if errs.is_empty() {
        Ok(conf)
    } else {
        Err(mapstructure::error_text(&errs))
    }
}
