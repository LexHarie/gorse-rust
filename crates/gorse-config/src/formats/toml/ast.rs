// Ports: unstable/ast.go, unstable/builder.go, unstable/kind.go @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! The AST of one top-level expression, as go-toml's parser builds it: nodes
//! in one array, linked to their first child and next sibling.
//!
//! Go stores the links as offsets relative to the node (0 for none) and walks
//! them with pointer arithmetic; here they are absolute indices into the
//! array (`None` for none), which describe the same tree. Go's `Iterator`
//! holds a node pointer; [`Iter`] holds an index and reads the tree it is
//! given, so the decoder can keep a key iterator while the parser moves on
//! (Go does the same, and never reads the stale nodes).

use std::borrow::Cow;
use std::fmt;

/// `Kind`: the type of TOML structure a node holds. (Go also has
/// `Invalid`, the zero value, and `Comment`, for comment nodes, which are
/// only made with `KeepComments`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    // Meta
    /// `Key`.
    Key,

    // Top level structures
    /// `Table`.
    Table,
    /// `ArrayTable`.
    ArrayTable,
    /// `KeyValue`.
    KeyValue,

    // Containers values
    /// `Array`.
    Array,
    /// `InlineTable`.
    InlineTable,

    // Values
    /// `String`.
    String,
    /// `Bool`.
    Bool,
    /// `Float`.
    Float,
    /// `Integer`.
    Integer,
    /// `LocalDate`.
    LocalDate,
    /// `LocalTime`.
    LocalTime,
    /// `LocalDateTime`.
    LocalDateTime,
    /// `DateTime`.
    DateTime,
}

/// `Kind.String`.
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Key => "Key",
            Kind::Table => "Table",
            Kind::ArrayTable => "ArrayTable",
            Kind::KeyValue => "KeyValue",
            Kind::Array => "Array",
            Kind::InlineTable => "InlineTable",
            Kind::String => "String",
            Kind::Bool => "Bool",
            Kind::Float => "Float",
            Kind::Integer => "Integer",
            Kind::LocalDate => "LocalDate",
            Kind::LocalTime => "LocalTime",
            Kind::LocalDateTime => "LocalDateTime",
            Kind::DateTime => "DateTime",
        })
    }
}

/// `Range`: a range of bytes in the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Range {
    /// `Offset`.
    pub(crate) offset: u32,
    /// `Length`.
    pub(crate) length: u32,
}

/// `Node`: a node of the AST.
///
/// - an Array has one child per element;
/// - an InlineTable has one child per key-value (each of kind KeyValue);
/// - a KeyValue has at least two children: the value, then the parts of the
///   (possibly dotted) key;
/// - a Table and an ArrayTable have the parts of their key as children.
#[derive(Clone, Debug)]
pub(crate) struct Node<'a> {
    /// `Kind`.
    pub(crate) kind: Kind,
    /// `Raw`: the node's bytes in the document, where relevant.
    pub(crate) raw: Range,
    /// `Data`: the node's value, borrowed from the document or (for strings
    /// with escapes) decoded into a buffer of its own.
    pub(crate) data: Cow<'a, [u8]>,
    /// `next`: the next sibling.
    next: Option<usize>,
    /// `child`: the first child.
    child: Option<usize>,
}

impl<'a> Node<'a> {
    /// A node with no links.
    pub(crate) fn new(kind: Kind, raw: Range, data: Cow<'a, [u8]>) -> Self {
        Node {
            kind,
            raw,
            data,
            next: None,
            child: None,
        }
    }
}

/// `reference`: the index of a node in the builder; Go's `invalidReference`
/// is `None`.
pub(crate) type Reference = Option<usize>;

/// `builder` (with its `root`): the nodes of the expression being parsed.
#[derive(Debug, Default)]
pub(crate) struct Builder<'a> {
    nodes: Vec<Node<'a>>,
    last_idx: usize,
}

impl<'a> Builder<'a> {
    /// `Reset`.
    pub(crate) fn reset(&mut self) {
        self.nodes.clear();
        self.last_idx = 0;
    }

    /// `Push`.
    pub(crate) fn push(&mut self, n: Node<'a>) -> usize {
        self.last_idx = self.nodes.len();
        self.nodes.push(n);
        self.last_idx
    }

    /// `PushAndChain`: pushes n as the next sibling of the last pushed node.
    pub(crate) fn push_and_chain(&mut self, n: Node<'a>) -> usize {
        let new_idx = self.nodes.len();
        self.nodes.push(n);
        // Go checks lastIdx >= 0, which always holds.
        self.nodes[self.last_idx].next = Some(new_idx);
        self.last_idx = new_idx;
        self.last_idx
    }

    /// `AttachChild`.
    pub(crate) fn attach_child(&mut self, parent: usize, child: usize) {
        self.nodes[parent].child = Some(child);
    }

    /// `Chain`.
    pub(crate) fn chain(&mut self, from: usize, to: usize) {
        self.nodes[from].next = Some(to);
    }

    /// `NodeAt`.
    pub(crate) fn node_at(&self, idx: usize) -> &Node<'a> {
        &self.nodes[idx]
    }

    /// `Node.Next` of the node at idx.
    pub(crate) fn next(&self, idx: usize) -> Option<usize> {
        self.nodes[idx].next
    }

    /// `Node.Key`: the parts of the key of a KeyValue, Table or ArrayTable.
    pub(crate) fn key(&self, idx: usize) -> Iter {
        let n = &self.nodes[idx];
        match n.kind {
            Kind::KeyValue => match n.child {
                Some(value) => Iter::new(self.next(value)),
                None => panic!("KeyValue should have at least two children"),
            },
            Kind::Table | Kind::ArrayTable => Iter::new(n.child),
            kind => panic!("Key() is not supported on a {kind}"),
        }
    }

    /// `Node.Value`: the value of a KeyValue.
    pub(crate) fn value(&self, idx: usize) -> usize {
        match self.nodes[idx].child {
            Some(value) => value,
            // Go returns nil and fails on its first use.
            None => panic!("KeyValue without a value"),
        }
    }

    /// `Node.Children`.
    pub(crate) fn children(&self, idx: usize) -> Iter {
        Iter::new(self.nodes[idx].child)
    }
}

/// `Iterator`: walks a node and its next siblings. It starts before the
/// first node; call [`Iter::next`] first.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Iter {
    started: bool,
    node: Option<usize>,
}

impl Iter {
    fn new(node: Option<usize>) -> Self {
        Iter {
            started: false,
            node,
        }
    }

    /// `Next`: moves to the next node and says whether there is one.
    pub(crate) fn next(&mut self, b: &Builder<'_>) -> bool {
        if !self.started {
            self.started = true;
        } else if let Some(n) = self.node {
            self.node = b.next(n);
        }
        self.node.is_some()
    }

    /// `IsLast`: whether the current node is the last one.
    pub(crate) fn is_last(&self, b: &Builder<'_>) -> bool {
        match self.node {
            Some(n) => b.next(n).is_none(),
            None => panic!("runtime error: invalid memory address or nil pointer dereference"),
        }
    }

    /// `Node`: the current node's index.
    pub(crate) fn node(&self) -> usize {
        match self.node {
            Some(n) => n,
            None => panic!("runtime error: invalid memory address or nil pointer dereference"),
        }
    }
}
