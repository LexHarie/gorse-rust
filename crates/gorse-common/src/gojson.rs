// Ports: $GOROOT/src/encoding/json/v2_decode.go (Unmarshal, UnmarshalTypeError.Error), v2_scanner.go (transformSyntacticError, syntaxErrorReplacer), v2_inject.go (transformUnmarshalError), jsontext/decode.go (CheckNextValue, checkEOF, consumeValue, consumeObject, consumeArray, consumeString, consumeNumber), jsontext/state.go (maxNestingDepth, appendStackPointer, appendEscapePointerName), internal/jsonwire/decode.go (ConsumeLiteral, ConsumeStringResumable, AppendUnquote, hasEscapedUTF16Prefix, ConsumeNumberResumable, parseHexUint16), internal/jsonwire/wire.go (QuoteRune, NewInvalidCharacterError, NewInvalidEscapeSequenceError, InvalidTextError), v2/arshal_default.go (the slice, map, interface, float, string and bool unmarshalers) @ go1.27.0 (the toolchain add21b5 builds with)
//
// Derived from Go's encoding/json: Copyright 2010 The Go Authors. All rights
// reserved. Use of this source code is governed by a BSD-style license that
// can be found in crates/gorse-common/licenses/GO-LICENSE.

//! Go's `json.Unmarshal` into a `[]any`, a `map[string]any` or an `any`, as
//! the Go 1.27
//! toolchain that builds Gorse runs it: the v1 API on top of the v2
//! implementation (`GOEXPERIMENT=jsonv2` is on by default in Go 1.27), with
//! v1's options.
//!
//! Go does three things, in this order:
//!
//! 1. It checks the whole input against the JSON grammar before decoding
//!    anything. The first syntax error, in input order, is the error; v1
//!    rewrites v2's texts ("unexpected end of JSON input", "invalid
//!    character 'x' looking for beginning of value", ...). Nesting deeper
//!    than 10000 levels is "exceeded max depth".
//! 2. For a `[]any` (or a `map[string]any`), a top-level value that is
//!    neither an array (an object) nor `null` is a type error ("json: cannot
//!    unmarshal string into Go value of type []interface {}"); `null` leaves
//!    the slice (the map) nil.
//! 3. It decodes the value. A number that overflows float64 is a type error
//!    naming the number and where it is ("json: cannot unmarshal number 1e400
//!    into .0 of type float64"); Go keeps decoding and returns the first
//!    such error in input order, which is the one returned here.
//!
//! Strings decode as v1 decodes them: invalid UTF-8 is allowed and each
//! invalid byte becomes U+FFFD, as does each `\u` escape of a surrogate that
//! is not part of a pair. Duplicate object names are allowed and the last
//! one wins, replacing the earlier value whole. Numbers are float64, parsed
//! with Go's `strconv.ParseFloat`.
//!
//! Go's maps have no order. Objects here keep their names in the order the
//! document first gives them, so that callers iterate deterministically; a
//! duplicate name keeps its first position and takes its last value.
//!
//! Not ported: the partial value Go returns along with an error, and the
//! byte offsets inside the errors. Only the error texts reach Gorse.

use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::gostrconv;
use crate::gostrings::Replacer;
use crate::goutf8::{RUNE_ERROR, decode_char, decode_rune, full_rune, is_space, rune_count, runes};

/// Go's `jsontext` `maxNestingDepth`.
const MAX_NESTING_DEPTH: usize = 10000;

/// A decoded JSON value, as Go's `any` holds it: `nil`, `bool`, `float64`,
/// `string`, `[]any` and `map[string]any`.
///
/// Dropping a value never recurses, so values nested as deep as Go allows
/// are safe to drop; the derived `Clone`, `PartialEq` and `Debug` do recurse.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// JSON `null` (Go's `nil`).
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as float64.
    Number(f64),
    /// A string. Decoding always yields valid UTF-8.
    String(String),
    /// An array (Go's `[]any`).
    Array(Vec<Value>),
    /// An object (Go's `map[string]any`), in document order (see the module
    /// docs).
    Object(IndexMap<String, Value>),
}

impl Drop for Value {
    fn drop(&mut self) {
        let mut pending = match self {
            Value::Array(items) => std::mem::take(items),
            Value::Object(members) => std::mem::take(members).into_values().collect(),
            _ => return,
        };
        while let Some(mut v) = pending.pop() {
            match &mut v {
                Value::Array(items) => pending.append(items),
                Value::Object(members) => pending.extend(std::mem::take(members).into_values()),
                _ => {}
            }
        }
    }
}

/// An error from [`unmarshal_slice_any`] or [`unmarshal_any`]; its text is
/// Go's `err.Error()`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnmarshalError {
    /// Go's `*json.SyntaxError`.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    /// Go's `*json.UnmarshalTypeError`.
    #[error(transparent)]
    Type(#[from] UnmarshalTypeError),
}

/// Go's `*json.SyntaxError` (without its `Offset`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{msg}")]
pub struct SyntaxError {
    msg: String,
}

/// Go's `*json.UnmarshalTypeError` (without its `Offset`). Its `Struct` is
/// always empty here: it names the root type, and `[]any` and `any` have no
/// name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.text())]
pub struct UnmarshalTypeError {
    /// Go's `Value`: "string", "number", "bool", "object", or "number"
    /// followed by the number as written.
    pub value: String,
    /// Go's `Type`, as `reflect.Type.String` writes it.
    pub go_type: &'static str,
    /// Go's `Field`: the JSON pointer to the value with its slashes made
    /// dots, empty for the top-level value.
    pub field: String,
}

impl UnmarshalTypeError {
    /// Go's `(*UnmarshalTypeError).Error` with an empty `Struct` and no
    /// `Err` (v1 drops the strconv error of a number).
    fn text(&self) -> String {
        if self.field.is_empty() {
            return format!(
                "json: cannot unmarshal {} into Go value of type {}",
                self.value, self.go_type
            );
        }
        // "As a heuristic, if the last path token looks like an index into
        // a JSON array (e.g., ".foo.bar.0"), avoid the phrase "Go struct
        // field "."
        let last = &self.field[self.field.rfind('.').map_or(0, |i| i + 1)..];
        let into_what = if !last.is_empty() && last.bytes().all(|c| c.is_ascii_digit()) {
            ""
        } else {
            "Go struct field "
        };
        format!(
            "json: cannot unmarshal {} into {into_what}.{} of type {}",
            self.value, self.field, self.go_type
        )
    }
}

/// `json.Unmarshal(data, &v)` with `var v []any`: `None` for a top-level
/// `null`, which leaves v nil.
pub fn unmarshal_slice_any(data: &[u8]) -> Result<Option<Vec<Value>>, UnmarshalError> {
    let pos = check_valid(data)?;
    // The v2 slice unmarshaler reads one token and checks its kind.
    let kind = match data[pos] {
        b'n' => return Ok(None),
        b'[' => None,
        b'"' => Some("string"),
        b't' | b'f' => Some("bool"),
        b'{' => Some("object"),
        _ => Some("number"),
    };
    if let Some(value) = kind {
        return Err(UnmarshalTypeError {
            value: value.to_owned(),
            go_type: "[]interface {}",
            field: String::new(),
        }
        .into());
    }
    match &mut decode(data, pos)? {
        Value::Array(items) => Ok(Some(std::mem::take(items))),
        _ => unreachable!("a JSON array decodes as an array"),
    }
}

/// `json.Unmarshal(data, &m)` with `var m map[string]any`: `None` for a
/// top-level `null`, which leaves m nil. The names are in document order
/// (see the module docs).
pub fn unmarshal_map_any(data: &[u8]) -> Result<Option<IndexMap<String, Value>>, UnmarshalError> {
    let pos = check_valid(data)?;
    // The v2 map unmarshaler reads one token and checks its kind.
    let kind = match data[pos] {
        b'n' => return Ok(None),
        b'{' => None,
        b'"' => Some("string"),
        b't' | b'f' => Some("bool"),
        b'[' => Some("array"),
        _ => Some("number"),
    };
    if let Some(value) = kind {
        return Err(UnmarshalTypeError {
            value: value.to_owned(),
            go_type: "map[string]interface {}",
            field: String::new(),
        }
        .into());
    }
    match &mut decode(data, pos)? {
        Value::Object(members) => Ok(Some(std::mem::take(members))),
        _ => unreachable!("a JSON object decodes as an object"),
    }
}

/// `json.Unmarshal(data, &v)` with `var v any`.
pub fn unmarshal_any(data: &[u8]) -> Result<Value, UnmarshalError> {
    let pos = check_valid(data)?;
    Ok(decode(data, pos)?)
}

// ------------------------------------------------------------ syntax check

/// A syntax error as v2 reports it, before v1 rewrites its text.
#[derive(Debug)]
enum SyntaxErr {
    /// `io.ErrUnexpectedEOF`.
    Eof,
    /// `errMaxDepth`.
    MaxDepth,
    /// `*jsonwire.InvalidTextError`.
    InvalidText {
        label: &'static str,
        what: Vec<u8>,
        where_: String,
    },
}

/// `jsonwire.NewInvalidCharacterError(b[pos:], where)`: the rune at pos.
fn invalid_character(b: &[u8], pos: usize, where_: impl Into<String>) -> SyntaxErr {
    let n = decode_rune(&b[pos..]).1;
    SyntaxErr::InvalidText {
        label: "character",
        what: b[pos..pos + n].to_vec(),
        where_: where_.into(),
    }
}

/// `jsonwire.NewInvalidEscapeSequenceError(what)`.
fn invalid_escape_sequence(what: &[u8]) -> SyntaxErr {
    SyntaxErr::InvalidText {
        label: if what.len() > 6 {
            "surrogate pair"
        } else {
            "escape sequence"
        },
        what: what.to_vec(),
        where_: "in string".to_owned(),
    }
}

/// `jsonwire.QuoteRune`.
fn quote_rune(b: &[u8]) -> String {
    let (r, n) = decode_rune(b);
    if r == RUNE_ERROR && n == 1 {
        return format!("'\\x{:x}'", b[0]);
    }
    gostrconv::quote_rune(decode_char(b).0)
}

/// `(*jsonwire.InvalidTextError).Error`.
fn invalid_text_error(label: &str, what: &[u8], where_: &str) -> String {
    let need_escape = runes(what).any(|r| {
        r == u32::from('`')
            || r == RUNE_ERROR
            || is_space(r)
            || !char::from_u32(r).is_some_and(gostrconv::is_print)
    });
    let what = if rune_count(what) == 1 {
        quote_rune(what)
    } else if need_escape {
        gostrconv::quote(what)
    } else {
        // No U+FFFD among the runes, so what is valid UTF-8.
        format!("`{}`", String::from_utf8_lossy(what))
    };
    let msg = format!("invalid {label} {what} {where_}");
    match msg.strip_suffix(' ') {
        Some(m) => m.to_owned(),
        None => msg,
    }
}

/// v1's `syntaxErrorReplacer`: v1's terms for v2's.
static SYNTAX_ERROR_REPLACER: LazyLock<Replacer> = LazyLock::new(|| {
    Replacer::new(&[
        ("object name", "object key"),
        ("at start of value", "looking for beginning of value"),
        (
            "at start of string",
            "looking for beginning of object key string",
        ),
        ("after object value", "after object key:value pair"),
        ("in number", "in numeric literal"),
    ])
});

impl From<SyntaxErr> for SyntaxError {
    /// v1's `transformSyntacticError`.
    fn from(e: SyntaxErr) -> SyntaxError {
        let mut msg = match e {
            SyntaxErr::Eof => "unexpected end of JSON input".to_owned(),
            SyntaxErr::MaxDepth => "exceeded max depth".to_owned(),
            SyntaxErr::InvalidText {
                label,
                what,
                where_,
            } => invalid_text_error(label, &what, &where_),
        };
        if let Some(i) = msg.find(" (expecting")
            && !msg.contains(" in literal")
        {
            msg.truncate(i);
        }
        SyntaxError {
            msg: SYNTAX_ERROR_REPLACER.replace_str(&msg),
        }
    }
}

/// `jsonwire.ConsumeWhitespace`.
fn consume_whitespace(b: &[u8], mut pos: usize) -> usize {
    while pos < b.len() && matches!(b[pos], b' ' | b'\t' | b'\r' | b'\n') {
        pos += 1;
    }
    pos
}

/// Whitespace, then at least one byte must follow.
fn whitespace_then_more(b: &[u8], pos: usize) -> Result<usize, SyntaxErr> {
    let pos = consume_whitespace(b, pos);
    if pos == b.len() {
        return Err(SyntaxErr::Eof);
    }
    Ok(pos)
}

/// `CheckNextValue(true)` then `checkEOF` of a decoder over data, as v1's
/// `Unmarshal` runs them: the position of the value's first byte.
fn check_valid(data: &[u8]) -> Result<usize, SyntaxError> {
    let start = whitespace_then_more(data, 0)?;
    let end = consume_value(data, start)?;
    let pos = consume_whitespace(data, end);
    if pos < data.len() {
        return Err(invalid_character(data, pos, "after top-level value").into());
    }
    Ok(start)
}

/// `consumeValue` at the top level (depth 1), with `consumeObject` and
/// `consumeArray` unrolled onto a stack so deep input cannot overflow ours.
fn consume_value(b: &[u8], mut pos: usize) -> Result<usize, SyntaxErr> {
    // The open containers: b'[' or b'{'.
    let mut open: Vec<u8> = Vec::new();
    'value: loop {
        let depth = open.len() + 1;
        match b[pos] {
            b'n' => pos = consume_literal(b, pos, "null")?,
            b'f' => pos = consume_literal(b, pos, "false")?,
            b't' => pos = consume_literal(b, pos, "true")?,
            b'"' => pos = consume_string(b, pos)?,
            b'-' | b'0'..=b'9' => pos = consume_number(b, pos)?,
            c @ (b'{' | b'[') => {
                if depth == MAX_NESTING_DEPTH + 1 {
                    return Err(SyntaxErr::MaxDepth);
                }
                pos = whitespace_then_more(b, pos + 1)?;
                if b[pos] == if c == b'{' { b'}' } else { b']' } {
                    pos += 1;
                } else {
                    open.push(c);
                    if c == b'{' {
                        pos = consume_name(b, pos)?;
                    }
                    continue 'value;
                }
            }
            _ => return Err(invalid_character(b, pos, "at start of value")),
        }
        // After a value: close what it completes, or go to the next one.
        while let Some(&c) = open.last() {
            pos = whitespace_then_more(b, pos)?;
            match (c, b[pos]) {
                (b'[', b',') => {
                    pos = whitespace_then_more(b, pos + 1)?;
                    continue 'value;
                }
                (b'{', b',') => {
                    pos = consume_name(b, pos + 1)?;
                    continue 'value;
                }
                (b'[', b']') | (b'{', b'}') => {
                    pos += 1;
                    open.pop();
                }
                (b'[', _) => {
                    return Err(invalid_character(
                        b,
                        pos,
                        "after array element (expecting ',' or ']')",
                    ));
                }
                _ => {
                    return Err(invalid_character(
                        b,
                        pos,
                        "after object value (expecting ',' or '}')",
                    ));
                }
            }
        }
        return Ok(pos);
    }
}

/// The name of an object member, its colon, and the whitespace before its
/// value; the position of the value's first byte.
fn consume_name(b: &[u8], pos: usize) -> Result<usize, SyntaxErr> {
    let pos = whitespace_then_more(b, pos)?;
    let pos = whitespace_then_more(b, consume_string(b, pos)?)?;
    if b[pos] != b':' {
        return Err(invalid_character(
            b,
            pos,
            "after object name (expecting ':')",
        ));
    }
    whitespace_then_more(b, pos + 1)
}

/// `jsonwire.ConsumeLiteral`.
fn consume_literal(b: &[u8], pos: usize, lit: &str) -> Result<usize, SyntaxErr> {
    let rest = &b[pos..];
    for (i, &c) in lit.as_bytes().iter().enumerate().take(rest.len()) {
        if rest[i] != c {
            return Err(invalid_character(
                b,
                pos + i,
                format!(
                    "in literal {lit} (expecting {})",
                    gostrconv::quote_rune(char::from(c))
                ),
            ));
        }
    }
    if rest.len() < lit.len() {
        return Err(SyntaxErr::Eof);
    }
    Ok(pos + lit.len())
}

/// `hasEscapedUTF16Prefix`: whether b may be the start of a `￿`
/// escape cut short.
fn has_escaped_utf16_prefix(b: &[u8], lower_surrogate_half: bool) -> bool {
    for (i, &c) in b.iter().enumerate() {
        let ok = match i {
            0 => c == b'\\',
            1 => c == b'u',
            2 if lower_surrogate_half && c != b'd' && c != b'D' => false,
            3 if lower_surrogate_half && !matches!(c, b'c'..=b'f' | b'C'..=b'F') => false,
            2..=5 => c.is_ascii_hexdigit(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// `parseHexUint16`.
fn parse_hex_u16(b: &[u8]) -> Option<u16> {
    if b.len() != 4 {
        return None;
    }
    let mut v = 0u16;
    for &c in b {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => 10 + c - b'a',
            b'A'..=b'F' => 10 + c - b'A',
            _ => return None,
        };
        v = v * 16 + u16::from(d);
    }
    Some(v)
}

/// The bytes a JSON string may hold as they are.
fn no_escape(c: u8) -> bool {
    (b' '..0x80).contains(&c) && c != b'\\' && c != b'"'
}

/// `jsonwire.ConsumeString` with `validateUTF8` false, as v1's
/// `AllowInvalidUTF8` sets it: escapes of surrogates are not checked.
fn consume_string(b: &[u8], pos: usize) -> Result<usize, SyntaxErr> {
    let s = &b[pos..];
    match s.first() {
        None => return Err(SyntaxErr::Eof),
        Some(b'"') => {}
        Some(_) => {
            return Err(invalid_character(
                b,
                pos,
                "at start of string (expecting '\"')",
            ));
        }
    }
    let mut n = 1;
    while n < s.len() {
        while n < s.len() && no_escape(s[n]) {
            n += 1;
        }
        if n >= s.len() {
            return Err(SyntaxErr::Eof);
        }
        if s[n] == b'"' {
            return Ok(pos + n + 1);
        }
        let (r, rn) = decode_rune(&s[n..]);
        if rn > 1 {
            n += rn;
        } else if r == u32::from('\\') {
            if s.len() < n + 2 {
                return Err(SyntaxErr::Eof);
            }
            match s[n + 1] {
                b'/' | b'"' | b'\\' | b'b' | b'f' | b'n' | b'r' | b't' => n += 2,
                b'u' => {
                    if s.len() < n + 6 {
                        if has_escaped_utf16_prefix(&s[n..], false) {
                            return Err(SyntaxErr::Eof);
                        }
                        return Err(invalid_escape_sequence(&s[n..]));
                    }
                    if parse_hex_u16(&s[n + 2..n + 6]).is_none() {
                        return Err(invalid_escape_sequence(&s[n..n + 6]));
                    }
                    n += 6;
                }
                _ => return Err(invalid_escape_sequence(&s[n..n + 2])),
            }
        } else if r == RUNE_ERROR {
            if !full_rune(&s[n..]) {
                return Err(SyntaxErr::Eof);
            }
            n += 1;
        } else {
            // A control character: everything else was taken above.
            return Err(invalid_character(
                b,
                pos + n,
                "in string (expecting non-control character)",
            ));
        }
    }
    Err(SyntaxErr::Eof)
}

/// `jsonwire.ConsumeNumber`, where a number may end at the end of input.
fn consume_number(b: &[u8], pos: usize) -> Result<usize, SyntaxErr> {
    let s = &b[pos..];
    let digits = |mut n: usize| {
        while n < s.len() && s[n].is_ascii_digit() {
            n += 1;
        }
        n
    };
    let expect_digit = |n: usize| -> Result<usize, SyntaxErr> {
        match s.get(n) {
            None => Err(SyntaxErr::Eof),
            Some(c) if c.is_ascii_digit() => Ok(digits(n + 1)),
            Some(_) => Err(invalid_character(b, pos + n, "in number (expecting digit)")),
        }
    };
    let mut n = usize::from(s.first() == Some(&b'-'));
    n = match s.get(n) {
        None => return Err(SyntaxErr::Eof),
        Some(b'0') => n + 1,
        Some(_) => expect_digit(n)?,
    };
    if s.get(n) == Some(&b'.') {
        n = expect_digit(n + 1)?;
    }
    if matches!(s.get(n), Some(b'e' | b'E')) {
        n += 1;
        if matches!(s.get(n), Some(b'-' | b'+')) {
            n += 1;
        }
        n = expect_digit(n)?;
    }
    Ok(pos + n)
}

// ---------------------------------------------------------------- decoding

/// An open container while decoding: the items so far, or the members so
/// far and the name whose value is being decoded.
enum Frame {
    Array(Vec<Value>),
    Object(IndexMap<String, Value>, String),
}

/// Decodes the value at pos of input that [`check_valid`] accepted. The
/// only error left is a number that overflows float64.
fn decode(b: &[u8], mut pos: usize) -> Result<Value, UnmarshalTypeError> {
    let mut open: Vec<Frame> = Vec::new();
    'value: loop {
        let mut v = match b[pos] {
            b'n' => {
                pos += 4;
                Value::Null
            }
            b't' => {
                pos += 4;
                Value::Bool(true)
            }
            b'f' => {
                pos += 5;
                Value::Bool(false)
            }
            b'"' => {
                let end = string_end(b, pos);
                let s = unquote(&b[pos..end]);
                pos = end;
                Value::String(s)
            }
            b'[' => {
                pos = consume_whitespace(b, pos + 1);
                if b[pos] == b']' {
                    pos += 1;
                    Value::Array(Vec::new())
                } else {
                    open.push(Frame::Array(Vec::new()));
                    continue 'value;
                }
            }
            b'{' => {
                pos = consume_whitespace(b, pos + 1);
                if b[pos] == b'}' {
                    pos += 1;
                    Value::Object(IndexMap::new())
                } else {
                    let (name, next) = decode_name(b, pos);
                    pos = next;
                    open.push(Frame::Object(IndexMap::new(), name));
                    continue 'value;
                }
            }
            _ => {
                let end = consume_number(b, pos).unwrap_or(b.len());
                let literal = &b[pos..end];
                let Some(f) = parse_float(literal) else {
                    return Err(UnmarshalTypeError {
                        value: format!("number {}", String::from_utf8_lossy(literal)),
                        go_type: "float64",
                        field: field_path(&open),
                    });
                };
                pos = end;
                Value::Number(f)
            }
        };
        // Put v where it belongs, closing the containers it completes.
        loop {
            let Some(frame) = open.last_mut() else {
                return Ok(v);
            };
            pos = consume_whitespace(b, pos);
            let more = b[pos] == b',';
            match frame {
                Frame::Array(items) => items.push(v),
                Frame::Object(members, name) => {
                    // A Go map: a duplicate name replaces the earlier value
                    // (and keeps its position).
                    members.insert(std::mem::take(name), v);
                    if more {
                        let (next_name, next) = decode_name(b, pos + 1);
                        *name = next_name;
                        pos = next;
                        continue 'value;
                    }
                }
            }
            if more {
                pos = consume_whitespace(b, pos + 1);
                continue 'value;
            }
            pos += 1;
            v = match open.pop() {
                Some(Frame::Array(items)) => Value::Array(items),
                Some(Frame::Object(members, _)) => Value::Object(members),
                None => unreachable!("a frame was open"),
            };
        }
    }
}

/// The name of a checked object member and the position of its value.
fn decode_name(b: &[u8], pos: usize) -> (String, usize) {
    let pos = consume_whitespace(b, pos);
    let end = string_end(b, pos);
    let name = unquote(&b[pos..end]);
    let colon = consume_whitespace(b, end);
    (name, consume_whitespace(b, colon + 1))
}

/// The end of the checked string starting at pos.
fn string_end(b: &[u8], pos: usize) -> usize {
    let mut i = pos + 1;
    loop {
        match b[i] {
            b'"' => return i + 1,
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
}

/// `jsonwire.AppendUnquote` of a checked string, ignoring its errors as
/// `UnquoteMayCopy` does: invalid UTF-8 and unpaired surrogate escapes
/// become U+FFFD.
fn unquote(src: &[u8]) -> String {
    let mut dst = String::with_capacity(src.len());
    let end = src.len() - 1; // the closing quote
    let mut n = 1;
    while n < end {
        let c = src[n];
        if no_escape(c) {
            dst.push(char::from(c));
            n += 1;
            continue;
        }
        if c != b'\\' {
            // A UTF-8 sequence or an invalid byte (U+FFFD).
            let (r, rn) = decode_char(&src[n..]);
            dst.push(r);
            n += rn;
            continue;
        }
        let e = src[n + 1];
        n += 2;
        match e {
            b'b' => dst.push('\u{8}'),
            b'f' => dst.push('\u{c}'),
            b'n' => dst.push('\n'),
            b'r' => dst.push('\r'),
            b't' => dst.push('\t'),
            b'u' => {
                let v1 = u32::from(parse_hex_u16(&src[n..n + 4]).unwrap_or(0));
                n += 4;
                let mut r = char::from_u32(v1);
                if (0xD800..0xE000).contains(&v1) {
                    // A surrogate half: U+FFFD unless a low half follows.
                    // When it does not, n stays put and whatever follows is
                    // decoded on its own.
                    r = None;
                    if src.len() >= n + 6
                        && src[n] == b'\\'
                        && src[n + 1] == b'u'
                        && let Some(v2) = parse_hex_u16(&src[n + 2..n + 6])
                    {
                        let v2 = u32::from(v2);
                        if (0xD800..0xDC00).contains(&v1) && (0xDC00..0xE000).contains(&v2) {
                            r = char::from_u32(0x10000 + ((v1 - 0xD800) << 10) + (v2 - 0xDC00));
                            n += 6;
                        }
                    }
                }
                dst.push(r.unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            // '"', '\\' and '/' stand for themselves.
            _ => dst.push(char::from(e)),
        }
    }
    dst
}

/// v1's `UnmarshalTypeError.Field` for the value being decoded: the JSON
/// pointer from the decoder's stack, with "/" made ".".
fn field_path(open: &[Frame]) -> String {
    let mut field = String::new();
    for (i, frame) in open.iter().enumerate() {
        if i > 0 {
            field.push('.');
        }
        match frame {
            Frame::Array(items) => field.push_str(&items.len().to_string()),
            Frame::Object(_, name) => {
                // appendEscapePointerName (RFC 6901).
                for r in name.chars() {
                    match r {
                        '~' => field.push_str("~0"),
                        '/' => field.push_str("~1"),
                        _ => field.push(r),
                    }
                }
            }
        }
    }
    field
}

/// `strconv.ParseFloat(literal, 64)` for a JSON number: `None` where Go
/// reports `ErrRange`.
fn parse_float(literal: &[u8]) -> Option<f64> {
    let (f, err) = gostrconv::parse_float(literal, 64);
    err.is_none().then_some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_values_drop_without_recursing() {
        // As deep as Go allows: 5000 arrays and 5000 objects.
        let deep = format!(
            "{}1{}",
            "[{\"a\":".repeat(MAX_NESTING_DEPTH / 2),
            "}]".repeat(MAX_NESTING_DEPTH / 2)
        );
        // Runs on a 256 KiB stack, far too small for a recursive drop.
        std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(move || {
                let v = unmarshal_slice_any(deep.as_bytes()).unwrap();
                assert!(v.is_some());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn objects_keep_document_order() {
        let names = |m: &IndexMap<String, Value>| m.keys().cloned().collect::<Vec<_>>();
        let m = unmarshal_map_any(br#"{"b":1,"a":{"z":1,"y":2},"c":3,"a":{"x":4}}"#)
            .unwrap()
            .unwrap();
        // "a" keeps its first position and takes its last value, whole.
        assert_eq!(names(&m), ["b", "a", "c"]);
        let Value::Object(a) = &m["a"] else {
            panic!("a is {:?}", m["a"]);
        };
        assert_eq!(names(a), ["x"]);
        let Value::Object(nested) = &unmarshal_any(br#"{"z":{"q":1,"p":2}}"#).unwrap() else {
            panic!("not an object");
        };
        let Value::Object(z) = &nested["z"] else {
            panic!("z is {:?}", nested["z"]);
        };
        assert_eq!(names(z), ["q", "p"]);
    }
}
