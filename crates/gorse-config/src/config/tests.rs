// Ports: config/config_test.go @ add21b5

//! The Go package's tests, case for case. They sit inside the crate because
//! the Go tests use the package's unexported `setDefault`, `bindings` and
//! `ConfigTOML`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gorse_common::expression::must_parse_feedback_type_expression as expr;
use gorse_common::gotime::Duration;

use super::CONFIG_TOML;
use super::defaults::{get_default_config, mk_dir, set_default, set_root_dir};
use super::load::{BINDINGS, load_config_with_env, unmarshal};
use super::schema::*;
use crate::viper::Viper;

const SECOND: i64 = 1_000_000_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;

/// A directory under the system temporary directory, unique to this process
/// and call, removed with the test process's temporary files.
fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gorse-config-{}-{n}-{name}", std::process::id()));
    fs::create_dir_all(&dir).expect("create a temporary directory");
    dir
}

/// Points `MkDir` at one temporary directory for the whole test binary, as the
/// Go tests' `$HOME` does. The root is process-wide, so every test sets the same
/// value; a per-test root would change it under a test that runs in parallel.
fn isolate_root() {
    static ROOT: std::sync::Once = std::sync::Once::new();
    ROOT.call_once(|| set_root_dir(temp_dir("root").join(".gorse").join("var").join("lib")));
}

/// `os.LookupEnv` over a fixed set of variables.
fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |k| map.get(k).cloned()
}

fn no_env(_: &str) -> Option<String> {
    None
}

/// The sample's text with the replacements `TestUnmarshal` makes.
fn unmarshal_text() -> String {
    let replacements = [
        ("ssl_mode = false", "ssl_mode = true"),
        ("ssl_ca = \"\"", "ssl_ca = \"ca.pem\""),
        ("ssl_cert = \"\"", "ssl_cert = \"cert.pem\""),
        ("ssl_key = \"\"", "ssl_key = \"key.pem\""),
        (
            "dashboard_user_name = \"\"",
            "dashboard_user_name = \"admin\"",
        ),
        (
            "dashboard_password = \"\"",
            "dashboard_password = \"password\"",
        ),
        ("admin_api_key = \"\"", "admin_api_key = \"super_api_key\""),
        ("api_key = \"\"", "api_key = \"19260817\""),
        ("table_prefix = \"\"", "table_prefix = \"gorse_\""),
        (
            "cache_table_prefix = \"gorse_\"",
            "cache_table_prefix = \"gorse_cache_\"",
        ),
        (
            "data_table_prefix = \"gorse_\"",
            "data_table_prefix = \"gorse_data_\"",
        ),
        ("http_cors_domains = []", "http_cors_domains = [\".*\"]"),
        (
            "http_cors_methods = []",
            "http_cors_methods = [\"GET\",\"PATCH\",\"POST\"]",
        ),
        ("issuer = \"\"", "issuer = \"https://accounts.google.com\""),
        ("client_id = \"\"", "client_id = \"client_id\""),
        ("client_secret = \"\"", "client_secret = \"client_secret\""),
        (
            "redirect_url = \"\"",
            "redirect_url = \"http://localhost:8088/callback/oauth2\"",
        ),
        (
            "auth_token = \"\"",
            "auth_token = \"<reranker_auth_token>\"",
        ),
        (
            "url = \"https://dashscope.aliyuncs.com/compatible-api/v1/reranks\"",
            "url = \"<reranker_url>\"",
        ),
    ];
    replacements
        .iter()
        .fold(CONFIG_TOML.to_owned(), |text, (from, to)| {
            text.replace(from, to)
        })
}

/// The assertions `TestUnmarshal` makes on a config read from its file.
fn assert_unmarshal_config(config: &Config) {
    // [database]
    assert_eq!(config.database.cache_store, "redis://localhost:6379/0");
    assert_eq!(
        config.database.data_store,
        "mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse"
    );
    assert_eq!(config.database.vector_store, "xvec://vectors");
    assert_eq!(config.database.table_prefix, "gorse_");
    assert_eq!(config.database.cache_table_prefix, "gorse_cache_");
    assert_eq!(config.database.cache_client_name, "gorse_cache_client");
    assert_eq!(config.database.data_table_prefix, "gorse_data_");
    assert_eq!(config.database.mysql.isolation_level, "READ-UNCOMMITTED");
    assert_eq!(config.database.mysql.max_open_conns, 0);
    assert_eq!(config.database.mysql.max_idle_conns, 0);
    assert_eq!(config.database.mysql.conn_max_lifetime, Duration(0));
    assert_eq!(config.database.postgres.max_open_conns, 64);
    assert_eq!(config.database.postgres.max_idle_conns, 64);
    assert_eq!(config.database.postgres.conn_max_lifetime, Duration(MINUTE));
    assert_eq!(config.database.redis.max_search_results, 10000);
    // [master]
    assert_eq!(config.master.port, 8086);
    assert_eq!(config.master.host, "0.0.0.0");
    assert!(config.master.ssl_mode);
    assert_eq!(config.master.ssl_ca, "ca.pem");
    assert_eq!(config.master.ssl_cert, "cert.pem");
    assert_eq!(config.master.ssl_key, "key.pem");
    assert_eq!(config.master.http_port, 8088);
    assert_eq!(config.master.http_host, "0.0.0.0");
    assert_eq!(config.master.http_cors_domains, Some(vec![".*".to_owned()]));
    assert_eq!(
        config.master.http_cors_methods,
        Some(vec![
            "GET".to_owned(),
            "PATCH".to_owned(),
            "POST".to_owned()
        ])
    );
    assert_eq!(config.master.n_jobs, 1);
    assert_eq!(config.master.meta_timeout, Duration(10 * SECOND));
    assert_eq!(config.master.dashboard_user_name, "admin");
    assert_eq!(config.master.dashboard_password, "password");
    assert_eq!(config.master.admin_api_key, "super_api_key");
    // [server]
    assert_eq!(config.server.default_n, 10);
    assert_eq!(config.server.api_key, "19260817");
    assert_eq!(config.server.clock_error, Duration(5 * SECOND));
    assert!(config.server.auto_insert_user);
    assert!(config.server.auto_insert_item);
    assert_eq!(config.server.cache_expire, Duration(10 * SECOND));
    // [recommend]
    assert_eq!(config.recommend.cache_size, 100);
    assert_eq!(config.recommend.cache_expire, Duration(72 * HOUR));
    assert_eq!(config.recommend.context_size, 100);
    // [recommend.data_source]
    assert_eq!(
        config.recommend.data_source.positive_feedback_types,
        Some(vec![expr("star"), expr("like"), expr("read>=3")])
    );
    assert_eq!(
        config.recommend.data_source.read_feedback_types,
        Some(vec![expr("read")])
    );
    assert_eq!(config.recommend.data_source.positive_feedback_ttl, 0);
    assert_eq!(config.recommend.data_source.item_ttl, 0);
    // [recommend.non-personalized]
    let non_personalized = config.recommend.non_personalized.as_ref().expect("one");
    assert_eq!(non_personalized.len(), 1);
    assert_eq!(non_personalized[0].name, "most_starred_weekly");
    assert_eq!(
        non_personalized[0].score,
        "count(feedback, .FeedbackType == 'star')"
    );
    assert_eq!(
        non_personalized[0].filter,
        "(now() - item.Timestamp).Hours() < 168"
    );
    // [recommend.collaborative]
    assert_eq!(config.recommend.collaborative.kind, "mf");
    assert_eq!(
        config.recommend.collaborative.fit_period,
        Duration(60 * MINUTE)
    );
    assert_eq!(config.recommend.collaborative.fit_epoch, 100);
    assert_eq!(
        config.recommend.collaborative.optimize_period,
        Duration(360 * MINUTE)
    );
    assert_eq!(config.recommend.collaborative.optimize_trials, 10);
    // [recommend.replacement]
    assert!(!config.recommend.replacement.enable_replacement);
    assert_eq!(config.recommend.replacement.positive_replacement_decay, 0.8);
    assert_eq!(config.recommend.replacement.read_replacement_decay, 0.6);
    // [recommend.ranker]
    assert_eq!(config.recommend.ranker.kind, "fm");
    assert_eq!(config.recommend.ranker.cache_expire, Duration(120 * HOUR));
    assert_eq!(
        config.recommend.ranker.recommenders,
        Some(vec![
            "latest".to_owned(),
            "collaborative".to_owned(),
            "non-personalized/most_starred_weekly".to_owned(),
            "item-to-item/neighbors".to_owned(),
            "user-to-user/neighbors".to_owned(),
        ])
    );
    assert_eq!(config.recommend.ranker.fit_period, Duration(60 * MINUTE));
    assert_eq!(config.recommend.ranker.fit_epoch, 100);
    assert_eq!(
        config.recommend.ranker.optimize_period,
        Duration(360 * MINUTE)
    );
    assert_eq!(config.recommend.ranker.optimize_trials, 10);
    assert_eq!(
        config.recommend.ranker.reranker_api.auth_token,
        "<reranker_auth_token>"
    );
    assert_eq!(config.recommend.ranker.reranker_api.model, "qwen3-rerank");
    assert_eq!(config.recommend.ranker.reranker_api.url, "<reranker_url>");
    assert!(
        config
            .recommend
            .ranker
            .reranker_api
            .query_template
            .contains("{{ repo.Comment }}")
    );
    assert!(
        config
            .recommend
            .ranker
            .reranker_api
            .document_template
            .contains("{{ item.Comment")
    );
    assert!(
        config
            .recommend
            .ranker
            .decision_api
            .state_template
            .contains("{{ repo.Comment }}")
    );
    assert!(
        config
            .recommend
            .ranker
            .decision_api
            .instructions_template
            .contains("Would the user be interested")
    );
    // [recommend.fallback]
    assert_eq!(
        config.recommend.fallback.recommenders,
        Some(vec![
            "item-to-item/neighbors".to_owned(),
            "latest".to_owned()
        ])
    );
    // [tracing]
    assert!(!config.tracing.enable_tracing);
    assert_eq!(config.tracing.exporter, "otlp");
    assert_eq!(config.tracing.collector_endpoint, "http://localhost:4317");
    assert_eq!(config.tracing.sampler, "always");
    assert_eq!(config.tracing.ratio, 1.0);
    // [oauth2]
    assert_eq!(config.oidc.issuer, "https://accounts.google.com");
    assert_eq!(config.oidc.client_id, "client_id");
    assert_eq!(config.oidc.client_secret, "client_secret");
    assert_eq!(
        config.oidc.redirect_url,
        "http://localhost:8088/callback/oauth2"
    );
    // [openai]
    assert_eq!(config.openai.base_url, "http://localhost:11434/v1");
    assert_eq!(config.openai.auth_token, "ollama");
    assert_eq!(config.openai.chat_completion_model, "qwen2.5");
    assert_eq!(config.openai.chat_completion_rpm, 15000);
    assert_eq!(config.openai.chat_completion_tpm, 1200000);
    assert_eq!(config.openai.embedding_model, "mxbai-embed-large");
    assert_eq!(config.openai.embedding_dimensions, 1024);
    assert_eq!(config.openai.embedding_rpm, 1800);
    assert_eq!(config.openai.embedding_tpm, 1200000);
}

#[test]
fn test_unmarshal_toml() {
    isolate_root();
    let dir = temp_dir("unmarshal");
    let path = dir.join("config.toml");
    fs::write(&path, unmarshal_text()).expect("write the config");
    let config =
        load_config_with_env(path.to_str().expect("utf-8"), &no_env).expect("load the config");
    assert_unmarshal_config(&config);
}

/// The JSON encoding of `TestUnmarshal`, as Go's yj writes it. The text is
/// the golden's `TestUnmarshal json` case, produced by the Go program.
#[test]
fn test_unmarshal_json() {
    isolate_root();
    let text = golden_load_text("TestUnmarshal json");
    let dir = temp_dir("unmarshal-json");
    let path = dir.join("config.json");
    fs::write(&path, text).expect("write the config");
    let config =
        load_config_with_env(path.to_str().expect("utf-8"), &no_env).expect("load the config");
    assert_unmarshal_config(&config);
}

/// The YAML encoding of `TestUnmarshal`; needs the YAML decoder (WP-1-15, the
/// YAML part).
#[test]
#[ignore = "needs WP-1-15 yaml"]
fn test_unmarshal_yaml() {
    isolate_root();
    let text = golden_load_text("TestUnmarshal yaml");
    let dir = temp_dir("unmarshal-yaml");
    let path = dir.join("config.yaml");
    fs::write(&path, text).expect("write the config");
    let config =
        load_config_with_env(path.to_str().expect("utf-8"), &no_env).expect("load the config");
    assert_unmarshal_config(&config);
}

/// The text of a load case in the golden file, as the Go program wrote it.
fn golden_load_text(name: &str) -> String {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/data/config_golden.json"))
            .expect("parse the golden file");
    golden["load"]
        .as_array()
        .expect("load cases")
        .iter()
        .find(|case| case["name"] == name)
        .and_then(|case| case["text"].as_str())
        .unwrap_or_else(|| panic!("golden case {name} has no text"))
        .to_owned()
}

#[test]
fn test_reload_config() {
    isolate_root();
    let dir = temp_dir("reload");
    let first = dir.join("first.toml");
    let second = dir.join("second.toml");
    fs::write(&first, "[server]\ndefault_n = 42\n").expect("write");
    fs::write(&second, "").expect("write");

    let first = load_config_with_env(first.to_str().expect("utf-8"), &no_env)
        .expect("load the first config");
    assert_eq!(first.server.default_n, 42);

    let second = load_config_with_env(second.to_str().expect("utf-8"), &no_env)
        .expect("load the second config");
    assert_eq!(
        second.server.default_n,
        get_default_config().server.default_n
    );
}

#[test]
fn test_set_default() {
    isolate_root();
    let mut viper = Viper::new(&no_env);
    set_default(&mut viper);
    let config = unmarshal(&viper.all_settings()).expect("unmarshal the defaults");
    assert_eq!(config, get_default_config());
}

#[test]
fn test_bind_env() {
    isolate_root();
    let variables: &[(&str, &str)] = &[
        ("GORSE_CACHE_STORE", "redis://<cache_store>"),
        ("GORSE_DATA_STORE", "mysql://<data_store>"),
        ("GORSE_VECTOR_STORE", "qdrant://<vector_store>"),
        ("GORSE_TABLE_PREFIX", "gorse_"),
        ("GORSE_DATA_TABLE_PREFIX", "gorse_data_"),
        ("GORSE_CACHE_TABLE_PREFIX", "gorse_cache_"),
        ("GORSE_CACHE_CLIENT_NAME", "gorse_cache_client_from_env"),
        ("GORSE_VECTOR_TABLE_PREFIX", "gorse_vector_"),
        ("GORSE_VECTOR_QUANTIZATION_TYPE", "rq"),
        ("GORSE_VECTOR_QUANTIZATION_BITS", "8"),
        ("GORSE_MASTER_PORT", "123"),
        ("GORSE_MASTER_HOST", "<master_host>"),
        ("GORSE_MASTER_SSL_MODE", "true"),
        ("GORSE_MASTER_SSL_CA", "ca.pem"),
        ("GORSE_MASTER_SSL_CERT", "cert.pem"),
        ("GORSE_MASTER_SSL_KEY", "key.pem"),
        ("GORSE_MASTER_HTTP_PORT", "456"),
        ("GORSE_MASTER_HTTP_HOST", "<master_http_host>"),
        ("GORSE_MASTER_JOBS", "789"),
        ("GORSE_DASHBOARD_USER_NAME", "user_name"),
        ("GORSE_DASHBOARD_PASSWORD", "password"),
        ("GORSE_DASHBOARD_AUTH_SERVER", "http://127.0.0.1:8888"),
        ("GORSE_DASHBOARD_REDACTED", "true"),
        ("GORSE_ADMIN_API_KEY", "<admin_api_key>"),
        ("GORSE_SERVER_API_KEY", "<server_api_key>"),
        ("GORSE_OIDC_ENABLE", "true"),
        ("GORSE_OIDC_ISSUER", "https://accounts.google.com"),
        ("GORSE_OIDC_CLIENT_ID", "client_id"),
        ("GORSE_OIDC_CLIENT_SECRET", "client_secret"),
        (
            "GORSE_OIDC_REDIRECT_URL",
            "http://localhost:8088/callback/oauth2",
        ),
        ("GORSE_BLOB_URI", "s3://<bucket>/path"),
        ("S3_ENDPOINT", "https://s3.example.com"),
        ("S3_ACCESS_KEY_ID", "<access_key_id>"),
        ("S3_SECRET_ACCESS_KEY", "<secret_access_key>"),
        ("GCS_CREDENTIALS_FILE", "/path/to/credentials.json"),
        (
            "AZURE_STORAGE_ENDPOINT",
            "https://<account>.blob.core.windows.net",
        ),
        ("AZURE_STORAGE_ACCOUNT_NAME", "<account_name>"),
        ("AZURE_STORAGE_ACCOUNT_KEY", "<account_key>"),
        (
            "AZURE_STORAGE_CONNECTION_STRING",
            "DefaultEndpointsProtocol=https;AccountName=<account>;AccountKey=<key>",
        ),
        ("OPENAI_BASE_URL", "https://api.openai.com/v1"),
        ("OPENAI_AUTH_TOKEN", "<auth_token>"),
        ("OPENAI_CHAT_COMPLETION_MODEL", "gpt-4"),
        ("RERANKER_AUTH_TOKEN", "<reranker_auth_token>"),
        ("RERANKER_URL", "<reranker_url>"),
        ("RERANKER_MODEL", "<reranker_model>"),
        ("DECISION_AUTH_TOKEN", "<decision_auth_token>"),
        ("DECISION_URL", "<decision_url>"),
        ("DECISION_MODEL", "<decision_model>"),
        ("GORSE_QUOTA_MAX_USERS_COUNT", "9"),
        ("GORSE_QUOTA_MAX_ITEMS_COUNT", "10"),
        ("GORSE_QUOTA_MAX_LABELS_SIZE", "11"),
        ("GORSE_QUOTA_MAX_COMMENT_SIZE", "12"),
        ("GORSE_QUOTA_MAX_CATEGORIES_COUNT", "13"),
        ("GORSE_QUOTA_MAX_CATEGORIES_SIZE", "14"),
    ];
    let dir = temp_dir("bind-env");
    let path = dir.join("config.toml");
    fs::write(&path, CONFIG_TOML).expect("write the config");
    let config = load_config_with_env(path.to_str().expect("utf-8"), &env_of(variables))
        .expect("load the config");

    assert_eq!(config.database.cache_store, "redis://<cache_store>");
    assert_eq!(config.database.data_store, "mysql://<data_store>");
    assert_eq!(config.database.vector_store, "qdrant://<vector_store>");
    assert_eq!(config.database.table_prefix, "gorse_");
    assert_eq!(config.database.cache_table_prefix, "gorse_cache_");
    assert_eq!(
        config.database.cache_client_name,
        "gorse_cache_client_from_env"
    );
    assert_eq!(config.database.data_table_prefix, "gorse_data_");
    assert_eq!(config.database.vector_table_prefix, "gorse_vector_");
    assert_eq!(config.database.vector.quantization_type, "rq");
    assert_eq!(config.database.vector.quantization_bits, 8);
    assert_eq!(config.master.port, 123);
    assert_eq!(config.master.host, "<master_host>");
    assert!(config.master.ssl_mode);
    assert_eq!(config.master.ssl_ca, "ca.pem");
    assert_eq!(config.master.ssl_cert, "cert.pem");
    assert_eq!(config.master.ssl_key, "key.pem");
    assert_eq!(config.master.http_port, 456);
    assert_eq!(config.master.http_host, "<master_http_host>");
    assert_eq!(config.master.n_jobs, 789);
    assert_eq!(config.master.dashboard_user_name, "user_name");
    assert_eq!(config.master.dashboard_password, "password");
    assert!(config.master.dashboard_redacted);
    assert_eq!(config.master.admin_api_key, "<admin_api_key>");
    assert_eq!(config.server.api_key, "<server_api_key>");
    assert!(config.oidc.enable);
    assert_eq!(config.oidc.issuer, "https://accounts.google.com");
    assert_eq!(config.oidc.client_id, "client_id");
    assert_eq!(config.oidc.client_secret, "client_secret");
    assert_eq!(
        config.oidc.redirect_url,
        "http://localhost:8088/callback/oauth2"
    );
    assert_eq!(config.blob.uri, "s3://<bucket>/path");
    assert_eq!(config.blob.s3.endpoint, "https://s3.example.com");
    assert_eq!(config.blob.s3.access_key_id, "<access_key_id>");
    assert_eq!(config.blob.s3.secret_access_key, "<secret_access_key>");
    assert_eq!(
        config.blob.gcs.credentials_file,
        "/path/to/credentials.json"
    );
    assert_eq!(
        config.blob.azure.endpoint,
        "https://<account>.blob.core.windows.net"
    );
    assert_eq!(config.blob.azure.account_name, "<account_name>");
    assert_eq!(config.blob.azure.account_key, "<account_key>");
    assert_eq!(
        config.blob.azure.connection_string,
        "DefaultEndpointsProtocol=https;AccountName=<account>;AccountKey=<key>"
    );
    assert_eq!(config.openai.base_url, "https://api.openai.com/v1");
    assert_eq!(config.openai.auth_token, "<auth_token>");
    assert_eq!(config.openai.chat_completion_model, "gpt-4");
    assert_eq!(
        config.recommend.ranker.reranker_api.auth_token,
        "<reranker_auth_token>"
    );
    assert_eq!(config.recommend.ranker.reranker_api.url, "<reranker_url>");
    assert_eq!(
        config.recommend.ranker.reranker_api.model,
        "<reranker_model>"
    );
    assert_eq!(
        config.recommend.ranker.decision_api.auth_token,
        "<decision_auth_token>"
    );
    assert_eq!(config.recommend.ranker.decision_api.url, "<decision_url>");
    assert_eq!(
        config.recommend.ranker.decision_api.model,
        "<decision_model>"
    );
    assert_eq!(config.quota.max_users_count, 9);
    assert_eq!(config.quota.max_items_count, 10);
    assert_eq!(config.quota.max_labels_size, 11);
    assert_eq!(config.quota.max_comment_size, 12);
    assert_eq!(config.quota.max_categories_count, 13);
    assert_eq!(config.quota.max_categories_size, 14);

    // check default values
    assert_eq!(config.recommend.cache_size, 100);
}

#[test]
fn test_bindings_match_the_go_table() {
    // The bindings the golden file records from config/config.go.
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/data/config_golden.json"))
            .expect("parse the golden file");
    let expected: Vec<(String, String)> = golden["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|b| {
            (
                b["key"].as_str().expect("key").to_owned(),
                b["env"].as_str().expect("env").to_owned(),
            )
        })
        .collect();
    let actual: Vec<(String, String)> = BINDINGS
        .iter()
        .map(|(k, e)| ((*k).to_owned(), (*e).to_owned()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn test_table_prefix_compat() {
    isolate_root();
    let text = CONFIG_TOML
        .replace("cache_table_prefix = \"\"", "")
        .replace("data_table_prefix = \"\"", "")
        .replace("table_prefix = \"\"", "table_prefix = \"gorse_\"");
    let dir = temp_dir("prefix");
    let path = dir.join("config.toml");
    fs::write(&path, text).expect("write the config");
    let config =
        load_config_with_env(path.to_str().expect("utf-8"), &no_env).expect("load the config");
    assert_eq!(config.database.table_prefix, "gorse_");
    assert_eq!(config.database.cache_table_prefix, "gorse_");
    assert_eq!(config.database.data_table_prefix, "gorse_");
}

#[test]
fn test_non_personalized_config() {
    let a = NonPersonalizedConfig::default();
    let b = NonPersonalizedConfig::default();
    assert_eq!(a.hash(), b.hash());

    let a = NonPersonalizedConfig {
        name: "a".into(),
        ..Default::default()
    };
    let b = NonPersonalizedConfig {
        name: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(), b.hash());
    assert_eq!(a.full_name(), "non-personalized/a");
    assert_eq!(b.full_name(), "non-personalized/b");

    let a = NonPersonalizedConfig {
        score: "a".into(),
        ..Default::default()
    };
    let b = NonPersonalizedConfig {
        score: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(), b.hash());

    let a = NonPersonalizedConfig {
        filter: "a".into(),
        ..Default::default()
    };
    let b = NonPersonalizedConfig {
        filter: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(), b.hash());
}

#[test]
fn test_item_to_item_config() {
    let a = ItemToItemConfig::default();
    let b = ItemToItemConfig::default();
    assert_eq!(a.hash(None), b.hash(None));

    let a = ItemToItemConfig {
        name: "a".into(),
        ..Default::default()
    };
    let b = ItemToItemConfig {
        name: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));
    assert_eq!(a.full_name(), "item-to-item/a");
    assert_eq!(b.full_name(), "item-to-item/b");

    let a = ItemToItemConfig {
        kind: "a".into(),
        ..Default::default()
    };
    let b = ItemToItemConfig {
        kind: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));

    let a = ItemToItemConfig {
        column: "a".into(),
        ..Default::default()
    };
    let b = ItemToItemConfig {
        column: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));

    let c = ItemToItemConfig {
        kind: "users".into(),
        ..Default::default()
    };
    let mut d = RecommendConfig::default();
    let mut e = RecommendConfig::default();
    assert_eq!(c.hash(Some(&d)), c.hash(Some(&e)));

    d.data_source.positive_feedback_types = Some(vec![expr("like")]);
    e.data_source.positive_feedback_types = Some(vec![expr("star")]);
    assert_ne!(c.hash(Some(&d)), c.hash(Some(&e)));
}

#[test]
fn test_user_to_user_config() {
    let a = UserToUserConfig::default();
    let b = UserToUserConfig::default();
    assert_eq!(a.hash(None), b.hash(None));

    let a = UserToUserConfig {
        name: "a".into(),
        ..Default::default()
    };
    let b = UserToUserConfig {
        name: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));
    assert_eq!(a.full_name(), "user-to-user/a");
    assert_eq!(b.full_name(), "user-to-user/b");

    let a = UserToUserConfig {
        kind: "a".into(),
        ..Default::default()
    };
    let b = UserToUserConfig {
        kind: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));

    let a = UserToUserConfig {
        column: "a".into(),
        ..Default::default()
    };
    let b = UserToUserConfig {
        column: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(None), b.hash(None));

    let c = UserToUserConfig {
        kind: "items".into(),
        ..Default::default()
    };
    let mut d = RecommendConfig::default();
    let mut e = RecommendConfig::default();
    assert_eq!(c.hash(Some(&d)), c.hash(Some(&e)));

    d.data_source.positive_feedback_types = Some(vec![expr("like")]);
    e.data_source.positive_feedback_types = Some(vec![expr("star")]);
    assert_ne!(c.hash(Some(&d)), c.hash(Some(&e)));
}

#[test]
fn test_collaborative_config() {
    let mut a = RecommendConfig::default();
    let mut b = RecommendConfig::default();
    let c = CollaborativeConfig::default();
    assert_eq!(c.hash(&a), c.hash(&b));

    a.data_source.positive_feedback_types = Some(vec![expr("like")]);
    b.data_source.positive_feedback_types = Some(vec![expr("star")]);
    assert_ne!(c.hash(&a), c.hash(&b));
}

#[test]
fn test_external_config() {
    let a = ExternalConfig::default();
    let b = ExternalConfig::default();
    assert_eq!(a.hash(), b.hash());

    let a = ExternalConfig {
        name: "a".into(),
        ..Default::default()
    };
    let b = ExternalConfig {
        name: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(), b.hash());
    assert_eq!(a.full_name(), "external/a");
    assert_eq!(b.full_name(), "external/b");

    let a = ExternalConfig {
        script: "a".into(),
        ..Default::default()
    };
    let b = ExternalConfig {
        script: "b".into(),
        ..Default::default()
    };
    assert_ne!(a.hash(), b.hash());
}

#[test]
fn test_recommend_config() {
    let mut a = RecommendConfig::default();
    let mut b = RecommendConfig::default();
    assert_eq!(a.hash(), b.hash());

    a.non_personalized = Some(vec![NonPersonalizedConfig {
        name: "a".into(),
        ..Default::default()
    }]);
    b.non_personalized = Some(vec![NonPersonalizedConfig {
        name: "b".into(),
        ..Default::default()
    }]);
    assert_ne!(a.hash(), b.hash());
    a.non_personalized = Some(vec![]);
    b.non_personalized = Some(vec![]);

    a.item_to_item = Some(vec![ItemToItemConfig {
        name: "a".into(),
        ..Default::default()
    }]);
    b.item_to_item = Some(vec![ItemToItemConfig {
        name: "b".into(),
        ..Default::default()
    }]);
    assert_ne!(a.hash(), b.hash());
    a.item_to_item = Some(vec![]);
    b.item_to_item = Some(vec![]);

    a.user_to_user = Some(vec![UserToUserConfig {
        name: "a".into(),
        ..Default::default()
    }]);
    b.user_to_user = Some(vec![UserToUserConfig {
        name: "b".into(),
        ..Default::default()
    }]);
    assert_ne!(a.hash(), b.hash());
    a.user_to_user = Some(vec![]);
    b.user_to_user = Some(vec![]);

    a.external = Some(vec![ExternalConfig {
        name: "a".into(),
        ..Default::default()
    }]);
    b.external = Some(vec![ExternalConfig {
        name: "b".into(),
        ..Default::default()
    }]);
    assert_ne!(a.hash(), b.hash());
    a.external = Some(vec![]);
    b.external = Some(vec![]);

    a.data_source.positive_feedback_types = Some(vec![expr("like")]);
    b.data_source.positive_feedback_types = Some(vec![expr("star")]);
    assert_ne!(a.hash(), b.hash());

    a.ranker.recommenders = Some(vec!["latest".into()]);
    b.ranker.recommenders = Some(vec!["collaborative".into()]);
    assert_ne!(a.hash(), b.hash());

    a.user_to_user = Some(vec![
        UserToUserConfig {
            name: "a".into(),
            ..Default::default()
        },
        UserToUserConfig {
            name: "b".into(),
            ..Default::default()
        },
    ]);
    b.user_to_user = Some(vec![
        UserToUserConfig {
            name: "b".into(),
            ..Default::default()
        },
        UserToUserConfig {
            name: "a".into(),
            ..Default::default()
        },
    ]);
    a.ranker.recommenders = Some(vec!["user-to-user/a".into(), "user-to-user/b".into()]);
    b.ranker.recommenders = Some(vec!["user-to-user/b".into(), "user-to-user/a".into()]);
    assert_eq!(a.hash(), b.hash());
}

#[test]
fn test_recommend_config_get_item_to_item_config() {
    let config = RecommendConfig {
        item_to_item: Some(vec![
            ItemToItemConfig {
                name: "a".into(),
                ..Default::default()
            },
            ItemToItemConfig {
                name: "b".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    let items = config.item_to_item.as_ref().expect("two");
    assert!(std::ptr::eq(
        &items[1],
        config.get_item_to_item_config("b").expect("b")
    ));
    assert!(config.get_item_to_item_config("missing").is_none());
}

#[test]
fn test_recommend_config_get_user_to_user_config() {
    let config = RecommendConfig {
        user_to_user: Some(vec![
            UserToUserConfig {
                name: "a".into(),
                ..Default::default()
            },
            UserToUserConfig {
                name: "b".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    let users = config.user_to_user.as_ref().expect("two");
    assert!(std::ptr::eq(
        &users[1],
        config.get_user_to_user_config("b").expect("b")
    ));
    assert!(config.get_user_to_user_config("missing").is_none());
}

/// `ValidateTestSuite`'s `SetupTest`: the default config with the cache and
/// data stores the suite sets.
fn suite_config() -> Config {
    isolate_root();
    let mut config = get_default_config();
    config.database.cache_store = "redis://localhost:6379/0".into();
    config.database.data_store = "mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse".into();
    config
}

#[test]
fn test_validate_duplicate_non_personalized() {
    let mut config = suite_config();
    let recommender = NonPersonalizedConfig {
        name: "most_starred_weekly".into(),
        score: "count(feedback, .FeedbackType == 'star')".into(),
        ..Default::default()
    };
    config.recommend.non_personalized = Some(vec![recommender.clone(), recommender]);
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_duplicate_item_to_item() {
    let mut config = suite_config();
    let recommender = ItemToItemConfig {
        name: "item_to_item".into(),
        kind: "users".into(),
        ..Default::default()
    };
    config.recommend.item_to_item = Some(vec![recommender.clone(), recommender]);
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_recommenders_existence() {
    let mut config = suite_config();
    config.recommend.ranker.recommenders = Some(vec!["not_exist".into()]);
    assert!(config.validate().is_err());

    config.recommend.collaborative.kind = "none".into();
    config.recommend.ranker.recommenders = Some(vec!["collaborative".into()]);
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_search_columns() {
    let mut config = suite_config();
    config.recommend.search.columns = Some(vec!["item.Labels.description".into()]);
    assert!(config.validate().is_ok());

    config.recommend.search.columns = Some(vec!["item.Labels.".into()]);
    assert!(config.validate().is_err());
}

#[test]
fn test_decision_api_required_config() {
    let mut config = get_default_config();
    config.recommend.ranker.kind = "decision".into();
    assert_eq!(
        config.validate().expect_err("no url").to_string(),
        "recommend.ranker.decision_api.url must not be empty"
    );
    config.recommend.ranker.decision_api.url = "http://localhost/decisions".into();
    assert_eq!(
        config.validate().expect_err("no model").to_string(),
        "recommend.ranker.decision_api.model must not be empty"
    );
    config.recommend.ranker.decision_api.model = "test-model".into();
    assert!(config.validate().is_ok());
}

#[test]
fn test_validate_quota() {
    let mut config = suite_config();
    assert!(config.validate().is_ok());
    config.quota.max_labels_size = -1;
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_cache_store() {
    // Test that redis+cluster:// prefix is accepted for cache_store
    let mut config = suite_config();
    config.database.cache_store =
        "redis+cluster://:password@192.168.1.11:6379?addr=192.168.0.5:6379&addr=192.168.0.7:6379"
            .into();
    assert!(config.validate().is_ok());

    // Test that rediss+cluster:// prefix is accepted for cache_store
    config.database.cache_store =
        "rediss+cluster://:password@192.168.1.11:6379?addr=192.168.0.5:6379".into();
    assert!(config.validate().is_ok());
}

#[test]
fn test_validate_vector_store() {
    let mut config = suite_config();
    config.database.vector_store = String::new();
    assert!(config.validate().is_err());

    for vector_store in [
        "xvec://vectors",
        "qdrant://localhost:6334",
        "weaviate://localhost:8080",
        "weaviates://localhost:8080",
        "milvus://localhost:19530",
    ] {
        config.database.vector_store = vector_store.into();
        assert!(config.validate().is_ok(), "{vector_store}");
    }

    config.database.vector_store = "sqlite://:memory:".into();
    assert!(config.validate().is_err());

    config.database.vector_store = "mysql://localhost:3306/gorse".into();
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_messages_match_the_golden() {
    // The golden's `validate` cases hold the messages for the struct checks
    // that have no test of their own; see `tests/config_golden.rs`.
    let mut config = suite_config();
    config.master.port = -1;
    assert_eq!(
        config.validate().expect_err("port").to_string(),
        "port must be 0 or greater"
    );
}

/// `mk_dir` makes the directories it returns, under the root.
#[test]
fn test_mk_dir_creates_directories() {
    isolate_root();
    let path = mk_dir(&["blob"]);
    assert!(path.is_dir());
    assert!(path.ends_with("blob"));
}
