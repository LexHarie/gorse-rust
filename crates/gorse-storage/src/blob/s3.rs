// Ports: storage/blob/s3.go @ add21b5, storage/blob/s3_test.go @ add21b5

//! An S3 bucket: `blob.uri` = `s3://bucket/prefix`. Go uses `minio-go`; the
//! port uses `object_store`, which speaks the same S3 API.

use std::sync::Arc;

use gorse_config::config::schema::S3Config;
use object_store::aws::AmazonS3Builder;

use super::BlobError;
use super::cloud::CloudStore;

/// Opens `bucket` under `prefix` (Go's `NewS3`).
pub fn new(cfg: &S3Config, bucket: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    // minio.New takes host[:port] and prefixes the scheme itself (plain HTTP
    // unless Secure is set), so a scheme in the endpoint is an error.
    if cfg.endpoint.contains("://") {
        return Err(BlobError::Config(
            "s3 endpoint must be host[:port] without a scheme".to_string(),
        ));
    }
    let store = AmazonS3Builder::new()
        .with_endpoint(format!("http://{}", cfg.endpoint))
        .with_allow_http(true)
        // minio-go addresses a custom endpoint by path, not by bucket subdomain.
        .with_virtual_hosted_style_request(false)
        .with_access_key_id(&cfg.access_key_id)
        .with_secret_access_key(&cfg.secret_access_key)
        .with_bucket_name(bucket)
        .build()?;
    Ok(CloudStore::new(Arc::new(store), prefix, "S3"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::Store;
    use crate::blob::cloud::required_env;
    use gorse_common::gocontext::Context;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_s3() {
        let ctx = Context::background();
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
        let (mut w, done) = client.create(&ctx, "test").await.unwrap();
        w.write(&ctx, b"hello world").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), done)
                .await
                .unwrap()
                .is_err()
        );

        // read the file
        let mut r = client.open(&ctx, "test").await.unwrap();
        let mut content = [0u8; 11];
        r.read_exact(&mut content).await.unwrap();
        assert_eq!(&content, b"hello world");
        // Go's minio reader returns io.EOF with the last bytes; a Rust reader
        // reports the end on the next read.
        assert_eq!(r.read(&mut [0u8; 1]).await.unwrap(), 0);
        drop(r);

        // list files
        let files = client.list(&ctx).await.unwrap();
        assert!(files.contains(&"test".to_string()));

        // remove the file
        client.remove(&ctx, "test").await.unwrap();
        let files = client.list(&ctx).await.unwrap();
        assert!(!files.contains(&"test".to_string()));
    }

    /// Not in Go. Data past one part goes up as a multipart upload, which the
    /// writer starts on its own; this checks that path round-trips intact.
    #[tokio::test]
    async fn test_s3_multipart() {
        let ctx = Context::background();
        let endpoint = required_env("S3_ENDPOINT");
        let access_key_id = required_env("S3_ACCESS_KEY_ID");
        let secret_access_key = required_env("S3_SECRET_ACCESS_KEY");
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

        let data: Vec<u8> = (0..12 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut w, done) = client.create(&ctx, "big").await.unwrap();
        for chunk in data.chunks(1024 * 1024) {
            w.write(&ctx, chunk).await.unwrap();
        }
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(120), done)
                .await
                .unwrap()
                .is_err()
        );

        let mut r = client.open(&ctx, "big").await.unwrap();
        let mut read = Vec::new();
        r.read_to_end(&mut read).await.unwrap();
        drop(r);
        assert_eq!(read.len(), data.len());
        assert!(read == data, "the multipart upload changed the bytes");

        client.remove(&ctx, "big").await.unwrap();
    }
}
