//! The Go oracle for the output layers of `gorse-cli`, run on every test pass.
//!
//! - YAML that `pipeline get` writes: `contracts/tools/cligolden yaml` encodes
//!   each value in `data/cli/yaml-cases.txt` with gopkg.in/yaml.v3, and
//!   `gorse_cmd::cli_yaml::encode` must write the same bytes.
//! - Errors that `pipeline patch` reports: `cligolden jsonpatch` applies each
//!   patch in `data/cli/jsonpatch-cases.txt` with evanphx/json-patch v5.9.11, and
//!   the port must report the same decode and apply errors and the same result.
//! - The order `sort.Slice` leaves tied items in, which feedback listings use:
//!   `cligolden sort` prints the order for each size, key modulus and direction,
//!   and `gorse_common::gosort::slice` must leave the same order.
//! - Usage and parse errors: the Go `gorse-cli` and the Rust one get the same
//!   arguments, and must exit with the same status and print the same `Error:`
//!   lines. The usage block after them is clap's help text in Rust, so it is not
//!   compared.
//!
//! The Go tools need the `go` toolchain, and the test fails rather than skips
//! when it is missing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gorse_cmd::cli_jsonpatch::{apply, decode};
use gorse_cmd::cli_yaml::encode;
use gorse_common::gosort;
use gorse_conformance::cluster::{GoBuild, RustBuild};
use serde_json::Value;

/// The document every patch applies to, as `cligolden` holds it.
const DOC: &str = r#"{"a":1,"arr":[1,2,3],"obj":{"x":"y"},"nul":null,"s":"str"}"#;

/// The argument lists whose parse or usage error both CLIs report. Each one
/// fails before any request is sent, so no master is needed.
const USAGE_CASES: &[&str] = &[
    "--bogus",
    "bogus",
    "get users -n abc",
    "get users -n",
    "get users --bogus",
    "-x",
    "context use",
    "stat",
    "statss",
    "get usr",
    "get users -nq",
    "get users -q",
    "get users --n=abc",
    "get users -n=abc",
    "-v=abc",
    "get users --n",
    "get items -n 2 --category",
    "context --bogus",
    "get user",
    "get users -vq",
    "recommend user",
    "pipeline patch",
    "get users -n 5 -q",
    "hel",
    "context add",
    "dump",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn data(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/cli")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Runs `go run ./contracts/tools/cligolden MODE` with `stdin`, and returns its stdout.
fn go_oracle(mode: &str, stdin: &str) -> String {
    let mut child = Command::new("go")
        .args(["run", "./contracts/tools/cligolden", mode])
        .current_dir(repo_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run Go, which the oracle needs");
    let mut input = child.stdin.take().expect("stdin is piped");
    let text = stdin.to_owned();
    let writer = std::thread::spawn(move || input.write_all(text.as_bytes()));
    let output = child.wait_with_output().expect("wait for the Go oracle");
    writer
        .join()
        .expect("the writer thread finishes")
        .expect("write the cases to the Go oracle");
    assert!(
        output.status.success(),
        "cligolden {mode} failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the oracle writes UTF-8")
}

#[test]
fn yaml_layouts_match_go() {
    let cases = data("yaml-cases.txt");
    let go = go_oracle("yaml", &cases);
    let mut checked = 0;
    let mut rest = go.as_str();
    while let Some(start) = rest.find("== ") {
        let after = &rest[start + 3..];
        let header_end = after.find('\n').expect("each case starts with its value");
        let json = &after[..header_end];
        let body = &after[header_end + 1..];
        let end = body
            .find("---END---\n")
            .expect("each case ends with ---END---");
        let expected = &body[..end];
        let value: Value = serde_json::from_str(json).expect("the case is JSON");
        assert_eq!(encode(&value), expected, "YAML of {json}");
        checked += 1;
        rest = &body[end + "---END---\n".len()..];
    }
    assert_eq!(checked, cases.lines().count(), "every case was compared");
    assert!(
        checked >= 60,
        "expected at least 60 YAML cases, got {checked}"
    );
}

#[test]
fn json_patch_errors_match_go() {
    let cases = data("jsonpatch-cases.txt");
    let go = go_oracle("jsonpatch", &cases);
    let lines: Vec<&str> = go.lines().collect();
    let patches: Vec<&str> = cases.lines().collect();
    assert_eq!(lines.len(), patches.len() * 2, "two output lines per case");
    let doc: Value = serde_json::from_str(DOC).expect("the document is JSON");
    for (i, patch) in patches.iter().enumerate() {
        assert_eq!(lines[2 * i], *patch, "the oracle echoes case {i}");
        let result = lines[2 * i + 1];
        match decode(patch.as_bytes()) {
            Err(err) => assert_eq!(result, format!("  decode-err: {err}"), "decode of {patch}"),
            Ok(parsed) => match apply(&doc, &parsed) {
                Err(err) => {
                    assert_eq!(result, format!("  apply-err: {err}"), "apply of {patch}");
                }
                Ok(value) => {
                    let expected = result
                        .strip_prefix("  ok: ")
                        .unwrap_or_else(|| panic!("apply of {patch} succeeded in Rust only"));
                    let expected: Value =
                        serde_json::from_str(expected).expect("Go's result is JSON");
                    assert_eq!(value, expected, "result of {patch}");
                }
            },
        }
    }
}

#[test]
fn sort_ties_match_go() {
    #[derive(Clone)]
    struct Item {
        id: usize,
        key: usize,
    }
    let go = go_oracle("sort", "");
    let mut expected = go.lines();
    for n in [0, 1, 2, 5, 12, 13, 16, 17, 30, 64, 100, 257, 1000] {
        for m in [1, 2, 3, 7, 50] {
            for desc in [true, false] {
                let mut items: Vec<Item> = (0..n)
                    .map(|i| Item {
                        id: i,
                        key: (i * 7919) % m,
                    })
                    .collect();
                if desc {
                    gosort::slice(&mut items, |a, b| a.key > b.key);
                } else {
                    gosort::slice(&mut items, |a, b| a.key < b.key);
                }
                let hash = items.iter().fold(0u64, |hash, item| {
                    hash.wrapping_mul(1_000_003)
                        .wrapping_add(item.id as u64 + 1)
                });
                let head: Vec<String> = items.iter().take(12).map(|i| i.id.to_string()).collect();
                let line = format!(
                    "n={n} m={m} desc={desc} hash={hash} head=[{}]",
                    head.join(" ")
                );
                assert_eq!(
                    Some(line.as_str()),
                    expected.next(),
                    "sort case n={n} m={m} desc={desc}"
                );
            }
        }
    }
    assert_eq!(expected.next(), None, "the oracle printed no extra cases");
}

#[test]
fn usage_errors_match_go_cobra() {
    let go_cli = GoBuild::shared()
        .expect("build the Go nodes")
        .binary("gorse-cli");
    let rust_cli = RustBuild::shared()
        .expect("build the Rust nodes")
        .binary("gorse-cli");
    let error_lines = |stderr: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(stderr)
            .lines()
            .filter(|line| line.starts_with("Error:"))
            .map(str::to_owned)
            .collect()
    };
    for case in USAGE_CASES {
        let args: Vec<&str> = case.split_whitespace().collect();
        let run = |bin: &Path| {
            Command::new(bin)
                .args(&args)
                .env_remove("GORSE_ADMIN_ENDPOINT")
                .env_remove("GORSE_ADMIN_API_KEY")
                .output()
                .unwrap_or_else(|e| panic!("run {}: {e}", bin.display()))
        };
        let go = run(&go_cli);
        let rust = run(&rust_cli);
        let go_errors = error_lines(&go.stderr);
        assert!(
            !go_errors.is_empty(),
            "Go accepted `{case}`, so it is not a usage case"
        );
        assert_eq!(
            go.status.code(),
            rust.status.code(),
            "exit status of `{case}`"
        );
        assert_eq!(
            go_errors,
            error_lines(&rust.stderr),
            "Error lines of `{case}`"
        );
    }
}
