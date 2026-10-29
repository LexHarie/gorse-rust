// Ports: server/rest_test.go (TestHealth) @ add21b5

//! The health routes, as Go's `TestHealth`: live and ready answer with the
//! same status, and the ready route answers 503 when a store is missing.

use std::sync::Arc;

use axum::http::StatusCode;
use gorse_storage::cache;
use gorse_storage::data;
use serde_json::json;

use super::{Suite, get};

#[tokio::test]
async fn health() {
    let suite = Suite::default_suite().await;

    // ready
    suite
        .send(get("/api/health/live"))
        .await
        .status(StatusCode::OK)
        .json(json!({
            "Ready": true,
            "DataStoreError": null,
            "CacheStoreError": null,
            "DataStoreConnected": true,
            "CacheStoreConnected": true,
        }));
    suite
        .send(get("/api/health/ready"))
        .await
        .status(StatusCode::OK)
        .json(json!({
            "Ready": true,
            "DataStoreError": null,
            "CacheStoreError": null,
            "DataStoreConnected": true,
            "CacheStoreConnected": true,
        }));

    // not ready
    let data = suite.data.clone();
    let cache_store = suite.cache.clone();
    suite.rest.set_data(Arc::new(data::no_database::NoDatabase));
    suite.rest.set_cache(Arc::new(cache::no_database::NoDatabase));
    suite
        .send(get("/api/health/live"))
        .await
        .status(StatusCode::OK)
        .json(json!({
            "Ready": false,
            "DataStoreError": {},
            "CacheStoreError": {},
            "DataStoreConnected": false,
            "CacheStoreConnected": false,
        }));
    suite
        .send(get("/api/health/ready"))
        .await
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .json(json!({
            "Ready": false,
            "DataStoreError": {},
            "CacheStoreError": {},
            "DataStoreConnected": false,
            "CacheStoreConnected": false,
        }));
    suite.rest.set_data(data);
    suite.rest.set_cache(cache_store);
}
