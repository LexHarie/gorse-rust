// Ports: dataset/unified_index_test.go @ add21b5

//! Go's `unified_index_test.go`, case for case, and one case for the error path.

use std::panic;

use super::{
    Error, UnifiedDirectIndex, UnifiedIndex, UnifiedMapIndexBuilder, marshal_unified_index,
    unmarshal_unified_index,
};

/// The strings of a list of names, for comparing with literals.
fn strs(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

#[test]
fn unified_map_index() {
    // create unified map index
    let mut builder = UnifiedMapIndexBuilder::new();
    let (num_users, num_items, num_user_labels, num_item_labels, num_ctx_labels) = (3, 4, 5, 6, 7);
    for i in 0..num_users {
        builder.add_user(&format!("user{i}"));
    }
    for i in 0..num_items {
        builder.add_item(&format!("item{i}"));
    }
    for i in 0..num_user_labels {
        builder.add_user_label(&format!("user_label{i}"));
    }
    for i in 0..num_item_labels {
        builder.add_item_label(&format!("item_label{i}"));
    }
    for i in 0..num_ctx_labels {
        builder.add_ctx_label(&format!("ctx_label{i}"));
    }
    let index = builder.build();
    // check count
    assert_eq!(
        index.len(),
        num_users + num_items + num_user_labels + num_item_labels + num_ctx_labels
    );
    assert_eq!(index.count_users(), num_users);
    assert_eq!(index.count_items(), num_items);
    assert_eq!(index.count_user_labels(), num_user_labels);
    assert_eq!(index.count_item_labels(), num_item_labels);
    assert_eq!(index.count_context_labels(), num_ctx_labels);
    // check encode
    let users = index.get_users();
    for i in 0..num_users {
        let user_index = index.encode_user(&format!("user{i}"));
        assert_eq!(user_index, i);
        assert_eq!(users[i as usize], format!("user{i}"));
    }
    let items = index.get_items();
    for i in 0..num_items {
        let item_index = index.encode_item(&format!("item{i}"));
        assert_eq!(item_index, num_users + i);
        assert_eq!(items[i as usize], format!("item{i}"));
    }
    let user_labels = index.get_user_labels();
    for i in 0..num_user_labels {
        let user_label_index = index.encode_user_label(&format!("user_label{i}"));
        assert_eq!(user_label_index, num_users + num_items + i);
        assert_eq!(user_labels[i as usize], format!("user_label{i}"));
    }
    let item_labels = index.get_item_labels();
    for i in 0..num_item_labels {
        let item_label_index = index.encode_item_label(&format!("item_label{i}"));
        assert_eq!(
            item_label_index,
            num_users + num_items + num_user_labels + i
        );
        assert_eq!(item_labels[i as usize], format!("item_label{i}"));
    }
    let ctx_labels = index.get_context_labels();
    for i in 0..num_ctx_labels {
        let ctx_label_index = index.encode_context_label(&format!("ctx_label{i}"));
        assert_eq!(
            ctx_label_index,
            num_users + num_items + num_user_labels + num_item_labels + i
        );
        assert_eq!(ctx_labels[i as usize], format!("ctx_label{i}"));
    }
    // Encode and decode
    let mut buf = Vec::new();
    marshal_unified_index(&mut buf, Some(&index)).expect("marshal into memory");
    let index_copy = unmarshal_unified_index(&mut buf.as_slice()).expect("unmarshal the bytes");
    assert_eq!(index_copy, Some(index));
}

#[test]
fn unified_direct_index() {
    let index = UnifiedIndex::Direct(UnifiedDirectIndex::new(10));
    assert_eq!(index.len(), 10);
    assert_eq!(strs(&index.get_items()), ["0", "1"]);
    assert_eq!(strs(&index.get_users()), ["2", "3"]);
    assert_eq!(strs(&index.get_item_labels()), ["4", "5"]);
    assert_eq!(strs(&index.get_user_labels()), ["6", "7"]);
    assert_eq!(strs(&index.get_context_labels()), ["8", "9"]);
    assert_eq!(index.count_items(), 2);
    assert_eq!(index.count_users(), 2);
    assert_eq!(index.count_item_labels(), 2);
    assert_eq!(index.count_user_labels(), 2);
    assert_eq!(index.count_context_labels(), 2);
    assert!(panics(|| index.encode_item("abc")));
    assert!(panics(|| index.encode_user("abc")));
    assert!(panics(|| index.encode_item_label("abc")));
    assert!(panics(|| index.encode_user_label("abc")));
    assert!(panics(|| index.encode_context_label("abc")));
    assert_eq!(index.encode_item("1"), 1);
    assert_eq!(index.encode_user("2"), 2);
    assert_eq!(index.encode_item_label("3"), 3);
    assert_eq!(index.encode_user_label("4"), 4);
    assert_eq!(index.encode_context_label("5"), 5);
    // Encode and decode
    let mut buf = Vec::new();
    marshal_unified_index(&mut buf, Some(&index)).expect("marshal into memory");
    let index_copy = unmarshal_unified_index(&mut buf.as_slice()).expect("unmarshal the bytes");
    assert_eq!(index_copy, Some(index));
}

#[test]
fn unified_index_nil_round_trip() {
    let mut buf = Vec::new();
    marshal_unified_index(&mut buf, None).expect("marshal into memory");
    assert_eq!(buf, [2]);
    let index = unmarshal_unified_index(&mut buf.as_slice()).expect("unmarshal the bytes");
    assert_eq!(index, None);
}

#[test]
fn unified_index_unknown_type_tag() {
    let err = unmarshal_unified_index(&mut [9u8].as_slice()).expect_err("tag 9 is unknown");
    assert!(matches!(err, Error::UnknownIndexType(9)));
}

/// Whether `f` panics. Go's `assert.Panics`.
fn panics<R>(f: impl FnOnce() -> R + panic::UnwindSafe) -> bool {
    panic::catch_unwind(f).is_err()
}
