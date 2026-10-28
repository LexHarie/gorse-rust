// Ports: config/config.go, config/config.toml @ add21b5

//! Loading, defaulting and validating `config.toml` and the `GORSE_*`
//! environment variables.
//!
//! Work in progress (WP-1-15): the Go value model and the format decoders
//! viper reads configuration files with.

pub mod formats;
pub mod value;
