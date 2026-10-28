// Ports: storage/blob/posix.go @ add21b5, storage/blob/posix_test.go @ add21b5

//! A blob store in a local directory: `blob.uri` without `://`.

use std::path::PathBuf;

use super::{BlobError, BlobWriter, Store};
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;

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
        unimplemented!("RED: posix open {name:?} in {:?}", self.dir)
    }

    fn create(&self, name: &str) -> Result<(Box<dyn BlobWriter>, Receiver<()>), BlobError> {
        unimplemented!("RED: posix create {name:?} in {:?}", self.dir)
    }

    fn list(&self) -> Result<Vec<String>, BlobError> {
        unimplemented!("RED: posix list in {:?}", self.dir)
    }

    fn remove(&self, name: &str) -> Result<(), BlobError> {
        unimplemented!("RED: posix remove {name:?} in {:?}", self.dir)
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
