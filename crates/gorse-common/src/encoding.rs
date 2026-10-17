// Ports: common/encoding/encoding.go @ add21b5

//! The binary primitives model blobs are made of (`contracts/model-blobs.md`):
//! length-prefixed byte strings, little-endian slices of fixed-size numbers,
//! and gob values wrapped in a length prefix.
//!
//! Lengths are `i32` little-endian, as in Go. Where Go would panic or build a
//! corrupt stream (a negative length read back, a length above `i32::MAX`
//! written), these functions return an error (`PORTING_NOTES.md` X-7).

pub mod gob;

use std::io::{self, Read, Write};

/// An error from the encoding functions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying reader or writer failed, or the input ended early.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A length prefix read from the input is negative.
    #[error("negative length {0}")]
    NegativeLength(i32),
    /// A slice or string is too long for an `i32` length prefix.
    #[error("length {0} does not fit in an int32")]
    TooLong(usize),
    /// A string read from the input is not UTF-8.
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    /// The gob value could not be encoded or decoded.
    #[error(transparent)]
    Gob(#[from] gob::Error),
}

/// A fixed-size number that `binary.Write` writes as little-endian bytes.
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

element!(i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);

fn write_len(w: &mut impl Write, len: usize) -> Result<(), Error> {
    let len = i32::try_from(len).map_err(|_| Error::TooLong(len))?;
    w.write_all(&len.to_le_bytes())?;
    Ok(())
}

fn read_len(r: &mut impl Read) -> Result<usize, Error> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    let len = i32::from_le_bytes(b);
    usize::try_from(len).map_err(|_| Error::NegativeLength(len))
}

/// Reads exactly `n` bytes, growing the buffer as data arrives so that a
/// corrupt length cannot allocate more than the input holds.
fn read_exact_vec(r: &mut impl Read, n: usize) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    r.take(n as u64).read_to_end(&mut data)?;
    if data.len() < n {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
    }
    Ok(data)
}

/// `WriteSlice`: the element count, then the elements.
///
/// # Errors
///
/// Fails when the writer fails or the slice has more than `i32::MAX` elements.
pub fn write_slice<T: Element>(w: &mut impl Write, s: &[T]) -> Result<(), Error> {
    write_len(w, s.len())?;
    let mut data = Vec::with_capacity(s.len() * T::SIZE);
    for &x in s {
        x.put(&mut data);
    }
    w.write_all(&data)?;
    Ok(())
}

/// `ReadSlice`.
///
/// # Errors
///
/// Fails when the input ends early or the count is negative.
pub fn read_slice<T: Element>(r: &mut impl Read) -> Result<Vec<T>, Error> {
    let n = read_len(r)?;
    let bytes = n.checked_mul(T::SIZE).ok_or(Error::TooLong(n))?;
    let data = read_exact_vec(r, bytes)?;
    Ok(data.chunks_exact(T::SIZE).map(T::get).collect())
}

/// `WriteString`: the UTF-8 bytes with [`write_bytes`].
///
/// # Errors
///
/// As [`write_bytes`].
pub fn write_string(w: &mut impl Write, s: &str) -> Result<(), Error> {
    write_bytes(w, s.as_bytes())
}

/// `ReadString`.
///
/// # Errors
///
/// As [`read_bytes`], and when the bytes are not UTF-8.
pub fn read_string(r: &mut impl Read) -> Result<String, Error> {
    String::from_utf8(read_bytes(r)?).map_err(|_| Error::InvalidUtf8)
}

/// `WriteBytes`: the length, then the bytes.
///
/// # Errors
///
/// Fails when the writer fails or there are more than `i32::MAX` bytes.
pub fn write_bytes(w: &mut impl Write, s: &[u8]) -> Result<(), Error> {
    write_len(w, s.len())?;
    w.write_all(s)?;
    Ok(())
}

/// `ReadBytes`. A zero length reads nothing more and returns no bytes; Go's
/// result then depends on the reader (`PORTING_NOTES.md` Q-19).
///
/// # Errors
///
/// Fails when the input ends early or the length is negative.
pub fn read_bytes(r: &mut impl Read) -> Result<Vec<u8>, Error> {
    let n = read_len(r)?;
    read_exact_vec(r, n)
}

/// `WriteGob`: a complete gob stream of `v` (from a fresh encoder) with
/// [`write_bytes`].
///
/// # Errors
///
/// Fails when encoding or writing fails.
pub fn write_gob<T: gob::GobValue>(w: &mut impl Write, v: &T) -> Result<(), Error> {
    write_bytes(w, &gob::encode(v)?)
}

/// `ReadGob`.
///
/// # Errors
///
/// Fails when reading fails or the bytes are not a gob stream of a `T`.
pub fn read_gob<T: gob::GobValue>(r: &mut impl Read) -> Result<T, Error> {
    Ok(gob::decode(&read_bytes(r)?)?)
}

#[cfg(test)]
mod tests {
    // Ports: common/encoding/encoding_test.go @ add21b5
    use super::*;

    #[test]
    fn test_read_write_slice() {
        let a: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0];
        let mut buf = Vec::new();
        write_slice(&mut buf, &a).expect("write");
        let b: Vec<f32> = read_slice(&mut buf.as_slice()).expect("read");
        assert_eq!(a, b);
    }

    #[test]
    fn test_write_string() {
        let a = "abc";
        let mut buf = Vec::new();
        write_string(&mut buf, a).expect("write");
        let b = read_string(&mut buf.as_slice()).expect("read");
        assert_eq!(a, b);
    }

    #[test]
    fn test_write_gob() {
        let a = "abc".to_owned();
        let mut buf = Vec::new();
        write_gob(&mut buf, &a).expect("write");
        let b: String = read_gob(&mut buf.as_slice()).expect("read");
        assert_eq!(a, b);
    }

    #[test]
    fn negative_lengths_are_errors() {
        let data = (-1_i32).to_le_bytes();
        assert!(matches!(
            read_bytes(&mut data.as_slice()),
            Err(Error::NegativeLength(-1))
        ));
        assert!(matches!(
            read_slice::<f32>(&mut data.as_slice()),
            Err(Error::NegativeLength(-1))
        ));
    }

    #[test]
    fn short_input_is_an_error() {
        let mut data = 3_i32.to_le_bytes().to_vec();
        data.extend_from_slice(b"ab");
        assert!(read_bytes(&mut data.as_slice()).is_err());
        // A huge length with little data fails without allocating it.
        let data = i32::MAX.to_le_bytes();
        assert!(read_slice::<f64>(&mut data.as_slice()).is_err());
    }

    #[test]
    fn zero_length_reads_nothing() {
        let data = 0_i32.to_le_bytes();
        assert_eq!(
            read_bytes(&mut data.as_slice()).expect("read"),
            Vec::<u8>::new()
        );
    }
}
