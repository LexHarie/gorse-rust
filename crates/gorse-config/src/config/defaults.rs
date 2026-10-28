// Ports: config/config.go (GetDefaultConfig, MkDir, RootDir, setDefault) @ add21b5

//! The default configuration, the directories it creates, and the defaults
//! viper is given before it reads the file.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use gorse_common::gotime::Duration;
use gorse_storage_core::scheme::XVEC_PREFIX;

use super::schema::*;
use crate::value::{Map, Value};
use crate::viper::Viper;

/// Go's `RootDir`: the directory [`mk_dir`] creates under. Set on first use
/// to `$HOME/.gorse/var/lib`, unless [`set_root_dir`] set it first.
static ROOT_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets the directory [`mk_dir`] creates under, as assigning Go's `RootDir`
/// does. The tests use it to keep the directories out of `$HOME`.
pub fn set_root_dir(dir: impl Into<PathBuf>) {
    *ROOT_DIR.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir.into());
}

fn root_dir() -> PathBuf {
    let mut root = ROOT_DIR.lock().unwrap_or_else(PoisonError::into_inner);
    root.get_or_insert_with(|| {
        // `os.UserHomeDir` on Linux: `$HOME`, which must be set.
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .expect("$HOME is not defined");
        PathBuf::from(home).join(".gorse").join("var").join("lib")
    })
    .clone()
}

/// MkDir creates a directory under the Gorse home directory and returns its
/// path. Panics if the directory cannot be created, as Go's `lo.Must0` does.
pub fn mk_dir(elem: &[&str]) -> PathBuf {
    let path = elem.iter().fold(root_dir(), |path, e| path.join(e));
    fs::create_dir_all(&path).expect("create a Gorse directory");
    path
}

/// GetDefaultConfig: the configuration with every default Gorse sets.
pub fn get_default_config() -> Config {
    Config {
        database: DatabaseConfig {
            data_store: format!("sqlite://{}", mk_dir(&[]).join("data.sqlite").display()),
            cache_store: format!("sqlite://{}", mk_dir(&[]).join("cache.sqlite").display()),
            vector_store: format!("{XVEC_PREFIX}{}", mk_dir(&[]).join("vectors").display()),
            cache_client_name: "gorse_cache_client".into(),
            mysql: MySQLConfig {
                isolation_level: "READ-UNCOMMITTED".into(),
                max_open_conns: 0,
                max_idle_conns: 0,
                conn_max_lifetime: Duration(0),
            },
            postgres: SQLConfig {
                max_open_conns: 64,
                max_idle_conns: 64,
                conn_max_lifetime: minute(),
            },
            redis: RedisConfig {
                max_search_results: 10000,
            },
            vector: VectorConfig {
                quantization_type: String::new(),
                quantization_bits: 0,
            },
            ..Default::default()
        },
        master: MasterConfig {
            port: 8086,
            host: "0.0.0.0".into(),
            http_port: 8088,
            http_host: "0.0.0.0".into(),
            http_cors_domains: Some(vec![".*".into()]),
            http_cors_methods: Some(
                ["GET", "POST", "PUT", "DELETE", "PATCH"]
                    .map(String::from)
                    .to_vec(),
            ),
            n_jobs: 1,
            meta_timeout: seconds(10),
            ..Default::default()
        },
        server: ServerConfig {
            default_n: 10,
            clock_error: seconds(5),
            auto_insert_user: true,
            auto_insert_item: true,
            cache_expire: seconds(10),
            ..Default::default()
        },
        recommend: RecommendConfig {
            cache_size: 100,
            cache_expire: hours(72),
            context_size: 100,
            collaborative: CollaborativeConfig {
                kind: "none".into(),
                fit_period: minutes(60),
                fit_epoch: 100,
                optimize_period: Duration(0),
                optimize_trials: 10,
                ..Default::default()
            },
            replacement: ReplacementConfig {
                enable_replacement: false,
                positive_replacement_decay: 0.8,
                read_replacement_decay: 0.6,
            },
            ranker: RankerConfig {
                kind: "none".into(),
                cache_expire: hours(120),
                fit_period: minutes(60),
                fit_epoch: 100,
                optimize_period: Duration(0),
                optimize_trials: 10,
                recommenders: Some(vec!["latest".into()]),
                ..Default::default()
            },
            ..Default::default()
        },
        tracing: TracingConfig {
            exporter: "otlp".into(),
            sampler: "always".into(),
            ..Default::default()
        },
        blob: BlobConfig {
            uri: mk_dir(&["blob"]).display().to_string(),
            ..Default::default()
        },
        oidc: OIDCConfig::default(),
        openai: OpenAIConfig::default(),
        quota: QuotaConfig::default(),
    }
}

fn seconds(n: i64) -> Duration {
    Duration(n * 1_000_000_000)
}

fn minutes(n: i64) -> Duration {
    Duration(n * 60_000_000_000)
}

fn hours(n: i64) -> Duration {
    Duration(n * 3_600_000_000_000)
}

fn minute() -> Duration {
    minutes(1)
}

/// The defaults as viper holds them: `setDefault` in Go. Each key takes the
/// Go value's type, so a value of another type in the file decodes as Go's
/// would.
pub fn set_default(viper: &mut Viper<'_>) {
    let d = get_default_config();
    // [database]
    viper.set_default("database.data_store", string(&d.database.data_store));
    viper.set_default("database.cache_store", string(&d.database.cache_store));
    viper.set_default("database.vector_store", string(&d.database.vector_store));
    viper.set_default("database.table_prefix", string(&d.database.table_prefix));
    viper.set_default("database.data_table_prefix", string(&d.database.data_table_prefix));
    viper.set_default("database.cache_table_prefix", string(&d.database.cache_table_prefix));
    viper.set_default("database.vector_table_prefix", string(&d.database.vector_table_prefix));
    viper.set_default("database.cache_client_name", string(&d.database.cache_client_name));
    // [database.mysql]
    viper.set_default("database.mysql.isolation_level", string(&d.database.mysql.isolation_level));
    viper.set_default("database.mysql.max_open_conns", int(d.database.mysql.max_open_conns));
    viper.set_default("database.mysql.max_idle_conns", int(d.database.mysql.max_idle_conns));
    viper.set_default("database.mysql.conn_max_lifetime", duration(d.database.mysql.conn_max_lifetime));
    // [database.postgres]
    viper.set_default("database.postgres.max_open_conns", int(d.database.postgres.max_open_conns));
    viper.set_default("database.postgres.max_idle_conns", int(d.database.postgres.max_idle_conns));
    viper.set_default("database.postgres.conn_max_lifetime", duration(d.database.postgres.conn_max_lifetime));
    // [database.redis]
    viper.set_default("database.redis.max_search_results", int(d.database.redis.max_search_results));
    // [database.vector]
    viper.set_default("database.vector.quantization_type", string(&d.database.vector.quantization_type));
    viper.set_default("database.vector.quantization_bits", int(d.database.vector.quantization_bits));
    // [master]
    viper.set_default("master.port", int(d.master.port));
    viper.set_default("master.host", string(&d.master.host));
    viper.set_default("master.http_port", int(d.master.http_port));
    viper.set_default("master.http_host", string(&d.master.http_host));
    viper.set_default("master.http_cors_domains", strings(&d.master.http_cors_domains));
    viper.set_default("master.http_cors_methods", strings(&d.master.http_cors_methods));
    viper.set_default("master.n_jobs", int(d.master.n_jobs));
    viper.set_default("master.meta_timeout", duration(d.master.meta_timeout));
    // [server]
    viper.set_default("server.api_key", string(&d.server.api_key));
    viper.set_default("server.default_n", int(d.server.default_n));
    viper.set_default("server.clock_error", duration(d.server.clock_error));
    viper.set_default("server.auto_insert_user", Value::Bool(d.server.auto_insert_user));
    viper.set_default("server.auto_insert_item", Value::Bool(d.server.auto_insert_item));
    viper.set_default("server.cache_expire", duration(d.server.cache_expire));
    // [recommend]
    viper.set_default("recommend.cache_size", int(d.recommend.cache_size));
    viper.set_default("recommend.cache_expire", duration(d.recommend.cache_expire));
    viper.set_default("recommend.context_size", int(d.recommend.context_size));
    // [recommend.collaborative]
    viper.set_default("recommend.collaborative.type", string(&d.recommend.collaborative.kind));
    viper.set_default("recommend.collaborative.fit_period", duration(d.recommend.collaborative.fit_period));
    viper.set_default("recommend.collaborative.fit_epoch", int(d.recommend.collaborative.fit_epoch));
    viper.set_default("recommend.collaborative.optimize_period", duration(d.recommend.collaborative.optimize_period));
    viper.set_default("recommend.collaborative.optimize_trials", int(d.recommend.collaborative.optimize_trials));
    // [recommend.replacement]
    viper.set_default("recommend.replacement.enable_replacement", Value::Bool(d.recommend.replacement.enable_replacement));
    viper.set_default("recommend.replacement.positive_replacement_decay", Value::Float64(d.recommend.replacement.positive_replacement_decay));
    viper.set_default("recommend.replacement.read_replacement_decay", Value::Float64(d.recommend.replacement.read_replacement_decay));
    // [recommend.ranker]
    viper.set_default("recommend.ranker.type", string(&d.recommend.ranker.kind));
    viper.set_default("recommend.ranker.cache_expire", duration(d.recommend.ranker.cache_expire));
    viper.set_default("recommend.ranker.fit_period", duration(d.recommend.ranker.fit_period));
    viper.set_default("recommend.ranker.fit_epoch", int(d.recommend.ranker.fit_epoch));
    viper.set_default("recommend.ranker.optimize_period", duration(d.recommend.ranker.optimize_period));
    viper.set_default("recommend.ranker.optimize_trials", int(d.recommend.ranker.optimize_trials));
    viper.set_default("recommend.ranker.recommenders", strings(&d.recommend.ranker.recommenders));
    // [recommend.fallback]
    viper.set_default(
        "recommend.fallback",
        Value::Struct(
            "config.FallbackConfig",
            Map::from(vec![(
                "recommenders".to_owned(),
                strings(&d.recommend.fallback.recommenders),
            )]),
        ),
    );
    // [tracing]
    viper.set_default("tracing.exporter", string(&d.tracing.exporter));
    viper.set_default("tracing.sampler", string(&d.tracing.sampler));
    // [blob]
    viper.set_default("blob.uri", string(&d.blob.uri));
    // [quota]
    viper.set_default("quota.max_users_count", int(d.quota.max_users_count));
    viper.set_default("quota.max_items_count", int(d.quota.max_items_count));
    viper.set_default("quota.max_labels_size", int(d.quota.max_labels_size));
    viper.set_default("quota.max_comment_size", int(d.quota.max_comment_size));
    viper.set_default("quota.max_categories_count", int(d.quota.max_categories_count));
    viper.set_default("quota.max_categories_size", int(d.quota.max_categories_size));
}

fn string(s: &str) -> Value {
    Value::String(s.to_owned())
}

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn duration(d: Duration) -> Value {
    Value::Duration(d.0)
}

fn strings(s: &Option<Vec<String>>) -> Value {
    Value::Strings(s.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_helpers_match_go_units() {
        assert_eq!(seconds(5), Duration(5_000_000_000));
        assert_eq!(hours(72), Duration(259_200_000_000_000));
        assert_eq!(minute(), Duration(60_000_000_000));
    }
}
