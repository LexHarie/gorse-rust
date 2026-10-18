// Ports: github.com/go-sql-driver/mysql dsn.go (Config, ParseDSN, FormatDSN, normalize), utils.go (readBool, TLS config registry), auth.go (server public key registry), collations.go (unsafeCollations) @ v1.9.3 (the version add21b5's go.mod pins)
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Derived from Go-MySQL-Driver: Copyright 2016 The Go-MySQL-Driver Authors;
// the license text is in crates/gorse-storage-core/licenses/GO-SQL-DRIVER-MYSQL-LICENSE.

//! go-sql-driver/mysql's DSN handling: [`parse_dsn`], [`Config::format_dsn`]
//! and the normalization between them, which `storage.AppendMySQLParams`
//! uses to add parameters to a MySQL DSN.
//!
//! Go strings are bytes, so the DSN and every text field are bytes; error
//! texts show invalid UTF-8 replaced by U+FFFD and keep Go's bytes in their
//! fields.
//!
//! Only the DSN side of the driver is here. The connection side (`Logger`,
//! `DialFunc`, `BeforeConnect` and the functional options applied with
//! `Config.Apply`) is not ported: Gorse only parses and formats DSNs.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use gorse_common::gonet::{join_host_port, split_host_port};
use gorse_common::gostrconv::{NumError, atoi, quote};
use gorse_common::gotime::{
    Duration, Location, LocationError, ParseDurationError, load_location, parse_duration, utc,
};
use gorse_common::gourl::{EscapeError, path_escape, path_unescape, query_escape, query_unescape};
use thiserror::Error;

/// Go's `defaultMaxAllowedPacket` (const.go): 64 MiB.
pub const DEFAULT_MAX_ALLOWED_PACKET: i64 = 64 << 20;

/// The errors of this module, with Go's texts.
///
/// Where Go's text embeds a value, the variant keeps its bytes; the text
/// shows them with invalid UTF-8 replaced by U+FFFD, where Go writes them
/// raw. Variants that wrap with `%w` in Go report the wrapped error as their
/// [`std::error::Error::source`]; those that format it with `%v` do not.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DsnError {
    /// Go's `errInvalidDSNUnescaped`.
    #[error("invalid DSN: did you forget to escape a param value?")]
    InvalidDsnUnescaped,
    /// Go's `errInvalidDSNAddr`.
    #[error("invalid DSN: network address not terminated (missing closing brace)")]
    InvalidDsnAddr,
    /// Go's `errInvalidDSNNoSlash`.
    #[error("invalid DSN: missing the slash separating the database name")]
    InvalidDsnNoSlash,
    /// Go's `errInvalidDSNUnsafeCollation`.
    #[error("invalid DSN: interpolateParams can not be used with unsafe collations")]
    InvalidDsnUnsafeCollation,
    /// The database name is not a valid path escape (Go's
    /// `fmt.Errorf("invalid dbname %q: %w", dbname, err)`).
    #[error("invalid dbname {}: {source}", quote(.dbname))]
    InvalidDbName {
        /// The database name as it is in the DSN.
        dbname: Vec<u8>,
        /// The unescaping error.
        source: EscapeError,
    },
    /// An empty address on a network other than "tcp" or "unix".
    #[error("default addr for network '{}' unknown", lossy(.0))]
    UnknownDefaultAddr(Vec<u8>),
    /// A `tls` name that is neither a keyword nor registered with
    /// [`register_tls_config`].
    #[error("invalid value / unknown config name: {}", lossy(.0))]
    UnknownTlsConfig(Vec<u8>),
    /// A `serverPubKey` name not registered with
    /// [`register_server_pub_key`].
    #[error("invalid value / unknown server pub key name: {}", lossy(.0))]
    UnknownServerPubKey(Vec<u8>),
    /// A boolean parameter that `readBool` rejects.
    #[error("invalid bool value: {}", lossy(.0))]
    InvalidBool(Vec<u8>),
    /// The `loc` value, or the value of a parameter the driver does not
    /// know, is not a valid query escape (Go returns the `url.EscapeError`
    /// itself).
    #[error(transparent)]
    Escape(EscapeError),
    /// `time.LoadLocation` cannot load the `loc` value.
    #[error(transparent)]
    Location(LocationError),
    /// The `timeTruncate` value is not a duration (Go's
    /// `fmt.Errorf("invalid timeTruncate value: %v, error: %w", value, err)`).
    #[error("invalid timeTruncate value: {}, error: {source}", lossy(.value))]
    InvalidTimeTruncate {
        /// The value as it is in the DSN.
        value: Vec<u8>,
        /// The parsing error.
        source: ParseDurationError,
    },
    /// The `readTimeout`, `timeout` or `writeTimeout` value is not a
    /// duration (Go returns the `time.ParseDuration` error itself).
    #[error(transparent)]
    Duration(ParseDurationError),
    /// The `serverPubKey` value is not a valid query escape.
    #[error("invalid value for server pub key name: {0}")]
    InvalidServerPubKeyName(EscapeError),
    /// The `tls` value is not a valid query escape.
    #[error("invalid value for TLS config name: {0}")]
    InvalidTlsConfigName(EscapeError),
    /// The `maxAllowedPacket` value is not an `int` (Go returns the
    /// `strconv.Atoi` error itself).
    #[error(transparent)]
    MaxAllowedPacket(NumError),
    /// The `connectionAttributes` value is not a valid query escape.
    #[error("invalid connectionAttributes value: {0}")]
    InvalidConnectionAttributes(EscapeError),
    /// [`register_tls_config`] was given a key that `tls` reads as a keyword.
    #[error("key '{}' is reserved", lossy(.0))]
    ReservedTlsKey(Vec<u8>),
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The part of Go's `*tls.Config` that the DSN handling sets and reads.
///
/// Go's `tls.Config` has many more fields; only these two exist here because
/// Gorse never registers a TLS config, so a config only ever comes from the
/// `tls` keywords ("true", "skip-verify", "preferred").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsConfig {
    /// Go's `ServerName`: the host name to verify the certificate against.
    pub server_name: Vec<u8>,
    /// Go's `InsecureSkipVerify`: do not verify the server certificate.
    pub insecure_skip_verify: bool,
}

/// A server public key, standing in for Go's `*rsa.PublicKey`.
///
/// The bytes are kept as given and never interpreted; Gorse never registers
/// a key. Cloning shares the key and `==` compares identity, as Go copies
/// and compares `*rsa.PublicKey` pointers; [`ServerPubKey::bytes`] gives the
/// contents for a deep comparison.
#[derive(Debug, Clone)]
pub struct ServerPubKey(Arc<Vec<u8>>);

impl ServerPubKey {
    /// A new key holding `bytes`.
    pub fn new(bytes: impl Into<Vec<u8>>) -> ServerPubKey {
        ServerPubKey(Arc::new(bytes.into()))
    }

    /// The key's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// A new key with the same bytes, as `Config.Clone` copies the key into
    /// a new `rsa.PublicKey`.
    fn deep_copy(&self) -> ServerPubKey {
        ServerPubKey::new(self.bytes().to_vec())
    }
}

impl PartialEq for ServerPubKey {
    fn eq(&self, other: &ServerPubKey) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ServerPubKey {}

/// Go's `mysql.Config`: a configuration parsed from a DSN.
///
/// Build one with [`new_config`], which sets the defaults, as Go's doc
/// says; [`Config::default`] is Go's zero `Config{}`.
pub struct Config {
    /// Go's `User`: the user name.
    pub user: Vec<u8>,
    /// Go's `Passwd`: the password (requires a user).
    pub passwd: Vec<u8>,
    /// Go's `Net`: the network ("tcp", "tcp6", "unix"; default "tcp").
    pub net: Vec<u8>,
    /// Go's `Addr`: the address (default "127.0.0.1:3306" for "tcp" and
    /// "/tmp/mysql.sock" for "unix").
    pub addr: Vec<u8>,
    /// Go's `DBName`: the database name.
    pub db_name: Vec<u8>,
    /// Go's `Params`: the parameters the driver does not know, sent to the
    /// server as system variables. `None` is Go's nil map, which stays
    /// distinct from an empty one.
    pub params: Option<BTreeMap<Vec<u8>, Vec<u8>>>,
    /// Go's `ConnectionAttributes`: comma-separated `key:value` pairs.
    pub connection_attributes: Vec<u8>,
    /// Go's `Collation`: when set, the connection sends
    /// `SET NAMES <charset> COLLATE <collation>`.
    pub collation: Vec<u8>,
    /// Go's `Loc`: the location for `time.Time` values.
    pub loc: Location,
    /// Go's `MaxAllowedPacket`: the maximum packet size.
    pub max_allowed_packet: i64,
    /// Go's `ServerPubKey`: the name of a key registered with
    /// [`register_server_pub_key`].
    pub server_pub_key: Vec<u8>,
    /// Go's `TLSConfig`: "true", "false", "skip-verify", "preferred" or the
    /// name of a config registered with [`register_tls_config`].
    pub tls_config: Vec<u8>,
    /// Go's `TLS`: the TLS configuration; when set it takes priority over
    /// `tls_config`.
    pub tls: Option<TlsConfig>,
    /// Go's `Timeout`: the dial timeout.
    pub timeout: Duration,
    /// Go's `ReadTimeout`: the I/O read timeout.
    pub read_timeout: Duration,
    /// Go's `WriteTimeout`: the I/O write timeout.
    pub write_timeout: Duration,

    /// Go's `AllowAllFiles`: allow any file for `LOAD DATA LOCAL INFILE`.
    pub allow_all_files: bool,
    /// Go's `AllowCleartextPasswords`: allow the cleartext client plugin.
    pub allow_cleartext_passwords: bool,
    /// Go's `AllowFallbackToPlaintext`: fall back to an unencrypted
    /// connection when the server does not support TLS.
    pub allow_fallback_to_plaintext: bool,
    /// Go's `AllowNativePasswords`: allow native password authentication.
    pub allow_native_passwords: bool,
    /// Go's `AllowOldPasswords`: allow the old insecure password method.
    pub allow_old_passwords: bool,
    /// Go's `CheckConnLiveness`: check connections before using them.
    pub check_conn_liveness: bool,
    /// Go's `ClientFoundRows`: report matched rows instead of changed rows.
    pub client_found_rows: bool,
    /// Go's `ColumnsWithAlias`: prefix column names with the table alias.
    pub columns_with_alias: bool,
    /// Go's `InterpolateParams`: interpolate placeholders into the query.
    pub interpolate_params: bool,
    /// Go's `MultiStatements`: allow several statements in one query.
    pub multi_statements: bool,
    /// Go's `ParseTime`: parse time values to `time.Time`.
    pub parse_time: bool,
    /// Go's `RejectReadOnly`: reject read-only connections.
    pub reject_read_only: bool,

    // Go's unexported fields.
    compress: bool,
    pub_key: Option<ServerPubKey>,
    time_truncate: Duration,
    charsets: Option<Vec<Vec<u8>>>,
}

/// Go's `NewConfig`: a config with the default values.
pub fn new_config() -> Config {
    Config {
        loc: utc(),
        max_allowed_packet: DEFAULT_MAX_ALLOWED_PACKET,
        allow_native_passwords: true,
        check_conn_liveness: true,
        ..Config::default()
    }
}

/// Go's zero `Config{}`, which Go's own tests build directly. Go's zero
/// `Loc` is nil, which every use treats as UTC; here it is [`utc`].
impl Default for Config {
    fn default() -> Config {
        Config {
            user: Vec::new(),
            passwd: Vec::new(),
            net: Vec::new(),
            addr: Vec::new(),
            db_name: Vec::new(),
            params: None,
            connection_attributes: Vec::new(),
            collation: Vec::new(),
            loc: utc(),
            max_allowed_packet: 0,
            server_pub_key: Vec::new(),
            tls_config: Vec::new(),
            tls: None,
            timeout: Duration(0),
            read_timeout: Duration(0),
            write_timeout: Duration(0),
            allow_all_files: false,
            allow_cleartext_passwords: false,
            allow_fallback_to_plaintext: false,
            allow_native_passwords: false,
            allow_old_passwords: false,
            check_conn_liveness: false,
            client_found_rows: false,
            columns_with_alias: false,
            interpolate_params: false,
            multi_statements: false,
            parse_time: false,
            reject_read_only: false,
            compress: false,
            pub_key: None,
            time_truncate: Duration(0),
            charsets: None,
        }
    }
}

/// Go's `Config.Clone`: the TLS config, the parameters and the server
/// public key are copied, so the clone's key is a new key with the same
/// bytes.
impl Clone for Config {
    fn clone(&self) -> Config {
        Config {
            user: self.user.clone(),
            passwd: self.passwd.clone(),
            net: self.net.clone(),
            addr: self.addr.clone(),
            db_name: self.db_name.clone(),
            params: self.params.clone(),
            connection_attributes: self.connection_attributes.clone(),
            collation: self.collation.clone(),
            loc: self.loc.clone(),
            max_allowed_packet: self.max_allowed_packet,
            server_pub_key: self.server_pub_key.clone(),
            tls_config: self.tls_config.clone(),
            tls: self.tls.clone(),
            timeout: self.timeout,
            read_timeout: self.read_timeout,
            write_timeout: self.write_timeout,
            allow_all_files: self.allow_all_files,
            allow_cleartext_passwords: self.allow_cleartext_passwords,
            allow_fallback_to_plaintext: self.allow_fallback_to_plaintext,
            allow_native_passwords: self.allow_native_passwords,
            allow_old_passwords: self.allow_old_passwords,
            check_conn_liveness: self.check_conn_liveness,
            client_found_rows: self.client_found_rows,
            columns_with_alias: self.columns_with_alias,
            interpolate_params: self.interpolate_params,
            multi_statements: self.multi_statements,
            parse_time: self.parse_time,
            reject_read_only: self.reject_read_only,
            compress: self.compress,
            pub_key: self.pub_key.as_ref().map(ServerPubKey::deep_copy),
            time_truncate: self.time_truncate,
            charsets: self.charsets.clone(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        f.debug_struct("Config")
            .field("user", &text(&self.user))
            .field("passwd", &text(&self.passwd))
            .field("net", &text(&self.net))
            .field("addr", &text(&self.addr))
            .field("db_name", &text(&self.db_name))
            .field(
                "params",
                &self.params.as_ref().map(|p| {
                    p.iter()
                        .map(|(k, v)| (text(k), text(v)))
                        .collect::<BTreeMap<_, _>>()
                }),
            )
            .field("connection_attributes", &text(&self.connection_attributes))
            .field("collation", &text(&self.collation))
            .field("loc", &self.loc)
            .field("max_allowed_packet", &self.max_allowed_packet)
            .field("server_pub_key", &text(&self.server_pub_key))
            .field("tls_config", &text(&self.tls_config))
            .field("tls", &self.tls)
            .field("timeout", &self.timeout)
            .field("read_timeout", &self.read_timeout)
            .field("write_timeout", &self.write_timeout)
            .field("allow_all_files", &self.allow_all_files)
            .field("allow_cleartext_passwords", &self.allow_cleartext_passwords)
            .field(
                "allow_fallback_to_plaintext",
                &self.allow_fallback_to_plaintext,
            )
            .field("allow_native_passwords", &self.allow_native_passwords)
            .field("allow_old_passwords", &self.allow_old_passwords)
            .field("check_conn_liveness", &self.check_conn_liveness)
            .field("client_found_rows", &self.client_found_rows)
            .field("columns_with_alias", &self.columns_with_alias)
            .field("interpolate_params", &self.interpolate_params)
            .field("multi_statements", &self.multi_statements)
            .field("parse_time", &self.parse_time)
            .field("reject_read_only", &self.reject_read_only)
            .field("compress", &self.compress)
            .field("pub_key", &self.pub_key)
            .field("time_truncate", &self.time_truncate)
            .field(
                "charsets",
                &self
                    .charsets
                    .as_ref()
                    .map(|c| c.iter().map(|s| text(s)).collect::<Vec<_>>()),
            )
            .finish()
    }
}

/// Go's `writeDSNParam`.
fn write_dsn_param(buf: &mut Vec<u8>, has_param: &mut bool, name: &[u8], value: &[u8]) {
    buf.reserve(1 + name.len() + 1 + value.len());
    if !*has_param {
        *has_param = true;
        buf.push(b'?');
    } else {
        buf.push(b'&');
    }
    buf.extend_from_slice(name);
    buf.push(b'=');
    buf.extend_from_slice(value);
}

impl Config {
    /// Go's `Config.FormatDSN`: the DSN that [`parse_dsn`] reads back as
    /// this config. Parameters at their default are left out, the others
    /// come in a fixed order, then `params` sorted by name.
    pub fn format_dsn(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // [username[:password]@]
        if !self.user.is_empty() {
            buf.extend_from_slice(&self.user);
            if !self.passwd.is_empty() {
                buf.push(b':');
                buf.extend_from_slice(&self.passwd);
            }
            buf.push(b'@');
        }

        // [protocol[(address)]]
        if !self.net.is_empty() {
            buf.extend_from_slice(&self.net);
            if !self.addr.is_empty() {
                buf.push(b'(');
                buf.extend_from_slice(&self.addr);
                buf.push(b')');
            }
        }

        // /dbname
        buf.push(b'/');
        buf.extend_from_slice(path_escape(&self.db_name).as_bytes());

        // [?param1=value1&...&paramN=valueN]
        let mut has_param = false;
        let mut param = |buf: &mut Vec<u8>, name: &str, value: &[u8]| {
            write_dsn_param(buf, &mut has_param, name.as_bytes(), value);
        };

        if self.allow_all_files {
            param(&mut buf, "allowAllFiles", b"true");
        }
        if self.allow_cleartext_passwords {
            param(&mut buf, "allowCleartextPasswords", b"true");
        }
        if self.allow_fallback_to_plaintext {
            param(&mut buf, "allowFallbackToPlaintext", b"true");
        }
        if !self.allow_native_passwords {
            param(&mut buf, "allowNativePasswords", b"false");
        }
        if self.allow_old_passwords {
            param(&mut buf, "allowOldPasswords", b"true");
        }
        if !self.check_conn_liveness {
            param(&mut buf, "checkConnLiveness", b"false");
        }
        if self.client_found_rows {
            param(&mut buf, "clientFoundRows", b"true");
        }
        if let Some(charsets) = self.charsets.as_ref().filter(|c| !c.is_empty()) {
            param(&mut buf, "charset", &charsets.join(&b","[..]));
        }
        if !self.collation.is_empty() {
            param(&mut buf, "collation", &self.collation);
        }
        if self.columns_with_alias {
            param(&mut buf, "columnsWithAlias", b"true");
        }
        if !self.connection_attributes.is_empty() {
            let value = query_escape(&self.connection_attributes);
            param(&mut buf, "connectionAttributes", value.as_bytes());
        }
        if self.compress {
            param(&mut buf, "compress", b"true");
        }
        if self.interpolate_params {
            param(&mut buf, "interpolateParams", b"true");
        }
        // Go's `cfg.Loc != time.UTC && cfg.Loc != nil` compares pointers.
        if !self.loc.is_utc() {
            param(&mut buf, "loc", query_escape(self.loc.name()).as_bytes());
        }
        if self.multi_statements {
            param(&mut buf, "multiStatements", b"true");
        }
        if self.parse_time {
            param(&mut buf, "parseTime", b"true");
        }
        if self.time_truncate.0 > 0 {
            param(
                &mut buf,
                "timeTruncate",
                self.time_truncate.to_string().as_bytes(),
            );
        }
        if self.read_timeout.0 > 0 {
            param(
                &mut buf,
                "readTimeout",
                self.read_timeout.to_string().as_bytes(),
            );
        }
        if self.reject_read_only {
            param(&mut buf, "rejectReadOnly", b"true");
        }
        if !self.server_pub_key.is_empty() {
            let value = query_escape(&self.server_pub_key);
            param(&mut buf, "serverPubKey", value.as_bytes());
        }
        if self.timeout.0 > 0 {
            param(&mut buf, "timeout", self.timeout.to_string().as_bytes());
        }
        if !self.tls_config.is_empty() {
            param(&mut buf, "tls", query_escape(&self.tls_config).as_bytes());
        }
        if self.write_timeout.0 > 0 {
            param(
                &mut buf,
                "writeTimeout",
                self.write_timeout.to_string().as_bytes(),
            );
        }
        if self.max_allowed_packet != DEFAULT_MAX_ALLOWED_PACKET {
            let value = self.max_allowed_packet.to_string();
            param(&mut buf, "maxAllowedPacket", value.as_bytes());
        }

        // other params
        if let Some(params) = &self.params {
            for (name, value) in params {
                write_dsn_param(
                    &mut buf,
                    &mut has_param,
                    name,
                    query_escape(value).as_bytes(),
                );
            }
        }

        buf
    }

    /// Go's unexported `Config.normalize`, which [`parse_dsn`] ends with:
    /// fills in the default network and address, sets `tls` from
    /// `tls_config` and the server public key from `server_pub_key`.
    /// For the test ports, which call it on a `Config{}`.
    #[doc(hidden)]
    pub fn normalize(&mut self) -> Result<(), DsnError> {
        if self.interpolate_params
            && !self.collation.is_empty()
            && UNSAFE_COLLATIONS.contains(&self.collation.as_slice())
        {
            return Err(DsnError::InvalidDsnUnsafeCollation);
        }

        // Set default network if empty
        if self.net.is_empty() {
            self.net = b"tcp".to_vec();
        }

        // Set default address if empty
        if self.addr.is_empty() {
            self.addr = match self.net.as_slice() {
                b"tcp" => b"127.0.0.1:3306".to_vec(),
                b"unix" => b"/tmp/mysql.sock".to_vec(),
                _ => return Err(DsnError::UnknownDefaultAddr(self.net.clone())),
            };
        } else if self.net == b"tcp" {
            self.addr = ensure_have_port(&self.addr);
        }

        if self.tls.is_none() {
            match self.tls_config.as_slice() {
                b"false" | b"" => {
                    // don't set anything
                }
                b"true" => self.tls = Some(TlsConfig::default()),
                b"skip-verify" => {
                    self.tls = Some(TlsConfig {
                        insecure_skip_verify: true,
                        ..TlsConfig::default()
                    });
                }
                b"preferred" => {
                    self.tls = Some(TlsConfig {
                        insecure_skip_verify: true,
                        ..TlsConfig::default()
                    });
                    self.allow_fallback_to_plaintext = true;
                }
                name => {
                    self.tls = get_tls_config_clone(name);
                    if self.tls.is_none() {
                        return Err(DsnError::UnknownTlsConfig(name.to_vec()));
                    }
                }
            }
        }

        if let Some(tls) = &mut self.tls
            && tls.server_name.is_empty()
            && !tls.insecure_skip_verify
            && let Ok((host, _)) = split_host_port(&self.addr)
        {
            tls.server_name = host.to_vec();
        }

        if !self.server_pub_key.is_empty() {
            self.pub_key = get_server_pub_key(&self.server_pub_key);
            if self.pub_key.is_none() {
                return Err(DsnError::UnknownServerPubKey(self.server_pub_key.clone()));
            }
        }

        Ok(())
    }

    /// Go's unexported `charsets`: the `charset` parameter split at commas.
    /// For the test ports.
    #[doc(hidden)]
    pub fn charsets(&self) -> Option<&[Vec<u8>]> {
        self.charsets.as_deref()
    }

    /// Sets Go's unexported `charsets`, as Go's tests do in `Config`
    /// literals. For the test ports.
    #[doc(hidden)]
    pub fn set_charsets(&mut self, charsets: Option<Vec<Vec<u8>>>) {
        self.charsets = charsets;
    }

    /// Go's unexported `timeTruncate`. For the test ports.
    #[doc(hidden)]
    pub fn time_truncate(&self) -> Duration {
        self.time_truncate
    }

    /// Sets Go's unexported `timeTruncate`, as Go's tests do in `Config`
    /// literals. For the test ports.
    #[doc(hidden)]
    pub fn set_time_truncate(&mut self, d: Duration) {
        self.time_truncate = d;
    }

    /// Go's unexported `compress`. For the test ports.
    #[doc(hidden)]
    pub fn compress(&self) -> bool {
        self.compress
    }

    /// Go's unexported `pubKey`: the key [`Config::normalize`] looked up
    /// for `server_pub_key`. For the test ports.
    #[doc(hidden)]
    pub fn pub_key(&self) -> Option<&ServerPubKey> {
        self.pub_key.as_ref()
    }

    /// Reports whether two configs are equal the way Go's
    /// `reflect.DeepEqual` compares two `*Config`s: locations and server
    /// public keys are equal when they are the same or hold the same data,
    /// and a nil map or slice differs from an empty one. For the test ports.
    #[doc(hidden)]
    pub fn deep_equal(&self, other: &Config) -> bool {
        let loc_equal = self.loc == other.loc || self.loc.deep_equal(&other.loc);
        let pub_key_equal = match (&self.pub_key, &other.pub_key) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b || a.bytes() == b.bytes(),
            _ => false,
        };
        self.user == other.user
            && self.passwd == other.passwd
            && self.net == other.net
            && self.addr == other.addr
            && self.db_name == other.db_name
            && self.params == other.params
            && self.connection_attributes == other.connection_attributes
            && self.collation == other.collation
            && loc_equal
            && self.max_allowed_packet == other.max_allowed_packet
            && self.server_pub_key == other.server_pub_key
            && self.tls_config == other.tls_config
            && self.tls == other.tls
            && self.timeout == other.timeout
            && self.read_timeout == other.read_timeout
            && self.write_timeout == other.write_timeout
            && self.allow_all_files == other.allow_all_files
            && self.allow_cleartext_passwords == other.allow_cleartext_passwords
            && self.allow_fallback_to_plaintext == other.allow_fallback_to_plaintext
            && self.allow_native_passwords == other.allow_native_passwords
            && self.allow_old_passwords == other.allow_old_passwords
            && self.check_conn_liveness == other.check_conn_liveness
            && self.client_found_rows == other.client_found_rows
            && self.columns_with_alias == other.columns_with_alias
            && self.interpolate_params == other.interpolate_params
            && self.multi_statements == other.multi_statements
            && self.parse_time == other.parse_time
            && self.reject_read_only == other.reject_read_only
            && self.compress == other.compress
            && pub_key_equal
            && self.time_truncate == other.time_truncate
            && self.charsets == other.charsets
    }
}

/// Go's `ParseDSN`: parses
/// `[user[:password]@][net[(addr)]]/dbname[?param1=value1&paramN=valueN]`.
///
/// The user part ends at the last '@' before the last '/', and the password
/// starts at its first ':', so both may contain '@' and '/'. Parameter
/// values must be query-escaped, except the boolean, duration, charset and
/// collation ones, which are read as they are.
///
/// Where Go returns a parameter error together with the partly filled
/// config, this returns only the error.
///
/// # Panics
///
/// On a `strict` parameter, with Go's message: the driver removed strict
/// mode and panics rather than ignore it.
pub fn parse_dsn(dsn: impl AsRef<[u8]>) -> Result<Config, DsnError> {
    let dsn = dsn.as_ref();
    // New config with some default values
    let mut cfg = new_config();

    // [user[:password]@][net[(addr)]]/dbname[?param1=value1&paramN=valueN]
    // Find the last '/' (since the password or the net addr might contain a '/')
    let Some(i) = dsn.iter().rposition(|&b| b == b'/') else {
        if !dsn.is_empty() {
            return Err(DsnError::InvalidDsnNoSlash);
        }
        cfg.normalize()?;
        return Ok(cfg);
    };

    // left part is empty if i <= 0
    if i > 0 {
        // [username[:password]@][protocol[(address)]]
        // Find the last '@' in dsn[:i]
        let at = dsn[..i].iter().rposition(|&b| b == b'@');
        if let Some(j) = at {
            // username[:password]
            // Find the first ':' in dsn[:j]
            let k = dsn[..j].iter().position(|&b| b == b':');
            if let Some(k) = k {
                cfg.passwd = dsn[k + 1..j].to_vec();
            }
            cfg.user = dsn[..k.unwrap_or(j)].to_vec();
        }

        // [protocol[(address)]]
        // Find the first '(' in dsn[j+1:i]
        let start = at.map_or(0, |j| j + 1);
        let mut k = i;
        if let Some(p) = dsn[start..i].iter().position(|&b| b == b'(') {
            k = start + p;
            // dsn[i-1] must be == ')' if an address is specified
            if dsn[i - 1] != b')' {
                if dsn[k + 1..i].contains(&b')') {
                    return Err(DsnError::InvalidDsnUnescaped);
                }
                return Err(DsnError::InvalidDsnAddr);
            }
            cfg.addr = dsn[k + 1..i - 1].to_vec();
        }
        cfg.net = dsn[start..k].to_vec();
    }

    // dbname[?param1=value1&...&paramN=valueN]
    // Find the first '?' in dsn[i+1:]
    let mut j = dsn.len();
    if let Some(q) = dsn[i + 1..].iter().position(|&b| b == b'?') {
        j = i + 1 + q;
        parse_dsn_params(&mut cfg, &dsn[j + 1..])?;
    }

    let dbname = &dsn[i + 1..j];
    cfg.db_name = path_unescape(dbname).map_err(|source| DsnError::InvalidDbName {
        dbname: dbname.to_vec(),
        source,
    })?;

    cfg.normalize()?;
    Ok(cfg)
}

/// A boolean parameter: Go's `readBool`, or "invalid bool value".
fn bool_param(value: &[u8]) -> Result<bool, DsnError> {
    read_bool(value).ok_or_else(|| DsnError::InvalidBool(value.to_vec()))
}

/// Go's `parseDSNParams`: parses the DSN "query string". Pairs without '='
/// are ignored.
fn parse_dsn_params(cfg: &mut Config, params: &[u8]) -> Result<(), DsnError> {
    for v in params.split(|&b| b == b'&') {
        let Some(eq) = v.iter().position(|&b| b == b'=') else {
            continue;
        };
        let (key, value) = (&v[..eq], &v[eq + 1..]);

        // cfg params
        match key {
            // Disable INFILE allowlist / enable all files
            b"allowAllFiles" => cfg.allow_all_files = bool_param(value)?,

            // Use cleartext authentication mode (MySQL 5.5.10+)
            b"allowCleartextPasswords" => cfg.allow_cleartext_passwords = bool_param(value)?,

            // Allow fallback to unencrypted connection if server does not support TLS
            b"allowFallbackToPlaintext" => cfg.allow_fallback_to_plaintext = bool_param(value)?,

            // Use native password authentication
            b"allowNativePasswords" => cfg.allow_native_passwords = bool_param(value)?,

            // Use old authentication mode (pre MySQL 4.1)
            b"allowOldPasswords" => cfg.allow_old_passwords = bool_param(value)?,

            // Check connections for Liveness before using them
            b"checkConnLiveness" => cfg.check_conn_liveness = bool_param(value)?,

            // Switch "rowsAffected" mode
            b"clientFoundRows" => cfg.client_found_rows = bool_param(value)?,

            // charset
            b"charset" => {
                cfg.charsets = Some(value.split(|&b| b == b',').map(<[u8]>::to_vec).collect());
            }

            // Collation
            b"collation" => cfg.collation = value.to_vec(),

            b"columnsWithAlias" => cfg.columns_with_alias = bool_param(value)?,

            // Compression
            b"compress" => cfg.compress = bool_param(value)?,

            // Enable client side placeholder substitution
            b"interpolateParams" => cfg.interpolate_params = bool_param(value)?,

            // Time Location
            b"loc" => {
                let name = query_unescape(value).map_err(DsnError::Escape)?;
                // A zone name is text; a name that is not UTF-8 is looked
                // up with its invalid bytes replaced.
                cfg.loc =
                    load_location(&String::from_utf8_lossy(&name)).map_err(DsnError::Location)?;
            }

            // multiple statements in one query
            b"multiStatements" => cfg.multi_statements = bool_param(value)?,

            // time.Time parsing
            b"parseTime" => cfg.parse_time = bool_param(value)?,

            // time.Time truncation
            b"timeTruncate" => {
                cfg.time_truncate =
                    parse_duration(value).map_err(|source| DsnError::InvalidTimeTruncate {
                        value: value.to_vec(),
                        source,
                    })?;
            }

            // I/O read Timeout
            b"readTimeout" => {
                cfg.read_timeout = parse_duration(value).map_err(DsnError::Duration)?
            }

            // Reject read-only connections
            b"rejectReadOnly" => cfg.reject_read_only = bool_param(value)?,

            // Server public key
            b"serverPubKey" => {
                cfg.server_pub_key =
                    query_unescape(value).map_err(DsnError::InvalidServerPubKeyName)?;
            }

            // Strict mode
            b"strict" => panic!(
                "strict mode has been removed. See https://github.com/go-sql-driver/mysql/wiki/strict-mode"
            ),

            // Dial Timeout
            b"timeout" => cfg.timeout = parse_duration(value).map_err(DsnError::Duration)?,

            // TLS-Encryption
            b"tls" => {
                cfg.tls_config = if let Some(bool_value) = read_bool(value) {
                    if bool_value {
                        b"true".to_vec()
                    } else {
                        b"false".to_vec()
                    }
                } else if let Some(vl) = tls_keyword(value) {
                    vl.to_vec()
                } else {
                    query_unescape(value).map_err(DsnError::InvalidTlsConfigName)?
                };
            }

            // I/O write Timeout
            b"writeTimeout" => {
                cfg.write_timeout = parse_duration(value).map_err(DsnError::Duration)?;
            }
            b"maxAllowedPacket" => {
                let (n, err) = atoi(value);
                if let Some(err) = err {
                    return Err(DsnError::MaxAllowedPacket(err));
                }
                cfg.max_allowed_packet = n;
            }

            // Connection attributes
            b"connectionAttributes" => {
                cfg.connection_attributes =
                    query_unescape(value).map_err(DsnError::InvalidConnectionAttributes)?;
            }

            _ => {
                // lazy init
                let params = cfg.params.get_or_insert_with(BTreeMap::new);
                let value = query_unescape(value).map_err(DsnError::Escape)?;
                params.insert(key.to_vec(), value);
            }
        }
    }

    Ok(())
}

/// Go's `ensureHavePort`: appends MySQL's default port 3306 to an address
/// that `net.SplitHostPort` rejects.
fn ensure_have_port(addr: &[u8]) -> Vec<u8> {
    if split_host_port(addr).is_err() {
        return join_host_port(addr, b"3306");
    }
    addr.to_vec()
}

/// Go's `readBool`: the boolean a parameter value spells, if it is one of
/// "1", "true", "TRUE", "True", "0", "false", "FALSE" or "False".
fn read_bool(input: &[u8]) -> Option<bool> {
    match input {
        b"1" | b"true" | b"TRUE" | b"True" => Some(true),
        b"0" | b"false" | b"FALSE" | b"False" => Some(false),
        // Not a valid bool value
        _ => None,
    }
}

/// The `tls` keyword `value` lowers to, Go's
/// `vl := strings.ToLower(value); vl == "skip-verify" || vl == "preferred"`.
fn tls_keyword(value: &[u8]) -> Option<&'static [u8]> {
    [&b"skip-verify"[..], b"preferred"]
        .into_iter()
        .find(|keyword| go_to_lower_equals(value, keyword))
}

/// Reports whether Go's `strings.ToLower(s)` is the ASCII string `target`.
///
/// Go lowers rune by rune with `unicode.ToLower`, decoding invalid UTF-8 as
/// U+FFFD, which lowers to itself. Besides 'A' to 'Z', the only runes it
/// lowers to ASCII are U+0130 (to 'i') and U+212A, the Kelvin sign (to 'k'):
/// the `LOWER_TO_ASCII` table in gorse-common's `dateparse/tables.rs`, which
/// is generated from Go's tables. Any other non-ASCII rune lowers to a
/// non-ASCII rune and so cannot match.
fn go_to_lower_equals(s: &[u8], target: &[u8]) -> bool {
    let mut lowered = Vec::with_capacity(s.len());
    for chunk in s.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\0'..='\x7f' => lowered.push(c.to_ascii_lowercase() as u8),
                '\u{130}' => lowered.push(b'i'),
                '\u{212a}' => lowered.push(b'k'),
                _ => return false,
            }
        }
        if !chunk.invalid().is_empty() {
            // U+FFFD
            return false;
        }
    }
    lowered == target
}

/// Go's `unsafeCollations` (collations.go): collations whose multibyte
/// characters can contain the byte of '\\', so client-side interpolation
/// cannot escape them safely.
const UNSAFE_COLLATIONS: [&[u8]; 12] = [
    b"big5_chinese_ci",
    b"sjis_japanese_ci",
    b"gbk_chinese_ci",
    b"big5_bin",
    b"gb2312_bin",
    b"gbk_bin",
    b"sjis_bin",
    b"cp932_japanese_ci",
    b"cp932_bin",
    b"gb18030_chinese_ci",
    b"gb18030_bin",
    b"gb18030_unicode_520_ci",
];

static TLS_CONFIG_REGISTRY: RwLock<BTreeMap<Vec<u8>, TlsConfig>> = RwLock::new(BTreeMap::new());

/// Go's `RegisterTLSConfig`: registers a TLS config that a DSN selects with
/// `tls=<key>`. Keys `tls` reads as keywords ("true", "skip-verify", ...) are
/// reserved.
///
/// Go keeps the caller's `*tls.Config`, so later changes to it reach the
/// DSNs parsed afterwards; here the registry keeps its own copy, and
/// registering again replaces it.
pub fn register_tls_config(key: impl AsRef<[u8]>, config: TlsConfig) -> Result<(), DsnError> {
    let key = key.as_ref();
    if read_bool(key).is_some() || tls_keyword(key).is_some() {
        return Err(DsnError::ReservedTlsKey(key.to_vec()));
    }

    TLS_CONFIG_REGISTRY
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.to_vec(), config);
    Ok(())
}

/// Go's `DeregisterTLSConfig`: removes the TLS config registered as `key`.
pub fn deregister_tls_config(key: impl AsRef<[u8]>) {
    TLS_CONFIG_REGISTRY
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(key.as_ref());
}

/// Go's unexported `getTLSConfigClone`: a copy of the TLS config registered
/// as `key`. For the test ports.
#[doc(hidden)]
pub fn get_tls_config_clone(key: impl AsRef<[u8]>) -> Option<TlsConfig> {
    TLS_CONFIG_REGISTRY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(key.as_ref())
        .cloned()
}

static SERVER_PUB_KEY_REGISTRY: RwLock<BTreeMap<Vec<u8>, ServerPubKey>> =
    RwLock::new(BTreeMap::new());

/// Go's `RegisterServerPubKey`: registers a server public key that a DSN
/// selects with `serverPubKey=<name>`.
pub fn register_server_pub_key(name: impl AsRef<[u8]>, pub_key: ServerPubKey) {
    SERVER_PUB_KEY_REGISTRY
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.as_ref().to_vec(), pub_key);
}

/// Go's `DeregisterServerPubKey`: removes the key registered as `name`.
pub fn deregister_server_pub_key(name: impl AsRef<[u8]>) {
    SERVER_PUB_KEY_REGISTRY
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(name.as_ref());
}

/// Go's `getServerPubKey`: the key registered as `name` itself.
fn get_server_pub_key(name: &[u8]) -> Option<ServerPubKey> {
    SERVER_PUB_KEY_REGISTRY
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
}
