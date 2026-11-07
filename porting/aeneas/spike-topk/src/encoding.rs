//! A stand-in for `crates/gorse-common/src/encoding.rs`, which this crate cannot
//! include: the real module declares the `gob` submodule and needs `thiserror`.
//! `heap.rs` names these items from it: the `Element` trait with the `i32` and `f32`
//! impls that its `read` and `write` use, the `Error` type, and the slice codecs
//! `write_slice` and `read_slice`. This file declares those, with the same
//! signatures. The codecs have no bodies that do anything.
//!
//! Charon translates only what `top_k_u32_i64` reaches. `TopKFilter` reaches none
//! of these items, so none of these stubs is extracted. The `TopKFilter` code is
//! the real `heap.rs`, through the symbolic link in `src/`.

use std::io::{Read, Write};

/// A fixed-size number that `binary.Write` writes as little-endian bytes. Same
/// shape as the real trait.
pub trait Element: Copy {
    /// Size in bytes.
    const SIZE: usize;
    /// Appends the little-endian bytes.
    fn put(self, out: &mut Vec<u8>);
    /// Reads from exactly [`Self::SIZE`] little-endian bytes.
    fn get(bytes: &[u8]) -> Self;
}

macro_rules! element {
    ($($t:ty),*) => {$(
        impl Element for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn put(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn get(bytes: &[u8]) -> Self {
                let mut b = [0; std::mem::size_of::<$t>()];
                b.copy_from_slice(bytes);
                <$t>::from_le_bytes(b)
            }
        }
    )*};
}

element!(i32, f32);

/// Stand-in for the real `encoding::Error`: the I/O failure that `heap.rs` converts
/// with `?`, and nothing else.
#[derive(Debug)]
pub enum Error {
    /// An I/O error from the reader or writer.
    Io(std::io::Error),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Stand-in for the real `write_slice`, which is not reached by `TopKFilter`.
pub fn write_slice<T: Element, W: Write>(_w: &mut W, _xs: &[T]) -> Result<(), Error> {
    Ok(())
}

/// Stand-in for the real `read_slice`, which is not reached by `TopKFilter`.
pub fn read_slice<T: Element, R: Read>(_r: &mut R) -> Result<Vec<T>, Error> {
    Ok(Vec::new())
}
