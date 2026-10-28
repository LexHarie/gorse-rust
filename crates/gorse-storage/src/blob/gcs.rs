// Ports: storage/blob/gcs.go @ add21b5, storage/blob/gcs_test.go @ add21b5

//! A Google Cloud Storage bucket: `blob.uri` = `gs://bucket/prefix`.

use std::sync::Arc;

use gorse_config::config::schema::GCSConfig;
use object_store::StaticCredentialProvider;
use object_store::gcp::{GcpCredential, GoogleCloudStorageBuilder};

use super::BlobError;
use super::cloud::CloudStore;

/// Opens `bucket` under `prefix` (Go's `NewGCS`). The environment variable
/// `GCS_EMULATOR_ENDPOINT`, when set, points the store at an emulator without
/// authentication, as in Go.
pub fn new(cfg: &GCSConfig, bucket: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
    if let Some(endpoint) = std::env::var("GCS_EMULATOR_ENDPOINT")
        .ok()
        .filter(|endpoint| !endpoint.is_empty())
    {
        // Go's SDK takes the JSON API root, such as "http://host:5050/storage/v1/";
        // object_store wants the server root.
        let base_url = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/storage/v1");
        // The emulator takes no credentials. A static empty token stops
        // object_store from asking the metadata server for one.
        let credential = GcpCredential {
            bearer: String::new(),
        };
        builder = builder
            .with_base_url(base_url)
            .with_skip_signature(true)
            .with_credentials(Arc::new(StaticCredentialProvider::new(credential)));
    }
    if !cfg.credentials_file.is_empty() {
        builder = builder.with_service_account_path(&cfg.credentials_file);
    }
    Ok(CloudStore::new(Arc::new(builder.build()?), prefix, "GCS"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::Store;
    use crate::blob::cloud::required_env;
    use gorse_common::gocontext::Context;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    /// Go writes through the emulator here. object_store writes through the GCS
    /// XML API, which fake-gcs-server does not serve (v1.54.0 and v1.56.1 both
    /// answer 400 "invalid uploadType"), so the write cannot run in this
    /// container. See `porting/notes/WP-2-10.md` (O-WP210-1).
    #[ignore = "fake-gcs-server has no XML upload API that object_store uses (O-WP210-1)"]
    #[tokio::test]
    async fn test_gcs() {
        let ctx = Context::background();
        // Go starts an in-process fake-gcs-server here; the Rust test needs an
        // emulator already running at GCS_EMULATOR_ENDPOINT, with the bucket
        // `gorse-test` created before the test.
        required_env("GCS_EMULATOR_ENDPOINT");

        // create client
        let client = new(&GCSConfig::default(), "gorse-test", "blob").unwrap();

        // create file
        let (mut w, done) = client.create(&ctx, "test.txt").await.unwrap();
        w.write(&ctx, b"hello").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), done)
                .await
                .unwrap()
                .is_err()
        );

        // list files
        let names = client.list(&ctx).await.unwrap();
        assert_eq!(names, vec!["test.txt".to_string()]);

        // read file
        let mut r = client.open(&ctx, "test.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        // remove file
        client.remove(&ctx, "test.txt").await.unwrap();

        // list files again
        let names = client.list(&ctx).await.unwrap();
        assert!(names.is_empty());
    }

    /// Not in Go. The object is seeded through the emulator's JSON upload API,
    /// which fake-gcs-server does serve, and then listed, read and removed
    /// through object_store. This covers the GCS read and delete paths that
    /// `test_gcs` cannot reach here.
    #[tokio::test]
    async fn test_gcs_read_list_remove() {
        let ctx = Context::background();
        let endpoint = required_env("GCS_EMULATOR_ENDPOINT");
        let base_url = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/storage/v1");
        reqwest::Client::new()
            .post(format!(
                "{base_url}/upload/storage/v1/b/gorse-test/o?uploadType=media&name=blob%2Fseeded.txt"
            ))
            .body("hello")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();

        let client = new(&GCSConfig::default(), "gorse-test", "blob").unwrap();

        let names = client.list(&ctx).await.unwrap();
        assert!(names.contains(&"seeded.txt".to_string()));

        let mut r = client.open(&ctx, "seeded.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        client.remove(&ctx, "seeded.txt").await.unwrap();
        let names = client.list(&ctx).await.unwrap();
        assert!(!names.contains(&"seeded.txt".to_string()));
    }
}
