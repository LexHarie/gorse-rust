// Ports: storage/vectors/database_test.go @ add21b5

//! The shared test suite of the vector stores. Every store runs these cases
//! through its [`Database`] (Go's `vectorsTestSuite`); the cases only use the
//! trait, so a backend that passes them behaves the same as the others.

use gorse_common::floats;
use gorse_common::gocontext::Context;
use gorse_common::gotime::{self, Duration, HOUR, MILLISECOND, SECOND};
use gorse_storage_core::errors::Error as StorageError;

use super::{Database, Distance, QuantizationType, ScoredVector, Vector, VectorConfig, purge};

/// Go's `defaultVectorSize`.
const DEFAULT_VECTOR_SIZE: i64 = 4;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn ids(results: &[ScoredVector]) -> Vec<&str> {
    results
        .iter()
        .map(|result| result.vector.id.as_str())
        .collect()
}

/// Go's `SetupTest`: every case starts from an empty database.
fn setup(ctx: &Context, db: &dyn Database) {
    purge(ctx, db).expect("purge");
}

/// The dense vector `values` with the id and categories given.
fn dense(id: &str, values: Vec<f32>, categories: &[&str]) -> Vector {
    Vector {
        id: id.to_owned(),
        values,
        categories: strings(categories),
        ..Default::default()
    }
}

// Ports: TestPurge @ add21b5
pub(super) fn test_purge(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "a",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add a");
    db.add_collection(
        ctx,
        "b",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add b");
    purge(ctx, db).expect("purge");

    let collections = db.list_collections(ctx).expect("list");
    assert!(collections.is_empty());
}

// Ports: TestCollections @ add21b5
pub(super) fn test_collections(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    // list collections
    let collections = db.list_collections(ctx).expect("list");
    assert!(collections.is_empty());
    // create collection
    db.add_collection(
        ctx,
        "test",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");
    // describe collection
    let info = db.describe_collection(ctx, "test").expect("describe");
    assert_eq!(info.name, "test");
    if info.dimension > 0 {
        assert_eq!(info.dimension, DEFAULT_VECTOR_SIZE);
    }
    assert_eq!(info.distance, Distance::Cosine);
    assert_eq!(info.config.quantization, QuantizationType::None);
    assert_eq!(info.config.bits, 0);
    // list collections
    let collections = db.list_collections(ctx).expect("list");
    assert_eq!(collections, ["test"]);
    // delete collection
    db.delete_collection(ctx, "test").expect("delete");
    // describe deleted collection
    let err = db.describe_collection(ctx, "test").unwrap_err();
    assert_eq!(err.kind(), Some(StorageError::NotFound));
    // list collections
    let collections = db.list_collections(ctx).expect("list");
    assert!(collections.is_empty());
    // delete non-existent collection
    assert!(db.delete_collection(ctx, "non-existent").is_err());
}

// Ports: TestVectors @ add21b5
pub(super) fn test_vectors(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");
    let count = db.count_vectors(ctx, "test").expect("count");
    assert_eq!(count, 0);

    let vector_a = vec![1.0, 0.0, 0.0, 0.0];
    let vector_b = vec![0.9, 0.1, 0.0, 0.0];

    db.add_vectors(
        ctx,
        "test",
        &[
            dense("a", vector_a.clone(), &["cat-a", "common"]),
            dense("b", vector_b.clone(), &["cat-b", "common"]),
        ],
    )
    .expect("add vectors");
    let count = db.count_vectors(ctx, "test").expect("count");
    assert_eq!(count, 2);

    let results = db
        .query_vectors(
            ctx,
            "test",
            &dense("", vector_a.clone(), &[]),
            &["cat-a"],
            10,
        )
        .expect("query");
    assert_eq!(ids(&results), ["a"]);
    assert!(!results[0].vector.categories.is_empty());

    let results = db
        .query_vectors(
            ctx,
            "test",
            &dense("", vector_a.clone(), &[]),
            &["common"],
            10,
        )
        .expect("query");
    assert_eq!(ids(&results), ["a", "b"]);
    assert!(results[0].score > results[1].score);
    for result in &results {
        assert!(!result.vector.categories.is_empty());
    }

    let results = db
        .query_vectors(
            ctx,
            "test",
            &dense("", vector_a.clone(), &[]),
            &["cat-a", "common"],
            10,
        )
        .expect("query");
    assert_eq!(ids(&results), ["a"]);

    let results = db
        .query_vectors(ctx, "test", &dense("", vector_a.clone(), &[]), &[], 1)
        .expect("query");
    assert!(!results.is_empty());
    for result in &results {
        assert!(!result.vector.categories.is_empty());
    }

    let query_vectors = db.get_vectors(ctx, "test", &["a"]).expect("get");
    assert_eq!(query_vectors.len(), 1);
    let results = db
        .query_vectors(ctx, "test", &query_vectors[0], &[], 10)
        .expect("query");
    assert_eq!(ids(&results), ["a", "b"]);
}

// Ports: TestHalfVectors @ add21b5
pub(super) fn test_half_vectors(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test_half",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");

    let vector_a = vec![1.0, 0.0, 0.0, 0.0];
    let vector_b = vec![0.0, 1.0, 0.0, 0.0];
    db.add_vectors(
        ctx,
        "test_half",
        &[
            Vector {
                id: "a".to_owned(),
                values: vec![0.0],
                h_values: floats::from_float32(&vector_a),
                indices: vec![1],
                ..Default::default()
            },
            Vector {
                id: "b".to_owned(),
                h_values: floats::from_float32(&vector_b),
                ..Default::default()
            },
        ],
    )
    .expect("add vectors");

    let vectors = db.get_vectors(ctx, "test_half", &["a", "b"]).expect("get");
    assert_eq!(vectors.len(), 2);
    assert_slice_close(&vector_a, &vectors[0].values, 0.001);
    assert_slice_close(&vector_b, &vectors[1].values, 0.001);

    let results = db
        .query_vectors(
            ctx,
            "test_half",
            &Vector {
                values: vec![0.0],
                h_values: floats::from_float32(&vector_a),
                indices: vec![1],
                ..Default::default()
            },
            &[],
            2,
        )
        .expect("query");
    assert_eq!(ids(&results), ["a", "b"]);
}

// Ports: TestGetVectors @ add21b5
pub(super) fn test_get_vectors(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");

    let timestamp_a = gotime::now().utc().truncate(MILLISECOND);
    let timestamp_b = timestamp_a.add(SECOND);
    let vector_a = Vector {
        id: "a".to_owned(),
        values: vec![1.0, 0.0, 0.0, 0.0],
        categories: strings(&["cat-a", "common"]),
        timestamp: timestamp_a.clone(),
        ..Default::default()
    };
    let vector_b = Vector {
        id: "b".to_owned(),
        values: vec![0.0, 1.0, 0.0, 0.0],
        is_hidden: true,
        categories: strings(&["cat-b", "common"]),
        timestamp: timestamp_b,
        ..Default::default()
    };
    db.add_vectors(ctx, "test", &[vector_a.clone(), vector_b.clone()])
        .expect("add vectors");

    let results = db
        .get_vectors(ctx, "test", &["b", "missing", "a", "b"])
        .expect("get");
    assert_eq!(results, [vector_b, vector_a]);

    let results = db.get_vectors(ctx, "test", &[]).expect("get");
    assert!(results.is_empty());
}

// Ports: TestSparse @ add21b5
pub(super) fn test_sparse(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test_sparse",
        0,
        Distance::Dot,
        VectorConfig::default(),
    )
    .expect("add");

    let info = db
        .describe_collection(ctx, "test_sparse")
        .expect("describe");
    assert_eq!(info.dimension, 0);
    assert_eq!(info.distance, Distance::Dot);

    let cutoff = gotime::now().utc().truncate(MILLISECOND);
    db.add_vectors(
        ctx,
        "test_sparse",
        &[
            Vector {
                id: "old".to_owned(),
                indices: vec![1, 100],
                values: vec![1.0, 1.0],
                timestamp: cutoff.add(Duration(-HOUR.0)),
                ..Default::default()
            },
            Vector {
                id: "match".to_owned(),
                indices: vec![1, 100],
                values: vec![1.0, 2.0],
                timestamp: cutoff.clone(),
                ..Default::default()
            },
            Vector {
                id: "other".to_owned(),
                indices: vec![2, 200],
                values: vec![1.0, 2.0],
                timestamp: cutoff.clone(),
                ..Default::default()
            },
        ],
    )
    .expect("add vectors");

    let count = db.count_vectors(ctx, "test_sparse").expect("count");
    assert_eq!(count, 3);

    let vectors = db
        .get_vectors(ctx, "test_sparse", &["match", "missing"])
        .expect("get");
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0].id, "match");
    assert_eq!(vectors[0].indices, [1, 100]);
    assert_eq!(vectors[0].values, [1.0, 2.0]);
    assert_eq!(vectors[0].timestamp, cutoff);

    let results = db
        .query_vectors(
            ctx,
            "test_sparse",
            &Vector {
                indices: vec![1, 100],
                values: vec![1.0, 2.0],
                ..Default::default()
            },
            &[],
            10,
        )
        .expect("query");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].vector.id, "match");

    db.delete_vectors(ctx, "test_sparse", &cutoff)
        .expect("delete vectors");
    let count = db.count_vectors(ctx, "test_sparse").expect("count");
    assert_eq!(count, 2);

    db.delete_collection(ctx, "test_sparse")
        .expect("delete collection");
    let err = db.describe_collection(ctx, "test_sparse").unwrap_err();
    assert_eq!(err.kind(), Some(StorageError::NotFound));
}

// Ports: TestHidden @ add21b5
pub(super) fn test_hidden(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test_hidden",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");

    let query = vec![1.0, 0.0, 0.0, 0.0];
    db.add_vectors(
        ctx,
        "test_hidden",
        &[
            dense("visible", vec![0.9, 0.1, 0.0, 0.0], &["common", "quo'te"]),
            Vector {
                id: "hidden".to_owned(),
                values: query.clone(),
                is_hidden: true,
                categories: strings(&["common", "quo'te"]),
                ..Default::default()
            },
        ],
    )
    .expect("add vectors");

    let count = db.count_vectors(ctx, "test_hidden").expect("count");
    assert_eq!(count, 2);

    let results = db
        .query_vectors(ctx, "test_hidden", &dense("", query.clone(), &[]), &[], 10)
        .expect("query");
    assert_eq!(ids(&results), ["visible"]);

    let results = db
        .query_vectors(
            ctx,
            "test_hidden",
            &dense("", query.clone(), &[]),
            &["common"],
            10,
        )
        .expect("query");
    assert_eq!(ids(&results), ["visible"]);

    let results = db
        .query_vectors(
            ctx,
            "test_hidden",
            &dense("", query.clone(), &[]),
            &["quo'te"],
            10,
        )
        .expect("query");
    assert_eq!(ids(&results), ["visible"]);
}

// Ports: TestDot @ add21b5
pub(super) fn test_dot(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test_dot",
        DEFAULT_VECTOR_SIZE,
        Distance::Dot,
        VectorConfig::default(),
    )
    .expect("add");

    let query = vec![1.0, 0.0, 0.0, 0.0];
    db.add_vectors(
        ctx,
        "test_dot",
        &[
            dense("a", vec![2.0, 0.0, 0.0, 0.0], &[]),
            dense("b", vec![1.0, 1.0, 0.0, 0.0], &[]),
        ],
    )
    .expect("add vectors");

    let results = db
        .query_vectors(ctx, "test_dot", &dense("", query, &[]), &[], 2)
        .expect("query");
    assert_eq!(ids(&results), ["a", "b"]);
    assert!(results[0].score > results[1].score);
}

// Ports: TestDeleteVectors @ add21b5
pub(super) fn test_delete_vectors(ctx: &Context, db: &dyn Database) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig::default(),
    )
    .expect("add");

    let vector_a = vec![1.0, 0.0, 0.0, 0.0];
    let vector_b = vec![0.9, 0.1, 0.0, 0.0];

    let cutoff = gotime::now().utc().truncate(MILLISECOND);
    db.add_vectors(
        ctx,
        "test",
        &[
            Vector {
                id: "old".to_owned(),
                values: vector_a.clone(),
                categories: strings(&["common"]),
                timestamp: cutoff.add(Duration(-HOUR.0)),
                ..Default::default()
            },
            Vector {
                id: "new".to_owned(),
                values: vector_b,
                categories: strings(&["common"]),
                timestamp: cutoff.clone(),
                ..Default::default()
            },
        ],
    )
    .expect("add vectors");

    db.delete_vectors(ctx, "test", &cutoff)
        .expect("delete vectors");
    let count = db.count_vectors(ctx, "test").expect("count");
    assert_eq!(count, 1);

    let results = db
        .query_vectors(ctx, "test", &dense("", vector_a, &[]), &["common"], 10)
        .expect("query");
    assert_eq!(ids(&results), ["new"]);
}

// Ports: testQuantization @ add21b5. The xvec suite does not call it (Go only
// calls it from the Qdrant and Milvus suites), so xvec_tests::quantization_none
// runs it with the one quantization xvec accepts.
pub(super) fn test_quantization(
    ctx: &Context,
    db: &dyn Database,
    quantization: QuantizationType,
    bits: i64,
) {
    setup(ctx, db);
    db.add_collection(
        ctx,
        "test_quantization",
        DEFAULT_VECTOR_SIZE,
        Distance::Cosine,
        VectorConfig {
            quantization: quantization.clone(),
            bits,
        },
    )
    .expect("add");

    let config = db
        .describe_collection(ctx, "test_quantization")
        .expect("describe");
    assert_eq!(config.config.quantization, quantization);
    if bits > 0 {
        assert_eq!(config.config.bits, bits);
    }

    let vector_a = vec![1.0, 0.0, 0.0, 0.0];
    let vector_b = vec![0.9, 0.1, 0.0, 0.0];
    db.add_vectors(
        ctx,
        "test_quantization",
        &[
            dense("a", vector_a.clone(), &["cat-a", "common"]),
            dense("b", vector_b, &["cat-b", "common"]),
        ],
    )
    .expect("add vectors");

    let results = db
        .query_vectors(
            ctx,
            "test_quantization",
            &dense("", vector_a, &[]),
            &["common"],
            10,
        )
        .expect("query");
    assert_eq!(results.len(), 2);

    db.delete_collection(ctx, "test_quantization")
        .expect("delete collection");
}

/// Go's `suite.InDeltaSlice`.
fn assert_slice_close(expected: &[f32], actual: &[f32], delta: f32) {
    assert_eq!(expected.len(), actual.len());
    for (e, a) in expected.iter().zip(actual) {
        assert!((e - a).abs() <= delta, "{e} vs {a}");
    }
}
