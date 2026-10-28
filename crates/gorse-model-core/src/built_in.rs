// Ports: model/built_in.go @ add21b5

//! The built-in datasets that models and the `dataset` package load by name,
//! and the directories under `~/.gorse` that hold them.
//!
//! [`locate_built_in_dataset`] downloads a dataset's zip from `cdn.gorse.io` the
//! first time it is needed and unpacks it into [`dataset_dir`]. Go's package
//! `init` also creates [`dataset_dir`] and [`temp_dir`] when the process starts.
//! Here [`create_built_in_dirs`] does that, and the binaries call it at startup
//! (`PORTING_NOTES.md`).
//!
//! The home directory comes from `$HOME` (`PORTING_NOTES.md`, X-55).

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

/// The format of a dataset's files. The discriminants are Go's `iota` values,
/// which the format-mismatch error prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetFormat {
    /// Go `FormatNCF`.
    Ncf = 0,
    /// Go `FormatLibFM`.
    LibFm = 1,
}

/// The errors of [`locate_built_in_dataset`] and the helpers under it.
#[derive(Debug, Error)]
pub enum Error {
    /// The name is not a built-in dataset.
    #[error("no such dataset {0}")]
    NoSuchDataset(String),
    /// The dataset is in another format. The numbers are the format values, as
    /// Go prints them (and Go's message spells "matchs").
    #[error("format not matchs {requested} != {actual}")]
    FormatMismatch {
        /// The format the caller asked for.
        requested: i64,
        /// The format the dataset is in.
        actual: i64,
    },
    /// A file or directory could not be created, read or written.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A zip archive could not be read.
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    /// A download failed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// An archive entry would be written outside the destination directory.
    #[error("{0}: illegal file path")]
    IllegalPath(PathBuf),
}

/// One entry of Go's `builtInDataSets`.
struct BuiltInDataSet {
    download_url: &'static str,
    train_file: &'static str,
    test_file: &'static str,
    format: DatasetFormat,
}

/// Go's `builtInDataSets`, by name.
const BUILT_IN_DATA_SETS: &[(&str, BuiltInDataSet)] = &[
    (
        "pinterest-20",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/pinterest-20.zip",
            train_file: "pinterest-20/train.txt",
            test_file: "pinterest-20/test.txt",
            format: DatasetFormat::Ncf,
        },
    ),
    (
        "ml-100k",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/ml-100k.zip",
            train_file: "ml-100k/train.txt",
            test_file: "ml-100k/test.txt",
            format: DatasetFormat::Ncf,
        },
    ),
    (
        "ml-1m",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/ml-1m.zip",
            train_file: "ml-1m/train.txt",
            test_file: "ml-1m/test.txt",
            format: DatasetFormat::Ncf,
        },
    ),
    (
        "ml-tag",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/ml-tag.zip",
            train_file: "ml-tag/train.libfm",
            test_file: "ml-tag/test.libfm",
            format: DatasetFormat::LibFm,
        },
    ),
    (
        "frappe",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/frappe.zip",
            train_file: "frappe/train.libfm",
            test_file: "frappe/test.libfm",
            format: DatasetFormat::LibFm,
        },
    ),
    (
        "criteo",
        BuiltInDataSet {
            download_url: "https://cdn.gorse.io/datasets/criteo.zip",
            train_file: "criteo/train.libfm",
            test_file: "criteo/test.libfm",
            format: DatasetFormat::LibFm,
        },
    ),
];

/// Go's `GorseDir`: `~/.gorse`.
pub fn gorse_dir() -> PathBuf {
    home_dir().join(".gorse")
}

/// Go's `DataSetDir`: `~/.gorse/dataset`.
pub fn dataset_dir() -> PathBuf {
    gorse_dir().join("dataset")
}

/// Go's `TempDir`: `~/.gorse/temp`.
pub fn temp_dir() -> PathBuf {
    gorse_dir().join("temp")
}

/// Creates [`dataset_dir`] and [`temp_dir`], which Go's package `init` creates
/// when the process starts. Go stops the process when this fails; the binaries
/// return the error instead.
pub fn create_built_in_dirs() -> io::Result<()> {
    create_dirs(&dataset_dir(), &temp_dir())
}

/// Go's `LocateBuiltInDataset`: the paths of the training and test files of a
/// built-in dataset. The zip is downloaded and unpacked first when the training
/// file is missing.
pub fn locate_built_in_dataset(
    name: &str,
    format: DatasetFormat,
) -> Result<(PathBuf, PathBuf), Error> {
    locate_built_in_dataset_in(name, format, &dataset_dir(), &temp_dir())
}

/// [`locate_built_in_dataset`] with the directories given, so tests can use
/// temporary ones.
fn locate_built_in_dataset_in(
    name: &str,
    format: DatasetFormat,
    dataset_dir: &Path,
    temp_dir: &Path,
) -> Result<(PathBuf, PathBuf), Error> {
    let data_set = BUILT_IN_DATA_SETS
        .iter()
        .find(|(data_set_name, _)| *data_set_name == name)
        .map(|(_, data_set)| data_set)
        .ok_or_else(|| Error::NoSuchDataset(name.to_owned()))?;
    if data_set.format != format {
        return Err(Error::FormatMismatch {
            requested: format as i64,
            actual: data_set.format as i64,
        });
    }
    let train_file_path = dataset_dir.join(data_set.train_file);
    let test_file_path = dataset_dir.join(data_set.test_file);
    // As Go's `os.IsNotExist`: only a missing training file triggers a download.
    if fs::metadata(&train_file_path).is_err_and(|err| err.kind() == io::ErrorKind::NotFound) {
        // Go ignores the download's error. A failed download leaves an empty
        // file, and the unzip reports that.
        let (zip_file_name, _) = download_from_url(data_set.download_url, temp_dir);
        unzip(&zip_file_name, dataset_dir)?;
    }
    Ok((train_file_path, test_file_path))
}

/// Go's `createDirs` inside `init`: both directories, created with their parents.
fn create_dirs(dataset_dir: &Path, temp_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dataset_dir)?;
    fs::create_dir_all(temp_dir)
}

/// The home directory. Go asks the password database (`user.Current`); this
/// reads `$HOME`, and gives an empty path when it is unset (`PORTING_NOTES.md`,
/// X-55).
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Go's `downloadFromUrl`: saves `src` under `dst`, named by the last segment of
/// its URL. The path comes back even when the download fails.
///
/// The status code is not checked: as with Go's `http.Get`, the body is saved
/// whatever it says, so an error page becomes the zip (`PORTING_NOTES.md`).
fn download_from_url(src: &str, dst: &Path) -> (PathBuf, Result<(), Error>) {
    tracing::info!(source = src, "Download dataset");
    let file_name = dst.join(src.rsplit('/').next().unwrap_or(src));
    let result = save_url(src, &file_name);
    (file_name, result)
}

/// The body of [`download_from_url`], with the file name given.
fn save_url(src: &str, file_name: &Path) -> Result<(), Error> {
    if let Some(dir) = file_name.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut output = match File::create(file_name) {
        Ok(output) => output,
        Err(err) => {
            tracing::error!(error = %err, filename = %file_name.display(), "failed to create file");
            return Err(err.into());
        }
    };
    // Go's `http.Get` has no timeout. The blocking client's default is 30 seconds,
    // which would cut off a large dataset, so it is turned off.
    let client = reqwest::blocking::Client::builder()
        .timeout(None::<Duration>)
        .build()?;
    let mut response = match client.get(src).send() {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, source = src, "failed to download");
            return Err(err.into());
        }
    };
    if let Err(err) = io::copy(&mut response, &mut output) {
        tracing::error!(error = %err, source = src, "failed to download");
        return Err(err.into());
    }
    Ok(())
}

/// Go's `unzip`: extracts the archive `src` under `dst` and returns the paths of
/// the entries, directories included.
///
/// Go keeps the paths it has written before an error and returns them with the
/// error; no caller reads them then, so this returns only the error.
/// Files get default permissions, not the archive's (`PORTING_NOTES.md`, X-56).
fn unzip(src: &Path, dst: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut archive = zip::ZipArchive::new(File::open(src)?)?;
    let clean_dst = lexical_clean(dst);
    let mut file_names = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // Go joins the destination with the entry name, so a leading `/` in the
        // name only makes the entry's path deeper, and the check below applies.
        let name = entry.name()?;
        let file_path = lexical_clean(&dst.join(name.trim_start_matches('/')));
        // Go's ZipSlip check: the entry must lie strictly inside the destination.
        if file_path == clean_dst || !file_path.starts_with(&clean_dst) {
            return Err(Error::IllegalPath(file_path));
        }
        file_names.push(file_path.clone());
        if entry.is_dir() {
            fs::create_dir_all(&file_path)?;
        } else {
            if let Some(dir) = file_path.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut output = File::create(&file_path)?;
            io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(file_names)
}

/// Cleans a path lexically, as Go's `filepath.Clean` does: `.` goes, `..`
/// removes the element before it, a `..` at the root stays at the root, and a
/// relative path keeps a leading `..`. The empty path becomes `.`.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match cleaned.components().next_back() {
                Some(Component::Normal(_)) => {
                    cleaned.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => cleaned.push(".."),
            },
            other => cleaned.push(other),
        }
    }
    if cleaned.as_os_str().is_empty() {
        cleaned.push(".");
    }
    cleaned
}

#[cfg(test)]
mod tests {
    // Ports: model/built_in_test.go @ add21b5. The helpers the Go tests call
    // (`downloadFromUrl`, `unzip`) are private here as in Go, so the tests live
    // in this module.

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use super::*;

    /// Go's `TestUnzip`. It downloads from `cdn.gorse.io`, which this container
    /// cannot reach, so it is ignored here (O-1) and runs where the CDN is reachable.
    #[test]
    #[ignore = "needs cdn.gorse.io, which this container cannot reach (O-1)"]
    fn unzip_downloaded_yelp_archive() {
        let (zip_name, result) = download_from_url(
            "https://cdn.gorse.io/datasets/yelp.zip",
            &std::env::temp_dir(),
        );
        result.expect("download file");
        let file_names = unzip(&zip_name, &dataset_dir()).expect("unzip file");
        assert_eq!(file_names.len(), 2, "Number of file doesn't match");
    }

    /// Go's `TestLocateBuiltInDataset`. It downloads `ml-1m` from `cdn.gorse.io`,
    /// which this container cannot reach, so it is ignored here (O-1).
    #[test]
    #[ignore = "needs cdn.gorse.io, which this container cannot reach (O-1)"]
    fn locate_built_in_dataset_ml_1m() {
        let (train, test) =
            locate_built_in_dataset("ml-1m", DatasetFormat::Ncf).expect("locate dataset");
        assert_eq!(train, dataset_dir().join("ml-1m").join("train.txt"));
        assert_eq!(test, dataset_dir().join("ml-1m").join("test.txt"));
    }

    /// Serves one HTTP response to one request on a local port, and returns the
    /// URL of the served path.
    fn serve_once(response: &'static [u8]) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local address").port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            stream.write_all(response).expect("write response");
        });
        (format!("http://127.0.0.1:{port}/data.zip"), handle)
    }

    #[test]
    fn download_saves_the_body_under_the_last_url_segment() {
        let temp = tempfile::tempdir().expect("temp dir");
        let (url, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nzip bytes",
        );
        let (path, result) = download_from_url(&url, temp.path());
        server.join().expect("server");
        result.expect("download");
        assert_eq!(path, temp.path().join("data.zip"));
        assert_eq!(fs::read(&path).expect("read"), b"zip bytes");
    }

    #[test]
    fn download_saves_the_body_of_an_error_status() {
        // Go's `http.Get` does not check the status, so the error page is saved.
        let temp = tempfile::tempdir().expect("temp dir");
        let (url, server) = serve_once(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found",
        );
        let (path, result) = download_from_url(&url, temp.path());
        server.join().expect("server");
        result.expect("download");
        assert_eq!(fs::read(&path).expect("read"), b"not found");
    }

    #[test]
    fn failed_download_leaves_an_empty_file_and_returns_its_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        // A port nothing listens on, so the connection is refused.
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("local address")
            .port();
        let (path, result) =
            download_from_url(&format!("http://127.0.0.1:{port}/ml-1m.zip"), temp.path());
        assert!(result.is_err());
        assert_eq!(path, temp.path().join("ml-1m.zip"));
        assert_eq!(fs::metadata(&path).expect("created").len(), 0);
    }

    #[test]
    fn locate_uses_the_files_already_in_the_dataset_dir() {
        let dataset = tempfile::tempdir().expect("dataset dir");
        let temp = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dataset.path().join("ml-1m")).expect("create");
        fs::write(dataset.path().join("ml-1m").join("train.txt"), "1 2 3\n").expect("write");

        let (train, test) =
            locate_built_in_dataset_in("ml-1m", DatasetFormat::Ncf, dataset.path(), temp.path())
                .expect("locate");

        assert_eq!(train, dataset.path().join("ml-1m").join("train.txt"));
        assert_eq!(test, dataset.path().join("ml-1m").join("test.txt"));
        // No download was attempted: the temporary directory got no zip.
        assert_eq!(fs::read_dir(temp.path()).expect("read dir").count(), 0);
    }

    #[test]
    fn locate_rejects_an_unknown_name() {
        let dataset = tempfile::tempdir().expect("dataset dir");
        let temp = tempfile::tempdir().expect("temp dir");
        let err =
            locate_built_in_dataset_in("no-such", DatasetFormat::Ncf, dataset.path(), temp.path())
                .expect_err("unknown name");
        assert_eq!(err.to_string(), "no such dataset no-such");
    }

    #[test]
    fn locate_rejects_a_format_mismatch_with_go_s_message() {
        let dataset = tempfile::tempdir().expect("dataset dir");
        let temp = tempfile::tempdir().expect("temp dir");
        // `ml-tag` is libfm; asking for NCF prints the two format numbers.
        let err =
            locate_built_in_dataset_in("ml-tag", DatasetFormat::Ncf, dataset.path(), temp.path())
                .expect_err("format mismatch");
        assert_eq!(err.to_string(), "format not matchs 0 != 1");
    }

    /// Writes a zip archive with one entry per `(name, contents)` pair.
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).expect("create"));
        for (name, contents) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(contents).expect("write");
        }
        writer.finish().expect("finish");
    }

    #[test]
    fn unzip_extracts_nested_files_and_lists_them() {
        let temp = tempfile::tempdir().expect("temp dir");
        let archive = temp.path().join("ml-1m.zip");
        write_zip(&archive, &[("ml-1m/train.txt", b"1 2 3\n")]);
        let dst = temp.path().join("dst");

        let names = unzip(&archive, &dst).expect("unzip");

        assert_eq!(names, vec![dst.join("ml-1m").join("train.txt")]);
        assert_eq!(
            fs::read_to_string(dst.join("ml-1m").join("train.txt")).expect("read"),
            "1 2 3\n"
        );
    }

    #[test]
    fn unzip_rejects_entries_outside_the_destination() {
        let temp = tempfile::tempdir().expect("temp dir");
        let archive = temp.path().join("evil.zip");
        write_zip(&archive, &[("../escaped.txt", b"x")]);
        let dst = temp.path().join("dst");
        fs::create_dir_all(&dst).expect("dst");

        let err = unzip(&archive, &dst).expect_err("zip slip");

        assert!(matches!(err, Error::IllegalPath(_)), "{err}");
        assert!(!temp.path().join("escaped.txt").exists());
    }

    #[test]
    fn unzip_writes_an_absolute_entry_name_under_the_destination() {
        // Go's `filepath.Join(dst, "/abs/x.txt")` is `dst/abs/x.txt`, so the
        // entry is extracted there and the check passes (`PORTING_NOTES.md`).
        let temp = tempfile::tempdir().expect("temp dir");
        let archive = temp.path().join("absolute.zip");
        write_zip(&archive, &[("/abs/x.txt", b"x")]);
        let dst = temp.path().join("dst");

        let names = unzip(&archive, &dst).expect("unzip");

        assert_eq!(names, vec![dst.join("abs").join("x.txt")]);
        assert_eq!(fs::read(dst.join("abs").join("x.txt")).expect("read"), b"x");
    }

    #[test]
    fn create_dirs_makes_both_directories_with_their_parents() {
        let temp = tempfile::tempdir().expect("temp dir");
        let dataset = temp.path().join("a").join("dataset");
        let scratch = temp.path().join("a").join("temp");

        create_dirs(&dataset, &scratch).expect("create");

        assert!(dataset.is_dir());
        assert!(scratch.is_dir());
    }

    #[test]
    fn lexical_clean_matches_go_clean() {
        assert_eq!(lexical_clean(Path::new("a/./b/../c")), PathBuf::from("a/c"));
        assert_eq!(lexical_clean(Path::new("../a")), PathBuf::from("../a"));
        assert_eq!(lexical_clean(Path::new("/../a")), PathBuf::from("/a"));
        assert_eq!(lexical_clean(Path::new("")), PathBuf::from("."));
    }
}
