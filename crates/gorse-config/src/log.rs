// Ports: common/log (the Logger that config.go writes to) @ add21b5 — placeholder

//! A placeholder for Go's `common/log` until WP-1-8 lands. `LoadConfig`
//! logs a warning when the file is missing and an info line when it has no
//! file. The placeholder drops both messages; nothing in the config's
//! results depends on them.

// TODO(WP-1-8): replace with gorse_common::log once common/log is merged.

/// `log.Logger().Warn`.
pub(crate) fn warn(_message: &str) {}

/// `log.Logger().Info`.
pub(crate) fn info(_message: &str) {}
