//! expr-lang's own parser, lexer and printer tests (v1.17.8), with the
//! cases and assertions of `parser/parser_test.go`,
//! `parser/lexer/lexer_test.go` and `ast/print_test.go`. The tables are read
//! from the Go source by `contracts/tools/exprparsegolden` into
//! `parser_go_tests.json`; the tests that build their input in code are
//! ported by hand. `TestPrint_ConstantNode` waits for `ConstantNode`
//! (WP-3-6).

mod common;

use common::{array, load, node_from_json, str_of};
use gorse_expr::ast::{self, Node, NodeKind};
use gorse_expr::conf::Config;
use gorse_expr::file::{Location, Source};
use gorse_expr::parser::{self, lexer};
use serde_json::Value as Json;

fn tables() -> &'static Json {
    static TABLES: std::sync::OnceLock<Json> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| load("parser_go_tests.json"))
}

fn table(key: &str) -> &'static [Json] {
    let rows = array(&tables()[key]);
    assert!(!rows.is_empty(), "{key} is empty");
    rows
}

#[test]
fn parser_go_tables_are_whole() {
    // The one row TestLex has that is not UTF-8, which the Rust lexer cannot
    // be given (it takes text).
    assert_eq!(
        tables()["skipped_invalid_utf8"],
        serde_json::json!({"lex": 1})
    );
    let counts: Vec<(&str, usize)> = [
        "parse",
        "parse_error",
        "optional_chaining",
        "node_budget",
        "lex",
        "print",
    ]
    .iter()
    .map(|k| (*k, table(k).len()))
    .collect();
    assert_eq!(
        counts,
        [
            ("parse", 105),
            ("parse_error", 27),
            ("optional_chaining", 6),
            ("node_budget", 5),
            ("lex", 29),
            ("print", 70),
        ]
    );
}

// TestParse
#[test]
fn parser_parse() {
    for test in table("parse") {
        let input = str_of(&test["input"]);
        let actual = parser::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(
            ast::dump(&node_from_json(&test["want"])),
            ast::dump(&actual.node),
            "{input}"
        );
    }
}

// TestParse_error
#[test]
fn parser_parse_error() {
    for test in table("parse_error") {
        let input = str_of(&test["input"]);
        let err = match parser::parse(input) {
            Ok(_) => "<nil>".to_owned(),
            Err(e) => e.to_string(),
        };
        assert_eq!(str_of(&test["err"]), err, "{input}");
    }
}

// TestParse_optional_chaining
#[test]
fn parser_parse_optional_chaining() {
    for test in table("optional_chaining") {
        let input = str_of(&test["input"]);
        let actual = parser::parse(input).unwrap_or_else(|e| panic!("{input}:\n{e}"));
        assert_eq!(
            ast::dump(&node_from_json(&test["expected"])),
            ast::dump(&actual.node),
            "{input}"
        );
    }
}

fn n(kind: NodeKind) -> Node {
    Node::new(kind)
}

fn ident(value: &str) -> Box<Node> {
    Box::new(n(NodeKind::Identifier {
        value: value.to_owned(),
    }))
}

fn builtin(name: &str, arguments: Vec<Node>) -> Node {
    n(NodeKind::Builtin {
        name: name.to_owned(),
        arguments,
        throws: false,
        map: None,
        threshold: None,
    })
}

// TestParse_pipe_operator
#[test]
fn parser_parse_pipe_operator() {
    let input = "arr | map(.foo) | len() | Foo()";
    let expect = n(NodeKind::Call {
        callee: ident("Foo"),
        arguments: vec![builtin(
            "len",
            vec![builtin(
                "map",
                vec![
                    *ident("arr"),
                    n(NodeKind::Predicate {
                        node: Box::new(n(NodeKind::Member {
                            node: Box::new(n(NodeKind::Pointer {
                                name: String::new(),
                            })),
                            property: Box::new(n(NodeKind::String {
                                value: b"foo".to_vec(),
                            })),
                            optional: false,
                            method: false,
                        })),
                    }),
                ],
            )],
        )],
    });

    let actual = parser::parse(input).expect("no error");
    assert_eq!(ast::dump(&expect), ast::dump(&actual.node));
}

// TestNodeBudget
#[test]
fn parser_node_budget() {
    for tt in table("node_budget") {
        let name = str_of(&tt["name"]);
        let expr = str_of(&tt["expr"]);
        let mut config = Config::create_new();
        config.max_nodes = tt["maxNodes"].as_u64().expect("maxNodes");
        config.disabled.clear();

        let err = parser::parse_with_config(expr, Some(&config)).err();
        let has_error = err
            .as_ref()
            .is_some_and(|e| e.to_string().contains("exceeds maximum allowed nodes"));
        let should_error = tt["shouldError"].as_bool().expect("shouldError");

        assert_eq!(
            has_error, should_error,
            "{name}: ParseWithConfig({expr:?}) error = {err:?}"
        );

        // Verify error message format when expected
        if should_error && let Some(err) = err {
            let expected = "compilation failed: expression exceeds maximum allowed nodes";
            assert!(
                err.to_string().contains(expected),
                "Expected error message to contain {expected:?}, got {:?}",
                err.to_string()
            );
        }
    }
}

// TestNodeBudgetDisabled
#[test]
fn parser_node_budget_disabled() {
    let mut config = Config::create_new();
    config.max_nodes = 0; // Disable node budget

    let expr = "a + ".repeat(1000) + "b";
    let err = parser::parse_with_config(&expr, Some(&config)).err();

    assert!(
        !err.is_some_and(|e| e.to_string().contains("exceeds maximum allowed nodes")),
        "Node budget check should be disabled when MaxNodes is 0"
    );
}

// TestLex: the kind and value of each token (compareTokens).
#[test]
fn parser_lex() {
    for test in table("lex") {
        let input = str_of(&test["input"]);
        let tokens = lexer::lex(&Source::new(input)).unwrap_or_else(|e| panic!("{input}:\n{e}"));
        let got: Vec<(String, String)> = tokens
            .iter()
            .map(|t| (t.kind.to_string(), hex::encode(&t.value)))
            .collect();
        let want: Vec<(String, String)> = array(&test["tokens"])
            .iter()
            .map(|t| {
                (
                    str_of(&t["Kind"]).to_owned(),
                    str_of(&t["Value"]).to_owned(),
                )
            })
            .collect();
        assert_eq!(got, want, "{input}");
    }
}

// TestLex_location
#[test]
fn parser_lex_location() {
    let source = Source::new("1..2\n3..4");
    let tokens = lexer::lex(&source).expect("no error");
    let token = |from, to, kind, value: &str| lexer::Token {
        location: Location { from, to },
        kind,
        value: value.as_bytes().to_vec(),
    };
    use lexer::Kind::{Eof, Number, Operator};
    assert_eq!(
        tokens,
        [
            token(0, 1, Number, "1"),
            token(1, 3, Operator, ".."),
            token(3, 4, Number, "2"),
            token(5, 6, Number, "3"),
            token(6, 8, Operator, ".."),
            token(8, 9, Number, "4"),
            token(8, 9, Eof, ""),
        ]
    );
}

// TestLex_error
#[test]
fn parser_lex_error() {
    let error_tests = str_of(&tables()["lex_error"]);
    let tests: Vec<&str> = error_tests.trim_matches('\n').split("\n\n").collect();
    assert_eq!(tests.len(), 9);

    for test in tests {
        let Some((input, want)) = test.split_once('\n') else {
            panic!("syntax error in test: {test:?}");
        };
        let err = match lexer::lex(&Source::new(input)) {
            Ok(_) => "<nil>".to_owned(),
            Err(e) => e.to_string(),
        };
        assert_eq!(want, err, "{input}");
    }
}

// TestPrint
#[test]
fn parser_print() {
    for tt in table("print") {
        let input = str_of(&tt["input"]);
        let tree = parser::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(str_of(&tt["want"]), tree.node.to_string(), "{input}");
    }
}

// TestPrint_MemberNode
#[test]
fn parser_print_member_node() {
    let node = n(NodeKind::Member {
        node: ident("a"),
        property: Box::new(n(NodeKind::String {
            value: b"b c".to_vec(),
        })),
        optional: true,
        method: false,
    });
    assert_eq!(r#"a?.["b c"]"#, node.to_string());
}
