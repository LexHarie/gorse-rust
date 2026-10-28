// Ports: storage/meta/sqlite.go @ add21b5

//! The SQLite meta store.
//!
//! Go's driver is modernc's pure-Go SQLite. This port uses `rusqlite` with
//! the bundled SQLite library (`porting/crate-selection.md`). Every call runs
//! on `spawn_blocking` (D-122). The store has one connection, behind a std
//! `Mutex` that only those blocking tasks lock; no lock is held across an
//! `.await`.
//!
//! A time is stored as `Time.String()` of its UTC value, which is what
//! modernc writes for a `time.Time`. The `nodes` table's `update_time` is
//! compared as text with `datetime('now', ...)`, so the two forms have to
//! match for the TTL check to work across Go and Rust nodes.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use gorse_common::gotime::{self, Duration, Time};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Database, Error, Node};

/// Go's `busy_timeout(10000)`: how long a call waits for a lock that another
/// connection holds.
const BUSY_TIMEOUT: StdDuration = StdDuration::from_millis(10_000);

/// The layout of `Time.String()`: how the store writes a time, and the first
/// layout it reads one with.
const STRING_LAYOUT: &str = "2006-01-02 15:04:05.999999999 -0700 MST";

/// The layouts modernc tries after [`STRING_LAYOUT`], in its order.
const PARSE_LAYOUTS: [&str; 7] = [
    "2006-01-02 15:04:05.999999999-07:00",
    "2006-01-02T15:04:05.999999999-07:00",
    "2006-01-02 15:04:05.999999999",
    "2006-01-02T15:04:05.999999999",
    "2006-01-02 15:04",
    "2006-01-02T15:04",
    "2006-01-02",
];

/// Go's `SQLite`: the meta store over one SQLite file.
pub struct SQLite {
    /// The connection, which is `None` once the store is closed.
    conn: Arc<Mutex<Option<Connection>>>,
    /// How long a node may go without reporting before it is dropped.
    ttl: Duration,
}

impl SQLite {
    /// Go's `Open` for SQLite. Go's `sql.Open` connects on first use. This
    /// connects now, so an unusable file fails here and not at `Init`.
    ///
    /// The file is the part of `data_source_name` before any `?`. Go writes
    /// its two pragmas into the query string, and this sets them on the
    /// connection directly. No other query parameter is read; the only caller
    /// passes none.
    pub(super) async fn open(data_source_name: &str, ttl: Duration) -> Result<SQLite, Error> {
        let file = data_source_name
            .split_once('?')
            .map_or(data_source_name, |(file, _)| file)
            .to_owned();
        let conn = blocking(move || {
            let conn = Connection::open(file)?;
            conn.busy_timeout(BUSY_TIMEOUT)?;
            conn.pragma_update(None, "journal_mode", "wal")?;
            Ok(conn)
        })
        .await?;
        Ok(SQLite {
            conn: Arc::new(Mutex::new(Some(conn))),
            ttl,
        })
    }

    /// Runs `f` with the connection, on the blocking pool.
    async fn call<R, F>(&self, f: F) -> Result<R, Error>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R, Error> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        blocking(move || {
            let guard = conn.lock().unwrap_or_else(PoisonError::into_inner);
            match guard.as_ref() {
                Some(conn) => f(conn),
                None => Err(Error::Closed),
            }
        })
        .await
    }

    /// The `-N seconds` modifier that `datetime('now', ...)` takes for the TTL,
    /// formatted as Go's `%.0f` formats it.
    fn ttl_modifier(&self) -> String {
        format!("-{:.0} seconds", self.ttl.seconds())
    }
}

/// Runs `task` on the blocking pool. A panic in the task is resumed on the
/// caller, as the same code would panic if it ran there.
async fn blocking<R, F>(task: F) -> Result<R, Error>
where
    R: Send + 'static,
    F: FnOnce() -> Result<R, Error> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
        Err(_) => Err(Error::TaskCancelled),
    }
}

/// How a time is written: `Time.String()` of its UTC value, as modernc binds a
/// `time.Time` after Go's `UTC()`.
pub(super) fn format_time(time: &Time) -> String {
    time.utc().string()
}

/// Reads a stored time as modernc does. It tries `Time.String()` first, with
/// any monotonic clock suffix cut off. Then it tries the other layouts, after
/// a trailing `Z` is removed.
pub(super) fn parse_time(text: &str) -> Result<Time, Error> {
    let string_form = match text.find("m=") {
        Some(x) if x > 0 => &text[..x],
        _ => text,
    };
    if let Ok(time) = gotime::parse(STRING_LAYOUT, string_form.trim()) {
        return Ok(time);
    }
    let value = text.strip_suffix('Z').unwrap_or(text);
    for layout in PARSE_LAYOUTS {
        if let Ok(time) = gotime::parse(layout, value) {
            return Ok(time);
        }
    }
    Err(Error::UnsupportedScan)
}

#[async_trait]
impl Database for SQLite {
    async fn close(&self) -> Result<(), Error> {
        let conn = Arc::clone(&self.conn);
        blocking(move || {
            let taken = conn.lock().unwrap_or_else(PoisonError::into_inner).take();
            match taken {
                Some(conn) => conn.close().map_err(|(_, err)| Error::Sqlite(err)),
                // Go's `DB.Close` is idempotent and returns nil the second time.
                None => Ok(()),
            }
        })
        .await
    }

    async fn init(&self) -> Result<(), Error> {
        self.call(|conn| {
            conn.execute(
                "
CREATE TABLE IF NOT EXISTS nodes (
	uuid TEXT PRIMARY KEY,
	hostname TEXT,
	type TEXT,
	version TEXT,
	update_time DATETIME
);",
                [],
            )?;
            conn.execute(
                "
CREATE TABLE IF NOT EXISTS cron_jobs (
	name TEXT PRIMARY KEY,
	description TEXT,
	current INTEGER,
	total INTEGER,
	start_time TIMESTAMP,
	end_time TIMESTAMP,
	update_time TIMESTAMP
);",
                [],
            )?;
            conn.execute(
                "
CREATE TABLE IF NOT EXISTS key_values (
	key TEXT PRIMARY KEY,
	value TEXT
);",
                [],
            )?;
            Ok(())
        })
        .await
    }

    async fn update_node(&self, node: &Node) -> Result<(), Error> {
        let uuid = node.uuid.clone();
        let hostname = node.hostname.clone();
        let node_type = node.r#type.clone();
        let version = node.version.clone();
        let update_time = format_time(&node.update_time);
        self.call(move |conn| {
            conn.execute(
                "
INSERT INTO nodes (uuid, hostname, type, version, update_time)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(uuid) DO UPDATE SET
	hostname = excluded.hostname,
	type = excluded.type,
	version = excluded.version,
	update_time = excluded.update_time
",
                params![uuid, hostname, node_type, version, update_time],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_nodes(&self) -> Result<Vec<Node>, Error> {
        let ttl = self.ttl_modifier();
        self.call(move |conn| {
            // The rows are read before the outdated nodes are deleted, as Go does.
            let nodes = {
                let mut stmt = conn.prepare(
                    "
SELECT uuid, hostname, type, version, update_time FROM nodes
WHERE update_time > datetime('now', ?)
",
                )?;
                let mut rows = stmt.query([ttl.as_str()])?;
                let mut nodes = Vec::new();
                while let Some(row) = rows.next()? {
                    nodes.push(Node {
                        uuid: row.get(0)?,
                        hostname: row.get(1)?,
                        r#type: row.get(2)?,
                        version: row.get(3)?,
                        update_time: parse_time(&row.get::<_, String>(4)?)?,
                    });
                }
                nodes
            };
            conn.execute(
                "
DELETE FROM nodes WHERE update_time < datetime('now', ?)
",
                [ttl.as_str()],
            )?;
            Ok(nodes)
        })
        .await
    }

    async fn put(&self, key: &str, value: &str) -> Result<(), Error> {
        let key = key.to_owned();
        let value = value.to_owned();
        self.call(move |conn| {
            conn.execute(
                "
INSERT INTO key_values (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value
",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }

    async fn get(&self, key: &str) -> Result<Option<String>, Error> {
        let key = key.to_owned();
        self.call(move |conn| {
            // No row is a missing key, which Go reports as a nil pointer.
            Ok(conn
                .query_row(
                    "
SELECT value FROM key_values WHERE key = ?
",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<(), Error> {
        let key = key.to_owned();
        self.call(move |conn| {
            conn.execute(
                "
DELETE FROM key_values WHERE key = ?
",
                params![key],
            )?;
            Ok(())
        })
        .await
    }
}
