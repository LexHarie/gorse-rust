// Ports: internal/tracker/seen.go @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! `SeenTracker`: which keys the document has defined, and as what, to flag
//! duplicates and mismatches as the TOML spec requires. The decoder checks
//! each top-level expression with it before decoding the expression.
//!
//! Each node of the tree of keys is an entry, linked to its first child and
//! next sibling. When an array table is entered again, the descendants of
//! its entry are removed (their slots go to a free list headed by the root's
//! `next`), so that the new element can define them again.

use std::borrow::Cow;
use std::fmt;

use super::ast::{Builder, Kind};
use super::with_stack;

/// `keyKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KeyKind {
    /// `invalidKind`.
    #[default]
    Invalid,
    /// `valueKind`.
    Value,
    /// `tableKind`.
    Table,
    /// `arrayTableKind`.
    ArrayTable,
}

/// `keyKind.String`.
impl fmt::Display for KeyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyKind::Invalid => "invalid",
            KeyKind::Value => "value",
            KeyKind::Table => "table",
            KeyKind::ArrayTable => "array table",
        })
    }
}

/// `entry`.
#[derive(Clone, Debug, Default)]
struct Entry<'a> {
    // -1 in Go means no child or no sibling.
    child: Option<usize>,
    next: Option<usize>,

    name: Cow<'a, [u8]>,
    kind: KeyKind,
    explicit: bool,
    kv: bool,
}

/// `SeenTracker`.
#[derive(Debug, Default)]
pub(crate) struct SeenTracker<'a> {
    entries: Vec<Entry<'a>>,
    current_idx: usize,
}

/// Go's `string(k)` of a key: the parser has checked that keys are UTF-8.
fn key_str(k: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(k)
}

impl<'a> SeenTracker<'a> {
    /// `reset`: only the root entry, at index 0, is the current table.
    fn reset(&mut self) {
        // Always contains a root element at index 0.
        self.current_idx = 0;
        self.entries.truncate(1);
        if self.entries.is_empty() {
            self.entries.push(Entry::default());
        }
        self.entries[0].child = None;
        self.entries[0].next = None;
    }

    /// `find`: the index of the child of parent_idx named k.
    fn find(&self, parent_idx: usize, k: &[u8]) -> Option<usize> {
        let mut i = self.entries[parent_idx].child;
        while let Some(idx) = i {
            if *self.entries[idx].name == *k {
                return Some(idx);
            }
            i = self.entries[idx].next;
        }
        None
    }

    /// `clear`: removes all descendants of the entry at idx.
    fn clear(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }

        let mut i = self.entries[idx].child;
        while let Some(cur) = i {
            let next = self.entries[cur].next;
            let n = self.entries[0].next;
            self.entries[0].next = Some(cur);
            self.entries[cur].next = n;
            self.entries[cur].name = Cow::Borrowed(&[]);
            with_stack(|| self.clear(cur));
            i = next;
        }

        self.entries[idx].child = None;
    }

    /// `create`: a new first child of parent_idx, reusing a freed slot if
    /// there is one.
    fn create(
        &mut self,
        parent_idx: usize,
        name: Cow<'a, [u8]>,
        kind: KeyKind,
        explicit: bool,
        kv: bool,
    ) -> usize {
        let e = Entry {
            child: None,
            next: self.entries[parent_idx].child,

            name,
            kind,
            explicit,
            kv,
        };
        let idx = match self.entries[0].next {
            Some(idx) => {
                self.entries[0].next = self.entries[idx].next;
                self.entries[idx] = e;
                idx
            }
            None => {
                self.entries.push(e);
                self.entries.len() - 1
            }
        };

        self.entries[parent_idx].child = Some(idx);

        idx
    }

    /// `setExplicitFlag`: the tables that key-values under parent_idx
    /// defined become explicitly defined.
    fn set_explicit_flag(&mut self, parent_idx: usize) {
        let mut i = self.entries[parent_idx].child;
        while let Some(idx) = i {
            if self.entries[idx].kv {
                self.entries[idx].explicit = true;
                self.entries[idx].kv = false;
            }
            with_stack(|| self.set_explicit_flag(idx));
            i = self.entries[idx].next;
        }
    }

    /// `CheckExpression`: checks that a top-level expression does not define
    /// keys again or with another type. Returns whether it is the first time
    /// the expression's key is seen (for array tables).
    pub(crate) fn check_expression(
        &mut self,
        b: &Builder<'a>,
        node: usize,
    ) -> Result<bool, String> {
        if self.entries.is_empty() {
            self.reset();
        }
        match b.node_at(node).kind {
            Kind::KeyValue => self.check_key_value(b, node),
            Kind::Table => self.check_table(b, node),
            Kind::ArrayTable => self.check_array_table(b, node),
            kind => panic!("this should not be a top level node type: {kind}"),
        }
    }

    fn check_table(&mut self, b: &Builder<'a>, node: usize) -> Result<bool, String> {
        // Go checks currentIdx >= 0, which always holds.
        self.set_explicit_flag(self.current_idx);

        let mut it = b.key(node);

        let mut parent_idx = 0;

        // This code is duplicated in checkArrayTable. This is because
        // factoring it in a function requires to copy the iterator, or
        // allocate it to the heap, which is not cheap.
        while it.next(b) {
            if it.is_last(b) {
                break;
            }

            let k = &b.node_at(it.node()).data;

            let idx = match self.find(parent_idx, k) {
                None => self.create(parent_idx, k.clone(), KeyKind::Table, false, false),
                Some(idx) => {
                    let entry = &self.entries[idx];
                    if entry.kind == KeyKind::Value {
                        return Err(format!(
                            "toml: expected {} to be a table, not a {}",
                            key_str(k),
                            entry.kind
                        ));
                    }
                    idx
                }
            };
            parent_idx = idx;
        }

        let k = &b.node_at(it.node()).data;
        let idx = self.find(parent_idx, k);

        let mut first = false;
        let idx = match idx {
            Some(idx) => {
                let kind = self.entries[idx].kind;
                if kind != KeyKind::Table {
                    return Err(format!(
                        "toml: key {} should be a table, not a {kind}",
                        key_str(k)
                    ));
                }
                if self.entries[idx].explicit {
                    return Err(format!("toml: table {} already exists", key_str(k)));
                }
                self.entries[idx].explicit = true;
                idx
            }
            None => {
                first = true;
                self.create(parent_idx, k.clone(), KeyKind::Table, true, false)
            }
        };

        self.current_idx = idx;

        Ok(first)
    }

    fn check_array_table(&mut self, b: &Builder<'a>, node: usize) -> Result<bool, String> {
        // Go checks currentIdx >= 0, which always holds.
        self.set_explicit_flag(self.current_idx);

        let mut it = b.key(node);

        let mut parent_idx = 0;

        while it.next(b) {
            if it.is_last(b) {
                break;
            }

            let k = &b.node_at(it.node()).data;

            let idx = match self.find(parent_idx, k) {
                None => self.create(parent_idx, k.clone(), KeyKind::Table, false, false),
                Some(idx) => {
                    let entry = &self.entries[idx];
                    if entry.kind == KeyKind::Value {
                        return Err(format!(
                            "toml: expected {} to be a table, not a {}",
                            key_str(k),
                            entry.kind
                        ));
                    }
                    idx
                }
            };

            parent_idx = idx;
        }

        let k = &b.node_at(it.node()).data;
        let found = self.find(parent_idx, k);

        let first_time = found.is_none();
        let idx = match found {
            None => self.create(parent_idx, k.clone(), KeyKind::ArrayTable, true, false),
            Some(idx) => {
                let kind = self.entries[idx].kind;
                if kind != KeyKind::ArrayTable {
                    // Go passes the kind and the key in the wrong order.
                    return Err(format!(
                        "toml: key {kind} already exists as a {},  but should be an array table",
                        key_str(k)
                    ));
                }
                self.clear(idx);
                idx
            }
        };

        self.current_idx = idx;

        Ok(first_time)
    }

    fn check_key_value(&mut self, b: &Builder<'a>, node: usize) -> Result<bool, String> {
        let mut parent_idx = self.current_idx;
        let mut it = b.key(node);

        while it.next(b) {
            let k = &b.node_at(it.node()).data;

            let idx = match self.find(parent_idx, k) {
                None => self.create(parent_idx, k.clone(), KeyKind::Table, false, true),
                Some(idx) => {
                    let entry = &self.entries[idx];
                    if it.is_last(b) {
                        return Err(format!("toml: key {} is already defined", key_str(k)));
                    } else if entry.kind != KeyKind::Table {
                        return Err(format!(
                            "toml: expected {} to be a table, not a {}",
                            key_str(k),
                            entry.kind
                        ));
                    } else if entry.explicit {
                        return Err(format!(
                            "toml: cannot redefine table {} that has already been explicitly defined",
                            key_str(k)
                        ));
                    }
                    idx
                }
            };

            parent_idx = idx;
        }

        self.entries[parent_idx].kind = KeyKind::Value;

        let value = b.value(node);

        match b.node_at(value).kind {
            Kind::InlineTable => with_stack(|| Self::check_inline_table(b, value)),
            Kind::Array => with_stack(|| Self::check_array(b, value)),
            _ => Ok(false),
        }
    }

    fn check_array(b: &Builder<'a>, node: usize) -> Result<bool, String> {
        let mut first = false;
        let mut it = b.children(node);
        while it.next(b) {
            let n = it.node();
            match b.node_at(n).kind {
                Kind::InlineTable => first = with_stack(|| Self::check_inline_table(b, n))?,
                Kind::Array => first = with_stack(|| Self::check_array(b, n))?,
                _ => {}
            }
        }
        Ok(first)
    }

    /// `checkInlineTable`: an inline table is checked with a tracker of its
    /// own (Go takes one from a pool and resets it).
    fn check_inline_table(b: &Builder<'a>, node: usize) -> Result<bool, String> {
        let mut s = SeenTracker::default();
        s.reset();

        let mut first = false;
        let mut it = b.children(node);
        while it.next(b) {
            let n = it.node();
            first = s.check_key_value(b, n)?;
        }

        // As inline tables are self-contained, the tracker does not need to
        // retain the details of what they contain. The keyValue element that
        // creates the inline table is kept to mark the presence of the inline
        // table and prevent redefinition of its keys: check* functions cannot
        // walk into a value.
        Ok(first)
    }
}
