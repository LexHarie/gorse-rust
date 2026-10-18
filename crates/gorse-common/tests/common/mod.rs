//! Helpers shared by the `gotime` and `dateparse` test binaries.
//!
//! Zones come from the TZif files in `tests/data/zoneinfo` and the stand-in
//! GOROOT zip in `tests/data/goroot`, which `contracts/tools/timegolden`
//! copied from Go's own `zoneinfo.zip`, so the tests do not depend on the
//! tzdata of the machine they run on.

#![allow(dead_code, reason = "each test binary uses a subset of the helpers")]

use std::sync::{Mutex, MutexGuard, Once, PoisonError};

use gorse_common::gotime::{self, Location, Time, internal};

/// The `tests/data` directory.
pub fn data_dir() -> String {
    format!("{}/tests/data", env!("CARGO_MANIFEST_DIR"))
}

/// The stand-in GOROOT whose `lib/time/zoneinfo.zip` holds every test zone.
pub fn goroot() -> String {
    format!("{}/goroot", data_dir())
}

/// The TZif bytes of an IANA zone.
pub fn tzdata(name: &str) -> Vec<u8> {
    let path = format!("{}/zoneinfo/{name}", data_dir());
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A location the way the golden tool names one: "UTC", "Local" (the current
/// `time.Local`), "Fixed:<name>:<offset>", or an IANA zone loaded with
/// `LoadLocationFromTZData` from the committed TZif bytes.
pub fn location(name: &str) -> Location {
    if name == "UTC" {
        return gotime::utc();
    }
    if name == "Local" {
        return gotime::local();
    }
    if let Some(rest) = name.strip_prefix("Fixed:") {
        let (zone, offset) = rest.rsplit_once(':').expect("Fixed:<name>:<offset>");
        return gotime::fixed_zone(zone, offset.parse().expect("offset"));
    }
    gotime::load_location_from_tz_data(name, &tzdata(name)).expect("TZif data")
}

/// Sets `time.Local` as the golden tool's `setLocal` does: "UTC" is
/// `time.UTC` itself, anything else a location from [`location`].
pub fn set_local(name: &str) {
    if name == "UTC" {
        gotime::set_local(gotime::utc());
        return;
    }
    gotime::set_local(location(name));
}

static STATE: Mutex<()> = Mutex::new(());

/// Serializes tests that change process-wide state (`time.Local`, the zone
/// sources).
pub fn lock() -> MutexGuard<'static, ()> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

static GO_SETUP: Once = Once::new();

/// The environment Go's own time tests run in: `time.Local` is
/// America/Los_Angeles named "Local", loaded from the GOROOT zip
/// (`ForceUSPacificForTesting`), and zones load from the GOROOT zip only.
///
/// Go's tests read the system zone directories wherever they do not call
/// `DisablePlatformSources`; here they are off for the whole binary so that
/// every zone comes from the same Go tzdata.
pub fn go_setup() {
    GO_SETUP.call_once(|| {
        std::mem::forget(internal::disable_platform_sources());
        internal::set_default_goroot(&goroot());
        let source = internal::goroot_zone_source(&goroot()).expect("goroot");
        let la = internal::load_location_from_sources("America/Los_Angeles", &[&source])
            .expect("cannot load America/Los_Angeles for testing");
        gotime::set_local(la.renamed("Local"));
    });
}

/// Set in the environment of a test re-run in a child process.
pub const CHILD_ENV: &str = "GORSE_GOTIME_TEST_CHILD";

/// Reports whether this process is a child started by [`run_in_child`].
pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// Re-runs the test `name` (its full path, e.g. `gotime::test_env_tz_usage`)
/// in a child process with `env` applied (`None` removes a variable), for the
/// Go tests that set environment variables, which a Rust test cannot do in
/// its own process without `unsafe`. Panics with the child's output if it
/// fails.
pub fn run_in_child(name: &str, env: &[(&str, Option<&str>)]) {
    let exe = std::env::current_exe().expect("test binary");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([name, "--exact", "--nocapture", "--test-threads=1"]);
    cmd.env(CHILD_ENV, "1");
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    let out = cmd.output().expect("run child test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "child {name} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// What the golden tool records about a time: Unix seconds, nanoseconds, zone
/// abbreviation and offset, location name and `Time.String`.
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
pub struct TimeResult {
    pub unix: String,
    pub nsec: i64,
    pub zone: String,
    pub offset: i64,
    pub location: String,
    pub string: String,
}

/// The [`TimeResult`] of t.
pub fn describe(t: &Time) -> TimeResult {
    let (zone, offset) = t.zone();
    TimeResult {
        unix: t.unix().to_string(),
        nsec: t.nanosecond(),
        zone,
        offset,
        location: t.location().name().to_owned(),
        string: t.string(),
    }
}

/// Reads a golden JSON file from `tests/data`.
pub fn load_golden<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/{name}", data_dir());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Go's `encoding/json` replaces each invalid UTF-8 byte of a string with
/// U+FFFD; this does the same to Rust output before comparing it with a
/// golden string.
pub fn go_json_string(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push('\u{FFFD}');
        }
    }
    out
}
