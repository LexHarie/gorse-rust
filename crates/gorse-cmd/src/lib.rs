// Ports: cmd/gorse-master/main.go, cmd/gorse-server/main.go, cmd/gorse-worker/main.go, cmd/gorse-in-one/main.go, cmd/version/version.go, common/log/log.go (AddFlags, SetLogger) @ add21b5

//! Command-line entry points of the Rust port: the `gorse-master`,
//! `gorse-server`, `gorse-worker` and `gorse-in-one` binaries, and the version
//! banner they print.
//!
//! Each binary is a thin `main` in `src/bin/` that calls the `main` of its
//! module here. The modules hold the flags (checked against
//! `contracts/cli/*.help.txt` by `tests/cli_contract.rs`), the node start-up
//! (what Go's `main` does, in the same order) and the shared flag handling in
//! [`cli`].

pub mod cli;
pub mod cli_context;
pub mod cli_json;
pub mod cli_output;
pub mod cli_time;
pub mod cli_yaml;
pub mod gorse_cli;
pub mod in_one;
pub mod master;
pub mod server;
pub mod version;
pub mod worker;
