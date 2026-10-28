// Ports: github.com/expr-lang/expr/vm/runtime/{runtime,helpers,sort}.go and the
// value side of vm/vm.go @ v1.17.8

//! The values an expression computes, and the operations on them.
//!
//! Go's VM works on `any` and dispatches on dynamic types with reflection.
//! [`Value`] has one variant per Go shape expr produces, and each variant
//! carries the Go type it stands for, so `%T` and the messages that print a
//! type read as Go's do. The operations in this module are the functions of
//! `vm/runtime`: their results and their panic messages (returned as
//! [`Panic`]) follow Go.
//!
//! Integers are all Go `int` here: expr never produces another integer type,
//! and the typed fast paths of Go's VM are not ported (WP-3-6 notes).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::Arc;

use gorse_common::gotime::{self, Duration as GoDuration, Time};

use crate::ty::{Field, Kind, Method, Ty};

/// The message of a Go panic that expr recovers, like `invalid operation: ...`.
pub type Panic = String;

thread_local! {
    // The named types are built once per thread. Their methods return the
    // types themselves, so each type is made first and given its methods after.
    static TIME: Ty = {
        let time = Ty::named_struct("time.Time", Vec::new(), Vec::new());
        time.set_methods(time_methods(&time));
        time
    };
    static DURATION: Ty = {
        let duration = Ty::named_basic("time.Duration", Kind::Int64, Vec::new());
        duration.set_methods(duration_methods(&duration));
        duration
    };
    static MONTH: Ty = {
        let month = Ty::named_basic("time.Month", Kind::Int, Vec::new());
        month.set_methods(vec![method("String", vec![month.clone()], vec![Ty::string()])]);
        month
    };
    static WEEKDAY: Ty = {
        let weekday = Ty::named_basic("time.Weekday", Kind::Int, Vec::new());
        weekday.set_methods(vec![method("String", vec![weekday.clone()], vec![Ty::string()])]);
        weekday
    };
}

/// The type `time.Time`, with the methods of Go's `time.Time` that expr
/// exposes. Those that return several values are not among them.
pub fn time_type() -> Ty {
    TIME.with(Ty::clone)
}

/// The type `time.Duration`.
pub fn duration_type() -> Ty {
    DURATION.with(Ty::clone)
}

/// The type `time.Month`: an `int` with a `String` method.
pub fn month_type() -> Ty {
    MONTH.with(Ty::clone)
}

/// The type `time.Weekday`: an `int` with a `String` method.
pub fn weekday_type() -> Ty {
    WEEKDAY.with(Ty::clone)
}

/// The type `*time.Location`. Its element has no methods that expr exposes.
pub fn location_type() -> Ty {
    Ty::pointer(Ty::named_struct("time.Location", Vec::new(), Vec::new()))
}

fn method(name: &str, ins: Vec<Ty>, outs: Vec<Ty>) -> Method {
    Method {
        name: name.to_owned(),
        ty: Ty::func(ins, outs, false),
    }
}

/// The methods of `time.Time`, each with its receiver as the first input.
fn time_methods(t: &Ty) -> Vec<Method> {
    let (d, loc, month, weekday) = (
        duration_type(),
        location_type(),
        month_type(),
        weekday_type(),
    );
    let (int, int64, string, boolean) = (Ty::int(), Ty::int64(), Ty::string(), Ty::bool());
    vec![
        method("Add", vec![t.clone(), d.clone()], vec![t.clone()]),
        method(
            "AddDate",
            vec![t.clone(), int.clone(), int.clone(), int.clone()],
            vec![t.clone()],
        ),
        method("After", vec![t.clone(), t.clone()], vec![boolean.clone()]),
        method("Before", vec![t.clone(), t.clone()], vec![boolean.clone()]),
        method("Compare", vec![t.clone(), t.clone()], vec![int.clone()]),
        method("Day", vec![t.clone()], vec![int.clone()]),
        method("Equal", vec![t.clone(), t.clone()], vec![boolean.clone()]),
        method(
            "Format",
            vec![t.clone(), string.clone()],
            vec![string.clone()],
        ),
        method("Hour", vec![t.clone()], vec![int.clone()]),
        method("In", vec![t.clone(), loc.clone()], vec![t.clone()]),
        method("IsDST", vec![t.clone()], vec![boolean.clone()]),
        method("IsZero", vec![t.clone()], vec![boolean.clone()]),
        method("Local", vec![t.clone()], vec![t.clone()]),
        method("Location", vec![t.clone()], vec![loc]),
        method("Minute", vec![t.clone()], vec![int.clone()]),
        method("Month", vec![t.clone()], vec![month]),
        method("Nanosecond", vec![t.clone()], vec![int.clone()]),
        method("Round", vec![t.clone(), d.clone()], vec![t.clone()]),
        method("Second", vec![t.clone()], vec![int.clone()]),
        method("String", vec![t.clone()], vec![string]),
        method("Sub", vec![t.clone(), t.clone()], vec![d.clone()]),
        method("Truncate", vec![t.clone(), d], vec![t.clone()]),
        method("UTC", vec![t.clone()], vec![t.clone()]),
        method("Unix", vec![t.clone()], vec![int64.clone()]),
        method("UnixMicro", vec![t.clone()], vec![int64.clone()]),
        method("UnixMilli", vec![t.clone()], vec![int64.clone()]),
        method("UnixNano", vec![t.clone()], vec![int64]),
        method("Weekday", vec![t.clone()], vec![weekday]),
        method("Year", vec![t.clone()], vec![int.clone()]),
        method("YearDay", vec![t.clone()], vec![int]),
    ]
}

/// The methods of `time.Duration`, each with its receiver as the first input.
fn duration_methods(d: &Ty) -> Vec<Method> {
    let (int64, float, string) = (Ty::int64(), Ty::float64(), Ty::string());
    vec![
        method("Abs", vec![d.clone()], vec![d.clone()]),
        method("Hours", vec![d.clone()], vec![float.clone()]),
        method("Microseconds", vec![d.clone()], vec![int64.clone()]),
        method("Milliseconds", vec![d.clone()], vec![int64.clone()]),
        method("Minutes", vec![d.clone()], vec![float.clone()]),
        method("Nanoseconds", vec![d.clone()], vec![int64]),
        method("Round", vec![d.clone(), d.clone()], vec![d.clone()]),
        method("Seconds", vec![d.clone()], vec![float]),
        method("String", vec![d.clone()], vec![string]),
        method("Truncate", vec![d.clone(), d.clone()], vec![d.clone()]),
    ]
}

/// A value an expression computes, with the Go type it has.
#[derive(Debug, Clone)]
pub enum Value {
    /// `nil`, an untyped nil interface.
    Nil,
    /// A `bool`.
    Bool(bool),
    /// An `int`.
    Int(i64),
    /// An integer of a Go type other than `int`: `int64`, `time.Month` or
    /// `time.Weekday`.
    Integer(Ty, i64),
    /// A `float64`.
    Float(f64),
    /// A `string`: bytes, which need not be UTF-8.
    Str(Arc<[u8]>),
    /// A `[]uint8`.
    Bytes(Arc<[u8]>),
    /// A `time.Time`.
    Time(Time),
    /// A `time.Duration`.
    Duration(GoDuration),
    /// A slice, with its Go type such as `[]interface {}`.
    Array(Ty, Arc<Vec<Value>>),
    /// A map, with its Go type such as `map[string]interface {}`.
    Map(Ty, Arc<Vec<(Value, Value)>>),
    /// A struct, with its Go type and its field values in declaration order.
    Struct(Ty, Arc<Vec<Value>>),
    /// A method of a receiver, as `OpMethod` leaves it on the stack.
    Method(Arc<Value>, &'static str),
    /// A `*time.Location`.
    Location(gotime::Location),
}

impl Value {
    /// A string from text.
    pub fn str(text: &str) -> Value {
        Value::Str(Arc::from(text.as_bytes()))
    }

    /// An `int64`.
    pub fn int64(n: i64) -> Value {
        Value::Integer(Ty::int64(), n)
    }

    /// A `[]interface {}` of `items`.
    pub fn any_array(items: Vec<Value>) -> Value {
        Value::Array(Ty::slice(Ty::any()), Arc::new(items))
    }

    /// A `map[string]interface {}` of `entries`.
    pub fn string_map(entries: Vec<(Value, Value)>) -> Value {
        Value::Map(Ty::map(Ty::string(), Ty::any()), Arc::new(entries))
    }

    /// The Go type of the value, or `None` for `nil`.
    pub fn ty(&self) -> Option<Ty> {
        Some(match self {
            Value::Nil => return None,
            Value::Bool(_) => Ty::bool(),
            Value::Int(_) => Ty::int(),
            Value::Integer(ty, _) => ty.clone(),
            Value::Float(_) => Ty::float64(),
            Value::Str(_) => Ty::string(),
            Value::Bytes(_) => Ty::byte_slice(),
            Value::Time(_) => time_type(),
            Value::Duration(_) => duration_type(),
            Value::Array(ty, _) | Value::Map(ty, _) | Value::Struct(ty, _) => ty.clone(),
            Value::Method(_, _) => Ty::func(Vec::new(), vec![Ty::any()], false),
            Value::Location(_) => location_type(),
        })
    }

    /// The name `%T` prints for the value: `<nil>` for nil.
    pub fn type_name(&self) -> String {
        match self.ty() {
            Some(ty) => ty.name().to_owned(),
            None => "<nil>".to_owned(),
        }
    }

    /// Whether the value is `nil`.
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// The text of a string, when the value is one.
    pub fn as_str(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The Go `%v` text of the value, as the panic messages print it.
    pub fn go_v(&self) -> String {
        let mut out = String::new();
        write_v(&mut out, self);
        out
    }
}

/// Go's `%v` of a `float64`: the shortest digits, in exponent form when the
/// exponent is below -4 or at least 6 (or the digit count, when that is more).
pub fn format_float_v(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", f.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("exponent form");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let nd = digits.len() as i32;
    let dp = exponent.parse::<i32>().expect("exponent") + 1;
    let mut eprec = 6;
    if eprec > nd && nd >= dp {
        eprec = nd;
    }
    let exp = dp - 1;
    if exp < -4 || exp >= eprec {
        let mut out = format!("{sign}{}", &digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
        return out;
    }
    let body = if dp <= 0 {
        format!("0.{}{digits}", "0".repeat((-dp) as usize))
    } else if dp >= nd {
        format!("{digits}{}", "0".repeat((dp - nd) as usize))
    } else {
        format!("{}.{}", &digits[..dp as usize], &digits[dp as usize..])
    };
    format!("{sign}{body}")
}

fn write_v(out: &mut String, value: &Value) {
    match value {
        Value::Nil => out.push_str("<nil>"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Integer(ty, i) => out.push_str(&integer_text(ty, *i)),
        Value::Float(f) => out.push_str(&format_float_v(*f)),
        Value::Str(s) => out.push_str(&String::from_utf8_lossy(s)),
        Value::Bytes(b) => {
            out.push('[');
            for (i, byte) in b.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "{byte}");
            }
            out.push(']');
        }
        Value::Time(t) => out.push_str(&t.to_string()),
        Value::Duration(d) => out.push_str(&d.to_string()),
        Value::Array(_, items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_v(out, item);
            }
            out.push(']');
        }
        Value::Map(_, entries) => {
            out.push_str("map[");
            // Go's fmt prints the entries in the order of their keys.
            let mut sorted: Vec<&(Value, Value)> = entries.iter().collect();
            sorted.sort_by(|a, b| map_key_order(&a.0, &b.0));
            for (i, (key, value)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_v(out, key);
                out.push(':');
                write_v(out, value);
            }
            out.push(']');
        }
        Value::Struct(_, fields) => {
            out.push('{');
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_v(out, field);
            }
            out.push('}');
        }
        Value::Method(_, name) => {
            let _ = write!(out, "{name}");
        }
        Value::Location(loc) => {
            let _ = write!(out, "{loc}");
        }
    }
}

/// The text `%v` prints for an integer of a named type: a month or a weekday
/// by its name, as their `String` methods do, and the number otherwise.
fn integer_text(ty: &Ty, n: i64) -> String {
    if *ty == month_type() {
        gotime::Month(n).to_string()
    } else if *ty == weekday_type() {
        gotime::Weekday(n).to_string()
    } else {
        n.to_string()
    }
}

/// The order `fmt` prints map keys in: numbers by value, strings by bytes, and
/// other keys by their text.
fn map_key_order(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => a.go_v().cmp(&b.go_v()),
    }
}

/// Go's conversion of a float to an int, as amd64 does it: a NaN or a value
/// out of range becomes the minimum int.
pub fn float_to_int(f: f64) -> i64 {
    if f.is_nan() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&f) {
        return i64::MIN;
    }
    f as i64
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Integer(ta, a), Value::Integer(tb, b)) => ta == tb && a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) | (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Time(a), Value::Time(b)) => a.equal(b),
            (Value::Duration(a), Value::Duration(b)) => a == b,
            (Value::Array(ta, a), Value::Array(tb, b)) => ta == tb && a == b,
            (Value::Map(ta, a), Value::Map(tb, b)) => ta == tb && a == b,
            (Value::Struct(ta, a), Value::Struct(tb, b)) => ta == tb && a == b,
            (Value::Method(a, x), Value::Method(b, y)) => a == b && x == y,
            (Value::Location(a), Value::Location(b)) => a.deep_equal(b),
            _ => false,
        }
    }
}

/// Go's `==` on two values as `vm/runtime.Equal` decides it.
pub fn equal(a: &Value, b: &Value) -> bool {
    match (&*as_int_value(a), &*as_int_value(b)) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(x), Value::Float(y)) => (*x as f64) == *y,
        (Value::Float(x), Value::Int(y)) => *x == (*y as f64),
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Time(x), Value::Time(y)) => x.equal(y),
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Array(_, x), Value::Array(_, y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| equal(p, q))
        }
        _ => {
            if a.is_nil() && b.is_nil() {
                return true;
            }
            deep_equal(a, b)
        }
    }
}

/// Go's `reflect.DeepEqual` on the values of this port.
fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Map(ta, x), Value::Map(tb, y)) => {
            ta == tb
                && x.len() == y.len()
                && x.iter().all(|(k, v)| {
                    y.iter()
                        .find(|(k2, _)| key_identical(k, k2))
                        .is_some_and(|(_, v2)| deep_equal(v, v2))
                })
        }
        (Value::Struct(ta, x), Value::Struct(tb, y)) => {
            ta == tb && x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| deep_equal(p, q))
        }
        (Value::Array(ta, x), Value::Array(tb, y)) => {
            ta == tb && x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| deep_equal(p, q))
        }
        (Value::Bytes(x), Value::Bytes(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Time(x), Value::Time(y)) => x.equal(y),
        (Value::Integer(tx, x), Value::Integer(ty, y)) => tx == ty && x == y,
        _ => false,
    }
}

/// Go's `==` on two interface keys of a map: same dynamic type and equal value.
pub fn key_identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Time(x), Value::Time(y)) => x == y,
        (Value::Integer(tx, x), Value::Integer(ty, y)) => tx == ty && x == y,
        _ => false,
    }
}

/// The ordering of two values for `<` and `>`, when Go's `Less` has one for
/// them; `None` when it panics with `invalid operation`.
fn order(a: &Value, b: &Value, op: &str) -> Result<Ordering, Panic> {
    let ordering = match (&*as_int_value(a), &*as_int_value(b)) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Time(x), Value::Time(y)) => {
            if x.before(y) {
                Some(Ordering::Less)
            } else if x.after(y) {
                Some(Ordering::Greater)
            } else {
                Some(Ordering::Equal)
            }
        }
        (Value::Duration(x), Value::Duration(y)) => Some(x.0.cmp(&y.0)),
        _ => None,
    };
    match ordering {
        Some(ordering) => Ok(ordering),
        // A comparison with NaN is false for every operator.
        None if is_numeric(a) && is_numeric(b) => Ok(Ordering::Equal),
        None => Err(format!(
            "invalid operation: {} {op} {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn is_numeric(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Integer(_, _) | Value::Float(_))
}

/// Go's `Less`: `a < b`.
pub fn less(a: &Value, b: &Value) -> Result<bool, Panic> {
    numeric_nan_false(a, b, "<", |o| o == Ordering::Less)
}

/// Go's `More`: `a > b`.
pub fn more(a: &Value, b: &Value) -> Result<bool, Panic> {
    numeric_nan_false(a, b, ">", |o| o == Ordering::Greater)
}

/// Go's `LessOrEqual`: `a <= b`.
pub fn less_or_equal(a: &Value, b: &Value) -> Result<bool, Panic> {
    numeric_nan_false(a, b, "<=", |o| o != Ordering::Greater)
}

/// Go's `MoreOrEqual`: `a >= b`.
pub fn more_or_equal(a: &Value, b: &Value) -> Result<bool, Panic> {
    numeric_nan_false(a, b, ">=", |o| o != Ordering::Less)
}

fn numeric_nan_false(
    a: &Value,
    b: &Value,
    op: &str,
    holds: impl Fn(Ordering) -> bool,
) -> Result<bool, Panic> {
    if is_numeric(a) && is_numeric(b) {
        let nan = match (a, b) {
            (Value::Float(x), _) if x.is_nan() => true,
            (_, Value::Float(y)) if y.is_nan() => true,
            _ => false,
        };
        if nan {
            return Ok(false);
        }
    }
    let ordering = order(a, b, op)?;
    Ok(holds(ordering))
}

/// The value of an operand as Go's runtime reads an integer: an integer of any
/// type is an `int` here, since Go converts the integer types to `int` for
/// arithmetic and comparisons.
fn as_int_value(v: &Value) -> Cow<'_, Value> {
    match v {
        Value::Integer(_, n) => Cow::Owned(Value::Int(*n)),
        _ => Cow::Borrowed(v),
    }
}

/// The numeric kind two operands promote to: a float when either is one.
fn promote_float(a: &Value, b: &Value) -> Option<(f64, f64)> {
    match (&*as_int_value(a), &*as_int_value(b)) {
        (Value::Int(x), Value::Float(y)) => Some((*x as f64, *y)),
        (Value::Float(x), Value::Int(y)) => Some((*x, *y as f64)),
        (Value::Float(x), Value::Float(y)) => Some((*x, *y)),
        _ => None,
    }
}

fn operator_panic(a: &Value, op: &str, b: &Value) -> Panic {
    format!(
        "invalid operation: {} {op} {}",
        a.type_name(),
        b.type_name()
    )
}

/// Go's `Add`.
pub fn add(a: &Value, b: &Value) -> Result<Value, Panic> {
    let (na, nb) = (as_int_value(a), as_int_value(b));
    if let (Value::Int(x), Value::Int(y)) = (&*na, &*nb) {
        return Ok(Value::Int(x.wrapping_add(*y)));
    }
    if let Some((x, y)) = promote_float(&na, &nb) {
        return Ok(Value::Float(x + y));
    }
    match (&*na, &*nb) {
        (Value::Str(x), Value::Str(y)) => {
            let mut joined = Vec::with_capacity(x.len() + y.len());
            joined.extend_from_slice(x);
            joined.extend_from_slice(y);
            Ok(Value::Str(Arc::from(joined)))
        }
        (Value::Time(x), Value::Duration(y)) => Ok(Value::Time(x.add(*y))),
        (Value::Duration(x), Value::Time(y)) => Ok(Value::Time(y.add(*x))),
        (Value::Duration(x), Value::Duration(y)) => {
            Ok(Value::Duration(GoDuration(x.0.wrapping_add(y.0))))
        }
        _ => Err(operator_panic(a, "+", b)),
    }
}

/// Go's `Subtract`.
pub fn subtract(a: &Value, b: &Value) -> Result<Value, Panic> {
    let (na, nb) = (as_int_value(a), as_int_value(b));
    if let (Value::Int(x), Value::Int(y)) = (&*na, &*nb) {
        return Ok(Value::Int(x.wrapping_sub(*y)));
    }
    if let Some((x, y)) = promote_float(&na, &nb) {
        return Ok(Value::Float(x - y));
    }
    match (&*na, &*nb) {
        (Value::Time(x), Value::Time(y)) => Ok(Value::Duration(x.sub(y))),
        (Value::Time(x), Value::Duration(y)) => Ok(Value::Time(x.add(GoDuration(-y.0)))),
        (Value::Duration(x), Value::Duration(y)) => {
            Ok(Value::Duration(GoDuration(x.0.wrapping_sub(y.0))))
        }
        _ => Err(operator_panic(a, "-", b)),
    }
}

/// Go's `Multiply`.
pub fn multiply(a: &Value, b: &Value) -> Result<Value, Panic> {
    let (na, nb) = (as_int_value(a), as_int_value(b));
    if let (Value::Int(x), Value::Int(y)) = (&*na, &*nb) {
        return Ok(Value::Int(x.wrapping_mul(*y)));
    }
    if let Some((x, y)) = promote_float(&na, &nb) {
        return Ok(Value::Float(x * y));
    }
    match (&*na, &*nb) {
        (Value::Int(x), Value::Duration(y)) | (Value::Duration(y), Value::Int(x)) => {
            Ok(Value::Duration(GoDuration(x.wrapping_mul(y.0))))
        }
        (Value::Duration(x), Value::Duration(y)) => {
            Ok(Value::Duration(GoDuration(x.0.wrapping_mul(y.0))))
        }
        _ => Err(operator_panic(a, "*", b)),
    }
}

/// Go's `Divide`: always a `float64`.
pub fn divide(a: &Value, b: &Value) -> Result<Value, Panic> {
    let (na, nb) = (as_int_value(a), as_int_value(b));
    match promote_float(&na, &nb) {
        Some((x, y)) => Ok(Value::Float(x / y)),
        None => match (&*na, &*nb) {
            (Value::Int(x), Value::Int(y)) => Ok(Value::Float(*x as f64 / *y as f64)),
            _ => Err(operator_panic(a, "/", b)),
        },
    }
}

/// Go's `Modulo`: `int % int`, with Go's panic on a zero divisor.
pub fn modulo(a: &Value, b: &Value) -> Result<Value, Panic> {
    match (&*as_int_value(a), &*as_int_value(b)) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                return Err("runtime error: integer divide by zero".to_owned());
            }
            Ok(Value::Int(x.wrapping_rem(*y)))
        }
        _ => Err(operator_panic(a, "%", b)),
    }
}

/// Go's `Exponent`: `math.Pow` of the two operands as floats.
pub fn exponent(a: &Value, b: &Value) -> Result<Value, Panic> {
    Ok(Value::Float(to_float64(a)?.powf(to_float64(b)?)))
}

/// Go's `Negate`.
pub fn negate(a: &Value) -> Result<Value, Panic> {
    match a {
        Value::Int(x) => Ok(Value::Int(x.wrapping_neg())),
        Value::Integer(ty, x) => Ok(Value::Integer(ty.clone(), x.wrapping_neg())),
        Value::Float(x) => Ok(Value::Float(-x)),
        _ => Err(format!("invalid operation: - {}", a.type_name())),
    }
}

/// Go's `ToInt`: an integer from a number, truncating floats.
pub fn to_int(a: &Value) -> Result<i64, Panic> {
    match a {
        Value::Int(x) | Value::Integer(_, x) => Ok(*x),
        Value::Float(x) => Ok(float_to_int(*x)),
        _ => Err(format!("invalid operation: int({})", a.type_name())),
    }
}

/// Go's `ToFloat64`.
pub fn to_float64(a: &Value) -> Result<f64, Panic> {
    match a {
        Value::Int(x) | Value::Integer(_, x) => Ok(*x as f64),
        Value::Float(x) => Ok(*x),
        _ => Err(format!("invalid operation: float({})", a.type_name())),
    }
}

/// Go's `ToBool`: `false` for nil.
pub fn to_bool(a: &Value) -> Result<bool, Panic> {
    match a {
        Value::Nil => Ok(false),
        Value::Bool(b) => Ok(*b),
        _ => Err(format!("invalid operation: bool({})", a.type_name())),
    }
}

/// Go's `MakeRange`: the integers from `min` to `max`, both included.
pub fn make_range(min: i64, max: i64) -> Value {
    let size = max - min + 1;
    if size <= 0 {
        return Value::Array(Ty::slice(Ty::int()), Arc::new(Vec::new()));
    }
    let items = (0..size).map(|i| Value::Int(min + i)).collect();
    Value::Array(Ty::slice(Ty::int()), Arc::new(items))
}

/// Go's `IsNil`.
pub fn is_nil(v: &Value) -> bool {
    v.is_nil()
}

/// Go's `Len`.
pub fn len(a: &Value) -> Result<i64, Panic> {
    match a {
        Value::Array(_, items) => Ok(items.len() as i64),
        Value::Map(_, entries) => Ok(entries.len() as i64),
        Value::Str(s) | Value::Bytes(s) => Ok(s.len() as i64),
        _ => Err(format!("invalid argument for len (type {})", a.type_name())),
    }
}

/// Go's `In`: whether `needle` is an element, a key or a field name of `array`.
pub fn in_(needle: &Value, array: &Value) -> Result<bool, Panic> {
    match array {
        Value::Nil => Ok(false),
        Value::Array(_, items) => Ok(items.iter().any(|item| equal(item, needle))),
        Value::Map(_, entries) => Ok(entries.iter().any(|(k, _)| key_identical(k, needle))),
        Value::Struct(ty, _) => {
            let Value::Str(name) = needle else {
                return Err(format!(
                    "cannot use {} as field name of {}",
                    needle.type_name(),
                    ty.name()
                ));
            };
            let name = String::from_utf8_lossy(name);
            Ok(ty.fields().iter().any(|f: &Field| f.name == name))
        }
        _ => Err(format!(
            "operator \"in\" not defined on {}",
            array.type_name()
        )),
    }
}

/// Go's `Slice`: `array[from:to]` on a slice, an array or a string.
pub fn slice(array: &Value, from: &Value, to: &Value) -> Result<Value, Panic> {
    let length = match array {
        Value::Array(_, items) => items.len() as i64,
        Value::Str(s) | Value::Bytes(s) => s.len() as i64,
        _ => return Err(format!("cannot slice {}", from.go_v())),
    };
    let mut a = to_int(from)?;
    let mut b = to_int(to)?;
    if a < 0 {
        a += length;
    }
    if a < 0 {
        a = 0;
    }
    if b < 0 {
        b += length;
    }
    if b < 0 {
        b = 0;
    }
    if b > length {
        b = length;
    }
    if a > b {
        a = b;
    }
    let (a, b) = (a as usize, b as usize);
    Ok(match array {
        Value::Array(ty, items) => Value::Array(ty.clone(), Arc::new(items[a..b].to_vec())),
        Value::Str(s) => Value::Str(Arc::from(&s[a..b])),
        Value::Bytes(s) => Value::Bytes(Arc::from(&s[a..b])),
        _ => unreachable!("checked above"),
    })
}

/// Go's `Fetch`: `from[i]`, a map entry, or a struct field named by `i`.
pub fn fetch(from: &Value, i: &Value) -> Result<Value, Panic> {
    if from.is_nil() {
        return Err(format!("cannot fetch {} from <nil>", i.go_v()));
    }
    if let Value::Str(s) = from {
        let index = to_int(i)?;
        let length = s.len() as i64;
        let index = if index < 0 { index + length } else { index };
        if index < 0 || index >= length {
            return Err(format!(
                "index out of range: {index} (array length is {length})"
            ));
        }
        return Ok(Value::Int(i64::from(s[index as usize])));
    }
    if let Value::Str(name) = i {
        let name = String::from_utf8_lossy(name).into_owned();
        if let Some(name) = method_name(from, &name) {
            return Ok(Value::Method(Arc::new(from.clone()), name));
        }
    }
    match from {
        Value::Array(_, items) => {
            let mut index = to_int(i)?;
            let length = items.len() as i64;
            if index < 0 {
                index += length;
            }
            if index < 0 || index >= length {
                return Err(format!(
                    "index out of range: {index} (array length is {length})"
                ));
            }
            Ok(items[index as usize].clone())
        }
        Value::Map(ty, entries) => {
            let key = if i.is_nil() { None } else { Some(i) };
            let found = entries.iter().find(|(k, _)| match key {
                Some(key) => key_identical(k, key),
                None => k.is_nil(),
            });
            match found {
                Some((_, value)) => Ok(value.clone()),
                None => Ok(zero_value(ty.elem().expect("map element"))),
            }
        }
        Value::Struct(ty, _) => {
            let Value::Str(name) = i else {
                return Err(format!(
                    "interface conversion: interface {{}} is {}, not string",
                    i.type_name()
                ));
            };
            let name = String::from_utf8_lossy(name).into_owned();
            match ty.field_index(&name) {
                Some(path) => field_by_path(from, &path),
                None => Err(format!("cannot fetch {name} from {}", from.type_name())),
            }
        }
        _ => Err(format!(
            "cannot fetch {} from {}",
            i.go_v(),
            from.type_name()
        )),
    }
}

/// Go's `FetchField`: the field at an index path of a struct.
pub fn fetch_field(from: &Value, path: &[usize], names: &[String]) -> Result<Value, Panic> {
    if !matches!(from, Value::Struct(_, _)) {
        return Err(format!("cannot get {} from {}", names[0], from.type_name()));
    }
    field_by_path(from, path)
}

/// The field at an index path, through embedded structs.
fn field_by_path(from: &Value, path: &[usize]) -> Result<Value, Panic> {
    let mut value = from.clone();
    for &i in path {
        value = match &value {
            Value::Struct(_, fields) => fields.get(i).cloned(),
            _ => None,
        }
        .ok_or_else(|| format!("cannot get field from {}", from.type_name()))?;
    }
    Ok(value)
}

/// Go's `FetchMethod`: the method at `name` of a value.
pub fn fetch_method(from: &Value, name: &str) -> Result<Value, Panic> {
    if from.is_nil() {
        return Err(format!("cannot fetch {name} from <nil>"));
    }
    match method_name(from, name) {
        Some(name) => Ok(Value::Method(Arc::new(from.clone()), name)),
        None => Err(format!("cannot fetch {name} from {}", from.type_name())),
    }
}

/// The names of the methods expr exposes, as static names: a method value
/// keeps one of these.
const METHOD_NAMES: &[&str] = &[
    "Abs",
    "Add",
    "AddDate",
    "After",
    "Before",
    "Compare",
    "Day",
    "Equal",
    "Format",
    "Hour",
    "Hours",
    "In",
    "IsDST",
    "IsZero",
    "Local",
    "Location",
    "Microseconds",
    "Milliseconds",
    "Minute",
    "Minutes",
    "Month",
    "Nanosecond",
    "Nanoseconds",
    "Round",
    "Second",
    "Seconds",
    "String",
    "Sub",
    "Truncate",
    "UTC",
    "Unix",
    "UnixMicro",
    "UnixMilli",
    "UnixNano",
    "Weekday",
    "Year",
    "YearDay",
];

/// The name of method `name` of a value's type, as a static name.
fn method_name(from: &Value, name: &str) -> Option<&'static str> {
    let ty = from.ty()?;
    let method = ty.method(name)?;
    METHOD_NAMES
        .iter()
        .copied()
        .find(|known| *known == method.name)
}

/// The zero value of a Go type, as `reflect.Zero` gives it.
pub fn zero_value(ty: &Ty) -> Value {
    match ty.kind() {
        Kind::Bool => Value::Bool(false),
        Kind::Int if ty.name() == "int" => Value::Int(0),
        Kind::Int | Kind::Int64 => Value::Integer(ty.clone(), 0),
        Kind::Float64 => Value::Float(0.0),
        Kind::String => Value::Str(Arc::from(&b""[..])),
        Kind::Slice => Value::Array(ty.clone(), Arc::new(Vec::new())),
        Kind::Map => Value::Map(ty.clone(), Arc::new(Vec::new())),
        Kind::Struct => Value::Struct(
            ty.clone(),
            Arc::new(ty.fields().iter().map(|f| zero_value(&f.ty)).collect()),
        ),
        _ => Value::Nil,
    }
}

/// Calls the method `name` of `receiver` with `args`, as Go's reflective call
/// does for the methods expr exposes.
pub fn call_method(receiver: &Value, name: &str, args: &[Value]) -> Result<Value, Panic> {
    // The arguments of a method, not counting its receiver.
    let expected = receiver
        .ty()
        .and_then(|ty| {
            ty.method(name)
                .and_then(|m| m.ty.signature())
                .map(|sig| sig.ins.len() - 1)
        })
        .ok_or_else(|| format!("cannot call method {name} on {}", receiver.type_name()))?;
    if args.len() != expected {
        return Err(format!(
            "invalid number of arguments: expected {expected}, got {}",
            args.len()
        ));
    }
    match receiver {
        Value::Time(t) => time_method(t, name, args),
        Value::Duration(d) => duration_method(*d, name, args),
        Value::Integer(ty, n) if name == "String" => Ok(Value::str(&integer_text(ty, *n))),
        _ => Err(format!(
            "cannot call method {name} on {}",
            receiver.type_name()
        )),
    }
}

/// The methods of `time.Time`, called on `t`.
fn time_method(t: &Time, name: &str, args: &[Value]) -> Result<Value, Panic> {
    Ok(match name {
        "Add" => Value::Time(t.add(arg_duration(&args[0])?)),
        "AddDate" => {
            Value::Time(t.add_date(arg_int(&args[0])?, arg_int(&args[1])?, arg_int(&args[2])?))
        }
        "After" => Value::Bool(t.after(arg_time(&args[0])?)),
        "Before" => Value::Bool(t.before(arg_time(&args[0])?)),
        "Compare" => Value::Int(t.compare(arg_time(&args[0])?)),
        "Day" => Value::Int(t.day()),
        "Equal" => Value::Bool(t.equal(arg_time(&args[0])?)),
        "Format" => Value::str(&t.format(&arg_text(&args[0])?)),
        "Hour" => Value::Int(t.hour()),
        "In" => Value::Time(t.in_location(arg_location(&args[0])?)),
        "IsDST" => Value::Bool(t.is_dst()),
        "IsZero" => Value::Bool(t.is_zero()),
        "Local" => Value::Time(t.local()),
        "Location" => Value::Location(t.location()),
        "Minute" => Value::Int(t.minute()),
        "Month" => Value::Integer(month_type(), t.month().0),
        "Nanosecond" => Value::Int(t.nanosecond()),
        "Round" => Value::Time(t.round(arg_duration(&args[0])?)),
        "Second" => Value::Int(t.second()),
        "String" => Value::str(&t.to_string()),
        "Sub" => Value::Duration(t.sub(arg_time(&args[0])?)),
        "Truncate" => Value::Time(t.truncate(arg_duration(&args[0])?)),
        "UTC" => Value::Time(t.utc()),
        "Unix" => Value::int64(t.unix()),
        "UnixMicro" => Value::int64(t.unix_micro()),
        "UnixMilli" => Value::int64(t.unix_milli()),
        "UnixNano" => Value::int64(t.unix_nano()),
        "Weekday" => Value::Integer(weekday_type(), t.weekday().0),
        "Year" => Value::Int(t.year()),
        "YearDay" => Value::Int(t.year_day()),
        _ => return Err(format!("cannot call method {name} on time.Time")),
    })
}

/// The methods of `time.Duration`, called on `d`.
fn duration_method(d: GoDuration, name: &str, args: &[Value]) -> Result<Value, Panic> {
    Ok(match name {
        "Abs" => Value::Duration(d.abs()),
        "Hours" => Value::Float(d.hours()),
        "Microseconds" => Value::int64(d.microseconds()),
        "Milliseconds" => Value::int64(d.milliseconds()),
        "Minutes" => Value::Float(d.minutes()),
        "Nanoseconds" => Value::int64(d.nanoseconds()),
        "Round" => Value::Duration(d.round(arg_duration(&args[0])?)),
        "Seconds" => Value::Float(d.seconds()),
        "String" => Value::str(&d.to_string()),
        "Truncate" => Value::Duration(d.truncate(arg_duration(&args[0])?)),
        _ => return Err(format!("cannot call method {name} on time.Duration")),
    })
}

/// The panic `reflect` raises when an argument has another type.
fn not_assignable(value: &Value, want: &str) -> Panic {
    format!(
        "reflect.Value.Call: value of type {} is not assignable to type {want}",
        value.type_name()
    )
}

fn arg_int(value: &Value) -> Result<i64, Panic> {
    match value {
        Value::Int(n) => Ok(*n),
        other => Err(not_assignable(other, "int")),
    }
}

fn arg_text(value: &Value) -> Result<String, Panic> {
    match value {
        Value::Str(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        other => Err(not_assignable(other, "string")),
    }
}

fn arg_time(value: &Value) -> Result<&Time, Panic> {
    match value {
        Value::Time(t) => Ok(t),
        other => Err(not_assignable(other, "time.Time")),
    }
}

fn arg_duration(value: &Value) -> Result<GoDuration, Panic> {
    match value {
        Value::Duration(d) => Ok(*d),
        other => Err(not_assignable(other, "time.Duration")),
    }
}

fn arg_location(value: &Value) -> Result<&gotime::Location, Panic> {
    match value {
        Value::Location(loc) => Ok(loc),
        other => Err(not_assignable(other, "*time.Location")),
    }
}

/// `time.Now()` as an expression sees it.
pub fn now() -> Value {
    Value::Time(gotime::now())
}
