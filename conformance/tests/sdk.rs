// Ports: client/setup-test.sh and client/client_test.go @ add21b5 (WP-5-2, Leg A).

//! The WP-5-2 acceptance test, Leg A: the Go SDK suite (`client/client_test.go`, run
//! as `TestGorseClientTestSuite`) against the three mixes of the strangler plan. The
//! harness is the one of WP-4-7 (`common/sdk_suite.rs`), and each leg fails unless
//! every suite test runs and passes:
//!
//! 1. a Go master with Rust workers and Go servers;
//! 2. a Go master with Rust workers and Rust servers;
//! 3. a Rust master, Rust workers and Rust servers (all Rust).
//!
//! The all-Go baseline is not one of these legs. WP-4-7 runs it in `mixed_cluster.rs`.
//!
//! Leg B, the `gorse_rs` suite, is not here. Its test source is outside this session's
//! GitHub scope, so nothing is written for it until the source is available.

mod common;

use common::sdk_suite::{LEGS, leg};

/// Leg A, the first mix: a Go master with Rust workers and Go servers.
#[test]
fn sdk_go_master_rust_workers_go_servers() {
    leg(LEGS[1]);
}

/// Leg A, the second mix: a Go master with Rust workers and Rust servers.
#[test]
fn sdk_go_master_rust_workers_rust_servers() {
    leg(LEGS[2]);
}

/// Leg A, the third mix: a Rust master with Rust workers and Rust servers.
#[test]
fn sdk_all_rust() {
    leg(LEGS[3]);
}
