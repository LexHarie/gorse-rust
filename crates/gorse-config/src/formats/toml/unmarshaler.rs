// Ports: unmarshaler.go (Unmarshal into a map[string]interface{}) @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! go-toml's decoder, for the one target viper gives it: a non-nil
//! `map[string]interface{}`.
//!
//! Go's decoder walks the target with `reflect`. Into a `map[string]interface{}`
//! it only ever meets four kinds of `reflect.Value`: the map itself, the
//! `interface{}` variables that hold its entries and the elements of arrays,
//! the `[]interface{}` of an array of tables, and scalar values; [`Target`]
//! names them. The struct, pointer, array and typed-map branches never run
//! and are not ported, nor is the state only they use (`skipUntilTable`,
//! `errorContext`, `arrayIndexes`, strict mode, `Unmarshaler`).
//!
//! Go's handlers return a replacement for the value they were given (a new
//! map for a nil interface, a longer slice after an append), which the caller
//! stores. Here they change the value in place. The results are the same:
//! Go's maps and slice elements are references, every replacement is stored
//! as soon as the handler returns, and an error abandons the whole map.
//!
//! The decoder interleaves with the parser as in Go: each expression is
//! checked by the [`SeenTracker`] and decoded before the next one is parsed,
//! so of two problems the first one in the document is reported, whether it
//! is a syntax error or not.

use std::borrow::Cow;

use super::ast::{Builder, Iter, Kind};
use super::decode::{
    parse_date_time, parse_float, parse_integer, parse_local_date, parse_local_date_time,
    parse_local_time,
};
use super::errors::{ParserError, new_parser_error};
use super::parser::Parser;
use super::tracker::SeenTracker;
use super::with_stack;
use crate::value::{Map, Value};

/// An error of the decoder: a `*unstable.ParserError`, which `Unmarshal`
/// wraps into a `DecodeError`, or any other Go error, by its text.
#[derive(Debug)]
pub(crate) enum DecoderError<'a> {
    /// `*unstable.ParserError`.
    Parser(ParserError<'a>),
    /// Any other error.
    Other(String),
}

impl<'a> From<ParserError<'a>> for DecoderError<'a> {
    fn from(err: ParserError<'a>) -> Self {
        DecoderError::Parser(err)
    }
}

type Result<'a, T = ()> = std::result::Result<T, DecoderError<'a>>;

/// A `reflect.Value` the decoder works on.
#[derive(Debug)]
enum Target<'v> {
    /// `reflect.Interface`: an `interface{}` variable; `Value::Nil` is a nil
    /// interface.
    Interface(&'v mut Value),
    /// A value of the Go type its variant names: `reflect.Map` for a
    /// `Value::Map`, `reflect.Slice` for a `Value::Slice`, and so on.
    Concrete(&'v mut Value),
}

impl Target<'_> {
    fn reborrow(&mut self) -> Target<'_> {
        match self {
            Target::Interface(v) => Target::Interface(v),
            Target::Concrete(v) => Target::Concrete(v),
        }
    }
}

/// `handlerFn`: what `handleKeyPart` does with the value of a key part.
#[derive(Clone, Copy, Debug)]
enum NextFn {
    /// `handleTable`.
    Table,
    /// `handleArrayTableCollection`.
    ArrayTableCollection,
}

/// `valueMakerFn`: what `handleKeyPart` puts under a new key part.
#[derive(Clone, Copy, Debug)]
enum MakeFn {
    /// `makeMapStringInterface`.
    Map,
    /// `makeSliceInterface`.
    Slice,
}

impl MakeFn {
    fn make(self) -> Value {
        match self {
            MakeFn::Map => Value::Map(Map::new()),
            MakeFn::Slice => Value::Slice(Vec::with_capacity(16)),
        }
    }
}

/// Go's `string(b)` of bytes the parser has checked to be UTF-8.
fn go_string(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The document bytes of a node's data. Numbers, dates and unquoted keys
/// always borrow from the document; Go's `SubsliceOffset` panics on a
/// highlight from anywhere else.
fn document_bytes<'a>(data: &Cow<'a, [u8]>) -> &'a [u8] {
    match data {
        Cow::Borrowed(b) => b,
        Cow::Owned(_) => panic!("subslice address is not within the document"),
    }
}

/// `decoder`.
#[derive(Debug)]
pub(crate) struct Decoder<'a> {
    // Which parser instance in use for this decoding session.
    p: Parser<'a>,

    // Flag indicating that the current expression is stashed. If set to true,
    // calling nextExpr will not actually pull a new expression but turn off
    // the flag instead.
    stashed_expr: bool,

    // Flag indicating that the current array/slice table should be cleared
    // because it is the first encounter of an array table.
    clear_array_table: bool,

    // Tracks keys that have been seen, with which type.
    seen: SeenTracker<'a>,
}

impl<'a> Decoder<'a> {
    /// A decoder of the document b (`Unmarshal`'s `d.p.Reset(data)`).
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Decoder {
            p: Parser::new(b),
            stashed_expr: false,
            clear_array_table: false,
            seen: SeenTracker::default(),
        }
    }

    /// The parser, for the document and the nodes.
    pub(crate) fn parser(&self) -> &Parser<'a> {
        &self.p
    }

    fn builder(&self) -> &Builder<'a> {
        self.p.builder()
    }

    fn expr(&self) -> usize {
        self.p.expression()
    }

    fn next_expr(&mut self) -> bool {
        if self.stashed_expr {
            self.stashed_expr = false;
            return true;
        }
        self.p.next_expression()
    }

    fn stash_expr(&mut self) {
        self.stashed_expr = true;
    }

    /// `fromParser`: decodes the whole document into root, a
    /// `map[string]interface{}`.
    pub(crate) fn decode(&mut self, root: &mut Value) -> Result<'a> {
        while self.next_expr() {
            let expr = self.expr();
            self.handle_root_expression(expr, root)?;
        }

        match self.p.error() {
            Some(err) => Err(DecoderError::Parser(err.clone())),
            None => Ok(()),
        }
    }

    // Rules for the unmarshal code:
    //
    // - The stack is used to keep track of which values need to be set where.
    // - handle* functions <=> switch on a given unstable.Kind.
    // - unmarshalX* functions need to unmarshal a node of kind X.
    // - An "object" is either a struct or a map.

    fn handle_root_expression(&mut self, expr: usize, v: &mut Value) -> Result<'a> {
        let first = self
            .seen
            .check_expression(self.p.builder(), expr)
            .map_err(DecoderError::Other)?;

        match self.builder().node_at(expr).kind {
            Kind::KeyValue => self.handle_key_value(expr, Target::Concrete(v)),
            Kind::Table => {
                let key = self.builder().key(expr);
                self.handle_table(key, Target::Concrete(v))
            }
            Kind::ArrayTable => {
                self.clear_array_table = first;
                let key = self.builder().key(expr);
                self.handle_array_table(key, Target::Concrete(v))
            }
            kind => panic!("parser should not permit expression of kind {kind} at document root"),
        }
    }

    fn handle_array_table(&mut self, mut key: Iter, v: Target<'_>) -> Result<'a> {
        if key.next(self.builder()) {
            return self.handle_array_table_part(key, v);
        }
        self.handle_key_values(v)
    }

    fn handle_array_table_collection_last(&mut self, key: Iter, v: Target<'_>) -> Result<'a> {
        match v {
            Target::Interface(elem) => {
                match elem {
                    Value::Nil => *elem = Value::Slice(Vec::with_capacity(16)),
                    // Go copies the slice it read from the interface into a
                    // settable one first.
                    Value::Slice(s) if self.clear_array_table && !s.is_empty() => {
                        s.clear();
                        self.clear_array_table = false;
                    }
                    _ => {}
                }
                self.handle_array_table_collection_last(key, Target::Concrete(elem))
            }
            Target::Concrete(Value::Slice(s)) => {
                if self.clear_array_table && !s.is_empty() {
                    // Go calls SetLen on the slice it read from the map,
                    // which cannot be set. (The tracker says an array table
                    // is new only when the map has no value for it.)
                    panic!("reflect: reflect.Value.SetLen using unaddressable value");
                }
                // The element type is interface{}: a new
                // map[string]interface{}, filled, then appended.
                s.push(Value::Map(Map::new()));
                let elem = s.len() - 1;
                self.handle_array_table(key, Target::Concrete(&mut s[elem]))
            }
            Target::Concrete(other) => Err(DecoderError::Other(format!(
                "toml: cannot decode TOML array table into a Go value of type {}",
                other.type_name()
            ))),
        }
    }

    /// `handleArrayTableCollection`: when parsing an array table
    /// expression, each part of the key needs to be evaluated like a normal
    /// key, but if it returns a collection, it also needs to point to the
    /// last element of the collection. Unless it is the last part of the
    /// key, then it needs to create a new element at the end.
    fn handle_array_table_collection(&mut self, key: Iter, v: Target<'_>) -> Result<'a> {
        if key.is_last(self.builder()) {
            return self.handle_array_table_collection_last(key, v);
        }

        match v {
            Target::Concrete(Value::Slice(s)) => {
                let Some(elem) = s.last_mut() else {
                    panic!("reflect: slice index out of range");
                };
                self.handle_array_table(key, Target::Interface(elem))
            }
            v => self.handle_array_table(key, v),
        }
    }

    fn handle_key_part(
        &mut self,
        key: Iter,
        v: Target<'_>,
        next_fn: NextFn,
        make_fn: MakeFn,
    ) -> Result<'a> {
        with_stack(|| self.handle_key_part_inner(key, v, next_fn, make_fn))
    }

    fn handle_key_part_inner(
        &mut self,
        key: Iter,
        v: Target<'_>,
        next_fn: NextFn,
        make_fn: MakeFn,
    ) -> Result<'a> {
        // First, dispatch over v to make sure it is a valid object.
        // There is no guarantee over what it could be.
        match v {
            Target::Concrete(Value::Map(m)) => {
                // Create the key for the map element.
                let mk = go_string(&self.builder().node_at(key.node()).data);

                match m.get_mut(&mk) {
                    // If there is no value in the map, create a new one
                    // according to the map type: the element type is
                    // interface, so either a map[string]interface{} or a
                    // []interface{} depending on whether this is the last
                    // part of the array table key.
                    None => m.insert(mk.clone(), make_fn.make()),
                    // An interface holding nothing.
                    Some(mv @ Value::Nil) => *mv = make_fn.make(),
                    // Otherwise the value the interface holds.
                    Some(_) => {}
                }
                let Some(mv) = m.get_mut(&mk) else {
                    unreachable!("the key was just set");
                };

                match next_fn {
                    NextFn::Table => self.handle_table(key, Target::Concrete(mv)),
                    NextFn::ArrayTableCollection => {
                        self.handle_array_table_collection(key, Target::Concrete(mv))
                    }
                }
            }
            Target::Interface(v) => {
                if matches!(v, Value::Nil) {
                    *v = Value::Map(Map::new());
                }

                self.handle_key_part(key, Target::Concrete(v), next_fn, make_fn)
            }
            Target::Concrete(other) => panic!("unhandled part: {}", other.kind().name()),
        }
    }

    /// `handleArrayTablePart`: navigates the value v using the key. It is
    /// only used for the prefix (non-last) parts of an array-table. When
    /// encountering a collection, it should go to the last element.
    fn handle_array_table_part(&mut self, key: Iter, v: Target<'_>) -> Result<'a> {
        let make_fn = if key.is_last(self.builder()) {
            MakeFn::Slice
        } else {
            MakeFn::Map
        };
        self.handle_key_part(key, v, NextFn::ArrayTableCollection, make_fn)
    }

    fn handle_table(&mut self, mut key: Iter, v: Target<'_>) -> Result<'a> {
        if let Target::Concrete(Value::Slice(s)) = v {
            let Some(elem) = s.last_mut() else {
                let data = &self.builder().node_at(key.node()).data;
                return Err(new_parser_error(
                    document_bytes(data),
                    "cannot store a table in a slice",
                )
                .into());
            };
            return self.handle_table(key, Target::Interface(elem));
        }
        if key.next(self.builder()) {
            // Still scoping the key
            return self.handle_table_part(key, v);
        }
        // Done scoping the key.
        // Now handle all the key-value expressions in this table.
        self.handle_key_values(v)
    }

    /// `handleKeyValues`: handles root expressions until the end of the
    /// document or the next non-key-value.
    fn handle_key_values(&mut self, mut v: Target<'_>) -> Result<'a> {
        while self.next_expr() {
            let expr = self.expr();
            if self.builder().node_at(expr).kind != Kind::KeyValue {
                // Stash the expression so that fromParser can just loop and
                // use the right handler.
                self.stash_expr();
                break;
            }

            self.seen
                .check_expression(self.p.builder(), expr)
                .map_err(DecoderError::Other)?;

            self.handle_key_value(expr, v.reborrow())?;
        }
        Ok(())
    }

    fn handle_table_part(&mut self, key: Iter, v: Target<'_>) -> Result<'a> {
        self.handle_key_part(key, v, NextFn::Table, MakeFn::Map)
    }

    /// `handleValue`, for the `interface{}` variable that receives a value
    /// (an interface is no `time.Time` and no `TextUnmarshaler`, so Go's
    /// `tryTextUnmarshaler` declines it).
    fn handle_value(&self, value: usize, v: &mut Value) -> Result<'a> {
        with_stack(|| self.handle_value_inner(value, v))
    }

    fn handle_value_inner(&self, value: usize, v: &mut Value) -> Result<'a> {
        let node = self.builder().node_at(value);
        match node.kind {
            Kind::String => {
                *v = Value::String(go_string(&node.data));
                Ok(())
            }
            Kind::Integer => {
                let i = parse_integer(document_bytes(&node.data))?;
                *v = Value::Int64(i);
                Ok(())
            }
            Kind::Float => {
                let f = parse_float(document_bytes(&node.data))?;
                *v = Value::Float64(f);
                Ok(())
            }
            Kind::Bool => {
                *v = Value::Bool(node.data[0] == b't');
                Ok(())
            }
            Kind::DateTime => {
                let dt = parse_date_time(document_bytes(&node.data))?;
                *v = Value::Time(dt);
                Ok(())
            }
            Kind::LocalDate => {
                let ld = parse_local_date(document_bytes(&node.data))?;
                *v = Value::LocalDate(ld);
                Ok(())
            }
            Kind::LocalTime => {
                let (lt, rest) = parse_local_time(document_bytes(&node.data))?;
                if !rest.is_empty() {
                    return Err(new_parser_error(
                        rest,
                        "extra characters at the end of a local time",
                    )
                    .into());
                }
                *v = Value::LocalTime(lt);
                Ok(())
            }
            Kind::LocalDateTime => {
                let (date, time, rest) = parse_local_date_time(document_bytes(&node.data))?;
                if !rest.is_empty() {
                    return Err(new_parser_error(
                        rest,
                        "extra characters at the end of a local date time",
                    )
                    .into());
                }
                *v = Value::LocalDateTime(date, time);
                Ok(())
            }
            Kind::InlineTable => self.unmarshal_inline_table(value, Target::Interface(v)),
            Kind::Array => self.unmarshal_array(value, Target::Interface(v)),
            kind => panic!("handleValue not implemented for {kind}"),
        }
    }

    fn unmarshal_array(&self, array: usize, v: Target<'_>) -> Result<'a> {
        match v {
            Target::Interface(elem) => {
                match elem {
                    Value::Nil => *elem = Value::Slice(Vec::with_capacity(16)),
                    // Go copies a []interface{} into a settable slice; the
                    // slice case below empties it.
                    Value::Slice(_) => {}
                    // Any other value fails below.
                    _ => {}
                }
                self.unmarshal_array(array, Target::Concrete(elem))
            }
            Target::Concrete(Value::Slice(s)) => {
                s.clear();

                let mut it = self.builder().children(array);
                while it.next(self.builder()) {
                    let n = it.node();

                    let mut elem = Value::Nil;
                    self.handle_value(n, &mut elem)?;

                    s.push(elem);
                }

                Ok(())
            }
            Target::Concrete(other) => Err(DecoderError::Other(format!(
                "toml: cannot decode TOML array into a Go value of type {}",
                other.type_name()
            ))),
        }
    }

    fn unmarshal_inline_table(&self, itable: usize, v: Target<'_>) -> Result<'a> {
        // Make sure v is an initialized object.
        match v {
            Target::Interface(elem) => {
                if matches!(elem, Value::Nil) {
                    *elem = Value::Map(Map::new());
                }
                self.unmarshal_inline_table(itable, Target::Concrete(elem))
            }
            Target::Concrete(v @ Value::Map(_)) => {
                let mut it = self.builder().children(itable);
                while it.next(self.builder()) {
                    let n = it.node();

                    self.handle_key_value(n, Target::Concrete(v))?;
                }

                Ok(())
            }
            Target::Concrete(other) => {
                let raw = self.p.raw(self.builder().node_at(itable).raw);
                Err(new_parser_error(
                    raw,
                    format!(
                        "cannot store inline table in Go type {}",
                        other.kind().name()
                    ),
                )
                .into())
            }
        }
    }

    fn handle_key_value(&self, expr: usize, v: Target<'_>) -> Result<'a> {
        let key = self.builder().key(expr);
        let value = self.builder().value(expr);
        self.handle_key_value_inner(key, value, v)
    }

    fn handle_key_value_inner(&self, mut key: Iter, value: usize, v: Target<'_>) -> Result<'a> {
        if key.next(self.builder()) {
            // Still scoping the key
            return self.handle_key_value_part(key, value, v);
        }
        // Done scoping the key.
        // v is whatever Go value we need to fill.
        match v {
            Target::Interface(v) => self.handle_value(value, v),
            // A key has at least one part, and the map case below passes the
            // last part a fresh interface.
            Target::Concrete(_) => unreachable!("a value is always decoded into an interface"),
        }
    }

    fn handle_key_value_part(&self, key: Iter, value: usize, v: Target<'_>) -> Result<'a> {
        with_stack(|| self.handle_key_value_part_inner(key, value, v))
    }

    fn handle_key_value_part_inner(&self, key: Iter, value: usize, v: Target<'_>) -> Result<'a> {
        // First, dispatch over v to make sure it is a valid object.
        // There is no guarantee over what it could be.
        match v {
            Target::Concrete(Value::Map(m)) => {
                let mk = go_string(&self.builder().node_at(key.node()).data);

                // A new interface{} for a new key and for the last part of
                // the key, which replaces any value there.
                if m.get(&mk).is_none() || key.is_last(self.builder()) {
                    m.insert(mk.clone(), Value::Nil);
                }
                let Some(mv) = m.get_mut(&mk) else {
                    unreachable!("the key was just set");
                };

                self.handle_key_value_inner(key, value, Target::Interface(mv))
            }
            Target::Interface(v) => {
                // Following encoding/json: decoding an object into an
                // interface{}, it needs to always hold a
                // map[string]interface{}. This is for the types to be
                // consistent whether a previous value was set or not.
                if !matches!(v, Value::Map(_)) {
                    *v = Value::Map(Map::new());
                }

                self.handle_key_value_part(key, value, Target::Concrete(v))
            }
            Target::Concrete(other) => Err(DecoderError::Other(format!(
                "unhandled kv part: {}",
                other.kind().name()
            ))),
        }
    }
}
