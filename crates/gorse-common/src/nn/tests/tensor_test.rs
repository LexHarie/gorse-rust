// Ports: common/nn/tensor_test.go @ add21b5

//! Ports of `common/nn/tensor_test.go`. Its `matMulLegacy` and
//! `batchMatMulLegacy` helpers and the benchmarks that call them are not
//! ported: they measure speed and assert nothing.

use super::*;

#[test]
fn test_tensor_slice() {
    let x = rand(None, &[3, 4, 5]);
    let y = x.slice(1, 3);
    assert_eq!(y.shape(), [2, 4, 5]);
    for i in 0..2 {
        for j in 0..4 {
            for k in 0..5 {
                assert_eq!(x.get(&[i + 1, j, k]), y.get(&[i, j, k]));
            }
        }
    }
}

#[test]
fn test_tensor_slice_indices() {
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[3, 2]);
    let y = x.slice_indices(&[2, 0]);
    assert_eq!(y.shape(), [2, 2]);
    assert_eq!(y.data(), [5.0, 6.0, 1.0, 2.0]);
}

#[test]
fn test_tensor_max() {
    let x = lit(&[3.0, 2.0, 5.0, 6.0, 0.0, 0.0], &[6]);
    let y = x.max(0, false);
    assert!(y.shape().is_empty());
    assert_eq!(y.data(), [6.0]);

    assert!(panics(|| {
        x.max(-1, false);
    }));
    assert!(panics(|| {
        x.max(2, false);
    }));

    let x = lit(
        &[
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
        &[3, 2, 2],
    );
    let y = x.max(1, false);
    assert_eq!(y.shape(), [3, 2]);
    assert_eq!(y.data(), [3.0, 4.0, 7.0, 8.0, 11.0, 12.0]);
    let y = x.max(1, true);
    assert_eq!(y.shape(), [3, 1, 2]);
    assert_eq!(y.data(), [3.0, 4.0, 7.0, 8.0, 11.0, 12.0]);
}

#[test]
fn test_tensor_sum() {
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[6]);
    let y = x.sum(0, false);
    assert!(y.shape().is_empty());
    assert_eq!(y.data(), [21.0]);

    assert!(panics(|| {
        x.sum(-1, false);
    }));
    assert!(panics(|| {
        x.sum(2, false);
    }));

    let x = lit(
        &[
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
        &[3, 2, 2],
    );
    let y = x.sum(1, false);
    assert_eq!(y.shape(), [3, 2]);
    assert_eq!(y.data(), [4.0, 6.0, 12.0, 14.0, 20.0, 22.0]);
    let y = x.sum(1, true);
    assert_eq!(y.shape(), [3, 1, 2]);
    assert_eq!(y.data(), [4.0, 6.0, 12.0, 14.0, 20.0, 22.0]);
}

#[test]
fn test_tensor_transpose() {
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[3, 2]);
    let y = x.transpose();
    assert_eq!(y.shape(), [2, 3]);
    assert_eq!(y.data(), [1.0, 3.0, 5.0, 2.0, 4.0, 6.0]);
}
