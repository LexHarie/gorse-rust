// Ports: github.com/expr-lang/expr/ast/print.go @ v1.17.8

//! `Node.String`: the tree printed back as an expression, with parentheses
//! only where precedence needs them.

use std::fmt::{self, Display, Write as _};

use super::{Node, NodeKind, with_stack};
use crate::gostr;
use crate::parser::operator::{self, Associativity};
use crate::parser::utils;

fn is_valid_identifier_bytes(s: &[u8]) -> bool {
    // Go decodes the bytes; one that is not UTF-8 decodes to U+FFFD, which
    // is neither a letter nor a digit.
    std::str::from_utf8(s).is_ok_and(utils::is_valid_identifier)
}

fn join(f: &mut fmt::Formatter<'_>, nodes: &[Node]) -> fmt::Result {
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{node}")?;
    }
    Ok(())
}

impl Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        with_stack(|| self.kind.fmt(f))
    }
}

impl Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("nil"),
            Self::Identifier { value } => f.write_str(value),
            Self::Integer { value } => write!(f, "{value}"),
            Self::Float { value } => f.write_str(&gostr::format_float_v(*value)),
            Self::Bool { value } => write!(f, "{value}"),
            Self::String { value } => f.write_str(&gostr::quote(value)),
            Self::Bytes { value } => write!(f, "b{}", gostr::quote(value)),
            Self::Unary { operator, node } => {
                let op = if operator == "not" { "not " } else { operator };
                let wrap = match &node.kind {
                    Self::Binary { operator: b, .. } => {
                        operator::binary(b).precedence < operator::unary(operator).precedence
                    }
                    Self::Conditional { .. } => true,
                    _ => false,
                };
                if wrap {
                    write!(f, "{op}({node})")
                } else {
                    write!(f, "{op}{node}")
                }
            }
            Self::Binary {
                operator,
                left,
                right,
            } => {
                if operator == ".." {
                    return write!(f, "{left}..{right}");
                }
                let mut lwrap = false;
                let mut rwrap = false;
                if let Self::Unary { operator: l, .. } = &left.kind
                    && operator::unary(l).precedence < operator::binary(operator).precedence
                {
                    lwrap = true;
                }
                if let Self::Binary { operator: lb, .. } = &left.kind {
                    if operator::less(lb, operator) {
                        lwrap = true;
                    }
                    if operator::binary(lb).precedence == operator::binary(operator).precedence
                        && operator::binary(operator).associativity == Associativity::Right
                    {
                        lwrap = true;
                    }
                    if lb == "??" {
                        lwrap = true;
                    }
                    if operator::is_boolean(lb) && operator != lb {
                        lwrap = true;
                    }
                }
                if let Self::Binary { operator: rb, .. } = &right.kind {
                    if operator::less(rb, operator) {
                        rwrap = true;
                    }
                    if operator::binary(rb).precedence == operator::binary(operator).precedence
                        && operator::binary(operator).associativity == Associativity::Left
                    {
                        rwrap = true;
                    }
                    if operator::is_boolean(rb) && operator != rb {
                        rwrap = true;
                    }
                }
                if matches!(left.kind, Self::Conditional { .. }) {
                    lwrap = true;
                }
                if matches!(right.kind, Self::Conditional { .. }) {
                    rwrap = true;
                }
                if lwrap {
                    write!(f, "({left})")?;
                } else {
                    write!(f, "{left}")?;
                }
                write!(f, " {operator} ")?;
                if rwrap {
                    write!(f, "({right})")
                } else {
                    write!(f, "{right}")
                }
            }
            Self::Chain { node } | Self::Predicate { node } => write!(f, "{node}"),
            Self::Member {
                node,
                property,
                optional,
                ..
            } => {
                let mut shown = node.to_string();
                if matches!(node.kind, Self::Binary { .. }) {
                    shown = format!("({shown})");
                }
                let name = match &property.kind {
                    Self::String { value } if is_valid_identifier_bytes(value) => Some(value),
                    _ => None,
                };
                if *optional {
                    return match name {
                        Some(name) => {
                            write!(f, "{shown}?.")?;
                            f.write_str(&String::from_utf8_lossy(name))
                        }
                        None => write!(f, "{shown}?.[{property}]"),
                    };
                }
                match name {
                    Some(name) => {
                        if !matches!(node.kind, Self::Pointer { .. }) {
                            f.write_str(&shown)?;
                        }
                        f.write_char('.')?;
                        // A valid identifier is UTF-8.
                        f.write_str(&String::from_utf8_lossy(name))
                    }
                    None => write!(f, "{shown}[{property}]"),
                }
            }
            Self::Slice { node, from, to } => match (from, to) {
                (None, None) => write!(f, "{node}[:]"),
                (None, Some(to)) => write!(f, "{node}[:{to}]"),
                (Some(from), None) => write!(f, "{node}[{from}:]"),
                (Some(from), Some(to)) => write!(f, "{node}[{from}:{to}]"),
            },
            Self::Call { callee, arguments } => {
                write!(f, "{callee}(")?;
                join(f, arguments)?;
                f.write_char(')')
            }
            Self::Builtin {
                name, arguments, ..
            } => {
                write!(f, "{name}(")?;
                join(f, arguments)?;
                f.write_char(')')
            }
            Self::Pointer { name } => write!(f, "#{name}"),
            Self::VariableDeclarator { name, value, expr } => {
                write!(f, "let {name} = {value}; {expr}")
            }
            Self::Sequence { nodes } => {
                for (i, node) in nodes.iter().enumerate() {
                    if i > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{node}")?;
                }
                Ok(())
            }
            Self::Conditional {
                ternary,
                cond,
                exp1,
                exp2,
            } => {
                if !ternary {
                    if let Self::Conditional { ternary: false, .. } = exp2.kind {
                        return write!(f, "if {cond} {{ {exp1} }} else {exp2}");
                    }
                    return write!(f, "if {cond} {{ {exp1} }} else {{ {exp2} }}");
                }
                let part = |f: &mut fmt::Formatter<'_>, n: &Node| {
                    if matches!(n.kind, Self::Conditional { .. }) {
                        write!(f, "({n})")
                    } else {
                        write!(f, "{n}")
                    }
                };
                part(f, cond)?;
                f.write_str(" ? ")?;
                part(f, exp1)?;
                f.write_str(" : ")?;
                part(f, exp2)
            }
            Self::Array { nodes } => {
                f.write_char('[')?;
                join(f, nodes)?;
                f.write_char(']')
            }
            Self::Map { pairs } => {
                f.write_char('{')?;
                join(f, pairs)?;
                f.write_char('}')
            }
            Self::Pair { key, value } => match &key.kind {
                Self::String { value: k } if is_valid_identifier_bytes(k) => {
                    f.write_str(&String::from_utf8_lossy(k))?;
                    write!(f, ": {value}")
                }
                Self::String { .. } => write!(f, "{key}: {value}"),
                _ => write!(f, "({key}): {value}"),
            },
        }
    }
}
