// Ports: config/config.go, config/config.toml @ add21b5

//! Loading, defaulting and validating `config.toml` and the `GORSE_*`
//! environment variables.
//!
//! The Go value model and the format decoders viper reads configuration files
//! with are in [`value`] and [`formats`]; [`config`] holds the Gorse
//! configuration itself.

pub mod config;
pub mod formats;
pub mod mapstructure;
pub mod value;
pub mod viper;

mod log;
