// Ports: storage/blob/s3.go @ add21b5, storage/blob/s3_test.go @ add21b5

//! An S3 bucket: `blob.uri` = `s3://bucket/prefix`. Go uses `minio-go`; the
//! port uses `object_store`, which speaks the same S3 API.

use gorse_config::config::schema::S3Config;

use super::cloud::CloudStore;
use super::BlobError;

/// Opens `bucket` under `prefix` (Go's `NewS3`).
pub fn new(cfg: &S3Config, bucket: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    unimplemented!("RED: s3 new {bucket:?} {prefix:?} via {:?}", cfg.endpoint)
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
    fn test_s3() {
        let endpoint = required_env("S3_ENDPOINT");
        let access_key_id = required_env("S3_ACCESS_KEY_ID");
        let secret_access_key = required_env("S3_SECRET_ACCESS_KEY");

        // create client. The bucket `gorse-test` is created by the emulator's
        // setup before the test runs, because object_store has no CreateBucket.
        let client = new(
            &S3Config {
                endpoint,
                access_key_id,
                secret_access_key,
            },
            "gorse-test",
            "blob",
        )
        .unwrap();

        // write a temp file
        let (mut w, done) = client.create("test").unwrap();
        w.write_all(b"hello world").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(60)),
            Err(RecvTimeoutError::Disconnected)
        ));

        // read the file
        let mut r = client.open("test").unwrap();
        let mut content = [0u8; 11];
        r.read_exact(&mut content).unwrap();
        assert_eq!(&content, b"hello world");
        // Go's minio reader returns io.EOF with the last bytes; a Rust reader
        // reports the end on the next read.
        assert_eq!(r.read(&mut [0u8; 1]).unwrap(), 0);
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
