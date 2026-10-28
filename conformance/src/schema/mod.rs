// Ports: contracts/tools/schemadump/main.go, contracts/tools/dump-schemas.sh and conformance/README.md (WP-2-13).

//! The schema conformance checker (WP-2-13).
//!
//! Each backend is initialised twice. The Go build does it through
//! `contracts/tools/schemadump` (see [`go`]). The Rust stores do it through
//! [`init`], which runs what the Go tool runs: `Init`, `Reconcile` and the sample
//! writes. Both results are dumped by [`dump`], which writes what the Go tool
//! writes, and each dump must equal the other and the frozen file in
//! `contracts/schema/`.
//!
//! [`dump`] is checked against the Go tool on the databases the Go run leaves
//! behind, so a difference in a Rust dump is a difference in the schema, not in
//! the dumper.

pub mod dump;
pub mod go;
pub mod gojson;
pub mod init;

use std::fmt::{Display, Write as _};
use std::path::{Path, PathBuf};

use gorse_common::mysql_dsn::parse_dsn;
use sqlx::mysql::MySqlConnectOptions;

/// A failed schema step: a store, a server, a dump or the Go run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

/// The result of a schema step.
pub type Result<T> = std::result::Result<T, Error>;

/// Names a failure as an [`Error`].
pub fn fail(e: impl Display) -> Error {
    Error(e.to_string())
}

/// The connection options of a Go-style MySQL DSN, `user:password@tcp(host:port)/database?params`.
/// The database is optional, as the server-level connection needs none.
pub fn mysql_options(dsn: &str) -> Result<MySqlConnectOptions> {
    let cfg = parse_dsn(dsn).map_err(fail)?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let addr = text(&cfg.addr);
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| fail(format!("MySQL address {addr:?} has no port")))?;
    let port: u16 = port.parse().map_err(fail)?;
    let mut options = MySqlConnectOptions::new()
        .host(host)
        .port(port)
        .username(&text(&cfg.user))
        .password(&text(&cfg.passwd));
    let database = text(&cfg.db_name);
    if !database.is_empty() {
        options = options.database(&database);
    }
    Ok(options)
}

/// The root of the repository: the parent of the `conformance` crate.
pub fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the conformance crate sits in the repository root")
}

/// Reads `contracts/schema/<file>`, the frozen expectation. Panics when the file
/// is missing, since the contract is part of the repository.
pub fn contract(file: &str) -> String {
    let path: PathBuf = repo_root().join("contracts/schema").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("contract {}: {e}", path.display()))
}

/// The FT.INFO fields whose values are wall-clock measurements. On both sides
/// each value is replaced by `<run-dependent>` before a comparison. This is the
/// only normalisation the schema checker applies (conformance/README.md).
const RUN_DEPENDENT_FT_INFO_FIELDS: &[&str] = &["total_indexing_time"];

/// Replaces the run-dependent FT.INFO values with `<run-dependent>`.
pub fn normalise_ft_info(text: &str) -> String {
    let mut out = text.to_owned();
    for field in RUN_DEPENDENT_FT_INFO_FIELDS {
        let marker = format!("{field} ");
        let mut result = String::new();
        let mut rest = out.as_str();
        while let Some(pos) = rest.find(&marker) {
            let (before, after) = rest.split_at(pos + marker.len());
            result.push_str(before);
            // The value runs to the next space or the closing bracket.
            let end = after.find([' ', ']']).unwrap_or(after.len());
            result.push_str("<run-dependent>");
            rest = &after[end..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// The mismatches of one test. Every mismatch is recorded, so one run reports
/// them all, and [`Report::finish`] fails the test when there is any.
#[derive(Debug, Default)]
pub struct Report {
    mismatches: Vec<String>,
}

impl Report {
    /// Records a mismatch, with the first differing lines, when `expected` and
    /// `actual` are not equal.
    pub fn same(&mut self, label: &str, expected: &str, actual: &str) {
        if expected != actual {
            self.mismatches
                .push(format!("{label}\n{}", describe_diff(expected, actual)));
        }
    }

    /// Fails the test, listing every recorded mismatch, if there is any.
    pub fn finish(self) {
        assert!(
            self.mismatches.is_empty(),
            "{} schema mismatch(es):\n\n{}",
            self.mismatches.len(),
            self.mismatches.join("\n\n")
        );
    }
}

/// The first lines where two texts differ, with their line numbers.
pub fn describe_diff(expected: &str, actual: &str) -> String {
    const SHOWN: usize = 12;
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    let mut shown = 0;
    for i in 0..expected.len().max(actual.len()) {
        let (e, a) = (expected.get(i), actual.get(i));
        if e == a {
            continue;
        }
        if shown == SHOWN {
            out.push_str("...\n");
            break;
        }
        let _ = writeln!(
            out,
            "line {}:\n  expected: {}\n  actual:   {}",
            i + 1,
            e.unwrap_or(&"<end of text>"),
            a.unwrap_or(&"<end of text>")
        );
        shown += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_normalise_replaces_only_the_timing_values() {
        let text = "[total_indexing_time 0.092609 indexing 0 total_indexing_time 1.5]";
        assert_eq!(
            normalise_ft_info(text),
            "[total_indexing_time <run-dependent> indexing 0 total_indexing_time <run-dependent>]"
        );
        assert_eq!(normalise_ft_info("[num_docs 1]"), "[num_docs 1]");
    }

    #[test]
    fn schema_describe_diff_shows_the_first_differing_lines() {
        let diff = describe_diff("a\nb\nc", "a\nB\nc\nd");
        assert_eq!(
            diff,
            "line 2:\n  expected: b\n  actual:   B\nline 4:\n  expected: <end of text>\n  actual:   d\n"
        );
    }

    #[test]
    #[should_panic(expected = "2 schema mismatch(es)")]
    fn schema_report_lists_every_mismatch() {
        let mut report = Report::default();
        report.same("first", "x", "y");
        report.same("same", "x", "x");
        report.same("second", "x", "z");
        report.finish();
    }
}
