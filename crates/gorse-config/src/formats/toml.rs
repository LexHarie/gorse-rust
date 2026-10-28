// Ports: github.com/pelletier/go-toml/v2 v2.2.4 Unmarshal into a map[string]interface{}, as spf13/viper v1.21.0 internal/encoding/toml/codec.go calls it @ add21b5
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! viper's TOML codec: `toml.Unmarshal(b, &v)` with go-toml v2, v a
//! `map[string]any`.
//!
//! go-toml implements TOML 1.0 (plus the `\e` escape), and its own rules on
//! top: what it accepts, what it rejects and with which message, and which
//! Go type each value becomes:
//!
//! | TOML | Go type | [`Value`] |
//! |---|---|---|
//! | string | `string` | `String` |
//! | integer | `int64` | `Int64` |
//! | float, `inf`, `nan` | `float64` | `Float64` |
//! | boolean | `bool` | `Bool` |
//! | offset date-time | `time.Time` in UTC or `time.FixedZone("", offset)` | `Time` |
//! | local date, time, date-time | `toml.LocalDate`, `toml.LocalTime`, `toml.LocalDateTime` | `LocalDate`, `LocalTime`, `LocalDateTime` |
//! | array, array of tables | `[]interface {}` | `Slice` |
//! | table, inline table | `map[string]interface {}` | `Map` |
//!
//! The port follows go-toml's code: `parser` (the `unstable` package),
//! `decode` (numbers and dates), `tracker` (`internal/tracker`, which
//! rejects keys defined twice) and `unmarshaler` (the decoder), with
//! `errors` for the error texts. `porting/notes/WP-1-15-toml.md` lists the
//! quirks it keeps.

mod ast;
mod characters;
mod decode;
mod errors;
mod parser;
mod tracker;
mod unmarshaler;

use std::fmt;

pub use errors::DecodeError;

use crate::value::{Map, Value};
use unmarshaler::{Decoder, DecoderError};

/// An error of go-toml's `Unmarshal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A `*toml.DecodeError`: a syntax error or a bad value, at a position
    /// in the document.
    Decode(DecodeError),
    /// Any other error, by its text: a key defined twice, a table that is
    /// also a value, and the like (go-toml makes these with `fmt.Errorf`).
    Other(String),
}

/// `Error()`.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Decode(err) => err.fmt(f),
            Error::Other(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Error {}

/// Runs `f` on a stack with room for it. go-toml's parser, tracker and
/// decoder recurse once per level of nesting (arrays, inline tables, the
/// parts of a key), and Go's growable stacks take any depth.
fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 128 * 1024;
    const NEW_STACK: usize = 4 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_STACK, f)
}

/// go-toml's `Unmarshal(input, &m)` with m an empty `map[string]any`: the
/// entries it leaves in m, in document order, or its error.
///
/// Errors at the very end of the document are placed as Go places them when
/// the input slice has spare capacity, which viper's input always has (it
/// reads the file into a `bytes.Buffer`). With `len == cap`, Go reslices to
/// zero capacity without moving the pointer and reports such errors at the
/// start of the slice it was scanning (`a=[` at column 3, not 4).
pub fn unmarshal(input: &[u8]) -> Result<Map, Error> {
    let mut d = Decoder::new(input);
    let mut root = Value::Map(Map::new());
    let result = d.decode(&mut root);
    let err = match result {
        Ok(()) => {
            return match &mut root {
                Value::Map(m) => Ok(std::mem::take(m)),
                // The decoder only changes the entries of the root map.
                other => unreachable!("the root became a {}", other.type_name()),
            };
        }
        Err(DecoderError::Parser(err)) => {
            Error::Decode(errors::wrap_decode_error(d.parser().data(), &err))
        }
        Err(DecoderError::Other(text)) => Error::Other(text),
    };
    Err(err)
}

/// viper's TOML codec: `toml.Unmarshal(b, &v)` into a `map[string]any`, or
/// the error's text (viper adds `While parsing config: `).
pub fn decode(input: &[u8]) -> Result<Map, String> {
    unmarshal(input).map_err(|err| err.to_string())
}
