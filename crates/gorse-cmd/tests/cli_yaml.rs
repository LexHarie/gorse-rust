//! The YAML writer (`gorse_cmd::cli_yaml`) against the output of gopkg.in/yaml.v3
//! v3.0.1. `golden/yaml-go.txt` holds what Go writes for each value; each block
//! starts with `== ` and the JSON value, then the YAML, then `---END---`. The
//! probe that recorded it is `golden/yaml-probe.go.txt`. The conformance crate
//! checks the same layouts through `pipeline get` on a live master.

use gorse_cmd::cli_yaml::encode;
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/yaml-go.txt");

/// Splits the golden file into `(json, yaml)` pairs, one per recorded value.
fn recorded_cases() -> Vec<(&'static str, &'static str)> {
    let mut cases = Vec::new();
    let mut rest = GOLDEN;
    while let Some(start) = rest.find("== ") {
        let after = &rest[start + 3..];
        let header_end = after.find('\n').expect("the header line ends");
        let json = &after[..header_end];
        let body_start = header_end + 1;
        let body_end = after[body_start..]
            .find("---END---\n")
            .expect("each case ends with ---END---");
        let yaml = &after[body_start..body_start + body_end];
        cases.push((json, yaml));
        rest = &after[body_start + body_end + "---END---\n".len()..];
    }
    cases
}

#[test]
fn every_recorded_value_is_written_as_go_writes_it() {
    let cases = recorded_cases();
    assert!(
        cases.len() >= 60,
        "the golden file holds {} cases, expected at least 60",
        cases.len()
    );
    for (json, expected) in &cases {
        let value: Value = serde_json::from_str(json).expect("the recorded value is JSON");
        assert_eq!(&encode(&value), expected, "value {json}");
    }
}
