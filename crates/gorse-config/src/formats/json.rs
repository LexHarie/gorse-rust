// Ports: spf13/viper v1.21.0 internal/encoding/json/codec.go (json.Unmarshal into a map[string]any) @ add21b5

//! viper's JSON codec: `json.Unmarshal(b, &v)` with `v` a `map[string]any`,
//! decoded by `gorse_common::gojson`, which gives Go 1.27's texts and rules.

use gorse_common::gojson::{self, Value as Json};

use crate::value::{Map, Value};

/// The entries `json.Unmarshal` leaves in viper's map, in document order, or
/// its error text, which viper wraps in `While parsing config: `. A top-level
/// `null` leaves the map empty.
pub fn decode(input: &[u8]) -> Result<Map, String> {
    match gojson::unmarshal_map_any(input) {
        Ok(None) => Ok(Map::new()),
        Ok(Some(members)) => Ok(Map::from(
            members
                .into_iter()
                .map(|(k, v)| (k, value(v)))
                .collect::<Vec<_>>(),
        )),
        Err(err) => Err(err.to_string()),
    }
}

/// The Go value `json.Unmarshal` stores in an `any`: float64 numbers, and the
/// same containers. The contents are taken out of `v`, because gojson's value
/// has a `Drop` impl that rules out moving its fields.
fn value(mut v: Json) -> Value {
    // The tree is as deep as the document, which Go's stacks take at any depth.
    stacker::maybe_grow(128 * 1024, 4 * 1024 * 1024, || match &mut v {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Float64(*n),
        Json::String(s) => Value::String(std::mem::take(s)),
        Json::Array(items) => Value::Slice(std::mem::take(items).into_iter().map(value).collect()),
        Json::Object(members) => Value::Map(Map::from(
            std::mem::take(members)
                .into_iter()
                .map(|(k, v)| (k, value(v)))
                .collect::<Vec<_>>(),
        )),
    })
}
