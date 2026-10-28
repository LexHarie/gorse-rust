// Ports: storage/blob/gcs.go @ add21b5, storage/blob/gcs_test.go @ add21b5

//! A Google Cloud Storage bucket: `blob.uri` = `gs://bucket/prefix`.

use gorse_config::config::schema::GCSConfig;

use super::cloud::CloudStore;
use super::BlobError;

/// Opens `bucket` under `prefix` (Go's `NewGCS`). The environment variable
/// `GCS_EMULATOR_ENDPOINT`, when set, points the store at an emulator without
/// authentication, as in Go.
pub fn new(cfg: &GCSConfig, bucket: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    unimplemented!("RED: gcs new {bucket:?} {prefix:?} creds {:?}", cfg.credentials_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::cloud::required_env;
    use crate::blob::Store;
    use std::io::{Read, Write};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    #[test]
    fn test_gcs() {
        // Go starts an in-process fake-gcs-server here; the Rust test needs an
        // emulator already running at GCS_EMULATOR_ENDPOINT, with the bucket
        // `gorse-test` created before the test.
        required_env("GCS_EMULATOR_ENDPOINT");

        // create client
        let client = new(&GCSConfig::default(), "gorse-test", "blob").unwrap();

        // create file
        let (mut w, done) = client.create("test.txt").unwrap();
        w.write_all(b"hello").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(60)),
            Err(RecvTimeoutError::Disconnected)
        ));

        // list files
        let names = client.list().unwrap();
        assert_eq!(names, vec!["test.txt".to_string()]);

        // read file
        let mut r = client.open("test.txt").unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        // remove file
        client.remove("test.txt").unwrap();

        // list files again
        let names = client.list().unwrap();
        assert!(names.is_empty());
    }
}
