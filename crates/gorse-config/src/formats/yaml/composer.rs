// Ports: decode.go (type parser: parse, node, document, alias, scalar, sequence, mapping, expect, peek, fail), yaml.go (Node, Kind, Style, ShortTag, indicatedString) @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3: Copyright 2011-2016 Canonical Ltd,
// licensed under the Apache License, Version 2.0; see
// crates/gorse-config/licenses/YAML-V3-LICENSE.

//! yaml.v3's composer: it builds the node tree of the first document from
//! the parser's events, registering anchors as it goes. Comments, columns
//! and the anchor field of nodes are left out: the decoder never reads them.

use std::collections::HashMap;

use super::parser::{Event, EventKind, Parser};
use super::resolve::{MAP_TAG, MERGE_TAG, NULL_TAG, SEQ_TAG, STR_TAG, resolve, short_tag};
use super::scanner::{ScalarStyle, SyntaxError};

/// An index into [`Tree::nodes`].
pub(crate) type NodeId = usize;

/// `Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Document,
    Sequence,
    Mapping,
    Scalar,
    Alias,
}

/// The `Style` bits yaml.v3 reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    /// `SingleQuotedStyle`, `DoubleQuotedStyle`, `LiteralStyle` or
    /// `FoldedStyle`: the scalar was not plain.
    pub(crate) quoted_or_block: bool,
}

/// `Node`.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    pub(crate) style: Style,
    pub(crate) tag: String,
    pub(crate) value: String,
    pub(crate) alias: Option<NodeId>,
    pub(crate) content: Vec<NodeId>,
    pub(crate) line: usize,
}

/// The nodes of a document.
#[derive(Default)]
pub(crate) struct Tree {
    pub(crate) nodes: Vec<Node>,
}

impl Tree {
    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// `(*Node).indicatedString`.
    pub(crate) fn indicated_string(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.kind == Kind::Scalar
            && (short_tag(&n.tag) == STR_TAG
                || ((n.tag.is_empty() || n.tag == "!") && n.style.quoted_or_block))
    }

    /// `(*Node).ShortTag`. Alias cycles cannot occur here: an alias names a
    /// node whose tag is set or that is not an alias.
    pub(crate) fn short_tag(&self, id: NodeId) -> String {
        if self.indicated_string(id) {
            return STR_TAG.to_owned();
        }
        let n = self.node(id);
        if n.tag.is_empty() || n.tag == "!" {
            return match n.kind {
                Kind::Mapping => MAP_TAG.to_owned(),
                Kind::Sequence => SEQ_TAG.to_owned(),
                Kind::Alias => n.alias.map(|a| self.short_tag(a)).unwrap_or_default(),
                Kind::Scalar => resolve("", &n.value)
                    .map(|(tag, _)| tag)
                    .unwrap_or_default(),
                Kind::Document => String::new(),
            };
        }
        short_tag(&n.tag)
    }

    /// `isMerge`.
    pub(crate) fn is_merge(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.kind == Kind::Scalar
            && n.value == "<<"
            && (n.tag.is_empty() || n.tag == "!" || short_tag(&n.tag) == MERGE_TAG)
    }

    /// Whether a node is a null for `prepare` and the mapping value rule:
    /// `n.ShortTag() == nullTag`.
    pub(crate) fn is_null(&self, id: NodeId) -> bool {
        self.short_tag(id) == NULL_TAG
    }
}

/// A composer failure: `p.fail()` on a parser error, or a `failf` text.
pub(crate) enum ComposeError {
    Syntax(SyntaxError),
    Fail(String),
}

impl From<SyntaxError> for ComposeError {
    fn from(e: SyntaxError) -> Self {
        ComposeError::Syntax(e)
    }
}

/// yaml.v3's `parser` type.
pub(crate) struct Composer {
    parser: Parser,
    event: Option<Event>,
    tree: Tree,
    anchors: HashMap<String, NodeId>,
}

type Compose<T> = Result<T, ComposeError>;

impl Composer {
    pub(crate) fn new(parser: Parser) -> Self {
        Composer {
            parser,
            event: None,
            tree: Tree::default(),
            anchors: HashMap::new(),
        }
    }

    /// `newParser(...).parse()` for the root: the tree and its root node, or
    /// `None` for an empty stream.
    pub(crate) fn compose(mut self) -> Compose<(Tree, Option<NodeId>)> {
        // p.init()
        self.expect_stream_start()?;
        let root = self.parse()?;
        Ok((self.tree, root))
    }

    /// `p.peek()`: the current event, parsing one if there is none.
    fn peek(&mut self) -> Compose<&Event> {
        if self.event.is_none() {
            // yaml_parser_parse yields no event after the stream end; yaml.v3
            // then sees yaml_NO_EVENT, which no caller expects.
            let e = self.parser.parse()?;
            self.event = Some(e.unwrap_or(Event {
                kind: EventKind::StreamEnd,
                start: Default::default(),
            }));
        }
        // Set just above.
        self.event
            .as_ref()
            .ok_or_else(|| ComposeError::Fail("internal error: no event".to_owned()))
    }

    /// `p.expect(e)` for the event kind `want` names.
    fn expect(&mut self, want: fn(&EventKind) -> bool, name: &str) -> Compose<()> {
        let event = self.peek()?;
        if event.kind == EventKind::StreamEnd && !want(&event.kind) {
            return Err(ComposeError::Fail(
                "attempted to go past the end of stream; corrupted value?".to_owned(),
            ));
        }
        if !want(&event.kind) {
            return Err(ComposeError::Fail(format!(
                "expected {name} event but got {}",
                event_name(&event.kind)
            )));
        }
        self.event = None;
        Ok(())
    }

    fn expect_stream_start(&mut self) -> Compose<()> {
        self.expect(|k| matches!(k, EventKind::StreamStart), "STREAM_START")
    }

    /// `p.parse()`.
    fn parse(&mut self) -> Compose<Option<NodeId>> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            let kind = self.peek()?.kind.clone();
            Ok(Some(match kind {
                EventKind::Scalar { .. } => self.scalar()?,
                EventKind::Alias(_) => self.alias()?,
                EventKind::MappingStart { .. } => self.mapping()?,
                EventKind::SequenceStart { .. } => self.sequence()?,
                EventKind::DocumentStart => self.document()?,
                EventKind::StreamEnd => return Ok(None),
                other => {
                    return Err(ComposeError::Fail(format!(
                        "internal error: attempted to parse unknown event (please report): {}",
                        event_name(&other)
                    )));
                }
            }))
        })
    }

    /// `p.node(kind, defaultTag, tag, value)`.
    fn node(&mut self, kind: Kind, default_tag: &str, tag: &str, value: String) -> NodeId {
        let tag = if !tag.is_empty() && tag != "!" {
            short_tag(tag)
        } else if !default_tag.is_empty() {
            default_tag.to_owned()
        } else if kind == Kind::Scalar {
            resolve("", &value).map(|(tag, _)| tag).unwrap_or_default()
        } else {
            tag.to_owned()
        };
        let line = self.event.as_ref().map_or(0, |e| e.start.line) + 1;
        self.tree.nodes.push(Node {
            kind,
            style: Style::default(),
            tag,
            value,
            alias: None,
            content: Vec::new(),
            line,
        });
        self.tree.nodes.len() - 1
    }

    /// `p.anchor(n, anchor)`.
    fn anchor(&mut self, id: NodeId, anchor: Option<String>) {
        if let Some(anchor) = anchor {
            self.anchors.insert(anchor, id);
        }
    }

    fn parse_child(&mut self, parent: NodeId) -> Compose<NodeId> {
        // A child is never the stream end: the parser always yields a node
        // inside a document or collection.
        let child = self.parse()?.ok_or_else(|| {
            ComposeError::Fail(
                "attempted to go past the end of stream; corrupted value?".to_owned(),
            )
        })?;
        self.tree.nodes[parent].content.push(child);
        Ok(child)
    }

    fn document(&mut self) -> Compose<NodeId> {
        let n = self.node(Kind::Document, "", "", String::new());
        self.expect(|k| matches!(k, EventKind::DocumentStart), "DOCUMENT_START")?;
        self.parse_child(n)?;
        self.expect(|k| matches!(k, EventKind::DocumentEnd), "DOCUMENT_END")?;
        Ok(n)
    }

    fn alias(&mut self) -> Compose<NodeId> {
        let name = match &self.peek()?.kind {
            EventKind::Alias(name) => name.clone(),
            _ => String::new(),
        };
        let n = self.node(Kind::Alias, "", "", name.clone());
        let target = self.anchors.get(&name).copied();
        self.tree.nodes[n].alias = target;
        if target.is_none() {
            return Err(ComposeError::Fail(format!(
                "unknown anchor '{name}' referenced"
            )));
        }
        self.expect(|k| matches!(k, EventKind::Alias(_)), "ALIAS")?;
        Ok(n)
    }

    fn scalar(&mut self) -> Compose<NodeId> {
        let (anchor, tag, value, style) = match &self.peek()?.kind {
            EventKind::Scalar {
                anchor,
                tag,
                value,
                style,
            } => (anchor.clone(), tag.clone(), value.clone(), *style),
            _ => (None, String::new(), String::new(), ScalarStyle::Plain),
        };
        let quoted_or_block = style != ScalarStyle::Plain;
        let default_tag = if !quoted_or_block {
            if value == "<<" { MERGE_TAG } else { "" }
        } else {
            STR_TAG
        };
        let n = self.node(Kind::Scalar, default_tag, &tag, value);
        self.tree.nodes[n].style.quoted_or_block = quoted_or_block;
        self.anchor(n, anchor);
        self.expect(|k| matches!(k, EventKind::Scalar { .. }), "SCALAR")?;
        Ok(n)
    }

    fn sequence(&mut self) -> Compose<NodeId> {
        let (anchor, tag) = match &self.peek()?.kind {
            EventKind::SequenceStart { anchor, tag, .. } => (anchor.clone(), tag.clone()),
            _ => (None, String::new()),
        };
        let n = self.node(Kind::Sequence, SEQ_TAG, &tag, String::new());
        self.anchor(n, anchor);
        self.expect(
            |k| matches!(k, EventKind::SequenceStart { .. }),
            "SEQUENCE_START",
        )?;
        while self.peek()?.kind != EventKind::SequenceEnd {
            self.parse_child(n)?;
        }
        self.expect(|k| matches!(k, EventKind::SequenceEnd), "SEQUENCE_END")?;
        Ok(n)
    }

    fn mapping(&mut self) -> Compose<NodeId> {
        let (anchor, tag) = match &self.peek()?.kind {
            EventKind::MappingStart { anchor, tag, .. } => (anchor.clone(), tag.clone()),
            _ => (None, String::new()),
        };
        let n = self.node(Kind::Mapping, MAP_TAG, &tag, String::new());
        self.anchor(n, anchor);
        self.expect(
            |k| matches!(k, EventKind::MappingStart { .. }),
            "MAPPING_START",
        )?;
        while self.peek()?.kind != EventKind::MappingEnd {
            self.parse_child(n)?;
            self.parse_child(n)?;
        }
        self.expect(|k| matches!(k, EventKind::MappingEnd), "MAPPING_END")?;
        Ok(n)
    }
}

/// `yaml_event_type_t.String()`.
fn event_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::StreamStart => "STREAM_START",
        EventKind::StreamEnd => "STREAM_END",
        EventKind::DocumentStart => "DOCUMENT_START",
        EventKind::DocumentEnd => "DOCUMENT_END",
        EventKind::Alias(_) => "ALIAS",
        EventKind::Scalar { .. } => "SCALAR",
        EventKind::SequenceStart { .. } => "SEQUENCE_START",
        EventKind::SequenceEnd => "SEQUENCE_END",
        EventKind::MappingStart { .. } => "MAPPING_START",
        EventKind::MappingEnd => "MAPPING_END",
    }
}
