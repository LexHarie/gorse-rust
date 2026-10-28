// Ports: github.com/expr-lang/expr/ast/node.go @ v1.17.8

//! The syntax tree.
//!
//! Go has one struct per node type behind a `Node` interface, each embedding
//! the location; here a [`Node`] is the location and a [`NodeKind`] whose
//! variants are those structs, with the same fields. Go's `nil` children of
//! a failed parse never leave the parser, so children are not optional
//! except where Go leaves them unset in a finished tree (slice bounds, the
//! optimizer's builtin fields).
//!
//! The checker attaches what it knows about each node as its [`Nature`]; the
//! optimizer makes [`NodeKind::Constant`] nodes for folded values.

mod dump;
mod print;
mod visitor;

pub use dump::dump;
pub use visitor::{Visitor, find, walk};

use std::fmt;

use crate::file::Location;
use crate::nature::Nature;
use crate::value::Value;

/// A node of the tree: where in the source it comes from, and what it is.
///
/// Cloning, comparing, printing and dropping take any depth (see
/// [`Drop`](#impl-Drop-for-Node)).
pub struct Node {
    /// The span the node was made from, usually the token that decided it
    /// (the operator of a binary node, the name of a member, ...).
    pub location: Location,
    /// The node.
    pub kind: NodeKind,
    /// What the checker knows about the value of the node.
    pub nature: Nature,
}

/// What a node is: one variant per Go node struct, named without the `Node`
/// suffix.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// `nil`.
    Nil,
    /// A name, like `foo` in `foo.bar`.
    Identifier {
        /// The name.
        value: String,
    },
    /// An integer literal (Go `int`, 64 bits).
    Integer {
        /// The value.
        value: i64,
    },
    /// A float literal.
    Float {
        /// The value.
        value: f64,
    },
    /// `true` or `false`.
    Bool {
        /// The value.
        value: bool,
    },
    /// A string literal, a member name or a map key. A Go string is bytes:
    /// an escape like `\UFFFFFFFF` overflows Go's 32-bit runes and leaves a
    /// byte that is not UTF-8, so the value is bytes too.
    String {
        /// The bytes of the string.
        value: Vec<u8>,
    },
    /// A bytes literal, like `b"\xff"`.
    Bytes {
        /// The bytes.
        value: Vec<u8>,
    },
    /// A unary operator, like `!foo` or `not foo`.
    Unary {
        /// `not`, `!`, `-` or `+`.
        operator: String,
        /// The operand.
        node: Box<Node>,
    },
    /// A binary operator, like `foo + bar` or `foo matches bar`.
    Binary {
        /// The operator.
        operator: String,
        /// The left operand.
        left: Box<Node>,
        /// The right operand.
        right: Box<Node>,
    },
    /// The root of an optional chain like `foo?.bar.baz`.
    Chain {
        /// The chain.
        node: Box<Node>,
    },
    /// A member access, like `foo.bar`, `foo["bar"]` or `foo?.[0]`.
    Member {
        /// What is accessed, like `foo` in `foo.bar`.
        node: Box<Node>,
        /// The member: a [`NodeKind::String`] for a property.
        property: Box<Node>,
        /// `?.` access.
        optional: bool,
        /// The member is called, like `foo.bar()`.
        method: bool,
    },
    /// A slice, like `array[1:4]`.
    Slice {
        /// What is sliced.
        node: Box<Node>,
        /// The first index, like `1` in `array[1:4]`.
        from: Option<Box<Node>>,
        /// The index after the last, like `4` in `array[1:4]`.
        to: Option<Box<Node>>,
    },
    /// A call of a function or method that is not a builtin.
    Call {
        /// What is called.
        callee: Box<Node>,
        /// The arguments.
        arguments: Vec<Node>,
    },
    /// A call of a builtin function, like `len(foo)`.
    Builtin {
        /// The builtin.
        name: String,
        /// The arguments.
        arguments: Vec<Node>,
        /// Whether accessing a field or index can fail; set by the
        /// optimizer.
        throws: bool,
        /// A `map` folded into a `filter`; set by the optimizer.
        map: Option<Box<Node>>,
        /// Where `count` may stop early; set by the optimizer.
        threshold: Option<i64>,
    },
    /// The body of a predicate, like `# > 0` in `filter(a, # > 0)`.
    Predicate {
        /// The body.
        node: Box<Node>,
    },
    /// `#`, `#name`, or the implicit `#` of `.field`, in a predicate.
    Pointer {
        /// The name, like `index` in `#index`; empty for `#`.
        name: String,
    },
    /// `cond ? exp1 : exp2`, `cond ?: exp2`, or `if cond { exp1 } else { exp2 }`.
    Conditional {
        /// The ternary form rather than `if`/`else`.
        ternary: bool,
        /// The condition.
        cond: Box<Node>,
        /// The value when the condition holds.
        exp1: Box<Node>,
        /// The value otherwise.
        exp2: Box<Node>,
    },
    /// `let name = value; expr`.
    VariableDeclarator {
        /// The variable.
        name: String,
        /// Its value.
        value: Box<Node>,
        /// The expression it is visible in.
        expr: Box<Node>,
    },
    /// Expressions separated by `;`.
    Sequence {
        /// The expressions.
        nodes: Vec<Node>,
    },
    /// An array literal.
    Array {
        /// The elements.
        nodes: Vec<Node>,
    },
    /// A map literal.
    Map {
        /// The entries, all [`NodeKind::Pair`].
        pairs: Vec<Node>,
    },
    /// An entry of a map literal.
    Pair {
        /// The key.
        key: Box<Node>,
        /// The value.
        value: Box<Node>,
    },
    /// A value the optimizer folded a subtree into.
    Constant {
        /// The value.
        value: Value,
    },
}

impl NodeKind {
    /// The Go type name of the node, like `BinaryNode`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "NilNode",
            Self::Identifier { .. } => "IdentifierNode",
            Self::Integer { .. } => "IntegerNode",
            Self::Float { .. } => "FloatNode",
            Self::Bool { .. } => "BoolNode",
            Self::String { .. } => "StringNode",
            Self::Bytes { .. } => "BytesNode",
            Self::Unary { .. } => "UnaryNode",
            Self::Binary { .. } => "BinaryNode",
            Self::Chain { .. } => "ChainNode",
            Self::Member { .. } => "MemberNode",
            Self::Slice { .. } => "SliceNode",
            Self::Call { .. } => "CallNode",
            Self::Builtin { .. } => "BuiltinNode",
            Self::Predicate { .. } => "PredicateNode",
            Self::Pointer { .. } => "PointerNode",
            Self::Conditional { .. } => "ConditionalNode",
            Self::VariableDeclarator { .. } => "VariableDeclaratorNode",
            Self::Sequence { .. } => "SequenceNode",
            Self::Array { .. } => "ArrayNode",
            Self::Map { .. } => "MapNode",
            Self::Pair { .. } => "PairNode",
            Self::Constant { .. } => "ConstantNode",
        }
    }

    /// Moves the children out into `out`, leaving `self` without any.
    fn take_children(&mut self, out: &mut Vec<Node>) {
        match std::mem::replace(self, Self::Nil) {
            Self::Unary { node, .. } | Self::Chain { node } | Self::Predicate { node } => {
                out.push(*node);
            }
            Self::Binary { left, right, .. } => out.extend([*left, *right]),
            Self::Member { node, property, .. } => out.extend([*node, *property]),
            Self::Slice { node, from, to } => {
                out.push(*node);
                out.extend(from.map(|n| *n));
                out.extend(to.map(|n| *n));
            }
            Self::Call { callee, arguments } => {
                out.push(*callee);
                out.extend(arguments);
            }
            Self::Builtin { arguments, map, .. } => {
                out.extend(arguments);
                out.extend(map.map(|n| *n));
            }
            Self::Conditional {
                cond, exp1, exp2, ..
            } => out.extend([*cond, *exp1, *exp2]),
            Self::VariableDeclarator { value, expr, .. } => out.extend([*value, *expr]),
            Self::Sequence { nodes } | Self::Array { nodes } | Self::Map { pairs: nodes } => {
                out.extend(nodes);
            }
            Self::Pair { key, value } => out.extend([*key, *value]),
            Self::Nil
            | Self::Identifier { .. }
            | Self::Integer { .. }
            | Self::Float { .. }
            | Self::Bool { .. }
            | Self::String { .. }
            | Self::Bytes { .. }
            | Self::Pointer { .. }
            | Self::Constant { .. } => {}
        }
    }

    fn is_leaf(&self) -> bool {
        matches!(
            self,
            Self::Nil
                | Self::Identifier { .. }
                | Self::Integer { .. }
                | Self::Float { .. }
                | Self::Bool { .. }
                | Self::String { .. }
                | Self::Bytes { .. }
                | Self::Pointer { .. }
                | Self::Constant { .. }
        )
    }
}

impl Node {
    /// A node at the default location (`0:0`).
    pub fn new(kind: NodeKind) -> Self {
        Self {
            location: Location::default(),
            kind,
            nature: Nature::default(),
        }
    }
}

impl Clone for Node {
    fn clone(&self) -> Self {
        with_stack(|| Self {
            location: self.location,
            kind: self.kind.clone(),
            nature: self.nature.clone(),
        })
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        with_stack(|| self.location == other.location && self.kind == other.kind)
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        with_stack(|| {
            f.debug_struct("Node")
                .field("location", &self.location)
                .field("kind", &self.kind)
                .finish()
        })
    }
}

/// Go frees trees with its garbage collector; dropping a deep tree here
/// iterates instead of recursing, so no depth the parser accepts can
/// overflow the stack.
impl Drop for Node {
    fn drop(&mut self) {
        if self.kind.is_leaf() {
            return;
        }
        let mut stack = Vec::new();
        self.kind.take_children(&mut stack);
        while let Some(mut node) = stack.pop() {
            node.kind.take_children(&mut stack);
        }
    }
}

/// `ast.Patch`: replaces `node` by `new`, keeping the location of `node`.
pub fn patch(node: &mut Node, mut new: Node) {
    new.location = node.location;
    *node = new;
}

/// Runs `f` on a stack with room for it: the tree functions recurse once per
/// level, and Go's growable stacks take any depth the parser accepts.
pub(crate) fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 128 * 1024;
    const NEW_STACK: usize = 4 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_STACK, f)
}
