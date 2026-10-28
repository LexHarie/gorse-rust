// Ports: decode.go (type decoder: unmarshal, document, alias, null, scalar, sequence, mapping, merge, terror, isStringMap, allowedAliasRatio, the possibly unhashable merge keys) @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3: Copyright 2011-2016 Canonical Ltd,
// licensed under the Apache License, Version 2.0; see
// crates/gorse-config/licenses/YAML-V3-LICENSE.

//! yaml.v3's decoder, for the targets viper's codec reaches: the root
//! `map[string]interface{}`, the `interface{}` values in it, and the
//! `string` keys of string maps.
//!
//! Values are Go values ([`GoValue`]): strings are bytes, because a
//! `!!binary` scalar decodes to any bytes, and maps follow Go's rules for
//! keys (a `NaN` key never matches, `0` and `-0` are one key, an
//! assignment to an existing key replaces the key too). Maps keep the order
//! in which keys were first set.

use std::collections::{HashMap, HashSet};

use gorse_common::gostrconv::quote;
use gorse_common::gotime::Time;

use super::composer::{Kind, NodeId, Tree};
use super::gofmt::{base64_decode, sharp_v};
use super::resolve::{BINARY_TAG, MAP_TAG, Resolved, SEQ_TAG, STR_TAG, resolve, short_tag};

/// A Go value of static type `interface{}`.
#[derive(Clone, Debug)]
pub(crate) enum GoValue {
    Nil,
    Bool(bool),
    /// `int`.
    Int(i64),
    Uint64(u64),
    Float64(f64),
    String(Vec<u8>),
    Time(Time),
    /// `[]interface {}`.
    Slice(Vec<GoValue>),
    /// `map[string]interface {}`.
    StringMap(StringMap),
    /// `map[interface {}]interface {}`.
    AnyMap(AnyMap),
}

impl GoValue {
    /// The Go type of a value that cannot be a map key, for the runtime's
    /// "hash of unhashable type" panic.
    fn unhashable_type(&self) -> Option<&'static str> {
        match self {
            GoValue::Slice(_) => Some("[]interface {}"),
            GoValue::StringMap(_) => Some("map[string]interface {}"),
            GoValue::AnyMap(_) => Some("map[interface {}]interface {}"),
            _ => None,
        }
    }

    /// Go's `==` on two hashable interface values.
    fn go_eq(&self, other: &GoValue) -> bool {
        match (self, other) {
            (GoValue::Nil, GoValue::Nil) => true,
            (GoValue::Bool(a), GoValue::Bool(b)) => a == b,
            (GoValue::Int(a), GoValue::Int(b)) => a == b,
            (GoValue::Uint64(a), GoValue::Uint64(b)) => a == b,
            (GoValue::Float64(a), GoValue::Float64(b)) => a == b,
            (GoValue::String(a), GoValue::String(b)) => a == b,
            (GoValue::Time(a), GoValue::Time(b)) => a == b,
            _ => false,
        }
    }
}

/// A non-nil `map[string]interface {}`.
#[derive(Clone, Debug, Default)]
pub(crate) struct StringMap {
    entries: Vec<(Vec<u8>, GoValue)>,
    index: HashMap<Vec<u8>, usize>,
}

impl StringMap {
    fn contains(&self, key: &[u8]) -> bool {
        self.index.contains_key(key)
    }

    fn set(&mut self, key: Vec<u8>, value: GoValue) {
        match self.index.get(&key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    /// The entries, in the order their keys were first set.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Vec<u8>, &GoValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub(crate) fn into_entries(self) -> Vec<(Vec<u8>, GoValue)> {
        self.entries
    }
}

/// A non-nil `map[interface {}]interface {}`. Keys are hashable values.
#[derive(Clone, Debug, Default)]
pub(crate) struct AnyMap {
    entries: Vec<(GoValue, GoValue)>,
}

impl AnyMap {
    fn position(&self, key: &GoValue) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k.go_eq(key))
    }

    fn set(&mut self, key: GoValue, value: GoValue) {
        match self.position(&key) {
            // The runtime updates the key as well (NeedKeyUpdate): -0
            // replaces 0.
            Some(i) => self.entries[i] = (key, value),
            None => self.entries.push((key, value)),
        }
    }

    /// The entries, in the order their keys were first set.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&GoValue, &GoValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub(crate) fn into_entries(self) -> Vec<(GoValue, GoValue)> {
        self.entries
    }
}

/// A map being decoded into.
enum MapRef<'a> {
    String(&'a mut StringMap),
    Any(&'a mut AnyMap),
}

impl MapRef<'_> {
    fn type_name(&self) -> &'static str {
        match self {
            MapRef::String(_) => "map[string]interface {}",
            MapRef::Any(_) => "map[interface {}]interface {}",
        }
    }
}

/// The `reflect.Value` a node is decoded into.
enum Out<'a> {
    /// An `interface{}`.
    Iface(&'a mut GoValue),
    /// A `string` (a key of a `map[string]interface {}`).
    String(&'a mut Vec<u8>),
    /// A non-nil map.
    Map(MapRef<'a>),
}

impl Out<'_> {
    fn type_name(&self) -> &'static str {
        match self {
            Out::Iface(_) => "interface {}",
            Out::String(_) => "string",
            Out::Map(m) => m.type_name(),
        }
    }
}

/// A `failf` text, without `yaml: `.
pub(crate) struct Fail(pub(crate) String);

type Decode<T> = Result<T, Fail>;

/// The decoder's outcome: the type errors, if any.
pub(crate) struct Outcome {
    pub(crate) terrors: Vec<String>,
}

/// `alias_ratio_range_low`, `alias_ratio_range_high`.
const ALIAS_RATIO_RANGE_LOW: usize = 400_000;
const ALIAS_RATIO_RANGE_HIGH: usize = 4_000_000;

/// `allowedAliasRatio`.
fn allowed_alias_ratio(decode_count: usize) -> f64 {
    if decode_count <= ALIAS_RATIO_RANGE_LOW {
        0.99
    } else if decode_count >= ALIAS_RATIO_RANGE_HIGH {
        0.10
    } else {
        let range = (ALIAS_RATIO_RANGE_HIGH - ALIAS_RATIO_RANGE_LOW) as f64;
        0.99 - 0.89 * ((decode_count - ALIAS_RATIO_RANGE_LOW) as f64 / range)
    }
}

/// yaml.v3's `decoder`.
pub(crate) struct Decoder<'t> {
    tree: &'t Tree,
    aliases: HashSet<NodeId>,
    terrors: Vec<String>,
    decode_count: usize,
    alias_count: usize,
    alias_depth: usize,
    merged_fields: Option<Vec<GoValue>>,
}

/// Decodes the document `root` into the viper map `out`: `d.unmarshal(node,
/// v)` with `v` the dereferenced `*map[string]any`.
pub(crate) fn decode_root(tree: &Tree, root: NodeId, out: &mut StringMap) -> Decode<Outcome> {
    let mut d = Decoder {
        tree,
        aliases: HashSet::new(),
        terrors: Vec::new(),
        decode_count: 0,
        alias_count: 0,
        alias_depth: 0,
        merged_fields: None,
    };
    d.unmarshal(root, Out::Map(MapRef::String(out)))?;
    Ok(Outcome { terrors: d.terrors })
}

impl Decoder<'_> {
    fn unmarshal(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
            self.decode_count += 1;
            if self.alias_depth > 0 {
                self.alias_count += 1;
            }
            if self.alias_count > 100
                && self.decode_count > 1000
                && self.alias_count as f64 / self.decode_count as f64 > allowed_alias_ratio(self.decode_count)
            {
                return Err(Fail("document contains excessive aliasing".to_owned()));
            }
            match self.tree.node(n).kind {
                Kind::Document => self.document(n, out),
                Kind::Alias => self.alias(n, out),
                // prepare() changes nothing for these targets: none is a
                // pointer or implements an unmarshaler.
                Kind::Scalar => self.scalar(n, out),
                Kind::Mapping => self.mapping(n, out),
                Kind::Sequence => self.sequence(n, out),
            }
        })
    }

    fn document(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        let content = &self.tree.node(n).content;
        if content.len() == 1 {
            self.unmarshal(content[0], out)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn alias(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        let node = self.tree.node(n);
        if self.aliases.contains(&n) {
            return Err(Fail(format!("anchor '{}' value contains itself", node.value)));
        }
        self.aliases.insert(n);
        self.alias_depth += 1;
        // The composer resolved every alias.
        let good = match node.alias {
            Some(target) => self.unmarshal(target, out)?,
            None => false,
        };
        self.alias_depth -= 1;
        self.aliases.remove(&n);
        Ok(good)
    }

    /// `d.null(out)`: every target here is addressable.
    fn null(out: Out<'_>) -> bool {
        match out {
            Out::Iface(v) => {
                *v = GoValue::Nil;
                true
            }
            // The map variable becomes nil; the caller's map is unchanged.
            Out::Map(_) => true,
            Out::String(_) => false,
        }
    }

    fn scalar(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        let node = self.tree.node(n);
        let (tag, resolved) = if self.tree.indicated_string(n) {
            (STR_TAG.to_owned(), Some(GoValue::String(node.value.clone().into_bytes())))
        } else {
            let (tag, resolved) = resolve(&node.tag, &node.value).map_err(Fail)?;
            let value = match resolved {
                Resolved::String(s) if tag == BINARY_TAG => match base64_decode(s.as_bytes()) {
                    Some(data) => Some(GoValue::String(data)),
                    None => return Err(Fail("!!binary value contains invalid base64 data".to_owned())),
                },
                Resolved::Null => None,
                Resolved::Bool(b) => Some(GoValue::Bool(b)),
                Resolved::Int(i) => Some(GoValue::Int(i)),
                Resolved::Uint64(u) => Some(GoValue::Uint64(u)),
                Resolved::Float64(f) => Some(GoValue::Float64(f)),
                Resolved::String(s) => Some(GoValue::String(s.into_bytes())),
                Resolved::Time(t) => Some(GoValue::Time(t)),
            };
            (tag, value)
        };
        let Some(resolved) = resolved else {
            return Ok(Self::null(out));
        };
        match out {
            Out::Iface(v) => {
                *v = resolved;
                Ok(true)
            }
            Out::String(s) => {
                // A string (the !!binary data included) is set as it is;
                // other values are set as the scalar's text.
                *s = match resolved {
                    GoValue::String(bytes) => bytes,
                    _ => node.value.clone().into_bytes(),
                };
                Ok(true)
            }
            out @ Out::Map(_) => {
                self.terror(n, &tag, out.type_name());
                Ok(false)
            }
        }
    }

    fn sequence(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        let slot = match out {
            Out::Iface(slot) => slot,
            out => {
                self.terror(n, SEQ_TAG, out.type_name());
                return Ok(false);
            }
        };
        let content = &self.tree.node(n).content;
        let mut items = Vec::with_capacity(content.len());
        for &child in content {
            let mut e = GoValue::Nil;
            if self.unmarshal(child, Out::Iface(&mut e))? {
                items.push(e);
            }
        }
        *slot = GoValue::Slice(items);
        Ok(true)
    }

    /// `isStringMap`.
    fn is_string_map(&self, n: NodeId) -> bool {
        let content = &self.tree.node(n).content;
        content.iter().step_by(2).all(|&k| {
            let tag = self.tree.short_tag(k);
            tag == STR_TAG || tag == super::resolve::MERGE_TAG
        })
    }

    fn mapping(&mut self, n: NodeId, out: Out<'_>) -> Decode<bool> {
        let node = self.tree.node(n);
        let l = node.content.len();
        // uniqueKeys
        let nerrs = self.terrors.len();
        for i in (0..l).step_by(2) {
            let ni = self.tree.node(node.content[i]);
            for j in (i + 2..l).step_by(2) {
                let nj = self.tree.node(node.content[j]);
                if ni.kind == nj.kind && ni.value == nj.value {
                    self.terrors.push(format!(
                        "line {}: mapping key {} already defined at line {}",
                        nj.line,
                        quote(nj.value.as_bytes()),
                        ni.line
                    ));
                }
            }
        }
        if self.terrors.len() > nerrs {
            return Ok(false);
        }
        match out {
            Out::Iface(slot) => {
                if self.is_string_map(n) {
                    let mut m = StringMap::default();
                    self.fill_mapping(n, MapRef::String(&mut m))?;
                    *slot = GoValue::StringMap(m);
                } else {
                    let mut m = AnyMap::default();
                    self.fill_mapping(n, MapRef::Any(&mut m))?;
                    *slot = GoValue::AnyMap(m);
                }
                Ok(true)
            }
            Out::Map(m) => {
                self.fill_mapping(n, m)?;
                Ok(true)
            }
            out @ Out::String(_) => {
                self.terror(n, MAP_TAG, out.type_name());
                Ok(false)
            }
        }
    }

    /// The part of `mapping` after the map is made. The map is never new
    /// (`mapIsNew` is false): yaml.v3 makes it before the check, and the
    /// viper map is not nil.
    fn fill_mapping(&mut self, n: NodeId, mut out: MapRef<'_>) -> Decode<()> {
        let content = &self.tree.node(n).content;
        let mut merged_fields = self.merged_fields.take();
        let mut merge_node = None;
        for pair in content.chunks(2) {
            let (key, value) = (pair[0], pair[1]);
            if self.tree.is_merge(key) {
                merge_node = Some(value);
                continue;
            }
            let k = match &out {
                MapRef::String(_) => {
                    let mut s = Vec::new();
                    if !self.unmarshal(key, Out::String(&mut s))? {
                        continue;
                    }
                    GoValue::String(s)
                }
                MapRef::Any(_) => {
                    let mut v = GoValue::Nil;
                    if !self.unmarshal(key, Out::Iface(&mut v))? {
                        continue;
                    }
                    v
                }
            };
            if let Some(fields) = &mut merged_fields {
                if get_possibly_unhashable_key(fields, &k)? {
                    continue;
                }
                set_possibly_unhashable_key(fields, k.clone())?;
            }
            if k.unhashable_type().is_some() {
                return Err(Fail(format!("invalid map key: {}", sharp_v(&k))));
            }
            let mut e = GoValue::Nil;
            let good = self.unmarshal(value, Out::Iface(&mut e))?;
            let absent = match (&out, &k) {
                (MapRef::String(m), GoValue::String(s)) => !m.contains(s),
                (MapRef::Any(m), k) => m.position(k).is_none(),
                _ => true,
            };
            if good || (self.tree.is_null(value) && absent) {
                match (&mut out, k) {
                    (MapRef::String(m), GoValue::String(s)) => m.set(s, e),
                    (MapRef::Any(m), k) => m.set(k, e),
                    // A string map's keys are strings.
                    (MapRef::String(_), _) => {}
                }
            }
        }
        self.merged_fields = merged_fields;
        if let Some(merge) = merge_node {
            self.merge(n, merge, out)?;
        }
        Ok(())
    }

    /// `d.merge(parent, merge, out)`.
    fn merge(&mut self, parent: NodeId, merge: NodeId, mut out: MapRef<'_>) -> Decode<()> {
        let fresh = self.merged_fields.is_none();
        if fresh {
            self.merged_fields = Some(Vec::new());
            let content = &self.tree.node(parent).content;
            for &key in content.iter().step_by(2) {
                let mut k = GoValue::Nil;
                if self.unmarshal(key, Out::Iface(&mut k))? {
                    if let Some(fields) = &mut self.merged_fields {
                        set_possibly_unhashable_key(fields, k)?;
                    }
                }
            }
        }
        let want_map = || Fail("map merge requires map or sequence of maps as the value".to_owned());
        let m = self.tree.node(merge);
        match m.kind {
            Kind::Mapping => {
                self.unmarshal(merge, Out::Map(reborrow(&mut out)))?;
            }
            Kind::Alias => {
                if m.alias.is_some_and(|a| self.tree.node(a).kind != Kind::Mapping) {
                    return Err(want_map());
                }
                self.unmarshal(merge, Out::Map(reborrow(&mut out)))?;
            }
            Kind::Sequence => {
                for &ni in &m.content {
                    let node = self.tree.node(ni);
                    if node.kind == Kind::Alias {
                        if node.alias.is_some_and(|a| self.tree.node(a).kind != Kind::Mapping) {
                            return Err(want_map());
                        }
                    } else if node.kind != Kind::Mapping {
                        return Err(want_map());
                    }
                    self.unmarshal(ni, Out::Map(reborrow(&mut out)))?;
                }
            }
            _ => return Err(want_map()),
        }
        if fresh {
            self.merged_fields = None;
        }
        Ok(())
    }

    /// `d.terror(n, tag, out)`.
    fn terror(&mut self, n: NodeId, tag: &str, out_type: &str) {
        let node = self.tree.node(n);
        let tag = if node.tag.is_empty() { tag } else { node.tag.as_str() };
        let v = node.value.as_bytes();
        let mut value = Vec::new();
        if tag != SEQ_TAG && tag != MAP_TAG {
            value.extend_from_slice(b" `");
            if v.len() > 10 {
                value.extend_from_slice(&v[..7]);
                value.extend_from_slice(b"...");
            } else {
                value.extend_from_slice(v);
            }
            value.push(b'`');
        } else {
            // yaml.v3 appends a sequence's or mapping's value unquoted.
            value.extend_from_slice(v);
        }
        self.terrors.push(format!(
            "line {}: cannot unmarshal {}{} into {}",
            node.line,
            short_tag(tag),
            lossy_per_byte(&value),
            out_type
        ));
    }
}

fn reborrow<'a>(m: &'a mut MapRef<'_>) -> MapRef<'a> {
    match m {
        MapRef::String(m) => MapRef::String(m),
        MapRef::Any(m) => MapRef::Any(m),
    }
}

/// The runtime panic of hashing a map or slice key, which
/// `get/setPossiblyUnhashableKey` turn into `failf("%v", err)`.
fn check_hashable(key: &GoValue) -> Decode<()> {
    match key.unhashable_type() {
        Some(t) => Err(Fail(format!("runtime error: hash of unhashable type {t}"))),
        None => Ok(()),
    }
}

/// `getPossiblyUnhashableKey`.
fn get_possibly_unhashable_key(fields: &[GoValue], key: &GoValue) -> Decode<bool> {
    check_hashable(key)?;
    Ok(fields.iter().any(|f| f.go_eq(key)))
}

/// `setPossiblyUnhashableKey(m, key, true)`.
fn set_possibly_unhashable_key(fields: &mut Vec<GoValue>, key: GoValue) -> Decode<()> {
    check_hashable(&key)?;
    match fields.iter().position(|f| f.go_eq(&key)) {
        Some(i) => fields[i] = key,
        None => fields.push(key),
    }
    Ok(())
}

/// Text with each byte that is not part of valid UTF-8 shown as U+FFFD, as
/// `encoding/json` writes a Go string (the error texts carry Go's bytes).
pub(crate) fn lossy_per_byte(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        out.extend(std::iter::repeat_n('\u{FFFD}', chunk.invalid().len()));
    }
    out
}

/// Text with each run of bytes that are not valid UTF-8 replaced by one
/// U+FFFD, as Go's `strings.ToValidUTF8(s, "�")`.
pub(crate) fn lossy_runs(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut in_run = false;
    for chunk in bytes.utf8_chunks() {
        if !chunk.valid().is_empty() {
            out.push_str(chunk.valid());
            in_run = false;
        }
        if !chunk.invalid().is_empty() && !in_run {
            out.push('\u{FFFD}');
            in_run = true;
        }
    }
    out
}
