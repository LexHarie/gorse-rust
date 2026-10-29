// Ports: go-restful v3.13.0's ReadEntity and the decoding of a JSON body by
// encoding/json (Go 1.27.0). The scanner is in gojson.rs.

//! Go's `ReadEntity`: a request body read into the Go type a handler expects.
//!
//! The content type is checked before the body. A JSON body is scanned by
//! [`super::gojson`], and its first value is decoded with `encoding/json`'s
//! rules: a member name matches a field whatever its case, `null` leaves a
//! string, bool or number as it is and sets a pointer, slice or `any` to nil,
//! and the first type error in document order is the one reported.

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use gorse_common::gotime::{self, Time};
use gorse_storage::data::types::{ItemPatch, Labels, User, UserPatch};
use indexmap::IndexMap;

use super::gojson::{Kind, Member, Node, is_blank, scan};
use super::response::ApiError;

/// The media type of a JSON body, which go-restful matches by containment.
const JSON: &str = "application/json";

/// The media type of an XML body, which go-restful matches by containment.
const XML: &str = "application/xml";

/// A Go struct that a request body decodes into.
pub(crate) trait GoStruct: Default {
    /// The struct's name, as `encoding/json` writes it in a field error
    /// (`User`).
    const NAME: &'static str;
    /// The struct's type, as Go writes it in a value error (`data.User`).
    const TYPE: &'static str;
    /// The struct's fields in declaration order: the JSON name, which is the
    /// field's name, and the kind of value the field takes.
    const FIELDS: &'static [(&'static str, FieldKind)];

    /// Sets the field called `name` to `value`. The pair always comes from
    /// `FIELDS`, so a pair the struct does not take is ignored.
    fn set(&mut self, name: &str, value: Assign);
}

/// The kind of a field, which decides what its JSON value must be and what
/// `null` does to it.
#[derive(Clone, Copy)]
pub(crate) enum FieldKind {
    /// A `string`: `null` leaves it as it is.
    String,
    /// A `*string`: `null` makes it nil.
    OptString,
    /// A `bool`: `null` leaves it as it is.
    Bool,
    /// A `*bool`: `null` makes it nil.
    OptBool,
    /// A `float64`: `null` leaves it as it is.
    Float64,
    /// A `[]string`: `null` makes it nil.
    Strings,
    /// A `*time.Time`: `null` makes it nil.
    OptTime,
    /// An `any`, which takes every JSON value. `null` makes it nil.
    Labels,
}

/// A decoded field value, for [`GoStruct::set`].
pub(crate) enum Assign {
    /// A string.
    String(String),
    /// A bool.
    Bool(bool),
    /// A float64.
    Float64(f64),
    /// A list of strings.
    Strings(Vec<String>),
    /// A time.
    Time(Time),
    /// Any value, which is [`Labels::Nil`] for `null`.
    Labels(Labels),
    /// Nil, for a pointer or a slice that `null` sets to nil.
    Null,
}

/// A value a request body decodes into: a struct, or a list of structs.
pub(crate) trait GoEntity: Sized {
    /// Decodes the root value of a body. The body has been scanned, so the
    /// root is a complete value.
    fn decode(root: &Node, body: &[u8]) -> Result<Self, String>;
}

impl<T: GoStruct> GoEntity for T {
    fn decode(root: &Node, body: &[u8]) -> Result<Self, String> {
        match &root.kind {
            Kind::Null => Ok(T::default()),
            Kind::Object(members) => decode_members(
                members,
                body,
                &Place {
                    name: T::NAME,
                    prefix: String::new(),
                },
            ),
            _ => Err(root_error(value_name(root), T::TYPE)),
        }
    }
}

impl<T: GoStruct> GoEntity for Vec<T> {
    fn decode(root: &Node, body: &[u8]) -> Result<Self, String> {
        match &root.kind {
            Kind::Null => Ok(Vec::new()),
            Kind::Array(items) => items
                .iter()
                .enumerate()
                .map(|(index, item)| decode_element::<T>(item, index, body))
                .collect(),
            _ => Err(root_error(value_name(root), &format!("[]{}", T::TYPE))),
        }
    }
}

/// Go's `ReadEntity`: the body, read into the Go type `T`.
///
/// # Errors
///
/// Fails with the text go-restful and `encoding/json` give. A content type
/// that is neither JSON nor XML is refused before the body is read. A body with
/// no value is `EOF`. An XML body is not decoded: one with an element is refused
/// here (X-WP42-17).
pub(crate) fn read_entity<T: GoEntity>(headers: &HeaderMap, body: &Bytes) -> Result<T, ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .map_or_else(String::new, |value| {
            String::from_utf8_lossy(value.as_bytes()).into_owned()
        });
    if content_type.contains(JSON) {
        json_entity(body)
    } else if content_type.contains(XML) {
        xml_entity(body)
    } else {
        Err(ApiError::BadRequest(format!(
            "[ServiceError:400] Unable to unmarshal content of type:{content_type}"
        )))
    }
}

/// Decodes a JSON body: its first value, scanned and then decoded.
fn json_entity<T: GoEntity>(body: &[u8]) -> Result<T, ApiError> {
    if is_blank(body) {
        return Err(ApiError::BadRequest("EOF".to_owned()));
    }
    let root = scan(body).map_err(ApiError::BadRequest)?;
    T::decode(&root, body).map_err(ApiError::BadRequest)
}

/// Refuses an XML body. Go's XML decoder reports a body with no element as
/// `EOF`, which is what this gives for it.
fn xml_entity<T>(body: &[u8]) -> Result<T, ApiError> {
    if body.contains(&b'<') {
        Err(ApiError::BadRequest(
            "XML request bodies are not supported".to_owned(),
        ))
    } else {
        Err(ApiError::BadRequest("EOF".to_owned()))
    }
}

/// Where a struct sits in the document. The names in its errors come from it.
struct Place {
    /// The struct's name in a field error: `User` for a root object, and `""`
    /// for an element of a root array, as the error reads `.0.UserId`.
    name: &'static str,
    /// The path that leads to the struct's fields: `""` for a root object,
    /// and `"0."` for element 0 of a root array.
    prefix: String,
}

/// Decodes the members of one object into `T`, in document order. The first
/// error is returned.
fn decode_members<T: GoStruct>(
    members: &[Member],
    body: &[u8],
    place: &Place,
) -> Result<T, String> {
    let mut value = T::default();
    for member in members {
        let Some(&(name, kind)) = find_field::<T>(&member.name) else {
            continue;
        };
        let path = format!("{}{}", place.prefix, member.name);
        if let Some(assigned) = assign(&member.value, kind, body, place, &path)? {
            value.set(name, assigned);
        }
    }
    Ok(value)
}

/// Decodes element `index` of a root array into `T`.
fn decode_element<T: GoStruct>(item: &Node, index: usize, body: &[u8]) -> Result<T, String> {
    match &item.kind {
        Kind::Null => Ok(T::default()),
        Kind::Object(members) => decode_members(
            members,
            body,
            &Place {
                name: "",
                prefix: format!("{index}."),
            },
        ),
        _ => Err(format!(
            "json: cannot unmarshal {} into .{index} of type {}",
            value_name(item),
            T::TYPE
        )),
    }
}

/// The field a member name matches: the field whose name is the same, or
/// else the first field whose name folds to the same letters.
fn find_field<T: GoStruct>(name: &str) -> Option<&'static (&'static str, FieldKind)> {
    T::FIELDS
        .iter()
        .find(|(field, _)| *field == name)
        .or_else(|| T::FIELDS.iter().find(|(field, _)| fold_eq(field, name)))
}

/// What a field takes from its value, or `None` when `null` leaves the field
/// as it is.
fn assign(
    node: &Node,
    kind: FieldKind,
    body: &[u8],
    place: &Place,
    path: &str,
) -> Result<Option<Assign>, String> {
    if matches!(node.kind, Kind::Null) {
        return Ok(match kind {
            FieldKind::String | FieldKind::Bool | FieldKind::Float64 => None,
            FieldKind::Labels => Some(Assign::Labels(Labels::Nil)),
            FieldKind::OptString | FieldKind::OptBool | FieldKind::Strings | FieldKind::OptTime => {
                Some(Assign::Null)
            }
        });
    }
    match (kind, &node.kind) {
        (FieldKind::Labels, _) => Ok(Some(Assign::Labels(labels(node, body)))),
        (FieldKind::String | FieldKind::OptString, Kind::String(text)) => {
            Ok(Some(Assign::String(text.clone())))
        }
        (FieldKind::Bool | FieldKind::OptBool, Kind::Bool(flag)) => Ok(Some(Assign::Bool(*flag))),
        (FieldKind::Float64, Kind::Number) => {
            let text = String::from_utf8_lossy(&body[node.span.clone()]);
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() => Ok(Some(Assign::Float64(value))),
                _ => Err(field_error(
                    &format!("number {text}"),
                    place,
                    path,
                    "float64",
                )),
            }
        }
        (FieldKind::Strings, Kind::Array(items)) => {
            strings(items, place, path).map(|list| Some(Assign::Strings(list)))
        }
        (FieldKind::OptTime, Kind::String(text)) => {
            rfc3339(text).map(|time| Some(Assign::Time(time)))
        }
        _ => Err(field_error(value_name(node), place, path, go_type(kind))),
    }
}

/// The strings of a `[]string` field. A `null` element is the empty string,
/// and any other element that is not a string is a type error.
fn strings(items: &[Node], place: &Place, path: &str) -> Result<Vec<String>, String> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match &item.kind {
            Kind::Null => Ok(String::new()),
            Kind::String(text) => Ok(text.clone()),
            _ => Err(field_error(
                value_name(item),
                place,
                &format!("{path}.{index}"),
                "string",
            )),
        })
        .collect()
}

/// The labels of a request, built from the scanned tree. Go decodes the value
/// into an `any`, keeping each number as its `json.Number` text, and it does
/// not refuse a value for its size or its nesting. The tree is walked with a
/// stack, so a value nested to the scanner's limit is built in full.
fn labels(root: &Node, body: &[u8]) -> Labels {
    /// A container being built, and the child that comes next.
    enum Open<'a> {
        Array {
            rest: std::slice::Iter<'a, Node>,
            items: Vec<Labels>,
        },
        Object {
            rest: std::slice::Iter<'a, Member>,
            map: IndexMap<String, Labels>,
            name: String,
        },
    }
    let mut open: Vec<Open> = Vec::new();
    let mut next = Some(root);
    let mut done = None;
    loop {
        if let Some(node) = next.take() {
            match &node.kind {
                Kind::Array(items) => open.push(Open::Array {
                    rest: items.iter(),
                    items: Vec::new(),
                }),
                Kind::Object(members) => open.push(Open::Object {
                    rest: members.iter(),
                    map: IndexMap::new(),
                    name: String::new(),
                }),
                _ => done = Some(scalar_label(node, body)),
            }
        }
        if let Some(value) = done.take() {
            match open.last_mut() {
                None => return value,
                Some(Open::Array { items, .. }) => items.push(value),
                Some(Open::Object { map, name, .. }) => {
                    map.insert(std::mem::take(name), value);
                }
            }
        }
        let finished = match open.last_mut() {
            None => unreachable!("a value with no open container is returned above"),
            Some(Open::Array { rest, items }) => match rest.next() {
                Some(child) => {
                    next = Some(child);
                    None
                }
                None => Some(Labels::Array(std::mem::take(items))),
            },
            Some(Open::Object { rest, map, name }) => match rest.next() {
                Some(member) => {
                    name.clone_from(&member.name);
                    next = Some(&member.value);
                    None
                }
                None => Some(Labels::Object(std::mem::take(map))),
            },
        };
        if let Some(value) = finished {
            open.pop();
            done = Some(value);
        }
    }
}

/// A label that is not a container: `null`, a `bool`, a string or a number.
fn scalar_label(node: &Node, body: &[u8]) -> Labels {
    match &node.kind {
        Kind::Null => Labels::Nil,
        Kind::Bool(flag) => Labels::Bool(*flag),
        Kind::String(text) => Labels::String(text.clone()),
        Kind::Number => {
            Labels::Number(String::from_utf8_lossy(&body[node.span.clone()]).into_owned())
        }
        Kind::Array(_) | Kind::Object(_) => unreachable!("containers are opened, not read"),
    }
}

/// A time string, read as `time.Time`'s `UnmarshalJSON` reads it: with
/// `time.Parse` and the RFC 3339 layout. Its error text is Go's.
fn rfc3339(text: &str) -> Result<Time, String> {
    gotime::parse(gotime::RFC3339, text).map_err(|err| err.to_string())
}

/// The text of a field error. A field's path ends in its name, or in the index of
/// an element of a list, and Go writes the two differently.
fn field_error(value: &str, place: &Place, path: &str, go_type: &str) -> String {
    let location = format!("{}.{path}", place.name);
    if ends_in_index(path) {
        format!("json: cannot unmarshal {value} into {location} of type {go_type}")
    } else {
        format!("json: cannot unmarshal {value} into Go struct field {location} of type {go_type}")
    }
}

/// Whether a path ends in an index: a last part of only digits.
fn ends_in_index(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The text of an error for a root value of the wrong kind.
fn root_error(value: &str, go_type: &str) -> String {
    format!("json: cannot unmarshal {value} into Go value of type {go_type}")
}

/// The name of a JSON value's kind, as `encoding/json` names it in an error.
fn value_name(node: &Node) -> &'static str {
    match node.kind {
        Kind::Null => "null",
        Kind::Bool(_) => "bool",
        Kind::Number => "number",
        Kind::String(_) => "string",
        Kind::Array(_) => "array",
        Kind::Object(_) => "object",
    }
}

/// The Go type a field takes, as an error names it.
fn go_type(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::String | FieldKind::OptString => "string",
        FieldKind::Bool | FieldKind::OptBool => "bool",
        FieldKind::Float64 => "float64",
        FieldKind::Strings => "[]string",
        FieldKind::OptTime => "time.Time",
        FieldKind::Labels => "interface {}",
    }
}

/// Whether two names are equal under Unicode simple case folding, which is the
/// comparison Go's decoder makes.
fn fold_eq(field: &str, name: &str) -> bool {
    field.chars().map(fold_char).eq(name.chars().map(fold_char))
}

/// A character folded to its lower-case letter, through its upper case, so that
/// `ſ` folds with `s` and the Kelvin sign with `k`.
fn fold_char(c: char) -> char {
    let mut upper = c.to_uppercase();
    let upper = match (upper.next(), upper.next()) {
        (Some(one), None) => one,
        _ => c,
    };
    upper.to_lowercase().next().unwrap_or(upper)
}

impl GoStruct for User {
    const NAME: &'static str = "User";
    const TYPE: &'static str = "data.User";
    const FIELDS: &'static [(&'static str, FieldKind)] = &[
        ("UserId", FieldKind::String),
        ("Labels", FieldKind::Labels),
        ("Comment", FieldKind::String),
    ];

    fn set(&mut self, name: &str, value: Assign) {
        match (name, value) {
            ("UserId", Assign::String(text)) => self.user_id = text,
            ("Labels", Assign::Labels(labels)) => self.labels = labels,
            ("Comment", Assign::String(text)) => self.comment = text,
            _ => {}
        }
    }
}

impl GoStruct for UserPatch {
    const NAME: &'static str = "UserPatch";
    const TYPE: &'static str = "data.UserPatch";
    const FIELDS: &'static [(&'static str, FieldKind)] = &[
        ("Labels", FieldKind::Labels),
        ("Comment", FieldKind::OptString),
    ];

    fn set(&mut self, name: &str, value: Assign) {
        match (name, value) {
            ("Labels", Assign::Labels(labels)) => self.labels = labels,
            ("Comment", Assign::String(text)) => self.comment = Some(text),
            ("Comment", Assign::Null) => self.comment = None,
            _ => {}
        }
    }
}

impl GoStruct for ItemPatch {
    const NAME: &'static str = "ItemPatch";
    const TYPE: &'static str = "data.ItemPatch";
    const FIELDS: &'static [(&'static str, FieldKind)] = &[
        ("IsHidden", FieldKind::OptBool),
        ("Categories", FieldKind::Strings),
        ("Timestamp", FieldKind::OptTime),
        ("Labels", FieldKind::Labels),
        ("Comment", FieldKind::OptString),
    ];

    fn set(&mut self, name: &str, value: Assign) {
        match (name, value) {
            ("IsHidden", Assign::Bool(flag)) => self.is_hidden = Some(flag),
            ("IsHidden", Assign::Null) => self.is_hidden = None,
            ("Categories", Assign::Strings(list)) => self.categories = Some(list),
            ("Categories", Assign::Null) => self.categories = None,
            ("Timestamp", Assign::Time(time)) => self.timestamp = Some(time),
            ("Timestamp", Assign::Null) => self.timestamp = None,
            ("Labels", Assign::Labels(labels)) => self.labels = labels,
            ("Comment", Assign::String(text)) => self.comment = Some(text),
            ("Comment", Assign::Null) => self.comment = None,
            _ => {}
        }
    }
}
