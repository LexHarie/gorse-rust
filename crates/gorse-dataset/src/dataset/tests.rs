// Ports: dataset/dataset_test.go @ add21b5

//! Go's `dataset_test.go`, case for case. The loader test downloads a dataset and
//! is ignored here (PORTING_NOTES.md O-1). Two extra tests cover the file parsers
//! on small local files, since only that ignored test reaches them otherwise.

use gorse_common::floats;
use gorse_common::gotime::{self, Month, Time};
use gorse_common::math32;
use gorse_storage::data::types as data;
use indexmap::IndexMap;

use super::{CfSplit, Dataset, DatasetItem, DatasetUser, Id, Value, load_data_from_built_in};

/// A text label value.
fn text(s: &str) -> data::Labels {
    data::Labels::String(s.to_owned())
}

/// A numeric label value (Go's `float64`).
fn number(n: f64) -> data::Labels {
    data::Labels::Float64(n)
}

/// A label value that is a list.
fn array(items: Vec<data::Labels>) -> data::Labels {
    data::Labels::Array(items)
}

/// A label value that is a map.
fn object(entries: Vec<(&str, data::Labels)>) -> data::Labels {
    data::Labels::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// A processed map of labels.
fn processed(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<IndexMap<_, _>>(),
    )
}

/// Go's `time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)`.
fn date_2025() -> Time {
    gotime::date(2025, Month(1), 1, 0, 0, 0, 0, &gotime::utc())
}

/// Asserts that `actual` is within `delta` of `expected`, as Go's `InDelta`.
fn assert_in_delta(actual: f32, expected: f32, delta: f32) {
    assert!(
        (actual - expected).abs() <= delta,
        "{actual} is not within {delta} of {expected}"
    );
}

#[test]
fn dataset_add_item() {
    let mut data_set = Dataset::new(gotime::now(), 0, 1);
    data_set.add_item(data::Item {
        item_id: "1".to_owned(),
        is_hidden: false,
        categories: Some(vec!["a".to_owned(), "b".to_owned()]),
        timestamp: date_2025(),
        labels: object(vec![
            ("a", number(1.0)),
            (
                "embedded",
                array(vec![number(1.1), number(2.2), number(3.3)]),
            ),
            ("tags", array(vec![text("a"), text("b"), text("c")])),
        ]),
        comment: "comment".to_owned(),
        ..data::Item::default()
    });
    data_set.add_item(data::Item {
        item_id: "2".to_owned(),
        is_hidden: true,
        categories: Some(vec!["a".to_owned(), "b".to_owned()]),
        timestamp: date_2025(),
        labels: object(vec![
            ("a", number(1.0)),
            (
                "embedded",
                array(vec![number(1.1), number(2.2), number(3.3)]),
            ),
            ("tags", array(vec![text("b"), text("c"), text("a")])),
            ("topics", array(vec![text("a"), text("b"), text("c")])),
        ]),
        comment: "comment".to_owned(),
        ..data::Item::default()
    });
    data_set.add_item(data::Item {
        item_id: "3".to_owned(),
        is_hidden: false,
        categories: Some(vec!["a".to_owned()]),
        timestamp: date_2025(),
        labels: object(vec![(
            "embedded",
            array(vec![number(1.1), number(0.0), number(2.0)]),
        )]),
        comment: "comment".to_owned(),
        ..data::Item::default()
    });
    assert_eq!(data_set.get_items().len(), 3);
    assert_eq!(
        data_set.get_items()[0],
        DatasetItem {
            item_id: "1".to_owned(),
            is_hidden: false,
            categories: Some(vec!["a".to_owned(), "b".to_owned()]),
            timestamp: date_2025(),
            labels: processed(vec![
                ("a", Value::Plain(number(1.0))),
                (
                    "embedded",
                    Value::Floats(floats::from_float32(&[1.1, 2.2, 3.3]))
                ),
                ("tags", Value::Ids(vec![Id(0), Id(1), Id(2)])),
            ]),
            comment: "comment".to_owned(),
        }
    );
    assert_eq!(
        data_set.get_items()[1],
        DatasetItem {
            item_id: "2".to_owned(),
            is_hidden: true,
            categories: Some(vec!["a".to_owned(), "b".to_owned()]),
            timestamp: date_2025(),
            labels: processed(vec![
                ("a", Value::Plain(number(1.0))),
                (
                    "embedded",
                    Value::Floats(floats::from_float32(&[1.1, 2.2, 3.3]))
                ),
                ("tags", Value::Ids(vec![Id(1), Id(2), Id(0)])),
                ("topics", Value::Ids(vec![Id(3), Id(4), Id(5)])),
            ]),
            comment: "comment".to_owned(),
        }
    );
    assert_eq!(
        data_set.get_items()[2],
        DatasetItem {
            item_id: "3".to_owned(),
            is_hidden: false,
            categories: Some(vec!["a".to_owned()]),
            timestamp: date_2025(),
            labels: processed(vec![(
                "embedded",
                Value::Floats(floats::from_float32(&[1.1, 0.0, 2.0])),
            )]),
            comment: "comment".to_owned(),
        }
    );
}

#[test]
fn dataset_get_item_column_values_idf() {
    let mut data_set = Dataset::new(gotime::now(), 0, 1);
    data_set.add_item(data::Item {
        item_id: "1".to_owned(),
        is_hidden: false,
        categories: Some(vec!["a".to_owned(), "b".to_owned()]),
        timestamp: date_2025(),
        labels: object(vec![("tags", array(vec![text("a"), text("b"), text("c")]))]),
        comment: "comment".to_owned(),
        ..data::Item::default()
    });
    data_set.add_item(data::Item {
        item_id: "2".to_owned(),
        is_hidden: false,
        categories: Some(vec!["a".to_owned(), "b".to_owned()]),
        timestamp: date_2025(),
        labels: object(vec![("tags", array(vec![text("a"), text("e")]))]),
        comment: "comment".to_owned(),
        ..data::Item::default()
    });
    let idf = data_set.get_item_column_values_idf();
    assert_eq!(idf.len(), 4);
    assert_in_delta(idf[0], math32::log(2.0), 1e-6);
    assert_in_delta(idf[1], math32::log(3.0), 1e-6);
}

#[test]
fn dataset_add_user() {
    let mut data_set = Dataset::new(gotime::now(), 1, 0);
    data_set.add_user(data::User {
        user_id: "1".to_owned(),
        labels: object(vec![("a", number(1.0)), ("b", text("a"))]),
        comment: "comment".to_owned(),
        ..data::User::default()
    });
    assert_eq!(data_set.users.len(), 1);
    assert_eq!(
        data_set.users[0],
        DatasetUser {
            user_id: "1".to_owned(),
            labels: processed(vec![
                ("a", Value::Plain(number(1.0))),
                ("b", Value::Id(Id(0)))
            ]),
            comment: "comment".to_owned(),
        }
    );
}

#[test]
fn dataset_get_user_column_values_idf() {
    let mut data_set = Dataset::new(gotime::now(), 1, 0);
    data_set.add_user(data::User {
        user_id: "1".to_owned(),
        labels: object(vec![("tags", array(vec![text("a"), text("b"), text("c")]))]),
        comment: "comment".to_owned(),
        ..data::User::default()
    });
    data_set.add_user(data::User {
        user_id: "2".to_owned(),
        labels: object(vec![("tags", array(vec![text("a"), text("e")]))]),
        comment: "comment".to_owned(),
        ..data::User::default()
    });
    let idf = data_set.get_user_column_values_idf();
    assert_eq!(idf.len(), 4);
    assert_in_delta(idf[0], math32::log(2.0), 1e-6);
    assert_in_delta(idf[1], math32::log(3.0), 1e-6);
}

#[test]
fn dataset_add_feedback() {
    let mut data_set = Dataset::new(gotime::now(), 10, 10);
    for i in 0..10 {
        data_set.add_user(data::User {
            user_id: i.to_string(),
            ..data::User::default()
        });
    }
    for i in 0..10 {
        data_set.add_item(data::Item {
            item_id: i.to_string(),
            ..data::Item::default()
        });
    }
    for i in 0..10usize {
        for j in i..10 {
            data_set.add_feedback(
                &i.to_string(),
                &j.to_string(),
                gotime::unix((i * 10 + j) as i64, 0),
            );
        }
    }
    let user_idf = data_set.get_user_idf();
    let item_idf = data_set.get_item_idf();
    for i in 0..10usize {
        assert_eq!(data_set.get_user_feedback()[i].len(), 10 - i);
        assert_eq!(data_set.get_item_feedback()[i].len(), i + 1);
        assert_eq!(data_set.timestamps[i].len(), 10 - i);
        assert_in_delta(user_idf[i], math32::log(1.0 + 10.0 / (10 - i) as f32), 1e-2);
        assert_in_delta(item_idf[i], math32::log(1.0 + 10.0 / (i + 1) as f32), 1e-2);
    }
}

#[test]
fn dataset_split() {
    const NUM_USERS: usize = 3;
    const NUM_ITEMS: usize = 5;
    // create dataset
    let mut dataset = Dataset::new(gotime::now(), NUM_USERS, NUM_ITEMS);
    for i in 0..NUM_USERS {
        dataset.add_user(data::User {
            user_id: format!("user{i}"),
            ..data::User::default()
        });
    }
    for i in 0..NUM_ITEMS {
        dataset.add_item(data::Item {
            item_id: format!("item{i}"),
            ..data::Item::default()
        });
    }
    for i in 0..NUM_USERS {
        for j in i + 1..NUM_ITEMS {
            dataset.add_feedback(&format!("user{i}"), &format!("item{j}"), Time::default());
        }
    }
    assert_eq!(dataset.count_feedback(), 9);
    // split
    let (train, test) = dataset.split_cf(0, 0);
    assert_eq!(train.count_users(), NUM_USERS);
    assert_eq!(train.count_items(), NUM_ITEMS);
    assert_eq!(train.count_feedback(), 9 - NUM_USERS);
    assert_eq!(test.count_users(), NUM_USERS);
    assert_eq!(test.count_items(), NUM_ITEMS);
    assert_eq!(test.count_feedback(), NUM_USERS);
    // part split
    let (train2, test2) = dataset.split_cf(2, 0);
    assert_eq!(train2.count_users(), NUM_USERS);
    assert_eq!(train2.count_items(), NUM_ITEMS);
    assert_eq!(train2.count_feedback(), 7);
    assert_eq!(test2.count_users(), NUM_USERS);
    assert_eq!(test2.count_items(), NUM_ITEMS);
    assert_eq!(test2.count_feedback(), 2);
}

#[test]
fn dataset_split_latest() {
    const NUM_USERS: usize = 3;
    const NUM_ITEMS: usize = 5;
    // create dataset
    let mut dataset = Dataset::new(gotime::now(), NUM_USERS, NUM_ITEMS);
    for i in 0..NUM_USERS {
        dataset.add_user(data::User {
            user_id: format!("user{i}"),
            ..data::User::default()
        });
    }
    for i in 0..NUM_ITEMS {
        dataset.add_item(data::Item {
            item_id: format!("item{i}"),
            ..data::Item::default()
        });
    }
    for i in 0..NUM_USERS {
        for j in i + 1..NUM_ITEMS {
            dataset.add_feedback(
                &format!("user{i}"),
                &format!("item{j}"),
                gotime::unix(j as i64, 0),
            );
        }
    }
    assert_eq!(dataset.count_feedback(), 9);
    // split
    let (train, test) = dataset.split_latest(i64::MAX);
    assert_eq!(train.count_users(), NUM_USERS);
    assert_eq!(train.count_items(), NUM_ITEMS);
    assert_eq!(test.count_users(), NUM_USERS);
    assert_eq!(test.count_items(), NUM_ITEMS);
    assert_eq!(train.count_feedback(), 6);
    assert_eq!(test.count_feedback(), 3);
    for i in 0..NUM_USERS {
        assert_eq!(train.get_user_feedback()[i].len(), NUM_ITEMS - i - 2);
        assert_eq!(test.get_user_feedback()[i].len(), 1);
        assert_eq!(test.get_user_feedback()[i][0], 4);
    }
}

#[test]
#[ignore = "needs cdn.gorse.io, which this container cannot reach (O-1)"]
fn dataset_load_movie_lens_1m() {
    let (train, test) = load_data_from_built_in("ml-1m").expect("load ml-1m");
    assert_eq!(train.get_users().len(), 6040);
    assert_eq!(train.get_items().len(), 3706);
    assert_eq!(train.count_feedback(), 994169);
    assert_eq!(test.get_users().len(), 6040);
    assert_eq!(test.get_items().len(), 3706);
    assert_eq!(test.count_feedback(), 6040);
}

/// Writes `content` to a file in `dir` and returns its path.
fn write_file(dir: &tempfile::TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("write the test file");
    path
}

#[test]
fn load_train_and_test_read_ncf_files() {
    let dir = tempfile::tempdir().expect("create a temporary directory");
    let train_path = write_file(&dir, "train.txt", "0\t2\n1\t0\n");
    let test_path = write_file(&dir, "test.txt", "(0,2)\t1\t0\n(1,0)\t2\n");
    let mut train = super::load_train(&train_path).expect("load the training file");
    assert_eq!(train.get_users().len(), 2);
    assert_eq!(train.get_items().len(), 3);
    assert_eq!(train.count_feedback(), 2);
    assert_eq!(train.get_user_feedback()[0], [2]);
    let mut test = Dataset::new(train.get_timestamp(), 0, 0);
    test.user_dict = train.user_dict.clone();
    test.item_dict = train.item_dict.clone();
    test.user_feedback = vec![Vec::new(); 2];
    test.item_feedback = vec![Vec::new(); 3];
    test.timestamps = vec![Vec::new(); 2];
    test.negatives = vec![Vec::new(); 2];
    test.users = train.users.clone();
    test.items = train.items.clone();
    super::load_test(&mut test, &test_path).expect("load the test file");
    assert_eq!(test.negatives[0], [1, 0]);
    assert_eq!(test.negatives[1], [2]);
    assert_eq!(test.count_feedback(), 2);
    // The sample is cached, so it is the negatives the file gave.
    let samples = train.sample_user_negatives(&test, 1);
    assert_eq!(samples.len(), 2);
}

#[test]
fn load_test_rejects_a_line_without_parentheses() {
    let dir = tempfile::tempdir().expect("create a temporary directory");
    let mut train = super::load_train(&write_file(&dir, "train.txt", "0\t0\n"))
        .expect("load the training file");
    let test_path = write_file(&dir, "test.txt", "0,0\t0\n");
    let err = super::load_test(&mut train, &test_path).expect_err("the line has no parentheses");
    assert!(matches!(err, crate::Error::WrongFormat(_)));
}
