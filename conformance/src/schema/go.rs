// Ports: contracts/tools/schemadump/main.go (the Go build initialises each backend and writes the dumps the contract holds) @ WP-2-13.

//! Runs the Go build's schema dump: `go run ./contracts/tools/schemadump`, the
//! tool `contracts/schema/` was generated with.
//!
//! The Go tool creates its own databases on each server (`gorse_schema_*`) and
//! leaves them in place, so the dumper can check its output against them.

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

/// The files one Go run wrote, in a directory that is removed with the value.
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
        return Err(Error(format!(
            "go run ./contracts/tools/schemadump failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(Output { dir })
}
