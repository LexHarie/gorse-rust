// Ports: pelletier/go-toml/v2 v2.2.4 Unmarshal into map[string]interface{} (spf13/viper v1.21.0 internal/encoding/toml) @ add21b5

//! Work in progress: the go-toml port lands here.

use crate::value::Map;

/// viper's TOML codec: `toml.Unmarshal(b, &v)` into a `map[string]any`.
pub fn decode(_input: &[u8]) -> Result<Map, String> {
    Err("toml: not ported yet".to_owned())
}
