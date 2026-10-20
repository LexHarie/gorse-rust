// Ports: storage/errors.go @ add21b5

//! The sentinel errors every store returns and callers test for.

use thiserror::Error;

/// Go's sentinel errors in package storage, with Go's texts. Go compares
/// them with `errors.Is`; here they are the variants of one enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
pub enum Error {
    /// Go's `ErrNotFound`.
    #[error("not found")]
    NotFound,
    /// Go's `ErrNoDatabase`.
    #[error("database not assigned")]
    NoDatabase,
    /// Go's `ErrInvalidArgument`.
    #[error("invalid argument")]
    InvalidArgument,
    /// Go's `ErrNotSupported`.
    #[error("not supported")]
    NotSupported,
    /// Go's `ErrAlreadyExists`.
    #[error("already exists")]
    AlreadyExists,
}
