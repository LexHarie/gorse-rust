// Ports: common/util/util.go, common/util/strconv.go @ add21b5

//! Small helpers shared by every Gorse package: slice and matrix makers,
//! panic recovery, id validation, digests, and the typed number parsers of
//! `strconv.go`.
//!
//! Go's `math/rand` and `strconv`, which these helpers build on and Gorse
//! needs bit for bit, are ported in [`rand`] and [`crate::gostrconv`].

pub mod rand;
pub mod random;
pub mod tls;

use std::panic::{self, UnwindSafe};

use md5::{Digest, Md5};

use crate::gostrconv;

pub use crate::gostrconv::{NumError, NumErrorKind};
pub use random::{RandomGenerator, new_rand};

/// `RangeInt(n)`: `[0, 1, ..., n-1]`.
pub fn range_int(n: usize) -> Vec<usize> {
    (0..n).collect()
}

/// `RepeatFloat32s(n, value)`: `value` repeated `n` times.
pub fn repeat_float32s(n: usize, value: f32) -> Vec<f32> {
    vec![value; n]
}

/// `NewMatrix32(row, col)`: a `row` x `col` matrix of zeros.
pub fn new_matrix32(row: usize, col: usize) -> Vec<Vec<f32>> {
    vec![vec![0.0; col]; row]
}

/// `NewTensor32(a, b, c)`: an `a` x `b` x `c` tensor of zeros.
pub fn new_tensor32(a: usize, b: usize, c: usize) -> Vec<Vec<Vec<f32>>> {
    vec![new_matrix32(b, c); a]
}

/// `NewMatrixInt(row, col)`: a `row` x `col` matrix of zeros.
pub fn new_matrix_int(row: usize, col: usize) -> Vec<Vec<i64>> {
    vec![vec![0; col]; row]
}

/// `defer CheckPanic()`: runs `f` and, if it panics, logs `panic recovered`
/// with the panic message at error level and returns `None` instead of
/// unwinding further.
pub fn check_panic<R>(f: impl FnOnce() -> R + UnwindSafe) -> Option<R> {
    match panic::catch_unwind(f) {
        Ok(r) => Some(r),
        Err(payload) => {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "(non-string panic payload)".to_owned()
            };
            tracing::error!(panic = %message, "panic recovered");
            None
        }
    }
}

/// Why [`validate_id`] rejected an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidId {
    /// The id is empty or only white space.
    #[error("id cannot be empty")]
    Empty,
    /// The id contains `/`.
    #[error("id cannot contain `/`")]
    ContainsSlash,
}

/// `ValidateId(text)`: a user or item id cannot be empty (after trimming
/// Unicode white space, as Go's `strings.TrimSpace`) and cannot contain `/`.
/// Go's comment also names `,`, but only `/` is checked.
pub fn validate_id(text: &str) -> Result<(), InvalidId> {
    let text = text.trim();
    if text.is_empty() {
        Err(InvalidId::Empty)
    } else if text.contains('/') {
        Err(InvalidId::ContainsSlash)
    } else {
        Ok(())
    }
}

/// `MD5(s...)`: the hex MD5 of the strings sorted byte-wise and
/// concatenated, so the order of the arguments does not matter. Go sorts the
/// caller's slice in place; no caller reads it afterwards, so a sorted copy
/// is hashed here.
pub fn md5<S: AsRef<str>>(s: &[S]) -> String {
    let mut sorted: Vec<&str> = s.iter().map(AsRef::as_ref).collect();
    sorted.sort_unstable();
    let mut hash = Md5::new();
    for str in sorted {
        hash.update(str.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// A float type for [`parse_float`] (`constraints.Float`).
pub trait Float: Copy {
    /// Bit size passed to `strconv.ParseFloat`.
    const BITS: i64;
    /// Converts the parsed value, which fits exactly.
    fn from_f64(v: f64) -> Self;
}

impl Float for f32 {
    const BITS: i64 = 32;
    fn from_f64(v: f64) -> Self {
        v as f32
    }
}

impl Float for f64 {
    const BITS: i64 = 64;
    fn from_f64(v: f64) -> Self {
        v
    }
}

/// An unsigned integer type for [`parse_uint`] (`constraints.Unsigned`).
pub trait Unsigned: Copy {
    /// Bit size passed to `strconv.ParseUint`.
    const BITS: i64;
    /// Converts the parsed value, which fits.
    fn from_u64(v: u64) -> Self;
}

/// A signed integer type for [`parse_int`] and [`format_int`]
/// (`constraints.Signed`).
pub trait Signed: Copy {
    /// Bit size passed to `strconv.ParseInt`.
    const BITS: i64;
    /// Converts the parsed value, which fits.
    fn from_i64(v: i64) -> Self;
    /// Widens to `i64`.
    fn to_i64(self) -> i64;
}

macro_rules! integer_types {
    ($trait:ident, $from:ident, $wide:ty: $($t:ty),*) => {$(
        impl $trait for $t {
            const BITS: i64 = <$t>::BITS as i64;
            fn $from(v: $wide) -> Self {
                v as $t
            }
        }
    )*};
}

integer_types!(Unsigned, from_u64, u64: u8, u16, u32, u64);

macro_rules! signed_types {
    ($($t:ty),*) => {$(
        impl Signed for $t {
            const BITS: i64 = <$t>::BITS as i64;
            fn from_i64(v: i64) -> Self {
                v as $t
            }
            fn to_i64(self) -> i64 {
                i64::from(self)
            }
        }
    )*};
}

signed_types!(i8, i16, i32, i64);

/// A Go `(value, error)` pair as a `Result`. Go also returns a value next to
/// an error (0, or the clamped value); every Gorse caller returns the error
/// without reading it, so it is dropped.
fn result<T>((value, err): (T, Option<NumError>)) -> Result<T, NumError> {
    err.map_or(Ok(value), Err)
}

/// `ParseFloat[T](s)`: `strconv.ParseFloat` with the bit size of `T`.
pub fn parse_float<T: Float>(s: impl AsRef<[u8]>) -> Result<T, NumError> {
    result(gostrconv::parse_float(s, T::BITS)).map(T::from_f64)
}

/// `ParseUInt[T](s)`: `strconv.ParseUint(s, 10, bits of T)`.
pub fn parse_uint<T: Unsigned>(s: impl AsRef<[u8]>) -> Result<T, NumError> {
    result(gostrconv::parse_uint(s, 10, T::BITS)).map(T::from_u64)
}

/// `ParseInt[T](s)`: `strconv.ParseInt(s, 10, bits of T)`.
pub fn parse_int<T: Signed>(s: impl AsRef<[u8]>) -> Result<T, NumError> {
    result(gostrconv::parse_int(s, 10, T::BITS)).map(T::from_i64)
}

/// `FormatInt[T](i)`: decimal digits with a leading `-` when negative.
pub fn format_int<T: Signed>(i: T) -> String {
    i.to_i64().to_string()
}

#[cfg(test)]
mod tests {
    // Ports: common/util/util_test.go @ add21b5
    use super::*;

    #[test]
    fn test_new_matrix32() {
        let a = new_matrix32(3, 4);
        assert_eq!(3, a.len());
        assert_eq!(4, a[0].len());
        assert_eq!(4, a[0].len());
        assert_eq!(4, a[0].len());
    }

    #[test]
    fn test_range_int() {
        let a = range_int(7);
        assert_eq!(7, a.len());
        for (i, v) in a.iter().enumerate() {
            assert_eq!(i, *v);
        }
    }

    #[test]
    fn test_repeat_float32s() {
        let a = repeat_float32s(3, 0.1);
        assert_eq!(vec![0.1f32, 0.1, 0.1], a);
    }

    #[test]
    fn test_new_matrix_int() {
        let m = new_matrix_int(4, 3);
        assert_eq!(4, m.len());
        for v in &m {
            assert_eq!(3, v.len());
        }
    }

    #[test]
    fn test_new_tensor32() {
        let a = new_tensor32(3, 4, 5);
        assert_eq!(3, a.len());
        assert_eq!(4, a[0].len());
        assert_eq!(5, a[0][0].len());
    }

    #[test]
    fn test_validate_id() {
        assert!(validate_id("").is_err());
        assert!(validate_id("/").is_err());
        assert!(validate_id("abc").is_ok());
    }

    #[test]
    fn test_md5() {
        let hash1 = md5(&["b", "a", "c"]);
        let hash2 = md5(&["c", "b", "a"]);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn check_panic_recovers() {
        assert_eq!(check_panic(|| 7), Some(7));
        assert_eq!(check_panic(|| -> i32 { panic!("boom") }), None);
        let owned = String::from("owned");
        assert_eq!(check_panic(move || -> i32 { panic!("{owned}") }), None);
    }
}
