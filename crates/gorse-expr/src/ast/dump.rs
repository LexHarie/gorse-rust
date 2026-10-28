// Ports: github.com/expr-lang/expr/ast/dump.go @ v1.17.8

//! `ast.Dump`: every exported field of every node, in Go's struct order, as
//! Go's reflection-driven dumper writes them.

use std::fmt::Write as _;

use super::{Node, NodeKind, with_stack};
use crate::gostr;

/// The tree as `ast.Dump` writes it.
pub fn dump(node: &Node) -> String {
    let mut out = String::new();
    dump_node(&mut out, node, "");
    out
}

/// A field value, as `dump` meets it through reflection.
enum Value<'a> {
    Node(&'a Node),
    OptionalNode(Option<&'a Node>),
    Nodes(&'a [Node]),
    Bytes(&'a [u8]),
    Str(&'a [u8]),
    Int(i64),
    OptionalInt(Option<i64>),
    Float(f64),
    Bool(bool),
}

fn fields(kind: &NodeKind) -> Vec<(&'static str, Value<'_>)> {
    use Value as V;
    match kind {
        NodeKind::Nil => vec![],
        NodeKind::Identifier { value } => vec![("Value", V::Str(value.as_bytes()))],
        NodeKind::Integer { value } => vec![("Value", V::Int(*value))],
        NodeKind::Float { value } => vec![("Value", V::Float(*value))],
        NodeKind::Bool { value } => vec![("Value", V::Bool(*value))],
        NodeKind::String { value } => vec![("Value", V::Str(value))],
        NodeKind::Bytes { value } => vec![("Value", V::Bytes(value))],
        NodeKind::Unary { operator, node } => vec![
            ("Operator", V::Str(operator.as_bytes())),
            ("Node", V::Node(node)),
        ],
        NodeKind::Binary {
            operator,
            left,
            right,
        } => vec![
            ("Operator", V::Str(operator.as_bytes())),
            ("Left", V::Node(left)),
            ("Right", V::Node(right)),
        ],
        NodeKind::Chain { node } | NodeKind::Predicate { node } => vec![("Node", V::Node(node))],
        NodeKind::Member {
            node,
            property,
            optional,
            method,
        } => vec![
            ("Node", V::Node(node)),
            ("Property", V::Node(property)),
            ("Optional", V::Bool(*optional)),
            ("Method", V::Bool(*method)),
        ],
        NodeKind::Slice { node, from, to } => vec![
            ("Node", V::Node(node)),
            ("From", V::OptionalNode(from.as_deref())),
            ("To", V::OptionalNode(to.as_deref())),
        ],
        NodeKind::Call { callee, arguments } => vec![
            ("Callee", V::Node(callee)),
            ("Arguments", V::Nodes(arguments)),
        ],
        NodeKind::Builtin {
            name,
            arguments,
            throws,
            map,
            threshold,
        } => vec![
            ("Name", V::Str(name.as_bytes())),
            ("Arguments", V::Nodes(arguments)),
            ("Throws", V::Bool(*throws)),
            ("Map", V::OptionalNode(map.as_deref())),
            ("Threshold", V::OptionalInt(*threshold)),
        ],
        NodeKind::Pointer { name } => vec![("Name", V::Str(name.as_bytes()))],
        NodeKind::Conditional {
            ternary,
            cond,
            exp1,
            exp2,
        } => vec![
            ("Ternary", V::Bool(*ternary)),
            ("Cond", V::Node(cond)),
            ("Exp1", V::Node(exp1)),
            ("Exp2", V::Node(exp2)),
        ],
        NodeKind::VariableDeclarator { name, value, expr } => vec![
            ("Name", V::Str(name.as_bytes())),
            ("Value", V::Node(value)),
            ("Expr", V::Node(expr)),
        ],
        NodeKind::Sequence { nodes } | NodeKind::Array { nodes } => {
            vec![("Nodes", V::Nodes(nodes))]
        }
        NodeKind::Map { pairs } => vec![("Pairs", V::Nodes(pairs))],
        NodeKind::Pair { key, value } => vec![("Key", V::Node(key)), ("Value", V::Node(value))],
        NodeKind::Constant { .. } => Vec::new(),
    }
}

fn dump_node(out: &mut String, node: &Node, ident: &str) {
    with_stack(|| {
        out.push_str(node.kind.type_name());
        out.push_str("{\n");
        let inner = format!("{ident}\t");
        for (name, value) in fields(&node.kind) {
            out.push_str(&inner);
            out.push_str(name);
            out.push_str(": ");
            dump_value(out, &value, &inner);
            out.push_str(",\n");
        }
        out.push_str(ident);
        out.push('}');
    });
}

/// A slice: `<type>{}` when empty, else one element per line.
fn dump_slice<T>(
    out: &mut String,
    type_name: &str,
    items: &[T],
    ident: &str,
    mut item: impl FnMut(&mut String, &T, &str),
) {
    out.push_str(type_name);
    if items.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    let inner = format!("{ident}\t");
    for (i, x) in items.iter().enumerate() {
        out.push_str(&inner);
        item(out, x, &inner);
        out.push(',');
        if i + 1 < items.len() {
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(ident);
    out.push('}');
}

fn dump_value(out: &mut String, value: &Value<'_>, ident: &str) {
    match value {
        Value::Node(node) => dump_node(out, node, ident),
        Value::OptionalNode(Some(node)) => dump_node(out, node, ident),
        Value::OptionalNode(None) | Value::OptionalInt(None) => out.push_str("nil"),
        Value::Nodes(nodes) => dump_slice(out, "[]ast.Node", nodes, ident, |out, n, ident| {
            dump_node(out, n, ident);
        }),
        Value::Bytes(bytes) => dump_slice(out, "[]uint8", bytes, ident, |out, b, _| {
            let _ = write!(out, "{b}");
        }),
        Value::Str(s) => out.push_str(&gostr::quote(s)),
        Value::Int(n) | Value::OptionalInt(Some(n)) => {
            let _ = write!(out, "{n}");
        }
        Value::Float(v) => out.push_str(&gostr::format_float_v(*v)),
        Value::Bool(b) => {
            let _ = write!(out, "{b}");
        }
    }
}
