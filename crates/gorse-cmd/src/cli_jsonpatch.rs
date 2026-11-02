// Ports: github.com/evanphx/json-patch/v5 v5.9.11 (patch.go: DecodePatch, validatePatch, validateOperation, Operation.Kind, Path, From, ValueInterface, ApplyIndentWithOptions, add, remove, replace, move, test, copy, findObject, the partialDoc and partialArray methods, decodePatchKey) @ add21b5

//! RFC 6902 JSON Patch, as the `pipeline patch` command of `gorse-cli` applies
//! it through evanphx/json-patch. The errors are that library's, word for word,
//! because the command prints them. Only the parts the command reaches are here:
//! the six operations on objects and arrays, with the library's default options
//! (negative array indices count from the end; a missing path is an error).

use gorse_common::gostrconv;
use serde_json::{Map, Value};

/// `ErrMissing`, the text the library's wrapped missing-value errors end with.
const MISSING: &str = "missing value";

/// `ErrInvalid`: a document or a patch that is not valid JSON, or a container
/// that is not an object or an array.
const INVALID_STATE: &str = "invalid state detected";

/// `ErrInvalidIndex`, the text of an array index out of range.
const INVALID_INDEX: &str = "invalid index referenced";

/// `ErrTestFailed`.
const TEST_FAILED: &str = "test failed";

/// One operation of a patch. A `null` in the patch is a nil map, which the
/// library reads as an operation with no fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Operation(Option<Map<String, Value>>);

/// A decoded patch: its operations, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch(Vec<Operation>);

/// `DecodePatch`: the operations of `buf`, with each one checked. The error is
/// the one the library returns.
pub fn decode(buf: &[u8]) -> Result<Patch, String> {
    let value: Value = serde_json::from_slice(buf).map_err(|_| INVALID_STATE.to_owned())?;
    let elements = match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type jsonpatch.Patch",
                json_kind(&other)
            ));
        }
    };
    let mut ops = Vec::with_capacity(elements.len());
    let mut first_error = None;
    for element in elements {
        match element {
            Value::Object(fields) => ops.push(Operation(Some(fields))),
            Value::Null => ops.push(Operation(None)),
            other => {
                first_error.get_or_insert_with(|| {
                    format!(
                        "json: cannot unmarshal {} into Go value of type jsonpatch.Operation",
                        json_kind(&other)
                    )
                });
                ops.push(Operation(None));
            }
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    let patch = Patch(ops);
    validate_patch(&patch)?;
    Ok(patch)
}

/// The name encoding/json gives a value's kind in its type errors.
pub fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The field `name` of an operation, unless it is absent or `null`. A `null`
/// field is a missing one to the library.
fn field<'a>(op: &'a Operation, name: &str) -> Option<&'a Value> {
    op.0.as_ref()?.get(name).filter(|value| !value.is_null())
}

/// `Operation.Kind`: the `op` field, or `unknown` when it is absent or not a string.
fn kind(op: &Operation) -> &str {
    match field(op, "op") {
        Some(Value::String(kind)) => kind,
        _ => "unknown",
    }
}

/// `Operation.Path`: the `path` field, which must be a string.
fn path(op: &Operation) -> Result<&str, String> {
    match field(op, "path") {
        Some(Value::String(path)) => Ok(path),
        Some(other) => Err(type_error(other, "string")),
        None => Err(format!("operation missing path field: {MISSING}")),
    }
}

/// `Operation.From`: the `from` field, which must be a string.
fn from(op: &Operation) -> Result<&str, String> {
    match field(op, "from") {
        Some(Value::String(from)) => Ok(from),
        Some(other) => Err(type_error(other, "string")),
        None => Err(format!("operation, missing from field: {MISSING}")),
    }
}

/// The `value` field. A `null` value is a value, and an absent one is an error.
fn value_interface(op: &Operation) -> Result<Value, String> {
    match op.0.as_ref().and_then(|fields| fields.get("value")) {
        Some(value) => Ok(value.clone()),
        None => Err(format!("operation, missing value field: {MISSING}")),
    }
}

/// The value of an operation that the checks have already passed. A missing
/// value reads as `null`, as the library's nil value does.
fn value_of(op: &Operation) -> Value {
    op.0.as_ref()
        .and_then(|fields| fields.get("value"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The text encoding/json gives when a value of kind `found` is decoded into `want`.
fn type_error(found: &Value, want: &str) -> String {
    format!(
        "json: cannot unmarshal {} into Go value of type {want}",
        json_kind(found)
    )
}

/// `validatePatch`: the first operation that does not validate, with the
/// operation's JSON, which the library writes with its keys sorted.
fn validate_patch(patch: &Patch) -> Result<(), String> {
    for op in &patch.0 {
        if let Err(err) = validate_operation(op) {
            return Err(format!("invalid operation {}: {err}", marshal(op)));
        }
    }
    Ok(())
}

/// `validateOperation`.
fn validate_operation(op: &Operation) -> Result<(), String> {
    match kind(op) {
        "add" | "replace" => {
            value_interface(op).map_err(|err| format!("failed to decode 'value': {err}"))?;
        }
        "move" | "copy" => {
            from(op).map_err(|err| format!("failed to decode 'from': {err}"))?;
        }
        "remove" | "test" => {}
        _ => return Err("unsupported operation".to_owned()),
    }
    path(op).map_err(|err| format!("failed to decode 'path': {err}"))?;
    Ok(())
}

/// Go's `json.Marshal` of an operation: sorted keys, compact, and `<`, `>`,
/// `&` and the two line separators written as escapes.
fn marshal(op: &Operation) -> String {
    let Some(fields) = &op.0 else {
        return "null".to_owned();
    };
    let sorted: std::collections::BTreeMap<&String, &Value> = fields.iter().collect();
    let text = serde_json::to_string(&sorted).unwrap_or_default();
    text.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// `ApplyIndentWithOptions`: applies the operations to `doc`, in order, and
/// returns the patched document. The first failure is returned, and nothing is
/// applied after it.
pub fn apply(doc: &Value, patch: &Patch) -> Result<Value, String> {
    if !matches!(doc, Value::Object(_) | Value::Array(_)) {
        return Err(INVALID_STATE.to_owned());
    }
    let mut doc = doc.clone();
    for op in &patch.0 {
        match kind(op) {
            "add" => add(&mut doc, op)?,
            "remove" => remove(&mut doc, op)?,
            "replace" => replace(&mut doc, op)?,
            "move" => move_value(&mut doc, op)?,
            "test" => test(&mut doc, op)?,
            "copy" => copy(&mut doc, op)?,
            other => return Err(format!("Unexpected kind: {other}")),
        }
    }
    Ok(doc)
}

/// `Patch.add`.
fn add(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let path = path(op).map_err(|_| format!("add operation failed to decode path: {MISSING}"))?;
    if path.is_empty() {
        return add_root(doc, value_of(op));
    }
    match find_object(doc, path) {
        None => Err(format!(
            "add operation does not apply: doc is missing path: \"{path}\": {MISSING}"
        )),
        Some((mut target, key)) => target
            .add(&key, value_of(op))
            .map_err(|err| format!("error in add for path: '{path}': {err}")),
    }
}

/// `Patch.remove`.
fn remove(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let path =
        path(op).map_err(|_| format!("remove operation failed to decode path: {MISSING}"))?;
    match find_object(doc, path) {
        None => Err(format!(
            "remove operation does not apply: doc is missing path: \"{path}\": {MISSING}"
        )),
        Some((mut target, key)) => target
            .remove(&key)
            .map_err(|err| format!("error in remove for path: '{path}': {err}")),
    }
}

/// `Patch.replace`. Its errors for a container that is missing, or a key that
/// is, are the library's own; a failed set is reported with the label
/// `remove`, as the library words it.
fn replace(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let path = path(op).map_err(|err| format!("replace operation failed to decode path: {err}"))?;
    if path.is_empty() {
        return replace_root(doc, value_of(op));
    }
    let Some((mut target, key)) = find_object(doc, path) else {
        return Err(format!(
            "replace operation does not apply: doc is missing path: {path}: {MISSING}"
        ));
    };
    if target.get(&key).is_err() {
        return Err(format!(
            "replace operation does not apply: doc is missing key: {path}: {MISSING}"
        ));
    }
    target
        .set(&key, value_of(op))
        .map_err(|err| format!("error in remove for path: '{path}': {err}"))
}

/// `Patch.add` at the empty path: the value becomes the whole document. Only an
/// object or an array can be decoded into the document; the library's errors
/// for anything else are its own, and reproduced here.
fn add_root(doc: &mut Value, value: Value) -> Result<(), String> {
    match value {
        Value::Object(_) | Value::Array(_) => {
            *doc = value;
            Ok(())
        }
        Value::Null => Err(
            "json: error calling MarshalJSON for type *jsonpatch.partialDoc: invalid value, expected object"
                .to_owned(),
        ),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type map[string]*jsonpatch.lazyNode",
            json_kind(&other)
        )),
    }
}

/// `Patch.replace` at the empty path: the value becomes the whole document. A
/// `null` replaces it with `null`. Any other scalar fails with the library's
/// message, which has an unset error in it, printed by Go as `%!w(<nil>)`.
fn replace_root(doc: &mut Value, value: Value) -> Result<(), String> {
    match value {
        Value::Object(_) | Value::Array(_) | Value::Null => {
            *doc = value;
            Ok(())
        }
        _ => Err("replace operation value must be object or array: %!w(<nil>)".to_owned()),
    }
}

/// `Patch.move`. The source's error labels use the key, not the path, as the
/// library does.
fn move_value(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let from = from(op).map_err(|err| format!("move operation failed to decode from: {err}"))?;
    if from.is_empty() {
        return Err(format!(
            "unable to move entire document to another path: {INVALID_STATE}"
        ));
    }
    let value = {
        let Some((mut target, key)) = find_object(doc, from) else {
            return Err(format!(
                "move operation does not apply: doc is missing from path: {from}: {MISSING}"
            ));
        };
        let value = target
            .get(&key)
            .map_err(|err| format!("error in move for path: '{key}': {err}"))?;
        target
            .remove(&key)
            .map_err(|err| format!("error in move for path: '{key}': {err}"))?;
        value
    };
    let path = path(op).map_err(|err| format!("move operation failed to decode path: {err}"))?;
    match find_object(doc, path) {
        None => Err(format!(
            "move operation does not apply: doc is missing destination path: {path}: {MISSING}"
        )),
        Some((mut target, key)) => target
            .add(&key, value)
            .map_err(|err| format!("error in move for path: '{path}': {err}")),
    }
}

/// `Patch.test`.
fn test(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let path = path(op).map_err(|err| format!("test operation failed to decode path: {err}"))?;
    let expected = value_of(op);
    if path.is_empty() {
        return if *doc == expected {
            Ok(())
        } else {
            Err(format!("testing value {path} failed: {TEST_FAILED}"))
        };
    }
    let Some((target, key)) = find_object(doc, path) else {
        return Err(format!(
            "test operation does not apply: is missing path: {path}: {MISSING}"
        ));
    };
    let found = match target.get(&key) {
        Ok(value) => Some(value),
        Err(GetError::Missing(_)) => None,
        Err(GetError::Other(err)) => {
            return Err(format!("error in test for path: '{path}': {err}"));
        }
    };
    let matches = match found {
        None => expected.is_null(),
        Some(value) => !expected.is_null() && value == expected,
    };
    if matches {
        Ok(())
    } else {
        Err(format!("testing value {path} failed: {TEST_FAILED}"))
    }
}

/// `Patch.copy`.
fn copy(doc: &mut Value, op: &Operation) -> Result<(), String> {
    let from = from(op).map_err(|err| format!("copy operation failed to decode from: {err}"))?;
    let value = {
        let Some((target, key)) = find_object(doc, from) else {
            return Err(format!(
                "copy operation does not apply: doc is missing from path: \"{from}\": {MISSING}"
            ));
        };
        target
            .get(&key)
            .map_err(|err| format!("error in copy for from: '{from}': {err}"))?
    };
    let path = path(op).map_err(|_| format!("copy operation failed to decode path: {MISSING}"))?;
    match find_object(doc, path) {
        None => Err(format!(
            "copy operation does not apply: doc is missing destination path: {path}: {MISSING}"
        )),
        Some((mut target, key)) => target
            .add(&key, value)
            .map_err(|err| format!("error while adding value during copy: {err}")),
    }
}

/// The container that holds the key a JSON Pointer names, and the key. The
/// library's `findObject`: every step must be an object or an array, and the
/// key is the last part of the pointer, with `~1` and `~0` decoded.
fn find_object<'a>(doc: &'a mut Value, path: &str) -> Option<(Target<'a>, String)> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return if path.is_empty() {
            Some((target_of(doc)?, String::new()))
        } else {
            None
        };
    }
    let key = decode_key(parts[parts.len() - 1]);
    let mut current: &'a mut Value = doc;
    for part in &parts[1..parts.len() - 1] {
        current = child(current, &decode_key(part))?;
    }
    Some((target_of(current)?, key))
}

/// The child of a container that a pointer step names, when it is an object or
/// an array. Anything else stops the walk.
fn child<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    let next = match value {
        Value::Object(map) => map.get_mut(key)?,
        Value::Array(items) => {
            let (index, cause) = gostrconv::atoi(key);
            if cause.is_some() {
                return None;
            }
            let index = resolve_index(items.len(), index)?;
            items.get_mut(index)?
        }
        _ => return None,
    };
    match next {
        Value::Object(_) | Value::Array(_) => Some(next),
        _ => None,
    }
}

/// The container that `value` is, when it is an object or an array.
fn target_of(value: &mut Value) -> Option<Target<'_>> {
    match value {
        Value::Object(map) => Some(Target::Object(map)),
        Value::Array(items) => Some(Target::Array(items)),
        _ => None,
    }
}

/// A container that a patch changes: an object, or an array.
enum Target<'a> {
    Object(&'a mut Map<String, Value>),
    Array(&'a mut Vec<Value>),
}

/// The error of a read: whether the key is missing, which the library tests
/// with `errors.Unwrap`.
enum GetError {
    Missing(String),
    Other(String),
}

impl std::fmt::Display for GetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetError::Missing(text) | GetError::Other(text) => f.write_str(text),
        }
    }
}

impl Target<'_> {
    /// `get`: the value at `key`.
    fn get(&self, key: &str) -> Result<Value, GetError> {
        match self {
            Target::Object(map) => {
                if key.is_empty() {
                    return Ok(Value::Object((*map).clone()));
                }
                map.get(key).cloned().ok_or_else(|| {
                    GetError::Missing(format!("unable to get nonexistent key: {key}: {MISSING}"))
                })
            }
            Target::Array(items) => {
                if key.is_empty() {
                    return Ok(Value::Array((*items).clone()));
                }
                let (index, cause) = gostrconv::atoi(key);
                if let Some(cause) = cause {
                    return Err(GetError::Other(cause.to_string()));
                }
                let index = resolve_index_or_err(items.len(), index).map_err(GetError::Other)?;
                items.get(index).cloned().ok_or_else(|| {
                    GetError::Other(format!(
                        "Unable to access invalid index: {index}: {INVALID_INDEX}"
                    ))
                })
            }
        }
    }

    /// `set`: the value at `key`, with the array's index rules. An object has no
    /// `EXPECTED_OBJECT` case here, because it is always an object.
    fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        match self {
            Target::Object(map) => {
                map.insert(key.to_owned(), value);
                Ok(())
            }
            Target::Array(items) => {
                let (index, cause) = gostrconv::atoi(key);
                if let Some(cause) = cause {
                    return Err(cause.to_string());
                }
                let index = resolve_index_or_err(items.len(), index)?;
                match items.get_mut(index) {
                    Some(slot) => {
                        *slot = value;
                        Ok(())
                    }
                    None => Err(format!(
                        "Unable to access invalid index: {index}: {INVALID_INDEX}"
                    )),
                }
            }
        }
    }

    /// `add`: inserts at `key`. An object sets the key; an array inserts before
    /// the index, and `-` appends.
    fn add(&mut self, key: &str, value: Value) -> Result<(), String> {
        match self {
            Target::Object(map) => {
                map.insert(key.to_owned(), value);
                Ok(())
            }
            Target::Array(items) => {
                if key == "-" {
                    items.push(value);
                    return Ok(());
                }
                let (index, cause) = gostrconv::atoi(key);
                if let Some(cause) = cause {
                    return Err(format!(
                        "value was not a proper array index: '{key}': {cause}"
                    ));
                }
                let len = items.len() as i64 + 1;
                if index >= len {
                    return Err(format!(
                        "Unable to access invalid index: {index}: {INVALID_INDEX}"
                    ));
                }
                let index = if index < 0 {
                    if index < -len {
                        return Err(format!(
                            "Unable to access invalid index: {index}: {INVALID_INDEX}"
                        ));
                    }
                    index + len
                } else {
                    index
                };
                items.insert(index as usize, value);
                Ok(())
            }
        }
    }

    /// `remove`: deletes `key`. A missing key or index is an error.
    fn remove(&mut self, key: &str) -> Result<(), String> {
        match self {
            Target::Object(map) => {
                if map.remove(key).is_none() {
                    return Err(format!(
                        "unable to remove nonexistent key: {key}: {MISSING}"
                    ));
                }
                Ok(())
            }
            Target::Array(items) => {
                let (index, cause) = gostrconv::atoi(key);
                if let Some(cause) = cause {
                    return Err(cause.to_string());
                }
                if index >= items.len() as i64 {
                    return Err(format!(
                        "Unable to access invalid index: {index}: {INVALID_INDEX}"
                    ));
                }
                let index = if index < 0 {
                    if index < -(items.len() as i64) {
                        return Err(format!(
                            "Unable to access invalid index: {index}: {INVALID_INDEX}"
                        ));
                    }
                    index + items.len() as i64
                } else {
                    index
                };
                items.remove(index as usize);
                Ok(())
            }
        }
    }
}

/// The array index a key names, counting negative indices from the end, or
/// `None` when it is out of range.
fn resolve_index(len: usize, index: i64) -> Option<usize> {
    let len = len as i64;
    let index = if index < 0 {
        if index < -len {
            return None;
        }
        index + len
    } else {
        index
    };
    usize::try_from(index)
        .ok()
        .filter(|index| (*index as i64) < len)
}

/// [`resolve_index`] with the library's error text for an out-of-range index.
fn resolve_index_or_err(len: usize, index: i64) -> Result<usize, String> {
    if index < -(len as i64) {
        return Err(format!(
            "Unable to access invalid index: {index}: {INVALID_INDEX}"
        ));
    }
    resolve_index(len, index)
        .ok_or_else(|| format!("Unable to access invalid index: {index}: {INVALID_INDEX}"))
}

/// `decodePatchKey`: `~1` is `/`, and `~0` is `~`, in one pass.
fn decode_key(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    let mut chars = part.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.peek() {
                Some('1') => {
                    chars.next();
                    out.push('/');
                    continue;
                }
                Some('0') => {
                    chars.next();
                    out.push('~');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}
