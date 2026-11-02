# gorse_rs at 2de2691, vendored for the WP-5-2 SDK suite

This folder is `gorse_rs` 0.5.0 from `github.com/gorse-io/gorse-rs` at commit
`2de2691d82fb40015dd69ba2e071dc1c224eae65` (`main`, 2026-11). It is copied:
`Cargo.toml`, `README.md` and `LICENSE` unchanged, and `src/lib.rs` with one formatting
change. The workspace rustfmt (style 2024) reflows one `assert!` in the tests and reorders
one `use crate::{...}` list in `blocking`. No code, test, name or attribute changed. The
check is a `diff` against `src/lib.rs` in `gorse-io/gorse-rs` at 2de2691, which differs only in those two spots.

Why it is vendored and not taken from crates.io: the crates.io 0.5.0 package has
no test suite, and the suite lives in this repository's `src/lib.rs` (the
`tests` and `blocking::tests` modules). The conformance test
`conformance/tests/sdk.rs` ports those tests, and the SDK at the same commit
gives the client they exercise. Vendoring keeps the build free of network access,
as `third_party/mongodb-3.9.1` does. The tree is excluded from the workspace, as
the other vendored crates are. Licence: Apache-2.0, as the workspace.
