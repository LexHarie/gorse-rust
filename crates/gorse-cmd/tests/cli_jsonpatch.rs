//! The JSON patch port (`gorse_cmd::cli_jsonpatch`) against the output of
//! evanphx/json-patch v5.9.11. `golden/jsonpatch-go.txt` holds what Go's
//! `DecodePatch` and `Apply` return for each line of `golden/jsonpatch-cases.txt`
//! on the same document; the probe that recorded it is `golden/jsonpatch-probe.go.txt`.
//! The conformance crate runs the same cases through both CLIs on a live master.

use gorse_cmd::cli_jsonpatch::{apply, decode};
use serde_json::Value;

/// The document every case applies to, as the probe's.
const DOC: &str = r#"{"a":1,"arr":[1,2,3],"obj":{"x":"y"},"nul":null,"s":"str"}"#;

/// Go's output for the cases, one case line then one result line.
const GOLDEN: &str = include_str!("golden/jsonpatch-go.txt");

#[test]
fn every_recorded_case_matches_go() {
    let lines: Vec<&str> = GOLDEN.lines().collect();
    assert_eq!(lines.len() % 2, 0, "each case has a result line");
    let doc: Value = serde_json::from_str(DOC).expect("the document is JSON");
    let mut checked = 0;
    for pair in lines.chunks(2) {
        let (case, expected) = (pair[0], pair[1]);
        let expected = expected
            .strip_prefix("  ")
            .expect("result lines are indented");
        match decode(case.as_bytes()) {
            Err(err) => assert_eq!(expected, format!("decode-err: {err}"), "case {case}"),
            Ok(patch) => match apply(&doc, &patch) {
                Err(err) => assert_eq!(expected, format!("apply-err: {err}"), "case {case}"),
                Ok(out) => {
                    let want: Value =
                        serde_json::from_str(expected.strip_prefix("ok: ").expect("an ok result"))
                            .expect("Go's result is JSON");
                    assert_eq!(out, want, "case {case}");
                }
            },
        }
        checked += 1;
    }
    assert_eq!(
        checked,
        GOLDEN.lines().count() / 2,
        "every recorded case is compared"
    );
}
