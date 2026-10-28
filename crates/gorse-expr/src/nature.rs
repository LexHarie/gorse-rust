// Ports: github.com/expr-lang/expr/checker/nature/{nature,utils}.go @ v1.17.8

//! What the checker knows about a value: its type when it is known, whether
//! it is `nil`, and what is needed to call or index it.
//!
//! Go's `Nature` wraps a `reflect.Type` and caches reflection results in a
//! shared `Cache`. Here a [`Ty`] already answers those questions directly, so
//! the cache is not needed; the fields below are the ones the checker and the
//! compiler read.

use std::collections::HashMap;
use std::rc::Rc;

use crate::builtin::Function;
use crate::ty::{Kind, Ty};

/// The element and field information of an environment map: the `Fields`
/// and `DefaultMapValue` of Go's `TypeData`.
#[derive(Debug, Clone, Default)]
pub struct MapFields {
    /// The natures of the named entries, like `item` in `map[string]any{"item": ...}`.
    pub fields: HashMap<String, Nature>,
}

/// What the checker knows about a value. An unknown value has no type.
#[derive(Debug, Clone, Default)]
pub struct Nature {
    /// The type, or `None` when the value is unknown.
    pub ty: Option<Ty>,
    /// Whether the value is the literal `nil`.
    pub nil: bool,
    /// Whether the value is a method taken from a receiver.
    pub method: bool,
    /// The index of the method in its type, when `method` is set.
    pub method_index: usize,
    /// Whether the type is one of Go's integer types.
    pub is_integer: bool,
    /// Whether the type is `float32` or `float64`.
    pub is_float: bool,
    /// Go's `Ref`: for a predicate, the nature of its output; for an array,
    /// the nature of its elements when that differs from the type.
    pub reference: Option<Box<Nature>>,
    /// The fields of an environment map, set on the environment only.
    pub map: Option<Rc<MapFields>>,
    /// Whether an unknown name is an error (the environment is strict).
    pub strict: bool,
    /// The builtin function this nature stands for, when an identifier names
    /// one.
    pub func: Option<&'static Function>,
}

/// Go's `NatureCheck`: the checks `IsAnyOf` and `MaybeCompatible` make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// `bool`.
    Bool,
    /// `string`.
    String,
    /// An integer type.
    Integer,
    /// A number: an integer or a float.
    Number,
    /// A map.
    Map,
    /// An array or a slice.
    Array,
    /// `time.Time`.
    Time,
    /// `time.Duration`.
    Duration,
}

/// The natures of the values of a Go type: `FromType` of Go's cache.
pub fn from_type(ty: &Ty) -> Nature {
    // Go's IsInteger and IsFloat hold for the builtin number types only, so a
    // named one such as time.Duration is not a number.
    let builtin = !ty.is_named();
    Nature {
        ty: Some(ty.clone()),
        is_integer: builtin && ty.kind().is_integer(),
        is_float: builtin && matches!(ty.kind(), Kind::Float32 | Kind::Float64),
        ..Nature::default()
    }
}

/// The nature of `nil`, which has no type.
pub fn nil() -> Nature {
    Nature {
        nil: true,
        ..Nature::default()
    }
}

/// `ArrayFromType`: a slice of `ty`, remembering `ty` as its element.
pub fn array_from_type(ty: &Ty) -> Nature {
    let mut nt = from_type(&Ty::slice(Ty::any()));
    nt.reference = Some(Box::new(from_type(ty)));
    nt
}

fn time_type() -> Ty {
    crate::value::time_type()
}

fn duration_type() -> Ty {
    crate::value::duration_type()
}

/// The environment of a map that has the given names, each with a value of
/// unknown nature. The parser needs the names only.
pub fn map_env(names: impl IntoIterator<Item = String>) -> Nature {
    map_env_of(names.into_iter().map(|name| (name, Nature::default())))
}

/// The environment of a map whose entries have the given natures, as Go's
/// `expr.Env` gives a map of values: the checker types each name by its nature.
pub fn map_env_of(entries: impl IntoIterator<Item = (String, Nature)>) -> Nature {
    let fields = entries.into_iter().collect();
    Nature {
        ty: Some(Ty::map(Ty::string(), Ty::any())),
        map: Some(Rc::new(MapFields { fields })),
        ..Nature::default()
    }
}

impl Nature {
    /// The kind of the type, [`Kind::Invalid`] when unknown.
    pub fn kind(&self) -> Kind {
        self.ty.as_ref().map_or(Kind::Invalid, Ty::kind)
    }

    /// Whether the value is unknown: it has no type, or its type is `any`.
    pub fn is_unknown(&self) -> bool {
        self.ty.is_none() && !self.nil || self.is_any()
    }

    /// Whether the value's type is `interface {}`.
    pub fn is_any(&self) -> bool {
        self.ty
            .as_ref()
            .is_some_and(|t| t.kind() == Kind::Interface && t.methods().is_empty())
    }

    /// The type's name, or `unknown`.
    pub fn string(&self) -> String {
        match &self.ty {
            Some(ty) => ty.name().to_owned(),
            None => "unknown".to_owned(),
        }
    }

    /// Go's `Deref`. Pointers are not modelled, so this is the value itself.
    pub fn deref(&self) -> Nature {
        self.clone()
    }

    /// The key nature of a map.
    pub fn key(&self) -> Nature {
        match &self.ty {
            Some(ty) if ty.kind() == Kind::Map => from_type(ty.key().expect("map key")),
            _ => Nature::default(),
        }
    }

    /// The element nature of a map, slice or array.
    pub fn elem(&self) -> Nature {
        match &self.ty {
            Some(ty) => match ty.kind() {
                Kind::Map => from_type(ty.elem().expect("map element")),
                Kind::Slice | Kind::Array => {
                    if let Some(reference) = self.reference.as_deref() {
                        return reference.clone();
                    }
                    from_type(ty.elem().expect("array element"))
                }
                _ => Nature::default(),
            },
            None => Nature::default(),
        }
    }

    /// Go's `AssignableTo`.
    pub fn assignable_to(&self, other: &Nature) -> bool {
        if self.nil {
            match other.kind() {
                Kind::Interface | Kind::Func | Kind::Map | Kind::Slice => return true,
                _ => {}
            }
        }
        match (&self.ty, &other.ty) {
            (Some(a), Some(b))
                if self.kind() == other.kind() || other.kind() == Kind::Interface =>
            {
                a.assignable_to(b)
            }
            _ => false,
        }
    }

    /// The method named `name`, as a nature.
    pub fn method_by_name(&self, name: &str) -> Option<Nature> {
        let method = self.ty.as_ref()?.method(name)?;
        let mut nt = from_type(&method.ty);
        nt.method = true;
        nt.method_index = self
            .ty
            .as_ref()?
            .methods()
            .iter()
            .position(|m| m.name == name)
            .expect("method is listed");
        Some(nt)
    }

    /// The number of inputs of a function type, not counting variadics.
    pub fn num_in(&self) -> usize {
        self.signature().map_or(0, |s| s.ins.len())
    }

    /// The number of outputs of a function type.
    pub fn num_out(&self) -> usize {
        self.signature().map_or(0, |s| s.outs.len())
    }

    /// Whether the function type is variadic.
    pub fn is_variadic(&self) -> bool {
        self.signature().is_some_and(|s| s.variadic)
    }

    /// The nature of input `i` of a function type.
    pub fn input(&self, i: usize) -> Nature {
        match self.signature().and_then(|s| s.ins.get(i)) {
            Some(ty) => from_type(ty),
            None => Nature::default(),
        }
    }

    /// The element nature of input `i`, for variadic inputs.
    pub fn input_elem(&self, i: usize) -> Nature {
        self.input(i).elem()
    }

    /// The nature of output `i` of a function type.
    pub fn output(&self, i: usize) -> Nature {
        match self.signature().and_then(|s| s.outs.get(i)) {
            Some(ty) => from_type(ty),
            None => Nature::default(),
        }
    }

    /// Whether the first input of a function type is unknown.
    pub fn is_first_arg_unknown(&self) -> bool {
        match self.signature() {
            Some(s) => match s.ins.first() {
                Some(ty) => from_type(ty).is_unknown(),
                None => false,
            },
            None => false,
        }
    }

    fn signature(&self) -> Option<&crate::ty::Signature> {
        self.ty.as_ref().and_then(Ty::signature)
    }

    /// The nature of the field `name` of a struct.
    pub fn field_by_name(&self, name: &str) -> Option<Nature> {
        if self.kind() != Kind::Struct {
            return None;
        }
        let path = self.ty.as_ref()?.field_index(name)?;
        let ty = self.ty.as_ref()?.field_type_at(&path)?;
        Some(from_type(&ty))
    }

    /// The index path of the field `name` of a struct.
    pub fn field_index(&self, name: &str) -> Option<Vec<usize>> {
        if self.kind() != Kind::Struct {
            return None;
        }
        self.ty.as_ref()?.field_index(name)
    }

    /// Go's `Get`: the entry `name` of an environment map, or the method or
    /// field `name` of a value.
    pub fn get(&self, name: &str) -> Option<Nature> {
        if self.kind() == Kind::Map
            && let Some(map) = &self.map
        {
            return map.fields.get(name).cloned();
        }
        if let Some(nt) = self.method_by_name(name) {
            return Some(nt);
        }
        if self.kind() == Kind::Struct {
            return self.field_by_name(name);
        }
        None
    }

    /// Whether the environment is a map whose values are all `interface {}`,
    /// so the compiler can load names from it by name.
    pub fn is_fast_map(&self) -> bool {
        match &self.ty {
            Some(ty) => {
                ty.kind() == Kind::Map
                    && ty.key().is_some_and(|k| k.kind() == Kind::String)
                    && ty.elem().is_some_and(|e| e.kind() == Kind::Interface)
            }
            None => false,
        }
    }

    /// Whether the value is a number.
    pub fn is_number(&self) -> bool {
        self.is_integer || self.is_float
    }

    /// The type of `self op rhs` for an arithmetic operator.
    pub fn promote_numeric_nature(&self, rhs: &Nature) -> Nature {
        if self.is_unknown() || rhs.is_unknown() {
            return Nature::default();
        }
        if self.is_float || rhs.is_float {
            return from_type(&Ty::float64());
        }
        from_type(&Ty::int())
    }

    /// Whether the value is a `time.Time`.
    pub fn is_time(&self) -> bool {
        self.ty.as_ref() == Some(&time_type())
    }

    /// Whether the value is a `time.Duration`.
    pub fn is_duration(&self) -> bool {
        self.ty.as_ref() == Some(&duration_type())
    }

    /// Whether the value is a bool.
    pub fn is_bool(&self) -> bool {
        self.kind() == Kind::Bool
    }

    /// Whether the value is a string.
    pub fn is_string(&self) -> bool {
        self.kind() == Kind::String
    }

    /// Whether the value is a `[]byte`.
    pub fn is_byte_slice(&self) -> bool {
        self.ty.as_ref() == Some(&Ty::byte_slice())
    }

    /// Whether the value is a slice or an array.
    pub fn is_array(&self) -> bool {
        matches!(self.kind(), Kind::Slice | Kind::Array)
    }

    /// Whether the value is a map.
    pub fn is_map(&self) -> bool {
        self.kind() == Kind::Map
    }

    /// Whether the value is a struct.
    pub fn is_struct(&self) -> bool {
        self.kind() == Kind::Struct
    }

    /// Whether the value is a function.
    pub fn is_func(&self) -> bool {
        self.kind() == Kind::Func
    }

    /// Whether any of the checks holds.
    pub fn is_any_of(&self, checks: &[Check]) -> bool {
        checks.iter().any(|check| match check {
            Check::Bool => self.is_bool(),
            Check::String => self.is_string(),
            Check::Integer => self.is_integer,
            Check::Number => self.is_number(),
            Check::Map => self.is_map(),
            Check::Array => self.is_array(),
            Check::Time => self.is_time(),
            Check::Duration => self.is_duration(),
        })
    }

    /// Whether `self == rhs` can be checked.
    pub fn comparable_to(&self, rhs: &Nature) -> bool {
        self.is_unknown()
            || rhs.is_unknown()
            || self.nil
            || rhs.nil
            || self.is_number() && rhs.is_number()
            || self.is_duration() && rhs.is_duration()
            || self.is_time() && rhs.is_time()
            || self.is_array() && rhs.is_array()
            || self.assignable_to(rhs)
    }

    /// Whether `self` and `rhs` may fit one of `checks`, one of them being
    /// unknown.
    pub fn maybe_compatible(&self, rhs: &Nature, checks: &[Check]) -> bool {
        let unknown = self.is_unknown();
        let rhs_unknown = rhs.is_unknown();
        unknown && rhs_unknown
            || unknown && rhs.is_any_of(checks)
            || rhs_unknown && self.is_any_of(checks)
    }

    /// The nature of a slice of values of this nature.
    pub fn make_array_of(&self) -> Nature {
        let mut nt = from_type(&Ty::slice(Ty::any()));
        nt.reference = Some(Box::new(self.clone()));
        nt
    }
}
