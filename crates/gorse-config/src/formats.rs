// Ports: spf13/viper v1.21.0 encoding.go (the codec registry) @ add21b5

//! The decoders viper picks by file extension (`DefaultCodecRegistry`):
//! `toml`, `yaml`/`yml`, `json` and `dotenv`/`env`. Each takes the file's
//! bytes and returns the entries its codec leaves in viper's
//! `map[string]any`, or the parser's error text, which viper wraps in
//! `While parsing config: `.

pub mod dotenv;
pub mod toml;
pub mod yaml;
