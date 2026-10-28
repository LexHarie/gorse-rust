// Ports: github.com/expr-lang/expr/builtin/validation.go and the Validate functions of builtin/builtin.go @ v1.17.8

//! The compile-time signatures of the builtins that check their arguments by
//! a function rather than by overloads: the argument types each accepts and
//! the type each returns. The messages follow Go, including `%!s(<nil>)` for
//! a nil argument type, which Go prints with `%s`.

use crate::ty::{Kind, Ty};
use crate::value::time_type;

/// The types of the arguments of a call; `None` is the nil type.
pub type Args<'a> = &'a [Option<Ty>];

/// `kind(t)`: the kind of a type; a nil type has [`Kind::Invalid`].
fn kind(t: Option<&Ty>) -> Kind {
    t.map_or(Kind::Invalid, Ty::kind)
}

/// How a type prints with `%s`: a nil type prints as `%!s(<nil>)`.
pub fn fmt_type(t: Option<&Ty>) -> String {
    t.map_or_else(|| "%!s(<nil>)".to_owned(), |t| t.name().to_owned())
}

/// Whether a kind is one of the numbers Go's validators accept (`reflect.Int`
/// through `reflect.Float64`, without `uintptr`).
fn is_number(k: Kind) -> bool {
    matches!(
        k,
        Kind::Int
            | Kind::Int8
            | Kind::Int16
            | Kind::Int32
            | Kind::Int64
            | Kind::Uint
            | Kind::Uint8
            | Kind::Uint16
            | Kind::Uint32
            | Kind::Uint64
            | Kind::Float32
            | Kind::Float64
    )
}

/// Whether a type is `*time.Location`, Go's `locationType`.
pub fn is_location(t: &Option<Ty>) -> bool {
    t.as_ref().is_some_and(|t| t.name() == "*time.Location")
}

fn count_error(got: usize, expected: usize) -> String {
    format!("invalid number of arguments (expected {expected}, got {got})")
}

fn array_type() -> Ty {
    Ty::slice(Ty::any())
}

fn map_type() -> Ty {
    Ty::map(Ty::any(), Ty::any())
}

/// The type of the one argument, when it is a number or an interface; the
/// validators of `abs` and the rounding functions return it or a float.
fn one_number_or_interface(name: &str, args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        k if is_number(k) || k == Kind::Interface => {
            Ok(args[0].clone().expect("a number has a type"))
        }
        _ => Err(format!(
            "invalid argument for {name} (type {})",
            fmt_type(args[0].as_ref())
        )),
    }
}

/// Go's `validateAggregateFunc`: `max`, `min`, `mean` and `median`.
pub fn aggregate(name: &str, args: Args) -> Result<Ty, String> {
    if args.is_empty() {
        return Err(format!("not enough arguments to call {name}"));
    }
    for arg in args {
        match kind(arg.as_ref()) {
            Kind::Interface | Kind::Array | Kind::Slice => return Ok(Ty::any()),
            k if is_number(k) => {}
            _ => {
                return Err(format!(
                    "invalid argument for {name} (type {})",
                    fmt_type(arg.as_ref())
                ));
            }
        }
    }
    Ok(args[0].clone().expect("a number has a type"))
}

/// Go's `validateRoundFunc`: `ceil`, `floor` and `round` return a float.
fn rounding(name: &str, args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        k if is_number(k) || k == Kind::Interface => Ok(Ty::float64()),
        _ => Err(format!(
            "invalid argument for {name} (type {})",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn max(args: Args) -> Result<Ty, String> {
    aggregate("max", args)
}

pub fn min(args: Args) -> Result<Ty, String> {
    aggregate("min", args)
}

pub fn mean(args: Args) -> Result<Ty, String> {
    aggregate("mean", args)
}

pub fn median(args: Args) -> Result<Ty, String> {
    aggregate("median", args)
}

pub fn ceil(args: Args) -> Result<Ty, String> {
    rounding("ceil", args)
}

pub fn floor(args: Args) -> Result<Ty, String> {
    rounding("floor", args)
}

pub fn round(args: Args) -> Result<Ty, String> {
    rounding("round", args)
}

pub fn abs(args: Args) -> Result<Ty, String> {
    one_number_or_interface("abs", args)
}

pub fn len(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Array | Kind::Map | Kind::Slice | Kind::String | Kind::Interface => Ok(Ty::int()),
        _ => Err(format!(
            "invalid argument for len (type {})",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn int(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::String => Ok(Ty::int()),
        k if is_number(k) => Ok(Ty::int()),
        _ => Err(format!(
            "invalid argument for int (type {})",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn float(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::String => Ok(Ty::float64()),
        k if is_number(k) => Ok(Ty::float64()),
        _ => Err(format!(
            "invalid argument for float (type {})",
            fmt_type(args[0].as_ref())
        )),
    }
}

/// Go's `first` and `last`: the element type of a slice or array.
fn element(word: &str, args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface => Ok(Ty::any()),
        Kind::Slice | Kind::Array => Ok(args[0]
            .as_ref()
            .and_then(Ty::elem)
            .cloned()
            .unwrap_or_else(Ty::any)),
        _ => Err(format!(
            "cannot get {word} element from {}",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn first(args: Args) -> Result<Ty, String> {
    element("first", args)
}

pub fn last(args: Args) -> Result<Ty, String> {
    element("last", args)
}

pub fn take(args: Args) -> Result<Ty, String> {
    if args.len() != 2 {
        return Err(count_error(args.len(), 2));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Slice | Kind::Array => {}
        _ => return Err(format!("cannot take from {}", fmt_type(args[0].as_ref()))),
    }
    match kind(args[1].as_ref()) {
        Kind::Interface | Kind::Int | Kind::Int8 | Kind::Int16 | Kind::Int32 | Kind::Int64 => {}
        _ => {
            return Err(format!(
                "cannot take {} elements",
                fmt_type(args[1].as_ref())
            ));
        }
    }
    Ok(args[0].clone().expect("checked above"))
}

/// Go's `keys` and `values`: an `[]any` of the keys or the values of a map.
fn map_entries(what: &str, args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Map => Ok(array_type()),
        _ => Err(format!(
            "cannot get {what} from {}",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn keys(args: Args) -> Result<Ty, String> {
    map_entries("keys", args)
}

pub fn values(args: Args) -> Result<Ty, String> {
    map_entries("values", args)
}

pub fn to_pairs(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Map => Ok(array_type()),
        _ => Err(format!(
            "cannot transform {} to pairs",
            fmt_type(args[0].as_ref())
        )),
    }
}

pub fn from_pairs(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Slice | Kind::Array => Ok(map_type()),
        _ => Err(format!(
            "cannot transform {} from pairs",
            fmt_type(args[0].as_ref())
        )),
    }
}

/// Go's `reverse` and `uniq`: an `[]any` of the elements of a slice or array.
fn slice_copy(name: &str, args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Slice | Kind::Array => Ok(array_type()),
        _ => Err(format!("cannot {name} {}", fmt_type(args[0].as_ref()))),
    }
}

pub fn reverse(args: Args) -> Result<Ty, String> {
    slice_copy("reverse", args)
}

pub fn uniq(args: Args) -> Result<Ty, String> {
    slice_copy("uniq", args)
}

pub fn concat(args: Args) -> Result<Ty, String> {
    if args.is_empty() {
        return Err("invalid number of arguments (expected at least 1, got 0)".to_owned());
    }
    for arg in args {
        match kind(arg.as_ref()) {
            Kind::Interface | Kind::Slice | Kind::Array => {}
            _ => return Err(format!("cannot concat {}", fmt_type(arg.as_ref()))),
        }
    }
    Ok(array_type())
}

pub fn flatten(args: Args) -> Result<Ty, String> {
    if args.len() != 1 {
        return Err(count_error(args.len(), 1));
    }
    match kind(args[0].as_ref()) {
        Kind::Interface | Kind::Slice | Kind::Array => Ok(array_type()),
        _ => Err(format!("cannot flatten {}", fmt_type(args[0].as_ref()))),
    }
}

pub fn now(args: Args) -> Result<Ty, String> {
    if args.is_empty() {
        return Ok(time_type());
    }
    if args.len() == 1 && is_location(&args[0]) {
        return Ok(time_type());
    }
    Err(count_error(args.len(), 0))
}

pub fn date(args: Args) -> Result<Ty, String> {
    if args.is_empty() {
        return Err("invalid number of arguments (expected at least 1, got 0)".to_owned());
    }
    let rest = if is_location(&args[0]) {
        args.len() - 1
    } else {
        args.len()
    };
    if rest > 3 {
        return Err(format!(
            "invalid number of arguments (expected at most 3, got {rest})"
        ));
    }
    Ok(time_type())
}
