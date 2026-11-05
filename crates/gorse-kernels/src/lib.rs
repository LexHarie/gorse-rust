// Ports: common/floats/floats.go, floats_amd64.go, floats_arm64.go, floats_noasm.go, mm.go @ add21b5

//! The numeric kernels of Gorse, with no behaviour change from the code that
//! `gorse-common` held before (verification plan P1-4).
//!
//! The kernels move here as one small crate with no dependencies on the rest
//! of the workspace. `gorse-common` re-exports `floats`, so every existing path
//! such as `gorse_common::floats::dot` still resolves.

pub mod floats;
