// Ports: github.com/expr-lang/expr/optimizer/fold.go @ v1.17.8

//! Constant folding: an operator, a builtin or an array on constants is
//! replaced by the value it computes.

use std::rc::Rc;

use crate::ast::{Node, NodeKind, Visitor};
use crate::file::Error;
use crate::value::Value;

use super::{Rewrite, apply, boolean, float, integer, string};

/// `fold`: shows every node the folding rules, and records whether one
/// applied, so the caller can run the rules again on the rewritten tree.
#[derive(Default)]
pub struct Fold {
    /// Whether a rule rewrote a node.
    pub applied: bool,
    /// The compile error of a fold, such as an integer division by zero.
    pub err: Option<Error>,
}

impl Visitor for Fold {
    fn visit(&mut self, node: &mut Node) {
        match rule(node) {
            Ok(Some(rewrite)) => {
                self.applied = true;
                apply(node, rewrite);
            }
            Ok(None) => {}
            Err(err) => self.err = Some(err),
        }
    }
}

/// The folding rule that applies to `node`, if one does.
fn rule(node: &Node) -> Result<Option<Rewrite>, Error> {
    Ok(match &node.kind {
        NodeKind::Unary {
            operator,
            node: operand,
        } => unary(operator, operand),
        NodeKind::Binary {
            operator,
            left,
            right,
        } => return binary(node, operator, left, right),
        NodeKind::Array { nodes } => array(nodes),
        NodeKind::Builtin {
            name, arguments, ..
        } => filter_of_filter(name, arguments),
        _ => None,
    })
}

fn unary(operator: &str, operand: &Node) -> Option<Rewrite> {
    match operator {
        "-" => match operand.kind {
            NodeKind::Integer { value } => Some(Rewrite::Typed(NodeKind::Integer {
                value: value.wrapping_neg(),
            })),
            NodeKind::Float { value } => Some(Rewrite::Typed(NodeKind::Float { value: -value })),
            _ => None,
        },
        "+" => match operand.kind {
            NodeKind::Integer { value } => Some(Rewrite::Typed(NodeKind::Integer { value })),
            NodeKind::Float { value } => Some(Rewrite::Typed(NodeKind::Float { value })),
            _ => None,
        },
        "!" | "not" => {
            boolean(operand).map(|value| Rewrite::Typed(NodeKind::Bool { value: !value }))
        }
        _ => None,
    }
}

/// Folds a binary operator. Only `%` can fail: an integer divided by zero.
fn binary(
    node: &Node,
    operator: &str,
    left: &Node,
    right: &Node,
) -> Result<Option<Rewrite>, Error> {
    let rewrite = match operator {
        "+" => number_op(left, right, Some(i64::wrapping_add), |a, b| a + b).or_else(|| {
            let (a, b) = (string(left)?, string(right)?);
            Some(NodeKind::String {
                value: [a, b].concat(),
            })
        }),
        "-" => number_op(left, right, Some(i64::wrapping_sub), |a, b| a - b),
        "*" => number_op(left, right, Some(i64::wrapping_mul), |a, b| a * b),
        "/" => number_op(left, right, None, |a, b| a / b),
        "%" => {
            if let (Some(a), Some(b)) = (integer(left), integer(right)) {
                if b == 0 {
                    return Err(Error::new(node.location, "integer divide by zero"));
                }
                Some(NodeKind::Integer {
                    value: a.wrapping_rem(b),
                })
            } else {
                None
            }
        }
        "**" | "^" => number_op(left, right, None, f64::powf),
        "and" | "&&" => {
            let (a, b) = (boolean(left), boolean(right));
            if a == Some(true) {
                return Ok(Some(Rewrite::Child(right.clone())));
            }
            if b == Some(true) {
                return Ok(Some(Rewrite::Child(left.clone())));
            }
            if a == Some(false) || b == Some(false) {
                Some(NodeKind::Bool { value: false })
            } else {
                None
            }
        }
        "or" | "||" => {
            let (a, b) = (boolean(left), boolean(right));
            if a == Some(false) {
                return Ok(Some(Rewrite::Child(right.clone())));
            }
            if b == Some(false) {
                return Ok(Some(Rewrite::Child(left.clone())));
            }
            if a == Some(true) || b == Some(true) {
                Some(NodeKind::Bool { value: true })
            } else {
                None
            }
        }
        "==" => {
            if let (Some(a), Some(b)) = (integer(left), integer(right)) {
                Some(NodeKind::Bool { value: a == b })
            } else if let (Some(a), Some(b)) = (string(left), string(right)) {
                Some(NodeKind::Bool { value: a == b })
            } else if let (Some(a), Some(b)) = (boolean(left), boolean(right)) {
                Some(NodeKind::Bool { value: a == b })
            } else {
                None
            }
        }
        _ => None,
    };
    Ok(rewrite.map(Rewrite::Typed))
}

/// Folds an operator on two numbers. Two integers give an integer through
/// `ints`, or a float through `floats` when `ints` is none (for `/`). Any
/// other pair of numbers gives a float.
fn number_op(
    left: &Node,
    right: &Node,
    ints: Option<fn(i64, i64) -> i64>,
    floats: fn(f64, f64) -> f64,
) -> Option<NodeKind> {
    if let (Some(a), Some(b)) = (integer(left), integer(right)) {
        return Some(match ints {
            Some(op) => NodeKind::Integer { value: op(a, b) },
            None => NodeKind::Float {
                value: floats(a as f64, b as f64),
            },
        });
    }
    let (a, b) = (number(left)?, number(right)?);
    Some(NodeKind::Float {
        value: floats(a, b),
    })
}

/// The value of a number literal, as a float when it is one.
fn number(node: &Node) -> Option<f64> {
    integer(node)
        .map(|value| value as f64)
        .or_else(|| float(node))
}

/// An array of literals is a constant slice of their values.
fn array(nodes: &[Node]) -> Option<Rewrite> {
    if nodes.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(nodes.len());
    for node in nodes {
        items.push(match &node.kind {
            NodeKind::Integer { value } => Value::Int(*value),
            NodeKind::Float { value } => Value::Float(*value),
            NodeKind::String { value } => Value::Str(Rc::from(value.as_slice())),
            NodeKind::Bool { value } => Value::Bool(*value),
            _ => return None,
        });
    }
    Some(Rewrite::Typed(NodeKind::Constant {
        value: Value::any_array(items),
    }))
}

/// `filter(filter(xs, p), q)` is `filter(xs, p && q)`.
fn filter_of_filter(name: &str, arguments: &[Node]) -> Option<Rewrite> {
    if name != "filter" || arguments.len() != 2 {
        return None;
    }
    let NodeKind::Builtin {
        name: base_name,
        arguments: base,
        ..
    } = &arguments[0].kind
    else {
        return None;
    };
    if base_name != "filter" || base.len() != 2 {
        return None;
    }
    let (NodeKind::Predicate { node: inner }, NodeKind::Predicate { node: outer }) =
        (&base[1].kind, &arguments[1].kind)
    else {
        return None;
    };
    let both = Node::new(NodeKind::Binary {
        operator: "&&".to_owned(),
        left: inner.clone(),
        right: outer.clone(),
    });
    let predicate = Node::new(NodeKind::Predicate {
        node: Box::new(both),
    });
    Some(Rewrite::Copy(super::builtin(
        "filter",
        vec![base[0].clone(), predicate],
    )))
}
