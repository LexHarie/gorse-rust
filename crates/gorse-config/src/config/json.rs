// Ports: config/config.go (the struct fields' json names and tags, which json.Marshal and json.Unmarshal of Config use) @ add21b5

//! The JSON form of [`Config`]: `json.Marshal` writes the Meta.config the
//! master sends (contracts/grpc.md), `json.Unmarshal` reads it back in the
//! workers and servers. Keys are the Go field names, except the four
//! recommender types with `json` tags. Durations are nanoseconds, feedback
//! type expressions are their strings, and strings are escaped as Go escapes
//! them (`<` is `<`).
//!
//! Decoding matches a key to a field exactly or else by `strings.EqualFold`,
//! and a JSON `null` leaves its field as it is, as `encoding/json` does. Go
//! keeps decoding after a type mismatch and returns the first one; here the
//! first mismatch is returned at once. The inputs Gorse decodes are Go's own
//! output, so no mismatch arises in practice.

use gorse_common::expression::{FeedbackTypeExpression, go_json_quote};
use gorse_common::gostrconv::format_float;
use gorse_common::gounicode::equal_fold;
use serde_json::{Map as JsonMap, Value as Json};

use super::schema::*;

/// The error text of a failed JSON call, as Go prints it.
type Result<T> = std::result::Result<T, String>;

impl Config {
    /// `json.Marshal(config)`: the form the master sends as Meta.config.
    /// Fails for a NaN or infinite float, with Go's text.
    pub fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("Database", self.database.marshal_json()?),
            ("Master", self.master.marshal_json()?),
            ("Server", self.server.marshal_json()?),
            ("Recommend", self.recommend.marshal_json()?),
            ("Tracing", self.tracing.marshal_json()?),
            ("OIDC", self.oidc.marshal_json()?),
            ("OpenAI", self.openai.marshal_json()?),
            ("Blob", self.blob.marshal_json()?),
            ("Quota", self.quota.marshal_json()?),
        ]))
    }

    /// `json.Unmarshal(data, config)`: decodes the fields `data` holds into
    /// `self` and leaves the others as they are.
    pub fn unmarshal_json(&mut self, data: &str) -> Result<()> {
        let value: Json = serde_json::from_str(data).map_err(|err| err.to_string())?;
        let Some(fields) = object_fields(&value, "Config")? else {
            return Ok(());
        };
        let out = self;
        field(fields, "Database", |v| out.database.unmarshal_json(v))?;
        field(fields, "Master", |v| out.master.unmarshal_json(v))?;
        field(fields, "Server", |v| out.server.unmarshal_json(v))?;
        field(fields, "Recommend", |v| out.recommend.decode(v))?;
        field(fields, "Tracing", |v| out.tracing.unmarshal_json(v))?;
        field(fields, "OIDC", |v| out.oidc.unmarshal_json(v))?;
        field(fields, "OpenAI", |v| out.openai.unmarshal_json(v))?;
        field(fields, "Blob", |v| out.blob.unmarshal_json(v))?;
        field(fields, "Quota", |v| out.quota.unmarshal_json(v))
    }
}

impl RecommendConfig {
    /// `json.Unmarshal(data, recommend)`: the golden cases decode a recommender
    /// set this way.
    pub fn unmarshal_json(&mut self, data: &str) -> Result<()> {
        let value: Json = serde_json::from_str(data).map_err(|err| err.to_string())?;
        self.decode(&value)
    }
}

/// The JSON object's fields, or `None` for `null`. Anything else is a
/// mismatch.
fn object_fields<'a>(value: &'a Json, go_type: &str) -> Result<Option<&'a JsonMap<String, Json>>> {
    match value {
        Json::Null => Ok(None),
        Json::Object(fields) => Ok(Some(fields)),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type config.{go_type}",
            kind(other)
        )),
    }
}

/// The value of the field `name`: the key equal to it, or else the first key
/// `strings.EqualFold` matches.
fn field<'a>(
    fields: &'a JsonMap<String, Json>,
    name: &str,
    set: impl FnOnce(&'a Json) -> Result<()>,
) -> Result<()> {
    let value = fields.get(name).or_else(|| {
        fields
            .iter()
            .find(|(key, _)| equal_fold(key, name))
            .map(|(_, value)| value)
    });
    match value {
        Some(value) => set(value),
        None => Ok(()),
    }
}

/// The JSON kind of a value, as Go's `UnmarshalTypeError` names it.
fn kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// `json: cannot unmarshal <kind> into Go struct field <at> of type <ty>`.
fn mismatch(value: &Json, at: &str, ty: &str) -> String {
    format!(
        "json: cannot unmarshal {} into Go struct field {at} of type {ty}",
        kind(value)
    )
}

fn json_string(value: &Json, out: &mut String, at: &str) -> Result<()> {
    match value {
        Json::Null => Ok(()),
        Json::String(s) => {
            s.clone_into(out);
            Ok(())
        }
        other => Err(mismatch(other, at, "string")),
    }
}

fn json_bool(value: &Json, out: &mut bool, at: &str) -> Result<()> {
    match value {
        Json::Null => Ok(()),
        Json::Bool(b) => {
            *out = *b;
            Ok(())
        }
        other => Err(mismatch(other, at, "bool")),
    }
}

/// A Go integer type named `ty` in the error text, which the value must fit.
fn json_int(value: &Json, out: &mut i64, at: &str, ty: &str) -> Result<()> {
    match value {
        Json::Null => Ok(()),
        Json::Number(n) => match n.as_i64() {
            Some(i) => {
                *out = i;
                Ok(())
            }
            None => Err(format!(
                "json: cannot unmarshal number {n} into Go struct field {at} of type {ty}"
            )),
        },
        other => Err(mismatch(other, at, ty)),
    }
}

fn json_uint(value: &Json, out: &mut u64, at: &str) -> Result<()> {
    match value {
        Json::Null => Ok(()),
        Json::Number(n) => match n.as_u64() {
            Some(u) => {
                *out = u;
                Ok(())
            }
            None => Err(format!(
                "json: cannot unmarshal number {n} into Go struct field {at} of type uint"
            )),
        },
        other => Err(mismatch(other, at, "uint")),
    }
}

fn json_float(value: &Json, out: &mut f64, at: &str) -> Result<()> {
    match value {
        Json::Null => Ok(()),
        Json::Number(n) => {
            *out = n.as_f64().unwrap_or_default();
            Ok(())
        }
        other => Err(mismatch(other, at, "float64")),
    }
}

/// A slice of Go values: `null` is a nil slice; each element is decoded into
/// a zero value.
fn json_list<T: Default>(
    value: &Json,
    out: &mut Option<Vec<T>>,
    at: &str,
    decode: impl Fn(&Json, &mut T) -> Result<()>,
) -> Result<()> {
    match value {
        Json::Null => {
            *out = None;
            Ok(())
        }
        Json::Array(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                let mut elem = T::default();
                decode(item, &mut elem)?;
                list.push(elem);
            }
            *out = Some(list);
            Ok(())
        }
        other => Err(mismatch(other, at, "slice")),
    }
}

fn json_strings(value: &Json, out: &mut Option<Vec<String>>, at: &str) -> Result<()> {
    json_list(value, out, at, |item, s: &mut String| {
        json_string(item, s, at)
    })
}

fn json_expressions(
    value: &Json,
    out: &mut Option<Vec<FeedbackTypeExpression>>,
    at: &str,
) -> Result<()> {
    json_list(
        value,
        out,
        at,
        |item, expr: &mut FeedbackTypeExpression| match item {
            Json::String(s) => expr.from_string(s).map_err(|err| err.to_string()),
            Json::Null => expr.from_string("").map_err(|err| err.to_string()),
            other => Err(mismatch(other, at, "expression.FeedbackTypeExpression")),
        },
    )
}

/// A JSON object of the struct `go_type`, which `body` decodes field by field.
fn json_struct(
    value: &Json,
    go_type: &str,
    body: impl FnOnce(&JsonMap<String, Json>) -> Result<()>,
) -> Result<()> {
    match object_fields(value, go_type)? {
        Some(fields) => body(fields),
        None => Ok(()),
    }
}

/// An object of `key: value` pairs, written as a JSON object.
fn object(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{}:{value}", go_json_quote(key)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// A slice; `None` is `null`.
fn list<T>(items: &Option<Vec<T>>, item: impl Fn(&T) -> Result<String>) -> Result<String> {
    match items {
        None => Ok("null".to_owned()),
        Some(items) => {
            let parts = items.iter().map(item).collect::<Result<Vec<_>>>()?;
            Ok(format!("[{}]", parts.join(",")))
        }
    }
}

fn strings_json(items: &Option<Vec<String>>) -> Result<String> {
    list(items, |s| Ok(go_json_quote(s)))
}

fn expressions_json(items: &Option<Vec<FeedbackTypeExpression>>) -> Result<String> {
    list(items, |e| Ok(e.marshal_json()))
}

/// `encodeFloat`: NaN and infinities are unsupported; a float of large or
/// small magnitude is written in exponent form with the `e-09` form cleaned
/// to `e-9`.
fn float_json(f: f64) -> Result<String> {
    if f.is_nan() || f.is_infinite() {
        return Err(format!(
            "json: unsupported value: {}",
            format_float(f, b'g', -1, 64)
        ));
    }
    let abs = f.abs();
    let fmt = if abs != 0.0 && (abs < 1e-6 || abs >= 1e21) {
        b'e'
    } else {
        b'f'
    };
    let mut text = format_float(f, fmt, -1, 64);
    // `e-09` becomes `e-9`
    if fmt == b'e' && text.len() >= 4 && text[text.len() - 4..].starts_with("e-0") {
        text.remove(text.len() - 2);
    }
    Ok(text)
}

fn int_json(i: i64) -> String {
    i.to_string()
}

fn duration_json(d: gorse_common::gotime::Duration) -> String {
    d.0.to_string()
}

fn bool_json(b: bool) -> String {
    b.to_string()
}

impl DatabaseConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("DataStore", go_json_quote(&self.data_store)),
            ("CacheStore", go_json_quote(&self.cache_store)),
            ("VectorStore", go_json_quote(&self.vector_store)),
            ("TablePrefix", go_json_quote(&self.table_prefix)),
            ("DataTablePrefix", go_json_quote(&self.data_table_prefix)),
            ("CacheTablePrefix", go_json_quote(&self.cache_table_prefix)),
            ("CacheClientName", go_json_quote(&self.cache_client_name)),
            (
                "VectorTablePrefix",
                go_json_quote(&self.vector_table_prefix),
            ),
            ("MySQL", self.mysql.marshal_json()),
            ("Postgres", self.postgres.marshal_json()),
            ("Redis", self.redis.marshal_json()),
            ("Vector", self.vector.marshal_json()),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "DatabaseConfig", |f| {
            field(f, "DataStore", |v| {
                json_string(v, &mut self.data_store, "DatabaseConfig.DataStore")
            })?;
            field(f, "CacheStore", |v| {
                json_string(v, &mut self.cache_store, "DatabaseConfig.CacheStore")
            })?;
            field(f, "VectorStore", |v| {
                json_string(v, &mut self.vector_store, "DatabaseConfig.VectorStore")
            })?;
            field(f, "TablePrefix", |v| {
                json_string(v, &mut self.table_prefix, "DatabaseConfig.TablePrefix")
            })?;
            field(f, "DataTablePrefix", |v| {
                json_string(
                    v,
                    &mut self.data_table_prefix,
                    "DatabaseConfig.DataTablePrefix",
                )
            })?;
            field(f, "CacheTablePrefix", |v| {
                json_string(
                    v,
                    &mut self.cache_table_prefix,
                    "DatabaseConfig.CacheTablePrefix",
                )
            })?;
            field(f, "CacheClientName", |v| {
                json_string(
                    v,
                    &mut self.cache_client_name,
                    "DatabaseConfig.CacheClientName",
                )
            })?;
            field(f, "VectorTablePrefix", |v| {
                json_string(
                    v,
                    &mut self.vector_table_prefix,
                    "DatabaseConfig.VectorTablePrefix",
                )
            })?;
            field(f, "MySQL", |v| self.mysql.unmarshal_json(v))?;
            field(f, "Postgres", |v| self.postgres.unmarshal_json(v))?;
            field(f, "Redis", |v| self.redis.unmarshal_json(v))?;
            field(f, "Vector", |v| self.vector.unmarshal_json(v))
        })
    }
}

impl MySQLConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("IsolationLevel", go_json_quote(&self.isolation_level)),
            ("MaxOpenConns", int_json(self.max_open_conns)),
            ("MaxIdleConns", int_json(self.max_idle_conns)),
            ("ConnMaxLifetime", duration_json(self.conn_max_lifetime)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "MySQLConfig", |f| {
            field(f, "IsolationLevel", |v| {
                json_string(v, &mut self.isolation_level, "MySQLConfig.IsolationLevel")
            })?;
            field(f, "MaxOpenConns", |v| {
                json_int(
                    v,
                    &mut self.max_open_conns,
                    "MySQLConfig.MaxOpenConns",
                    "int",
                )
            })?;
            field(f, "MaxIdleConns", |v| {
                json_int(
                    v,
                    &mut self.max_idle_conns,
                    "MySQLConfig.MaxIdleConns",
                    "int",
                )
            })?;
            field(f, "ConnMaxLifetime", |v| {
                json_duration(
                    v,
                    &mut self.conn_max_lifetime,
                    "MySQLConfig.ConnMaxLifetime",
                )
            })
        })
    }
}

impl SQLConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("MaxOpenConns", int_json(self.max_open_conns)),
            ("MaxIdleConns", int_json(self.max_idle_conns)),
            ("ConnMaxLifetime", duration_json(self.conn_max_lifetime)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "SQLConfig", |f| {
            field(f, "MaxOpenConns", |v| {
                json_int(v, &mut self.max_open_conns, "SQLConfig.MaxOpenConns", "int")
            })?;
            field(f, "MaxIdleConns", |v| {
                json_int(v, &mut self.max_idle_conns, "SQLConfig.MaxIdleConns", "int")
            })?;
            field(f, "ConnMaxLifetime", |v| {
                json_duration(v, &mut self.conn_max_lifetime, "SQLConfig.ConnMaxLifetime")
            })
        })
    }
}

impl RedisConfig {
    fn marshal_json(&self) -> String {
        object(&[("MaxSearchResults", int_json(self.max_search_results))])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "RedisConfig", |f| {
            field(f, "MaxSearchResults", |v| {
                json_int(
                    v,
                    &mut self.max_search_results,
                    "RedisConfig.MaxSearchResults",
                    "int",
                )
            })
        })
    }
}

impl VectorConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("QuantizationType", go_json_quote(&self.quantization_type)),
            ("QuantizationBits", int_json(self.quantization_bits)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "VectorConfig", |f| {
            field(f, "QuantizationType", |v| {
                json_string(
                    v,
                    &mut self.quantization_type,
                    "VectorConfig.QuantizationType",
                )
            })?;
            field(f, "QuantizationBits", |v| {
                json_int(
                    v,
                    &mut self.quantization_bits,
                    "VectorConfig.QuantizationBits",
                    "int",
                )
            })
        })
    }
}

impl MasterConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("Port", int_json(self.port)),
            ("Host", go_json_quote(&self.host)),
            ("SSLMode", bool_json(self.ssl_mode)),
            ("SSLCA", go_json_quote(&self.ssl_ca)),
            ("SSLCert", go_json_quote(&self.ssl_cert)),
            ("SSLKey", go_json_quote(&self.ssl_key)),
            ("HttpPort", int_json(self.http_port)),
            ("HttpHost", go_json_quote(&self.http_host)),
            ("HttpCorsDomains", strings_json(&self.http_cors_domains)?),
            ("HttpCorsMethods", strings_json(&self.http_cors_methods)?),
            ("NumJobs", int_json(self.n_jobs)),
            ("MetaTimeout", duration_json(self.meta_timeout)),
            (
                "DashboardUserName",
                go_json_quote(&self.dashboard_user_name),
            ),
            ("DashboardPassword", go_json_quote(&self.dashboard_password)),
            ("DashboardRedacted", bool_json(self.dashboard_redacted)),
            ("AdminAPIKey", go_json_quote(&self.admin_api_key)),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "MasterConfig", |f| {
            field(f, "Port", |v| {
                json_int(v, &mut self.port, "MasterConfig.Port", "int")
            })?;
            field(f, "Host", |v| {
                json_string(v, &mut self.host, "MasterConfig.Host")
            })?;
            field(f, "SSLMode", |v| {
                json_bool(v, &mut self.ssl_mode, "MasterConfig.SSLMode")
            })?;
            field(f, "SSLCA", |v| {
                json_string(v, &mut self.ssl_ca, "MasterConfig.SSLCA")
            })?;
            field(f, "SSLCert", |v| {
                json_string(v, &mut self.ssl_cert, "MasterConfig.SSLCert")
            })?;
            field(f, "SSLKey", |v| {
                json_string(v, &mut self.ssl_key, "MasterConfig.SSLKey")
            })?;
            field(f, "HttpPort", |v| {
                json_int(v, &mut self.http_port, "MasterConfig.HttpPort", "int")
            })?;
            field(f, "HttpHost", |v| {
                json_string(v, &mut self.http_host, "MasterConfig.HttpHost")
            })?;
            field(f, "HttpCorsDomains", |v| {
                json_strings(
                    v,
                    &mut self.http_cors_domains,
                    "MasterConfig.HttpCorsDomains",
                )
            })?;
            field(f, "HttpCorsMethods", |v| {
                json_strings(
                    v,
                    &mut self.http_cors_methods,
                    "MasterConfig.HttpCorsMethods",
                )
            })?;
            field(f, "NumJobs", |v| {
                json_int(v, &mut self.n_jobs, "MasterConfig.NumJobs", "int")
            })?;
            field(f, "MetaTimeout", |v| {
                json_duration(v, &mut self.meta_timeout, "MasterConfig.MetaTimeout")
            })?;
            field(f, "DashboardUserName", |v| {
                json_string(
                    v,
                    &mut self.dashboard_user_name,
                    "MasterConfig.DashboardUserName",
                )
            })?;
            field(f, "DashboardPassword", |v| {
                json_string(
                    v,
                    &mut self.dashboard_password,
                    "MasterConfig.DashboardPassword",
                )
            })?;
            field(f, "DashboardRedacted", |v| {
                json_bool(
                    v,
                    &mut self.dashboard_redacted,
                    "MasterConfig.DashboardRedacted",
                )
            })?;
            field(f, "AdminAPIKey", |v| {
                json_string(v, &mut self.admin_api_key, "MasterConfig.AdminAPIKey")
            })
        })
    }
}

impl ServerConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("APIKey", go_json_quote(&self.api_key)),
            ("DefaultN", int_json(self.default_n)),
            ("ClockError", duration_json(self.clock_error)),
            ("AutoInsertUser", bool_json(self.auto_insert_user)),
            ("AutoInsertItem", bool_json(self.auto_insert_item)),
            ("CacheExpire", duration_json(self.cache_expire)),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "ServerConfig", |f| {
            field(f, "APIKey", |v| {
                json_string(v, &mut self.api_key, "ServerConfig.APIKey")
            })?;
            field(f, "DefaultN", |v| {
                json_int(v, &mut self.default_n, "ServerConfig.DefaultN", "int")
            })?;
            field(f, "ClockError", |v| {
                json_duration(v, &mut self.clock_error, "ServerConfig.ClockError")
            })?;
            field(f, "AutoInsertUser", |v| {
                json_bool(v, &mut self.auto_insert_user, "ServerConfig.AutoInsertUser")
            })?;
            field(f, "AutoInsertItem", |v| {
                json_bool(v, &mut self.auto_insert_item, "ServerConfig.AutoInsertItem")
            })?;
            field(f, "CacheExpire", |v| {
                json_duration(v, &mut self.cache_expire, "ServerConfig.CacheExpire")
            })
        })
    }
}

impl RecommendConfig {
    /// Go's `json.Marshal` of the recommend config, as the master stores it in
    /// the meta database (`RECOMMEND_CONFIG`).
    pub fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("CacheSize", int_json(self.cache_size)),
            ("CacheExpire", duration_json(self.cache_expire)),
            ("ContextSize", int_json(self.context_size)),
            ("ActiveUserTTL", int_json(self.active_user_ttl)),
            ("DataSource", self.data_source.marshal_json()?),
            ("Search", self.search.marshal_json()?),
            (
                "NonPersonalized",
                list(&self.non_personalized, NonPersonalizedConfig::marshal_json)?,
            ),
            (
                "ItemToItem",
                list(&self.item_to_item, ItemToItemConfig::marshal_json)?,
            ),
            (
                "UserToUser",
                list(&self.user_to_user, UserToUserConfig::marshal_json)?,
            ),
            ("Collaborative", self.collaborative.marshal_json()?),
            (
                "External",
                list(&self.external, ExternalConfig::marshal_json)?,
            ),
            ("Replacement", self.replacement.marshal_json()?),
            ("Ranker", self.ranker.marshal_json()?),
            ("Fallback", self.fallback.marshal_json()?),
        ]))
    }

    fn decode(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "RecommendConfig", |f| {
            field(f, "CacheSize", |v| {
                json_int(v, &mut self.cache_size, "RecommendConfig.CacheSize", "int")
            })?;
            field(f, "CacheExpire", |v| {
                json_duration(v, &mut self.cache_expire, "RecommendConfig.CacheExpire")
            })?;
            field(f, "ContextSize", |v| {
                json_int(
                    v,
                    &mut self.context_size,
                    "RecommendConfig.ContextSize",
                    "int",
                )
            })?;
            field(f, "ActiveUserTTL", |v| {
                json_int(
                    v,
                    &mut self.active_user_ttl,
                    "RecommendConfig.ActiveUserTTL",
                    "int",
                )
            })?;
            field(f, "DataSource", |v| self.data_source.unmarshal_json(v))?;
            field(f, "Search", |v| self.search.unmarshal_json(v))?;
            field(f, "NonPersonalized", |v| {
                json_list(
                    v,
                    &mut self.non_personalized,
                    "RecommendConfig.NonPersonalized",
                    NonPersonalizedConfig::unmarshal_json,
                )
            })?;
            field(f, "ItemToItem", |v| {
                json_list(
                    v,
                    &mut self.item_to_item,
                    "RecommendConfig.ItemToItem",
                    ItemToItemConfig::unmarshal_json,
                )
            })?;
            field(f, "UserToUser", |v| {
                json_list(
                    v,
                    &mut self.user_to_user,
                    "RecommendConfig.UserToUser",
                    UserToUserConfig::unmarshal_json,
                )
            })?;
            field(f, "Collaborative", |v| self.collaborative.unmarshal_json(v))?;
            field(f, "External", |v| {
                json_list(
                    v,
                    &mut self.external,
                    "RecommendConfig.External",
                    ExternalConfig::unmarshal_json,
                )
            })?;
            field(f, "Replacement", |v| self.replacement.unmarshal_json(v))?;
            field(f, "Ranker", |v| self.ranker.unmarshal_json(v))?;
            field(f, "Fallback", |v| self.fallback.unmarshal_json(v))
        })
    }
}

impl DataSourceConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            (
                "PositiveFeedbackTypes",
                expressions_json(&self.positive_feedback_types)?,
            ),
            (
                "NegativeFeedbackTypes",
                expressions_json(&self.negative_feedback_types)?,
            ),
            (
                "ReadFeedbackTypes",
                expressions_json(&self.read_feedback_types)?,
            ),
            (
                "PositiveFeedbackTTL",
                self.positive_feedback_ttl.to_string(),
            ),
            ("ItemTTL", self.item_ttl.to_string()),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "DataSourceConfig", |f| {
            field(f, "PositiveFeedbackTypes", |v| {
                json_expressions(
                    v,
                    &mut self.positive_feedback_types,
                    "DataSourceConfig.PositiveFeedbackTypes",
                )
            })?;
            field(f, "NegativeFeedbackTypes", |v| {
                json_expressions(
                    v,
                    &mut self.negative_feedback_types,
                    "DataSourceConfig.NegativeFeedbackTypes",
                )
            })?;
            field(f, "ReadFeedbackTypes", |v| {
                json_expressions(
                    v,
                    &mut self.read_feedback_types,
                    "DataSourceConfig.ReadFeedbackTypes",
                )
            })?;
            field(f, "PositiveFeedbackTTL", |v| {
                json_uint(
                    v,
                    &mut self.positive_feedback_ttl,
                    "DataSourceConfig.PositiveFeedbackTTL",
                )
            })?;
            field(f, "ItemTTL", |v| {
                json_uint(v, &mut self.item_ttl, "DataSourceConfig.ItemTTL")
            })
        })
    }
}

impl SearchConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[("Columns", strings_json(&self.columns)?)]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "SearchConfig", |f| {
            field(f, "Columns", |v| {
                json_strings(v, &mut self.columns, "SearchConfig.Columns")
            })
        })
    }
}

impl NonPersonalizedConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("name", go_json_quote(&self.name)),
            ("score", go_json_quote(&self.score)),
            ("filter", go_json_quote(&self.filter)),
        ]))
    }

    fn unmarshal_json(value: &Json, out: &mut Self) -> Result<()> {
        json_struct(value, "NonPersonalizedConfig", |f| {
            field(f, "name", |v| {
                json_string(v, &mut out.name, "NonPersonalizedConfig.name")
            })?;
            field(f, "score", |v| {
                json_string(v, &mut out.score, "NonPersonalizedConfig.score")
            })?;
            field(f, "filter", |v| {
                json_string(v, &mut out.filter, "NonPersonalizedConfig.filter")
            })
        })
    }
}

impl ItemToItemConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("name", go_json_quote(&self.name)),
            ("type", go_json_quote(&self.kind)),
            ("column", go_json_quote(&self.column)),
        ]))
    }

    fn unmarshal_json(value: &Json, out: &mut Self) -> Result<()> {
        json_struct(value, "ItemToItemConfig", |f| {
            field(f, "name", |v| {
                json_string(v, &mut out.name, "ItemToItemConfig.name")
            })?;
            field(f, "type", |v| {
                json_string(v, &mut out.kind, "ItemToItemConfig.type")
            })?;
            field(f, "column", |v| {
                json_string(v, &mut out.column, "ItemToItemConfig.column")
            })
        })
    }
}

impl UserToUserConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("name", go_json_quote(&self.name)),
            ("type", go_json_quote(&self.kind)),
            ("column", go_json_quote(&self.column)),
        ]))
    }

    fn unmarshal_json(value: &Json, out: &mut Self) -> Result<()> {
        json_struct(value, "UserToUserConfig", |f| {
            field(f, "name", |v| {
                json_string(v, &mut out.name, "UserToUserConfig.name")
            })?;
            field(f, "type", |v| {
                json_string(v, &mut out.kind, "UserToUserConfig.type")
            })?;
            field(f, "column", |v| {
                json_string(v, &mut out.column, "UserToUserConfig.column")
            })
        })
    }
}

impl CollaborativeConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("Type", go_json_quote(&self.kind)),
            ("FitPeriod", duration_json(self.fit_period)),
            ("FitEpoch", int_json(self.fit_epoch)),
            ("OptimizePeriod", duration_json(self.optimize_period)),
            ("OptimizeTrials", int_json(self.optimize_trials)),
            ("EarlyStopping", self.early_stopping.marshal_json()),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "CollaborativeConfig", |f| {
            field(f, "Type", |v| {
                json_string(v, &mut self.kind, "CollaborativeConfig.Type")
            })?;
            field(f, "FitPeriod", |v| {
                json_duration(v, &mut self.fit_period, "CollaborativeConfig.FitPeriod")
            })?;
            field(f, "FitEpoch", |v| {
                json_int(
                    v,
                    &mut self.fit_epoch,
                    "CollaborativeConfig.FitEpoch",
                    "int",
                )
            })?;
            field(f, "OptimizePeriod", |v| {
                json_duration(
                    v,
                    &mut self.optimize_period,
                    "CollaborativeConfig.OptimizePeriod",
                )
            })?;
            field(f, "OptimizeTrials", |v| {
                json_int(
                    v,
                    &mut self.optimize_trials,
                    "CollaborativeConfig.OptimizeTrials",
                    "int",
                )
            })?;
            field(f, "EarlyStopping", |v| {
                self.early_stopping.unmarshal_json(v)
            })
        })
    }
}

impl EarlyStoppingConfig {
    fn marshal_json(&self) -> String {
        object(&[("Patience", int_json(self.patience))])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "EarlyStoppingConfig", |f| {
            field(f, "Patience", |v| {
                json_int(v, &mut self.patience, "EarlyStoppingConfig.Patience", "int")
            })
        })
    }
}

impl ExternalConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("name", go_json_quote(&self.name)),
            ("script", go_json_quote(&self.script)),
        ]))
    }

    fn unmarshal_json(value: &Json, out: &mut Self) -> Result<()> {
        json_struct(value, "ExternalConfig", |f| {
            field(f, "name", |v| {
                json_string(v, &mut out.name, "ExternalConfig.name")
            })?;
            field(f, "script", |v| {
                json_string(v, &mut out.script, "ExternalConfig.script")
            })
        })
    }
}

impl ReplacementConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("EnableReplacement", bool_json(self.enable_replacement)),
            (
                "PositiveReplacementDecay",
                float_json(self.positive_replacement_decay)?,
            ),
            (
                "ReadReplacementDecay",
                float_json(self.read_replacement_decay)?,
            ),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "ReplacementConfig", |f| {
            field(f, "EnableReplacement", |v| {
                json_bool(
                    v,
                    &mut self.enable_replacement,
                    "ReplacementConfig.EnableReplacement",
                )
            })?;
            field(f, "PositiveReplacementDecay", |v| {
                json_float(
                    v,
                    &mut self.positive_replacement_decay,
                    "ReplacementConfig.PositiveReplacementDecay",
                )
            })?;
            field(f, "ReadReplacementDecay", |v| {
                json_float(
                    v,
                    &mut self.read_replacement_decay,
                    "ReplacementConfig.ReadReplacementDecay",
                )
            })
        })
    }
}

impl RankerConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("Type", go_json_quote(&self.kind)),
            ("Recommenders", strings_json(&self.recommenders)?),
            ("CacheExpire", duration_json(self.cache_expire)),
            ("FitPeriod", duration_json(self.fit_period)),
            ("FitEpoch", int_json(self.fit_epoch)),
            ("OptimizePeriod", duration_json(self.optimize_period)),
            ("OptimizeTrials", int_json(self.optimize_trials)),
            ("EarlyStopping", self.early_stopping.marshal_json()),
            ("RerankerAPI", self.reranker_api.marshal_json()),
            ("DecisionAPI", self.decision_api.marshal_json()),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "RankerConfig", |f| {
            field(f, "Type", |v| {
                json_string(v, &mut self.kind, "RankerConfig.Type")
            })?;
            field(f, "Recommenders", |v| {
                json_strings(v, &mut self.recommenders, "RankerConfig.Recommenders")
            })?;
            field(f, "CacheExpire", |v| {
                json_duration(v, &mut self.cache_expire, "RankerConfig.CacheExpire")
            })?;
            field(f, "FitPeriod", |v| {
                json_duration(v, &mut self.fit_period, "RankerConfig.FitPeriod")
            })?;
            field(f, "FitEpoch", |v| {
                json_int(v, &mut self.fit_epoch, "RankerConfig.FitEpoch", "int")
            })?;
            field(f, "OptimizePeriod", |v| {
                json_duration(v, &mut self.optimize_period, "RankerConfig.OptimizePeriod")
            })?;
            field(f, "OptimizeTrials", |v| {
                json_int(
                    v,
                    &mut self.optimize_trials,
                    "RankerConfig.OptimizeTrials",
                    "int",
                )
            })?;
            field(f, "EarlyStopping", |v| {
                self.early_stopping.unmarshal_json(v)
            })?;
            field(f, "RerankerAPI", |v| self.reranker_api.unmarshal_json(v))?;
            field(f, "DecisionAPI", |v| self.decision_api.unmarshal_json(v))
        })
    }
}

impl FallbackConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[(
            "Recommenders",
            strings_json(&self.recommenders)?,
        )]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "FallbackConfig", |f| {
            field(f, "Recommenders", |v| {
                json_strings(v, &mut self.recommenders, "FallbackConfig.Recommenders")
            })
        })
    }
}

impl TracingConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("EnableTracing", bool_json(self.enable_tracing)),
            ("Exporter", go_json_quote(&self.exporter)),
            ("CollectorEndpoint", go_json_quote(&self.collector_endpoint)),
            ("Sampler", go_json_quote(&self.sampler)),
            ("Ratio", float_json(self.ratio)?),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "TracingConfig", |f| {
            field(f, "EnableTracing", |v| {
                json_bool(v, &mut self.enable_tracing, "TracingConfig.EnableTracing")
            })?;
            field(f, "Exporter", |v| {
                json_string(v, &mut self.exporter, "TracingConfig.Exporter")
            })?;
            field(f, "CollectorEndpoint", |v| {
                json_string(
                    v,
                    &mut self.collector_endpoint,
                    "TracingConfig.CollectorEndpoint",
                )
            })?;
            field(f, "Sampler", |v| {
                json_string(v, &mut self.sampler, "TracingConfig.Sampler")
            })?;
            field(f, "Ratio", |v| {
                json_float(v, &mut self.ratio, "TracingConfig.Ratio")
            })
        })
    }
}

impl OIDCConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("Enable", bool_json(self.enable)),
            ("Issuer", go_json_quote(&self.issuer)),
            ("ClientID", go_json_quote(&self.client_id)),
            ("ClientSecret", go_json_quote(&self.client_secret)),
            ("RedirectURL", go_json_quote(&self.redirect_url)),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "OIDCConfig", |f| {
            field(f, "Enable", |v| {
                json_bool(v, &mut self.enable, "OIDCConfig.Enable")
            })?;
            field(f, "Issuer", |v| {
                json_string(v, &mut self.issuer, "OIDCConfig.Issuer")
            })?;
            field(f, "ClientID", |v| {
                json_string(v, &mut self.client_id, "OIDCConfig.ClientID")
            })?;
            field(f, "ClientSecret", |v| {
                json_string(v, &mut self.client_secret, "OIDCConfig.ClientSecret")
            })?;
            field(f, "RedirectURL", |v| {
                json_string(v, &mut self.redirect_url, "OIDCConfig.RedirectURL")
            })
        })
    }
}

impl DecisionAPIConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("StateTemplate", go_json_quote(&self.state_template)),
            (
                "InstructionsTemplate",
                go_json_quote(&self.instructions_template),
            ),
            ("AuthToken", go_json_quote(&self.auth_token)),
            ("Model", go_json_quote(&self.model)),
            ("URL", go_json_quote(&self.url)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "DecisionAPIConfig", |f| {
            field(f, "StateTemplate", |v| {
                json_string(
                    v,
                    &mut self.state_template,
                    "DecisionAPIConfig.StateTemplate",
                )
            })?;
            field(f, "InstructionsTemplate", |v| {
                json_string(
                    v,
                    &mut self.instructions_template,
                    "DecisionAPIConfig.InstructionsTemplate",
                )
            })?;
            field(f, "AuthToken", |v| {
                json_string(v, &mut self.auth_token, "DecisionAPIConfig.AuthToken")
            })?;
            field(f, "Model", |v| {
                json_string(v, &mut self.model, "DecisionAPIConfig.Model")
            })?;
            field(f, "URL", |v| {
                json_string(v, &mut self.url, "DecisionAPIConfig.URL")
            })
        })
    }
}

impl RerankerAPIConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("QueryTemplate", go_json_quote(&self.query_template)),
            ("DocumentTemplate", go_json_quote(&self.document_template)),
            ("AuthToken", go_json_quote(&self.auth_token)),
            ("Model", go_json_quote(&self.model)),
            ("URL", go_json_quote(&self.url)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "RerankerAPIConfig", |f| {
            field(f, "QueryTemplate", |v| {
                json_string(
                    v,
                    &mut self.query_template,
                    "RerankerAPIConfig.QueryTemplate",
                )
            })?;
            field(f, "DocumentTemplate", |v| {
                json_string(
                    v,
                    &mut self.document_template,
                    "RerankerAPIConfig.DocumentTemplate",
                )
            })?;
            field(f, "AuthToken", |v| {
                json_string(v, &mut self.auth_token, "RerankerAPIConfig.AuthToken")
            })?;
            field(f, "Model", |v| {
                json_string(v, &mut self.model, "RerankerAPIConfig.Model")
            })?;
            field(f, "URL", |v| {
                json_string(v, &mut self.url, "RerankerAPIConfig.URL")
            })
        })
    }
}

impl OpenAIConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("BaseURL", go_json_quote(&self.base_url)),
            ("AuthToken", go_json_quote(&self.auth_token)),
            (
                "ChatCompletionModel",
                go_json_quote(&self.chat_completion_model),
            ),
            ("ChatCompletionRPM", int_json(self.chat_completion_rpm)),
            ("ChatCompletionTPM", int_json(self.chat_completion_tpm)),
            ("EmbeddingModel", go_json_quote(&self.embedding_model)),
            ("EmbeddingDimensions", int_json(self.embedding_dimensions)),
            ("EmbeddingRPM", int_json(self.embedding_rpm)),
            ("EmbeddingTPM", int_json(self.embedding_tpm)),
            ("LogFile", go_json_quote(&self.log_file)),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "OpenAIConfig", |f| {
            field(f, "BaseURL", |v| {
                json_string(v, &mut self.base_url, "OpenAIConfig.BaseURL")
            })?;
            field(f, "AuthToken", |v| {
                json_string(v, &mut self.auth_token, "OpenAIConfig.AuthToken")
            })?;
            field(f, "ChatCompletionModel", |v| {
                json_string(
                    v,
                    &mut self.chat_completion_model,
                    "OpenAIConfig.ChatCompletionModel",
                )
            })?;
            field(f, "ChatCompletionRPM", |v| {
                json_int(
                    v,
                    &mut self.chat_completion_rpm,
                    "OpenAIConfig.ChatCompletionRPM",
                    "int",
                )
            })?;
            field(f, "ChatCompletionTPM", |v| {
                json_int(
                    v,
                    &mut self.chat_completion_tpm,
                    "OpenAIConfig.ChatCompletionTPM",
                    "int",
                )
            })?;
            field(f, "EmbeddingModel", |v| {
                json_string(v, &mut self.embedding_model, "OpenAIConfig.EmbeddingModel")
            })?;
            field(f, "EmbeddingDimensions", |v| {
                json_int(
                    v,
                    &mut self.embedding_dimensions,
                    "OpenAIConfig.EmbeddingDimensions",
                    "int",
                )
            })?;
            field(f, "EmbeddingRPM", |v| {
                json_int(
                    v,
                    &mut self.embedding_rpm,
                    "OpenAIConfig.EmbeddingRPM",
                    "int",
                )
            })?;
            field(f, "EmbeddingTPM", |v| {
                json_int(
                    v,
                    &mut self.embedding_tpm,
                    "OpenAIConfig.EmbeddingTPM",
                    "int",
                )
            })?;
            field(f, "LogFile", |v| {
                json_string(v, &mut self.log_file, "OpenAIConfig.LogFile")
            })
        })
    }
}

impl BlobConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("URI", go_json_quote(&self.uri)),
            ("S3", self.s3.marshal_json()),
            ("GCS", self.gcs.marshal_json()),
            ("Azure", self.azure.marshal_json()),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "BlobConfig", |f| {
            field(f, "URI", |v| {
                json_string(v, &mut self.uri, "BlobConfig.URI")
            })?;
            field(f, "S3", |v| self.s3.unmarshal_json(v))?;
            field(f, "GCS", |v| self.gcs.unmarshal_json(v))?;
            field(f, "Azure", |v| self.azure.unmarshal_json(v))
        })
    }
}

impl S3Config {
    fn marshal_json(&self) -> String {
        object(&[
            ("Endpoint", go_json_quote(&self.endpoint)),
            ("AccessKeyID", go_json_quote(&self.access_key_id)),
            ("SecretAccessKey", go_json_quote(&self.secret_access_key)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "S3Config", |f| {
            field(f, "Endpoint", |v| {
                json_string(v, &mut self.endpoint, "S3Config.Endpoint")
            })?;
            field(f, "AccessKeyID", |v| {
                json_string(v, &mut self.access_key_id, "S3Config.AccessKeyID")
            })?;
            field(f, "SecretAccessKey", |v| {
                json_string(v, &mut self.secret_access_key, "S3Config.SecretAccessKey")
            })
        })
    }
}

impl GCSConfig {
    fn marshal_json(&self) -> String {
        object(&[("CredentialsFile", go_json_quote(&self.credentials_file))])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "GCSConfig", |f| {
            field(f, "CredentialsFile", |v| {
                json_string(v, &mut self.credentials_file, "GCSConfig.CredentialsFile")
            })
        })
    }
}

impl AzureBlobConfig {
    fn marshal_json(&self) -> String {
        object(&[
            ("Endpoint", go_json_quote(&self.endpoint)),
            ("AccountName", go_json_quote(&self.account_name)),
            ("AccountKey", go_json_quote(&self.account_key)),
            ("ConnectionString", go_json_quote(&self.connection_string)),
        ])
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "AzureBlobConfig", |f| {
            field(f, "Endpoint", |v| {
                json_string(v, &mut self.endpoint, "AzureBlobConfig.Endpoint")
            })?;
            field(f, "AccountName", |v| {
                json_string(v, &mut self.account_name, "AzureBlobConfig.AccountName")
            })?;
            field(f, "AccountKey", |v| {
                json_string(v, &mut self.account_key, "AzureBlobConfig.AccountKey")
            })?;
            field(f, "ConnectionString", |v| {
                json_string(
                    v,
                    &mut self.connection_string,
                    "AzureBlobConfig.ConnectionString",
                )
            })
        })
    }
}

impl QuotaConfig {
    fn marshal_json(&self) -> Result<String> {
        Ok(object(&[
            ("MaxUsersCount", int_json(self.max_users_count)),
            ("MaxItemsCount", int_json(self.max_items_count)),
            ("MaxLabelsSize", int_json(self.max_labels_size)),
            ("MaxCommentSize", int_json(self.max_comment_size)),
            ("MaxCategoriesCount", int_json(self.max_categories_count)),
            ("MaxCategoriesSize", int_json(self.max_categories_size)),
        ]))
    }

    fn unmarshal_json(&mut self, value: &Json) -> Result<()> {
        json_struct(value, "QuotaConfig", |f| {
            field(f, "MaxUsersCount", |v| {
                json_int(
                    v,
                    &mut self.max_users_count,
                    "QuotaConfig.MaxUsersCount",
                    "int",
                )
            })?;
            field(f, "MaxItemsCount", |v| {
                json_int(
                    v,
                    &mut self.max_items_count,
                    "QuotaConfig.MaxItemsCount",
                    "int",
                )
            })?;
            field(f, "MaxLabelsSize", |v| {
                json_int(
                    v,
                    &mut self.max_labels_size,
                    "QuotaConfig.MaxLabelsSize",
                    "int",
                )
            })?;
            field(f, "MaxCommentSize", |v| {
                json_int(
                    v,
                    &mut self.max_comment_size,
                    "QuotaConfig.MaxCommentSize",
                    "int",
                )
            })?;
            field(f, "MaxCategoriesCount", |v| {
                json_int(
                    v,
                    &mut self.max_categories_count,
                    "QuotaConfig.MaxCategoriesCount",
                    "int",
                )
            })?;
            field(f, "MaxCategoriesSize", |v| {
                json_int(
                    v,
                    &mut self.max_categories_size,
                    "QuotaConfig.MaxCategoriesSize",
                    "int",
                )
            })
        })
    }
}

/// A `time.Duration` decoded from a JSON number of nanoseconds.
fn json_duration(value: &Json, out: &mut gorse_common::gotime::Duration, at: &str) -> Result<()> {
    json_int(value, &mut out.0, at, "time.Duration")
}
