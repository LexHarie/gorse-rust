// Ports: contracts/tools/schemadump/main.go and contracts/tools/dump-schemas.sh (WP-2-13): the Go side of each test runs them.

//! WP-2-13, the schema conformance checker.
//!
//! Each test initialises one backend with the Go build (the full run of
//! `contracts/tools/schemadump`, and `pg_dump` for PostgreSQL as `dump-schemas.sh`
//! does) and with the Rust stores (`gorse_conformance::schema::init`). The Rust
//! databases are then dumped twice: by the Go tool in its `-dump-only` mode, which
//! is the primary check, and by the Rust dumper, which is the second check. Every
//! dump must equal the Go-initialised dump and the frozen file in
//! `contracts/schema/`.
//!
//! The backends come from `storage/docker-compose.yml`. A test whose backend URI
//! is not set fails; it is never skipped (`conformance/README.md`).

use std::path::Path;

use gorse_conformance::schema::{Report, contract, dump, go, init, normalise_ft_info};

/// The value of an environment variable a test needs. A missing value fails the
/// test: a backend the checker cannot reach is not a pass.
fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set; this test needs its backend (conformance/README.md)")
    })
}

/// Checks one schema file of a Rust-initialised store against the frozen
/// contract. `go` is the Go run's dump of the Go-initialised store, `go_tool` is
/// the Go tool's dump of the Rust-initialised store, and `rust` is the Rust
/// dumper's dump of it.
fn check(report: &mut Report, file: &str, go: &str, go_tool: &str, rust: &str) {
    let frozen = contract(file);
    report.same(&format!("{file}: Go dump vs contract"), &frozen, go);
    report.same(
        &format!("{file}: Go tool on the Rust database vs Go dump"),
        go,
        go_tool,
    );
    report.same(
        &format!("{file}: Go tool on the Rust database vs contract"),
        &frozen,
        go_tool,
    );
    report.same(&format!("{file}: Rust dumper vs contract"), &frozen, rust);
}

/// Checks one schema file of MariaDB 10.2, which has no frozen dump, against the
/// Go-initialised dump. `go_tool` and `rust` are the two dumps of the
/// Rust-initialised store.
fn check_against_go(report: &mut Report, file: &str, go: &str, go_tool: &str, rust: &str) {
    report.same(
        &format!("{file}: Go tool on the Rust database vs Go dump"),
        go,
        go_tool,
    );
    report.same(&format!("{file}: Rust dumper vs Go dump"), go, rust);
}

#[tokio::test]
async fn schema_sqlite() {
    let go = go::run(&go::Targets::default()).expect("Go schemadump");
    let work = tempfile::tempdir().expect("temporary directory");
    let mut report = Report::default();

    let data = work.path().join("data.sqlite");
    let data_url = format!("sqlite://{}", data.display());
    init::data(&data_url, false)
        .await
        .expect("Rust data store, Init and samples");
    check(
        &mut report,
        "data/sqlite.sql",
        &go.read("data/sqlite.sql"),
        &go::dump_sqlite(&data).expect("Go tool dump"),
        &dump::sqlite(&data).expect("Rust dump"),
    );
    init::data(&data_url, true)
        .await
        .expect("Rust data store, Reconcile");
    check(
        &mut report,
        "data/sqlite.search.sql",
        &go.read("data/sqlite.search.sql"),
        &go::dump_sqlite(&data).expect("Go tool dump"),
        &dump::sqlite(&data).expect("Rust dump"),
    );

    let cache = work.path().join("cache.sqlite");
    init::cache(&format!("sqlite://{}", cache.display()))
        .await
        .expect("Rust cache store");
    check(
        &mut report,
        "cache/sqlite.sql",
        &go.read("cache/sqlite.sql"),
        &go::dump_sqlite(&cache).expect("Go tool dump"),
        &dump::sqlite(&cache).expect("Rust dump"),
    );

    let meta = work.path().join("meta.sqlite3");
    init::meta(Path::new(&meta)).await.expect("Rust meta store");
    check(
        &mut report,
        "meta/sqlite.sql",
        &go.read("meta/sqlite.sql"),
        &go::dump_sqlite(&meta).expect("Go tool dump"),
        &dump::sqlite(&meta).expect("Rust dump"),
    );

    report.finish();
}

#[tokio::test]
async fn schema_mysql() {
    // `MYSQL_URI` is `mysql://root:password@tcp(127.0.0.1:3306)/`: the Go DSN
    // with the scheme in front.
    let uri = required_env("MYSQL_URI");
    let dsn = uri
        .strip_prefix("mysql://")
        .expect("MYSQL_URI starts with mysql://");
    let go = go::run(&go::Targets {
        mysql: Some(dsn),
        ..Default::default()
    })
    .expect("Go schemadump");
    let mut report = Report::default();

    // The Go run leaves its databases in place. Both dumpers must reproduce the
    // Go dump of the state the Go run finished in (its search variant).
    let go_data = format!("{dsn}{}?parseTime=true", init::GO_DATA_DB);
    report.same(
        "data/mysql.search.sql: Go tool on the Go-created database vs Go dump",
        &go.read("data/mysql.search.sql"),
        &go::dump_mysql(&go_data).expect("Go tool dump"),
    );
    report.same(
        "data/mysql.search.sql: Rust dumper on the Go-created database vs Go dump",
        &go.read("data/mysql.search.sql"),
        &dump::mysql(&go_data).await.expect("Rust dump"),
    );

    init::recreate_mysql(dsn, &[init::RUST_DATA_DB, init::RUST_CACHE_DB])
        .await
        .expect("create the Rust databases");
    let rust_data = format!("{dsn}{}?parseTime=true", init::RUST_DATA_DB);
    let rust_cache = format!("{dsn}{}?parseTime=true", init::RUST_CACHE_DB);

    init::data(&format!("mysql://{rust_data}"), false)
        .await
        .expect("Rust data store, Init and samples");
    check(
        &mut report,
        "data/mysql.sql",
        &go.read("data/mysql.sql"),
        &go::dump_mysql(&rust_data).expect("Go tool dump"),
        &dump::mysql(&rust_data).await.expect("Rust dump"),
    );
    init::data(&format!("mysql://{rust_data}"), true)
        .await
        .expect("Rust data store, Reconcile");
    check(
        &mut report,
        "data/mysql.search.sql",
        &go.read("data/mysql.search.sql"),
        &go::dump_mysql(&rust_data).expect("Go tool dump"),
        &dump::mysql(&rust_data).await.expect("Rust dump"),
    );

    init::cache(&format!("mysql://{rust_cache}"))
        .await
        .expect("Rust cache store");
    check(
        &mut report,
        "cache/mysql.sql",
        &go.read("cache/mysql.sql"),
        &go::dump_mysql(&rust_cache).expect("Go tool dump"),
        &dump::mysql(&rust_cache).await.expect("Rust dump"),
    );

    report.finish();
}

#[tokio::test]
async fn schema_postgres() {
    // `POSTGRES_URI` is `postgres://gorse:gorse_pass@127.0.0.1:5432/`. Both
    // dumpers run `pg_dump` inside the container, as `dump-schemas.sh` does.
    let uri = required_env("POSTGRES_URI");
    let container =
        std::env::var("POSTGRES_CONTAINER").unwrap_or_else(|_| "storage-postgres-1".to_owned());
    // The Go run creates the Go databases, which are dumped below.
    go::run(&go::Targets {
        postgres: Some(&uri),
        ..Default::default()
    })
    .expect("Go schemadump");
    let mut report = Report::default();
    let go_dump = |db: &str| go::dump_postgres(&container, db).expect("Go tool dump");
    let rust_dump = |db: &str| dump::postgres(&container, db).expect("Rust dump");

    // The Go-created databases, dumped by both dumpers, must equal the contract.
    for (file, db) in [
        ("data/postgres.sql", init::GO_DATA_DB),
        ("data/postgres.search.sql", init::GO_DATA_SEARCH_DB),
        ("cache/postgres.sql", init::GO_CACHE_DB),
    ] {
        report.same(
            &format!("{file}: Go tool on the Go-created database vs contract"),
            &contract(file),
            &go_dump(db),
        );
        report.same(
            &format!("{file}: Rust dumper on the Go-created database vs contract"),
            &contract(file),
            &rust_dump(db),
        );
    }

    init::recreate_postgres(
        &uri,
        &[
            init::RUST_DATA_DB,
            init::RUST_DATA_SEARCH_DB,
            init::RUST_CACHE_DB,
        ],
    )
    .await
    .expect("create the Rust databases");
    let url = |db: &str| format!("{uri}{db}?sslmode=disable");

    init::data(&url(init::RUST_DATA_DB), false)
        .await
        .expect("Rust data store, Init and samples");
    check(
        &mut report,
        "data/postgres.sql",
        &go_dump(init::GO_DATA_DB),
        &go_dump(init::RUST_DATA_DB),
        &rust_dump(init::RUST_DATA_DB),
    );
    init::data(&url(init::RUST_DATA_SEARCH_DB), true)
        .await
        .expect("Rust data store, Reconcile");
    check(
        &mut report,
        "data/postgres.search.sql",
        &go_dump(init::GO_DATA_SEARCH_DB),
        &go_dump(init::RUST_DATA_SEARCH_DB),
        &rust_dump(init::RUST_DATA_SEARCH_DB),
    );
    init::cache(&url(init::RUST_CACHE_DB))
        .await
        .expect("Rust cache store");
    check(
        &mut report,
        "cache/postgres.sql",
        &go_dump(init::GO_CACHE_DB),
        &go_dump(init::RUST_CACHE_DB),
        &rust_dump(init::RUST_CACHE_DB),
    );

    report.finish();
}

#[tokio::test]
async fn schema_mongodb() {
    // `MONGO_URI` is `mongodb://root:password@127.0.0.1:27017/`, without a database.
    let uri = required_env("MONGO_URI");
    let go = go::run(&go::Targets {
        mongo: Some(&uri),
        ..Default::default()
    })
    .expect("Go schemadump");
    let mut report = Report::default();

    report.same(
        "data/mongodb.search.json: Go tool on the Go-created database vs Go dump",
        &go.read("data/mongodb.search.json"),
        &go::dump_mongodb(&uri, init::GO_DATA_DB).expect("Go tool dump"),
    );
    report.same(
        "data/mongodb.search.json: Rust dumper on the Go-created database vs Go dump",
        &go.read("data/mongodb.search.json"),
        &dump::mongodb(&uri, init::GO_DATA_DB)
            .await
            .expect("Rust dump"),
    );
    report.same(
        "cache/mongodb.json: Go tool on the Go-created database vs Go dump",
        &go.read("cache/mongodb.json"),
        &go::dump_mongodb(&uri, init::GO_CACHE_DB).expect("Go tool dump"),
    );
    report.same(
        "cache/mongodb.json: Rust dumper on the Go-created database vs Go dump",
        &go.read("cache/mongodb.json"),
        &dump::mongodb(&uri, init::GO_CACHE_DB)
            .await
            .expect("Rust dump"),
    );

    init::recreate_mongodb(&uri, &[init::RUST_DATA_DB, init::RUST_CACHE_DB])
        .await
        .expect("drop the Rust databases");
    let data_url = format!("{uri}{}?authSource=admin", init::RUST_DATA_DB);
    let cache_url = format!("{uri}{}?authSource=admin", init::RUST_CACHE_DB);

    init::data(&data_url, false)
        .await
        .expect("Rust data store, Init and samples");
    check(
        &mut report,
        "data/mongodb.json",
        &go.read("data/mongodb.json"),
        &go::dump_mongodb(&uri, init::RUST_DATA_DB).expect("Go tool dump"),
        &dump::mongodb(&uri, init::RUST_DATA_DB)
            .await
            .expect("Rust dump"),
    );
    init::data(&data_url, true)
        .await
        .expect("Rust data store, Reconcile");
    check(
        &mut report,
        "data/mongodb.search.json",
        &go.read("data/mongodb.search.json"),
        &go::dump_mongodb(&uri, init::RUST_DATA_DB).expect("Go tool dump"),
        &dump::mongodb(&uri, init::RUST_DATA_DB)
            .await
            .expect("Rust dump"),
    );
    init::cache(&cache_url).await.expect("Rust cache store");
    check(
        &mut report,
        "cache/mongodb.json",
        &go.read("cache/mongodb.json"),
        &go::dump_mongodb(&uri, init::RUST_CACHE_DB).expect("Go tool dump"),
        &dump::mongodb(&uri, init::RUST_CACHE_DB)
            .await
            .expect("Rust dump"),
    );

    report.finish();
}

#[tokio::test]
async fn schema_redis() {
    // `REDIS_URI` is `redis://127.0.0.1:6379/`. Each run flushes the server.
    let url = required_env("REDIS_URI");
    let go = go::run(&go::Targets {
        redis: Some(&url),
        ..Default::default()
    })
    .expect("Go schemadump");
    let mut report = Report::default();

    // Every FT.INFO call counts as a use of the index (`number_of_uses`), so each
    // dump reads a freshly initialised index exactly once. The Go tool's read
    // comes first; the state is then rebuilt, so the Rust dumper's read is also
    // the first of its state.
    init::flush_redis(&url).await.expect("FLUSHALL");
    init::cache(&url).await.expect("Rust cache store");
    let go_tool = go::dump_redis(&url).expect("Go tool dump");
    init::flush_redis(&url).await.expect("FLUSHALL");
    init::cache(&url).await.expect("Rust cache store");
    let rust = dump::redis_ft_info(&url).await.expect("Rust dump");

    let frozen = normalise_ft_info(&contract("cache/redis.ft-info.txt"));
    let go_text = normalise_ft_info(&go.read("cache/redis.ft-info.txt"));
    report.same(
        "cache/redis.ft-info.txt: Go dump vs contract",
        &frozen,
        &go_text,
    );
    report.same(
        "cache/redis.ft-info.txt: Go tool on the Rust index vs Go dump",
        &go_text,
        &normalise_ft_info(&go_tool),
    );
    report.same(
        "cache/redis.ft-info.txt: Go tool on the Rust index vs contract",
        &frozen,
        &normalise_ft_info(&go_tool),
    );
    report.same(
        "cache/redis.ft-info.txt: Rust dumper vs contract",
        &frozen,
        &normalise_ft_info(&rust),
    );

    report.finish();
}

#[tokio::test]
async fn schema_mariadb() {
    // `MARIADB_URI` is `mysql://root:password@tcp(127.0.0.1:3307)/` for MariaDB
    // 10.2, the Go CI's version. Its dumps differ from MySQL's (the multi-valued
    // index is skipped, and the JSON column is LONGTEXT), and contracts/schema
    // holds MySQL 8.0 only, so the builds are compared with each other.
    let uri = required_env("MARIADB_URI");
    let dsn = uri
        .strip_prefix("mysql://")
        .expect("MARIADB_URI starts with mysql://");
    let go = go::run(&go::Targets {
        mysql: Some(dsn),
        ..Default::default()
    })
    .expect("Go schemadump");
    let mut report = Report::default();

    let go_data = format!("{dsn}{}?parseTime=true", init::GO_DATA_DB);
    report.same(
        "data/mysql.search.sql: Go tool on the Go-created database vs Go dump (MariaDB)",
        &go.read("data/mysql.search.sql"),
        &go::dump_mysql(&go_data).expect("Go tool dump"),
    );

    init::recreate_mysql(dsn, &[init::RUST_DATA_DB, init::RUST_CACHE_DB])
        .await
        .expect("create the Rust databases");
    let rust_data = format!("{dsn}{}?parseTime=true", init::RUST_DATA_DB);
    let rust_cache = format!("{dsn}{}?parseTime=true", init::RUST_CACHE_DB);

    init::data(&format!("mysql://{rust_data}"), false)
        .await
        .expect("Rust data store, Init and samples");
    check_against_go(
        &mut report,
        "data/mysql.sql",
        &go.read("data/mysql.sql"),
        &go::dump_mysql(&rust_data).expect("Go tool dump"),
        &dump::mysql(&rust_data).await.expect("Rust dump"),
    );
    init::data(&format!("mysql://{rust_data}"), true)
        .await
        .expect("Rust data store, Reconcile");
    check_against_go(
        &mut report,
        "data/mysql.search.sql",
        &go.read("data/mysql.search.sql"),
        &go::dump_mysql(&rust_data).expect("Go tool dump"),
        &dump::mysql(&rust_data).await.expect("Rust dump"),
    );

    init::cache(&format!("mysql://{rust_cache}"))
        .await
        .expect("Rust cache store");
    check_against_go(
        &mut report,
        "cache/mysql.sql",
        &go.read("cache/mysql.sql"),
        &go::dump_mysql(&rust_cache).expect("Go tool dump"),
        &dump::mysql(&rust_cache).await.expect("Rust dump"),
    );

    report.finish();
}
