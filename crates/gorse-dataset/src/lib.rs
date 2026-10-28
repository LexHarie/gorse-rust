// Ports: dataset/dataset.go, dataset/dict.go, dataset/index.go, dataset/unified_index.go @ add21b5

//! In-memory training datasets and ID indexes built from the data store.
//!
//! [`dict`] holds the frequency dictionary that maps sparse IDs to dense
//! indices, [`index`] the dense-index map that model blobs store,
//! [`unified_index`] the encoding space of factorisation machines, and
//! [`dataset`] the dataset with its splits and the built-in loaders.

use std::io;

use gorse_common::{encoding, gostrconv};
use gorse_model_core::built_in;

pub mod dataset;
pub mod dict;
pub mod index;
pub mod unified_index;

/// The errors of this crate's readers, writers and loaders. Go returns these
/// as `error` values, and the Go code wraps them with `errors.WithStack`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A reader or writer failed, or the input ended early.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A length-prefixed string could not be read or written.
    #[error(transparent)]
    Encoding(#[from] encoding::Error),
    /// The built-in dataset could not be located or downloaded.
    #[error(transparent)]
    BuiltIn(#[from] built_in::Error),
    /// A field of a dataset file is not an integer.
    #[error(transparent)]
    Parse(#[from] gostrconv::NumError),
    /// A line of a test file is not `(user,item)` followed by negatives.
    /// The text is Go's, misspelling included.
    #[error("wrong foramt: {0}")]
    WrongFormat(String),
    /// A line has fewer tab- or comma-separated fields than it must.
    #[error("too few fields: {0}")]
    TooFewFields(String),
    /// A serialised index declares a negative number of entries.
    #[error("negative index length {0}")]
    NegativeLength(i32),
    /// A serialised unified index has a type tag this version does not know.
    #[error("unknown index type ({0})")]
    UnknownIndexType(u8),
    /// A test negative refers to a user the training set does not have.
    #[error("user index {0} out of range")]
    UserOutOfRange(i64),
}
