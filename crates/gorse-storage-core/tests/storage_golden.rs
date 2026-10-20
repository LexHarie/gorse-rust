//! gorse-storage-core against `tests/data/storage_golden.json`, which
//! `contracts/tools/storagegolden` wrote by running go1.27.0 on Gorse's
//! storage package (add21b5) and the libraries it calls: the sentinel errors,
//! `NewOptions`, `TablePrefix`, gorm's naming strategy under
//! `NewGORMConfig`, `json_contains` through modernc's SQLite,
//! `AppendURLParams`, and go-sql-driver/mysql's `ParseDSN` and `FormatDSN`
//! with `AppendMySQLParams`.
//!
//! Texts that Go keeps as bytes are compared as bytes.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use gorse_common::gostrconv::quote;
use gorse_common::gotime::{self, Duration, SECOND, internal};
use gorse_storage_core::errors::Error;
use gorse_storage_core::gorm_naming::{COMMON_INITIALISMS, NamingStrategy};
use gorse_storage_core::json_contains::{Arg, json_contains, modernc_error_text, modernc_text};
use gorse_storage_core::mysql_dsn::{Config, parse_dsn};
use gorse_storage_core::options::{
    Opt, Options, new_options, with_conn_max_lifetime, with_isolation_level, with_max_idle_conns,
    with_max_open_conns, with_max_search_results, with_num_jobs, with_redis_client_name,
    with_skip_unindexed_segments,
};
use gorse_storage_core::scheme::{
    TablePrefix, append_mysql_params, append_url_params, new_gorm_config,
};
use serde::Deserialize;

/// A Go string: a JSON string when it is valid UTF-8, else its bytes in hex.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum GoString {
    Str(String),
    Hex { hex: String },
}

impl GoString {
    fn bytes(&self) -> Vec<u8> {
        match self {
            GoString::Str(s) => s.as_bytes().to_vec(),
            GoString::Hex { hex } => hex::decode(hex).expect("hex"),
        }
    }
}

fn golden() -> &'static Golden {
    static GOLDEN: std::sync::OnceLock<Golden> = std::sync::OnceLock::new();
    GOLDEN.get_or_init(|| {
        let path = format!(
            "{}/tests/data/storage_golden.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let g: Golden = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(g.go_version, "go1.27.0");
        g
    })
}

#[derive(Deserialize)]
struct Golden {
    go_version: String,
    errors: BTreeMap<String, String>,
    options: Vec<OptionsCase>,
    table_prefix: Vec<TablePrefixCase>,
    naming: Naming,
    json_contains: Vec<JsonContainsCase>,
    append_url_params: Vec<AppendUrlParams>,
    mysql_dsn: MysqlDsn,
}

/// Collects failures so that one run reports all of them.
#[derive(Default)]
struct Failures(Vec<String>);

impl Failures {
    fn check<T: PartialEq + std::fmt::Debug>(
        &mut self,
        what: impl FnOnce() -> String,
        got: T,
        want: T,
    ) {
        if got != want {
            self.0
                .push(format!("{}: got {got:?}, Go has {want:?}", what()));
        }
    }

    fn bytes(&mut self, what: impl FnOnce() -> String, got: &[u8], want: &GoString) {
        let want = want.bytes();
        if got != want {
            self.0.push(format!(
                "{}: got {}, Go has {}",
                what(),
                quote(got),
                quote(&want)
            ));
        }
    }

    fn finish(self, total: usize) {
        assert!(
            self.0.is_empty(),
            "{} of {total} checks differ from Go:\n{}",
            self.0.len(),
            self.0.join("\n")
        );
    }
}

// ------------------------------------------------------------------ errors

#[test]
fn errors_match_go() {
    let want = &golden().errors;
    let got: BTreeMap<String, String> = [
        ("ErrNotFound", Error::NotFound),
        ("ErrNoDatabase", Error::NoDatabase),
        ("ErrInvalidArgument", Error::InvalidArgument),
        ("ErrNotSupported", Error::NotSupported),
        ("ErrAlreadyExists", Error::AlreadyExists),
    ]
    .into_iter()
    .map(|(name, e)| (name.to_owned(), e.to_string()))
    .collect();
    assert_eq!(&got, want);
}

// ----------------------------------------------------------------- options

#[derive(Deserialize)]
struct OptionsCase {
    case: String,
    options: OptionsOut,
}

#[derive(Deserialize, Debug, PartialEq)]
struct OptionsOut {
    isolation_level: String,
    max_open_conns: i64,
    max_idle_conns: i64,
    conn_max_lifetime: i64,
    max_search_results: i64,
    redis_client_name: String,
    num_jobs: i64,
    skip_unindexed_segments: bool,
}

impl From<Options> for OptionsOut {
    fn from(o: Options) -> OptionsOut {
        OptionsOut {
            isolation_level: o.isolation_level,
            max_open_conns: o.max_open_conns,
            max_idle_conns: o.max_idle_conns,
            conn_max_lifetime: o.conn_max_lifetime.0,
            max_search_results: o.max_search_results,
            redis_client_name: o.redis_client_name,
            num_jobs: o.num_jobs,
            skip_unindexed_segments: o.skip_unindexed_segments,
        }
    }
}

#[test]
fn options_match_go() {
    // The option lists the golden tool applies, by case name.
    let cases: Vec<(&str, Vec<Opt>)> = vec![
        ("none", vec![]),
        (
            "all",
            vec![
                with_num_jobs(4),
                with_skip_unindexed_segments(true),
                with_isolation_level("SERIALIZABLE"),
                with_max_open_conns(10),
                with_max_idle_conns(5),
                with_conn_max_lifetime(Duration(90 * SECOND.0)),
                with_max_search_results(7),
                with_redis_client_name("gorse"),
            ],
        ),
        (
            "last wins",
            vec![with_num_jobs(1), with_num_jobs(2), with_isolation_level("")],
        ),
        (
            "negative",
            vec![
                with_max_open_conns(-1),
                with_max_idle_conns(-2),
                with_conn_max_lifetime(Duration(-SECOND.0)),
                with_max_search_results(-3),
            ],
        ),
    ];
    let golden = &golden().options;
    assert_eq!(
        golden.iter().map(|c| c.case.as_str()).collect::<Vec<_>>(),
        cases.iter().map(|c| c.0).collect::<Vec<_>>()
    );
    for (c, (name, opts)) in golden.iter().zip(&cases) {
        assert_eq!(OptionsOut::from(new_options(opts)), c.options, "{name}");
    }
}

// ------------------------------------------------------------ table prefix

#[derive(Deserialize)]
struct TablePrefixCase {
    prefix: GoString,
    values: GoString,
    sets: GoString,
    documents: GoString,
    points: GoString,
    users: GoString,
    items: GoString,
    feedback: GoString,
    key: GoString,
    key_empty: GoString,
}

#[test]
fn table_prefix_matches_go() {
    let mut f = Failures::default();
    let cases = &golden().table_prefix;
    for c in cases {
        let tp = TablePrefix(c.prefix.bytes());
        let p = quote(&tp.0);
        let checks: [(&str, Vec<u8>, &GoString); 9] = [
            ("ValuesTable", tp.values_table(), &c.values),
            ("SetsTable", tp.sets_table(), &c.sets),
            ("DocumentTable", tp.document_table(), &c.documents),
            ("PointsTable", tp.points_table(), &c.points),
            ("UsersTable", tp.users_table(), &c.users),
            ("ItemsTable", tp.items_table(), &c.items),
            ("FeedbackTable", tp.feedback_table(), &c.feedback),
            ("Key(\"key\")", tp.key("key"), &c.key),
            ("Key(\"\")", tp.key(""), &c.key_empty),
        ];
        for (name, got, want) in checks {
            f.bytes(|| format!("{p}.{name}"), &got, want);
        }
    }
    f.finish(cases.len() * 9);
}

// ------------------------------------------------------------------ naming

#[derive(Deserialize)]
struct Naming {
    initialisms: Vec<(String, String)>,
    gorse: Vec<NamingStrategyCase>,
    plain: Vec<PlainName>,
}

#[derive(Deserialize)]
struct NamingStrategyCase {
    table_prefix: String,
    tables: Vec<GoString>,
    names: Vec<NameCase>,
}

#[derive(Deserialize)]
struct NameCase {
    name: GoString,
    table: GoString,
    column: GoString,
    join_table: GoString,
    index: Vec<GoString>,
    unique: Vec<GoString>,
    checker: Vec<GoString>,
}

#[derive(Deserialize)]
struct PlainName {
    name: GoString,
    column: GoString,
}

#[test]
fn naming_matches_go() {
    let naming = &golden().naming;
    // The initialisms gorm's init gives its replacer, in order.
    assert_eq!(
        naming
            .initialisms
            .iter()
            .map(|p| p.0.as_str())
            .collect::<Vec<_>>(),
        COMMON_INITIALISMS
    );

    let mut f = Failures::default();
    let mut total = 0;
    for s in &naming.gorse {
        let ns = new_gorm_config(&s.table_prefix).naming_strategy;
        let tables: Vec<Vec<u8>> = vec![
            ns.table_name("SQLDocument"),
            [s.table_prefix.as_bytes(), b"items"].concat(),
        ];
        for (got, want) in tables.iter().zip(&s.tables) {
            f.bytes(|| format!("{:?} tables", s.table_prefix), got, want);
        }
        for c in &s.names {
            let n = c.name.bytes();
            let what = |m: &str| format!("prefix {:?} {m}({})", s.table_prefix, quote(&n));
            f.bytes(|| what("TableName"), &ns.table_name(&n), &c.table);
            f.bytes(|| what("ColumnName"), &ns.column_name("", &n), &c.column);
            f.bytes(
                || what("JoinTableName"),
                &ns.join_table_name(&n),
                &c.join_table,
            );
            for (i, t) in tables.iter().enumerate() {
                let tq = quote(t);
                f.bytes(
                    || format!("{} on {tq}", what("IndexName")),
                    &ns.index_name(t, &n),
                    &c.index[i],
                );
                f.bytes(
                    || format!("{} on {tq}", what("UniqueName")),
                    &ns.unique_name(t, &n),
                    &c.unique[i],
                );
                f.bytes(
                    || format!("{} on {tq}", what("CheckerName")),
                    &ns.checker_name(t, &n),
                    &c.checker[i],
                );
            }
            total += 3 + 3 * tables.len();
        }
    }
    let plain = NamingStrategy::default();
    for c in &naming.plain {
        let n = c.name.bytes();
        f.bytes(
            || format!("zero NamingStrategy ColumnName({})", quote(&n)),
            &plain.column_name("", &n),
            &c.column,
        );
        total += 1;
    }
    assert!(total > 2000, "corpus too small");
    f.finish(total);
}

// ----------------------------------------------------------- json_contains

#[derive(Deserialize)]
struct JsonContainsCase {
    args: [GoArg; 2],
    null: Option<bool>,
    int: Option<i64>,
    err: Option<GoString>,
    panic: Option<GoString>,
}

#[derive(Deserialize)]
struct GoArg {
    kind: String,
    int: Option<i64>,
    bits: Option<String>,
    bytes: Option<GoString>,
}

#[test]
fn json_contains_matches_go() {
    let cases = &golden().json_contains;
    assert!(cases.len() > 250, "corpus too small");
    let mut f = Failures::default();
    for c in cases {
        let bytes: Vec<Vec<u8>> = c
            .args
            .iter()
            .map(|a| a.bytes.as_ref().map(GoString::bytes).unwrap_or_default())
            .collect();
        let args: Vec<Arg<'_>> = c
            .args
            .iter()
            .zip(&bytes)
            .map(|(a, b)| match a.kind.as_str() {
                "null" => Arg::Null,
                "integer" => Arg::Integer(a.int.unwrap_or_default()),
                "real" => Arg::Real(f64::from_bits(
                    u64::from_str_radix(a.bits.as_deref().expect("bits"), 16).expect("hex bits"),
                )),
                // What modernc hands Go for each SQLite type.
                "text" => Arg::Text(modernc_text(b)),
                "blob" => Arg::Blob(b),
                k => panic!("kind {k}"),
            })
            .collect();
        let what = || format!("json_contains({:?}, {:?})", args[0], args[1]);
        match (json_contains(args[0], args[1]), c) {
            (
                Ok(None),
                JsonContainsCase {
                    null: Some(true), ..
                },
            ) => {}
            (Ok(Some(b)), JsonContainsCase { int: Some(i), .. }) => f.check(what, i64::from(b), *i),
            (
                Err(e @ gorse_storage_core::json_contains::Error::Unhashable(_)),
                JsonContainsCase { panic: Some(p), .. },
            ) => {
                f.bytes(what, e.to_string().as_bytes(), p);
            }
            (
                Err(e),
                JsonContainsCase {
                    err: Some(want),
                    panic: None,
                    ..
                },
            ) if !matches!(e, gorse_storage_core::json_contains::Error::Unhashable(_)) => {
                f.bytes(what, modernc_error_text(&e).as_bytes(), want);
            }
            (got, _) => f.0.push(format!(
                "{}: got {got:?}, Go has null={:?} int={:?} err={:?} panic={:?}",
                what(),
                c.null,
                c.int,
                c.err.as_ref().map(|e| quote(e.bytes())),
                c.panic.as_ref().map(|e| quote(e.bytes())),
            )),
        }
    }
    f.finish(cases.len());
}

// ------------------------------------------------------- AppendURLParams

#[derive(Deserialize)]
struct AppendUrlParams {
    param_lists: Vec<Option<Vec<(GoString, GoString)>>>,
    cases: Vec<AppendUrlCase>,
}

#[derive(Deserialize)]
struct AppendUrlCase {
    url: GoString,
    params: usize,
    out: Option<GoString>,
    err: Option<GoString>,
}

#[test]
fn append_url_params_matches_go() {
    let g = &golden().append_url_params[0];
    let lists: Vec<Vec<(Vec<u8>, Vec<u8>)>> = g
        .param_lists
        .iter()
        .map(|l| {
            l.iter()
                .flatten()
                .map(|(k, v)| (k.bytes(), v.bytes()))
                .collect()
        })
        .collect();
    let mut f = Failures::default();
    for c in &g.cases {
        let url = c.url.bytes();
        let what = || format!("AppendURLParams({}, list {})", quote(&url), c.params);
        match (append_url_params(&url, &lists[c.params]), &c.out, &c.err) {
            (Ok(out), Some(want), None) => f.bytes(what, &out, want),
            (Err(e), None, Some(want)) => f.bytes(what, e.to_string().as_bytes(), want),
            (got, out, err) => f.0.push(format!(
                "{}: got {:?}, Go has out={:?} err={:?}",
                what(),
                got.map(quote).map_err(|e| e.to_string()),
                out.as_ref().map(|o| quote(o.bytes())),
                err.as_ref().map(|e| quote(e.bytes())),
            )),
        }
    }
    assert!(g.cases.len() > 300, "corpus too small");
    f.finish(g.cases.len());
}

// ------------------------------------------------------------- MySQL DSN

#[derive(Deserialize)]
struct MysqlDsn {
    parse: Vec<ParseCase>,
    append: Vec<AppendCase>,
}

#[derive(Deserialize)]
struct ParseCase {
    dsn: GoString,
    config: Option<ConfigOut>,
    format: Option<GoString>,
    err: Option<GoString>,
    panic: Option<GoString>,
}

#[derive(Deserialize)]
struct AppendCase {
    isolation_var: String,
    dsn: GoString,
    out: Option<GoString>,
    err: Option<GoString>,
    panic: Option<GoString>,
}

#[derive(Deserialize, Debug)]
struct ConfigOut {
    user: GoString,
    passwd: GoString,
    net: GoString,
    addr: GoString,
    db_name: GoString,
    param_keys: Option<Vec<GoString>>,
    param_values: Option<Vec<GoString>>,
    connection_attributes: GoString,
    collation: GoString,
    loc: GoString,
    loc_is: String,
    max_allowed_packet: i64,
    server_pub_key: GoString,
    tls_config: GoString,
    tls: Option<TlsOut>,
    timeout: i64,
    read_timeout: i64,
    write_timeout: i64,
    allow_all_files: bool,
    allow_cleartext_passwords: bool,
    allow_fallback_to_plaintext: bool,
    allow_native_passwords: bool,
    allow_old_passwords: bool,
    check_conn_liveness: bool,
    client_found_rows: bool,
    columns_with_alias: bool,
    interpolate_params: bool,
    multi_statements: bool,
    parse_time: bool,
    reject_read_only: bool,
    compress: bool,
    time_truncate: i64,
    charsets: Option<Vec<GoString>>,
    has_pub_key: bool,
}

#[derive(Deserialize, Debug)]
struct TlsOut {
    server_name: GoString,
    insecure_skip_verify: bool,
}

/// Zones load as `time.LoadLocation` loads them, from the system's zone
/// directories and then the GOROOT zip, here gorse-common's stand-in, which
/// holds every zone the corpus names (the golden tool read it through
/// $ZONEINFO). Only a zone's name and whether it loads are compared, so which
/// source it comes from does not matter; the directories do matter for a
/// name with a NUL, which Go and gotime both fail to open with EINVAL
/// ("invalid argument").
fn zones_setup() {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| {
        let goroot = format!(
            "{}/../gorse-common/tests/data/goroot",
            env!("CARGO_MANIFEST_DIR")
        );
        internal::set_default_goroot(&goroot);
    });
}

/// The panic message of f, or its result.
fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic".to_owned())
    })
}

fn check_config(f: &mut Failures, dsn: &str, cfg: &Config, want: &ConfigOut) {
    let what = |field: &str| format!("ParseDSN({dsn}).{field}");
    f.bytes(|| what("User"), &cfg.user, &want.user);
    f.bytes(|| what("Passwd"), &cfg.passwd, &want.passwd);
    f.bytes(|| what("Net"), &cfg.net, &want.net);
    f.bytes(|| what("Addr"), &cfg.addr, &want.addr);
    f.bytes(|| what("DBName"), &cfg.db_name, &want.db_name);
    let to_bytes = |l: &Option<Vec<GoString>>| {
        l.as_ref()
            .map(|l| l.iter().map(GoString::bytes).collect::<Vec<_>>())
    };
    f.check(
        || what("Params keys"),
        cfg.params
            .as_ref()
            .map(|p| p.keys().cloned().collect::<Vec<_>>()),
        to_bytes(&want.param_keys),
    );
    f.check(
        || what("Params values"),
        cfg.params
            .as_ref()
            .map(|p| p.values().cloned().collect::<Vec<_>>()),
        to_bytes(&want.param_values),
    );
    f.bytes(
        || what("ConnectionAttributes"),
        &cfg.connection_attributes,
        &want.connection_attributes,
    );
    f.bytes(|| what("Collation"), &cfg.collation, &want.collation);
    f.bytes(|| what("Loc"), cfg.loc.name().as_bytes(), &want.loc);
    // Go compares the pointer with time.UTC and time.Local; `loc=Local` is
    // the only way to a location named "Local".
    let loc_is = if cfg.loc.is_utc() {
        "UTC"
    } else if cfg.loc.name() == "Local" {
        "Local"
    } else {
        "loaded"
    };
    f.check(|| what("Loc is"), loc_is, want.loc_is.as_str());
    f.check(
        || what("MaxAllowedPacket"),
        cfg.max_allowed_packet,
        want.max_allowed_packet,
    );
    f.bytes(
        || what("ServerPubKey"),
        &cfg.server_pub_key,
        &want.server_pub_key,
    );
    f.bytes(|| what("TLSConfig"), &cfg.tls_config, &want.tls_config);
    f.check(
        || what("TLS"),
        cfg.tls
            .as_ref()
            .map(|t| (t.server_name.clone(), t.insecure_skip_verify)),
        want.tls
            .as_ref()
            .map(|t| (t.server_name.bytes(), t.insecure_skip_verify)),
    );
    f.check(|| what("Timeout"), cfg.timeout.0, want.timeout);
    f.check(
        || what("ReadTimeout"),
        cfg.read_timeout.0,
        want.read_timeout,
    );
    f.check(
        || what("WriteTimeout"),
        cfg.write_timeout.0,
        want.write_timeout,
    );
    let flags = [
        ("AllowAllFiles", cfg.allow_all_files, want.allow_all_files),
        (
            "AllowCleartextPasswords",
            cfg.allow_cleartext_passwords,
            want.allow_cleartext_passwords,
        ),
        (
            "AllowFallbackToPlaintext",
            cfg.allow_fallback_to_plaintext,
            want.allow_fallback_to_plaintext,
        ),
        (
            "AllowNativePasswords",
            cfg.allow_native_passwords,
            want.allow_native_passwords,
        ),
        (
            "AllowOldPasswords",
            cfg.allow_old_passwords,
            want.allow_old_passwords,
        ),
        (
            "CheckConnLiveness",
            cfg.check_conn_liveness,
            want.check_conn_liveness,
        ),
        (
            "ClientFoundRows",
            cfg.client_found_rows,
            want.client_found_rows,
        ),
        (
            "ColumnsWithAlias",
            cfg.columns_with_alias,
            want.columns_with_alias,
        ),
        (
            "InterpolateParams",
            cfg.interpolate_params,
            want.interpolate_params,
        ),
        (
            "MultiStatements",
            cfg.multi_statements,
            want.multi_statements,
        ),
        ("ParseTime", cfg.parse_time, want.parse_time),
        (
            "RejectReadOnly",
            cfg.reject_read_only,
            want.reject_read_only,
        ),
        ("compress", cfg.compress(), want.compress),
        ("pubKey != nil", cfg.pub_key().is_some(), want.has_pub_key),
    ];
    for (name, got, want) in flags {
        f.check(|| what(name), got, want);
    }
    f.check(
        || what("timeTruncate"),
        cfg.time_truncate().0,
        want.time_truncate,
    );
    f.check(
        || what("charsets"),
        cfg.charsets().map(<[Vec<u8>]>::to_vec),
        to_bytes(&want.charsets),
    );
}

#[test]
fn mysql_parse_dsn_matches_go() {
    zones_setup();
    let cases = &golden().mysql_dsn.parse;
    assert!(cases.len() > 800, "corpus too small");
    let mut f = Failures::default();
    for c in cases {
        let dsn = c.dsn.bytes();
        let q = quote(&dsn);
        match (catch(|| parse_dsn(&dsn)), c) {
            (
                Ok(Ok(cfg)),
                ParseCase {
                    config: Some(want),
                    format: Some(format),
                    ..
                },
            ) => {
                check_config(&mut f, &q, &cfg, want);
                f.bytes(|| format!("FormatDSN of {q}"), &cfg.format_dsn(), format);
            }
            (
                Ok(Err(e)),
                ParseCase {
                    err: Some(want), ..
                },
            ) => {
                f.bytes(|| format!("ParseDSN({q}) error"), &e.error_bytes(), want);
            }
            (
                Err(p),
                ParseCase {
                    panic: Some(want), ..
                },
            ) => {
                f.bytes(|| format!("ParseDSN({q}) panic"), p.as_bytes(), want);
            }
            (got, c) => f.0.push(format!(
                "ParseDSN({q}): got {}, Go has config={} err={:?} panic={:?}",
                match got {
                    Ok(Ok(_)) => "a config".to_owned(),
                    Ok(Err(e)) => format!("error {}", quote(e.error_bytes())),
                    Err(p) => format!("panic {p:?}"),
                },
                c.config.is_some(),
                c.err.as_ref().map(|e| quote(e.bytes())),
                c.panic.as_ref().map(|e| quote(e.bytes())),
            )),
        }
    }
    f.finish(cases.len());
}

#[test]
fn append_mysql_params_matches_go() {
    zones_setup();
    let cases = &golden().mysql_dsn.append;
    assert!(cases.len() > 2500, "corpus too small");
    let mut f = Failures::default();
    for c in cases {
        // The parameters storage/data and storage/cache add, with the
        // variable name ProbeMySQLIsolationVariableName returned.
        let mut params = BTreeMap::new();
        params.insert(
            "sql_mode".to_owned(),
            "'ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'".to_owned(),
        );
        params.insert(
            c.isolation_var.clone(),
            format!("'{}'", new_options([]).isolation_level),
        );
        params.insert("parseTime".to_owned(), "true".to_owned());
        let dsn = c.dsn.bytes();
        let what = || {
            format!(
                "AppendMySQLParams({}) with {:?}",
                quote(&dsn),
                c.isolation_var
            )
        };
        match (catch(|| append_mysql_params(&dsn, &params)), c) {
            (
                Ok(Ok(out)),
                AppendCase {
                    out: Some(want), ..
                },
            ) => f.bytes(what, &out, want),
            (
                Ok(Err(e)),
                AppendCase {
                    err: Some(want), ..
                },
            ) => f.bytes(what, &e.error_bytes(), want),
            (
                Err(p),
                AppendCase {
                    panic: Some(want), ..
                },
            ) => f.bytes(what, p.as_bytes(), want),
            (got, c) => f.0.push(format!(
                "{}: got {:?}, Go has out={:?} err={:?} panic={:?}",
                what(),
                got.map(|r| r.map(quote).map_err(|e| quote(e.error_bytes()))),
                c.out.as_ref().map(|e| quote(e.bytes())),
                c.err.as_ref().map(|e| quote(e.bytes())),
                c.panic.as_ref().map(|e| quote(e.bytes())),
            )),
        }
    }
    f.finish(cases.len());
}

#[test]
fn zones_come_from_the_test_goroot() {
    zones_setup();
    // A zone the stand-in zip holds, so `loc=` cases load the zones Go did.
    assert_eq!(
        gotime::load_location("Asia/Shanghai")
            .expect("Asia/Shanghai")
            .name(),
        "Asia/Shanghai"
    );
}
