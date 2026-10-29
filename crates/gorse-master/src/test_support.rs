// Ports: master/master_test.go @ add21b5, and the helpers of master/rpc_test.go and master/tasks_test.go

//! The fixtures the tests of this crate share. Go's `MasterTestSuite` opens a
//! sqlite data store, a sqlite cache store and an xvec vector store for each
//! test, and the RPC tests open a sqlite meta store. These do the same, in a
//! directory the test owns.

use std::path::Path;
use std::sync::Arc;

use gorse_common::gotime::SECOND;
use gorse_config::config::schema::Config;
use gorse_storage::cache;
use gorse_storage::data;
use gorse_storage::meta;
use gorse_storage::vectors;

use crate::master::Master;

/// A master over `cfg` whose meta database is open in `dir`. Its data, cache
/// and vector stores are the empty ones [`Master::new`] sets, as in Go's
/// `mockMasterRPC`.
pub(crate) async fn new_master(dir: &Path, cfg: Config, config_path: &str) -> Arc<Master> {
    let master = Master::new(cfg, &dir.display().to_string(), false, config_path);
    let meta_store = meta::open(&format!("sqlite://{}/meta.db", dir.display()), SECOND)
        .await
        .expect("open the meta database");
    meta_store.init().await.expect("init the meta database");
    *master
        .meta
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::from(meta_store));
    master
}

/// Opens the sqlite data store, the sqlite cache store and the xvec vector store
/// in `dir`, initialises them, and sets them on `master`. Go's `SetupTest`.
pub(crate) async fn open_stores(master: &Master, dir: &Path) {
    let cfg = master.config();
    let data_url = format!("sqlite://{}/data.db", dir.display());
    let mut data_registry = data::Registry::default();
    data::register_builtin(&mut data_registry);
    let data_client: Arc<dyn data::Database> = Arc::from(
        data_registry
            .open(&data_url, "", &cfg.database.storage_options(&data_url))
            .await
            .expect("open the data store"),
    );
    data_client.init().await.expect("init the data store");

    let cache_url = format!("sqlite://{}/cache.db", dir.display());
    let mut cache_registry = cache::Registry::default();
    cache::register_builtin(&mut cache_registry);
    let cache_client: Arc<dyn cache::Database> = Arc::from(
        cache_registry
            .open(&cache_url, "", &cfg.database.storage_options(&cache_url))
            .await
            .expect("open the cache store"),
    );
    cache_client.init().await.expect("init the cache store");

    let vector_url = format!("xvec://{}/vectors", dir.display());
    let vector_client: Arc<dyn vectors::Database> = Arc::from(
        vectors::open(&vector_url, "", &[])
            .await
            .expect("open the vector store"),
    );
    vector_client.init().await.expect("init the vector store");

    master.set_stores(data_client, cache_client, vector_client);
}

/// Closes the stores [`open_stores`] set, as Go's `TearDownTest` does.
pub(crate) async fn close_stores(master: &Master) {
    master.data().close().await.expect("close the data store");
    master.cache().close().await.expect("close the cache store");
    master
        .vectors()
        .close()
        .await
        .expect("close the vector store");
}
