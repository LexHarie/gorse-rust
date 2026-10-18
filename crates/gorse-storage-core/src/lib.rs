// Ports: storage/scheme.go, storage/options.go, storage/errors.go @ add21b5

//! Backend URL prefixes, storage options, table-prefix helpers and storage
//! error kinds: the Go `storage` root package.
//!
//! This is a separate crate because `config` imports it while `storage/data`
//! and `storage/blob` import `config`; see `porting/crate-selection.md`.
//!
//! `storage/scheme.go` leans on three libraries whose behaviour the stores
//! depend on, so their parts are ported here too: go-sql-driver/mysql's DSN
//! parser ([`mysql_dsn`], for `AppendMySQLParams`), gorm's naming strategy
//! ([`gorm_naming`], for `NewGORMConfig`) and the `encoding/json` decoding
//! behind the SQLite `json_contains` function ([`json_contains`]).

pub mod errors;
pub mod gorm_naming;
pub mod json_contains;
pub mod mysql_dsn;
pub mod options;
pub mod scheme;
