// Ports: the parts of Go's reflect.Type that expr-lang v1.17.8 reads (checker/
// nature, vm/runtime, compiler) @ v1.17.8

//! The Go types an expression sees.
//!
//! expr-lang asks `reflect` for the kind, the element, key and field types of
//! every value, and prints them in its errors (`mismatched types int and
//! string`). A [`Ty`] carries the same facts for the types this port knows:
//! the basic types, slices, arrays, maps, functions, and named structs and
//! defined types such as `time.Time` and `data.Item`.
//!
//! Pointers are not modelled. Gorse's expressions never see one, so Go's
//! dereference steps do nothing here and the compiler does not emit them.

use std::cell::OnceCell;
use std::fmt;
use std::rc::Rc;

/// What a type is: `reflect.Kind`, without the kinds expr never sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// No type.
    Invalid,
    /// `bool`.
    Bool,
    /// `int`.
    Int,
    /// `int8`.
    Int8,
    /// `int16`.
    Int16,
    /// `int32`.
    Int32,
    /// `int64`.
    Int64,
    /// `uint`.
    Uint,
    /// `uint8`.
    Uint8,
    /// `uint16`.
    Uint16,
    /// `uint32`.
    Uint32,
    /// `uint64`.
    Uint64,
    /// `uintptr`.
    Uintptr,
    /// `float32`.
    Float32,
    /// `float64`.
    Float64,
    /// `[N]T`.
    Array,
    /// A function.
    Func,
    /// An interface, like `interface {}`.
    Interface,
    /// `map[K]V`.
    Map,
    /// `*T`.
    Ptr,
    /// `[]T`.
    Slice,
    /// `string`.
    String,
    /// A struct.
    Struct,
}

impl Kind {
    /// The name `reflect.Kind` prints, like `float64`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Invalid => "invalid",
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Int8 => "int8",
            Kind::Int16 => "int16",
            Kind::Int32 => "int32",
            Kind::Int64 => "int64",
            Kind::Uint => "uint",
            Kind::Uint8 => "uint8",
            Kind::Uint16 => "uint16",
            Kind::Uint32 => "uint32",
            Kind::Uint64 => "uint64",
            Kind::Uintptr => "uintptr",
            Kind::Float32 => "float32",
            Kind::Float64 => "float64",
            Kind::Array => "array",
            Kind::Func => "func",
            Kind::Interface => "interface",
            Kind::Map => "map",
            Kind::Ptr => "ptr",
            Kind::Slice => "slice",
            Kind::String => "string",
            Kind::Struct => "struct",
        }
    }

    /// Whether the kind is one of Go's integer kinds, `uintptr` included.
    pub fn is_integer(self) -> bool {
        matches!(
            self,
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
                | Kind::Uintptr
        )
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A field of a struct type: `reflect.StructField`, for the fields expr sees.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The field's name, like `ItemId`.
    pub name: String,
    /// Its type.
    pub ty: Ty,
    /// Whether the field is embedded; its fields are promoted to the struct.
    pub embedded: bool,
}

/// A method of a named type, with its receiver as the first input.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    /// The method's name, like `Year`.
    pub name: String,
    /// The method's function type; its first input is the receiver.
    pub ty: Ty,
}

/// The signature of a function type.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// The input types.
    pub ins: Vec<Ty>,
    /// The output types.
    pub outs: Vec<Ty>,
    /// Whether the last input is variadic, like `...interface {}`.
    pub variadic: bool,
}

#[derive(Debug)]
struct Data {
    kind: Kind,
    // `Type.String()`: `int`, `[]interface {}`, `data.Item`, ...
    name: String,
    // Whether the type is defined by a package (Go's `PkgPath() != ""`).
    named: bool,
    elem: Option<Ty>,
    key: Option<Ty>,
    len: usize,
    fields: Vec<Field>,
    // Set once: a method may name the type it belongs to, so the list is
    // filled in after the type exists (see `Ty::set_methods`).
    methods: OnceCell<Vec<Method>>,
    func: Option<Signature>,
}

/// A Go type. Two types are the same when they print the same, which is how
/// Go decides identity for the types of this port.
#[derive(Debug, Clone)]
pub struct Ty(Rc<Data>);

impl PartialEq for Ty {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
            || (self.0.kind == other.0.kind && self.0.name == other.0.name)
    }
}

impl Eq for Ty {}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.name)
    }
}

impl Ty {
    fn new(data: Data) -> Ty {
        Ty(Rc::new(data))
    }

    fn plain(kind: Kind, name: &str) -> Ty {
        Ty::new(Data {
            kind,
            name: name.to_owned(),
            named: false,
            elem: None,
            key: None,
            len: 0,
            fields: Vec::new(),
            methods: OnceCell::new(),
            func: None,
        })
    }

    /// `bool`.
    pub fn bool() -> Ty {
        Ty::plain(Kind::Bool, "bool")
    }

    /// `int`.
    pub fn int() -> Ty {
        Ty::plain(Kind::Int, "int")
    }

    /// `int64`.
    pub fn int64() -> Ty {
        Ty::plain(Kind::Int64, "int64")
    }

    /// `float64`.
    pub fn float64() -> Ty {
        Ty::plain(Kind::Float64, "float64")
    }

    /// `string`.
    pub fn string() -> Ty {
        Ty::plain(Kind::String, "string")
    }

    /// `uint8`.
    pub fn uint8() -> Ty {
        Ty::plain(Kind::Uint8, "uint8")
    }

    /// `interface {}`, Go's `any`.
    pub fn any() -> Ty {
        Ty::plain(Kind::Interface, "interface {}")
    }

    /// The basic type of `kind`, like `int8` for [`Kind::Int8`]. Kinds that
    /// are not basic types are [`Kind::Invalid`].
    pub fn basic(kind: Kind) -> Ty {
        Ty::plain(kind, kind.name())
    }

    /// `[]uint8`, Go's `[]byte`.
    pub fn byte_slice() -> Ty {
        Ty::slice(Ty::uint8())
    }

    /// `[]elem`.
    pub fn slice(elem: Ty) -> Ty {
        let name = format!("[]{}", elem.0.name);
        Ty::new(Data {
            elem: Some(elem),
            ..Data::unnamed(Kind::Slice, name)
        })
    }

    /// `[len]elem`.
    pub fn array(elem: Ty, len: usize) -> Ty {
        let name = format!("[{len}]{}", elem.0.name);
        Ty::new(Data {
            elem: Some(elem),
            len,
            ..Data::unnamed(Kind::Array, name)
        })
    }

    /// `map[key]elem`.
    pub fn map(key: Ty, elem: Ty) -> Ty {
        let name = format!("map[{}]{}", key.0.name, elem.0.name);
        Ty::new(Data {
            elem: Some(elem),
            key: Some(key),
            ..Data::unnamed(Kind::Map, name)
        })
    }

    /// A function type, like `func(interface {}) bool`.
    pub fn func(ins: Vec<Ty>, outs: Vec<Ty>, variadic: bool) -> Ty {
        let mut name = String::from("func(");
        for (i, t) in ins.iter().enumerate() {
            if i > 0 {
                name.push_str(", ");
            }
            if variadic && i + 1 == ins.len() {
                name.push_str("...");
                name.push_str(
                    &t.0.elem
                        .as_ref()
                        .map_or_else(String::new, |e| e.0.name.clone()),
                );
            } else {
                name.push_str(&t.0.name);
            }
        }
        name.push(')');
        match outs.len() {
            0 => {}
            1 => {
                name.push(' ');
                name.push_str(&outs[0].0.name);
            }
            _ => {
                name.push_str(" (");
                let names: Vec<&str> = outs.iter().map(|t| t.0.name.as_str()).collect();
                name.push_str(&names.join(", "));
                name.push(')');
            }
        }
        Ty::new(Data {
            func: Some(Signature {
                ins,
                outs,
                variadic,
            }),
            ..Data::unnamed(Kind::Func, name)
        })
    }

    /// A defined type that is a struct, like `data.Item`. `name` is the
    /// qualified name, as `Type.String()` prints it.
    pub fn named_struct(name: &str, fields: Vec<Field>, methods: Vec<Method>) -> Ty {
        Ty::new(Data {
            named: true,
            fields,
            methods: method_cell(methods),
            ..Data::unnamed(Kind::Struct, name.to_owned())
        })
    }

    /// A defined type over a basic kind, like `time.Duration` over `int64`.
    pub fn named_basic(name: &str, kind: Kind, methods: Vec<Method>) -> Ty {
        Ty::new(Data {
            named: true,
            methods: method_cell(methods),
            ..Data::unnamed(kind, name.to_owned())
        })
    }

    /// `*elem`, a pointer to a defined type, like `*time.Location`.
    pub fn pointer(elem: Ty) -> Ty {
        let name = format!("*{}", elem.0.name);
        Ty::new(Data {
            elem: Some(elem),
            ..Data::unnamed(Kind::Ptr, name)
        })
    }

    /// Sets the methods of a defined type, once. A method may name the type it
    /// belongs to, as `time.Time`'s `Add` returns a `time.Time`; such a type is
    /// created first and given its methods after.
    pub fn set_methods(&self, methods: Vec<Method>) {
        if self.0.methods.set(methods).is_err() {
            panic!("the methods of {self} are set twice");
        }
    }

    /// The kind.
    pub fn kind(&self) -> Kind {
        self.0.kind
    }

    /// The name `Type.String()` prints.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Whether the type is defined by a package (Go's `PkgPath() != ""`).
    pub fn is_named(&self) -> bool {
        self.0.named
    }

    /// The element type of a pointer, slice, array or map.
    pub fn elem(&self) -> Option<&Ty> {
        self.0.elem.as_ref()
    }

    /// The key type of a map.
    pub fn key(&self) -> Option<&Ty> {
        self.0.key.as_ref()
    }

    /// The length of an array, Go's `Type.Len`.
    pub fn array_len(&self) -> usize {
        self.0.len
    }

    /// The fields of a struct, in declaration order.
    pub fn fields(&self) -> &[Field] {
        &self.0.fields
    }

    /// The methods of a defined type.
    pub fn methods(&self) -> &[Method] {
        match self.0.methods.get() {
            Some(methods) => methods,
            None => &[],
        }
    }

    /// The signature of a function type.
    pub fn signature(&self) -> Option<&Signature> {
        self.0.func.as_ref()
    }

    /// The method named `name`, if the type has one.
    pub fn method(&self, name: &str) -> Option<&Method> {
        self.methods().iter().find(|m| m.name == name)
    }

    /// Go's `Type.AssignableTo`: the same type, or an interface the type
    /// implements. The interfaces here have no methods, so any type fits
    /// `interface {}`.
    pub fn assignable_to(&self, other: &Ty) -> bool {
        self == other || other.kind() == Kind::Interface
    }

    /// The index path of the field `name`, following promoted fields of
    /// embedded structs, like Go's `FieldByName`. Own fields win; embedded
    /// ones are searched in order.
    pub fn field_index(&self, name: &str) -> Option<Vec<usize>> {
        if self.kind() != Kind::Struct {
            return None;
        }
        if let Some(i) = self.0.fields.iter().position(|f| f.name == name) {
            return Some(vec![i]);
        }
        for (i, field) in self.0.fields.iter().enumerate() {
            if !field.embedded || field.ty.kind() != Kind::Struct {
                continue;
            }
            if let Some(mut path) = field.ty.field_index(name) {
                path.insert(0, i);
                return Some(path);
            }
        }
        None
    }

    /// The type of the field at `path`, which is what [`Ty::field_index`]
    /// returns.
    pub fn field_type_at(&self, path: &[usize]) -> Option<Ty> {
        let mut current = self.clone();
        for &i in path {
            let field = current.fields().get(i)?;
            current = field.ty.clone();
        }
        Some(current)
    }
}

/// The method set of a new type. It is set now when there are methods, so
/// that a type made without them can still be given some by
/// [`Ty::set_methods`].
fn method_cell(methods: Vec<Method>) -> OnceCell<Vec<Method>> {
    let cell = OnceCell::new();
    if !methods.is_empty() {
        let _ = cell.set(methods);
    }
    cell
}

impl Data {
    fn unnamed(kind: Kind, name: String) -> Data {
        Data {
            kind,
            name,
            named: false,
            elem: None,
            key: None,
            len: 0,
            fields: Vec::new(),
            methods: OnceCell::new(),
            func: None,
        }
    }
}
