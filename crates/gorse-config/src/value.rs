// Ports: the `any` values spf13/viper v1.21.0 hands to go-viper/mapstructure v2.5.0, spf13/cast v1.10.0 basic.go (ToStringE) and map.go (ToStringMapE), and pelletier/go-toml v2.4.3 localtime.go (String) @ add21b5
//
// Derived from spf13/cast: Copyright (c) 2014 Steve Francia, MIT license,
// crates/gorse-config/licenses/CAST-LICENSE. Derived from go-toml: Copyright
// (c) 2021 - 2023 Thomas Pelletier, MIT license,
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! Go values of static type `any`, as viper holds them: what the format
//! decoders leave in a `map[string]any`, the typed defaults of `SetDefault`
//! and the strings of environment variables. mapstructure's messages name the
//! Go type or kind of a value ([`Value::type_name`], [`Value::kind`]), so
//! the variants keep Go's types apart even where the numbers are the same
//! (`int` from YAML, `int64` from TOML).

use gorse_common::{gostrconv, gotime};

/// A Go value of static type `any`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `nil`.
    Nil,
    /// `bool`.
    Bool(bool),
    /// `int` (yaml.v3 integers).
    Int(i64),
    /// `int64` (go-toml integers).
    Int64(i64),
    /// `uint64` (yaml.v3 integers above `math.MaxInt64`).
    Uint64(u64),
    /// `float64`.
    Float64(f64),
    /// `string`.
    String(String),
    /// `time.Time` (a TOML offset date-time, a YAML `!!timestamp`). Its fields
    /// are unexported, so mapstructure sees an empty struct.
    Time(gotime::Time),
    /// go-toml's `toml.LocalDate`.
    LocalDate(LocalDate),
    /// go-toml's `toml.LocalTime`.
    LocalTime(LocalTime),
    /// go-toml's `toml.LocalDateTime`, which embeds a `LocalDate` and a
    /// `LocalTime`.
    LocalDateTime(LocalDate, LocalTime),
    /// `[]interface {}`.
    Slice(Vec<Value>),
    /// `map[string]interface {}`.
    Map(Map),
    /// `map[interface {}]interface {}`, a yaml.v3 mapping with a key that is
    /// not a string.
    AnyMap(Vec<(Value, Value)>),
    /// `time.Duration` (a default).
    Duration(i64),
    /// `[]string` (a default); `None` is a nil slice.
    Strings(Option<Vec<String>>),
    /// A struct value (a default): its Go type name and the map mapstructure
    /// makes of its exported fields.
    Struct(&'static str, Map),
}

/// go-toml's `LocalDate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDate {
    /// `Year`.
    pub year: i64,
    /// `Month`.
    pub month: i64,
    /// `Day`.
    pub day: i64,
}

/// go-toml's `LocalTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    /// `Hour`.
    pub hour: i64,
    /// `Minute`.
    pub minute: i64,
    /// `Second`.
    pub second: i64,
    /// `Nanosecond`.
    pub nanosecond: i64,
    /// `Precision`, the number of fractional digits written.
    pub precision: i64,
}

/// Go's `reflect.Kind`, as mapstructure switches on it and prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No value (`reflect.Invalid`).
    Invalid,
    /// `bool`.
    Bool,
    /// `int`.
    Int,
    /// `int64`.
    Int64,
    /// `uint64`.
    Uint64,
    /// `float64`.
    Float64,
    /// `string`.
    String,
    /// `slice`.
    Slice,
    /// `map`.
    Map,
    /// `struct`.
    Struct,
}

impl Kind {
    /// `reflect.Kind.String()`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Invalid => "invalid",
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Int64 => "int64",
            Kind::Uint64 => "uint64",
            Kind::Float64 => "float64",
            Kind::String => "string",
            Kind::Slice => "slice",
            Kind::Map => "map",
            Kind::Struct => "struct",
        }
    }
}

impl Value {
    /// The value's `reflect.Kind`.
    pub fn kind(&self) -> Kind {
        match self {
            Value::Nil => Kind::Invalid,
            Value::Bool(_) => Kind::Bool,
            Value::Int(_) => Kind::Int,
            Value::Int64(_) | Value::Duration(_) => Kind::Int64,
            Value::Uint64(_) => Kind::Uint64,
            Value::Float64(_) => Kind::Float64,
            Value::String(_) => Kind::String,
            Value::Slice(_) | Value::Strings(_) => Kind::Slice,
            Value::Map(_) | Value::AnyMap(_) => Kind::Map,
            Value::Time(_)
            | Value::LocalDate(_)
            | Value::LocalTime(_)
            | Value::LocalDateTime(..)
            | Value::Struct(..) => Kind::Struct,
        }
    }

    /// The value's Go type as `reflect.Type.String()` prints it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "<nil>",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Int64(_) => "int64",
            Value::Uint64(_) => "uint64",
            Value::Float64(_) => "float64",
            Value::String(_) => "string",
            Value::Time(_) => "time.Time",
            Value::LocalDate(_) => "toml.LocalDate",
            Value::LocalTime(_) => "toml.LocalTime",
            Value::LocalDateTime(..) => "toml.LocalDateTime",
            Value::Slice(_) => "[]interface {}",
            Value::Map(_) => "map[string]interface {}",
            Value::AnyMap(_) => "map[interface {}]interface {}",
            Value::Duration(_) => "time.Duration",
            Value::Strings(_) => "[]string",
            Value::Struct(name, _) => name,
        }
    }

    /// Whether the value is a nil slice or map (Go's `reflect.Value.IsNil`).
    pub fn is_nil_container(&self) -> bool {
        matches!(self, Value::Strings(None))
    }

    /// The map mapstructure's `decodeMapFromStruct` makes of a struct value:
    /// its exported fields by name, an embedded struct as a nested map. None
    /// for a value that is not a struct.
    pub fn struct_fields(&self) -> Option<Map> {
        fn int(v: i64) -> Value {
            Value::Int(v)
        }
        fn date(d: &LocalDate) -> Map {
            Map::from(vec![
                ("Year".to_owned(), int(d.year)),
                ("Month".to_owned(), int(d.month)),
                ("Day".to_owned(), int(d.day)),
            ])
        }
        fn time(t: &LocalTime) -> Map {
            Map::from(vec![
                ("Hour".to_owned(), int(t.hour)),
                ("Minute".to_owned(), int(t.minute)),
                ("Second".to_owned(), int(t.second)),
                ("Nanosecond".to_owned(), int(t.nanosecond)),
                ("Precision".to_owned(), int(t.precision)),
            ])
        }
        match self {
            Value::Time(_) => Some(Map::new()),
            Value::LocalDate(d) => Some(date(d)),
            Value::LocalTime(t) => Some(time(t)),
            Value::LocalDateTime(d, t) => Some(Map::from(vec![
                ("LocalDate".to_owned(), Value::Map(date(d))),
                ("LocalTime".to_owned(), Value::Map(time(t))),
            ])),
            Value::Struct(_, fields) => Some(fields.clone()),
            _ => None,
        }
    }
}

impl LocalDate {
    /// go-toml's `LocalDate.String`.
    pub fn string(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl LocalTime {
    /// go-toml's `LocalTime.String`: the fraction to the precision it was
    /// written with, or without trailing zeros if that is unknown.
    pub fn string(&self) -> String {
        let mut s = format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second);
        let frac = format!(".{:09}", self.nanosecond);
        if self.precision > 0 {
            let end = usize::try_from(self.precision + 1).map_or(frac.len(), |n| n.min(frac.len()));
            s.push_str(&frac[..end]);
        } else if self.nanosecond > 0 {
            s.push_str(frac.trim_end_matches('0'));
        }
        s
    }
}

/// spf13/cast's `ToString` (the error discarded): what `cast.ToStringMap`
/// makes of the keys of a `map[interface {}]interface {}`.
pub fn cast_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Float64(f) => gostrconv::format_float(*f, b'f', -1, 64),
        Value::Int(i) | Value::Int64(i) => gostrconv::format_int(*i, 10),
        Value::Uint64(u) => gostrconv::format_uint(*u, 10),
        Value::Nil => String::new(),
        // fmt.Stringer.
        Value::Time(t) => t.string(),
        Value::Duration(d) => gotime::Duration(*d).to_string(),
        Value::LocalDate(d) => d.string(),
        Value::LocalTime(t) => t.string(),
        Value::LocalDateTime(d, t) => format!("{}T{}", d.string(), t.string()),
        // No other type has a case or a String method.
        Value::Slice(_)
        | Value::Map(_)
        | Value::AnyMap(_)
        | Value::Strings(_)
        | Value::Struct(..) => String::new(),
    }
}

/// spf13/cast's `ToStringMap` of a `map[interface {}]interface {}`: each key
/// through [`cast_to_string`]. Go fills the new map in its random iteration
/// order, so of two keys that become the same string either value may win;
/// here the later one in the mapping does (`PORTING_NOTES.md`,
/// "Configuration").
pub fn cast_to_string_map(entries: &[(Value, Value)]) -> Map {
    let mut m = Map::new();
    for (k, v) in entries {
        m.insert(cast_to_string(k), v.clone());
    }
    m
}

/// A `map[string]any`. Go maps have no order; this one keeps insertion order
/// so that the port is deterministic where Go's iteration order is not
/// (`PORTING_NOTES.md`, "Configuration").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map(Vec<(String, Value)>);

impl Map {
    /// An empty map.
    pub fn new() -> Self {
        Map(Vec::new())
    }

    /// `m[key]`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// A mutable `m[key]`.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `m[key] = value`: an existing key keeps its place.
    pub fn insert(&mut self, key: String, value: Value) {
        match self.get_mut(&key) {
            Some(v) => *v = value,
            None => self.0.push((key, value)),
        }
    }

    /// `delete(m, key)`, returning the value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }

    /// The keys, in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    /// The entries, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// `len(m)`.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `len(m) == 0`.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<(String, Value)>> for Map {
    /// A map of the entries; a later duplicate key replaces the earlier value.
    fn from(entries: Vec<(String, Value)>) -> Self {
        let mut m = Map::new();
        for (k, v) in entries {
            m.insert(k, v);
        }
        m
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
