// Ports: storage/blob/azure.go @ add21b5, storage/blob/azure_test.go @ add21b5

//! Azure Blob Storage: `blob.uri` = `az://container/prefix`.

use std::sync::Arc;

use gorse_config::config::schema::AzureBlobConfig;
use object_store::azure::MicrosoftAzureBuilder;

use super::BlobError;
use super::cloud::CloudStore;

/// Opens `container` under `prefix` (Go's `NewAzureBlob`). A connection string
/// takes precedence over the account name and key.
pub fn new(cfg: &AzureBlobConfig, container: &str, prefix: &str) -> Result<CloudStore, BlobError> {
    let mut builder = MicrosoftAzureBuilder::new().with_container_name(container);
    if !cfg.connection_string.is_empty() {
        let connection = parse_connection_string(&cfg.connection_string)?;
        // object_store only speaks HTTPS unless told otherwise; an emulator's
        // connection string says http://, which the Azure SDK also accepts.
        let allow_http = connection.blob_endpoint.starts_with("http://");
        builder = builder
            .with_account(connection.account_name)
            .with_access_key(connection.access_key)
            .with_endpoint(connection.blob_endpoint)
            .with_allow_http(allow_http);
    } else {
        if cfg.account_name.is_empty() || cfg.account_key.is_empty() {
            return Err(BlobError::Config(
                "azure blob requires account_name and account_key or connection_string".to_string(),
            ));
        }
        builder = builder
            .with_account(&cfg.account_name)
            .with_access_key(&cfg.account_key);
        if !cfg.endpoint.is_empty() {
            builder = builder.with_endpoint(cfg.endpoint.clone());
        }
    }
    Ok(CloudStore::new(
        Arc::new(builder.build()?),
        prefix,
        "Azure Blob",
    ))
}

/// The settings of an Azure storage connection string that the blob client
/// needs.
struct Connection {
    account_name: String,
    access_key: String,
    blob_endpoint: String,
}

/// Reads `key=value;` pairs. Without `BlobEndpoint`, the endpoint is
/// `protocol://account.blob.suffix`, as in the Azure SDK.
fn parse_connection_string(connection_string: &str) -> Result<Connection, BlobError> {
    let (mut account_name, mut account_key, mut blob_endpoint) = (None, None, None);
    let mut protocol = "https".to_string();
    let mut suffix = "core.windows.net".to_string();
    for pair in connection_string.split(';').filter(|pair| !pair.is_empty()) {
        // Keys are case-insensitive; account keys are base64 and may end in '='.
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "accountname" => account_name = Some(value.to_string()),
            "accountkey" => account_key = Some(value.to_string()),
            "blobendpoint" => blob_endpoint = Some(value.trim_end_matches('/').to_string()),
            "defaultendpointsprotocol" => protocol = value.to_string(),
            "endpointsuffix" => suffix = value.to_string(),
            _ => {}
        }
    }
    let account_name = account_name.ok_or_else(|| {
        BlobError::Config("azure connection string has no AccountName".to_string())
    })?;
    let access_key = account_key.ok_or_else(|| {
        BlobError::Config("azure connection string has no AccountKey".to_string())
    })?;
    let blob_endpoint =
        blob_endpoint.unwrap_or_else(|| format!("{protocol}://{account_name}.blob.{suffix}"));
    Ok(Connection {
        account_name,
        access_key,
        blob_endpoint,
    })
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
    async fn test_azure_blob_emulator() {
        let ctx = Context::background();
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

        let (mut w, done) = client.create(&ctx, "test.txt").await.unwrap();
        w.write(&ctx, b"hello").await.unwrap();
        w.close(&ctx).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(60), done)
                .await
                .unwrap()
                .is_err()
        );

        let mut r = client.open(&ctx, "test.txt").await.unwrap();
        let mut data = Vec::new();
        r.read_to_end(&mut data).await.unwrap();
        assert_eq!(data, b"hello");
        drop(r);

        let names = client.list(&ctx).await.unwrap();
        assert!(names.contains(&"test.txt".to_string()));

        client.remove(&ctx, "test.txt").await.unwrap();
        let names = client.list(&ctx).await.unwrap();
        assert!(!names.contains(&"test.txt".to_string()));
    }
}
