// Ports: common/floats/floats_amd64.go, floats_avx.go, floats_avx512.go, floats_arm64.go, floats_neon.go @ add21b5

//! The C kernels of `common/floats/src` (compiled by `build.rs`) behind safe
//! wrappers. Go calls the same functions through assembly generated from
//! these sources.
//!
//! Each wrapper checks the slice lengths and the CPU's features itself, so it
//! is sound whatever its caller passes, and returns `false` (or `None`) when
//! no kernel applies so the caller runs the scalar loop.

// Calling the C kernels is what this module is for; every call states why it
// is sound.
#![allow(unsafe_code)]

#[cfg(target_arch = "x86_64")]
mod imp {
    use crate::floats::{Feature, check_mm};

    unsafe extern "C" {
        // floats_avx.c, compiled with -mavx -mf16c.
        fn _mm256_from_float32(a: *const f32, dst: *mut u16, n: i64);
        fn _mm256_to_float32(a: *const u16, dst: *mut f32, n: i64);
        fn _mm256_mul_const_add_to(
            a: *const f32,
            b: *const f32,
            c: *const f32,
            dst: *mut f32,
            n: i64,
        );
        fn _mm256_mul_const_add(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm256_mul_const_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm256_mul_const(a: *mut f32, b: *const f32, n: i64);
        fn _mm256_add_const(a: *mut f32, b: *const f32, n: i64);
        fn _mm256_sub_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm256_sub(a: *mut f32, b: *const f32, n: i64);
        fn _mm256_mul_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm256_div_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm256_sqrt_to(a: *const f32, b: *mut f32, n: i64);
        fn _mm256_dot(a: *const f32, b: *const f32, n: i64) -> f32;
        fn _mm256_euclidean(a: *const f32, b: *const f32, n: i64) -> f32;
        #[allow(clippy::too_many_arguments)]
        fn _mm256_mm(
            trans_a: bool,
            trans_b: bool,
            m: i64,
            n: i64,
            k: i64,
            a: *const f32,
            lda: i64,
            b: *const f32,
            ldb: i64,
            c: *mut f32,
            ldc: i64,
        );

        // floats_avx512.c, compiled with -mavx -mfma -mavx512f.
        fn _mm512_mul_const_add_to(
            a: *const f32,
            b: *const f32,
            c: *const f32,
            dst: *mut f32,
            n: i64,
        );
        fn _mm512_mul_const_add(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm512_mul_const_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm512_mul_const(a: *mut f32, b: *const f32, n: i64);
        fn _mm512_add_const(a: *mut f32, b: *const f32, n: i64);
        fn _mm512_sub_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm512_sub(a: *mut f32, b: *const f32, n: i64);
        fn _mm512_mul_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm512_div_to(a: *const f32, b: *const f32, c: *mut f32, n: i64);
        fn _mm512_sqrt_to(a: *const f32, b: *mut f32, n: i64);
        fn _mm512_dot(a: *const f32, b: *const f32, n: i64) -> f32;
        fn _mm512_euclidean(a: *const f32, b: *const f32, n: i64) -> f32;
        #[allow(clippy::too_many_arguments)]
        fn _mm512_mm(
            trans_a: bool,
            trans_b: bool,
            m: i64,
            n: i64,
            k: i64,
            a: *const f32,
            lda: i64,
            b: *const f32,
            ldb: i64,
            c: *mut f32,
            ldc: i64,
        );
    }

    /// The kernel family `floats_amd64.go` picks for these features.
    enum Level {
        Avx512,
        Avx,
    }

    fn level(f: Feature) -> Option<Level> {
        let f = f.usable();
        if f.contains(Feature::AVX512) {
            Some(Level::Avx512)
        } else if f.contains(Feature::AVX) {
            Some(Level::Avx)
        } else {
            None
        }
    }

    /// A length as the kernels' `int64_t`; slices never exceed `isize::MAX`.
    fn n(len: usize) -> i64 {
        i64::try_from(len).expect("floats: length fits in i64")
    }

    pub(in crate::floats) fn from_float32(f: Feature, a: &[f32], dst: &mut [u16]) -> bool {
        assert_eq!(a.len(), dst.len());
        if !f.usable().contains(Feature::AVX | Feature::F16C) {
            return false;
        }
        // SAFETY: the CPU has AVX and F16C, the features the kernel was
        // compiled for; both slices hold `a.len()` elements.
        unsafe { _mm256_from_float32(a.as_ptr(), dst.as_mut_ptr(), n(a.len())) };
        true
    }

    pub(in crate::floats) fn to_float32(f: Feature, a: &[u16], dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        if !f.usable().contains(Feature::AVX | Feature::F16C) {
            return false;
        }
        // SAFETY: the CPU has AVX and F16C; both slices hold `a.len()`
        // elements.
        unsafe { _mm256_to_float32(a.as_ptr(), dst.as_mut_ptr(), n(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_const_add_to(
        f: Feature,
        a: &[f32],
        b: f32,
        c: &[f32],
        dst: &mut [f32],
    ) -> bool {
        assert!(a.len() == c.len() && a.len() == dst.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features (`level`); `a`, `c`
            // and `dst` hold `len` elements and `b` is one value.
            Some(Level::Avx512) => unsafe {
                _mm512_mul_const_add_to(a.as_ptr(), &b, c.as_ptr(), dst.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_mul_const_add_to(a.as_ptr(), &b, c.as_ptr(), dst.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn mul_const_add(f: Feature, a: &[f32], b: f32, dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; `a` and `dst` hold
            // `len` elements and `b` is one value.
            Some(Level::Avx512) => unsafe {
                _mm512_mul_const_add(a.as_ptr(), &b, dst.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_mul_const_add(a.as_ptr(), &b, dst.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn mul_const_to(f: Feature, a: &[f32], b: f32, dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; `a` and `dst` hold
            // `len` elements and `b` is one value.
            Some(Level::Avx512) => unsafe {
                _mm512_mul_const_to(a.as_ptr(), &b, dst.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_mul_const_to(a.as_ptr(), &b, dst.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn mul_const(f: Feature, a: &mut [f32], b: f32) -> bool {
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; `a` holds `len`
            // elements and `b` is one value.
            Some(Level::Avx512) => unsafe { _mm512_mul_const(a.as_mut_ptr(), &b, len) },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe { _mm256_mul_const(a.as_mut_ptr(), &b, len) },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn add_const(f: Feature, a: &mut [f32], b: f32) -> bool {
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; `a` holds `len`
            // elements and `b` is one value.
            Some(Level::Avx512) => unsafe { _mm512_add_const(a.as_mut_ptr(), &b, len) },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe { _mm256_add_const(a.as_mut_ptr(), &b, len) },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn sub(f: Feature, a: &mut [f32], b: &[f32]) -> bool {
        assert_eq!(a.len(), b.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; both slices hold
            // `len` elements.
            Some(Level::Avx512) => unsafe { _mm512_sub(a.as_mut_ptr(), b.as_ptr(), len) },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe { _mm256_sub(a.as_mut_ptr(), b.as_ptr(), len) },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn sub_to(f: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; the three slices
            // hold `len` elements.
            Some(Level::Avx512) => unsafe {
                _mm512_sub_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_sub_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn mul_to(f: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; the three slices
            // hold `len` elements.
            Some(Level::Avx512) => unsafe {
                _mm512_mul_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_mul_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn div_to(f: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; the three slices
            // hold `len` elements.
            Some(Level::Avx512) => unsafe {
                _mm512_div_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe {
                _mm256_div_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), len)
            },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn sqrt_to(f: Feature, a: &[f32], b: &mut [f32]) -> bool {
        assert_eq!(a.len(), b.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; both slices hold
            // `len` elements.
            Some(Level::Avx512) => unsafe { _mm512_sqrt_to(a.as_ptr(), b.as_mut_ptr(), len) },
            // SAFETY: as above.
            Some(Level::Avx) => unsafe { _mm256_sqrt_to(a.as_ptr(), b.as_mut_ptr(), len) },
            None => return false,
        }
        true
    }

    pub(in crate::floats) fn dot(f: Feature, a: &[f32], b: &[f32]) -> Option<f32> {
        assert_eq!(a.len(), b.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; both slices hold
            // `len` elements.
            Some(Level::Avx512) => Some(unsafe { _mm512_dot(a.as_ptr(), b.as_ptr(), len) }),
            // SAFETY: as above.
            Some(Level::Avx) => Some(unsafe { _mm256_dot(a.as_ptr(), b.as_ptr(), len) }),
            None => None,
        }
    }

    pub(in crate::floats) fn euclidean(f: Feature, a: &[f32], b: &[f32]) -> Option<f32> {
        assert_eq!(a.len(), b.len());
        let len = n(a.len());
        match level(f) {
            // SAFETY: the CPU has the kernel's features; both slices hold
            // `len` elements.
            Some(Level::Avx512) => Some(unsafe { _mm512_euclidean(a.as_ptr(), b.as_ptr(), len) }),
            // SAFETY: as above.
            Some(Level::Avx) => Some(unsafe { _mm256_euclidean(a.as_ptr(), b.as_ptr(), len) }),
            None => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::floats) fn mm(
        f: Feature,
        trans_a: bool,
        trans_b: bool,
        m: usize,
        nn: usize,
        k: usize,
        a: &[f32],
        lda: usize,
        b: &[f32],
        ldb: usize,
        c: &mut [f32],
        ldc: usize,
    ) -> bool {
        let Some(level) = level(f) else {
            return false;
        };
        check_mm(
            trans_a,
            trans_b,
            m,
            nn,
            k,
            a.len(),
            lda,
            b.len(),
            ldb,
            c.len(),
            ldc,
        );
        let kernel = match level {
            Level::Avx512 => _mm512_mm,
            Level::Avx => _mm256_mm,
        };
        // SAFETY: the CPU has the kernel's features, and `check_mm` checked
        // that every element the kernel reads or writes is inside its slice
        // (with no rows or columns it touches nothing).
        unsafe {
            kernel(
                trans_a,
                trans_b,
                n(m),
                n(nn),
                n(k),
                a.as_ptr(),
                n(lda),
                b.as_ptr(),
                n(ldb),
                c.as_mut_ptr(),
                n(ldc),
            );
        }
        true
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    use crate::floats::{Feature, check_mm};
    use std::ffi::c_long;

    unsafe extern "C" {
        // floats_neon.c, compiled with -O3.
        fn vfrom_float32(a: *const f32, dst: *mut u16, n: i64);
        fn vto_float32(a: *const u16, dst: *mut f32, n: i64);
        fn vmul_const_add_to(a: *const f32, b: *const f32, c: *const f32, dst: *mut f32, n: c_long);
        fn vmul_const_add(a: *const f32, b: *const f32, c: *mut f32, n: c_long);
        fn vmul_const_to(a: *const f32, b: *const f32, c: *mut f32, n: c_long);
        fn vmul_const(a: *mut f32, b: *const f32, n: c_long);
        fn vadd_const(a: *mut f32, b: *const f32, n: c_long);
        fn vsub_to(a: *const f32, b: *const f32, c: *mut f32, n: c_long);
        fn vsub(a: *mut f32, b: *const f32, n: c_long);
        fn vmul_to(a: *const f32, b: *const f32, c: *mut f32, n: c_long);
        fn vdiv_to(a: *const f32, b: *const f32, c: *mut f32, n: c_long);
        fn vsqrt_to(a: *const f32, b: *mut f32, n: c_long);
        fn vdot(a: *const f32, b: *const f32, n: c_long) -> f32;
        fn veuclidean(a: *const f32, b: *const f32, n: c_long) -> f32;
        #[allow(clippy::too_many_arguments)]
        fn vmm(
            trans_a: bool,
            trans_b: bool,
            m: c_long,
            n: c_long,
            k: c_long,
            a: *const f32,
            lda: c_long,
            b: *const f32,
            ldb: c_long,
            c: *mut f32,
            ldc: c_long,
        );
    }

    fn l(len: usize) -> c_long {
        c_long::try_from(len).expect("floats: length fits in a C long")
    }

    fn n64(len: usize) -> i64 {
        i64::try_from(len).expect("floats: length fits in i64")
    }

    // NEON is part of every AArch64 CPU, so every kernel always applies; that
    // is the CPU feature half of each SAFETY argument below.

    pub(in crate::floats) fn from_float32(_: Feature, a: &[f32], dst: &mut [u16]) -> bool {
        assert_eq!(a.len(), dst.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        unsafe { vfrom_float32(a.as_ptr(), dst.as_mut_ptr(), n64(a.len())) };
        true
    }

    pub(in crate::floats) fn to_float32(_: Feature, a: &[u16], dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        unsafe { vto_float32(a.as_ptr(), dst.as_mut_ptr(), n64(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_const_add_to(
        _: Feature,
        a: &[f32],
        b: f32,
        c: &[f32],
        dst: &mut [f32],
    ) -> bool {
        assert!(a.len() == c.len() && a.len() == dst.len());
        // SAFETY: NEON is present; `a`, `c` and `dst` hold `a.len()` elements
        // and `b` is one value.
        unsafe { vmul_const_add_to(a.as_ptr(), &b, c.as_ptr(), dst.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_const_add(_: Feature, a: &[f32], b: f32, dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        // SAFETY: NEON is present; `a` and `dst` hold `a.len()` elements and
        // `b` is one value.
        unsafe { vmul_const_add(a.as_ptr(), &b, dst.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_const_to(_: Feature, a: &[f32], b: f32, dst: &mut [f32]) -> bool {
        assert_eq!(a.len(), dst.len());
        // SAFETY: NEON is present; `a` and `dst` hold `a.len()` elements and
        // `b` is one value.
        unsafe { vmul_const_to(a.as_ptr(), &b, dst.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_const(_: Feature, a: &mut [f32], b: f32) -> bool {
        // SAFETY: NEON is present; `a` holds `a.len()` elements and `b` is one
        // value.
        unsafe { vmul_const(a.as_mut_ptr(), &b, l(a.len())) };
        true
    }

    pub(in crate::floats) fn add_const(_: Feature, a: &mut [f32], b: f32) -> bool {
        // SAFETY: NEON is present; `a` holds `a.len()` elements and `b` is one
        // value.
        unsafe { vadd_const(a.as_mut_ptr(), &b, l(a.len())) };
        true
    }

    pub(in crate::floats) fn sub(_: Feature, a: &mut [f32], b: &[f32]) -> bool {
        assert_eq!(a.len(), b.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        unsafe { vsub(a.as_mut_ptr(), b.as_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn sub_to(_: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        // SAFETY: NEON is present; the three slices hold `a.len()` elements.
        unsafe { vsub_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn mul_to(_: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        // SAFETY: NEON is present; the three slices hold `a.len()` elements.
        unsafe { vmul_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn div_to(_: Feature, a: &[f32], b: &[f32], c: &mut [f32]) -> bool {
        assert!(a.len() == b.len() && a.len() == c.len());
        // SAFETY: NEON is present; the three slices hold `a.len()` elements.
        unsafe { vdiv_to(a.as_ptr(), b.as_ptr(), c.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn sqrt_to(_: Feature, a: &[f32], b: &mut [f32]) -> bool {
        assert_eq!(a.len(), b.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        unsafe { vsqrt_to(a.as_ptr(), b.as_mut_ptr(), l(a.len())) };
        true
    }

    pub(in crate::floats) fn dot(_: Feature, a: &[f32], b: &[f32]) -> Option<f32> {
        assert_eq!(a.len(), b.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        Some(unsafe { vdot(a.as_ptr(), b.as_ptr(), l(a.len())) })
    }

    pub(in crate::floats) fn euclidean(_: Feature, a: &[f32], b: &[f32]) -> Option<f32> {
        assert_eq!(a.len(), b.len());
        // SAFETY: NEON is present; both slices hold `a.len()` elements.
        Some(unsafe { veuclidean(a.as_ptr(), b.as_ptr(), l(a.len())) })
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::floats) fn mm(
        _: Feature,
        trans_a: bool,
        trans_b: bool,
        m: usize,
        n: usize,
        k: usize,
        a: &[f32],
        lda: usize,
        b: &[f32],
        ldb: usize,
        c: &mut [f32],
        ldc: usize,
    ) -> bool {
        check_mm(
            trans_a,
            trans_b,
            m,
            n,
            k,
            a.len(),
            lda,
            b.len(),
            ldb,
            c.len(),
            ldc,
        );
        // SAFETY: NEON is present, and `check_mm` checked that every element
        // the kernel reads or writes is inside its slice.
        unsafe {
            vmm(
                trans_a,
                trans_b,
                l(m),
                l(n),
                l(k),
                a.as_ptr(),
                l(lda),
                b.as_ptr(),
                l(ldb),
                c.as_mut_ptr(),
                l(ldc),
            );
        }
        true
    }
}

/// Other architectures: no kernels (`PORTING_NOTES.md` X-9).
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod imp {
    use crate::floats::Feature;

    pub(in crate::floats) fn from_float32(_: Feature, _: &[f32], _: &mut [u16]) -> bool {
        false
    }
    pub(in crate::floats) fn to_float32(_: Feature, _: &[u16], _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn mul_const_add_to(
        _: Feature,
        _: &[f32],
        _: f32,
        _: &[f32],
        _: &mut [f32],
    ) -> bool {
        false
    }
    pub(in crate::floats) fn mul_const_add(_: Feature, _: &[f32], _: f32, _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn mul_const_to(_: Feature, _: &[f32], _: f32, _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn mul_const(_: Feature, _: &mut [f32], _: f32) -> bool {
        false
    }
    pub(in crate::floats) fn add_const(_: Feature, _: &mut [f32], _: f32) -> bool {
        false
    }
    pub(in crate::floats) fn sub(_: Feature, _: &mut [f32], _: &[f32]) -> bool {
        false
    }
    pub(in crate::floats) fn sub_to(_: Feature, _: &[f32], _: &[f32], _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn mul_to(_: Feature, _: &[f32], _: &[f32], _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn div_to(_: Feature, _: &[f32], _: &[f32], _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn sqrt_to(_: Feature, _: &[f32], _: &mut [f32]) -> bool {
        false
    }
    pub(in crate::floats) fn dot(_: Feature, _: &[f32], _: &[f32]) -> Option<f32> {
        None
    }
    pub(in crate::floats) fn euclidean(_: Feature, _: &[f32], _: &[f32]) -> Option<f32> {
        None
    }
    #[allow(clippy::too_many_arguments)]
    pub(in crate::floats) fn mm(
        _: Feature,
        _: bool,
        _: bool,
        _: usize,
        _: usize,
        _: usize,
        _: &[f32],
        _: usize,
        _: &[f32],
        _: usize,
        _: &mut [f32],
        _: usize,
    ) -> bool {
        false
    }
}

pub(super) use imp::*;
