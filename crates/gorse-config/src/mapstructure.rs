// Ports: go-viper/mapstructure v2.5.0 mapstructure.go (decode, decodeString, decodeInt, decodeUint, decodeBool, decodeFloat, decodeSlice, decodeStruct, decodeStructFromMap), errors.go, decode_hooks.go (StringToTimeDurationHookFunc, ComposeDecodeHookFunc) and config/config.go (StringToFeedbackTypeHookFunc) @ add21b5
//
// Derived from go-viper/mapstructure: Copyright (c) 2013 Mitchell Hashimoto.
// Use of this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/MAPSTRUCTURE-LICENSE.

//! mapstructure as `viper.Unmarshal` configures it for Gorse: weakly typed
//! input, the `mapstructure` tag, `strings.EqualFold` key matching, and the
//! `time.Duration` and feedback type expression decode hooks.
//!
//! Go walks the target with reflection; here each target type implements
//! [`Decode`], and struct types list their fields with [`decode_struct`].
//! Errors are collected as mapstructure joins them: one line per failing leaf,
//! `'<field path>' <message>`, in field order.

use gorse_common::expression::{ExprType, FeedbackTypeExpression};
use gorse_common::gostrconv::{self, NumError};
use gorse_common::gotime;
use gorse_common::gounicode::equal_fold;

use crate::value::{Kind, Map, Value};

/// The leaf errors of one decode, in the order mapstructure reports them.
pub type Errors = Vec<String>;

/// `viper.Unmarshal`'s error: mapstructure's joined errors under its header.
pub fn error_text(errs: &[String]) -> String {
    format!(
        "decoding failed due to the following error(s):\n\n{}",
        errs.join("\n")
    )
}

/// A Go type mapstructure can decode into.
pub trait Decode {
    /// The type as Go's `reflect.Type.String()` prints it, for messages.
    fn go_type() -> String
    where
        Self: Sized;

    /// mapstructure's decode of a non-nil `input` into `self`, after the decode
    /// hooks. Errors are pushed to `errs`.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors);
}

/// mapstructure's `decode`: a nil input leaves `out` as it is.
pub fn decode<T: Decode>(name: &str, input: &Value, out: &mut T, errs: &mut Errors) {
    if matches!(input, Value::Nil) {
        return;
    }
    out.decode_value(name, input, errs);
}

/// A number or boolean as mapstructure's `getKind` groups it.
enum Basic<'a> {
    Int(i64),
    Uint(u64),
    Float(f64),
    Bool(bool),
    String(&'a str),
    Other,
}

fn basic(input: &Value) -> Basic<'_> {
    match input {
        Value::Int(i) | Value::Int64(i) | Value::Duration(i) => Basic::Int(*i),
        Value::Uint64(u) => Basic::Uint(*u),
        Value::Float64(f) => Basic::Float(*f),
        Value::Bool(b) => Basic::Bool(*b),
        Value::String(s) => Basic::String(s),
        _ => Basic::Other,
    }
}

fn unconvertible(name: &str, expected: &str, input: &Value) -> String {
    format!(
        "'{name}' expected type '{expected}', got unconvertible type '{}'",
        input.type_name()
    )
}

/// `ParseError` around mapstructure's `wrapStrconvNumError`, which drops the
/// quoted input from strconv's message.
fn parse_error(name: &str, expected: &str, err: &NumError) -> String {
    format!(
        "'{name}' cannot parse value as '{expected}': strconv.{}: {}",
        err.func, err.err
    )
}

/// Go's `int64(f)` on amd64 (`CVTTSD2SQ`): NaN and values out of range give
/// the "integer indefinite" `math.MinInt64`, not Rust's saturation.
pub(crate) fn go_f64_to_i64(f: f64) -> i64 {
    if f.is_nan() || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) {
        i64::MIN
    } else {
        f as i64
    }
}

/// Go's `uint64(f)` on amd64: below 2^63 the signed conversion (negative
/// values wrap), otherwise the signed conversion of `f - 2^63` with the top bit
/// set, so NaN and values out of range give 2^63.
pub(crate) fn go_f64_to_u64(f: f64) -> u64 {
    const CUTOFF: f64 = 9_223_372_036_854_775_808.0;
    if f < CUTOFF {
        go_f64_to_i64(f) as u64
    } else {
        go_f64_to_i64(f - CUTOFF) as u64 | 1 << 63
    }
}

/// Go's `strconv.ParseBool`, as a [`NumError`] on failure.
fn parse_bool(s: &str) -> Result<bool, NumError> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(NumError {
            func: "ParseBool",
            num: s.as_bytes().to_vec(),
            err: gostrconv::NumErrorKind::Syntax,
        }),
    }
}

impl Decode for String {
    fn go_type() -> String {
        "string".into()
    }

    /// `decodeString`.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        match basic(input) {
            Basic::String(s) => s.clone_into(self),
            Basic::Bool(b) => *self = if b { "1" } else { "0" }.into(),
            Basic::Int(i) => *self = gostrconv::format_int(i, 10),
            Basic::Uint(u) => *self = gostrconv::format_uint(u, 10),
            Basic::Float(f) => *self = gostrconv::format_float(f, b'f', -1, 64),
            // A `[]uint8` would convert, but no decoder makes one.
            Basic::Other => errs.push(unconvertible(name, "string", input)),
        }
    }
}

/// `decodeInt` into an `int`-kind Go type named `go_type`.
fn decode_int(name: &str, go_type: &str, input: &Value, out: &mut i64, errs: &mut Errors) {
    match basic(input) {
        Basic::Int(i) => *out = i,
        Basic::Uint(u) => *out = u as i64,
        Basic::Float(f) => *out = go_f64_to_i64(f),
        Basic::Bool(b) => *out = i64::from(b),
        Basic::String(s) => {
            let s = if s.is_empty() { "0" } else { s };
            match gostrconv::parse_int(s, 0, 64) {
                (i, None) => *out = i,
                (_, Some(err)) => errs.push(parse_error(name, go_type, &err)),
            }
        }
        Basic::Other => errs.push(unconvertible(name, go_type, input)),
    }
}

impl Decode for i64 {
    /// Go's `int`, 64 bits on the platforms Gorse ships for.
    fn go_type() -> String {
        "int".into()
    }

    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        decode_int(name, "int", input, self, errs);
    }
}

impl Decode for gotime::Duration {
    fn go_type() -> String {
        "time.Duration".into()
    }

    /// `StringToTimeDurationHookFunc`, then `decodeInt`. The hook's error goes
    /// through `wrapTimeParseDurationError`: an unknown unit loses the unit
    /// and the input, any other `time:` message loses everything after its
    /// last space.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        if let Value::String(s) = input {
            match gotime::parse_duration(s.as_bytes()) {
                Ok(d) => *self = d,
                Err(err) => {
                    let text = err.to_string();
                    let text = if text.starts_with("time: unknown unit ") {
                        "time: unknown unit"
                    } else if text.starts_with("time: ") {
                        &text[..text.rfind(' ').unwrap_or(text.len())]
                    } else {
                        &text
                    };
                    errs.push(format!("'{name}' {text}"));
                }
            }
            return;
        }
        decode_int(name, "time.Duration", input, &mut self.0, errs);
    }
}

impl Decode for u64 {
    /// Go's `uint`.
    fn go_type() -> String {
        "uint".into()
    }

    /// `decodeUint`; weakly typed input lets negative numbers wrap.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        match basic(input) {
            Basic::Int(i) => *self = i as u64,
            Basic::Uint(u) => *self = u,
            Basic::Float(f) => *self = go_f64_to_u64(f),
            Basic::Bool(b) => *self = u64::from(b),
            Basic::String(s) => {
                let s = if s.is_empty() { "0" } else { s };
                match gostrconv::parse_uint(s, 0, 64) {
                    (u, None) => *self = u,
                    (_, Some(err)) => errs.push(parse_error(name, "uint", &err)),
                }
            }
            Basic::Other => errs.push(unconvertible(name, "uint", input)),
        }
    }
}

impl Decode for f64 {
    fn go_type() -> String {
        "float64".into()
    }

    /// `decodeFloat`.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        match basic(input) {
            Basic::Int(i) => *self = i as f64,
            Basic::Uint(u) => *self = u as f64,
            Basic::Float(f) => *self = f,
            Basic::Bool(b) => *self = if b { 1.0 } else { 0.0 },
            Basic::String(s) => {
                let s = if s.is_empty() { "0" } else { s };
                match gostrconv::parse_float(s, 64) {
                    (f, None) => *self = f,
                    (_, Some(err)) => errs.push(parse_error(name, "float64", &err)),
                }
            }
            Basic::Other => errs.push(unconvertible(name, "float64", input)),
        }
    }
}

impl Decode for bool {
    fn go_type() -> String {
        "bool".into()
    }

    /// `decodeBool`: a string `strconv.ParseBool` rejects is an error unless
    /// it is empty, which is false.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        match basic(input) {
            Basic::Bool(b) => *self = b,
            Basic::Int(i) => *self = i != 0,
            Basic::Uint(u) => *self = u != 0,
            Basic::Float(f) => *self = f != 0.0,
            Basic::String(s) => match parse_bool(s) {
                Ok(b) => *self = b,
                Err(_) if s.is_empty() => *self = false,
                Err(err) => errs.push(parse_error(name, "bool", &err)),
            },
            Basic::Other => errs.push(unconvertible(name, "bool", input)),
        }
    }
}

/// A Go slice; `None` is nil.
impl<T: Decode + Default> Decode for Option<Vec<T>> {
    fn go_type() -> String {
        format!("[]{}", T::go_type())
    }

    /// `decodeSlice` with weakly typed input: an empty map becomes an empty
    /// slice, any other value that is not a slice becomes a slice of itself,
    /// and a nil slice leaves the target as it is. Elements are decoded into
    /// the target's existing elements, as Go reuses its backing array.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        let items: &[Value] = match input {
            Value::Slice(items) => items,
            Value::Strings(None) => return,
            Value::Strings(Some(strings)) => {
                let items: Vec<Value> = strings.iter().cloned().map(Value::String).collect();
                return self.decode_value(name, &Value::Slice(items), errs);
            }
            Value::Map(m) if m.is_empty() => {
                *self = Some(Vec::new());
                return;
            }
            Value::AnyMap(m) if m.is_empty() => {
                *self = Some(Vec::new());
                return;
            }
            other => return self.decode_value(name, &Value::Slice(vec![other.clone()]), errs),
        };
        let mut out = match self.take() {
            None => Vec::new(),
            Some(mut v) => {
                v.truncate(items.len());
                v
            }
        };
        for (i, item) in items.iter().enumerate() {
            if out.len() <= i {
                out.push(T::default());
            }
            decode(&format!("{name}[{i}]"), item, &mut out[i], errs);
        }
        *self = Some(out);
    }
}

/// The keys of a map being decoded into a struct, as `decodeStructFromMap`
/// looks fields up in it.
pub struct Fields<'a> {
    name: &'a str,
    entries: Vec<(Option<&'a str>, &'a Value)>,
}

impl<'a> Fields<'a> {
    /// The value for the field tagged `tag` and the field's path: the key
    /// equal to the tag, or else the first string key that `strings.EqualFold`
    /// matches. Go tries the keys in its random map order; here they are tried
    /// in the map's order (`PORTING_NOTES.md`, "Configuration").
    pub fn get(&self, tag: &str) -> Option<(String, &'a Value)> {
        let value = self
            .entries
            .iter()
            .find(|(k, _)| *k == Some(tag))
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|(k, _)| k.is_some_and(|k| equal_fold(k, tag)))
            })
            .map(|(_, v)| *v)?;
        let path = if self.name.is_empty() {
            tag.to_owned()
        } else {
            format!("{}.{tag}", self.name)
        };
        Some((path, value))
    }

    /// Decodes the field tagged `tag` into `out`, if the map has it.
    pub fn decode<T: Decode>(&self, tag: &str, out: &mut T, errs: &mut Errors) {
        if let Some((path, value)) = self.get(tag) {
            decode(&path, value, out, errs);
        }
    }
}

/// `decodeStruct` for a struct type named `go_type`: `fields` decodes each of
/// the struct's fields from the map it is given, in field order.
///
/// A value of the same struct type is assigned as it is; here that is a
/// decode of its fields into a zero value, which gives the same struct since
/// they have the target's own types. Another struct goes through the map of
/// its exported fields, as `decodeMapFromStruct` makes it.
pub fn decode_struct<T: Default>(
    out: &mut T,
    name: &str,
    go_type: &str,
    input: &Value,
    errs: &mut Errors,
    fields: impl FnOnce(&mut T, &Fields<'_>, &mut Errors),
) {
    let struct_map;
    let entries: Vec<(Option<&str>, &Value)> = match input {
        Value::Map(m) => m.iter().map(|(k, v)| (Some(k), v)).collect(),
        Value::AnyMap(m) => m
            .iter()
            .map(|(k, v)| match k {
                Value::String(k) => (Some(k.as_str()), v),
                _ => (None, v),
            })
            .collect(),
        Value::Struct(ty, m) if *ty == go_type => {
            *out = T::default();
            m.iter().map(|(k, v)| (Some(k), v)).collect()
        }
        other if other.kind() == Kind::Struct => {
            struct_map = other.struct_fields().unwrap_or_else(Map::new);
            struct_map.iter().map(|(k, v)| (Some(k), v)).collect()
        }
        other => {
            errs.push(format!(
                "'{name}' expected a map or struct, got \"{}\"",
                other.kind().name()
            ));
            return;
        }
    };
    fields(out, &Fields { name, entries }, errs);
}

impl Decode for FeedbackTypeExpression {
    fn go_type() -> String {
        "expression.FeedbackTypeExpression".into()
    }

    /// `StringToFeedbackTypeHookFunc` turns a string into an expression with
    /// `FromString`; anything else is decoded as the struct, whose fields
    /// have no tags.
    fn decode_value(&mut self, name: &str, input: &Value, errs: &mut Errors) {
        if let Value::String(s) = input {
            let mut expr = FeedbackTypeExpression::default();
            match expr.from_string(s) {
                Ok(()) => *self = expr,
                Err(err) => errs.push(format!("'{name}' {err}")),
            }
            return;
        }
        decode_struct(
            self,
            name,
            "expression.FeedbackTypeExpression",
            input,
            errs,
            |out, fields, errs| {
                fields.decode("FeedbackType", &mut out.feedback_type, errs);
                if let Some((path, value)) = fields.get("ExprType") {
                    let mut raw = expr_type_to_int(out.expr_type);
                    if !matches!(value, Value::Nil) {
                        decode_int(&path, "expression.ExprType", value, &mut raw, errs);
                    }
                    match expr_type_from_int(raw) {
                        Some(t) => out.expr_type = t,
                        None => errs.push(format!(
                            "'{path}' expression.ExprType {raw} is not one of the five comparisons"
                        )),
                    }
                }
                fields.decode("Value", &mut out.value, errs);
            },
        );
    }
}

fn expr_type_to_int(t: ExprType) -> i64 {
    match t {
        ExprType::None => 0,
        ExprType::Less => 1,
        ExprType::LessOrEqual => 2,
        ExprType::Greater => 3,
        ExprType::GreaterOrEqual => 4,
    }
}

/// Go's `ExprType` is an `int` that can hold any value; the Rust enum holds
/// the five Go names (`PORTING_NOTES.md`, "Configuration").
fn expr_type_from_int(i: i64) -> Option<ExprType> {
    Some(match i {
        0 => ExprType::None,
        1 => ExprType::Less,
        2 => ExprType::LessOrEqual,
        3 => ExprType::Greater,
        4 => ExprType::GreaterOrEqual,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_conversions_match_go_on_amd64() {
        // Printed by Go 1.27 on linux/amd64 for int64(f) and uint64(f).
        let cases: &[(f64, i64, u64)] = &[
            (0.0, 0, 0),
            (1.9, 1, 1),
            (-1.9, -1, u64::MAX),
            (1e19, i64::MIN, 10_000_000_000_000_000_000),
            (-1e19, i64::MIN, 1 << 63),
            (9.3e18, i64::MIN, 9_300_000_000_000_000_000),
            (1.8e19, i64::MIN, 18_000_000_000_000_000_000),
            (1.85e19, i64::MIN, 1 << 63),
            (1e30, i64::MIN, 1 << 63),
            (f64::INFINITY, i64::MIN, 1 << 63),
            (f64::NEG_INFINITY, i64::MIN, 1 << 63),
            (f64::NAN, i64::MIN, 1 << 63),
            (-9_223_372_036_854_775_808.0, i64::MIN, 1 << 63),
            (18_446_744_073_709_551_615.0, i64::MIN, 1 << 63),
        ];
        for &(f, i, u) in cases {
            assert_eq!(go_f64_to_i64(f), i, "int64({f})");
            assert_eq!(go_f64_to_u64(f), u, "uint64({f})");
        }
    }

    #[test]
    fn parse_bool_matches_go() {
        for s in ["1", "t", "T", "TRUE", "true", "True"] {
            assert_eq!(parse_bool(s), Ok(true));
        }
        for s in ["0", "f", "F", "FALSE", "false", "False"] {
            assert_eq!(parse_bool(s), Ok(false));
        }
        for s in ["", "yes", "tRUE", " true"] {
            assert!(parse_bool(s).is_err(), "{s:?}");
        }
    }
}
