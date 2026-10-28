// Ports: go.yaml.in/yaml/v3 v3.0.4 Unmarshal into map[string]interface{} (spf13/viper v1.21.0 internal/encoding/yaml) @ add21b5

//! Work in progress: the yaml.v3 port lands here.

use crate::value::Map;

/// viper's YAML codec: `yaml.Unmarshal(b, &v)` into a `map[string]any`.
pub fn decode(_input: &[u8]) -> Result<Map, String> {
    Err("yaml: not ported yet".to_owned())
}
