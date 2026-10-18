// Ports: github.com/go-sql-driver/mysql dsn_test.go (TestDSNParser, TestDSNParserInvalid, TestDSNReformat, TestDSNServerPubKey, TestDSNServerPubKeyQueryEscape, TestDSNWithCustomTLS, TestDSNTLSConfig, TestDSNWithCustomTLSQueryEscape, TestDSNUnsafeCollation, TestParamsAreSorted, TestCloneConfig, TestNormalizeTLSConfig), dsn_fuzz_test.go (FuzzFormatDSN's seed corpus) @ v1.9.3 (the version add21b5's go.mod pins)
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.
//
// Derived from Go-MySQL-Driver: Copyright 2016 The Go-MySQL-Driver Authors;
// the license text is in crates/gorse-storage-core/licenses/GO-SQL-DRIVER-MYSQL-LICENSE.

//! Go's `dsn_test.go`, case for case, and `FuzzFormatDSN` run on its seed
//! corpus, as `go test` runs it without `-fuzz`. `BenchmarkParseDSN` is not
//! ported.
//!
//! Go's expected configs also set `Logger: defaultLogger`; the logger is not
//! ported, so that field is not compared.

mod mysql {
    use std::collections::BTreeMap;
    use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

    use gorse_common::gonet::split_host_port;
    use gorse_common::gostrconv::quote;
    use gorse_common::gotime::{self, Duration, HOUR, SECOND, utc};
    use gorse_common::gourl::query_escape;
    use gorse_storage_core::mysql_dsn::{
        Config, DEFAULT_MAX_ALLOWED_PACKET, DsnError, ServerPubKey, TlsConfig,
        deregister_server_pub_key, deregister_tls_config, get_tls_config_clone, new_config,
        parse_dsn, register_server_pub_key, register_tls_config,
    };

    fn b(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    fn lossy(s: &[u8]) -> String {
        String::from_utf8_lossy(s).into_owned()
    }

    /// A Go `Config{...}` literal: the zero config with the listed fields
    /// set.
    fn config(set: impl FnOnce(&mut Config)) -> Config {
        let mut c = Config::default();
        set(&mut c);
        c
    }

    fn params(pairs: &[(&str, &str)]) -> Option<BTreeMap<Vec<u8>, Vec<u8>>> {
        Some(pairs.iter().map(|(k, v)| (b(k), b(v))).collect())
    }

    /// Serializes the tests that use the TLS config and server public key
    /// registries, which Go's tests use one after another.
    static REGISTRIES: Mutex<()> = Mutex::new(());

    fn lock_registries() -> MutexGuard<'static, ()> {
        REGISTRIES.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Go's `defer DeregisterServerPubKey(name)`.
    struct DeregisterServerPubKey(&'static str);

    impl Drop for DeregisterServerPubKey {
        fn drop(&mut self) {
            deregister_server_pub_key(self.0);
        }
    }

    /// Go's `defer DeregisterTLSConfig(key)`.
    struct DeregisterTlsConfig(&'static str);

    impl Drop for DeregisterTlsConfig {
        fn drop(&mut self) {
            deregister_tls_config(self.0);
        }
    }

    /// auth_test.go's `testPubKeyRSA`. Go parses `testPubKey` into an
    /// `*rsa.PublicKey`; the tests only compare it by identity, so here it
    /// holds the PEM bytes uninterpreted.
    static TEST_PUB_KEY_RSA: LazyLock<ServerPubKey> = LazyLock::new(|| {
        ServerPubKey::new(
            "-----BEGIN PUBLIC KEY-----\n\
             MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAol0Z8G8U+25Btxk/g/fm\n\
             UAW/wEKjQCTjkibDE4B+qkuWeiumg6miIRhtilU6m9BFmLQSy1ltYQuu4k17A4tQ\n\
             rIPpOQYZges/qsDFkZh3wyK5jL5WEFVdOasf6wsfszExnPmcZS4axxoYJfiuilrN\n\
             hnwinBAqfi3S0sw5MpSI4Zl1AbOrHG4zDI62Gti2PKiMGyYDZTS9xPrBLbN95Kby\n\
             FFclQLEzA9RJcS1nHFsWtRgHjGPhhjCQxEm9NQ1nePFhCfBfApyfH1VM2VCOQum6\n\
             Ci9bMuHWjTjckC84mzF99kOxOWVU7mwS6gnJqBzpuz8t3zq8/iQ2y7QrmZV+jTJP\n\
             WQIDAQAB\n\
             -----END PUBLIC KEY-----\n",
        )
    });

    fn test_dsns() -> Vec<(&'static str, Config)> {
        vec![
            (
                "username:password@protocol(address)/dbname?param=value",
                config(|c| {
                    c.user = b("username");
                    c.passwd = b("password");
                    c.net = b("protocol");
                    c.addr = b("address");
                    c.db_name = b("dbname");
                    c.params = params(&[("param", "value")]);
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "username:password@protocol(address)/dbname?param=value&columnsWithAlias=true",
                config(|c| {
                    c.user = b("username");
                    c.passwd = b("password");
                    c.net = b("protocol");
                    c.addr = b("address");
                    c.db_name = b("dbname");
                    c.params = params(&[("param", "value")]);
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.columns_with_alias = true;
                }),
            ),
            (
                "username:password@protocol(address)/dbname?param=value&columnsWithAlias=true&multiStatements=true",
                config(|c| {
                    c.user = b("username");
                    c.passwd = b("password");
                    c.net = b("protocol");
                    c.addr = b("address");
                    c.db_name = b("dbname");
                    c.params = params(&[("param", "value")]);
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.columns_with_alias = true;
                    c.multi_statements = true;
                }),
            ),
            (
                "user@unix(/path/to/socket)/dbname?charset=utf8",
                config(|c| {
                    c.user = b("user");
                    c.net = b("unix");
                    c.addr = b("/path/to/socket");
                    c.db_name = b("dbname");
                    c.set_charsets(Some(vec![b("utf8")]));
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "user:password@tcp(localhost:5555)/dbname?charset=utf8&tls=true",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("password");
                    c.net = b("tcp");
                    c.addr = b("localhost:5555");
                    c.db_name = b("dbname");
                    c.set_charsets(Some(vec![b("utf8")]));
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.tls_config = b("true");
                }),
            ),
            (
                "user:password@tcp(localhost:5555)/dbname?charset=utf8mb4,utf8&tls=skip-verify",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("password");
                    c.net = b("tcp");
                    c.addr = b("localhost:5555");
                    c.db_name = b("dbname");
                    c.set_charsets(Some(vec![b("utf8mb4"), b("utf8")]));
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.tls_config = b("skip-verify");
                }),
            ),
            (
                "user:password@/dbname?loc=UTC&timeout=30s&readTimeout=1s&writeTimeout=1s&allowAllFiles=1&clientFoundRows=true&allowOldPasswords=TRUE&collation=utf8mb4_unicode_ci&maxAllowedPacket=16777216&tls=false&allowCleartextPasswords=true&parseTime=true&rejectReadOnly=true",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("password");
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname");
                    c.collation = b("utf8mb4_unicode_ci");
                    c.loc = utc();
                    c.tls_config = b("false");
                    c.allow_cleartext_passwords = true;
                    c.allow_native_passwords = true;
                    c.timeout = Duration(30 * SECOND.0);
                    c.read_timeout = SECOND;
                    c.write_timeout = SECOND;
                    c.allow_all_files = true;
                    c.allow_old_passwords = true;
                    c.check_conn_liveness = true;
                    c.client_found_rows = true;
                    c.max_allowed_packet = 16777216;
                    c.parse_time = true;
                    c.reject_read_only = true;
                }),
            ),
            (
                "user:password@/dbname?allowNativePasswords=false&checkConnLiveness=false&maxAllowedPacket=0&allowFallbackToPlaintext=true",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("password");
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname");
                    c.loc = utc();
                    c.max_allowed_packet = 0;
                    c.allow_fallback_to_plaintext = true;
                    c.allow_native_passwords = false;
                    c.check_conn_liveness = false;
                }),
            ),
            (
                "user:p@ss(word)@tcp([de:ad:be:ef::ca:fe]:80)/dbname?loc=Local",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("p@ss(word)");
                    c.net = b("tcp");
                    c.addr = b("[de:ad:be:ef::ca:fe]:80");
                    c.db_name = b("dbname");
                    c.loc = gotime::local();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "/dbname",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "/dbname%2Fwithslash",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname/withslash");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "@/",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "/",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "user:p@/ssword@/",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("p@/ssword");
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "unix/?arg=%2Fsome%2Fpath.ext",
                config(|c| {
                    c.net = b("unix");
                    c.addr = b("/tmp/mysql.sock");
                    c.params = params(&[("arg", "/some/path.ext")]);
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "tcp(127.0.0.1)/dbname",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "tcp(de:ad:be:ef::ca:fe)/dbname",
                config(|c| {
                    c.net = b("tcp");
                    c.addr = b("[de:ad:be:ef::ca:fe]:3306");
                    c.db_name = b("dbname");
                    c.loc = utc();
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                }),
            ),
            (
                "user:password@/dbname?loc=UTC&timeout=30s&parseTime=true&timeTruncate=1h",
                config(|c| {
                    c.user = b("user");
                    c.passwd = b("password");
                    c.net = b("tcp");
                    c.addr = b("127.0.0.1:3306");
                    c.db_name = b("dbname");
                    c.loc = utc();
                    c.timeout = Duration(30 * SECOND.0);
                    c.parse_time = true;
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.set_time_truncate(HOUR);
                }),
            ),
            (
                "foo:bar@tcp(192.168.1.50:3307)/baz?timeout=10s&connectionAttributes=program_name:MySQLGoDriver%2FTest,program_version:1.2.3",
                config(|c| {
                    c.user = b("foo");
                    c.passwd = b("bar");
                    c.net = b("tcp");
                    c.addr = b("192.168.1.50:3307");
                    c.db_name = b("baz");
                    c.loc = utc();
                    c.timeout = Duration(10 * SECOND.0);
                    c.max_allowed_packet = DEFAULT_MAX_ALLOWED_PACKET;
                    c.allow_native_passwords = true;
                    c.check_conn_liveness = true;
                    c.connection_attributes =
                        b("program_name:MySQLGoDriver/Test,program_version:1.2.3");
                }),
            ),
        ]
    }

    #[test]
    fn test_dsn_parser() {
        let mut errors = Vec::new();
        for (i, (input, want)) in test_dsns().into_iter().enumerate() {
            let mut cfg = match parse_dsn(input) {
                Ok(cfg) => cfg,
                Err(err) => {
                    errors.push(format!("{input}: {err}"));
                    continue;
                }
            };

            // pointer not static
            cfg.tls = None;

            if !cfg.deep_equal(&want) {
                errors.push(format!(
                    "{i}. ParseDSN({input:?}) mismatch:\ngot  {cfg:?}\nwant {want:?}"
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_dsn_parser_invalid() {
        let invalid_dsns = [
            "@net(addr/",                                                        // no closing brace
            "@tcp(/",                                                            // no closing brace
            "tcp(/",                                                             // no closing brace
            "(/",                                                                // no closing brace
            "net(addr)//",                                                       // unescaped
            "User:pass@tcp(1.2.3.4:3306)", // no trailing slash
            "net()/",                      // unknown default addr
            "user:pass@tcp(127.0.0.1:3306)/db/name", // invalid dbname
            "user:password@/dbname?allowFallbackToPlaintext=PREFERRED", // wrong bool flag
            "user:password@/dbname?connectionAttributes=attr1:/unescaped/value", // unescaped
        ];
        // Go's list ends with a commented-out case:
        //"/dbname?arg=/some/unescaped/path",

        let mut errors = Vec::new();
        for (i, tst) in invalid_dsns.iter().enumerate() {
            if parse_dsn(tst).is_ok() {
                errors.push(format!("invalid DSN #{i}. ({tst}) didn't error!"));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_dsn_reformat() {
        let mut errors = Vec::new();
        for (i, (dsn1, _)) in test_dsns().into_iter().enumerate() {
            let mut cfg1 = match parse_dsn(dsn1) {
                Ok(cfg) => cfg,
                Err(err) => {
                    errors.push(format!("{dsn1}: {err}"));
                    continue;
                }
            };
            cfg1.tls = None; // pointer not static

            let dsn2 = cfg1.format_dsn();
            if dsn2 != dsn1.as_bytes() {
                // Just log
                println!("{i}. {dsn1:?} reformatted as {:?}", lossy(&dsn2));
            }

            let mut cfg2 = match parse_dsn(&dsn2) {
                Ok(cfg) => cfg,
                Err(err) => {
                    errors.push(format!("{}: {err}", lossy(&dsn2)));
                    continue;
                }
            };
            cfg2.tls = None; // pointer not static

            // Go compares the `%+v` texts of the two configs; this compares
            // the configs themselves, field by field.
            if !cfg1.deep_equal(&cfg2) {
                errors.push(format!("{i}. {cfg2:?} does not match {cfg1:?}"));
            }

            let dsn3 = cfg2.format_dsn();
            if dsn3 != dsn2 {
                errors.push(format!(
                    "{i}. {:?} does not match {:?}",
                    lossy(&dsn2),
                    lossy(&dsn3)
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn test_dsn_server_pub_key() {
        let _registries = lock_registries();
        let base_dsn = "User:password@tcp(localhost:5555)/dbname?serverPubKey=";

        register_server_pub_key("testKey", TEST_PUB_KEY_RSA.clone());
        let _deregister = DeregisterServerPubKey("testKey");

        let tst = format!("{base_dsn}testKey");
        let cfg = parse_dsn(&tst).unwrap_or_else(|err| panic!("{err}"));

        assert_eq!(
            cfg.server_pub_key,
            b"testKey",
            "unexpected cfg.ServerPubKey value: {}",
            lossy(&cfg.server_pub_key)
        );
        assert!(
            cfg.pub_key() == Some(&*TEST_PUB_KEY_RSA),
            "pub key pointer doesn't match"
        );

        // Key is missing
        let tst = format!("{base_dsn}invalid_name");
        if let Ok(cfg) = parse_dsn(&tst) {
            panic!("invalid name in DSN ({tst}) but did not error. Got config: {cfg:?}");
        }
    }

    #[test]
    fn test_dsn_server_pub_key_query_escape() {
        let _registries = lock_registries();
        const NAME: &str = "&%!:";
        let dsn = format!(
            "User:password@tcp(localhost:5555)/dbname?serverPubKey={}",
            query_escape(NAME)
        );

        register_server_pub_key(NAME, TEST_PUB_KEY_RSA.clone());
        let _deregister = DeregisterServerPubKey(NAME);

        let cfg = parse_dsn(&dsn).unwrap_or_else(|err| panic!("{err}"));

        assert!(
            cfg.pub_key() == Some(&*TEST_PUB_KEY_RSA),
            "pub key pointer doesn't match"
        );
    }

    #[test]
    fn test_dsn_with_custom_tls() {
        let _registries = lock_registries();
        let base_dsn = "User:password@tcp(localhost:5555)/dbname?tls=";
        let mut tls_cfg = TlsConfig::default();

        register_tls_config("utils_test", tls_cfg.clone()).expect("utils_test is not reserved");
        let _deregister = DeregisterTlsConfig("utils_test");

        // Custom TLS is missing
        let tst = format!("{base_dsn}invalid_tls");
        if let Ok(cfg) = parse_dsn(&tst) {
            panic!("invalid custom TLS in DSN ({tst}) but did not error. Got config: {cfg:?}");
        }

        let tst = format!("{base_dsn}utils_test");

        // Go changes the registered *tls.Config in place below; the registry
        // keeps its own copy here, so the changed config is registered again.

        // Custom TLS with a server name
        let name = "foohost";
        tls_cfg.server_name = b(name);
        register_tls_config("utils_test", tls_cfg.clone()).expect("utils_test is not reserved");
        let cfg = parse_dsn(&tst).unwrap_or_else(|err| panic!("{err}"));
        let tls = cfg.tls.as_ref().expect("cfg.TLS");
        assert_eq!(
            tls.server_name,
            name.as_bytes(),
            "did not get the correct TLS ServerName ({name}) parsing DSN ({tst})."
        );

        // Custom TLS without a server name
        let name = "localhost";
        tls_cfg.server_name = Vec::new();
        register_tls_config("utils_test", tls_cfg.clone()).expect("utils_test is not reserved");
        let cfg = parse_dsn(&tst).unwrap_or_else(|err| panic!("{err}"));
        let tls = cfg.tls.as_ref().expect("cfg.TLS");
        assert_eq!(
            tls.server_name,
            name.as_bytes(),
            "did not get the correct ServerName ({name}) parsing DSN ({tst})."
        );
        // Go's tlsCfg is the registered config itself; here that is the
        // registry's copy.
        let registered = get_tls_config_clone("utils_test").expect("utils_test is registered");
        assert!(
            registered.server_name.is_empty(),
            "tlsCfg was mutated ServerName ({name}) should be empty parsing DSN ({tst})."
        );
    }

    #[test]
    fn test_dsn_tls_config() {
        let expected_server_name = "example.com";
        let dsn = "tcp(example.com:1234)/?tls=true";

        let cfg = parse_dsn(dsn).unwrap_or_else(|err| panic!("{err}"));
        let tls = cfg.tls.as_ref().expect("cfg.tls should not be nil");
        assert_eq!(
            tls.server_name,
            expected_server_name.as_bytes(),
            "cfg.tls.ServerName should be {expected_server_name:?}, got {:?} (host with port)",
            lossy(&tls.server_name)
        );

        let dsn = "tcp(example.com)/?tls=true";
        let cfg = parse_dsn(dsn).unwrap_or_else(|err| panic!("{err}"));
        let tls = cfg.tls.as_ref().expect("cfg.tls should not be nil");
        assert_eq!(
            tls.server_name,
            expected_server_name.as_bytes(),
            "cfg.tls.ServerName should be {expected_server_name:?}, got {:?} (host without port)",
            lossy(&tls.server_name)
        );
    }

    #[test]
    fn test_dsn_with_custom_tls_query_escape() {
        let _registries = lock_registries();
        const CONFIG_KEY: &str = "&%!:";
        let dsn = format!(
            "User:password@tcp(localhost:5555)/dbname?tls={}",
            query_escape(CONFIG_KEY)
        );
        let name = "foohost";
        let tls_cfg = TlsConfig {
            server_name: b(name),
            ..TlsConfig::default()
        };

        register_tls_config(CONFIG_KEY, tls_cfg).expect("&%!: is not reserved");
        let _deregister = DeregisterTlsConfig(CONFIG_KEY);

        let cfg = parse_dsn(&dsn).unwrap_or_else(|err| panic!("{err}"));
        let tls = cfg.tls.as_ref().expect("cfg.TLS");
        assert_eq!(
            tls.server_name,
            name.as_bytes(),
            "did not get the correct TLS ServerName ({name}) parsing DSN ({dsn})."
        );
    }

    #[test]
    fn test_dsn_unsafe_collation() {
        let err = parse_dsn("/dbname?collation=gbk_chinese_ci&interpolateParams=true").err();
        assert_eq!(
            err,
            Some(DsnError::InvalidDsnUnsafeCollation),
            "expected {}, got {err:?}",
            DsnError::InvalidDsnUnsafeCollation
        );

        for dsn in [
            "/dbname?collation=gbk_chinese_ci&interpolateParams=false",
            "/dbname?collation=gbk_chinese_ci",
            "/dbname?collation=ascii_bin&interpolateParams=true",
            "/dbname?collation=latin1_german1_ci&interpolateParams=true",
            "/dbname?collation=utf8_general_ci&interpolateParams=true",
            "/dbname?collation=utf8mb4_general_ci&interpolateParams=true",
        ] {
            let err = parse_dsn(dsn).err();
            assert_eq!(err, None, "{dsn}: expected nil, got {err:?}");
        }
    }

    #[test]
    fn test_params_are_sorted() {
        let expected = "/dbname?interpolateParams=true&foobar=baz&quux=loo";
        let mut cfg = new_config();
        cfg.db_name = b("dbname");
        cfg.interpolate_params = true;
        cfg.params = params(&[("quux", "loo"), ("foobar", "baz")]);
        let actual = cfg.format_dsn();
        assert_eq!(
            actual,
            expected.as_bytes(),
            "generic Config.Params were not sorted: want {expected:?}, got {:?}",
            lossy(&actual)
        );
    }

    #[test]
    fn test_clone_config() {
        let _registries = lock_registries();
        register_server_pub_key("testKey", TEST_PUB_KEY_RSA.clone());
        let _deregister = DeregisterServerPubKey("testKey");

        let expected_server_name = "example.com";
        let dsn = "tcp(example.com:1234)/?tls=true&foobar=baz&serverPubKey=testKey";
        let cfg = parse_dsn(dsn).unwrap_or_else(|err| panic!("{err}"));

        let mut cfg2 = cfg.clone();
        assert!(
            !std::ptr::eq(&cfg, &cfg2),
            "Config.Clone did not create a separate config struct"
        );

        let tls2 = cfg2.tls.as_mut().expect("cfg2.TLS");
        assert_eq!(
            tls2.server_name,
            expected_server_name.as_bytes(),
            "cfg.tls.ServerName should be {expected_server_name:?}, got {:?} (host with port)",
            lossy(&tls2.server_name)
        );

        tls2.server_name = b("example2.com");
        let tls = cfg.tls.as_ref().expect("cfg.TLS");
        assert_ne!(
            tls.server_name, tls2.server_name,
            "changed cfg.tls.Server name should not propagate to original Config"
        );

        let params2 = cfg2.params.as_mut().expect("cfg2.Params");
        assert!(
            params2.contains_key(&b"foobar"[..]),
            "cloned Config is missing custom params"
        );

        params2.remove(&b"foobar"[..]);

        assert!(
            cfg.params
                .as_ref()
                .is_some_and(|p| p.contains_key(&b"foobar"[..])),
            "custom params in cloned Config should not propagate to original Config"
        );

        // reflect.DeepEqual on the two *rsa.PublicKey values.
        let deep_equal = match (cfg.pub_key(), cfg2.pub_key()) {
            (None, None) => true,
            (Some(a), Some(b)) => a.bytes() == b.bytes(),
            _ => false,
        };
        assert!(deep_equal, "public key in Config should be identical");
    }

    #[test]
    fn test_normalize_tls_config() {
        let _registries = lock_registries();
        let tt: [(&str, Option<TlsConfig>); 6] = [
            ("", None),
            ("false", None),
            (
                "true",
                Some(TlsConfig {
                    server_name: b("myserver"),
                    ..TlsConfig::default()
                }),
            ),
            (
                "skip-verify",
                Some(TlsConfig {
                    insecure_skip_verify: true,
                    ..TlsConfig::default()
                }),
            ),
            (
                "preferred",
                Some(TlsConfig {
                    insecure_skip_verify: true,
                    ..TlsConfig::default()
                }),
            ),
            (
                "test_tls_config",
                Some(TlsConfig {
                    server_name: b("myServerName"),
                    ..TlsConfig::default()
                }),
            ),
        ];

        register_tls_config(
            "test_tls_config",
            TlsConfig {
                server_name: b("myServerName"),
                ..TlsConfig::default()
            },
        )
        .expect("test_tls_config is not reserved");
        let _deregister = DeregisterTlsConfig("test_tls_config");

        let mut errors = Vec::new();
        for (tls_config, want) in tt {
            let mut cfg = config(|c| {
                c.addr = b("myserver:3306");
                c.tls_config = b(tls_config);
            });

            // Go ignores the error.
            let _ = cfg.normalize();

            let Some(got) = &cfg.tls else {
                if want.is_some() {
                    errors.push(format!(
                        "{tls_config}: wanted a tls config but got nil instead"
                    ));
                }
                continue;
            };
            // Go dereferences the nil want here and panics.
            let Some(want) = want else {
                errors.push(format!("{tls_config}: got a tls config, wanted nil"));
                continue;
            };

            if got.server_name != want.server_name {
                errors.push(format!(
                    "{tls_config}: tls.ServerName doesn't match (want: '{}', got: '{}')",
                    lossy(&want.server_name),
                    lossy(&got.server_name)
                ));
            }
            if got.insecure_skip_verify != want.insecure_skip_verify {
                errors.push(format!(
                    "{tls_config}: tls.InsecureSkipVerify doesn't match (want: {}, got :{})",
                    want.insecure_skip_verify, got.insecure_skip_verify
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    /// How a `FuzzFormatDSN` input ended: passed, or skipped with Go's
    /// `t.Skip` reason. Failures are the `Err` of [`fuzz_format_dsn`].
    enum Outcome {
        Pass,
        Skip(String),
    }

    /// The body of `FuzzFormatDSN`'s `f.Fuzz` function.
    fn fuzz_format_dsn(dsn1: &[u8]) -> Result<Outcome, String> {
        // Do not waste resources
        if dsn1.len() > 1000 {
            return Ok(Outcome::Skip("ignore: too long".to_owned()));
        }

        let cfg1 = match parse_dsn(dsn1) {
            Ok(cfg) => cfg,
            Err(err) => return Ok(Outcome::Skip(format!("invalid DSN: {err}"))),
        };

        let dsn2 = cfg1.format_dsn();
        if dsn2 == dsn1 {
            return Ok(Outcome::Pass);
        }

        // Skip known cases of bad config that are not strictly checked by ParseDSN
        if let Err(err) = split_host_port(&cfg1.addr) {
            return Ok(Outcome::Skip(format!(
                "invalid addr {}: {err}",
                quote(&cfg1.addr)
            )));
        }

        let cfg2 = parse_dsn(&dsn2)
            .map_err(|err| format!("{} rewritten as {}: {err}", quote(dsn1), quote(&dsn2)))?;

        let dsn3 = cfg2.format_dsn();
        if dsn3 != dsn2 {
            return Err(format!("{} rewritten as {}", quote(&dsn2), quote(&dsn3)));
        }
        Ok(Outcome::Pass)
    }

    /// `FuzzFormatDSN` on its seed corpus, the inputs of `testDSNs`, which
    /// is what `go test` runs without `-fuzz`.
    #[test]
    fn fuzz_format_dsn_seed_corpus() {
        let mut errors = Vec::new();
        for (i, (dsn, _)) in test_dsns().into_iter().enumerate() {
            match fuzz_format_dsn(dsn.as_bytes()) {
                Ok(Outcome::Pass) => {}
                Ok(Outcome::Skip(reason)) => {
                    println!("--- SKIP: FuzzFormatDSN/seed#{i}: {reason}");
                }
                Err(err) => errors.push(format!("seed#{i}: {err}")),
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
}
