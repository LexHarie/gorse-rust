// Ports: protocol/*.proto, protocol/generate.go @ add21b5

//! Protobuf messages and gRPC services of the Gorse cluster protocol.
//!
//! The code is generated at build time from the frozen copies of the upstream
//! `.proto` files in `contracts/proto` (see `build.rs`); nothing here is written
//! by hand. Message encodings are checked byte for byte against the Go
//! `protocol` package in `tests/golden.rs`.

/// Everything generated from package `protocol`.
#[allow(missing_docs, clippy::all, clippy::pedantic)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/protocol.rs"));
}

pub use generated::*;
