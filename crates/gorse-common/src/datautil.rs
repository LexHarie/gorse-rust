// Ports: common/datautil/datautil.go @ add21b5

//! Datasets that the Gorse tests and tools load: the iris dataset, downloaded
//! from `cdn.gorse.io` once and unpacked under `~/.gorse/dataset`.
//!
//! Go ignores the error of the download and reports the unzip's error instead,
//! which says the zip is missing. That is kept here.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::util::parse_float;

/// The errors of [`load_iris`] and the download helpers.
#[derive(Debug, Error)]
pub enum Error {
    /// A file could not be read, written or created.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The CSV file is malformed.
    #[error(transparent)]
    Csv(#[from] csv::Error),
    /// A zip archive could not be read.
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    /// The download failed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// A number in the data does not parse.
    #[error("{0}")]
    Number(#[from] crate::gostrconv::NumError),
    /// A row has fewer than five fields, which Go's `row[:4]` and `row[4]` panic on.
    #[error("row {row} has {fields} fields, want at least 5")]
    ShortRow {
        /// Zero-based index of the row in the file.
        row: usize,
        /// Number of fields the row has.
        fields: usize,
    },
    /// An archive entry would be written outside the destination directory.
    #[error("{0}: illegal file path")]
    IllegalPath(PathBuf),
}

/// Go's `datautil.LoadIris`: the 150 iris flowers' four measurements, and each
/// flower's class index (in order of first appearance).
pub fn load_iris() -> Result<(Vec<Vec<f32>>, Vec<i64>), Error> {
    let path = download_and_unzip("iris")?;
    load_iris_file(&path.join("iris.data"))
}

/// Reads the iris CSV at `path`, as [`load_iris`] does after the download.
pub fn load_iris_file(path: &Path) -> Result<(Vec<Vec<f32>>, Vec<i64>), Error> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(File::open(path)?);
    let mut data = Vec::new();
    let mut target = Vec::new();
    let mut types: HashMap<String, i64> = HashMap::new();
    for (index, row) in reader.records().enumerate() {
        let row = row?;
        if row.len() < 5 {
            return Err(Error::ShortRow {
                row: index,
                fields: row.len(),
            });
        }
        let mut values = Vec::with_capacity(4);
        for cell in row.iter().take(4) {
            values.push(parse_float::<f32>(cell)?);
        }
        data.push(values);
        let next = types.len() as i64;
        target.push(*types.entry(row[4].to_owned()).or_insert(next));
    }
    Ok((data, target))
}

/// Go's `datautil.DownloadAndUnzip`: the directory `name` under the dataset
/// directory, downloading and unpacking `cdn.gorse.io/datasets/<name>.zip` if
/// that directory is missing.
pub fn download_and_unzip(name: &str) -> Result<PathBuf, Error> {
    let url = format!("https://cdn.gorse.io/datasets/{name}.zip");
    let path = dataset_dir().join(name);
    // As Go's `os.IsNotExist`: only a missing path downloads, not any stat error.
    if fs::metadata(&path).is_err_and(|err| err.kind() == io::ErrorKind::NotFound) {
        // Go ignores this error: a failed download leaves no zip, and the unzip reports it.
        let (zip_file_name, _) = download_from_url(&url, &temp_dir());
        unzip(&zip_file_name, &dataset_dir())?;
    }
    Ok(path)
}

/// `~/.gorse/dataset`, Go's `datasetDir`.
fn dataset_dir() -> PathBuf {
    home_dir().join(".gorse").join("dataset")
}

/// `~/.gorse/temp`, Go's `tempDir`.
fn temp_dir() -> PathBuf {
    home_dir().join(".gorse").join("temp")
}

/// The home directory. Go asks the password database (`user.Current`) and
/// stops the process when that fails; this reads `$HOME` and gives an empty
/// path when it is unset (`porting/notes/WP-1-8.md`, X-WP8-4).
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Go's `downloadFromUrl`: saves `src` under `dst`, named by the last segment
/// of its URL. The file name comes back even when the download fails.
fn download_from_url(src: &str, dst: &Path) -> (PathBuf, Result<(), Error>) {
    tracing::info!(source = src, destination = %dst.display(), "download dataset");
    let file_name = dst.join(src.rsplit('/').next().unwrap_or_default());
    let result = (|| -> Result<(), Error> {
        if let Some(dir) = file_name.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut output = File::create(&file_name)?;
        // As Go's http.Get: the body is saved whatever the status code.
        let mut response = reqwest::blocking::get(src)?;
        io::copy(&mut response, &mut output)?;
        Ok(())
    })();
    (file_name, result)
}

/// Go's `unzip`: extracts `src` under `dst` and returns the paths written.
fn unzip(src: &Path, dst: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut archive = zip::ZipArchive::new(File::open(src)?)?;
    let clean_dst = lexical_clean(dst);
    let mut file_names = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // Check for ZipSlip: the entry must stay strictly inside the destination.
        let name = entry.name()?;
        let file_path = lexical_clean(&dst.join(&*name));
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

/// Joins and cleans a path lexically, as Go's `filepath.Join` and `Clean` do:
/// `.` components go and `..` removes the component before it.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other),
        }
    }
    cleaned
}

#[cfg(test)]
mod tests {
    // Ports: common/datautil/datautil_test.go @ add21b5

    use super::*;

    /// Go's `TestLoadIris`. It downloads from `cdn.gorse.io`, which this
    /// container cannot reach, so it is ignored here and run where the CDN is reachable.
    #[test]
    #[ignore = "needs cdn.gorse.io, which this container cannot reach"]
    fn load_iris_downloads_the_dataset() {
        let (data, target) = load_iris().expect("iris");
        assert_eq!(data.len(), 150);
        assert_eq!(data[0].len(), 4);
        assert_eq!(target.len(), 150);
    }

    #[test]
    fn load_iris_file_parses_rows_and_class_indexes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("iris.data");
        fs::write(
            &path,
            "5.1,3.5,1.4,0.2,Iris-setosa\n7.0,3.2,4.7,1.4,Iris-versicolor\n\n6.3,3.3,6.0,2.5,Iris-virginica\n5.0,3.6,1.4,0.2,Iris-setosa\n",
        )
        .expect("write fixture");
        let (data, target) = load_iris_file(&path).expect("parse");
        assert_eq!(data.len(), 4);
        assert_eq!(data[1], vec![7.0, 3.2, 4.7, 1.4]);
        assert_eq!(target, vec![0, 1, 2, 0]);
    }

    #[test]
    fn unzip_rejects_entries_outside_the_destination() {
        use std::io::Write;
        let temp = tempfile::tempdir().expect("temp dir");
        let archive_path = temp.path().join("evil.zip");
        {
            let mut writer = zip::ZipWriter::new(File::create(&archive_path).expect("create"));
            writer
                .start_file("../escaped.txt", zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(b"x").expect("write");
            writer.finish().expect("finish");
        }
        let dst = temp.path().join("dst");
        fs::create_dir_all(&dst).expect("dst");
        let err = unzip(&archive_path, &dst).expect_err("zip slip");
        assert!(matches!(err, Error::IllegalPath(_)), "{err}");
        assert!(!temp.path().join("escaped.txt").exists());
    }

    #[test]
    fn unzip_extracts_nested_files() {
        use std::io::Write;
        let temp = tempfile::tempdir().expect("temp dir");
        let archive_path = temp.path().join("iris.zip");
        {
            let mut writer = zip::ZipWriter::new(File::create(&archive_path).expect("create"));
            writer
                .start_file("iris/iris.data", zip::write::SimpleFileOptions::default())
                .expect("start");
            writer
                .write_all(b"5.1,3.5,1.4,0.2,Iris-setosa\n")
                .expect("write");
            writer.finish().expect("finish");
        }
        let dst = temp.path().join("dst");
        let names = unzip(&archive_path, &dst).expect("unzip");
        assert_eq!(names, vec![dst.join("iris").join("iris.data")]);
        assert_eq!(
            fs::read_to_string(dst.join("iris").join("iris.data")).expect("read"),
            "5.1,3.5,1.4,0.2,Iris-setosa\n"
        );
    }
}
