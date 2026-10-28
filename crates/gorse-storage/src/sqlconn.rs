// Ports: storage/scheme.go (AppendMySQLParams, ProbeMySQLIsolationVariableName,
// ApplySQLPool, and the json_contains function Gorse registers with its SQLite
// driver) @ add21b5

//! The connection glue the SQL stores share: a MySQL DSN as the connection
//! options and the session variables it sets, the pool limits Go applies, and
//! the SQLite connection with `json_contains` registered on it.
//!
//! Go's `database/sql` and its drivers are not ported. This module is the part
//! of them the stores depend on, so that the MySQL, Postgres and SQLite stores
//! of the cache and data packages connect the same way (D-18, O-6, O-7).

#[cfg(any(feature = "mysql", feature = "postgres"))]
use gorse_common::gotime::Duration;
#[cfg(feature = "mysql")]
use gorse_common::mysql_dsn;
use gorse_common::mysql_dsn::DsnError;
#[cfg(any(feature = "mysql", feature = "postgres"))]
use gorse_storage_core::options::{Options, SqlPool, apply_sql_pool};

/// Why a connection string could not be turned into connection options.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The DSN does not parse (Go's `mysql.ParseDSN` error).
    #[error(transparent)]
    Dsn(#[from] DsnError),
    /// The DSN asks for something this port does not provide.
    #[error("{0}")]
    Unsupported(String),
}

/// The pool limits `storage.ApplySQLPool` sets. Go's `MaxIdleConns` has no
/// counterpart in sqlx, so it is read and dropped.
#[cfg(any(feature = "mysql", feature = "postgres"))]
#[derive(Debug, Default)]
struct PoolSettings {
    max_open: Option<u32>,
    max_lifetime: Option<std::time::Duration>,
}

#[cfg(any(feature = "mysql", feature = "postgres"))]
impl SqlPool for PoolSettings {
    fn set_max_open_conns(&mut self, n: i64) {
        self.max_open = u32::try_from(n).ok();
    }

    fn set_max_idle_conns(&mut self, _n: i64) {}

    fn set_conn_max_lifetime(&mut self, d: Duration) {
        self.max_lifetime = u64::try_from(d.0).ok().map(std::time::Duration::from_nanos);
    }
}

/// The MySQL pool options for the store's `Options`, as Go's `ApplySQLPool`
/// applies them.
#[cfg(feature = "mysql")]
pub fn mysql_pool_options(opts: &Options) -> sqlx::mysql::MySqlPoolOptions {
    let settings = pool_settings(opts);
    let mut pool = sqlx::mysql::MySqlPoolOptions::new();
    if let Some(max) = settings.max_open {
        pool = pool.max_connections(max);
    }
    if let Some(lifetime) = settings.max_lifetime {
        pool = pool.max_lifetime(lifetime);
    }
    pool
}

/// The Postgres pool options for the store's `Options`, as Go's `ApplySQLPool`
/// applies them.
#[cfg(feature = "postgres")]
pub fn postgres_pool_options(opts: &Options) -> sqlx::postgres::PgPoolOptions {
    let settings = pool_settings(opts);
    let mut pool = sqlx::postgres::PgPoolOptions::new();
    if let Some(max) = settings.max_open {
        pool = pool.max_connections(max);
    }
    if let Some(lifetime) = settings.max_lifetime {
        pool = pool.max_lifetime(lifetime);
    }
    pool
}

#[cfg(any(feature = "mysql", feature = "postgres"))]
fn pool_settings(opts: &Options) -> PoolSettings {
    let mut settings = PoolSettings::default();
    apply_sql_pool(&mut settings, opts);
    settings
}

/// The connection options of a MySQL DSN (the part after `mysql://`), and the
/// session variables Go's driver sets on every new connection.
///
/// Go's driver takes the parameters it does not know as session variables. It
/// also takes the transaction isolation Go appends to the DSN (see
/// [`mysql_after_connect`]). The parameters the driver does know are read here,
/// and a TLS mode the port cannot honour is an error, not a silent plain
/// connection.
#[cfg(feature = "mysql")]
pub fn mysql_connect_options(
    body: &str,
) -> Result<(sqlx::mysql::MySqlConnectOptions, Vec<(String, String)>), Error> {
    use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

    let mut cfg = mysql_dsn::parse_dsn(body)?;
    cfg.normalize()?;
    if cfg.tls.is_some() {
        return Err(Error::Unsupported(
            "a registered TLS configuration is not supported by the SQL stores".to_owned(),
        ));
    }
    let ssl_mode = match cfg.tls_config.as_slice() {
        b"" | b"false" => MySqlSslMode::Disabled,
        b"preferred" => MySqlSslMode::Preferred,
        b"skip-verify" => MySqlSslMode::Required,
        b"true" => MySqlSslMode::VerifyIdentity,
        other => {
            return Err(Error::Unsupported(format!(
                "unsupported TLS setting {}",
                String::from_utf8_lossy(other)
            )));
        }
    };
    let mut options = MySqlConnectOptions::new()
        .username(&String::from_utf8_lossy(&cfg.user))
        .password(&String::from_utf8_lossy(&cfg.passwd))
        .database(&String::from_utf8_lossy(&cfg.db_name))
        .ssl_mode(ssl_mode);
    if cfg.net == b"unix" {
        options = options.socket(String::from_utf8_lossy(&cfg.addr).as_ref());
    } else {
        let addr = String::from_utf8_lossy(&cfg.addr).into_owned();
        let (host, port) = addr
            .rsplit_once(':')
            .ok_or_else(|| Error::Unsupported(format!("address {addr} has no port")))?;
        let port: u16 = port
            .parse()
            .map_err(|_| Error::Unsupported(format!("address {addr} has no port")))?;
        options = options
            .host(host.trim_start_matches('[').trim_end_matches(']'))
            .port(port);
    }
    let mut sets = Vec::new();
    for (key, value) in cfg.params.iter().flatten() {
        let key = String::from_utf8_lossy(key).into_owned();
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Unsupported(format!(
                "session variable {key} is not a plain name"
            )));
        }
        sets.push((key, String::from_utf8_lossy(value).into_owned()));
    }
    Ok((options, sets))
}

/// Sets the session of a new MySQL connection, as Go's driver does on connect:
/// the transaction isolation level under the name the server uses (Go probes
/// it once, here it is read on each connection), unless the DSN sets that
/// variable itself, then each parameter of the DSN.
#[cfg(feature = "mysql")]
pub async fn mysql_after_connect(
    conn: &mut sqlx::MySqlConnection,
    sets: &[(String, String)],
    isolation: &str,
) -> Result<(), sqlx::Error> {
    use gorse_storage_core::scheme::MYSQL_ISOLATION_VARIABLE_QUERY;
    use sqlx::Row;

    let row = sqlx::query(MYSQL_ISOLATION_VARIABLE_QUERY)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(row) = row {
        // The first column is `Variable_name`.
        let name: String = row.try_get(0)?;
        if !name.is_empty() && !sets.iter().any(|(key, _)| *key == name) {
            set_session(conn, &name, isolation).await?;
        }
    }
    for (key, value) in sets {
        set_session(conn, key, value).await?;
    }
    Ok(())
}

#[cfg(feature = "mysql")]
async fn set_session(
    conn: &mut sqlx::MySqlConnection,
    key: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    // A MySQL string literal escapes quotes and backslashes.
    let value = value.replace('\\', "\\\\").replace('\'', "''");
    let sql = format!("SET SESSION {key} = '{value}'");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Registers `json_contains(a, b)` on a SQLite connection, as Go's SQLite
/// driver does on every connection (Q-25). It is deterministic, and its
/// arguments and errors follow modernc's rules (Q-108, O-6).
#[cfg(feature = "sqlite")]
pub fn register_json_contains(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use gorse_storage_core::json_contains::{self, Arg, modernc_error_text, modernc_text};
    use rusqlite::functions::FunctionFlags;
    use rusqlite::types::ValueRef;

    fn argument(value: ValueRef<'_>) -> Arg<'_> {
        match value {
            ValueRef::Null => Arg::Null,
            ValueRef::Integer(n) => Arg::Integer(n),
            ValueRef::Real(f) => Arg::Real(f),
            ValueRef::Text(text) => Arg::Text(modernc_text(text)),
            ValueRef::Blob(blob) => Arg::Blob(blob),
        }
    }

    conn.create_scalar_function(
        "json_contains",
        2,
        FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let a = argument(ctx.get_raw(0));
            let b = argument(ctx.get_raw(1));
            json_contains::json_contains(a, b).map_err(|err| {
                rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                    modernc_error_text(&err),
                )))
            })
        },
    )
}
