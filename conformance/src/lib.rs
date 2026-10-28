//! Differential conformance tests between the Go and Rust builds of Gorse.
//!
//! [`schema`] holds the WP-2-13 schema checker: each backend is initialised by
//! the Go build and by the Rust stores, and the resulting schemas are dumped and
//! compared with each other and with `contracts/schema/`. The other test files
//! (`metrics`, `mixed_cluster`, `rest_golden`, `sdk`, `playground`) belong to
//! later work packages; see `conformance/README.md`.

pub mod schema;
