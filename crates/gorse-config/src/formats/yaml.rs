// Ports: go.yaml.in/yaml/v3 v3.0.4 Unmarshal into map[string]interface{} (yaml.go unmarshal, handleErr, TypeError), as spf13/viper v1.21.0 internal/encoding/yaml calls it @ add21b5
//
// Derived from go.yaml.in/yaml/v3: Copyright 2011-2016 Canonical Ltd,
// licensed under the Apache License, Version 2.0, and (its libyaml port)
// Copyright (c) 2006-2011 Kirill Simonov, MIT licensed; see
// crates/gorse-config/licenses/YAML-V3-LICENSE.

//! viper's YAML codec: `yaml.Unmarshal(b, &v)` into a `map[string]any`.
//!
//! The pipeline is yaml.v3's: libyaml's reader ([`reader`]), its scanner
//! (the `yaml-rust2` scanner behind an adapter, [`scanner`]) and parser
//! ([`parser`]), then yaml.v3's composer ([`composer`]), decoder
//! ([`decoder`]) and tag resolution ([`resolve`]). Only the first document
//! of a stream is decoded. The differences from Go are listed in
//! `porting/notes/WP-1-15-yaml.md`.

mod composer;
mod decoder;
mod gofmt;
mod parser;
mod reader;
mod resolve;
mod scanner;

use crate::value::{Map, Value};

use composer::{ComposeError, Composer};
use decoder::{GoValue, StringMap, decode_root, lossy_runs};

/// viper's YAML codec: `yaml.Unmarshal(b, &v)` into a `map[string]any`.
///
/// Returns the entries the codec leaves in viper's (empty) map, in the
/// order their keys first appear, or yaml.v3's error text (which viper
/// wraps in `While parsing config: `). A null document leaves the map
/// empty. Strings are Go strings, so bytes that are not UTF-8 (only a
/// `!!binary` scalar makes them) are replaced by U+FFFD, one per run.
pub fn decode(input: &[u8]) -> Result<Map, String> {
    // newParser: an empty input is read as a line break.
    let input: &[u8] = if input.is_empty() { b"\n" } else { input };
    let scanner = scanner::Scanner::new(reader::decode(input));
    let (tree, root) = match Composer::new(parser::Parser::new(scanner)).compose() {
        Ok(composed) => composed,
        Err(ComposeError::Syntax(e)) => return Err(format!("yaml: {}", e.message())),
        Err(ComposeError::Fail(message)) => return Err(format!("yaml: {message}")),
    };
    let mut out = StringMap::default();
    if let Some(root) = root {
        match decode_root(&tree, root, &mut out) {
            Ok(outcome) if outcome.terrors.is_empty() => {}
            Ok(outcome) => return Err(format!("yaml: unmarshal errors:\n  {}", outcome.terrors.join("\n  "))),
            Err(decoder::Fail(message)) => return Err(format!("yaml: {message}")),
        }
    }
    Ok(string_map(out))
}

fn string_map(m: StringMap) -> Map {
    Map::from(m.into_entries().into_iter().map(|(k, v)| (lossy_runs(&k), value(v))).collect::<Vec<_>>())
}

/// The [`Value`] of a decoded Go value.
fn value(v: GoValue) -> Value {
    // A mapping or sequence nests as deep as the input (10000 levels at most),
    // so the recursion grows the stack as the composer's does.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || match v {
        GoValue::Nil => Value::Nil,
        GoValue::Bool(b) => Value::Bool(b),
        GoValue::Int(i) => Value::Int(i),
        GoValue::Uint64(u) => Value::Uint64(u),
        GoValue::Float64(f) => Value::Float64(f),
        GoValue::String(s) => Value::String(lossy_runs(&s)),
        GoValue::Time(t) => Value::Time(t),
        GoValue::Slice(items) => Value::Slice(items.into_iter().map(value).collect()),
        GoValue::StringMap(m) => Value::Map(string_map(m)),
        GoValue::AnyMap(m) => Value::AnyMap(m.into_entries().into_iter().map(|(k, v)| (value(k), value(v))).collect()),
    })
}
