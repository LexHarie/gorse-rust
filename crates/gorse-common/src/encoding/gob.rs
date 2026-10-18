// Ports: encoding/gob of the Go standard library (decoder.go, decode.go, dec_helpers.go, encoder.go, encode.go, type.go) @ go1.27.0, restricted to the subset in contracts/model-blobs.md

//! The part of Go's `encoding/gob` that Gorse puts in model blobs.
//!
//! Gorse gob-encodes one value per stream (`common/encoding.WriteGob` makes a
//! fresh `gob.Encoder` every time), and only these Go types:
//!
//! | Go type | Rust type |
//! |---|---|
//! | `int`, `int64` | [`i64`] |
//! | `int32` | [`i32`] |
//! | `bool` | [`bool`] |
//! | `float32` | [`f32`] |
//! | `float64` | [`f64`] |
//! | `string` | [`String`] |
//! | `[]int` | `Vec<i64>` |
//! | `[]float32` | `Vec<f32>` (vectors in `common/ann` HNSW indexes) |
//! | `model.Params` (`map[ParamName]any`) | [`Params`] |
//!
//! The decoder follows Go's decoder step by step: message framing, type
//! definitions (including the ones Go sends in the middle of a value when an
//! interface holds a type the stream has not described yet), singleton deltas,
//! struct field deltas, and the same range checks. Any stream Go's decoder
//! accepts for these types is accepted, and the inputs Go rejects are rejected.
//!
//! The encoder produces the bytes a fresh Go process produces. Go assigns type
//! ids once per process in first-use order, so a long-running Go process can
//! number the same types differently; decoders on both sides only use the ids
//! a stream defines, so either form is read the same way.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Ids of the predefined gob types (`type.go`).
const T_BOOL: i32 = 1;
const T_INT: i32 = 2;
const T_UINT: i32 = 3;
const T_FLOAT: i32 = 4;
const T_BYTES: i32 = 5;
const T_STRING: i32 = 6;
const T_COMPLEX: i32 = 7;
const T_INTERFACE: i32 = 8;
/// The first id a stream may define (`firstUserId`).
const FIRST_USER_ID: i32 = 64;
/// Go's sanity limit for sizes on 64-bit platforms (`tooBig`).
const TOO_BIG: u64 = 1 << 33;
/// Go's limit on interface type names (`decodeInterface`).
const MAX_NAME_LEN: usize = 1024;

/// A gob decoding or encoding failure. The messages follow Go's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The stream is empty (Go `io.EOF`).
    #[error("EOF")]
    Eof,
    /// The stream ends inside a message, or after type definitions with no
    /// value (Go `io.ErrUnexpectedEOF`).
    #[error("unexpected EOF")]
    UnexpectedEof,
    /// The stream is malformed or holds a different type than requested.
    #[error("{0}")]
    Invalid(String),
    /// The stream is valid gob but holds something outside the subset this
    /// port reads (a registered Go type Gorse never encodes, or a string that
    /// is not UTF-8).
    #[error("{0}")]
    Unsupported(String),
}

type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl fmt::Display) -> Error {
    Error::Invalid(format!("gob: {message}"))
}

/// A dynamically typed value held by a Go `any` inside [`Params`].
///
/// The variant is the Go dynamic type, which matters: Gorse's `Params`
/// accessors switch on it (`GetInt` accepts only `int`, `GetFloat32` accepts
/// `float32`, `float64` and `int`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A nil interface.
    Nil,
    /// Go `int`.
    Int(i64),
    /// Go `int64`.
    Int64(i64),
    /// Go `int32`.
    Int32(i32),
    /// Go `float32`.
    Float32(f32),
    /// Go `float64`.
    Float64(f64),
    /// Go `bool`.
    Bool(bool),
    /// Go `string`.
    String(String),
    /// Go `[]int`.
    IntSlice(Vec<i64>),
}

impl Value {
    /// The name Go registers the dynamic type under (`gob.Register` of the
    /// basic types), which is what the stream carries.
    fn registered_name(&self) -> &'static str {
        match self {
            Value::Nil => "",
            Value::Int(_) => "int",
            Value::Int64(_) => "int64",
            Value::Int32(_) => "int32",
            Value::Float32(_) => "float32",
            Value::Float64(_) => "float64",
            Value::Bool(_) => "bool",
            Value::String(_) => "string",
            Value::IntSlice(_) => "[]int",
        }
    }
}

/// The gob form of Gorse's `model.Params`, a `map[ParamName]any`.
///
/// Entries are kept sorted by key, so encoding is deterministic; Go writes them
/// in map order, which is random, and both orders decode to the same map.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Params(pub BTreeMap<String, Value>);

/// Names Go's `gob` package registers for interface values by default
/// (`registerBasics` in `type.go`) that are outside this port's subset.
const OTHER_REGISTERED_NAMES: &[&str] = &[
    "int8",
    "int16",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "complex64",
    "complex128",
    "[]int8",
    "[]int16",
    "[]int32",
    "[]int64",
    "[]uint",
    "[]uint8",
    "[]uint16",
    "[]uint32",
    "[]uint64",
    "[]float32",
    "[]float64",
    "[]complex64",
    "[]complex128",
    "[]uintptr",
    "[]bool",
    "[]string",
];

// ---------------------------------------------------------------------------
// Wire types (`wireType` and the types it points to, `type.go`).

#[derive(Debug, Clone, Default)]
struct CommonType {
    name: String,
    id: i32,
}

#[derive(Debug, Clone, Default)]
struct ArrayType {
    common: CommonType,
    elem: i32,
    len: i64,
}

#[derive(Debug, Clone, Default)]
struct SliceType {
    common: CommonType,
    elem: i32,
}

#[derive(Debug, Clone, Default)]
struct FieldType {
    name: String,
    id: i32,
}

#[derive(Debug, Clone, Default)]
struct StructType {
    common: CommonType,
    fields: Vec<FieldType>,
}

#[derive(Debug, Clone, Default)]
struct MapType {
    common: CommonType,
    key: i32,
    elem: i32,
}

/// A type definition received from the stream. Go sets exactly one field when
/// encoding; a crafted stream may set several, and Go's checks then look at
/// each field independently, as here.
#[derive(Debug, Clone, Default)]
struct WireType {
    array: Option<ArrayType>,
    slice: Option<SliceType>,
    strukt: Option<StructType>,
    map: Option<MapType>,
    gob_encoder: Option<CommonType>,
    binary_marshaler: Option<CommonType>,
    text_marshaler: Option<CommonType>,
}

impl WireType {
    fn has_external_encoding(&self) -> bool {
        self.gob_encoder.is_some()
            || self.binary_marshaler.is_some()
            || self.text_marshaler.is_some()
    }

    /// `wireType.string` in Go: the name of the first field that is set.
    fn name(&self) -> &str {
        if let Some(t) = &self.array {
            &t.common.name
        } else if let Some(t) = &self.slice {
            &t.common.name
        } else if let Some(t) = &self.strukt {
            &t.common.name
        } else if let Some(t) = &self.map {
            &t.common.name
        } else if let Some(t) = &self.gob_encoder {
            &t.name
        } else if let Some(t) = &self.binary_marshaler {
            &t.name
        } else if let Some(t) = &self.text_marshaler {
            &t.name
        } else {
            UNKNOWN_TYPE
        }
    }
}

/// What Go's `wireType.string` says for an id it cannot name.
const UNKNOWN_TYPE: &str = "unknown type";

// ---------------------------------------------------------------------------
// Decoding.

/// Decodes one value of type `T` from a complete gob stream, like
/// `gob.NewDecoder(bytes.NewBuffer(data)).Decode(&v)`.
///
/// Like Go, bytes after the value (the rest of its message and any further
/// messages) are ignored.
///
/// # Errors
///
/// Returns an error when the stream is malformed, when it holds a type that
/// cannot be decoded into `T` (the same compatibility rules as Go), or when it
/// holds something outside the subset ([`Error::Unsupported`]).
pub fn decode<T: GobValue>(data: &[u8]) -> Result<T> {
    let mut dec = Decoder {
        input: data,
        buf: &[],
        wire_types: HashMap::new(),
    };
    let id = dec.decode_type_sequence(false)?;
    // Go names the local type of a top-level value through the pointer
    // passed to Decode, as in `*int64`.
    dec.decode_value::<T>(id, &format!("*{}", T::GO_TYPE))
}

/// The state of one decode. It is only reachable through [`decode`].
#[derive(Debug)]
pub struct Decoder<'a> {
    /// The unread part of the stream (`dec.r`).
    input: &'a [u8],
    /// The unread part of the current message (`dec.buf`).
    buf: &'a [u8],
    /// Type definitions received so far.
    wire_types: HashMap<i32, WireType>,
}

/// Reads a gob unsigned integer the way `decodeUintReader` does, for message
/// counts and type ids.
fn decode_uint_reader(src: &mut &[u8]) -> Result<u64> {
    let Some((&first, rest)) = src.split_first() else {
        return Err(Error::Eof);
    };
    *src = rest;
    if first <= 0x7f {
        return Ok(u64::from(first));
    }
    let n = byte_count(first);
    if n > 8 {
        return Err(invalid("encoded unsigned integer out of range"));
    }
    if src.len() < n {
        *src = &[];
        return Err(Error::UnexpectedEof);
    }
    let (bytes, rest) = src.split_at(n);
    *src = rest;
    Ok(be_uint(bytes))
}

/// The byte count of a multi-byte gob unsigned integer: the first byte holds
/// its negation (`-int(int8(b))`).
fn byte_count(first: u8) -> usize {
    usize::from(first.wrapping_neg())
}

fn be_uint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |x, &b| x << 8 | u64::from(b))
}

/// Go's `toInt`: undoes the zig-zag encoding of signed integers.
fn to_int(x: u64) -> i64 {
    // The shift leaves 63 bits, which always fit in an i64.
    let i = i64::try_from(x >> 1).unwrap_or(i64::MAX);
    if x & 1 != 0 { !i } else { i }
}

impl<'a> Decoder<'a> {
    fn recv_message(&mut self) -> Result<()> {
        let nbytes = decode_uint_reader(&mut self.input)?;
        let bad_count = || Error::Invalid("invalid message length".to_owned());
        if nbytes >= TOO_BIG {
            return Err(bad_count());
        }
        let n = usize::try_from(nbytes).map_err(|_| bad_count())?;
        if self.input.len() < n {
            self.input = &[];
            return Err(Error::UnexpectedEof);
        }
        let (message, rest) = self.input.split_at(n);
        self.buf = message;
        self.input = rest;
        Ok(())
    }

    fn next_int(&mut self) -> Result<i64> {
        decode_uint_reader(&mut self.buf).map(to_int)
    }

    fn next_uint(&mut self) -> Result<u64> {
        decode_uint_reader(&mut self.buf)
    }

    /// Reads type definitions until a type id that announces a value, and
    /// returns that id (`decodeTypeSequence`).
    fn decode_type_sequence(&mut self, is_interface: bool) -> Result<i32> {
        let mut first_message = true;
        loop {
            if self.buf.is_empty()
                && let Err(err) = self.recv_message()
            {
                // Only an empty input is a plain EOF; type definitions must
                // be followed by a value.
                return Err(if !first_message && err == Error::Eof {
                    Error::UnexpectedEof
                } else {
                    err
                });
            }
            // Go converts the id to its 32-bit typeId, dropping the high bits.
            let id = self.next_int()? as i32;
            if id >= 0 {
                return Ok(id);
            }
            self.recv_type(id.wrapping_neg())?;
            // Inside an interface a delimited value may follow the type in the
            // same buffer; its count is skipped.
            if !self.buf.is_empty() {
                if !is_interface {
                    return Err(Error::Invalid("extra data in buffer".to_owned()));
                }
                self.next_uint()?;
            }
            first_message = false;
        }
    }

    fn recv_type(&mut self, id: i32) -> Result<()> {
        if id < FIRST_USER_ID || self.wire_types.contains_key(&id) {
            return Err(invalid("duplicate type received"));
        }
        let wire = self.decode_wire_type()?;
        self.wire_types.insert(id, wire);
        Ok(())
    }

    // --- primitive reads on the current message (`decoderState`) ---

    fn decode_uint(&mut self) -> Result<u64> {
        let Some((&first, rest)) = self.buf.split_first() else {
            return Err(Error::Eof);
        };
        self.buf = rest;
        if first <= 0x7f {
            return Ok(u64::from(first));
        }
        let n = byte_count(first);
        if n > 8 {
            return Err(invalid("encoded unsigned integer out of range"));
        }
        if self.buf.len() < n {
            return Err(invalid(format_args!(
                "invalid uint data length {n}: exceeds input size {}",
                self.buf.len()
            )));
        }
        let (bytes, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(be_uint(bytes))
    }

    fn decode_int(&mut self) -> Result<i64> {
        self.decode_uint().map(to_int)
    }

    /// Go `int(state.decodeUint())`: a 64-bit reinterpretation.
    fn decode_uint_as_int(&mut self) -> Result<i64> {
        self.decode_uint().map(|u| u as i64)
    }

    fn decode_int32(&mut self, what: &str) -> Result<i32> {
        let v = self.decode_int()?;
        i32::try_from(v).map_err(|_| Error::Invalid(format!("value for \"{what}\" out of range")))
    }

    fn decode_bool(&mut self) -> Result<bool> {
        Ok(self.decode_uint()? != 0)
    }

    fn decode_float64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.decode_uint()?.swap_bytes()))
    }

    fn decode_float32(&mut self, what: &str) -> Result<f32> {
        let v = self.decode_float64()?;
        let av = v.abs();
        // +Inf is fine in both widths and underflow is always fine.
        if f64::from(f32::MAX) < av && av <= f64::MAX {
            return Err(Error::Invalid(format!("value for \"{what}\" out of range")));
        }
        Ok(v as f32)
    }

    /// `decString`: a length (checked by `getLength`) and the bytes.
    fn decode_string_bytes(&mut self, what: &str) -> Result<&'a [u8]> {
        let n = self.decode_uint_as_int()?;
        let len = self.buf.len();
        let n = match usize::try_from(n) {
            Ok(n) if n <= len && (n as u64) < TOO_BIG => n,
            _ => return Err(invalid(format_args!("bad {what} slice length: 0"))),
        };
        let (bytes, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(bytes)
    }

    fn decode_string(&mut self, what: &str) -> Result<String> {
        let bytes = self.decode_string_bytes(what)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::Unsupported(format!("gob: {what} value is not valid UTF-8")))
    }

    /// `decodeStruct`: field deltas until a zero delta or the end of the
    /// message. `field` decodes field `i` of a struct with `num_fields` fields.
    fn decode_struct(
        &mut self,
        num_fields: i64,
        mut field: impl FnMut(&mut Self, i64) -> Result<()>,
    ) -> Result<()> {
        let mut fieldnum: i64 = -1;
        while !self.buf.is_empty() {
            let delta = self.decode_uint_as_int()?;
            if delta < 0 {
                return Err(invalid("decode: corrupted data: negative delta"));
            }
            if delta == 0 {
                break;
            }
            if fieldnum >= num_fields - delta {
                return Err(invalid("bad data: field numbers out of bounds"));
            }
            fieldnum += delta;
            field(self, fieldnum)?;
        }
        Ok(())
    }

    // --- wire types ---

    fn decode_wire_type(&mut self) -> Result<WireType> {
        let mut wire = WireType::default();
        self.decode_struct(7, |dec, i| {
            match i {
                0 => wire.array = Some(dec.decode_array_type()?),
                1 => wire.slice = Some(dec.decode_slice_type()?),
                2 => wire.strukt = Some(dec.decode_struct_type()?),
                3 => wire.map = Some(dec.decode_map_type()?),
                4 => wire.gob_encoder = Some(dec.decode_gob_encoder_type()?),
                5 => wire.binary_marshaler = Some(dec.decode_gob_encoder_type()?),
                _ => wire.text_marshaler = Some(dec.decode_gob_encoder_type()?),
            }
            Ok(())
        })?;
        Ok(wire)
    }

    fn decode_common_type(&mut self) -> Result<CommonType> {
        let mut common = CommonType::default();
        self.decode_struct(2, |dec, i| {
            if i == 0 {
                common.name = dec.decode_string("string")?;
            } else {
                common.id = dec.decode_int32("Id")?;
            }
            Ok(())
        })?;
        Ok(common)
    }

    fn decode_array_type(&mut self) -> Result<ArrayType> {
        let mut t = ArrayType::default();
        self.decode_struct(3, |dec, i| {
            match i {
                0 => t.common = dec.decode_common_type()?,
                1 => t.elem = dec.decode_int32("Elem")?,
                _ => t.len = dec.decode_int()?,
            }
            Ok(())
        })?;
        Ok(t)
    }

    fn decode_slice_type(&mut self) -> Result<SliceType> {
        let mut t = SliceType::default();
        self.decode_struct(2, |dec, i| {
            if i == 0 {
                t.common = dec.decode_common_type()?;
            } else {
                t.elem = dec.decode_int32("Elem")?;
            }
            Ok(())
        })?;
        Ok(t)
    }

    fn decode_struct_type(&mut self) -> Result<StructType> {
        let mut t = StructType::default();
        self.decode_struct(2, |dec, i| {
            if i == 0 {
                t.common = dec.decode_common_type()?;
            } else {
                t.fields = dec.decode_field_types()?;
            }
            Ok(())
        })?;
        Ok(t)
    }

    /// The `[]fieldType` of a struct definition (`decodeSlice` of structs).
    fn decode_field_types(&mut self) -> Result<Vec<FieldType>> {
        // 24 bytes: the size of Go's fieldType{Name string; Id typeId}.
        let n = self.decode_slice_len("gob.fieldType", 24)?;
        let mut fields = Vec::new();
        for _ in 0..n {
            if self.buf.is_empty() {
                return Err(invalid(format_args!(
                    "decoding array or slice: length exceeds input size ({n} elements)"
                )));
            }
            let mut field = FieldType::default();
            self.decode_struct(2, |dec, i| {
                if i == 0 {
                    field.name = dec.decode_string("string")?;
                } else {
                    field.id = dec.decode_int32("Id")?;
                }
                Ok(())
            })?;
            fields.push(field);
        }
        Ok(fields)
    }

    fn decode_map_type(&mut self) -> Result<MapType> {
        let mut t = MapType::default();
        self.decode_struct(3, |dec, i| {
            match i {
                0 => t.common = dec.decode_common_type()?,
                1 => t.key = dec.decode_int32("Key")?,
                _ => t.elem = dec.decode_int32("Elem")?,
            }
            Ok(())
        })?;
        Ok(t)
    }

    fn decode_gob_encoder_type(&mut self) -> Result<CommonType> {
        let mut common = CommonType::default();
        self.decode_struct(1, |dec, _| {
            common = dec.decode_common_type()?;
            Ok(())
        })?;
        Ok(common)
    }

    /// The element count of a slice, with `decodeSlice`'s size checks.
    fn decode_slice_len(&mut self, elem: &str, elem_size: u64) -> Result<u64> {
        let u = self.decode_uint()?;
        let n_bytes = u.wrapping_mul(elem_size);
        if i64::try_from(u).is_err() || n_bytes > TOO_BIG || n_bytes / elem_size != u {
            return Err(invalid(format_args!(
                "{elem} slice too big: {u} elements of {elem_size} bytes"
            )));
        }
        Ok(u)
    }

    // --- values ---

    /// Go's `typeString`, for error messages.
    fn type_string(&self, id: i32) -> String {
        let builtin = match id {
            T_BOOL => "bool",
            T_INT => "int",
            T_UINT => "uint",
            T_FLOAT => "float",
            T_BYTES => "[]byte",
            T_STRING => "string",
            T_COMPLEX => "complex",
            T_INTERFACE => "interface",
            _ => "",
        };
        if !builtin.is_empty() {
            return builtin.to_owned();
        }
        self.wire_types
            .get(&id)
            .map_or(UNKNOWN_TYPE, WireType::name)
            .to_owned()
    }

    /// `decodeValue` for a non-struct local type: the compatibility check of
    /// `compileSingle`, then `decodeSingle`. `local` names the local type in
    /// error messages.
    fn decode_value<T: GobValue>(&mut self, wire_id: i32, local: &str) -> Result<T> {
        if !T::compatible(self, wire_id) {
            return Err(invalid(format_args!(
                "decoding into local type {local}, received remote type {}",
                self.type_string(wire_id)
            )));
        }
        if self.decode_uint()? != 0 {
            return Err(invalid(
                "decode: corrupted data: non-zero delta for singleton",
            ));
        }
        T::decode_body(self, local)
    }

    /// Whether `wire_id` is a slice whose elements are gob ints.
    fn is_int_slice(&self, wire_id: i32) -> bool {
        self.is_slice_of(wire_id, T_INT)
    }

    /// Whether `wire_id` is a slice of floats (`[]float32` or `[]float64`).
    fn is_float_slice(&self, wire_id: i32) -> bool {
        self.is_slice_of(wire_id, T_FLOAT)
    }

    fn is_slice_of(&self, wire_id: i32, elem: i32) -> bool {
        // The predefined slice types (ids below 64) hold gob structs.
        self.wire_types.get(&wire_id).is_some_and(|w| {
            !w.has_external_encoding() && w.slice.as_ref().is_some_and(|s| s.elem == elem)
        })
    }

    /// Whether `wire_id` is a map from strings to interfaces.
    fn is_string_interface_map(&self, wire_id: i32) -> bool {
        self.wire_types.get(&wire_id).is_some_and(|w| {
            !w.has_external_encoding()
                && w.map
                    .as_ref()
                    .is_some_and(|m| m.key == T_STRING && m.elem == T_INTERFACE)
        })
    }

    fn decode_int_slice(&mut self) -> Result<Vec<i64>> {
        let n = self.decode_slice_len("int", 8)?;
        let mut v = Vec::new();
        for _ in 0..n {
            if self.buf.is_empty() {
                return Err(invalid(format_args!(
                    "decoding int array or slice: length exceeds input size ({n} elements)"
                )));
            }
            v.push(self.decode_int()?);
        }
        Ok(v)
    }

    /// `decFloat32Slice`; `local` names the slice for the overflow error.
    fn decode_float32_slice(&mut self, local: &str) -> Result<Vec<f32>> {
        let n = self.decode_slice_len("float32", 4)?;
        let what = format!("element of {local}");
        let mut v = Vec::new();
        for _ in 0..n {
            if self.buf.is_empty() {
                return Err(invalid(format_args!(
                    "decoding float32 array or slice: length exceeds input size ({n} elements)"
                )));
            }
            v.push(self.decode_float32(&what)?);
        }
        Ok(v)
    }

    /// `decodeInterface`.
    fn decode_interface(&mut self) -> Result<Value> {
        let nr = self.decode_uint()?;
        if nr > 1 << 31 {
            return Err(invalid(format_args!("invalid type name length {nr}")));
        }
        let n = match usize::try_from(nr) {
            Ok(n) if n <= self.buf.len() => n,
            _ => {
                return Err(invalid(format_args!(
                    "invalid type name length {nr}: exceeds input size"
                )));
            }
        };
        let (name, rest) = self.buf.split_at(n);
        self.buf = rest;
        if name.is_empty() {
            return Ok(Value::Nil);
        }
        if name.len() > MAX_NAME_LEN {
            return Err(invalid(format_args!(
                "name too long ({} bytes)",
                name.len()
            )));
        }
        let name = String::from_utf8_lossy(name).into_owned();
        let known = matches!(
            name.as_str(),
            "int" | "int64" | "int32" | "float32" | "float64" | "bool" | "string" | "[]int"
        );
        if !known {
            if OTHER_REGISTERED_NAMES.contains(&name.as_str()) {
                return Err(Error::Unsupported(format!(
                    "gob: interface value of type {name} is outside the subset Gorse encodes"
                )));
            }
            return Err(invalid(format_args!(
                "name not registered for interface: {name:?}"
            )));
        }
        let concrete_id = self.decode_type_sequence(true)?;
        // The byte count of the value, which Go reads and ignores.
        self.decode_uint()?;
        let id = concrete_id;
        Ok(match name.as_str() {
            "int" => Value::Int(self.decode_value(id, &name)?),
            "int64" => Value::Int64(self.decode_value(id, &name)?),
            "int32" => Value::Int32(self.decode_value(id, &name)?),
            "float32" => Value::Float32(self.decode_value(id, &name)?),
            "float64" => Value::Float64(self.decode_value(id, &name)?),
            "bool" => Value::Bool(self.decode_value(id, &name)?),
            "string" => Value::String(self.decode_value(id, &name)?),
            _ => Value::IntSlice(self.decode_value(id, &name)?),
        })
    }

    /// `decodeMap` for `map[ParamName]any`.
    fn decode_params(&mut self) -> Result<Params> {
        // Go converts the count to int; a count that wraps negative decodes
        // no entries.
        let n = self.decode_uint_as_int()?;
        let mut map = BTreeMap::new();
        for _ in 0..n.max(0) {
            let key = self.decode_string("model.ParamName")?;
            let value = self.decode_interface()?;
            map.insert(key, value);
        }
        Ok(Params(map))
    }
}

// ---------------------------------------------------------------------------
// Encoding.

/// Encodes `v` as a complete gob stream, byte for byte what
/// `gob.NewEncoder(w).Encode(v)` writes in a fresh Go process.
///
/// # Errors
///
/// Returns an error for a value Go cannot encode either: a message longer than
/// Go's limit.
pub fn encode<T: GobValue>(v: &T) -> Result<Vec<u8>> {
    let mut enc = Encoder {
        out: Vec::new(),
        sent: HashMap::new(),
        next_id: FIRST_USER_ID,
    };
    let mut b = Vec::new();
    let id = T::send_type(&mut enc, &mut b)?;
    encode_int(&mut b, i64::from(id));
    // The singleton delta of a top-level non-struct value.
    encode_uint(&mut b, 0);
    v.encode_body(&mut enc, &mut b)?;
    enc.write_message(&mut b)?;
    Ok(enc.out)
}

/// The state of one encode. It is only reachable through [`encode`].
#[derive(Debug)]
pub struct Encoder {
    /// The stream written so far.
    out: Vec<u8>,
    /// Types whose definitions are already in the stream, by Go type string.
    sent: HashMap<&'static str, i32>,
    /// The id the next defined type gets.
    next_id: i32,
}

fn encode_uint(b: &mut Vec<u8>, x: u64) {
    if x <= 0x7f {
        // Checked just above.
        b.push(u8::try_from(x).unwrap_or(0));
        return;
    }
    let bytes = x.to_be_bytes();
    // Leading zero bytes are dropped; at least one byte remains.
    let skip = (x.leading_zeros() / 8) as usize;
    let n = 8 - skip;
    // The count is sent negated, as a byte.
    b.push(u8::try_from(n).unwrap_or(0).wrapping_neg());
    b.extend_from_slice(&bytes[skip..]);
}

fn encode_int(b: &mut Vec<u8>, i: i64) {
    let x = if i < 0 {
        ((!i) as u64) << 1 | 1
    } else {
        (i as u64) << 1
    };
    encode_uint(b, x);
}

fn encode_float(b: &mut Vec<u8>, f: f64) {
    encode_uint(b, f.to_bits().swap_bytes());
}

fn encode_string(b: &mut Vec<u8>, s: &str) {
    encode_uint(b, s.len() as u64);
    b.extend_from_slice(s.as_bytes());
}

impl Encoder {
    /// `writeMessage`: sends `b` as one message prefixed by its length, and
    /// empties `b`.
    fn write_message(&mut self, b: &mut Vec<u8>) -> Result<()> {
        if b.len() as u64 >= TOO_BIG {
            return Err(invalid("encoder: message too big"));
        }
        encode_uint(&mut self.out, b.len() as u64);
        self.out.append(b);
        Ok(())
    }

    /// `sendTypeDescriptor` for a type that needs a definition: if the stream
    /// has not defined `key` yet, appends `-id` and the definition to `b` and
    /// flushes `b` as a message. `b` may already hold the start of a value
    /// (Go then sends that partial value in the same message). Returns the id.
    fn define(
        &mut self,
        b: &mut Vec<u8>,
        key: &'static str,
        wire: impl FnOnce(&mut Vec<u8>, i32),
    ) -> Result<i32> {
        if let Some(&id) = self.sent.get(key) {
            return Ok(id);
        }
        let id = self.next_id;
        self.next_id += 1;
        encode_int(b, -i64::from(id));
        wire(b, id);
        self.write_message(b)?;
        self.sent.insert(key, id);
        Ok(id)
    }

    /// `encodeInterface`.
    fn encode_interface(&mut self, b: &mut Vec<u8>, v: &Value) -> Result<()> {
        if *v == Value::Nil {
            encode_uint(b, 0);
            return Ok(());
        }
        encode_string(b, v.registered_name());
        let id = match v {
            Value::Nil | Value::Int(_) | Value::Int64(_) | Value::Int32(_) => T_INT,
            Value::Float32(_) | Value::Float64(_) => T_FLOAT,
            Value::Bool(_) => T_BOOL,
            Value::String(_) => T_STRING,
            Value::IntSlice(_) => <Vec<i64>>::send_type(self, b)?,
        };
        encode_int(b, i64::from(id));
        let mut data = Vec::new();
        encode_uint(&mut data, 0);
        match v {
            Value::Nil => {}
            Value::Int(x) | Value::Int64(x) => encode_int(&mut data, *x),
            Value::Int32(x) => encode_int(&mut data, i64::from(*x)),
            Value::Float32(x) => encode_float(&mut data, f64::from(*x)),
            Value::Float64(x) => encode_float(&mut data, *x),
            Value::Bool(x) => encode_uint(&mut data, u64::from(*x)),
            Value::String(x) => encode_string(&mut data, x),
            Value::IntSlice(x) => encode_int_slice(&mut data, x),
        }
        encode_uint(b, data.len() as u64);
        b.append(&mut data);
        Ok(())
    }
}

fn encode_int_slice(b: &mut Vec<u8>, v: &[i64]) {
    encode_uint(b, v.len() as u64);
    for &x in v {
        encode_int(b, x);
    }
}

/// `wireType{SliceT: &sliceType{CommonType{Id: id}, Elem: elem}}`.
fn encode_slice_type(b: &mut Vec<u8>, id: i32, elem: i32) {
    encode_uint(b, 2);
    encode_uint(b, 1);
    encode_common_type(b, "", id);
    encode_uint(b, 1);
    encode_int(b, i64::from(elem));
    encode_uint(b, 0);
    encode_uint(b, 0);
}

/// `CommonType` of a definition: `Name` when not empty, then `Id`.
fn encode_common_type(b: &mut Vec<u8>, name: &str, id: i32) {
    if name.is_empty() {
        encode_uint(b, 2);
    } else {
        encode_uint(b, 1);
        encode_string(b, name);
        encode_uint(b, 1);
    }
    encode_int(b, i64::from(id));
    encode_uint(b, 0);
}

// ---------------------------------------------------------------------------
// The types of the subset.

mod sealed {
    pub trait Sealed {}
    impl Sealed for i64 {}
    impl Sealed for i32 {}
    impl Sealed for bool {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
    impl Sealed for String {}
    impl Sealed for Vec<i64> {}
    impl Sealed for Vec<f32> {}
    impl Sealed for super::Params {}
}

/// A Rust type that stands for one Go type of the subset. Implemented for the
/// types listed in the module documentation only.
pub trait GobValue: Sized + sealed::Sealed {
    /// The Go type, as Go names it in error messages.
    const GO_TYPE: &'static str;

    /// Defines the type in the stream if it needs a definition and returns
    /// its id.
    #[doc(hidden)]
    fn send_type(enc: &mut Encoder, b: &mut Vec<u8>) -> Result<i32>;

    /// Encodes the value itself, without the singleton delta.
    #[doc(hidden)]
    fn encode_body(&self, enc: &mut Encoder, b: &mut Vec<u8>) -> Result<()>;

    /// Whether a value of wire type `wire_id` can be decoded into this type
    /// (`compatibleType`).
    #[doc(hidden)]
    fn compatible(dec: &Decoder<'_>, wire_id: i32) -> bool;

    /// Decodes the value itself, after the singleton delta. `local` names the
    /// local type in error messages.
    #[doc(hidden)]
    fn decode_body(dec: &mut Decoder<'_>, local: &str) -> Result<Self>;
}

impl GobValue for i64 {
    const GO_TYPE: &'static str = "int64";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_INT)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_int(b, *self);
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_INT
    }
    fn decode_body(dec: &mut Decoder<'_>, _: &str) -> Result<Self> {
        dec.decode_int()
    }
}

impl GobValue for i32 {
    const GO_TYPE: &'static str = "int32";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_INT)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_int(b, i64::from(*self));
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_INT
    }
    fn decode_body(dec: &mut Decoder<'_>, local: &str) -> Result<Self> {
        dec.decode_int32(local)
    }
}

impl GobValue for bool {
    const GO_TYPE: &'static str = "bool";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_BOOL)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_uint(b, u64::from(*self));
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_BOOL
    }
    fn decode_body(dec: &mut Decoder<'_>, _: &str) -> Result<Self> {
        dec.decode_bool()
    }
}

impl GobValue for f32 {
    const GO_TYPE: &'static str = "float32";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_FLOAT)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_float(b, f64::from(*self));
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_FLOAT
    }
    fn decode_body(dec: &mut Decoder<'_>, local: &str) -> Result<Self> {
        dec.decode_float32(local)
    }
}

impl GobValue for f64 {
    const GO_TYPE: &'static str = "float64";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_FLOAT)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_float(b, *self);
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_FLOAT
    }
    fn decode_body(dec: &mut Decoder<'_>, _: &str) -> Result<Self> {
        dec.decode_float64()
    }
}

impl GobValue for String {
    const GO_TYPE: &'static str = "string";
    fn send_type(_: &mut Encoder, _: &mut Vec<u8>) -> Result<i32> {
        Ok(T_STRING)
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_string(b, self);
        Ok(())
    }
    fn compatible(_: &Decoder<'_>, wire_id: i32) -> bool {
        wire_id == T_STRING
    }
    fn decode_body(dec: &mut Decoder<'_>, local: &str) -> Result<Self> {
        // decString names the decoded value's own type, not the pointer.
        dec.decode_string(local.trim_start_matches('*'))
    }
}

impl GobValue for Vec<i64> {
    const GO_TYPE: &'static str = "[]int";
    fn send_type(enc: &mut Encoder, b: &mut Vec<u8>) -> Result<i32> {
        enc.define(b, Self::GO_TYPE, |b, id| encode_slice_type(b, id, T_INT))
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_int_slice(b, self);
        Ok(())
    }
    fn compatible(dec: &Decoder<'_>, wire_id: i32) -> bool {
        dec.is_int_slice(wire_id)
    }
    fn decode_body(dec: &mut Decoder<'_>, _: &str) -> Result<Self> {
        dec.decode_int_slice()
    }
}

impl GobValue for Vec<f32> {
    const GO_TYPE: &'static str = "[]float32";
    fn send_type(enc: &mut Encoder, b: &mut Vec<u8>) -> Result<i32> {
        enc.define(b, Self::GO_TYPE, |b, id| encode_slice_type(b, id, T_FLOAT))
    }
    fn encode_body(&self, _: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_uint(b, self.len() as u64);
        for &x in self {
            encode_float(b, f64::from(x));
        }
        Ok(())
    }
    fn compatible(dec: &Decoder<'_>, wire_id: i32) -> bool {
        dec.is_float_slice(wire_id)
    }
    fn decode_body(dec: &mut Decoder<'_>, local: &str) -> Result<Self> {
        dec.decode_float32_slice(local)
    }
}

impl GobValue for Params {
    const GO_TYPE: &'static str = "model.Params";
    fn send_type(enc: &mut Encoder, b: &mut Vec<u8>) -> Result<i32> {
        enc.define(b, Self::GO_TYPE, |b, id| {
            // wireType{MapT: &mapType{CommonType{"Params", id}, tString, tInterface}}
            encode_uint(b, 4);
            encode_uint(b, 1);
            encode_common_type(b, "Params", id);
            encode_uint(b, 1);
            encode_int(b, i64::from(T_STRING));
            encode_uint(b, 1);
            encode_int(b, i64::from(T_INTERFACE));
            encode_uint(b, 0);
            encode_uint(b, 0);
        })
    }
    fn encode_body(&self, enc: &mut Encoder, b: &mut Vec<u8>) -> Result<()> {
        encode_uint(b, self.0.len() as u64);
        for (key, value) in &self.0 {
            encode_string(b, key);
            enc.encode_interface(b, value)?;
        }
        Ok(())
    }
    fn compatible(dec: &Decoder<'_>, wire_id: i32) -> bool {
        dec.is_string_interface_map(wire_id)
    }
    fn decode_body(dec: &mut Decoder<'_>, _: &str) -> Result<Self> {
        dec.decode_params()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint_encoding_matches_go_doc_examples() {
        // From the encoding/gob package documentation.
        let mut b = Vec::new();
        encode_uint(&mut b, 7);
        assert_eq!(b, [0x07]);
        b.clear();
        encode_uint(&mut b, 256);
        assert_eq!(b, [0xfe, 0x01, 0x00]);
        b.clear();
        encode_int(&mut b, -129);
        assert_eq!(b, [0xfe, 0x01, 0x01]);
        b.clear();
        encode_float(&mut b, 17.0);
        assert_eq!(b, [0xfe, 0x31, 0x40]);
    }

    #[test]
    fn decode_uint_rejects_more_than_eight_bytes() {
        let mut src: &[u8] = &[0xf7, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(
            decode_uint_reader(&mut src),
            Err(Error::Invalid(
                "gob: encoded unsigned integer out of range".to_owned()
            ))
        );
    }
}
