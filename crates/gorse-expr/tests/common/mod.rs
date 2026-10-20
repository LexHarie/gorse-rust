//! What the parser tests share: reading the Go data, building trees and
//! configs from it, and recording a parse the way the Go generator
//! (`contracts/tools/exprparsegolden`) does.

#![allow(dead_code)]

use gorse_expr::ast::{self, Node, NodeKind, Visitor};
use gorse_expr::conf::Config;
use gorse_expr::file::Source;
use gorse_expr::parser::{self, lexer};
use serde_json::Value as Json;
use sha2::{Digest as _, Sha256};

pub fn load(name: &str) -> Json {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

pub fn str_of(v: &Json) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

pub fn opt_str(v: &Json) -> Option<&str> {
    if v.is_null() { None } else { Some(str_of(v)) }
}

pub fn array(v: &Json) -> &Vec<Json> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}"))
}

/// A tree written as the Go generator writes an `ast` composite literal:
/// `{"node": "XNode", "fields": {...}}` with the fields the literal sets;
/// the others take Go's zero values.
pub fn node_from_json(v: &Json) -> Node {
    let fields = &v["fields"];
    let field = |name: &str| fields.get(name).filter(|f| !f.is_null());
    let string = |name: &str| field(name).map_or(String::new(), |f| str_of(f).to_owned());
    let boolean = |name: &str| field(name).is_some_and(|f| f.as_bool().expect("bool"));
    let child = |name: &str| {
        Box::new(node_from_json(
            field(name).unwrap_or_else(|| panic!("{v}: no {name}")),
        ))
    };
    let optional = |name: &str| field(name).map(|f| Box::new(node_from_json(f)));
    let nodes = |name: &str| {
        field(name).map_or(Vec::new(), |f| {
            array(f).iter().map(node_from_json).collect()
        })
    };
    let kind = match str_of(&v["node"]) {
        "NilNode" => NodeKind::Nil,
        "IdentifierNode" => NodeKind::Identifier {
            value: string("Value"),
        },
        "IntegerNode" => NodeKind::Integer {
            value: field("Value").map_or(0, |f| f.as_i64().expect("int")),
        },
        "FloatNode" => NodeKind::Float {
            value: field("Value").map_or(0.0, |f| f.as_f64().expect("float")),
        },
        "BoolNode" => NodeKind::Bool {
            value: boolean("Value"),
        },
        "StringNode" => NodeKind::String {
            value: string("Value").into_bytes(),
        },
        "BytesNode" => NodeKind::Bytes {
            value: field("Value").map_or(Vec::new(), |f| {
                hex::decode(str_of(&f["bytes"])).expect("hex")
            }),
        },
        "UnaryNode" => NodeKind::Unary {
            operator: string("Operator"),
            node: child("Node"),
        },
        "BinaryNode" => NodeKind::Binary {
            operator: string("Operator"),
            left: child("Left"),
            right: child("Right"),
        },
        "ChainNode" => NodeKind::Chain {
            node: child("Node"),
        },
        "MemberNode" => NodeKind::Member {
            node: child("Node"),
            property: child("Property"),
            optional: boolean("Optional"),
            method: boolean("Method"),
        },
        "SliceNode" => NodeKind::Slice {
            node: child("Node"),
            from: optional("From"),
            to: optional("To"),
        },
        "CallNode" => NodeKind::Call {
            callee: child("Callee"),
            arguments: nodes("Arguments"),
        },
        "BuiltinNode" => NodeKind::Builtin {
            name: string("Name"),
            arguments: nodes("Arguments"),
            throws: boolean("Throws"),
            map: optional("Map"),
            threshold: field("Threshold").map(|f| f.as_i64().expect("int")),
        },
        "PredicateNode" => NodeKind::Predicate {
            node: child("Node"),
        },
        "PointerNode" => NodeKind::Pointer {
            name: string("Name"),
        },
        "ConditionalNode" => NodeKind::Conditional {
            ternary: boolean("Ternary"),
            cond: child("Cond"),
            exp1: child("Exp1"),
            exp2: child("Exp2"),
        },
        "VariableDeclaratorNode" => NodeKind::VariableDeclarator {
            name: string("Name"),
            value: child("Value"),
            expr: child("Expr"),
        },
        "SequenceNode" => NodeKind::Sequence {
            nodes: nodes("Nodes"),
        },
        "ArrayNode" => NodeKind::Array {
            nodes: nodes("Nodes"),
        },
        "MapNode" => NodeKind::Map {
            pairs: nodes("Pairs"),
        },
        "PairNode" => NodeKind::Pair {
            key: child("Key"),
            value: child("Value"),
        },
        other => panic!("unknown node type {other}"),
    };
    Node::new(kind)
}

/// `makeConfig` of the generator.
pub fn config_from_json(c: &Json) -> Config {
    if c["zero"].as_bool().expect("zero") {
        return Config::default();
    }
    let names = |key: &str| {
        c[key].as_array().map_or(Vec::new(), |a| {
            a.iter().map(|n| str_of(n).to_owned()).collect()
        })
    };
    let mut config = Config::create_new();
    config.env.extend(names("env"));
    config.max_nodes = c["max_nodes"].as_u64().expect("max_nodes");
    config.disable_if_operator = c["disable_if_operator"]
        .as_bool()
        .expect("disable_if_operator");
    config.disabled.extend(names("disabled"));
    config.functions.extend(names("functions"));
    config
}

struct Walker(String);

impl Visitor for Walker {
    fn visit(&mut self, node: &mut Node) {
        let loc = node.location;
        self.0.push_str(&format!(
            "{} {}:{}\n",
            node.kind.type_name(),
            loc.from,
            loc.to
        ));
    }
}

/// The nodes in walk order with their locations, as the generator's walker
/// writes them.
pub fn walk_text(node: &mut Node) -> String {
    let mut w = Walker(String::new());
    ast::walk(node, &mut w);
    w.0
}

/// What the generator's `result` records.
#[derive(Debug, Default, PartialEq)]
pub struct GoResult {
    pub error: Option<String>,
    pub dump: Option<String>,
    pub string: Option<String>,
    pub walk: Option<String>,
}

pub fn result(input: &str, config: Option<&Config>, with_dump: bool) -> GoResult {
    match parser::parse_with_config(input, config) {
        Err(err) => GoResult {
            error: Some(err.to_string()),
            ..GoResult::default()
        },
        Ok(mut tree) => GoResult {
            error: None,
            dump: with_dump.then(|| ast::dump(&tree.node)),
            string: Some(tree.node.to_string()),
            walk: Some(walk_text(&mut tree.node)),
        },
    }
}

/// The generator's `lexText`.
pub fn lex_text(input: &str) -> String {
    match lexer::lex(&Source::new(input)) {
        Err(err) => format!("error {err}"),
        Ok(tokens) => tokens
            .iter()
            .map(|t| {
                format!(
                    "{} {}:{} {} {}\n",
                    t.kind,
                    t.location.from,
                    t.location.to,
                    hex::encode(&t.value),
                    t
                )
            })
            .collect(),
    }
}

/// The generator's `digest`.
pub fn digest(parts: &[Option<&str>]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        match p {
            None => h.update(b"-\n"),
            Some(p) => h.update(format!("+{}:{p}\n", p.len()).as_bytes()),
        }
    }
    hex::encode(&h.finalize()[..8])
}

/// `fullDigest`: everything a full case records (no panic: a panic fails the
/// test instead).
pub fn full_digest(input: &str) -> (bool, String) {
    let r = result(input, None, true);
    let lex = lex_text(input);
    let d = digest(&[
        r.error.as_deref(),
        None,
        r.dump.as_deref(),
        r.string.as_deref(),
        r.walk.as_deref(),
        Some(&lex),
    ]);
    (r.error.is_none(), d)
}
