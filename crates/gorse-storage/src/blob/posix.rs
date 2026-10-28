// Ports: storage/blob/posix.go @ add21b5, storage/blob/posix_test.go @ add21b5

//! A blob store in a local directory: `blob.uri` without `://`.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use super::{BlobError, BlobWriter, Store, check_name};

/// A blob store that keeps each blob as a file in one directory (Go's
/// `blob.POSIX`).
#[derive(Debug, Clone)]
pub struct Posix {
    dir: PathBuf,
}

impl Posix {
    /// Creates a store over `dir`. The directory is created by [`Store::create`]
    /// when needed.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl Store for Posix {
    fn open(&self, name: &str) -> Result<Box<dyn Read + Send>, BlobError> {
        check_name(name)?;
        Ok(Box::new(File::open(self.dir.join(name))?))
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        check_name(name)?;
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(&path)?;
        let (done, closed) = mpsc::channel();
        Ok((Box::new(PosixWriter { file, _done: done }), closed))
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        // Go's os.ReadDir returns entries sorted by name; read_dir does not.
        names.sort();
        Ok(names)
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        check_name(name)?;
        let path = self.dir.join(name);
        if let Err(err) = fs::remove_file(&path) {
            tracing::error!(file = %path.display(), error = %err, "failed to remove file");
            return Err(err.into());
        }
        Ok(())
    }
}

/// Writes straight into the file. Go copies through an `io.Pipe` on a
/// goroutine; the file is complete when the writer is dropped, which
/// disconnects the `done` channel.
struct PosixWriter {
    file: File,
    _done: Sender<()>,
}

impl Write for PosixWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl BlobWriter for PosixWriter {
    fn close(self: Box<Self>) -> Result<(), BlobError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    #[test]
    fn test_posix() {
        let root = tempfile::tempdir().unwrap();
        // create client
        let client = Posix::new(root.path().join("blob"));

        // write a temp file
        let (mut w, done) = client.create("test").unwrap();
        w.write_all(b"hello world").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        ));

        // read the file
        let mut r = client.open("test").unwrap();
        let mut content = [0u8; 11];
        r.read_exact(&mut content).unwrap();
        assert_eq!(&content, b"hello world");
        drop(r);

        // list files
        let files = client.list().unwrap();
        assert!(files.contains(&"test".to_string()));

        // remove the file
        client.remove("test").unwrap();
        let files = client.list().unwrap();
        assert!(!files.contains(&"test".to_string()));
    }
}
