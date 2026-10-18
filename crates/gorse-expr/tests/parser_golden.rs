//! The parser against what Go's (expr-lang v1.17.8) does with the curated
//! corpus of `parser_golden.json`, written by
//! `contracts/tools/exprparsegolden`.

mod common;

use common::{array, config_from_json, digest, lex_text, load, opt_str, result, str_of};
use gorse_expr::builtin;
use gorse_expr::conf::{Config, DEFAULT_MAX_NODES};
use gorse_expr::file::{Location, Source};
use gorse_expr::parser::lexer::{self, Kind, Token};
use serde_json::Value as Json;

fn golden() -> &'static Json {
    static GOLDEN: std::sync::OnceLock<Json> = std::sync::OnceLock::new();
    GOLDEN.get_or_init(|| load("parser_golden.json"))
}

#[test]
fn parser_golden_builtins_and_budget() {
    let names: Vec<&str> = array(&golden()["builtins"]).iter().map(str_of).collect();
    assert_eq!(names, builtin::NAMES);
    for (i, name) in builtin::NAMES.iter().enumerate() {
        assert_eq!(builtin::index(name), Some(i));
    }
    assert_eq!(builtin::index("nope"), None);
    assert_eq!(
        golden()["default_max_nodes"].as_u64(),
        Some(DEFAULT_MAX_NODES)
    );
}

/// `Token.String` of Bytes tokens holding arbitrary bytes: `strconv.Quote`,
/// which every error message and printed string uses.
#[test]
fn parser_golden_quote() {
    let cases = array(&golden()["quote"]);
    assert!(cases.len() > 4000);
    for case in cases {
        let value = hex::decode(str_of(&case[0])).expect("hex");
        let token = Token {
            location: Location::default(),
            kind: Kind::Bytes,
            value,
        };
        assert_eq!(token.to_string(), str_of(&case[1]), "{}", str_of(&case[0]));
    }
}

fn check_lex(input: &str, want: &Json) {
    match lexer::lex(&Source::new(input)) {
        Err(err) => assert_eq!(
            Some(err.to_string().as_str()),
            opt_str(&want["error"]),
            "lex {input:?}"
        ),
        Ok(tokens) => {
            assert!(
                want["error"].is_null(),
                "lex {input:?}: Go fails: {}",
                want["error"]
            );
            let got: Vec<Json> = tokens
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "kind": t.kind.as_str(),
                        "value": hex::encode(&t.value),
                        "from": t.location.from,
                        "to": t.location.to,
                        "string": t.to_string(),
                    })
                })
                .collect();
            assert_eq!(&got, array(&want["tokens"]), "lex {input:?}");
        }
    }
}

/// Every curated case in full: the error, or the dump, the printed tree and
/// the walk; and the tokens.
#[test]
fn parser_golden_cases() {
    let cases = array(&golden()["cases"]);
    assert_eq!(cases.len(), 746);
    assert_eq!(
        golden()["skipped_invalid_utf8"],
        serde_json::json!({"parser/lexer/lexer_test.go": 1})
    );
    let mut accepted = 0;
    for case in cases {
        let input = str_of(&case["input"]);
        assert!(case["panic"].is_null(), "Go panics on {input:?}");
        let r = result(input, None, true);
        assert_eq!(
            r.error.as_deref(),
            opt_str(&case["error"]),
            "error of {input:?}"
        );
        assert_eq!(
            r.dump.as_deref(),
            opt_str(&case["dump"]),
            "dump of {input:?}"
        );
        assert_eq!(
            r.string.as_deref(),
            opt_str(&case["string"]),
            "string of {input:?}"
        );
        assert_eq!(
            r.walk.as_deref(),
            opt_str(&case["walk"]),
            "walk of {input:?}"
        );
        check_lex(input, &case["lex"]);
        accepted += usize::from(r.error.is_none());
    }
    // Both verdicts are well represented.
    assert!(accepted > 300 && cases.len() - accepted > 100, "{accepted}");
}

/// The cases parsed with configs: node budgets, the if operator off,
/// disabled builtins, and functions or environment names overriding them.
#[test]
fn parser_golden_config() {
    let configs: Vec<Config> = array(&golden()["configs"])
        .iter()
        .map(config_from_json)
        .collect();
    assert_eq!(configs.len(), 11);
    let cases = array(&golden()["config"]);
    assert_eq!(cases.len(), 11 * 623);
    let mut seen = vec![[0usize; 2]; configs.len()];
    for case in cases {
        let i = usize::try_from(case[0].as_u64().expect("index")).expect("index");
        let input = str_of(&case[1]);
        let r = result(input, Some(&configs[i]), true);
        let accepted = r.error.is_none();
        assert_eq!(
            Some(accepted),
            case[2].as_bool(),
            "config {i}, {input:?}: {:?}",
            r.error
        );
        let d = digest(&[
            r.error.as_deref(),
            None,
            r.dump.as_deref(),
            r.string.as_deref(),
            r.walk.as_deref(),
        ]);
        assert_eq!(d, str_of(&case[3]), "config {i}, {input:?}: {r:?}");
        seen[i][usize::from(accepted)] += 1;
    }
    // Each config accepts some cases and rejects others.
    for (i, [rejected, accepted]) in seen.iter().enumerate() {
        assert!(
            *rejected > 0 && *accepted > 0,
            "config {i}: {rejected} rejected, {accepted} accepted"
        );
    }
}

/// The generator's `synthetic`.
fn synthetic(family: &str, n: usize) -> String {
    match family {
        "paren" => "(".repeat(n) + "a" + &")".repeat(n),
        "unary" => "-".repeat(n) + "1",
        "not" => "not ".repeat(n) + "a",
        "plus" => "1".to_owned() + &"+1".repeat(n),
        "array" => "[".to_owned() + "1,".repeat(n).trim_end_matches(',') + "]",
        "nested_array" => "[".repeat(n) + &"]".repeat(n),
        "member" => "a".to_owned() + &".b".repeat(n),
        "call" => "f(".repeat(n) + "1" + &")".repeat(n),
        "ternary" => "a?b:".repeat(n) + "c",
        "sequence" => "1;".repeat(n) + "1",
        "pipe" => "a".to_owned() + &" | f()".repeat(n),
        "identifier" => "a".repeat(n),
        "string" => "'".to_owned() + &"x".repeat(n) + "'",
        "comment" => "/*".to_owned() + &"*".repeat(n) + "*/1",
        // TestNodeBudgetDisabled
        "repeat_plus" => "a + ".repeat(n) + "b",
        _ => panic!("unknown family {family}"),
    }
}

/// Deep nesting, long chains on both sides of the node budget, and long
/// tokens.
#[test]
fn parser_golden_synthetic() {
    let cases = array(&golden()["synthetic"]);
    assert_eq!(cases.len(), 30);
    for case in cases {
        let family = str_of(&case["family"]);
        let count = usize::try_from(case["count"].as_u64().expect("count")).expect("count");
        let input = synthetic(family, count);
        assert!(case["panic"].is_null());
        let r = result(&input, None, false);
        let line = r
            .error
            .as_deref()
            .map(|e| e.split('\n').next().unwrap_or_default());
        assert_eq!(line, opt_str(&case["error_line"]), "{family} {count}");
        let lex = lex_text(&input);
        let d = digest(&[
            r.error.as_deref(),
            None,
            r.string.as_deref(),
            r.walk.as_deref(),
            Some(&lex),
        ]);
        assert_eq!(d, str_of(&case["digest"]), "{family} {count}");
    }
}
