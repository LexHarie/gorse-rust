// Ports: storage/meta/database_test.go @ add21b5

//! The suite every meta backend runs: `TestNodes` and `TestKeyValues`, which
//! Go's testify suite runs once per backend, and `TestModel_Equal`.
//!
//! Go's testify suite becomes plain functions. The backend's test module does
//! the setup and teardown that `SetupTest` and `TearDownTest` do, then calls
//! the function.

use gorse_common::gotime::{self, Duration, HOUR, Time};
use gorse_model_core::params::{ParamValue, Params};

use super::{Database, Model, Node};

/// Builds a [`Params`] from its entries.
pub(super) fn params(entries: &[(&str, ParamValue)]) -> Params {
    Params(
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect(),
    )
}

/// Builds a node with the given UUID, version and update time.
fn node(uuid: &str, version: &str, update_time: Time) -> Node {
    Node {
        uuid: uuid.to_owned(),
        hostname: "localhost".to_owned(),
        r#type: "master".to_owned(),
        version: version.to_owned(),
        update_time,
    }
}

/// Go's `TestNodes`.
pub async fn test_nodes(db: &dyn Database) {
    // Add node
    db.update_node(&node("node-1", "v0.1.0", gotime::now()))
        .await
        .unwrap();
    // Add duplicate node
    db.update_node(&node("node-1", "v0.1.1", gotime::now()))
        .await
        .unwrap();
    // Add outdated node
    let outdated = gotime::now().add(Duration(-HOUR.0));
    db.update_node(&node("node-2", "v0.1.0", outdated))
        .await
        .unwrap();
    // List nodes
    let nodes = db.list_nodes().await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].uuid, "node-1");
    assert_eq!(nodes[0].hostname, "localhost");
    assert_eq!(nodes[0].r#type, "master");
    assert_eq!(nodes[0].version, "v0.1.1");
}

/// Go's `TestKeyValues`.
pub async fn test_key_values(db: &dyn Database) {
    db.put("key1", "value1").await.unwrap();
    db.put("key2", "value2").await.unwrap();
    db.put("key3", "value3").await.unwrap();

    assert_eq!(db.get("key1").await.unwrap().as_deref(), Some("value1"));
    assert_eq!(db.get("key2").await.unwrap().as_deref(), Some("value2"));
    assert_eq!(db.get("key3").await.unwrap().as_deref(), Some("value3"));

    // Test overwrite
    db.put("key1", "new_value1").await.unwrap();
    assert_eq!(db.get("key1").await.unwrap().as_deref(), Some("new_value1"));

    // Test non-existing key
    assert_eq!(db.get("non-existing-key").await.unwrap(), None);

    // Test delete existing key
    db.delete("key2").await.unwrap();
    assert_eq!(db.get("key2").await.unwrap(), None);

    // Test delete non-existing key
    db.delete("non-existing-key").await.unwrap();
}

/// Go's `TestModel_Equal`.
#[test]
fn model_equal() {
    let mut a = Model {
        id: 1,
        r#type: "test".to_owned(),
        params: params(&[
            ("param1", ParamValue::Int(1)),
            ("param2", ParamValue::String("value2".to_owned())),
        ]),
        score: 0_i64,
    };
    let b = Model {
        id: 2,
        r#type: "test".to_owned(),
        params: params(&[
            ("param1", ParamValue::Int(1)),
            ("param2", ParamValue::String("value2".to_owned())),
        ]),
        score: 1_i64,
    };
    assert!(a.equal(&b));

    a.r#type = "different".to_owned();
    assert!(!a.equal(&b));
    a.r#type = "test".to_owned();

    a.params.0.insert(
        "param2".to_owned(),
        ParamValue::String("different".to_owned()),
    );
    assert!(!a.equal(&b));
    a.params
        .0
        .insert("param2".to_owned(), ParamValue::String("value2".to_owned()));
}
