// Ports: storage/scheme.go (prefixes, AppendURLParams, AppendMySQLParams, ProbeMySQLIsolationVariableName, TablePrefix, NewGORMConfig) @ add21b5

//! The storage URL prefixes, the helpers that add parameters to a store's
//! URL or DSN, the table names under a prefix, and the gorm settings the Go
//! SQL stores use. The `json_contains` function registered in the same Go
//! file is in [`crate::json_contains`].
//!
//! Go strings are bytes, so URLs, DSNs and table names are bytes here.

use std::collections::BTreeMap;

use gorse_common::gostrings;
use gorse_common::gourl;
use thiserror::Error;

use crate::gorm_naming::NamingStrategy;
use crate::mysql_dsn::{Config, DsnError, parse_dsn};

/// Go's `MySQLPrefix`.
pub const MYSQL_PREFIX: &str = "mysql://";
/// Go's `MongoPrefix`.
pub const MONGO_PREFIX: &str = "mongodb://";
/// Go's `MongoSrvPrefix`.
pub const MONGO_SRV_PREFIX: &str = "mongodb+srv://";
/// Go's `PostgresPrefix`.
pub const POSTGRES_PREFIX: &str = "postgres://";
/// Go's `PostgreSQLPrefix`.
pub const POSTGRESQL_PREFIX: &str = "postgresql://";
/// Go's `SQLitePrefix`.
pub const SQLITE_PREFIX: &str = "sqlite://";
/// Go's `XvecPrefix`.
pub const XVEC_PREFIX: &str = "xvec://";
/// Go's `RedisPrefix`.
pub const REDIS_PREFIX: &str = "redis://";
/// Go's `RedissPrefix`.
pub const REDISS_PREFIX: &str = "rediss://";
/// Go's `RedisClusterPrefix`.
pub const REDIS_CLUSTER_PREFIX: &str = "redis+cluster://";
/// Go's `RedissClusterPrefix`.
pub const REDISS_CLUSTER_PREFIX: &str = "rediss+cluster://";
/// Go's `QdrantPrefix`.
pub const QDRANT_PREFIX: &str = "qdrant://";
/// Go's `WeaviatePrefix`.
pub const WEAVIATE_PREFIX: &str = "weaviate://";
/// Go's `WeaviatesPrefix`.
pub const WEAVIATES_PREFIX: &str = "weaviates://";
/// Go's `MilvusPrefix`.
pub const MILVUS_PREFIX: &str = "milvus://";

/// Go's `AppendURLParams`: raw_url parsed as a URL, each parameter added to
/// its query in order, and the URL written back. The query comes out
/// re-encoded with its keys sorted (Go's `url.Values.Encode`), and a query
/// pair that does not parse is dropped.
pub fn append_url_params<K: AsRef<[u8]>, V: AsRef<[u8]>>(
    raw_url: impl AsRef<[u8]>,
    params: &[(K, V)],
) -> Result<Vec<u8>, gourl::Error> {
    let mut parsed = gourl::parse(raw_url)?;
    let mut q = parsed.query();
    for (key, value) in params {
        q.add(key, value);
    }
    parsed.raw_query = q.encode().into_bytes();
    Ok(parsed.string())
}

/// Go's `AppendMySQLParams`: the MySQL DSN with each parameter it does not
/// already have, formatted again by the driver. The keys are distinct, as
/// in the Go map.
///
/// # Panics
///
/// As [`parse_dsn`], on a DSN with a `strict` parameter.
pub fn append_mysql_params<K: AsRef<[u8]>, V: AsRef<[u8]>>(
    dsn: impl AsRef<[u8]>,
    params: impl IntoIterator<Item = (K, V)>,
) -> Result<Vec<u8>, DsnError> {
    let mut cfg = parse_dsn(dsn)?;
    let cfg_params = cfg.params.get_or_insert_with(BTreeMap::new);
    for (key, value) in params {
        cfg_params
            .entry(key.as_ref().to_vec())
            .or_insert_with(|| value.as_ref().to_vec());
    }
    Ok(cfg.format_dsn())
}

/// The query `ProbeMySQLIsolationVariableName` runs: MySQL 8 names the
/// variable `transaction_isolation`, MySQL 5.7 and MariaDB `tx_isolation`.
pub const MYSQL_ISOLATION_VARIABLE_QUERY: &str = "SHOW VARIABLES WHERE variable_name = 'transaction_isolation' OR variable_name = 'tx_isolation'";

/// Why [`probe_mysql_isolation_variable_name`] failed.
#[derive(Debug, Error)]
pub enum ProbeError<E> {
    /// The DSN does not parse (Go's `sql.Open` parses it first).
    #[error(transparent)]
    Dsn(DsnError),
    /// Connecting, querying or reading the row failed.
    #[error(transparent)]
    Query(E),
}

/// Go's `ProbeMySQLIsolationVariableName`: the name of the server's
/// isolation variable, or empty when the server has neither.
///
/// Go opens a connection with the driver; here `query` stands for it: given
/// the parsed DSN, it runs [`MYSQL_ISOLATION_VARIABLE_QUERY`] and returns the
/// `Variable_name` of the first row, `None` when there are no rows.
///
/// # Panics
///
/// As [`parse_dsn`], on a DSN with a `strict` parameter.
pub fn probe_mysql_isolation_variable_name<E>(
    dsn: impl AsRef<[u8]>,
    query: impl FnOnce(&Config, &str) -> Result<Option<Vec<u8>>, E>,
) -> Result<Vec<u8>, ProbeError<E>> {
    let cfg = parse_dsn(dsn).map_err(ProbeError::Dsn)?;
    let name = query(&cfg, MYSQL_ISOLATION_VARIABLE_QUERY).map_err(ProbeError::Query)?;
    Ok(name.unwrap_or_default())
}

/// Go's `TablePrefix`: the prefix of every table and key a store creates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TablePrefix(pub Vec<u8>);

impl TablePrefix {
    fn table(&self, name: &[u8]) -> Vec<u8> {
        let mut table = self.0.clone();
        table.extend_from_slice(name);
        table
    }

    /// Go's `ValuesTable`: `<prefix>values`.
    pub fn values_table(&self) -> Vec<u8> {
        self.table(b"values")
    }

    /// Go's `SetsTable`: `<prefix>sets`.
    pub fn sets_table(&self) -> Vec<u8> {
        self.table(b"sets")
    }

    /// Go's `DocumentTable`: `<prefix>documents`.
    pub fn document_table(&self) -> Vec<u8> {
        self.table(b"documents")
    }

    /// Go's `PointsTable`: `<prefix>time_series_points`.
    pub fn points_table(&self) -> Vec<u8> {
        self.table(b"time_series_points")
    }

    /// Go's `UsersTable`: `<prefix>users`.
    pub fn users_table(&self) -> Vec<u8> {
        self.table(b"users")
    }

    /// Go's `ItemsTable`: `<prefix>items`.
    pub fn items_table(&self) -> Vec<u8> {
        self.table(b"items")
    }

    /// Go's `FeedbackTable`: `<prefix>feedback`.
    pub fn feedback_table(&self) -> Vec<u8> {
        self.table(b"feedback")
    }

    /// Go's `Key`: `<prefix><key>`.
    pub fn key(&self, key: impl AsRef<[u8]>) -> Vec<u8> {
        self.table(key.as_ref())
    }
}

impl From<&str> for TablePrefix {
    fn from(prefix: &str) -> TablePrefix {
        TablePrefix(prefix.as_bytes().to_vec())
    }
}

/// The gorm settings [`new_gorm_config`] makes. Go also silences gorm's
/// logger, which has no counterpart here.
#[derive(Debug)]
pub struct GormConfig {
    /// gorm's `CreateBatchSize`: rows per `INSERT` when creating a slice.
    pub create_batch_size: i64,
    /// gorm's `SkipDefaultTransaction`: single writes are not wrapped in a
    /// transaction.
    pub skip_default_transaction: bool,
    /// gorm's `NamingStrategy`: how model and field names become table and
    /// column names.
    pub naming_strategy: NamingStrategy,
}

/// Go's `NewGORMConfig`: batches of 1000, no default transaction, and singular
/// table names under the prefix, with Gorse's model names replaced first:
/// `SQLValue` → `Values`, `SQLSet` → `Sets`, `SQLUser` → `Users`, `SQLItem` →
/// `Items`, `SQLFeedback` → `Feedback`, `SQLDocument` and `PostgresDocument`
/// → `Documents`, `TimeSeriesPoint` → `time_series_points`.
pub fn new_gorm_config(table_prefix: impl AsRef<[u8]>) -> GormConfig {
    GormConfig {
        create_batch_size: 1000,
        skip_default_transaction: true,
        naming_strategy: NamingStrategy {
            table_prefix: table_prefix.as_ref().to_vec(),
            name_replacer: Some(Box::new(gostrings::Replacer::new(&[
                ("SQLValue", "Values"),
                ("SQLSet", "Sets"),
                ("SQLUser", "Users"),
                ("SQLItem", "Items"),
                ("SQLFeedback", "Feedback"),
                ("SQLDocument", "Documents"),
                ("PostgresDocument", "Documents"),
                ("TimeSeriesPoint", "time_series_points"),
            ]))),
            ..NamingStrategy::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_the_first_row_name_or_empty() {
        let probe = |rows: Option<&str>| {
            probe_mysql_isolation_variable_name("u:p@tcp(h:3306)/d", |cfg, query| {
                assert_eq!(cfg.addr, b"h:3306");
                assert_eq!(query, MYSQL_ISOLATION_VARIABLE_QUERY);
                Ok::<_, String>(rows.map(|r| r.as_bytes().to_vec()))
            })
        };
        assert_eq!(
            probe(Some("transaction_isolation")).unwrap(),
            b"transaction_isolation"
        );
        assert_eq!(probe(None).unwrap(), b"");
        let failed = probe_mysql_isolation_variable_name("u:p@tcp(h:3306)/d", |_, _| {
            Err::<Option<Vec<u8>>, _>("connection refused".to_owned())
        });
        assert!(matches!(failed, Err(ProbeError::Query(e)) if e == "connection refused"));
    }

    #[test]
    fn probe_parses_the_dsn_before_querying() {
        let got = probe_mysql_isolation_variable_name("no slash", |_, _| -> Result<_, String> {
            panic!("queried with a DSN that does not parse")
        });
        assert!(
            matches!(got, Err(ProbeError::Dsn(DsnError::InvalidDsnNoSlash))),
            "{got:?}"
        );
    }
}
