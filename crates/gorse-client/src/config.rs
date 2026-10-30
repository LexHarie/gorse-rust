// Ports: client/config.go @ add21b5

//! The config file a `ml-100k` playground writes (Go's `client.ConfigTOML`).

/// The embedded `client/config.toml`.
pub const CONFIG_TOML: &str = include_str!("../../../client/config.toml");
