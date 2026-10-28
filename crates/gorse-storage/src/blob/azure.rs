// Ports: storage/blob/azure.go @ add21b5, storage/blob/azure_test.go @ add21b5

//! Azure Blob Storage: `blob.uri` = `az://container/prefix`.

use gorse_config::config::schema::AzureBlobConfig;

use super::cloud::CloudStore;
use super::BlobError;

/// Opens `container` under `prefix` (Go's `NewAzureBlob`). A connection string
/// takes precedence over the account name and key.
pub fn new(cfg: &AzureBlobConfig, container: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    unimplemented!("RED: azure new {container:?} {prefix:?} conn {:?}", cfg.connection_string.len())
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
    fn test_azure_blob_emulator() {
        let connection_string = required_env("AZURE_STORAGE_CONNECTION_STRING");

        // create client. The container `gorse-test` is created by the emulator's
        // setup before the test runs (Go's CreateContainer step).
        let client = new(
            &AzureBlobConfig {
                connection_string,
                ..Default::default()
            },
            "gorse-test",
            "blob",
        )
        .unwrap();

        let (mut w, done) = client.create("test.txt").unwrap();
        w.write_all(b"hello").unwrap();
        w.close().unwrap();
        assert!(matches!(
            done.recv_timeout(Duration::from_secs(60)),
            Err(RecvTimeoutError::Disconnected)
        ));

        let mut r = client.open("test.txt").unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        let names = client.list().unwrap();
        assert!(names.contains(&"test.txt".to_string()));

        client.remove("test.txt").unwrap();
        let names = client.list().unwrap();
        assert!(!names.contains(&"test.txt".to_string()));
    }
}
