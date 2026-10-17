// Ports: storage/scheme.go, storage/options.go, storage/errors.go @ add21b5

//! Backend URL prefixes, storage options, table-prefix helpers and storage error kinds.
//!
//! This is the Go `storage` root package. It is a separate crate because `config`
//! imports it while `storage/data` and `storage/blob` import `config`; see
//! `porting/crate-selection.md`.
//!
//! Skeleton only (Phase 0): no Go code has been ported into this crate yet.
