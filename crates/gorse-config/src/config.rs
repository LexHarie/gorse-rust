// Ports: config/config.go @ add21b5

//! The Go `config` package: the `Config` tree and its defaults, loading it from
//! `config.toml` and the environment, validating it, and the JSON form the
//! master sends to the other nodes.

pub mod defaults;
pub mod json;
pub mod load;
pub mod schema;
pub mod validate;

#[cfg(test)]
mod tests;

pub use defaults::{get_default_config, mk_dir, set_default, set_root_dir};
pub use load::{LoadError, load_config, load_config_with_env, unmarshal};
pub use schema::*;
pub use validate::ValidateError;

/// The `config.toml` the repository ships, which the tests load.
pub const CONFIG_TOML: &str = include_str!("../../../config/config.toml");
