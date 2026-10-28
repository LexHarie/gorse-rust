// Ports: model/ctr/data_test.go @ add21b5

use std::collections::BTreeMap;

use gorse_common::floats::{from_float32, to_float32};
use gorse_common::gotime::unix;
use gorse_dataset::dataset::CtrSplit;
use gorse_dataset::unified_index::UnifiedMapIndexBuilder;

use super::{
    Dataset, Embedding, Label, Value, convert_embeddings, convert_labels, load_data_from_built_in,
};
use crate::ctr::new_map_index_dataset;

/// A map of string keys to values, for the nested test inputs.
fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

/// The labels sorted by name, for `assert.ElementsMatch`.
fn sorted_labels(mut labels: Vec<Label>) -> Vec<Label> {
    labels.sort_by(|a, b| a.name.cmp(&b.name));
    labels
}

fn label(name: &str, value: f32) -> Label {
    Label {
        name: name.to_string(),
        value,
    }
}

fn strings(values: &[&str]) -> Value {
    Value::Slice(
        values
            .iter()
            .map(|s| Value::String((*s).to_string()))
            .collect(),
    )
}

#[test]
fn convert_labels_test() {
    let features = convert_labels(&Value::Nil);
    assert!(features.is_empty());

    // categorical features
    let features = convert_labels(&Value::String("label".to_string()));
    assert_eq!(features, vec![label("label", 1.0)]);
    let features = convert_labels(&strings(&["1", "2", "3"]));
    assert_eq!(
        sorted_labels(features),
        vec![label("1", 1.0), label("2", 1.0), label("3", 1.0)]
    );
    let features = convert_labels(&map(&[
        ("city", Value::String("wenzhou".to_string())),
        ("tags", strings(&["1", "2", "3"])),
    ]));
    assert_eq!(
        sorted_labels(features),
        vec![
            label("city.wenzhou", 1.0),
            label("tags.1", 1.0),
            label("tags.2", 1.0),
            label("tags.3", 1.0),
        ]
    );
    let features = convert_labels(&map(&[(
        "address",
        map(&[
            ("province", Value::String("zhejiang".to_string())),
            ("city", Value::String("wenzhou".to_string())),
        ]),
    )]));
    assert_eq!(
        sorted_labels(features),
        vec![
            label("address.city.wenzhou", 1.0),
            label("address.province.zhejiang", 1.0),
        ]
    );

    // numerical features
    let features = convert_labels(&Value::Number("1".to_string()));
    assert_eq!(features, vec![label("", 1.0)]);
    let features = convert_labels(&map(&[
        ("city", Value::String("wenzhou".to_string())),
        ("tags", Value::Number("0.5".to_string())),
    ]));
    assert_eq!(
        sorted_labels(features),
        vec![label("city.wenzhou", 1.0), label("tags.", 0.5)]
    );

    // not supported
    let features = convert_labels(&Value::Slice(vec![
        Value::Float64(1.0),
        Value::Float64(2.0),
        Value::Float64(3.0),
    ]));
    assert!(features.is_empty());
    let features = convert_labels(&map(&[
        ("city", Value::String("wenzhou".to_string())),
        (
            "tags",
            Value::Slice(vec![
                Value::Float64(1.0),
                Value::Float64(2.0),
                Value::Float64(3.0),
            ]),
        ),
    ]));
    assert_eq!(features, vec![label("city.wenzhou", 1.0)]);
}

#[test]
fn convert_embeddings_test() {
    let embeddings = convert_embeddings(&Value::Nil);
    assert!(embeddings.is_empty());

    let embeddings = convert_embeddings(&Value::Float32s(vec![1.0, 2.0, 3.0]));
    assert_eq!(
        embeddings,
        vec![Embedding {
            name: String::new(),
            value: from_float32(&[1.0, 2.0, 3.0]),
        }]
    );

    let embeddings = convert_embeddings(&Value::Float64s(vec![1.0, 2.0, 3.0]));
    assert_eq!(
        embeddings,
        vec![Embedding {
            name: String::new(),
            value: from_float32(&[1.0, 2.0, 3.0]),
        }]
    );

    let embeddings = convert_embeddings(&Value::Slice(vec![
        Value::Float64(1.0),
        Value::Float32(2.0),
        Value::Float64(3.0),
    ]));
    assert_eq!(
        embeddings,
        vec![Embedding {
            name: String::new(),
            value: from_float32(&[1.0, 2.0, 3.0]),
        }]
    );

    let embeddings = convert_embeddings(&map(&[
        ("embedding1", Value::Float32s(vec![1.0, 2.0, 3.0])),
        (
            "a",
            map(&[("embedding2", Value::Float64s(vec![4.0, 5.0, 6.0]))]),
        ),
        ("no_embedding", Value::String("test".to_string())),
    ]));
    assert_eq!(embeddings.len(), 2);
    let mut embeddings = embeddings;
    embeddings.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        embeddings,
        vec![
            Embedding {
                name: "a.embedding2".to_string(),
                value: from_float32(&[4.0, 5.0, 6.0]),
            },
            Embedding {
                name: "embedding1".to_string(),
                value: from_float32(&[1.0, 2.0, 3.0]),
            },
        ]
    );
}

#[test]
#[ignore = "O-1: dataset from cdn.gorse.io"]
fn load_data_from_built_in_frappe() {
    let (train, test) = load_data_from_built_in("frappe").expect("frappe");
    assert_eq!(train.count(), 202027);
    assert_eq!(test.count(), 28860);
}

#[test]
fn dataset_split() {
    // create dataset
    let mut unified_index = UnifiedMapIndexBuilder::new();
    let mut data_set = new_map_index_dataset();
    let (num_users, num_items) = (5usize, 6usize);
    let mut user_labels = Vec::new();
    let mut item_labels = Vec::new();
    let mut item_embeddings = Vec::new();
    for i in 0..num_users {
        unified_index.add_user(&format!("user{i}"));
        unified_index.add_user_label(&format!("user_label{}", 2 * i));
        unified_index.add_user_label(&format!("user_label{}", 2 * i + 1));
        user_labels.push(vec![((2 * i) as i32, 1.0), ((2 * i + 1) as i32, 1.0)]);
    }
    for i in 0..num_items {
        unified_index.add_item(&format!("item{i}"));
        unified_index.add_item_label(&format!("item_label{}", 3 * i));
        unified_index.add_item_label(&format!("item_label{}", 3 * i + 1));
        unified_index.add_item_label(&format!("item_label{}", 3 * i + 2));
        item_labels.push(vec![
            ((3 * i) as i32, 1.0),
            ((3 * i + 1) as i32, 1.0),
            ((3 * i + 2) as i32, 1.0),
        ]);
        item_embeddings.push(vec![from_float32(&[
            i as f32,
            i as f32 + 0.1,
            i as f32 + 0.2,
        ])]);
    }
    data_set.user_labels = user_labels;
    data_set.item_labels = item_labels;
    data_set.item_embeddings = item_embeddings;
    let mut context_labels = Vec::new();
    for i in 0..num_users {
        for j in 0..num_items {
            data_set.users.push(i as i32);
            data_set.items.push(j as i32);
            context_labels.push(vec![((i * j) as i32, 0.5)]);
            if i + j > 4 {
                data_set.target.push(1.0);
                data_set
                    .timestamps
                    .push(unix((i * num_items + j) as i64, 0));
                data_set.positive_count += 1;
            } else {
                data_set.target.push(-1.0);
                data_set
                    .timestamps
                    .push(unix((i * num_items + j) as i64, 0));
                data_set.negative_count += 1;
            }
        }
    }
    data_set.context_labels = Some(context_labels);
    data_set.index = Some(unified_index.build());

    assert_eq!(data_set.count(), num_users * num_items);
    assert_eq!(data_set.count_users(), num_users);
    assert_eq!(data_set.count_items(), num_items);
    assert_eq!(data_set.positive_count, num_users * num_items / 2);
    assert_eq!(data_set.negative_count, num_users * num_items / 2);

    let (features, values, embeddings, target) = data_set.get(2);
    let index = data_set.index.as_ref().expect("index");
    let (uc, ic) = (index.count_users(), index.count_items());
    let ulc = index.count_user_labels();
    assert_eq!(
        features,
        vec![
            0,
            uc + 2,
            uc + ic,
            uc + ic + 1,
            uc + ic + ulc + 6,
            uc + ic + ulc + 7,
            uc + ic + ulc + 8,
            0,
        ]
    );
    let floats = to_float32(&embeddings[0]);
    for (got, want) in floats.iter().zip([2.0f32, 2.1, 2.2]) {
        assert!((got - want).abs() <= 0.001, "{got} != {want}");
    }
    assert_eq!(values, vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5]);
    assert_eq!(target, -1.0);

    // split
    let (train, test) = data_set.split(0.2, 0);
    assert_eq!(train.count_users(), num_users);
    assert_eq!(train.count_items(), num_items);
    assert_eq!(train.count(), 24);
    assert_eq!(train.positive_count, 12);
    assert_eq!(train.negative_count, 12);
    assert_eq!(test.count_users(), num_users);
    assert_eq!(test.count_items(), num_items);
    assert_eq!(test.count(), 6);
    assert_eq!(test.positive_count, 3);
    assert_eq!(test.negative_count, 3);
}

#[test]
fn dataset_split_by_user_time() {
    let mut unified_index = UnifiedMapIndexBuilder::new();
    for i in 0..3 {
        unified_index.add_user(&format!("user{i}"));
    }
    for i in 0..6 {
        unified_index.add_item(&format!("item{i}"));
    }

    let data_set = Dataset {
        index: Some(unified_index.build()),
        users: vec![0, 0, 0, 1, 1, 2],
        items: vec![0, 1, 2, 3, 4, 5],
        target: vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0],
        timestamps: vec![
            unix(30, 0),
            unix(10, 0),
            unix(20, 0),
            unix(40, 0),
            unix(50, 0),
            unix(60, 0),
        ],
        positive_count: 3,
        negative_count: 3,
        ..Dataset::default()
    };

    let (train, test) = data_set.split_by_user_time(0.5);

    type Sample = (i32, i32, f32, i64);
    let collect = |data_set: &Dataset| -> Vec<Sample> {
        let mut samples: Vec<Sample> = (0..data_set.count())
            .map(|i| {
                (
                    data_set.users[i],
                    data_set.items[i],
                    data_set.target[i],
                    data_set.timestamps[i].unix(),
                )
            })
            .collect();
        samples.sort_by_key(|sample| (sample.0, sample.1));
        samples
    };

    assert_eq!(train.count(), 4);
    assert_eq!(train.positive_count, 1);
    assert_eq!(train.negative_count, 3);
    assert_eq!(
        collect(&train),
        vec![
            (0, 1, -1.0, 10),
            (0, 2, 1.0, 20),
            (1, 3, -1.0, 40),
            (2, 5, -1.0, 60),
        ]
    );

    assert_eq!(test.count(), 2);
    assert_eq!(test.positive_count, 2);
    assert_eq!(test.negative_count, 0);
    assert_eq!(collect(&test), vec![(0, 0, 1.0, 30), (1, 4, 1.0, 50)]);
}
