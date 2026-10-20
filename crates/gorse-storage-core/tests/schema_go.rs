// Ports: storage/schema_test.go (TestAppendURLParams) @ add21b5

//! Go's `schema_test.go`, case for case.

use gorse_storage_core::scheme::append_url_params;

#[test]
fn test_append_url_params() {
    // test windows path
    let url = append_url_params(r"c:\\sqlite.db", &[("a", "b")]).expect("no error");
    assert_eq!(url, br"c:\\sqlite.db?a=b");
    // test no scheme
    let url = append_url_params("sqlite.db", &[("a", "b")]).expect("no error");
    assert_eq!(url, b"sqlite.db?a=b");
}
