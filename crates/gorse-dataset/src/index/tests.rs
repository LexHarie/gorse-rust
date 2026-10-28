// Ports: dataset/index_test.go @ add21b5

//! Go's `index_test.go`, case for case, and two cases for the error paths.

use super::{Index, NOT_ID, marshal_index, unmarshal_index};
use crate::Error;

#[test]
fn index() {
    // Null indexer
    let index: Option<&Index> = None;
    assert_eq!(index.map_or(0, Index::len), 0);
    // Create a indexer
    let mut index = Index::new();
    assert_eq!(index.len(), 0);
    // Add Names
    index.add("1");
    index.add("2");
    index.add("4");
    index.add("8");
    assert_eq!(index.len(), 4);
    assert_eq!(index.to_number("1"), 0);
    assert_eq!(index.to_number("2"), 1);
    assert_eq!(index.to_number("4"), 2);
    assert_eq!(index.to_number("8"), 3);
    assert_eq!(index.to_number("1000"), NOT_ID);
    assert_eq!(index.to_name(0), "1");
    assert_eq!(index.to_name(1), "2");
    assert_eq!(index.to_name(2), "4");
    assert_eq!(index.to_name(3), "8");
    // Get names
    assert_eq!(index.get_names(), ["1", "2", "4", "8"].map(String::from));
    // Encode and decode
    let mut buf = Vec::new();
    marshal_index(&mut buf, &index).expect("marshal into memory");
    let index_copy = unmarshal_index(&mut buf.as_slice()).expect("unmarshal the bytes");
    assert_eq!(index, index_copy);
}

#[test]
fn index_unmarshal_drops_repeated_names() {
    // Two names, the second repeated: the index keeps the first position.
    let mut buf = Vec::new();
    buf.extend_from_slice(&3i32.to_le_bytes());
    for name in ["a", "b", "a"] {
        buf.extend_from_slice(&(name.len() as i32).to_le_bytes());
        buf.extend_from_slice(name.as_bytes());
    }
    let index = unmarshal_index(&mut buf.as_slice()).expect("unmarshal the bytes");
    assert_eq!(index.get_names(), ["a", "b"].map(String::from));
    assert_eq!(index.to_number("a"), 0);
}

#[test]
fn index_unmarshal_rejects_negative_length() {
    let buf = (-1i32).to_le_bytes();
    let err = unmarshal_index(&mut buf.as_slice()).expect_err("a negative length is an error");
    assert!(matches!(err, Error::NegativeLength(-1)));
}
