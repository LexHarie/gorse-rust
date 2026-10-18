// Ports: common/floats/floats_test.go, floats_amd64_test.go, floats_arm64_test.go @ add21b5

use super::*;

fn ldexp(x: f32, e: i32) -> f32 {
    x * 2.0_f32.powi(e)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn float16_conversion() {
    let values = [
        0.0,
        -0.0,
        1.0,
        -2.0,
        ldexp(1.0, -24),
        ldexp(1.0, -25),
        1.0 + ldexp(1.0, -11),
        1.0 + ldexp(1.0, -10),
        65504.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let encoded: [u16; 11] = [
        0x0000, 0x8000, 0x3c00, 0xc000, 0x0001, 0x0000, 0x3c00, 0x3c01, 0x7bff, 0x7c00, 0xfc00,
    ];
    assert_eq!(from_float32(&values), encoded);
    let decoded = [
        0.0,
        -0.0,
        1.0,
        -2.0,
        ldexp(1.0, -24),
        0.0,
        1.0,
        1.0 + ldexp(1.0, -10),
        65504.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    assert_eq!(bits(&to_float32(&encoded)), bits(&decoded));
    assert!(from_float32(&[]).is_empty());
    assert!(to_float32(&[]).is_empty());
}

#[test]
fn mat_zero_fills_zeros() {
    let mut a = vec![
        vec![3.0, 2.0, 5.0, 6.0, 0.0, 0.0],
        vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
    ];
    mat_zero(&mut a);
    assert_eq!(a, vec![vec![0.0; 6], vec![0.0; 6]]);
}

#[test]
fn zero_fills_zeros() {
    let mut a = [3.0, 2.0, 5.0, 6.0, 0.0, 0.0];
    zero(&mut a);
    assert_eq!(a, [0.0; 6]);
}

#[test]
fn add_adds() {
    let mut a = [1.0, 2.0, 3.0, 4.0];
    add(&mut a, &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(a, [6.0, 8.0, 10.0, 12.0]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn add_panics_on_lengths() {
    add(&mut [1.0], &[]);
}

#[test]
fn sub_subtracts() {
    let mut a = [1.0, 2.0, 3.0, 4.0];
    sub(&mut a, &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(a, [-4.0; 4]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn sub_panics_on_lengths() {
    sub(&mut [1.0], &[]);
}

#[test]
fn sub_to_subtracts() {
    let mut c = [0.0; 4];
    sub_to(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0], &mut c);
    assert_eq!(c, [-4.0; 4]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn sub_to_panics_on_lengths() {
    sub_to(&[1.0], &[], &mut []);
}

#[test]
fn mul_to_multiplies() {
    let mut c = [0.0; 4];
    mul_to(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0], &mut c);
    assert_eq!(c, [5.0, 12.0, 21.0, 32.0]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn mul_to_panics_on_lengths() {
    mul_to(&[1.0], &[], &mut []);
}

#[test]
fn mul_const_multiplies() {
    let mut a = [1.0, 2.0, 3.0, 4.0];
    mul_const(&mut a, 2.0);
    assert_eq!(a, [2.0, 4.0, 6.0, 8.0]);
}

#[test]
fn div_divides() {
    let mut a = [1.0, 4.0, 9.0, 16.0];
    div(&mut a, &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(a, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn div_panics_on_lengths() {
    div(&mut [1.0], &[]);
}

const A11: [f32; 11] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
const B11: [f32; 11] = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0];
const C11: [f32; 11] = [0.0, 3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0];

#[test]
fn mul_const_to_multiplies() {
    let mut dst = [0.0; 11];
    mul_const_to(&A11, 2.0, &mut dst);
    assert_eq!(dst, B11);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn mul_const_to_panics_on_lengths() {
    mul_const_to(&[], 2.0, &mut [0.0; 11]);
}

#[test]
fn mul_const_add_accumulates() {
    let mut dst = A11;
    mul_const_add(&A11, 2.0, &mut dst);
    assert_eq!(dst, C11);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn mul_const_add_panics_on_lengths() {
    mul_const_add(&[], 1.0, &mut A11.clone());
}

#[test]
fn mul_const_add_to_computes() {
    let mut dst = [0.0; 11];
    mul_const_add_to(&A11, 3.0, &B11, &mut dst);
    assert_eq!(
        dst,
        [
            0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0
        ]
    );
}

#[test]
fn mul_add_to_accumulates() {
    let mut c = C11;
    mul_add_to(&A11, &B11, &mut c);
    assert_eq!(
        c,
        [
            0.0, 5.0, 14.0, 27.0, 44.0, 65.0, 90.0, 119.0, 152.0, 189.0, 230.0
        ]
    );
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn mul_add_to_panics_on_lengths() {
    mul_add_to(&[], &[], &mut C11.clone());
}

#[test]
fn add_to_adds() {
    let mut dst = [0.0; 11];
    add_to(&A11, &B11, &mut dst);
    assert_eq!(dst, C11);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn add_to_panics_on_lengths() {
    add_to(&[], &[], &mut [0.0; 11]);
}

#[test]
fn add_const_adds() {
    let mut a = [1.0, 2.0, 3.0, 4.0];
    add_const(&mut a, 2.0);
    assert_eq!(a, [3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn div_to_divides() {
    let mut c = [0.0; 4];
    div_to(&[1.0, 4.0, 9.0, 16.0], &[1.0, 2.0, 3.0, 4.0], &mut c);
    assert_eq!(c, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn div_to_panics_on_lengths() {
    div_to(&[1.0], &[], &mut []);
}

#[test]
fn sqrt_to_takes_roots() {
    let mut b = [0.0; 4];
    sqrt_to(&[1.0, 4.0, 9.0, 16.0], &mut b);
    assert_eq!(b, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn sqrt_to_panics_on_lengths() {
    sqrt_to(&[1.0], &mut []);
}

#[test]
fn sqrt_takes_roots() {
    let mut a = [1.0, 4.0, 9.0, 16.0];
    sqrt(&mut a);
    assert_eq!(a, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn dot_multiplies() {
    assert_eq!(dot(&A11, &B11), 770.0);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn dot_panics_on_lengths() {
    dot(&[1.0], &[]);
}

#[test]
fn euclidean_measures() {
    assert_eq!(euclidean(&A11, &B11), 19.621416);
}

#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn euclidean_panics_on_lengths() {
    euclidean(&[1.0], &[]);
}

// NativeTestSuite: the scalar loops.

#[test]
fn native_loops() {
    assert_eq!(native::dot(&A11, &B11), 770.0);
    assert!((native::euclidean(&A11, &B11) - 19.621416).abs() <= 1e-6);

    let mut dst = [0.0; 11];
    native::mul_const_add_to(&A11, 3.0, &B11, &mut dst);
    assert_eq!(
        dst,
        [
            0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0
        ]
    );

    let mut dst = A11;
    native::mul_const_add(&A11, 2.0, &mut dst);
    assert_eq!(dst, C11);

    let mut dst = [0.0; 11];
    native::mul_const_to(&A11, 2.0, &mut dst);
    assert_eq!(dst, B11);

    let mut a = [1.0, 2.0, 3.0, 4.0];
    native::add_const(&mut a, 2.0);
    assert_eq!(a, [3.0, 4.0, 5.0, 6.0]);

    let mut a = [1.0, 2.0, 3.0, 4.0];
    native::sub(&mut a, &[5.0, 6.0, 7.0, 8.0]);
    assert_eq!(a, [-4.0; 4]);

    let mut c = [0.0; 4];
    native::sub_to(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0], &mut c);
    assert_eq!(c, [-4.0; 4]);

    let mut c = [0.0; 4];
    native::mul_to(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0], &mut c);
    assert_eq!(c, [5.0, 12.0, 21.0, 32.0]);

    let mut c = [0.0; 4];
    native::div_to(&[1.0, 4.0, 9.0, 16.0], &[1.0, 2.0, 3.0, 4.0], &mut c);
    assert_eq!(c, [1.0, 2.0, 3.0, 4.0]);

    let mut b = [0.0; 4];
    native::sqrt_to(&[1.0, 4.0, 9.0, 16.0], &mut b);
    assert_eq!(b, [1.0, 2.0, 3.0, 4.0]);

    let mut a = [1.0, 2.0, 3.0, 4.0];
    native::mul_const(&mut a, 2.0);
    assert_eq!(a, [2.0, 4.0, 6.0, 8.0]);
}

/// `TestMM`: the four products of a 2×3 `a` and a 3×4 `b`, with `mm` called
/// as `mm(trans_a, trans_b, a, lda, b, ldb, c)` for m=2, n=4, k=3, ldc=4.
fn check_mm_targets(mm: impl Fn(bool, bool, &[f32], usize, &[f32], usize, &mut [f32])) {
    let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let b = [
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
    ];
    let cases: [(bool, bool, usize, usize, [f32; 8]); 4] = [
        (
            false,
            false,
            3,
            4,
            [38.0, 44.0, 50.0, 56.0, 83.0, 98.0, 113.0, 128.0],
        ),
        (
            false,
            true,
            3,
            3,
            [14.0, 32.0, 50.0, 68.0, 32.0, 77.0, 122.0, 167.0],
        ),
        (
            true,
            false,
            2,
            4,
            [61.0, 70.0, 79.0, 88.0, 76.0, 88.0, 100.0, 112.0],
        ),
        (
            true,
            true,
            2,
            3,
            [22.0, 49.0, 76.0, 103.0, 28.0, 64.0, 100.0, 136.0],
        ),
    ];
    for (trans_a, trans_b, lda, ldb, target) in cases {
        let mut c = [0.0; 8];
        mm(trans_a, trans_b, &a, lda, &b, ldb, &mut c);
        assert_eq!(c, target, "trans_a={trans_a} trans_b={trans_b}");
    }
}

#[test]
fn native_mm() {
    check_mm_targets(|trans_a, trans_b, a, lda, b, ldb, c| {
        native::mm(
            Feature::NONE,
            trans_a,
            trans_b,
            2,
            4,
            3,
            a,
            lda,
            b,
            ldb,
            c,
            4,
        );
    });
}

#[test]
fn package_mm() {
    check_mm_targets(|trans_a, trans_b, a, lda, b, ldb, c| {
        mm(trans_a, trans_b, 2, 4, 3, a, lda, b, ldb, c, 4);
    });
}

/// SIMDTestSuite: each SIMD level against the scalar loops.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod simd_suite {
    use super::*;

    const A20: [f32; 20] = [
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        17.0, 18.0, 19.0, 20.0,
    ];
    const B20: [f32; 20] = [
        10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 140.0,
        150.0, 160.0, 170.0, 180.0, 190.0, 200.0,
    ];
    const SQUARES20: [f32; 20] = [
        1.0, 4.0, 9.0, 16.0, 25.0, 36.0, 49.0, 64.0, 81.0, 100.0, 121.0, 144.0, 169.0, 196.0,
        225.0, 256.0, 289.0, 324.0, 361.0, 400.0,
    ];

    /// Runs the suite at `f`, or reports that the CPU lacks it (Go's `Skipf`).
    fn simd_suite(f: Feature) {
        if Feature::detect(Some("")) & f != f {
            eprintln!("skipped: {f} is not supported by this CPU");
            return;
        }

        let a = [
            0.0,
            -0.0,
            1.0,
            -2.0,
            ldexp(1.0, -24),
            ldexp(1.0, -25),
            1.0 + ldexp(1.0, -11),
            1.0 + ldexp(1.0, -10),
            65504.0,
            0.1,
            -0.2,
            1.5,
            2.5,
            3.5,
            4.5,
            5.5,
            6.5,
        ];
        let (mut expected, mut actual) = (vec![0; a.len()], vec![0; a.len()]);
        native::from_float32(&a, &mut expected);
        f.from_float32(&a, &mut actual);
        assert_eq!(expected, actual, "{f}: from_float32");

        let h: [u16; 17] = [
            0x0000, 0x8000, 0x0001, 0x03ff, 0x0400, 0x3c00, 0xc000, 0x2e66, 0xb266, 0x7bff, 0x7c00,
            0xfc00, 0x3e00, 0x4100, 0x4300, 0x4480, 0x4580,
        ];
        let (mut expected, mut actual) = (vec![0.0; h.len()], vec![0.0; h.len()]);
        native::to_float32(&h, &mut expected);
        f.to_float32(&h, &mut actual);
        assert_eq!(bits(&expected), bits(&actual), "{f}: to_float32");

        let (mut expected, mut actual) = ([0.0; 20], [0.0; 20]);
        f.mul_const_add_to(&A20, 2.0, &B20, &mut actual);
        native::mul_const_add_to(&A20, 2.0, &B20, &mut expected);
        assert_eq!(expected, actual, "{f}: mul_const_add_to");

        let (mut expected, mut actual) = (B20, B20);
        f.mul_const_add(&A20, 2.0, &mut actual);
        native::mul_const_add(&A20, 2.0, &mut expected);
        assert_eq!(expected, actual, "{f}: mul_const_add");

        let (mut expected, mut actual) = (B20, B20);
        f.mul_const_to(&A20, 2.0, &mut actual);
        native::mul_const_to(&A20, 2.0, &mut expected);
        assert_eq!(expected, actual, "{f}: mul_const_to");

        let (mut expected, mut actual) = (A20, A20);
        f.add_const(&mut actual, 2.0);
        native::add_const(&mut expected, 2.0);
        assert_eq!(expected, actual, "{f}: add_const");

        let (mut expected, mut actual) = (A20, A20);
        f.sub(&mut actual, &B20);
        native::sub(&mut expected, &B20);
        assert_eq!(expected, actual, "{f}: sub");

        let (mut expected, mut actual) = ([0.0; 20], [0.0; 20]);
        f.sub_to(&A20, &B20, &mut actual);
        native::sub_to(&A20, &B20, &mut expected);
        assert_eq!(expected, actual, "{f}: sub_to");

        let (mut expected, mut actual) = ([0.0; 20], [0.0; 20]);
        f.mul_to(&A20, &B20, &mut actual);
        native::mul_to(&A20, &B20, &mut expected);
        assert_eq!(expected, actual, "{f}: mul_to");

        let (mut expected, mut actual) = (B20, B20);
        f.mul_const(&mut actual, 2.0);
        native::mul_const(&mut expected, 2.0);
        assert_eq!(expected, actual, "{f}: mul_const");

        let (mut expected, mut actual) = ([0.0; 20], [0.0; 20]);
        f.div_to(&SQUARES20, &A20, &mut actual);
        native::div_to(&SQUARES20, &A20, &mut expected);
        assert_eq!(expected, actual, "{f}: div_to");

        let (mut expected, mut actual) = ([0.0; 20], [0.0; 20]);
        f.sqrt_to(&SQUARES20, &mut actual);
        native::sqrt_to(&SQUARES20, &mut expected);
        assert_eq!(expected, actual, "{f}: sqrt_to");

        assert_eq!(native::dot(&A20, &B20), f.dot(&A20, &B20), "{f}: dot");
        assert_eq!(
            native::euclidean(&A20, &B20),
            f.euclidean(&A20, &B20),
            "{f}: euclidean"
        );

        check_mm_targets(|trans_a, trans_b, a, lda, b, ldb, c| {
            f.mm(trans_a, trans_b, 2, 4, 3, a, lda, b, ldb, c, 4);
        });
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx_suite() {
        simd_suite(Feature::AVX | Feature::F16C);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx512_suite() {
        simd_suite(Feature::AVX512);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn asimd_suite() {
        simd_suite(Feature::NONE);
    }
}

// The port's own behaviour.

#[test]
fn empty_slices_return_the_scalar_results() {
    // Go's SIMD builds panic here (index out of range); see X-9.
    for f in levels() {
        assert_eq!(f.dot(&[], &[]).to_bits(), 0.0_f32.to_bits());
        assert_eq!(f.euclidean(&[], &[]).to_bits(), 0.0_f32.to_bits());
        f.mul_const_add_to(&[], 1.0, &[], &mut []);
        f.mul_const_add(&[], 1.0, &mut []);
        f.mul_const_to(&[], 1.0, &mut []);
        f.mul_const(&mut [], 1.0);
        f.add_const(&mut [], 1.0);
        f.sub(&mut [], &[]);
        f.sub_to(&[], &[], &mut []);
        f.mul_to(&[], &[], &mut []);
        f.div_to(&[], &[], &mut []);
        f.sqrt_to(&[], &mut []);
        f.from_float32(&[], &mut []);
        f.to_float32(&[], &mut []);
        for (trans_a, trans_b) in [(false, false), (false, true), (true, false), (true, true)] {
            f.mm(trans_a, trans_b, 0, 0, 0, &[], 0, &[], 0, &mut [], 0);
        }
    }
}

/// Every level this CPU provides, from the scalar loops up.
#[cfg(target_arch = "x86_64")]
fn levels() -> Vec<Feature> {
    let cpu = Feature::detect(Some(""));
    [
        Feature::NONE,
        Feature::AVX | Feature::F16C,
        Feature::AVX512 | Feature::F16C,
    ]
    .into_iter()
    .filter(|&f| cpu.contains(f))
    .collect()
}

/// The one level: NEON on AArch64, the scalar loops elsewhere.
#[cfg(not(target_arch = "x86_64"))]
fn levels() -> Vec<Feature> {
    vec![Feature::NONE]
}

/// The levels that run SIMD kernels.
fn simd_levels() -> Vec<Feature> {
    let levels = levels();
    if cfg!(target_arch = "aarch64") {
        levels
    } else {
        levels.into_iter().skip(1).collect()
    }
}

#[test]
fn mm_without_inner_dimension() {
    for f in levels() {
        // Only `a * bᵀ` writes: empty dot products.
        let mut c = [7.0; 6];
        f.mm(false, true, 2, 3, 0, &[], 0, &[], 0, &mut c, 3);
        assert_eq!(c, [0.0; 6], "{f}");
        for (trans_a, trans_b) in [(false, false), (true, false), (true, true)] {
            let mut c = [7.0; 6];
            f.mm(trans_a, trans_b, 2, 3, 0, &[], 0, &[], 0, &mut c, 3);
            assert_eq!(c, [7.0; 6], "{f}");
        }
    }
}

#[test]
fn mm_with_leading_dimensions() {
    // A 2×3 window of a 2×5 matrix times a 3×2 window of a 3×4 matrix, into
    // a 2×2 window of a 2×3 matrix.
    let a = [1.0, 2.0, 3.0, 99.0, 99.0, 4.0, 5.0, 6.0, 99.0, 99.0];
    let b = [
        1.0, 2.0, 99.0, 99.0, 3.0, 4.0, 99.0, 99.0, 5.0, 6.0, 99.0, 99.0,
    ];
    for f in simd_levels() {
        let mut c = [0.0, 0.0, -1.0, 0.0, 0.0, -1.0];
        f.mm(false, false, 2, 2, 3, &a, 5, &b, 4, &mut c, 3);
        assert_eq!(c, [22.0, 28.0, -1.0, 49.0, 64.0, -1.0], "{f}");
    }
}

/// Go's scalar `mm` slices whole rows and combines them with `MulConstAdd`,
/// which needs `ldb == ldc`; the kernels need not.
#[cfg(not(target_arch = "aarch64"))]
#[test]
#[should_panic(expected = "floats: slice lengths do not match")]
fn scalar_mm_needs_equal_leading_dimensions() {
    let mut c = [0.0; 6];
    Feature::NONE.mm(
        false, false, 2, 2, 3, &[1.0; 6], 3, &[1.0; 12], 4, &mut c, 3,
    );
}

#[test]
#[should_panic(expected = "floats: matrix slices are too short for their dimensions")]
fn mm_rejects_short_slices() {
    let mut c = [0.0; 4];
    Feature::current().mm(false, false, 2, 2, 2, &[1.0; 3], 2, &[1.0; 4], 2, &mut c, 2);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn features_beyond_the_cpu_are_ignored() {
    // Asking for every feature runs the best kernels the CPU has. The inputs
    // round differently at each level, so the levels are distinguishable.
    let a: Vec<f32> = (0..37).map(|i| (i as f32 * 0.37).sin()).collect();
    let b: Vec<f32> = (0..37).map(|i| (i as f32 * 0.73).cos()).collect();
    let all = Feature(u64::MAX);
    assert_eq!(
        all.dot(&a, &b).to_bits(),
        Feature::detect(Some("")).dot(&a, &b).to_bits()
    );
}

#[cfg(target_arch = "x86_64")]
mod godebug {
    use super::super::detect;
    use super::Feature;

    const ALL: Feature =
        Feature(Feature::AVX.0 | Feature::FMA.0 | Feature::AVX512F.0 | Feature::F16C.0);

    #[test]
    fn options_lower_the_level() {
        let cases = [
            ("", ALL, "AVX512+F16C"),
            (
                "cpu.avx512f=off",
                Feature::AVX | Feature::FMA | Feature::F16C,
                "AVX+F16C",
            ),
            (
                "cpu.fma=off",
                Feature::AVX | Feature::AVX512F | Feature::F16C,
                "AVX+F16C",
            ),
            // Without AVX nothing else is used, F16C included.
            ("cpu.avx=off", Feature::FMA | Feature::AVX512F, "AMD64"),
            ("cpu.all=off", Feature::NONE, "AMD64"),
            (
                "cpu.all=off,cpu.avx=on",
                Feature::AVX | Feature::F16C,
                "AVX+F16C",
            ),
            // A later field wins.
            ("cpu.avx=off,cpu.avx=on", ALL, "AVX512+F16C"),
            (
                "gctrace=1,cpu.avx512f=off,madvdontneed=1",
                Feature::AVX | Feature::FMA | Feature::F16C,
                "AVX+F16C",
            ),
            // Ignored, as Go ignores them (with a warning on stderr).
            ("cpu.avx", ALL, "AVX512+F16C"),
            ("cpu.avx=no", ALL, "AVX512+F16C"),
            ("cpu.AVX=off", ALL, "AVX512+F16C"),
            ("cpu.sse3=off", ALL, "AVX512+F16C"),
            ("avx=off", ALL, "AVX512+F16C"),
        ];
        for (godebug, want, name) in cases {
            let got = detect(ALL, godebug);
            assert_eq!(got, want, "GODEBUG={godebug}");
            assert_eq!(got.to_string(), name, "GODEBUG={godebug}");
        }
    }

    #[test]
    fn options_cannot_enable_what_the_cpu_lacks() {
        let avx2_era = Feature::AVX | Feature::FMA | Feature::F16C;
        assert_eq!(detect(avx2_era, "cpu.avx512f=on"), avx2_era);
        assert_eq!(detect(avx2_era, "cpu.all=on"), avx2_era);
        assert_eq!(detect(Feature::NONE, "cpu.all=on"), Feature::NONE);
        // F16C comes only with AVX.
        assert_eq!(detect(Feature::F16C, ""), Feature::NONE);
    }
}
