//! Differential conformance tests between the Go and Rust builds of Gorse.
//!
//! [`schema`] holds the WP-2-13 schema checker: each backend is initialised by
//! the Go build and by the Rust stores, and the resulting schemas are dumped and
//! compared with each other and with `contracts/schema/`. [`cluster`] holds the
//! WP-4-1 mixed-cluster harness, which runs Go nodes and a Rust worker against
//! one set of stores. The other test files (`metrics`, `rest_golden`, `sdk`,
//! `playground`) belong to later work packages; see `conformance/README.md`.

pub mod cluster;
pub mod schema;
