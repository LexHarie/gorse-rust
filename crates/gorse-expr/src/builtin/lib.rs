// Ports: github.com/expr-lang/expr/builtin/lib.go, builtin/utils.go and the Func and Safe functions of builtin/builtin.go @ v1.17.8

//! The functions the builtins call at run time. Each takes the values of its
//! arguments as Go's `...any` does and returns the result, or the error text
//! that Go's function returns or panics with, as [`Panic`].

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine as _;
use gorse_common::{gojson, gosort, gostrconv, gotime, goutf8};

use crate::builtin::strings;
use crate::ty::{Kind, Ty};
use crate::value::{self, Panic, Value};
use crate::vm::assertion_failed;

/// `builtin.MaxDepth`: the recursion depth that the nested-array helpers allow.
const MAX_DEPTH: usize = 10_000;

/// `builtin.ErrorMaxDepth`.
const ERROR_MAX_DEPTH: &str = "recursion depth exceeded";

/// The "invalid number of arguments" message of a builtin.
fn count_error(got: usize, expected: &str) -> Panic {
    format!("invalid number of arguments (expected {expected}, got {got})")
}

/// `args[i].(string)`: the text of a string argument.
fn text(v: &Value) -> Result<&[u8], Panic> {
    v.as_str().ok_or_else(|| assertion_failed(v, "string"))
}

/// The string value of some bytes.
fn string(bytes: Vec<u8>) -> Value {
    Value::Str(Rc::from(bytes))
}

/// `reflect.Kind.String()` of a value's type, which some messages print.
fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "invalid",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Integer(ty, _) => ty.kind().name(),
        Value::Float(_) => "float64",
        Value::Str(_) => "string",
        Value::Bytes(_) | Value::Array(_, _) => "slice",
        Value::Map(_, _) => "map",
        Value::Time(_) | Value::Struct(_, _) => "struct",
        Value::Duration(_) => "int64",
        Value::Method(_, _) => "func",
        Value::Location(_) => "ptr",
    }
}

/// The elements of a slice or an array, when the value is one. A byte slice
/// yields its bytes as integers.
fn elements(v: &Value) -> Option<Cow<'_, [Value]>> {
    match v {
        Value::Array(_, items) => Some(Cow::Borrowed(items.as_slice())),
        Value::Bytes(bytes) => Some(Cow::Owned(
            bytes.iter().map(|&b| Value::Int(i64::from(b))).collect(),
        )),
        _ => None,
    }
}

/// A number as `float64`, as `mean` and `median` read one: an integer, a
/// float or a duration (whose kind is int64).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(x) => Some(*x as f64),
        Value::Float(x) => Some(*x),
        Value::Duration(d) => Some(d.0 as f64),
        _ => None,
    }
}

/// Go's `toInt` of the bit functions: an integer, or the error it returns.
fn int_arg(v: &Value) -> Result<i64, Panic> {
    match v {
        Value::Int(x) => Ok(*x),
        other => Err(format!(
            "cannot use {} as argument (type int)",
            other.type_name()
        )),
    }
}

/// Go's `Len`: the length of an array, a map, a byte slice, or a string in runes.
pub fn len(args: &[Value]) -> Result<Value, Panic> {
    let x = &args[0];
    match x {
        Value::Str(s) => Ok(Value::Int(goutf8::rune_count(s) as i64)),
        _ => match elements(x) {
            Some(items) => Ok(Value::Int(items.len() as i64)),
            None => match x {
                Value::Map(_, entries) => Ok(Value::Int(entries.len() as i64)),
                _ => Err(format!("invalid argument for len (type {})", x.type_name())),
            },
        },
    }
}

/// Go's `Type`: the name of the kind of a value, as expr reports it.
pub fn type_of(args: &[Value]) -> Result<Value, Panic> {
    let name = match &args[0] {
        Value::Nil => "nil".to_owned(),
        Value::Bool(_) => "bool".to_owned(),
        Value::Int(_) => "int".to_owned(),
        Value::Integer(ty, _) if ty.is_named() => ty.name().to_owned(),
        Value::Integer(_, _) => "int".to_owned(),
        Value::Float(_) => "float".to_owned(),
        Value::Str(_) => "string".to_owned(),
        Value::Bytes(_) | Value::Array(_, _) => "array".to_owned(),
        Value::Map(_, _) => "map".to_owned(),
        Value::Method(_, _) => "func".to_owned(),
        Value::Time(_) => "time.Time".to_owned(),
        Value::Duration(_) => "time.Duration".to_owned(),
        Value::Struct(ty, _) if ty.is_named() => ty.name().to_owned(),
        Value::Struct(_, _) => "struct".to_owned(),
        Value::Location(_) => "unknown".to_owned(),
    };
    Ok(Value::str(&name))
}

/// Go's `Abs`: the absolute value of an integer or a float.
pub fn abs(args: &[Value]) -> Result<Value, Panic> {
    match &args[0] {
        Value::Int(x) => Ok(Value::Int(if *x < 0 { x.wrapping_neg() } else { *x })),
        Value::Float(x) => Ok(Value::Float(if *x < 0.0 { -x } else { *x })),
        other => Err(format!(
            "invalid argument for abs (type {})",
            other.type_name()
        )),
    }
}

/// Go's `Ceil`, `Floor` and `Round`: a float, rounded.
fn rounding(name: &str, args: &[Value], round: fn(f64) -> f64) -> Result<Value, Panic> {
    match &args[0] {
        Value::Float(x) => Ok(Value::Float(round(*x))),
        Value::Int(x) => Ok(Value::Float(*x as f64)),
        other => Err(format!(
            "invalid argument for {name} (type {})",
            other.type_name()
        )),
    }
}

pub fn ceil(args: &[Value]) -> Result<Value, Panic> {
    rounding("ceil", args, f64::ceil)
}

pub fn floor(args: &[Value]) -> Result<Value, Panic> {
    rounding("floor", args, f64::floor)
}

pub fn round(args: &[Value]) -> Result<Value, Panic> {
    rounding("round", args, f64::round)
}

/// Go's `Int`: an integer from a number, a duration or a string.
pub fn int(args: &[Value]) -> Result<Value, Panic> {
    match &args[0] {
        Value::Int(x) => Ok(Value::Int(*x)),
        Value::Float(x) => Ok(Value::Int(value::float_to_int(*x))),
        Value::Duration(d) => Ok(Value::Int(d.0)),
        Value::Str(s) => {
            let (n, err) = gostrconv::atoi(s);
            match err {
                None => Ok(Value::Int(n)),
                Some(_) => Err(format!(
                    "invalid operation: int({})",
                    String::from_utf8_lossy(s)
                )),
            }
        }
        other => Err(format!("invalid operation: int({})", other.type_name())),
    }
}

/// Go's `Float`: a float from a number or a string.
pub fn float(args: &[Value]) -> Result<Value, Panic> {
    match &args[0] {
        Value::Float(x) => Ok(Value::Float(*x)),
        Value::Int(x) => Ok(Value::Float(*x as f64)),
        Value::Str(s) => {
            let (f, err) = gostrconv::parse_float(s, 64);
            match err {
                None => Ok(Value::Float(f)),
                Some(_) => Err(format!(
                    "invalid operation: float({})",
                    String::from_utf8_lossy(s)
                )),
            }
        }
        other => Err(format!("invalid operation: float({})", other.type_name())),
    }
}

/// Go's `String`: the `%v` text of a value.
pub fn string_of(args: &[Value]) -> Result<Value, Panic> {
    Ok(Value::str(&args[0].go_v()))
}

/// The order that a `min` or a `max` keeps: `Less` for the maximum, `More` for
/// the minimum.
type Order = fn(&Value, &Value) -> Result<bool, Panic>;

/// Replaces `val` with `x` when `val` is unset or `x` comes first in `order`.
fn better(order: Order, val: &mut Option<Value>, x: &Value) -> Result<(), Panic> {
    let take = match val {
        None => true,
        Some(current) => order(current, x)?,
    };
    if take {
        *val = Some(x.clone());
    }
    Ok(())
}

/// Go's `minMax`: the extreme number of the arguments, looking into arrays.
fn min_max(name: &str, order: Order, depth: usize, args: &[Value]) -> Result<Value, Panic> {
    if depth > MAX_DEPTH {
        return Err(ERROR_MAX_DEPTH.to_owned());
    }
    let mut val = None;
    for arg in args {
        match arg {
            // Go's fast paths for []int, []float64 and []any take the elements
            // one by one; a nested array is a single value of its own.
            Value::Array(ty, items)
                if matches!(
                    ty.elem().map(Ty::kind),
                    Some(Kind::Interface | Kind::Int | Kind::Float64)
                ) =>
            {
                for elem in items.iter() {
                    match elem {
                        Value::Int(_) | Value::Float(_) => better(order, &mut val, elem)?,
                        Value::Array(_, _) | Value::Bytes(_) => {
                            let nested =
                                min_max(name, order, depth + 1, std::slice::from_ref(elem))?;
                            if !nested.is_nil() {
                                better(order, &mut val, &nested)?;
                            }
                        }
                        other => {
                            return Err(format!(
                                "invalid argument for {name} (type {})",
                                other.type_name()
                            ));
                        }
                    }
                }
            }
            // Go's slow path: each element on its own, which must come out a number.
            Value::Array(_, _) | Value::Bytes(_) => {
                for elem in elements(arg).unwrap_or_default().iter() {
                    let elem_val = min_max(name, order, depth + 1, std::slice::from_ref(elem))?;
                    match elem_val {
                        Value::Int(_) | Value::Float(_) => better(order, &mut val, &elem_val)?,
                        other => {
                            return Err(format!(
                                "invalid argument for {name} (type {})",
                                other.type_name()
                            ));
                        }
                    }
                }
            }
            Value::Int(_) | Value::Float(_) | Value::Duration(_) => better(order, &mut val, arg)?,
            _ => {
                if args.len() == 1 {
                    return Ok(arg.clone());
                }
                return Err(format!(
                    "invalid argument for {name} (type {})",
                    arg.type_name()
                ));
            }
        }
    }
    Ok(val.unwrap_or(Value::Nil))
}

pub fn max(args: &[Value]) -> Result<Value, Panic> {
    min_max("max", value::less, 0, args)
}

pub fn min(args: &[Value]) -> Result<Value, Panic> {
    min_max("min", value::more, 0, args)
}

/// Go's `mean`: the count and the sum of the numbers in the arguments.
fn mean_parts(depth: usize, args: &[Value]) -> Result<(i64, f64), Panic> {
    if depth > MAX_DEPTH {
        return Err(ERROR_MAX_DEPTH.to_owned());
    }
    let (mut count, mut total) = (0, 0.0);
    for arg in args {
        if let Some(elems) = elements(arg) {
            for elem in elems.iter() {
                let (c, s) = mean_parts(depth + 1, std::slice::from_ref(elem))?;
                count += c;
                total += s;
            }
        } else {
            match number(arg) {
                Some(x) => {
                    count += 1;
                    total += x;
                }
                None => {
                    return Err(format!(
                        "invalid argument for mean (type {})",
                        arg.type_name()
                    ));
                }
            }
        }
    }
    Ok((count, total))
}

pub fn mean(args: &[Value]) -> Result<Value, Panic> {
    let (count, sum) = mean_parts(0, args)?;
    if count == 0 {
        return Ok(Value::Float(0.0));
    }
    Ok(Value::Float(sum / count as f64))
}

/// Go's `median`: every number of the arguments, looking into arrays.
fn median_values(depth: usize, args: &[Value]) -> Result<Vec<f64>, Panic> {
    if depth > MAX_DEPTH {
        return Err(ERROR_MAX_DEPTH.to_owned());
    }
    let mut values = Vec::new();
    for arg in args {
        if let Some(elems) = elements(arg) {
            for elem in elems.iter() {
                values.extend(median_values(depth + 1, std::slice::from_ref(elem))?);
            }
        } else {
            match number(arg) {
                Some(x) => values.push(x),
                None => {
                    return Err(format!(
                        "invalid argument for median (type {})",
                        arg.type_name()
                    ));
                }
            }
        }
    }
    Ok(values)
}

/// `sort.Float64Slice`: NaN sorts before the other numbers.
struct Floats<'a>(&'a mut Vec<f64>);

impl gosort::Interface for Floats<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        let (x, y) = (self.0[i], self.0[j]);
        x < y || (x.is_nan() && !y.is_nan())
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.0.swap(i, j);
    }
}

pub fn median(args: &[Value]) -> Result<Value, Panic> {
    let mut values = median_values(0, args)?;
    gosort::sort(&mut Floats(&mut values));
    let n = values.len();
    if n == 0 {
        return Ok(Value::Float(0.0));
    }
    if n % 2 == 1 {
        return Ok(Value::Float(values[n / 2]));
    }
    Ok(Value::Float((values[n / 2 - 1] + values[n / 2]) / 2.0))
}

/// Go's `toJSON`: `json.MarshalIndent` with two spaces.
pub fn to_json(args: &[Value]) -> Result<Value, Panic> {
    let mut out = Vec::new();
    encode_json(&args[0], 0, &mut out)?;
    Ok(string(out))
}

fn indent(depth: usize, out: &mut Vec<u8>) {
    for _ in 0..depth {
        out.extend_from_slice(b"  ");
    }
}

/// Writes a value as `encoding/json` does, indented as `MarshalIndent` does.
fn encode_json(v: &Value, depth: usize, out: &mut Vec<u8>) -> Result<(), Panic> {
    match v {
        Value::Nil => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Int(x) | Value::Integer(_, x) => out.extend_from_slice(x.to_string().as_bytes()),
        Value::Duration(d) => out.extend_from_slice(d.0.to_string().as_bytes()),
        Value::Float(f) => {
            if f.is_nan() || f.is_infinite() {
                return Err(format!(
                    "json: unsupported value: {}",
                    value::format_float_v(*f)
                ));
            }
            let abs = f.abs();
            let fmt = if abs != 0.0 && (abs < 1e-6 || abs >= 1e21) {
                b'e'
            } else {
                b'f'
            };
            let mut text = gostrconv::format_float(*f, fmt, -1, 64).into_bytes();
            if fmt == b'e' {
                // Go drops the zero of a two-digit negative exponent: e-09 is e-9.
                let n = text.len();
                if n >= 4 && text[n - 4] == b'e' && text[n - 3] == b'-' && text[n - 2] == b'0' {
                    text[n - 2] = text[n - 1];
                    text.truncate(n - 1);
                }
            }
            out.extend_from_slice(&text);
        }
        Value::Str(s) => encode_json_string(s, out),
        Value::Bytes(b) => {
            let text = base64::engine::general_purpose::STANDARD.encode(b);
            encode_json_string(text.as_bytes(), out);
        }
        Value::Time(t) => {
            let text = t.format(gotime::RFC3339_NANO);
            encode_json_string(text.as_bytes(), out);
        }
        Value::Array(_, items) => {
            if items.is_empty() {
                out.extend_from_slice(b"[]");
                return Ok(());
            }
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.push(b'\n');
                indent(depth + 1, out);
                encode_json(item, depth + 1, out)?;
            }
            out.push(b'\n');
            indent(depth, out);
            out.push(b']');
        }
        Value::Map(ty, entries) => {
            let mut members: Vec<(String, &Value)> = Vec::with_capacity(entries.len());
            for (key, value) in entries.iter() {
                let name = match key {
                    Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
                    Value::Int(x) => x.to_string(),
                    _ => return Err(format!("json: unsupported type: {}", ty.name())),
                };
                members.push((name, value));
            }
            members.sort_by(|a, b| a.0.cmp(&b.0));
            encode_members(members, depth, out)?;
        }
        Value::Struct(ty, fields) => {
            encode_members(json_members(ty, fields), depth, out)?;
        }
        Value::Location(_) | Value::Method(_, _) => {
            return Err(format!("json: unsupported type: {}", v.type_name()));
        }
    }
    Ok(())
}

/// The members of a struct as `encoding/json` writes them: its exported
/// fields, with the fields of an embedded struct written in its place. Go's
/// rule for two members of one name, where the shallower wins, is not ported:
/// no value of the expressions has one.
fn json_members<'a>(ty: &crate::ty::Ty, fields: &'a [Value]) -> Vec<(String, &'a Value)> {
    let mut members = Vec::new();
    for (field, value) in ty.fields().iter().zip(fields) {
        match value {
            Value::Struct(inner, inner_fields) if field.embedded => {
                members.extend(json_members(inner, &inner_fields[..]));
            }
            _ if field.name.starts_with(char::is_uppercase) => {
                members.push((field.name.clone(), value));
            }
            _ => {}
        }
    }
    members
}

/// Writes the members of an object, in the order given.
fn encode_members(
    members: Vec<(String, &Value)>,
    depth: usize,
    out: &mut Vec<u8>,
) -> Result<(), Panic> {
    if members.is_empty() {
        out.extend_from_slice(b"{}");
        return Ok(());
    }
    out.push(b'{');
    for (i, (name, value)) in members.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.push(b'\n');
        indent(depth + 1, out);
        encode_json_string(name.as_bytes(), out);
        out.extend_from_slice(b": ");
        encode_json(value, depth + 1, out)?;
    }
    out.push(b'\n');
    indent(depth, out);
    out.push(b'}');
    Ok(())
}

/// A JSON string with the escapes `encoding/json` writes, HTML-safe ones included.
fn encode_json_string(s: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let mut rest = s;
    while !rest.is_empty() {
        let (r, size) = goutf8::decode_rune(rest);
        let c = if r == 0xFFFD && size == 1 {
            None
        } else {
            char::from_u32(r)
        };
        match c {
            None => out.extend_from_slice(b"\\ufffd"),
            Some(c @ ('"' | '\\')) => {
                out.push(b'\\');
                out.push(c as u8);
            }
            Some('\n') => out.extend_from_slice(b"\\n"),
            Some('\r') => out.extend_from_slice(b"\\r"),
            Some('\t') => out.extend_from_slice(b"\\t"),
            Some(c @ ('<' | '>' | '&')) => {
                let c = c as u8;
                out.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(c >> 4)],
                    HEX[usize::from(c & 15)],
                ]);
            }
            Some(c) if (c as u32) < 0x20 => {
                let c = c as u8;
                out.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(c >> 4)],
                    HEX[usize::from(c & 15)],
                ]);
            }
            Some('\u{2028}') => out.extend_from_slice(b"\\u2028"),
            Some('\u{2029}') => out.extend_from_slice(b"\\u2029"),
            Some(c) => {
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        rest = &rest[size..];
    }
    out.push(b'"');
}

/// Go's `fromJSON`: the value of a JSON document, numbers as float64.
pub fn from_json(args: &[Value]) -> Result<Value, Panic> {
    let data = text(&args[0])?;
    let document = gojson::unmarshal_any(data).map_err(|e| e.to_string())?;
    Ok(json_value(&document))
}

fn json_value(v: &gojson::Value) -> Value {
    match v {
        gojson::Value::Null => Value::Nil,
        gojson::Value::Bool(b) => Value::Bool(*b),
        gojson::Value::Number(f) => Value::Float(*f),
        gojson::Value::String(s) => Value::str(s),
        gojson::Value::Array(items) => Value::any_array(items.iter().map(json_value).collect()),
        gojson::Value::Object(members) => Value::string_map(
            members
                .iter()
                .map(|(k, v)| (Value::str(k), json_value(v)))
                .collect(),
        ),
    }
}

/// Go's `toBase64`: the standard encoding of the bytes of a string.
pub fn to_base64(args: &[Value]) -> Result<Value, Panic> {
    let data = text(&args[0])?;
    Ok(Value::str(
        &base64::engine::general_purpose::STANDARD.encode(data),
    ))
}

/// Go's `fromBase64`: the bytes of the standard encoding in a string.
pub fn from_base64(args: &[Value]) -> Result<Value, Panic> {
    let data = text(&args[0])?;
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut si = 0;
    while si < data.len() {
        si = decode_base64_quantum(data, si, &mut out)
            .map_err(|offset| format!("illegal base64 data at input byte {offset}"))?;
    }
    Ok(string(out))
}

/// The value of a character of the standard base64 alphabet.
fn base64_digit(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a') + 26,
        b'0'..=b'9' => u32::from(c - b'0') + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

/// Decodes one quantum of four characters, as Go's `decodeQuantum` does for
/// the padded standard encoding. An error is the input offset Go reports.
fn decode_base64_quantum(src: &[u8], mut si: usize, out: &mut Vec<u8>) -> Result<usize, usize> {
    let mut dbuf = [0_u32; 4];
    let mut dlen = 4;
    let mut j = 0;
    while j < dbuf.len() {
        if si == src.len() {
            if j == 0 {
                return Ok(si);
            }
            if j == 1 {
                return Err(si - j);
            }
            dlen = j;
            break;
        }
        let c = src[si];
        si += 1;
        if let Some(digit) = base64_digit(c) {
            dbuf[j] = digit;
            j += 1;
            continue;
        }
        if c == b'\n' || c == b'\r' {
            continue;
        }
        if c != b'=' {
            return Err(si - 1);
        }
        match j {
            0 | 1 => return Err(si - 1),
            2 => {
                while si < src.len() && matches!(src[si], b'\n' | b'\r') {
                    si += 1;
                }
                if si == src.len() {
                    return Err(src.len());
                }
                if src[si] != b'=' {
                    return Err(si - 1);
                }
                si += 1;
            }
            _ => {}
        }
        while si < src.len() && matches!(src[si], b'\n' | b'\r') {
            si += 1;
        }
        if si < src.len() {
            return Err(si);
        }
        dlen = j;
        break;
    }
    let val = (dbuf[0] << 18) | (dbuf[1] << 12) | (dbuf[2] << 6) | dbuf[3];
    let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
    out.extend_from_slice(&bytes[..dlen - 1]);
    Ok(si)
}

/// Go's `now`: the current time, or the current time in a location.
pub fn now(args: &[Value]) -> Result<Value, Panic> {
    match args {
        [] => Ok(value::now()),
        [Value::Location(loc)] => match value::now() {
            Value::Time(t) => Ok(Value::Time(t.in_location(loc))),
            other => Ok(other),
        },
        _ => Err(count_error(args.len(), "0")),
    }
}

/// Go's `duration`: the duration a string such as `1h30m` names.
pub fn duration(args: &[Value]) -> Result<Value, Panic> {
    let data = text(&args[0])?;
    gotime::parse_duration(data)
        .map(Value::Duration)
        .map_err(|e| e.to_string())
}

/// The layouts `date` tries, in Go's order, when it is not given one.
const DATE_LAYOUTS: [&str; 7] = [
    "2006-01-02",
    "15:04:05",
    "2006-01-02 15:04:05",
    gotime::RFC3339,
    gotime::RFC822,
    gotime::RFC850,
    gotime::RFC1123,
];

/// Go's `date`: a time parsed from a string, with an optional location, and
/// an optional layout and location name.
pub fn date(args: &[Value]) -> Result<Value, Panic> {
    let (location, args) = match args.first() {
        Some(Value::Location(loc)) => (Some(loc.clone()), &args[1..]),
        _ => (None, args),
    };
    let Some(first) = args.first() else {
        return Err("runtime error: index out of range [0] with length 0".to_owned());
    };
    let date = text(first)?;
    let parse = |layout: &[u8], date: &[u8]| match &location {
        Some(loc) => gotime::parse_in_location(layout, date, loc),
        None => gotime::parse(layout, date),
    };
    match args.len() {
        2 => {
            let layout = text(&args[1])?;
            parse(layout, date)
                .map(Value::Time)
                .map_err(|e| e.to_string())
        }
        3 => {
            let layout = text(&args[1])?;
            let zone = gotime::load_location(&String::from_utf8_lossy(text(&args[2])?))
                .map_err(|e| e.to_string())?;
            gotime::parse_in_location(layout, date, &zone)
                .map(Value::Time)
                .map_err(|e| e.to_string())
        }
        _ => {
            for layout in DATE_LAYOUTS {
                if let Ok(t) = parse(layout.as_bytes(), date) {
                    return Ok(Value::Time(t));
                }
            }
            Err(format!("invalid date {}", String::from_utf8_lossy(date)))
        }
    }
}

/// Go's `timezone`: the location a name such as `Europe/Paris` names.
pub fn timezone(args: &[Value]) -> Result<Value, Panic> {
    let name = String::from_utf8_lossy(text(&args[0])?).into_owned();
    gotime::load_location(&name)
        .map(Value::Location)
        .map_err(|e| e.to_string())
}

/// Go's `first`: the first element, or nil when there is none.
pub fn first(args: &[Value]) -> Result<Value, Panic> {
    Ok(value::fetch(&args[0], &Value::Int(0)).unwrap_or(Value::Nil))
}

/// Go's `last`: the last element, or nil when there is none.
pub fn last(args: &[Value]) -> Result<Value, Panic> {
    Ok(value::fetch(&args[0], &Value::Int(-1)).unwrap_or(Value::Nil))
}

/// Go's `get`: the element, key or field of a value, or nil when it has none.
pub fn get(args: &[Value]) -> Result<Value, Panic> {
    if args.len() < 2 {
        return Err(count_error(args.len(), "2"));
    }
    let (from, i) = (&args[0], &args[1]);
    match from {
        Value::Nil => return Ok(Value::Nil),
        Value::Time(_) | Value::Duration(_) => {
            if let Value::Str(name) = i
                && let Ok(method) = value::fetch_method(from, &String::from_utf8_lossy(name)) {
                    return Ok(method);
                }
        }
        _ => {}
    }
    match from {
        Value::Str(_) | Value::Bytes(_) | Value::Array(_, _) => {
            let index = value::to_int(i)?;
            let length = elements(from)
                .map_or_else(|| value::len(from).unwrap_or(0), |items| items.len() as i64);
            let index = if index < 0 { length + index } else { index };
            if 0 <= index && index < length {
                return value::fetch(from, &Value::Int(index));
            }
        }
        Value::Map(ty, _) => {
            let key_ty = ty.key().cloned().unwrap_or_else(Ty::any);
            let key = if i.is_nil() {
                value::zero_value(&key_ty)
            } else {
                if key_ty.kind() != Kind::Interface && i.ty().as_ref() != Some(&key_ty) {
                    return Err(format!(
                        "reflect.Value.MapIndex: value of type {} is not assignable to type {}",
                        i.type_name(),
                        key_ty.name()
                    ));
                }
                i.clone()
            };
            if let Value::Map(_, entries) = from
                && let Some((_, found)) =
                    entries.iter().find(|(k, _)| value::key_identical(k, &key))
                {
                    return Ok(found.clone());
                }
        }
        Value::Struct(ty, _) => {
            let Value::Str(name) = i else {
                return Err(assertion_failed(i, "string"));
            };
            if ty.field_index(&String::from_utf8_lossy(name)).is_some() {
                return value::fetch(from, i);
            }
        }
        _ => {}
    }
    Ok(Value::Nil)
}

/// Go's `take`: the first `n` elements of a slice or an array, or all of them.
pub fn take(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 2 {
        return Err(count_error(args.len(), "2"));
    }
    let length = match elements(&args[0]) {
        Some(items) => items.len() as i64,
        None => {
            return Err(format!("cannot take from {}", kind_name(&args[0])));
        }
    };
    let n = match &args[1] {
        Value::Int(n) => *n,
        Value::Duration(d) => d.0,
        other => return Err(format!("cannot take {} elements", kind_name(other))),
    };
    let to = if n > length { length } else { n };
    if to < 0 {
        return Err("reflect.Value.Slice: slice index out of bounds".to_owned());
    }
    let to = to as usize;
    Ok(match &args[0] {
        Value::Array(ty, items) => Value::Array(ty.clone(), Rc::new(items[..to].to_vec())),
        Value::Bytes(bytes) => Value::Bytes(Rc::from(&bytes[..to])),
        _ => unreachable!("checked above"),
    })
}

/// Go's `keys`: the keys of a map, as an `[]any`.
pub fn keys(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    match &args[0] {
        Value::Map(_, entries) => Ok(Value::any_array(
            entries.iter().map(|(k, _)| k.clone()).collect(),
        )),
        other => Err(format!("cannot get keys from {}", kind_name(other))),
    }
}

/// Go's `values`: the values of a map, as an `[]any`.
pub fn values(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    match &args[0] {
        Value::Map(_, entries) => Ok(Value::any_array(
            entries.iter().map(|(_, v)| v.clone()).collect(),
        )),
        other => Err(format!("cannot get values from {}", kind_name(other))),
    }
}

/// Go's `toPairs`: the key and value pairs of a map, as `[][2]any`.
pub fn to_pairs(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    match &args[0] {
        Value::Map(_, entries) => {
            let pairs = entries
                .iter()
                .map(|(k, v)| {
                    Value::Array(Ty::array(Ty::any(), 2), Rc::new(vec![k.clone(), v.clone()]))
                })
                .collect();
            Ok(Value::Array(
                Ty::slice(Ty::array(Ty::any(), 2)),
                Rc::new(pairs),
            ))
        }
        other => Err(format!("cannot transform {} to pairs", kind_name(other))),
    }
}

/// Go's `fromPairs`: the map of the pairs in a slice or an array.
pub fn from_pairs(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    let Some(pairs) = elements(&args[0]) else {
        return Err(format!("cannot transform {} from pairs", fmt_s(&args[0])));
    };
    let mut entries: Vec<(Value, Value)> = Vec::new();
    for pair in pairs.iter() {
        let items = match elements(pair) {
            Some(items) => items,
            None => return Err(format!("invalid pair {}", pair.go_v())),
        };
        if items.len() != 2 {
            return Err(format!("invalid pair length {}", pair.go_v()));
        }
        let (key, value) = (items[0].clone(), items[1].clone());
        if matches!(key, Value::Array(_, _) | Value::Map(_, _) | Value::Bytes(_)) {
            return Err(format!(
                "runtime error: hash of unhashable type {}",
                key.type_name()
            ));
        }
        match entries
            .iter_mut()
            .find(|(k, _)| value::key_identical(k, &key))
        {
            Some(entry) => entry.1 = value,
            None => entries.push((key, value)),
        }
    }
    Ok(Value::Map(Ty::map(Ty::any(), Ty::any()), Rc::new(entries)))
}

/// How `%s` prints a value handed to it as a `reflect.Value`.
fn fmt_s(v: &Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
        Value::Nil => "<invalid reflect.Value>".to_owned(),
        other => format!("%!s({}={})", other.type_name(), other.go_v()),
    }
}

/// Go's `reverse`: the elements of a slice or an array, in reverse order.
pub fn reverse(args: &[Value]) -> Result<(Value, u64), Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    let Some(items) = elements(&args[0]) else {
        return Err(format!("cannot reverse {}", kind_name(&args[0])));
    };
    let reversed: Vec<Value> = items.iter().rev().cloned().collect();
    let size = reversed.len() as u64;
    Ok((Value::any_array(reversed), size))
}

/// Go's `uniq`: the elements of a slice or an array, each one once.
pub fn uniq(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    let Some(items) = elements(&args[0]) else {
        return Err(format!("cannot uniq {}", kind_name(&args[0])));
    };
    let mut ret: Vec<Value> = Vec::new();
    for item in items.iter() {
        if !ret.iter().any(|r| value::equal(item, r)) {
            ret.push(item.clone());
        }
    }
    Ok(Value::any_array(ret))
}

/// Go's `concat`: the elements of the slices and arrays, one after another.
pub fn concat(args: &[Value]) -> Result<(Value, u64), Panic> {
    if args.is_empty() {
        return Err("invalid number of arguments (expected at least 1, got 0)".to_owned());
    }
    let mut out = Vec::new();
    for arg in args {
        match elements(arg) {
            Some(items) => out.extend(items.iter().cloned()),
            None => return Err(format!("cannot concat {}", kind_name(arg))),
        }
    }
    let size = out.len() as u64;
    Ok((Value::any_array(out), size))
}

/// Go's `flatten`: the elements of a slice, with the nested arrays spliced in.
fn flatten_into(depth: usize, arg: &[Value], out: &mut Vec<Value>) -> Result<(), Panic> {
    if depth > MAX_DEPTH {
        return Err(ERROR_MAX_DEPTH.to_owned());
    }
    for item in arg {
        match elements(item) {
            Some(items) => flatten_into(depth + 1, &items, out)?,
            None => out.push(item.clone()),
        }
    }
    Ok(())
}

pub fn flatten(args: &[Value]) -> Result<(Value, u64), Panic> {
    if args.len() != 1 {
        return Err(count_error(args.len(), "1"));
    }
    let Some(items) = elements(&args[0]) else {
        return Err(format!("cannot flatten {}", kind_name(&args[0])));
    };
    let mut out = Vec::new();
    flatten_into(0, &items, &mut out)?;
    let size = out.len() as u64;
    Ok((Value::any_array(out), size))
}

/// Go's `runtime.Sort`: the values in order, ascending or descending.
struct Sort {
    desc: bool,
    items: Vec<Value>,
    // The first error `Less` met; Go's `Less` panics where this records it.
    error: RefCell<Option<Panic>>,
}

impl gosort::Interface for Sort {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        let (a, b) = if self.desc {
            (&self.items[j], &self.items[i])
        } else {
            (&self.items[i], &self.items[j])
        };
        match value::less(a, b) {
            Ok(less) => less,
            Err(err) => {
                self.error.borrow_mut().get_or_insert(err);
                false
            }
        }
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.items.swap(i, j);
    }
}

/// Go's `sort`: the elements of a slice of numbers or strings, in order.
pub fn sort(args: &[Value]) -> Result<(Value, u64), Panic> {
    if args.len() != 1 && args.len() != 2 {
        return Err(count_error(args.len(), "1 or 2"));
    }
    let items = match &args[0] {
        Value::Array(ty, items)
            if matches!(
                ty.elem().map(Ty::kind),
                Some(Kind::Interface | Kind::Int | Kind::Float64 | Kind::String)
            ) =>
        {
            items.to_vec()
        }
        _ => Vec::new(),
    };
    let mut desc = false;
    if let Some(order) = args.get(1) {
        let Some(order) = order.as_str() else {
            return Err(format!(
                "sort order argument must be a string (got {})",
                order.type_name()
            ));
        };
        desc = match order {
            b"asc" => false,
            b"desc" => true,
            _ => {
                return Err(format!(
                    "invalid order {}, expected asc or desc",
                    String::from_utf8_lossy(order)
                ));
            }
        };
    }
    let size = items.len() as u64;
    let mut sortable = Sort {
        desc,
        items,
        error: RefCell::new(None),
    };
    gosort::sort(&mut sortable);
    if let Some(err) = sortable.error.into_inner() {
        return Err(err);
    }
    Ok((Value::any_array(sortable.items), size))
}

/// The bit functions: two integers in, one integer out.
fn bit(name: &str, args: &[Value], op: fn(i64, i64) -> Result<i64, Panic>) -> Result<Value, Panic> {
    if args.len() != 2 {
        return Err(format!(
            "invalid number of arguments for {name} (expected 2, got {})",
            args.len()
        ));
    }
    let x = int_arg(&args[0]).map_err(|e| format!("{e} to call {name}"))?;
    let y = int_arg(&args[1]).map_err(|e| format!("{e} to call {name}"))?;
    op(x, y).map(Value::Int)
}

fn negative_shift(y: i64) -> Panic {
    format!("invalid operation: negative shift count {y} (type int)")
}

pub fn bitand(args: &[Value]) -> Result<Value, Panic> {
    bit("bitand", args, |x, y| Ok(x & y))
}

pub fn bitor(args: &[Value]) -> Result<Value, Panic> {
    bit("bitor", args, |x, y| Ok(x | y))
}

pub fn bitxor(args: &[Value]) -> Result<Value, Panic> {
    bit("bitxor", args, |x, y| Ok(x ^ y))
}

pub fn bitnand(args: &[Value]) -> Result<Value, Panic> {
    bit("bitnand", args, |x, y| Ok(x & !y))
}

pub fn bitshl(args: &[Value]) -> Result<Value, Panic> {
    bit("bitshl", args, |x, y| {
        if y < 0 {
            return Err(negative_shift(y));
        }
        Ok(u32::try_from(y)
            .ok()
            .and_then(|s| x.checked_shl(s))
            .unwrap_or(0))
    })
}

pub fn bitshr(args: &[Value]) -> Result<Value, Panic> {
    bit("bitshr", args, |x, y| {
        if y < 0 {
            return Err(negative_shift(y));
        }
        Ok(match u32::try_from(y).ok().and_then(|s| x.checked_shr(s)) {
            Some(v) => v,
            None if x < 0 => -1,
            None => 0,
        })
    })
}

pub fn bitushr(args: &[Value]) -> Result<Value, Panic> {
    bit("bitushr", args, |x, y| {
        if y < 0 {
            return Err(negative_shift(y));
        }
        Ok(u32::try_from(y)
            .ok()
            .and_then(|s| (x as u64).checked_shr(s))
            .map_or(0, |v| v as i64))
    })
}

pub fn bitnot(args: &[Value]) -> Result<Value, Panic> {
    if args.len() != 1 {
        return Err(format!(
            "invalid number of arguments for bitnot (expected 1, got {})",
            args.len()
        ));
    }
    let x = int_arg(&args[0]).map_err(|e| format!("{e} to call bitnot"))?;
    Ok(Value::Int(!x))
}

/// Go's `trim`: the spaces around a string, or the characters of a cutset.
pub fn trim(args: &[Value]) -> Result<Value, Panic> {
    match args.len() {
        1 => Ok(string(strings::trim_space(text(&args[0])?))),
        2 => Ok(string(strings::trim(text(&args[0])?, text(&args[1])?))),
        n => Err(format!(
            "invalid number of arguments for trim (expected 1 or 2, got {n})"
        )),
    }
}

/// Go's `trimPrefix` and `trimSuffix`, which trim a space when given no cutset.
pub fn trim_prefix(args: &[Value]) -> Result<Value, Panic> {
    let sep = if args.len() == 2 {
        text(&args[1])?
    } else {
        b" "
    };
    Ok(string(strings::trim_prefix(text(&args[0])?, sep)))
}

pub fn trim_suffix(args: &[Value]) -> Result<Value, Panic> {
    let sep = if args.len() == 2 {
        text(&args[1])?
    } else {
        b" "
    };
    Ok(string(strings::trim_suffix(text(&args[0])?, sep)))
}

pub fn upper(args: &[Value]) -> Result<Value, Panic> {
    Ok(string(strings::to_upper(text(&args[0])?)))
}

pub fn lower(args: &[Value]) -> Result<Value, Panic> {
    Ok(string(strings::to_lower(text(&args[0])?)))
}

/// `[]string` of the pieces of a split.
fn string_slice(pieces: Vec<Vec<u8>>) -> Value {
    let items = pieces.into_iter().map(string).collect();
    Value::Array(Ty::slice(Ty::string()), Rc::new(items))
}

/// Go's `split`, `splitAfter`: the pieces of a string around a separator.
pub fn split(args: &[Value]) -> Result<Value, Panic> {
    match args.len() {
        2 => Ok(string_slice(strings::split(
            text(&args[0])?,
            text(&args[1])?,
            0,
            -1,
        ))),
        3 => {
            let n = value::to_int(&args[2])?;
            Ok(string_slice(strings::split(
                text(&args[0])?,
                text(&args[1])?,
                0,
                n,
            )))
        }
        n => Err(format!(
            "invalid number of arguments for split (expected 2 or 3, got {n})"
        )),
    }
}

pub fn split_after(args: &[Value]) -> Result<Value, Panic> {
    let (s, sep) = (text(&args[0])?, text(&args[1])?);
    match args.len() {
        2 => Ok(string_slice(strings::split(s, sep, sep.len(), -1))),
        3 => {
            let n = value::to_int(&args[2])?;
            Ok(string_slice(strings::split(s, sep, sep.len(), n)))
        }
        n => Err(format!(
            "invalid number of arguments for splitAfter (expected 2 or 3, got {n})"
        )),
    }
}

/// Go's `replace`: a replacement, for all occurrences or the first `n`.
pub fn replace(args: &[Value]) -> Result<Value, Panic> {
    match args.len() {
        4 => {
            let (s, old, new) = (text(&args[0])?, text(&args[1])?, text(&args[2])?);
            let n = value::to_int(&args[3])?;
            Ok(string(strings::replace(s, old, new, n)))
        }
        3 => Ok(string(strings::replace(
            text(&args[0])?,
            text(&args[1])?,
            text(&args[2])?,
            -1,
        ))),
        n => Err(format!(
            "invalid number of arguments for replace (expected 3 or 4, got {n})"
        )),
    }
}

/// Go's `repeat`, which also charges the memory it takes.
pub fn repeat(args: &[Value]) -> Result<(Value, u64), Panic> {
    let s = text(&args[0])?;
    let n = value::to_int(&args[1])?;
    if n < 0 {
        return Err(format!(
            "invalid argument for repeat (expected positive integer, got {n})"
        ));
    }
    if n > 1_000_000 {
        return Err("memory budget exceeded".to_owned());
    }
    let n = n as usize;
    Ok((string(s.repeat(n)), (s.len() * n) as u64))
}

/// Go's `join`: the elements of an array of strings, with a separator.
pub fn join(args: &[Value]) -> Result<Value, Panic> {
    let glue = if args.len() == 2 {
        text(&args[1])?
    } else {
        b""
    };
    match elements(&args[0]) {
        Some(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(text(item)?);
            }
            Ok(string(parts.join(glue)))
        }
        None => Err(format!(
            "invalid argument for join (type {})",
            match &args[0] {
                Value::Nil => "%!s(<nil>)".to_owned(),
                other => other.type_name(),
            }
        )),
    }
}

/// Go's `indexOf`: the byte index of the first occurrence, or -1.
pub fn index_of(args: &[Value]) -> Result<Value, Panic> {
    Ok(Value::Int(strings::index(text(&args[0])?, text(&args[1])?)))
}

/// Go's `lastIndexOf`: the byte index of the last occurrence, or -1.
pub fn last_index_of(args: &[Value]) -> Result<Value, Panic> {
    Ok(Value::Int(strings::last_index(
        text(&args[0])?,
        text(&args[1])?,
    )))
}

pub fn has_prefix(args: &[Value]) -> Result<Value, Panic> {
    Ok(Value::Bool(text(&args[0])?.starts_with(text(&args[1])?)))
}

pub fn has_suffix(args: &[Value]) -> Result<Value, Panic> {
    Ok(Value::Bool(text(&args[0])?.ends_with(text(&args[1])?)))
}
