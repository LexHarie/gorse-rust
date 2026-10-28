//! `gounicode` against Go's `strings.ToLower` and `strings.EqualFold`
//! (`case_golden.json`, written by `contracts/tools/casegolden`).

use gorse_common::gounicode::{equal_fold, strings_to_lower};
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    to_lower: Vec<[String; 2]>,
    equal_fold: Vec<[String; 2]>,
    not_equal_fold: Vec<[String; 2]>,
}

fn golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/case_golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read golden")).expect("golden JSON")
}

#[test]
fn to_lower_matches_go() {
    let g = golden();
    for [s, want] in &g.to_lower {
        assert_eq!(&strings_to_lower(s), want, "ToLower({s:?})");
    }
}

#[test]
fn equal_fold_matches_go() {
    let g = golden();
    for [s, t] in &g.equal_fold {
        assert!(equal_fold(s, t), "EqualFold({s:?}, {t:?}) is true in Go");
        assert!(equal_fold(t, s), "EqualFold({t:?}, {s:?}) is true in Go");
    }
    for [s, t] in &g.not_equal_fold {
        assert!(!equal_fold(s, t), "EqualFold({s:?}, {t:?}) is false in Go");
    }
}
