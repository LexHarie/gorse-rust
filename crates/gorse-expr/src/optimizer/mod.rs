// Ports: github.com/expr-lang/expr/optimizer/optimizer.go @ v1.17.8, with the
// Gorse patches of sum_range.go and count_threshold.go

//! The optimizer: rewrites a checked tree into one that computes the same
//! values with less work, folding constants and turning common builtin shapes
//! into cheaper ones.
//!
//! Go's `Optimize` also runs `const_expr`, which calls the functions of
//! `config.ConstFns`. Gorse configures none, and Go skips that pass with none,
//! so it is not ported.

mod fold;
mod rewrite;

use crate::ast::{self, Node, NodeKind, walk};
use crate::file::{Error, Source};
use crate::nature::{self, Nature};
use crate::ty::Ty;
use crate::value::Value;

use fold::Fold;
use rewrite::{
    CountAny, CountThreshold, FilterFirst, FilterLast, FilterLen, FilterMap, InArray, InRange,
    PredicateCombination, SumArray, SumMap, SumRange,
};

/// `Optimize`: rewrites `node` in place. An error is a compile error of the
/// expression, such as `integer divide by zero`, bound to `source`.
pub fn optimize(node: &mut Node, source: &Source) -> Result<(), Error> {
    walk(node, &mut InArray);
    // Folding runs until no rule applies, and at most 1000 times over.
    for _ in 0..=1000 {
        let mut fold = Fold::default();
        walk(node, &mut fold);
        if let Some(err) = fold.err {
            return Err(err.bind(source));
        }
        if !fold.applied {
            break;
        }
    }
    walk(node, &mut InRange);
    walk(node, &mut FilterMap);
    walk(node, &mut FilterLen);
    walk(node, &mut FilterLast);
    walk(node, &mut FilterFirst);
    walk(node, &mut PredicateCombination);
    walk(node, &mut SumRange);
    walk(node, &mut SumArray);
    walk(node, &mut SumMap);
    walk(node, &mut CountAny);
    walk(node, &mut CountThreshold);
    Ok(())
}

/// A replacement for a node, as the passes make them.
enum Rewrite {
    /// `patchWithType`: the node is `kind`, with the type of the constant.
    Typed(NodeKind),
    /// `patchCopyType`: the node is `kind`, with the type it had.
    Copy(NodeKind),
    /// `patchCopy` of a child: the node becomes the child, with its type.
    Child(Node),
}

/// Replaces `node` by what `rewrite` gives it.
fn apply(node: &mut Node, rewrite: Rewrite) {
    match rewrite {
        Rewrite::Typed(kind) => {
            let mut new = Node::new(kind);
            new.nature = constant_nature(&new.kind);
            ast::patch(node, new);
        }
        Rewrite::Copy(kind) => {
            let mut new = Node::new(kind);
            new.nature = node.nature.clone();
            ast::patch(node, new);
        }
        Rewrite::Child(mut child) => {
            child.nature = node.nature.clone();
            ast::patch(node, child);
        }
    }
}

/// The nature of a constant node: its type is the Go type of its value.
fn constant_nature(kind: &NodeKind) -> Nature {
    let ty = match kind {
        NodeKind::Integer { .. } => Ty::int(),
        NodeKind::Float { .. } => Ty::float64(),
        NodeKind::Bool { .. } => Ty::bool(),
        NodeKind::String { .. } => Ty::string(),
        NodeKind::Constant { value } => match value.ty() {
            Some(ty) => ty,
            None => return nature::nil(),
        },
        _ => return Nature::default(),
    };
    nature::from_type(&ty)
}

/// A constant node holding `value`, with the type of the value.
fn constant(value: Value) -> Node {
    let mut node = Node::new(NodeKind::Constant { value });
    node.nature = constant_nature(&node.kind);
    node
}

/// A builtin call with no optimizer fields set.
fn builtin(name: &str, arguments: Vec<Node>) -> NodeKind {
    NodeKind::Builtin {
        name: name.to_owned(),
        arguments,
        throws: false,
        map: None,
        threshold: None,
    }
}

/// The value of an integer literal.
fn integer(node: &Node) -> Option<i64> {
    match node.kind {
        NodeKind::Integer { value } => Some(value),
        _ => None,
    }
}

/// The value of a float literal.
fn float(node: &Node) -> Option<f64> {
    match node.kind {
        NodeKind::Float { value } => Some(value),
        _ => None,
    }
}

/// The bytes of a string literal.
fn string(node: &Node) -> Option<&[u8]> {
    match &node.kind {
        NodeKind::String { value } => Some(value),
        _ => None,
    }
}

/// The value of a bool literal.
fn boolean(node: &Node) -> Option<bool> {
    match node.kind {
        NodeKind::Bool { value } => Some(value),
        _ => None,
    }
}

/// Whether `node` is the literal `value` of an integer.
fn is_integer(node: &Node, value: i64) -> bool {
    integer(node) == Some(value)
}

/// Whether `node` is a literal `value` of the form `#`, the element of a
/// predicate with no name.
fn is_plain_pointer(node: &Node) -> bool {
    matches!(&node.kind, NodeKind::Pointer { name } if name.is_empty())
}
