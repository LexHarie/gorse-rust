// Ports: github.com/expr-lang/expr/builtin/builtin.go and builtin/function.go @ v1.17.8 (Names, Index, the Builtins table)

//! The builtin functions of expr: their names, their compile-time signatures
//! and their run-time functions. The parser needs the names; the checker, the
//! compiler and the VM need the rest.

mod lib;
mod strings;
mod validation;

use crate::ty::{Kind, Ty};
use crate::value::{self, Panic, Value};

/// A builtin's compile-time check: the argument types (None for nil) to the
/// result type, or the error text Go's checker reports.
pub type Validate = fn(&[Option<Ty>]) -> Result<Ty, String>;

/// A builtin's run-time function, from the values of its arguments.
pub type Call = fn(&[Value]) -> Result<Value, Panic>;

/// A run-time function that also says how much memory its result takes.
pub type SafeCall = fn(&[Value]) -> Result<(Value, u64), Panic>;

/// One builtin, as `builtin.Function` describes it.
#[derive(Debug)]
pub struct Function {
    /// The name the expression calls it by.
    pub name: &'static str,
    /// Whether the builtin takes a predicate, which the compiler turns into a
    /// loop rather than a call.
    pub predicate: bool,
    /// The compile-time check, for builtins that check by a function.
    pub validate: Option<Validate>,
    /// The overloads, each a function type; the first is the default.
    pub types: &'static [fn() -> Ty],
    /// The run-time function.
    pub func: Option<Call>,
    /// The run-time function, when it also reports its memory.
    pub safe: Option<SafeCall>,
}

impl Function {
    /// `Function.Type()`: the first overload, or the type of the function.
    pub fn ty(&self) -> Ty {
        match self.types.first() {
            Some(ty) => ty(),
            None => sig(
                vec![Ty::slice(Ty::any())],
                vec![Ty::any(), error_type()],
                true,
            ),
        }
    }
}

/// `func(ins...) outs` of a plain function type.
fn sig(ins: Vec<Ty>, outs: Vec<Ty>, variadic: bool) -> Ty {
    Ty::func(ins, outs, variadic)
}

/// `func(...) (..., error)`: the signatures of the builtins with no overload.
fn fn_sig(ins: Vec<Ty>, out: Ty) -> Ty {
    sig(ins, vec![out], false)
}

fn error_type() -> Ty {
    Ty::named_basic("error", Kind::Interface, Vec::new())
}

fn any_slice() -> Ty {
    Ty::slice(Ty::any())
}

fn string_slice() -> Ty {
    Ty::slice(Ty::string())
}

/// The signature of a predicate builtin that takes a collection and a
/// `func(any) bool`, and returns `out`.
fn predicate(out: Ty) -> Ty {
    fn_sig(vec![any_slice(), fn_sig(vec![Ty::any()], Ty::bool())], out)
}

const fn plain(name: &'static str, func: Call) -> Function {
    Function {
        name,
        predicate: false,
        validate: None,
        types: &[],
        func: Some(func),
        safe: None,
    }
}

const fn checked(name: &'static str, validate: Validate, func: Call) -> Function {
    Function {
        name,
        predicate: false,
        validate: Some(validate),
        types: &[],
        func: Some(func),
        safe: None,
    }
}

const fn typed(name: &'static str, types: &'static [fn() -> Ty], func: Call) -> Function {
    Function {
        name,
        predicate: false,
        validate: None,
        types,
        func: Some(func),
        safe: None,
    }
}

const fn safe(name: &'static str, types: &'static [fn() -> Ty], call: SafeCall) -> Function {
    Function {
        name,
        predicate: false,
        validate: None,
        types,
        func: None,
        safe: Some(call),
    }
}

const fn checked_safe(name: &'static str, validate: Validate, call: SafeCall) -> Function {
    Function {
        name,
        predicate: false,
        validate: Some(validate),
        types: &[],
        func: None,
        safe: Some(call),
    }
}

const fn collection(name: &'static str, types: &'static [fn() -> Ty]) -> Function {
    Function {
        name,
        predicate: true,
        validate: None,
        types,
        func: None,
        safe: None,
    }
}

/// `builtin.Builtins`: every builtin, in Go's order.
pub static BUILTINS: &[Function] = &[
    collection("all", &[|| predicate(Ty::bool())]),
    collection("none", &[|| predicate(Ty::bool())]),
    collection("any", &[|| predicate(Ty::bool())]),
    collection("one", &[|| predicate(Ty::bool())]),
    collection("filter", &[|| predicate(any_slice())]),
    collection(
        "map",
        &[|| {
            fn_sig(
                vec![any_slice(), fn_sig(vec![Ty::any()], Ty::any())],
                any_slice(),
            )
        }],
    ),
    collection("find", &[|| predicate(Ty::any())]),
    collection("findIndex", &[|| predicate(Ty::int())]),
    collection("findLast", &[|| predicate(Ty::any())]),
    collection("findLastIndex", &[|| predicate(Ty::int())]),
    collection("count", &[|| predicate(Ty::int())]),
    collection("sum", &[|| predicate(Ty::int())]),
    collection(
        "groupBy",
        &[|| {
            fn_sig(
                vec![any_slice(), fn_sig(vec![Ty::any()], Ty::any())],
                Ty::map(Ty::any(), any_slice()),
            )
        }],
    ),
    collection(
        "sortBy",
        &[|| {
            sig(
                vec![
                    any_slice(),
                    fn_sig(vec![Ty::any()], Ty::bool()),
                    Ty::string(),
                ],
                vec![any_slice()],
                false,
            )
        }],
    ),
    collection(
        "reduce",
        &[|| {
            fn_sig(
                vec![
                    any_slice(),
                    sig(vec![Ty::any(), Ty::any()], vec![Ty::any()], false),
                    Ty::any(),
                ],
                Ty::any(),
            )
        }],
    ),
    checked("len", validation::len, lib::len),
    typed(
        "type",
        &[|| fn_sig(vec![Ty::any()], Ty::string())],
        lib::type_of,
    ),
    checked("abs", validation::abs, lib::abs),
    checked("ceil", validation::ceil, lib::ceil),
    checked("floor", validation::floor, lib::floor),
    checked("round", validation::round, lib::round),
    checked("int", validation::int, lib::int),
    checked("float", validation::float, lib::float),
    typed(
        "string",
        &[|| fn_sig(vec![Ty::any()], Ty::string())],
        lib::string_of,
    ),
    typed(
        "trim",
        &[
            || fn_sig(vec![Ty::string()], Ty::string()),
            || fn_sig(vec![Ty::string(), Ty::string()], Ty::string()),
        ],
        lib::trim,
    ),
    typed(
        "trimPrefix",
        &[
            || fn_sig(vec![Ty::string(), Ty::string()], Ty::string()),
            || fn_sig(vec![Ty::string()], Ty::string()),
        ],
        lib::trim_prefix,
    ),
    typed(
        "trimSuffix",
        &[
            || fn_sig(vec![Ty::string(), Ty::string()], Ty::string()),
            || fn_sig(vec![Ty::string()], Ty::string()),
        ],
        lib::trim_suffix,
    ),
    typed(
        "upper",
        &[|| fn_sig(vec![Ty::string()], Ty::string())],
        lib::upper,
    ),
    typed(
        "lower",
        &[|| fn_sig(vec![Ty::string()], Ty::string())],
        lib::lower,
    ),
    typed(
        "split",
        &[
            || fn_sig(vec![Ty::string(), Ty::string()], string_slice()),
            || fn_sig(vec![Ty::string(), Ty::string(), Ty::int()], string_slice()),
        ],
        lib::split,
    ),
    typed(
        "splitAfter",
        &[
            || fn_sig(vec![Ty::string(), Ty::string()], string_slice()),
            || fn_sig(vec![Ty::string(), Ty::string(), Ty::int()], string_slice()),
        ],
        lib::split_after,
    ),
    typed(
        "replace",
        &[
            || {
                fn_sig(
                    vec![Ty::string(), Ty::string(), Ty::string(), Ty::int()],
                    Ty::string(),
                )
            },
            || fn_sig(vec![Ty::string(), Ty::string(), Ty::string()], Ty::string()),
        ],
        lib::replace,
    ),
    safe(
        "repeat",
        &[|| fn_sig(vec![Ty::string(), Ty::int()], Ty::string())],
        lib::repeat,
    ),
    typed(
        "join",
        &[
            || fn_sig(vec![string_slice(), Ty::string()], Ty::string()),
            || fn_sig(vec![any_slice(), Ty::string()], Ty::string()),
            || fn_sig(vec![any_slice()], Ty::string()),
            || fn_sig(vec![string_slice(), Ty::string()], Ty::string()),
            || fn_sig(vec![string_slice()], Ty::string()),
        ],
        lib::join,
    ),
    typed(
        "indexOf",
        &[|| fn_sig(vec![Ty::string(), Ty::string()], Ty::int())],
        lib::index_of,
    ),
    typed(
        "lastIndexOf",
        &[|| fn_sig(vec![Ty::string(), Ty::string()], Ty::int())],
        lib::last_index_of,
    ),
    typed(
        "hasPrefix",
        &[|| fn_sig(vec![Ty::string(), Ty::string()], Ty::bool())],
        lib::has_prefix,
    ),
    typed(
        "hasSuffix",
        &[|| fn_sig(vec![Ty::string(), Ty::string()], Ty::bool())],
        lib::has_suffix,
    ),
    checked("max", validation::max, lib::max),
    checked("min", validation::min, lib::min),
    checked("mean", validation::mean, lib::mean),
    checked("median", validation::median, lib::median),
    typed(
        "toJSON",
        &[|| fn_sig(vec![Ty::any()], Ty::string())],
        lib::to_json,
    ),
    typed(
        "fromJSON",
        &[|| fn_sig(vec![Ty::string()], Ty::any())],
        lib::from_json,
    ),
    typed(
        "toBase64",
        &[|| fn_sig(vec![Ty::string()], Ty::string())],
        lib::to_base64,
    ),
    typed(
        "fromBase64",
        &[|| fn_sig(vec![Ty::string()], Ty::string())],
        lib::from_base64,
    ),
    checked("now", validation::now, lib::now),
    typed(
        "duration",
        &[|| fn_sig(vec![Ty::string()], value::duration_type())],
        lib::duration,
    ),
    checked("date", validation::date, lib::date),
    typed(
        "timezone",
        &[|| fn_sig(vec![Ty::string()], value::location_type())],
        lib::timezone,
    ),
    checked("first", validation::first, lib::first),
    checked("last", validation::last, lib::last),
    plain("get", lib::get),
    checked("take", validation::take, lib::take),
    checked("keys", validation::keys, lib::keys),
    checked("values", validation::values, lib::values),
    checked("toPairs", validation::to_pairs, lib::to_pairs),
    checked("fromPairs", validation::from_pairs, lib::from_pairs),
    checked_safe("reverse", validation::reverse, lib::reverse),
    checked("uniq", validation::uniq, lib::uniq),
    checked_safe("concat", validation::concat, lib::concat),
    checked_safe("flatten", validation::flatten, lib::flatten),
    safe(
        "sort",
        &[
            || fn_sig(vec![any_slice(), Ty::string()], any_slice()),
            || fn_sig(vec![Ty::slice(Ty::int()), Ty::string()], any_slice()),
            || fn_sig(vec![Ty::slice(Ty::float64()), Ty::string()], any_slice()),
            || fn_sig(vec![string_slice(), Ty::string()], any_slice()),
            || fn_sig(vec![any_slice()], any_slice()),
            || fn_sig(vec![Ty::slice(Ty::float64())], any_slice()),
            || fn_sig(vec![string_slice()], any_slice()),
            || fn_sig(vec![Ty::slice(Ty::int())], any_slice()),
        ],
        lib::sort,
    ),
    typed(
        "bitand",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitand,
    ),
    typed(
        "bitor",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitor,
    ),
    typed(
        "bitxor",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitxor,
    ),
    typed(
        "bitnand",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitnand,
    ),
    typed(
        "bitshl",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitshl,
    ),
    typed(
        "bitshr",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitshr,
    ),
    typed(
        "bitushr",
        &[|| fn_sig(vec![Ty::int(), Ty::int()], Ty::int())],
        lib::bitushr,
    ),
    typed(
        "bitnot",
        &[|| fn_sig(vec![Ty::int()], Ty::int())],
        lib::bitnot,
    ),
];

/// The builtin of a name, or `None` when there is none.
pub fn lookup(name: &str) -> Option<&'static Function> {
    BUILTINS.iter().find(|f| f.name == name)
}

/// `builtin.Names`, in the order of `builtin.Builtins`.
pub const NAMES: [&str; 71] = [
    "all",
    "none",
    "any",
    "one",
    "filter",
    "map",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "count",
    "sum",
    "groupBy",
    "sortBy",
    "reduce",
    "len",
    "type",
    "abs",
    "ceil",
    "floor",
    "round",
    "int",
    "float",
    "string",
    "trim",
    "trimPrefix",
    "trimSuffix",
    "upper",
    "lower",
    "split",
    "splitAfter",
    "replace",
    "repeat",
    "join",
    "indexOf",
    "lastIndexOf",
    "hasPrefix",
    "hasSuffix",
    "max",
    "min",
    "mean",
    "median",
    "toJSON",
    "fromJSON",
    "toBase64",
    "fromBase64",
    "now",
    "duration",
    "date",
    "timezone",
    "first",
    "last",
    "get",
    "take",
    "keys",
    "values",
    "toPairs",
    "fromPairs",
    "reverse",
    "uniq",
    "concat",
    "flatten",
    "sort",
    "bitand",
    "bitor",
    "bitxor",
    "bitnand",
    "bitshl",
    "bitshr",
    "bitushr",
    "bitnot",
];

/// `builtin.Index[name]`: the position of a builtin in [`NAMES`].
pub fn index(name: &str) -> Option<usize> {
    NAMES.iter().position(|&n| n == name)
}
