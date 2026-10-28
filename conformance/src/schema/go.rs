// Ports: contracts/tools/schemadump/main.go (the full run and its -dump-only mode) @ WP-2-13.

//! Runs the Go build's schema tool, `go run ./contracts/tools/schemadump`.
//!
//! [`run`] is the full run: it initialises each store with the Go build and
//! writes the files the contract holds. The `dump_*` functions are its
//! `-dump-only` mode: each dumps one existing store to stdout and changes
//! nothing, so the Go tool can dump a database the Rust stores initialised.

use std::path::PathBuf;
use std::process::Command;

use crate::schema::{Error, Result, fail, repo_root};

/// The backends to initialise with the Go build. A backend left as `None` is not
/// touched. SQLite and the meta store are always initialised.
#[derive(Debug, Default, Clone, Copy)]
pub struct Targets<'a> {
    /// A MySQL or MariaDB DSN without a database, such as
    /// `root:password@tcp(127.0.0.1:3306)/`.
    pub mysql: Option<&'a str>,
    /// A PostgreSQL URL without a database, such as
    /// `postgres://gorse:gorse_pass@127.0.0.1:5432/`.
    pub postgres: Option<&'a str>,
    /// A MongoDB URL without a database, such as `mongodb://root:password@127.0.0.1:27017/`.
    pub mongo: Option<&'a str>,
    /// A Redis URL, such as `redis://127.0.0.1:6379/`.
    pub redis: Option<&'a str>,
}

/// The files one full run wrote, in a directory that is removed with the value.
#[derive(Debug)]
pub struct Output {
    dir: tempfile::TempDir,
}

impl Output {
    /// The text of one file the run wrote, such as `data/mysql.sql`. Panics when
    /// the file is missing: the Go run either wrote it or the test has failed.
    pub fn read(&self, file: &str) -> String {
        let path: PathBuf = self.dir.path().join(file);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Go schemadump did not write {}: {e}", path.display()))
    }
}

/// Runs `go run ./contracts/tools/schemadump` from the repository root with the
/// targets given. Its stderr is returned on failure.
pub fn run(targets: &Targets) -> Result<Output> {
    let dir = tempfile::tempdir().map_err(fail)?;
    let mut command = Command::new("go");
    command
        .current_dir(repo_root())
        .args(["run", "./contracts/tools/schemadump", "-out"])
        .arg(dir.path());
    if let Some(dsn) = targets.mysql {
        command.args(["-mysql", dsn]);
    }
    if let Some(url) = targets.postgres {
        command.args(["-postgres", url]);
    }
    if let Some(url) = targets.mongo {
        command.args(["-mongo", url]);
    }
    if let Some(url) = targets.redis {
        command.args(["-redis", url]);
    }
    let output = command
        .output()
        .map_err(|e| fail(format!("starting go: {e}")))?;
    if !output.status.success() {
        return Err(failed(&output));
    }
    Ok(Output { dir })
}

/// The Go tool's dump of a SQLite file (`-dump-only sqlite`).
pub fn dump_sqlite(path: &std::path::Path) -> Result<String> {
    let target = path.display().to_string();
    dump_only(&["sqlite", "-target", &target])
}

/// The Go tool's dump of a MySQL or MariaDB database (`-dump-only mysql`). `dsn`
/// names the database, as `root:password@tcp(127.0.0.1:3306)/gorse_rs_schema_data?parseTime=true`.
pub fn dump_mysql(dsn: &str) -> Result<String> {
    dump_only(&["mysql", "-target", dsn])
}

/// The Go tool's dump of a PostgreSQL database, with `pg_dump` run in
/// `container` (`-dump-only postgres`).
pub fn dump_postgres(container: &str, database: &str) -> Result<String> {
    dump_only(&[
        "postgres",
        "-postgres-container",
        container,
        "-database",
        database,
    ])
}

/// The Go tool's dump of a MongoDB database. `uri` is the server without a
/// database (`-dump-only mongodb`).
pub fn dump_mongodb(uri: &str, database: &str) -> Result<String> {
    dump_only(&["mongodb", "-target", uri, "-database", database])
}

/// The Go tool's `FT.INFO documents` text for a Redis server (`-dump-only redis`).
/// Each call is a read that the index counts (`number_of_uses`).
pub fn dump_redis(url: &str) -> Result<String> {
    dump_only(&["redis", "-target", url])
}

/// Runs the `-dump-only` mode with the kind and flags in `args` and returns the
/// dump it writes to stdout.
fn dump_only(args: &[&str]) -> Result<String> {
    let output = Command::new("go")
        .current_dir(repo_root())
        .args(["run", "./contracts/tools/schemadump", "-dump-only"])
        .args(args)
        .output()
        .map_err(|e| fail(format!("starting go: {e}")))?;
    if !output.status.success() {
        return Err(failed(&output));
    }
    String::from_utf8(output.stdout).map_err(fail)
}

/// The error for a `go run` that exited with a failure.
fn failed(output: &std::process::Output) -> Error {
    Error(format!(
        "go run ./contracts/tools/schemadump failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    ))
}
