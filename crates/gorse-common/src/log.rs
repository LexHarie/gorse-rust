// Ports: common/log/log.go @ add21b5 (zap's loggers become tracing events; lumberjack's rotation is ported as RotatingFile)

//! Gorse's logs: the main log, the access log and the OpenAI log, and the
//! redaction of database URLs before they are logged.
//!
//! Go keeps three zap loggers in package variables and replaces them when
//! flags are read. Here the three are tracing layers, rebuilt whenever the
//! set-up changes and swapped into one global subscriber, so code logs with
//! `tracing` macros and picks its log by target: [`ACCESS_TARGET`] and
//! [`OPENAI_TARGET`]. Without a set-up, events go to stderr as JSON at info
//! level, which is zap's production logger. Log lines are tracing's JSON, not
//! zap's (`PORTING_NOTES.md`, X-52).
//!
//! Files rotate as lumberjack does: a file that would grow past the maximum
//! size is renamed with a timestamp, and backups beyond the limits are removed.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use tracing::{Level, Metadata};
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

use crate::gotime;
use crate::gourl;
use crate::mysql_dsn;

/// The tracing target of access log events, Go's `AccessLogger`.
pub const ACCESS_TARGET: &str = "gorse::access";
/// The tracing target of OpenAI log events, Go's `OpenAILogger`.
pub const OPENAI_TARGET: &str = "gorse::openai";

/// The log flags of Go's `AddFlags`, with the same defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// `--log-path`: the log file, or `None` when the flag was not set.
    pub log_path: Option<String>,
    /// `--access-log-path`: the access log file, or empty for none.
    pub access_log_path: String,
    /// `--log-max-size`: megabytes before a file rotates (0 means 100).
    pub max_size: i64,
    /// `--log-max-age`: days a backup is kept (0 keeps them all).
    pub max_age: i64,
    /// `--log-max-backups`: backups kept (0 keeps them all).
    pub max_backups: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            log_path: None,
            access_log_path: String::new(),
            max_size: 100,
            max_age: 0,
            max_backups: 0,
        }
    }
}

/// Where a log line goes.
#[derive(Clone)]
enum Output {
    Stdout,
    Stderr,
    File(Arc<Mutex<RotatingFile>>),
    Capture(Arc<Mutex<Vec<u8>>>),
}

/// Writes each line to every output, as Go's `zapcore.NewMultiWriteSyncer` does.
#[derive(Clone)]
struct Tee(Vec<Output>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut first_error = None;
        for output in &self.0 {
            let result = match output {
                Output::Stdout => io::stdout().lock().write_all(buf),
                Output::Stderr => io::stderr().lock().write_all(buf),
                Output::File(file) => lock(file).write_line(buf),
                Output::Capture(text) => {
                    lock(text).extend_from_slice(buf);
                    Ok(())
                }
            };
            if let Err(err) = result {
                first_error.get_or_insert(err);
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

impl<'a> MakeWriter<'a> for Tee {
    type Writer = Tee;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The three logs and the test capture, as set by [`set_logger`],
/// [`init_openai_logger`] and [`set_test_logger`]: Go's package variables.
#[derive(Clone, Default)]
struct Setup {
    /// The main log's file, if `--log-path` was set.
    main_file: Option<Arc<Mutex<RotatingFile>>>,
    /// Console lines at debug level (`set_logger` with debug); otherwise JSON at info.
    debug: bool,
    /// Whether the main log writes to stdout (`set_logger`); the default is stderr.
    stdout: bool,
    /// The access log's file, if one was set.
    access_file: Option<Arc<Mutex<RotatingFile>>>,
    /// The OpenAI log's file, if one was set.
    openai_file: Option<Arc<Mutex<RotatingFile>>>,
    /// The test capture: when set, it takes every log line.
    capture: Option<Arc<Mutex<Vec<u8>>>>,
    /// Set by [`close_logger`]: the main log is silenced.
    closed: bool,
}

type Layers = Vec<Box<dyn Layer<Registry> + Send + Sync>>;

static SETUP: Mutex<Setup> = Mutex::new(Setup {
    main_file: None,
    debug: false,
    stdout: false,
    access_file: None,
    openai_file: None,
    capture: None,
    closed: false,
});

static HANDLE: OnceLock<reload::Handle<Layers, Registry>> = OnceLock::new();

/// Installs the global subscriber on first use. Another subscriber installed
/// first keeps the events; this handle then changes nothing that is logged.
fn handle(setup: &Setup) -> &'static reload::Handle<Layers, Registry> {
    HANDLE.get_or_init(|| {
        let (layer, handle) = reload::Layer::new(layers(setup));
        let _ = Registry::default().with(layer).try_init();
        handle
    })
}

/// Changes the set-up and rebuilds the layers from it.
fn apply(update: impl FnOnce(&mut Setup)) {
    let mut setup = lock(&SETUP);
    update(&mut setup);
    let current = layers(&setup);
    let _ = handle(&setup).modify(|layers| *layers = current);
}

/// The layers for `setup`: the main log, then the access and OpenAI logs when
/// they have files. A test capture replaces all of them.
fn layers(setup: &Setup) -> Layers {
    if let Some(capture) = &setup.capture {
        let output = Tee(vec![Output::Capture(Arc::clone(capture))]);
        return vec![layer(output, false, Level::DEBUG, |_| true)];
    }
    let mut result = Vec::new();
    // Access lines go to their own file when there is one; otherwise to the main log.
    let access_has_file = setup.access_file.is_some();
    let closed = setup.closed;
    let mut main_outputs = vec![if setup.stdout {
        Output::Stdout
    } else {
        Output::Stderr
    }];
    if let Some(file) = &setup.main_file {
        main_outputs.push(Output::File(Arc::clone(file)));
    }
    let main_level = if setup.debug {
        Level::DEBUG
    } else {
        Level::INFO
    };
    result.push(layer(
        Tee(main_outputs),
        !setup.debug,
        main_level,
        move |target| {
            !closed && target != OPENAI_TARGET && (!access_has_file || target != ACCESS_TARGET)
        },
    ));
    if let Some(file) = &setup.access_file {
        let output = Tee(vec![Output::File(Arc::clone(file))]);
        result.push(layer(output, true, Level::INFO, |target| {
            target == ACCESS_TARGET
        }));
    }
    if let Some(file) = &setup.openai_file {
        let output = Tee(vec![Output::File(Arc::clone(file))]);
        result.push(layer(output, true, Level::INFO, |target| {
            target == OPENAI_TARGET
        }));
    }
    result
}

fn layer(
    output: Tee,
    json: bool,
    level: Level,
    keep: impl Fn(&str) -> bool + Send + Sync + 'static,
) -> Box<dyn Layer<Registry> + Send + Sync> {
    let route = Route {
        output,
        level,
        keep: Arc::new(keep),
    };
    if json {
        fmt::layer().json().with_writer(route).boxed()
    } else {
        fmt::layer().with_writer(route).boxed()
    }
}

/// Sends an event to `output` when its level and target are wanted, and to
/// nothing otherwise. Routing happens in the writer, not in a per-layer filter,
/// because layers are swapped in after the subscriber is built, and a
/// per-layer filter cannot be added to a running subscriber.
#[derive(Clone)]
struct Route {
    output: Tee,
    level: Level,
    keep: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl<'a> MakeWriter<'a> for Route {
    type Writer = Tee;

    fn make_writer(&'a self) -> Tee {
        self.output.clone()
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Tee {
        if meta.level() <= &self.level && (self.keep)(meta.target()) {
            self.output.clone()
        } else {
            Tee(Vec::new())
        }
    }
}

/// Go's `SetLogger`: sets up the main log from the flags, and the access log
/// when `--access-log-path` is set. `debug` selects console lines at debug
/// level; otherwise lines are JSON at info level. Logs go to stdout and, when
/// `--log-path` was set, to that file.
pub fn set_logger(options: &Options, debug: bool) {
    let main_file = options
        .log_path
        .as_deref()
        .map(|path| Arc::new(Mutex::new(RotatingFile::new(path, options))));
    let access_file = (!options.access_log_path.is_empty()).then(|| {
        Arc::new(Mutex::new(RotatingFile::new(
            &options.access_log_path,
            options,
        )))
    });
    apply(|setup| {
        setup.main_file = main_file;
        setup.debug = debug;
        setup.stdout = true;
        if let Some(file) = access_file {
            setup.access_file = Some(file);
        }
    });
}

/// Go's `CloseLogger`: silences the main log. The access and OpenAI logs are kept.
pub fn close_logger() {
    apply(|setup| setup.closed = true);
}

/// Go's `InitOpenAILogger`: JSON lines at info level for OpenAI calls, to
/// `filename`, unless it is empty. Without a file, those lines are dropped.
pub fn init_openai_logger(filename: &str) {
    if filename.is_empty() {
        return;
    }
    let file = Arc::new(Mutex::new(RotatingFile::new(filename, &Options::default())));
    apply(|setup| setup.openai_file = Some(file));
}

/// Go's `SetTestLogger`: sends every log line to a buffer until the guard is
/// dropped, which restores the set-up that was active before.
pub fn set_test_logger() -> TestLogger {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let previous = lock(&SETUP).clone();
    let previous_capture = capture.clone();
    apply(|setup| setup.capture = Some(previous_capture));
    TestLogger { previous, capture }
}

/// The guard [`set_test_logger`] returns.
pub struct TestLogger {
    previous: Setup,
    capture: Arc<Mutex<Vec<u8>>>,
}

impl TestLogger {
    /// Everything logged while the guard was active, as text.
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&lock(&self.capture)).into_owned()
    }
}

impl Drop for TestLogger {
    fn drop(&mut self) {
        let previous = self.previous.clone();
        apply(|setup| *setup = previous);
    }
}

const MYSQL_PREFIX: &str = "mysql://";

/// Go's `RedactDBURL`: a database URL with its user name and password replaced
/// by `x`s, so it can be logged. A SQLite path is returned as it is, and a URL
/// that does not parse is returned unchanged.
pub fn redact_db_url(raw_url: &str) -> String {
    if raw_url.starts_with("sqlite://") {
        raw_url.to_owned()
    } else if let Some(dsn) = raw_url.strip_prefix(MYSQL_PREFIX) {
        let Ok(mut parsed) = mysql_dsn::parse_dsn(dsn) else {
            return raw_url.to_owned();
        };
        parsed.user = vec![b'x'; parsed.user.len()];
        parsed.passwd = vec![b'x'; parsed.passwd.len()];
        format!(
            "{MYSQL_PREFIX}{}",
            String::from_utf8_lossy(&parsed.format_dsn())
        )
    } else {
        let Ok(mut parsed) = gourl::parse(raw_url) else {
            return raw_url.to_owned();
        };
        let (username, password) = match &parsed.user {
            Some(user) => (user.username().len(), user.password().0.len()),
            None => (0, 0),
        };
        parsed.user = Some(gourl::user_password(
            "x".repeat(username),
            "x".repeat(password),
        ));
        parsed.to_string()
    }
}

/// A log file that rotates as lumberjack's `Logger` does: it opens on the
/// first write, renames to a timestamped backup when a write would pass
/// `max_size`, and removes backups beyond `max_backups` or older than `max_age`.
pub struct RotatingFile {
    path: PathBuf,
    max_size: i64,
    max_age: i64,
    max_backups: i64,
    file: Option<File>,
    size: i64,
}

/// Lumberjack's backup timestamp layout.
const BACKUP_TIME_FORMAT: &str = "2006-01-02T15-04-05.000";
/// Lumberjack's default maximum size in megabytes.
const DEFAULT_MAX_SIZE_MB: i64 = 100;
const MEGABYTE: i64 = 1 << 20;

impl RotatingFile {
    fn new(path: &str, options: &Options) -> Self {
        Self {
            path: PathBuf::from(path),
            max_size: options.max_size,
            max_age: options.max_age,
            max_backups: options.max_backups,
            file: None,
            size: 0,
        }
    }

    /// The most bytes a file may hold before it rotates.
    fn max(&self) -> i64 {
        if self.max_size == 0 {
            DEFAULT_MAX_SIZE_MB * MEGABYTE
        } else {
            self.max_size * MEGABYTE
        }
    }

    /// Writes one line, rotating first when the line would not fit.
    pub fn write_line(&mut self, buf: &[u8]) -> io::Result<()> {
        let write_len = buf.len() as i64;
        if self.file.is_none() {
            self.open_existing_or_new(write_len)?;
        }
        if self.size + write_len > self.max() {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is not open"))?;
        file.write_all(buf)?;
        self.size += write_len;
        Ok(())
    }

    fn open_existing_or_new(&mut self, write_len: i64) -> io::Result<()> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return self.open_new(),
            Err(err) => return Err(err),
        };
        let size = metadata.len() as i64;
        if size + write_len >= self.max() {
            return self.rotate();
        }
        self.file = Some(OpenOptions::new().append(true).open(&self.path)?);
        self.size = size;
        Ok(())
    }

    /// Lumberjack's `openNew`: creates the directories and an empty file.
    fn open_new(&mut self) -> io::Result<()> {
        if let Some(dir) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        self.file = Some(
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&self.path)?,
        );
        self.size = 0;
        Ok(())
    }

    /// Renames the current file to a timestamped backup, opens a new file and
    /// removes the backups past the limits.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        let backup = self.backup_name(&gotime::now().format(BACKUP_TIME_FORMAT));
        fs::rename(&self.path, backup)?;
        self.open_new()?;
        self.remove_old_backups();
        Ok(())
    }

    /// `prefix-timestamp.ext` in the file's directory, as lumberjack names backups.
    fn backup_name(&self, timestamp: &str) -> PathBuf {
        let (prefix, ext) = self.prefix_and_ext();
        let name = format!("{prefix}-{timestamp}{ext}");
        match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
            _ => PathBuf::from(name),
        }
    }

    fn prefix_and_ext(&self) -> (String, String) {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match name.rfind('.') {
            Some(dot) if dot > 0 => (name[..dot].to_owned(), name[dot..].to_owned()),
            _ => (name, String::new()),
        }
    }

    /// Lumberjack's `millRun`, run synchronously: backups are sorted by their
    /// timestamps, newest first. Those past `max_backups`, or older than
    /// `max_age` days, are removed.
    fn remove_old_backups(&self) {
        if self.max_backups <= 0 && self.max_age <= 0 {
            return;
        }
        let (prefix, ext) = self.prefix_and_ext();
        let Some(dir) = self
            .path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .or(Some(Path::new(".")))
        else {
            return;
        };
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let start = format!("{prefix}-");
        let mut backups: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let stamp = name.strip_prefix(&start)?.strip_suffix(&ext)?;
                (stamp.len() == BACKUP_TIME_FORMAT.len()).then(|| (stamp.to_owned(), entry.path()))
            })
            .collect();
        backups.sort_by(|a, b| b.0.cmp(&a.0));
        let cutoff = (self.max_age > 0).then(|| {
            let age = gotime::Duration(-(24 * self.max_age) * gotime::HOUR.0);
            gotime::now().add(age).format(BACKUP_TIME_FORMAT)
        });
        for (index, (stamp, path)) in backups.iter().enumerate() {
            let past_backups = self.max_backups > 0 && index as i64 >= self.max_backups;
            let too_old = cutoff
                .as_deref()
                .is_some_and(|cutoff| stamp.as_str() < cutoff);
            if past_backups || too_old {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Go's `util.CheckPanic`-style locking: a poisoned log lock still works, since
/// its state is only a buffer or a file.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    // Ports: common/log/log_test.go @ add21b5

    use std::sync::Mutex;

    use super::*;

    /// The logs are process-wide, so the tests that set them up run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn set_development_logger_writes_to_files() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let temp = tempfile::tempdir().expect("temp dir");
        let options = Options {
            log_path: Some(temp.path().join("gorse.log").display().to_string()),
            ..Options::default()
        };
        // set existed path
        set_logger(&options, true);
        tracing::debug!("test");
        assert!(temp.path().join("gorse.log").is_file());
        // set non-existed path
        let options = Options {
            log_path: Some(
                temp.path()
                    .join("gorse")
                    .join("gorse.log")
                    .display()
                    .to_string(),
            ),
            ..Options::default()
        };
        set_logger(&options, true);
        tracing::debug!("test");
        assert!(temp.path().join("gorse").join("gorse.log").is_file());
        set_logger(&Options::default(), false);
    }

    #[test]
    fn set_production_logger_writes_to_files() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let temp = tempfile::tempdir().expect("temp dir");
        let options = Options {
            log_path: Some(temp.path().join("gorse.log").display().to_string()),
            ..Options::default()
        };
        // set existed path
        set_logger(&options, false);
        tracing::info!("test");
        assert!(temp.path().join("gorse.log").is_file());
        // set non-existed path
        let options = Options {
            log_path: Some(
                temp.path()
                    .join("gorse")
                    .join("gorse.log")
                    .display()
                    .to_string(),
            ),
            ..Options::default()
        };
        set_logger(&options, false);
        tracing::info!("test");
        assert!(temp.path().join("gorse").join("gorse.log").is_file());
        set_logger(&Options::default(), false);
    }

    #[test]
    fn set_test_logger_captures_until_the_guard_drops() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        {
            let guard = super::set_test_logger();
            tracing::info!("test");
            tracing::info!(target: OPENAI_TARGET, "test");
            tracing::info!(target: ACCESS_TARGET, "test");
            let output = guard.output();
            assert!(output.lines().count() >= 3, "{output}");
            assert!(output.contains(ACCESS_TARGET), "{output}");
            assert!(output.contains(OPENAI_TARGET), "{output}");
        }
        // Once the guard is gone, nothing more reaches its buffer.
        let after = super::set_test_logger();
        let text_before = after.output();
        drop(after);
        tracing::info!("after the guard");
        assert!(!text_before.contains("after the guard"));
    }

    #[test]
    fn redact_db_url_cases() {
        assert_eq!(
            redact_db_url("sqlite://data/data.sqlite"),
            "sqlite://data/data.sqlite"
        );
        assert_eq!(
            redact_db_url("mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse?parseTime=true"),
            "mysql://xxxxx:xxxxxxxxxx@tcp(localhost:3306)/gorse?parseTime=true"
        );
        assert_eq!(
            redact_db_url("postgres://bob:secret@1.2.3.4:5432/mydb?sslmode=verify-full"),
            "postgres://xxx:xxxxxx@1.2.3.4:5432/mydb?sslmode=verify-full"
        );
        assert_eq!(
            redact_db_url("mysql://gorse:gorse_pass@tcp(localhost:3306) gorse?parseTime=true"),
            "mysql://gorse:gorse_pass@tcp(localhost:3306) gorse?parseTime=true"
        );
        assert_eq!(
            redact_db_url("postgres://bob:secret@1.2.3.4:5432 mydb?sslmode=verify-full"),
            "postgres://bob:secret@1.2.3.4:5432 mydb?sslmode=verify-full"
        );
    }

    #[test]
    fn rotation_keeps_the_backups_the_flags_allow() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("gorse.log");
        // Megabyte units, as the flags use: 1 MB files, 0.5 MB writes.
        let options = Options {
            log_path: Some(path.display().to_string()),
            max_size: 1,
            max_backups: 2,
            ..Options::default()
        };
        let mut file = RotatingFile::new(&path.display().to_string(), &options);
        let chunk = vec![b'x'; MEGABYTE as usize / 2];
        for _ in 0..8 {
            // Backup names carry millisecond timestamps, as Go's lumberjack names
            // them. Two rotations in one millisecond would give one name, and the
            // second would replace the first backup. The pause keeps each rotation
            // in a millisecond of its own, however fast the writes run.
            std::thread::sleep(std::time::Duration::from_millis(2));
            file.write_line(&chunk).expect("write");
        }
        let names: Vec<String> = fs::read_dir(temp.path())
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let backups = names
            .iter()
            .filter(|name| name.starts_with("gorse-"))
            .count();
        assert!(names.iter().any(|name| name == "gorse.log"));
        assert_eq!(backups, 2, "{names:?}");
    }
}
