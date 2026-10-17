// Ports: common/blas/blas.go @ add21b5

//! The CBLAS enumerations of Go's `blas` package.
//!
//! Go's `SGEMM` exists only in builds with cgo and the `mkl` or `openblas` tag,
//! or with cgo on macOS ARM64 (Accelerate). The release builds use
//! `CGO_ENABLED=0` and so never call it; this port has no BLAS backend
//! (`PORTING_NOTES.md` X-9).

/// `blas.Order`: the CBLAS matrix layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Order {
    /// `CblasRowMajor`.
    RowMajor = 101,
}

/// `blas.Transpose`: whether CBLAS transposes an operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Transpose {
    /// `CblasNoTrans`.
    NoTrans = 111,
    /// `CblasTrans`.
    Trans = 112,
}

impl Transpose {
    /// `blas.NewTranspose`.
    pub fn new(transpose: bool) -> Transpose {
        if transpose {
            Transpose::Trans
        } else {
            Transpose::NoTrans
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_match_cblas() {
        assert_eq!(Order::RowMajor as i32, 101);
        assert_eq!(Transpose::NoTrans as i32, 111);
        assert_eq!(Transpose::Trans as i32, 112);
        assert_eq!(Transpose::new(false), Transpose::NoTrans);
        assert_eq!(Transpose::new(true), Transpose::Trans);
    }
}
