// Ports: worker/pipeline_test.go @ add21b5

//! The pipeline suite of `worker/pipeline_test.go`, one test per Go suite
//! method, and the top-level `TestCompressLabelsEmbeddings`. Two more tests
//! pin the label compression that the item cache applies and the view the
//! rerankers render.

use std::collections::BTreeMap;

use gorse_common::floats::from_float32;
use gorse_common::gocontext::Context;
use gorse_model::ctr::data::Value;
use gorse_storage::data::types::{Labels, User};

use super::{CachedItem, ItemCache, compress_labels_embeddings};
use crate::testing::{fixture, item};

/// The ids `"1"` to `"5"` the suite inserts, plus `"6"`, which is missing.
fn ids(range: std::ops::RangeInclusive<u32>) -> Vec<String> {
    range.map(|i| i.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn get_slice() {
    let fx = fixture().await;
    let ctx = Context::background();
    fx.data
        .batch_insert_items(
            &ctx,
            &ids(1..=5).iter().map(|id| item(id)).collect::<Vec<_>>(),
        )
        .await
        .expect("insert the items");
    let cache = ItemCache::new(fx.data.clone());
    let items = cache
        .get_slice(&ctx, &ids(1..=6))
        .await
        .expect("get the items");
    assert_eq!(items.len(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_map() {
    let fx = fixture().await;
    let ctx = Context::background();
    fx.data
        .batch_insert_items(
            &ctx,
            &ids(1..=5).iter().map(|id| item(id)).collect::<Vec<_>>(),
        )
        .await
        .expect("insert the items");
    let cache = ItemCache::new(fx.data.clone());
    let items = cache
        .get_map(&ctx, &ids(1..=6))
        .await
        .expect("get the items");
    assert_eq!(items.len(), 5);
}

#[test]
fn compress_labels_embeddings_cases() {
    // Test nil input
    assert_eq!(compress_labels_embeddings(&Value::Nil), Value::Nil);

    // Test direct string input
    let input = Value::String("tag".to_owned());
    assert_eq!(compress_labels_embeddings(&input), input);

    // Test map input preserves keys and values after compression/alignment
    let input = Value::Map(BTreeMap::from([
        ("category".to_owned(), Value::String("books".to_owned())),
        ("score".to_owned(), Value::Float64(1.0)),
    ]));
    let Value::Map(output) = compress_labels_embeddings(&input) else {
        panic!("a map stays a map");
    };
    assert_eq!(
        output.get("category"),
        Some(&Value::String("books".to_owned()))
    );
    assert_eq!(output.get("score"), Some(&Value::Float64(1.0)));

    // Test embedding vector as []any
    let input = Value::Slice(vec![
        Value::Float64(1.0),
        Value::Float64(2.0),
        Value::Float64(3.0),
    ]);
    assert_eq!(
        compress_labels_embeddings(&input),
        Value::Uint16s(from_float32(&[1.0, 2.0, 3.0]))
    );

    // Test embedding vector as []float32
    let input = Value::Float32s(vec![1.0, 2.0, 3.0]);
    assert_eq!(
        compress_labels_embeddings(&input),
        Value::Uint16s(from_float32(&[1.0, 2.0, 3.0]))
    );

    // Test embedding vector as []float64
    let input = Value::Float64s(vec![1.0, 2.0, 3.0]);
    assert_eq!(
        compress_labels_embeddings(&input),
        Value::Uint16s(from_float32(&[1.0, 2.0, 3.0]))
    );

    // Test already compressed []uint16
    let input = Value::Uint16s(from_float32(&[1.0, 2.0, 3.0]));
    assert_eq!(compress_labels_embeddings(&input), input);

    // Test map with embedding
    let input = Value::Map(BTreeMap::from([
        ("title".to_owned(), Value::String("test item".to_owned())),
        (
            "embedding".to_owned(),
            Value::Slice(vec![
                Value::Float64(1.0),
                Value::Float64(2.0),
                Value::Float64(3.0),
            ]),
        ),
    ]));
    let Value::Map(output) = compress_labels_embeddings(&input) else {
        panic!("a map stays a map");
    };
    assert_eq!(
        output.get("embedding"),
        Some(&Value::Uint16s(from_float32(&[1.0, 2.0, 3.0])))
    );
    assert_eq!(
        output.get("title"),
        Some(&Value::String("test item".to_owned()))
    );

    // Test nested map
    let input = Value::Map(BTreeMap::from([(
        "features".to_owned(),
        Value::Map(BTreeMap::from([(
            "embedding".to_owned(),
            Value::Float32s(vec![1.0, 2.0]),
        )])),
    )]));
    let Value::Map(output) = compress_labels_embeddings(&input) else {
        panic!("a map stays a map");
    };
    let Some(Value::Map(features)) = output.get("features") else {
        panic!("the nested map stays a map");
    };
    assert!(matches!(features.get("embedding"), Some(Value::Uint16s(_))));

    // Test non-embedding []any (strings)
    let input = Value::Slice(vec![
        Value::String("tag1".to_owned()),
        Value::String("tag2".to_owned()),
    ]);
    assert_eq!(compress_labels_embeddings(&input), input);

    // Go's FromAny calls an empty []any an embedding of no values, so it
    // compresses to an empty []uint16 (PORTING_NOTES.md Q-WP41-1).
    assert_eq!(
        compress_labels_embeddings(&Value::Slice(Vec::new())),
        Value::Uint16s(Vec::new())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn item_cache_stores_compressed_labels() {
    let fx = fixture().await;
    let ctx = Context::background();
    let mut numeric = item("1");
    numeric.labels = Labels::Array(vec![
        Labels::Float64(1.0),
        Labels::Float64(2.0),
        Labels::Float64(3.0),
    ]);
    fx.data
        .batch_insert_items(&ctx, &[numeric])
        .await
        .expect("insert the item");
    let cache = ItemCache::new(fx.data.clone());
    let items = cache
        .get_slice(&ctx, &["1".to_owned()])
        .await
        .expect("get the item");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].labels,
        Value::Uint16s(from_float32(&[1.0, 2.0, 3.0]))
    );
}

#[test]
fn rerankers_see_fp16_bits_of_embeddings() {
    let cached = CachedItem::from_item(gorse_storage::data::types::Item {
        item_id: "1".to_owned(),
        labels: Labels::Array(vec![Labels::Float64(1.0), Labels::Float64(2.0)]),
        ..Default::default()
    });
    let view = cached.to_item();
    assert_eq!(
        view.labels,
        Labels::Array(vec![
            Labels::Float64(f64::from(from_float32(&[1.0])[0])),
            Labels::Float64(f64::from(from_float32(&[2.0])[0])),
        ])
    );
    // The user is not compressed, as Go's user labels are not.
    let user = User {
        user_id: "u".to_owned(),
        labels: Labels::Array(vec![Labels::Float64(1.0)]),
        ..Default::default()
    };
    assert_eq!(
        crate::pipeline::plain_labels(&user.labels),
        Value::Slice(vec![Value::Float64(1.0)])
    );
}
