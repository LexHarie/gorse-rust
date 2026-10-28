// Ports: common/nn/op_test.go @ add21b5

//! Ports of `common/nn/op_test.go`: each operator's values, and its gradient
//! against a numerical derivative.

#![allow(
    clippy::excessive_precision,
    reason = "the expected values are Go's float64 results, printed in full"
)]

use super::*;
use crate::math32;

#[test]
fn test_add() {
    // (2,3) + (2,3) -> (2,3)
    let mut x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0, 5.0, 6.0, 7.0], &[2, 3]);
    let mut z = add(&x, &[&y]);
    assert_eq!(z.data(), [3.0, 5.0, 7.0, 9.0, 11.0, 13.0]);

    // Test gradient
    x = rand(None, &[2, 3]);
    let mut y = rand(None, &[2, 3]);
    z = add(&x, &[&y]);
    z.backward();
    let dx = numerical_diff(|x| add(x, &[&y]), &x);
    all_close(&x.grad().unwrap(), &dx);
    let dy = numerical_diff(|y| add(&x, &[y]), &y);
    all_close(&y.grad().unwrap(), &dy);

    // (2,3) + () -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0], &[]);
    z = add(&x, &[&y]);
    assert_eq!(z.data(), [3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
    assert_eq!(y.grad().unwrap().data(), [6.0]);

    // (2,3) + (3) -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0, 3.0, 4.0], &[3]);
    z = add(&x, &[&y]);
    assert_eq!(z.data(), [3.0, 5.0, 7.0, 6.0, 8.0, 10.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
    assert_eq!(y.grad().unwrap().data(), [2.0, 2.0, 2.0]);
}

#[test]
fn test_sub() {
    // (2,3) - (2,3) -> (2,3)
    let mut x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0, 5.0, 6.0, 7.0], &[2, 3]);
    let z = sub(&x, &y);
    assert_eq!(z.data(), [-1.0; 6]);

    // Test gradient
    x = rand(None, &[2, 3]);
    let mut y = rand(None, &[2, 3]);
    let z = sub(&x, &y);
    z.backward();
    let dx = numerical_diff(|x| sub(x, &y), &x);
    all_close(&x.grad().unwrap(), &dx);
    let dy = numerical_diff(|y| sub(&x, y), &y);
    all_close(&y.grad().unwrap(), &dy);

    // (2,3) - () -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0], &[]);
    let z = sub(&x, &y);
    assert_eq!(z.data(), [-1.0, 0.0, 1.0, 2.0, 3.0, 4.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
    assert_eq!(y.grad().unwrap().data(), [-6.0]);

    // (2,3) - (3) -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0, 3.0, 4.0], &[3]);
    let z = sub(&x, &y);
    assert_eq!(z.data(), [-1.0, -1.0, -1.0, 2.0, 2.0, 2.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
    assert_eq!(y.grad().unwrap().data(), [-2.0, -2.0, -2.0]);
}

#[test]
fn test_mul() {
    // (2,3) * (2,3) -> (2,3)
    let mut x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0, 5.0, 6.0, 7.0], &[2, 3]);
    let z = mul(&x, &y);
    assert_eq!(z.data(), [2.0, 6.0, 12.0, 20.0, 30.0, 42.0]);

    // Test gradient
    x = rand(None, &[2, 3]);
    let mut y = rand(None, &[2, 3]);
    let z = mul(&x, &y);
    z.backward();
    let dx = numerical_diff(|x| mul(x, &y), &x);
    all_close(&x.grad().unwrap(), &dx);
    let dy = numerical_diff(|y| mul(&x, y), &y);
    all_close(&y.grad().unwrap(), &dy);

    // (2,3) * () -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0], &[]);
    let z = mul(&x, &y);
    assert_eq!(z.data(), [2.0, 4.0, 6.0, 8.0, 10.0, 12.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [2.0; 6]);
    assert_eq!(y.grad().unwrap().data(), [21.0]);

    // (2,3) * (3) -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    y = lit(&[2.0, 3.0, 4.0], &[3]);
    let z = mul(&x, &y);
    assert_eq!(z.data(), [2.0, 6.0, 12.0, 8.0, 15.0, 24.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().data(), [2.0, 3.0, 4.0, 2.0, 3.0, 4.0]);
    assert_eq!(y.grad().unwrap().data(), [5.0, 7.0, 9.0]);
}

#[test]
fn test_div() {
    // (2,3) / (2,3) -> (2,3)
    let mut x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0, 5.0, 6.0, 7.0], &[2, 3]);
    let z = div(&x, &y);
    assert_in_delta_slice(
        &[0.5, 2.0 / 3.0, 0.75, 4.0 / 5.0, 5.0 / 6.0, 6.0 / 7.0],
        &z.data(),
        1e-6,
    );

    // Test gradient
    z.backward();
    let dx = numerical_diff(|x| div(x, &y), &x);
    all_close(&x.grad().unwrap(), &dx);
    let dy = numerical_diff(|y| div(&x, y), &y);
    all_close(&y.grad().unwrap(), &dy);

    // (2,3) / () -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0], &[]);
    let z = div(&x, &y);
    assert_in_delta_slice(&[0.5, 1.0, 1.5, 2.0, 2.5, 3.0], &z.data(), 1e-6);

    // Test gradient
    z.backward();
    assert_in_delta_slice(&[0.5; 6], &x.grad().unwrap().data(), 1e-6);
    assert_in_delta_slice(&[-21.0 / 4.0], &y.grad().unwrap().data(), 1e-6);

    // (2,3) / (3) -> (2,3)
    x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0], &[3]);
    let z = div(&x, &y);
    assert_in_delta_slice(
        &[0.5, 2.0 / 3.0, 3.0 / 4.0, 2.0, 5.0 / 3.0, 1.5],
        &z.data(),
        1e-6,
    );

    // Test gradient
    z.backward();
    assert_in_delta_slice(
        &[
            1.0 / 2.0,
            1.0 / 3.0,
            1.0 / 4.0,
            1.0 / 2.0,
            1.0 / 3.0,
            1.0 / 4.0,
        ],
        &x.grad().unwrap().data(),
        1e-6,
    );
    assert_in_delta_slice(
        &[-5.0 / 4.0, -7.0 / 9.0, -9.0 / 16.0],
        &y.grad().unwrap().data(),
        1e-6,
    );
}

#[test]
fn test_square() {
    // (2,3) -> (2,3)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = square(&x);
    assert_eq!(y.data(), [1.0, 4.0, 9.0, 16.0, 25.0, 36.0]);

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = square(&x);
    y.backward();
    let dx = numerical_diff(square, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_pow() {
    // (2,3) ** (2,3) -> (2,3)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0, 3.0, 4.0, 5.0, 6.0, 7.0], &[2, 3]);
    let z = pow(&x, &y);
    assert_in_delta_slice(
        &[1.0, 8.0, 81.0, 1024.0, 15625.0, 279936.0],
        &z.data(),
        1e-6,
    );

    // Test gradient
    let x = uniform(0.5, 1.0, &[2, 3]);
    let y = uniform(0.5, 1.0, &[2, 3]);
    let z = pow(&x, &y);
    z.backward();
    let dx = numerical_diff(|x| pow(x, &y), &x);
    all_close(&x.grad().unwrap(), &dx);
    let dy = numerical_diff(|y| pow(&x, y), &y);
    all_close(&y.grad().unwrap(), &dy);

    // (2,3) ** () -> (2,3)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(&[2.0], &[]);
    let z = pow(&x, &y);
    assert_in_delta_slice(&[1.0, 4.0, 9.0, 16.0, 25.0, 36.0], &z.data(), 1e-6);

    // Test gradient
    z.backward();
    assert_in_delta_slice(
        &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0],
        &x.grad().unwrap().data(),
        1e-6,
    );
    assert_in_delta_slice(
        &[math32::pow(1.0, 2.0) * math32::log(1.0)
            + math32::pow(2.0, 2.0) * math32::log(2.0)
            + math32::pow(3.0, 2.0) * math32::log(3.0)
            + math32::pow(4.0, 2.0) * math32::log(4.0)
            + math32::pow(5.0, 2.0) * math32::log(5.0)
            + math32::pow(6.0, 2.0) * math32::log(6.0)],
        &y.grad().unwrap().data(),
        1e-6,
    );
}

#[test]
fn test_exp() {
    // (2,3) -> (2,3)
    let x = lit(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[2, 3]);
    let y = exp(&x);
    assert_in_delta_slice(
        &[
            1.0,
            math32::exp(1.0),
            math32::exp(2.0),
            math32::exp(3.0),
            math32::exp(4.0),
            math32::exp(5.0),
        ],
        &y.data(),
        1e-5,
    );

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = exp(&x);
    y.backward();
    let dx = numerical_diff(exp, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_log() {
    // (2,3) -> (2,3)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = log(&x);
    assert_in_delta_slice(
        &[
            0.0,
            math32::log(2.0),
            math32::log(3.0),
            math32::log(4.0),
            math32::log(5.0),
            math32::log(6.0),
        ],
        &y.data(),
        1e-6,
    );

    // Test gradient
    let x = uniform(1.0, 2.0, &[2, 3]);
    let y = log(&x);
    y.backward();
    let dx = numerical_diff(log, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_abs() {
    // (2,3) -> (2,3)
    let x = lit(&[1.0, -2.0, 3.0, -4.0, 5.0, -6.0], &[2, 3]);
    let y = abs(&x);
    assert_eq!(y.data(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    // Test gradient away from zero, where Abs is not differentiable.
    let x = lit(&[-0.9, -0.5, -0.1, 0.1, 0.5, 0.9], &[2, 3]);
    let y = abs(&x);
    y.backward();
    let dx = numerical_diff(abs, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_sum() {
    // (2,3) -> ()
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = sum(&x, &[]);
    assert_eq!(y.data(), [21.0]);

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = sum(&x, &[]);
    y.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);

    // (2,3,2) -> (2,2)
    let x = lit(
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        &[2, 3, 2],
    );
    let y = sum(&x, &[1]);
    assert_eq!(y.shape(), [2, 2]);
    assert_eq!(y.data(), [9.0, 12.0, 9.0, 12.0]);

    // Test gradient
    let x = rand(None, &[2, 3, 2]);
    let y = sum(&x, &[1]);
    y.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 3, 2]);
    assert_eq!(x.grad().unwrap().data(), [1.0; 12]);
}

#[test]
fn test_mean() {
    // (2,3) -> ()
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = mean(&x);
    assert_eq!(y.data(), [3.5]);

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = mean(&x);
    y.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0 / 6.0; 6]);
}

#[test]
fn test_cos() {
    // (2,3) -> (2,3)
    let x = lit(&[0.0, 0.1, 0.2, 0.3, 0.4, 0.5], &[2, 3]);
    let y = cos(&x);
    assert_in_delta_slice(
        &[
            1.0,
            0.9950041652780258,
            0.9800665778412416,
            0.955336489125606,
            0.9210609940028851,
            0.8775825618903728,
        ],
        &y.data(),
        1e-6,
    );

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = cos(&x);
    y.backward();
    let dx = numerical_diff(cos, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_sin() {
    // (2,3) -> (2,3)
    let x = lit(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[2, 3]);
    let y = sin(&x);
    assert_in_delta_slice(
        &[
            0.0,
            0.8414709848078965,
            0.9092974268256817,
            0.1411200080598672,
            -0.7568024953079282,
            -0.9589242746631385,
        ],
        &y.data(),
        1e-6,
    );

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = sin(&x);
    y.backward();
    let dx = numerical_diff(sin, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_mat_mul() {
    // (2,3) * (3,4) -> (2,4)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = lit(
        &[
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
        &[3, 4],
    );
    let z = mat_mul(&x, &y, false, false, 0);
    assert_eq!(z.shape(), [2, 4]);
    assert_eq!(z.data(), [38.0, 44.0, 50.0, 56.0, 83.0, 98.0, 113.0, 128.0]);

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 3]);
    assert_eq!(
        x.grad().unwrap().data(),
        [10.0, 26.0, 42.0, 10.0, 26.0, 42.0]
    );
    assert_eq!(y.grad().unwrap().shape(), [3, 4]);
    assert_eq!(
        y.grad().unwrap().data(),
        [5.0, 5.0, 5.0, 5.0, 7.0, 7.0, 7.0, 7.0, 9.0, 9.0, 9.0, 9.0]
    );

    // (3,2).T * (3,4) -> (2,4)
    let x = rand(None, &[3, 2]);
    let y = rand(None, &[3, 4]);
    let z = mat_mul(&x, &y, true, false, 0);
    assert_eq!(z.shape(), [2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [3, 2]);
    assert_eq!(y.grad().unwrap().shape(), [3, 4]);

    // (2,3) * (4,3).T -> (2,4)
    let x = rand(None, &[2, 3]);
    let y = rand(None, &[4, 3]);
    let z = mat_mul(&x, &y, false, true, 0);
    assert_eq!(z.shape(), [2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 3]);
    assert_eq!(y.grad().unwrap().shape(), [4, 3]);

    // (3,2).T * (4,3).T -> (2,4)
    let x = rand(None, &[3, 2]);
    let y = rand(None, &[4, 3]);
    let z = mat_mul(&x, &y, true, true, 0);
    assert_eq!(z.shape(), [2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [3, 2]);
}

#[test]
fn test_bmm() {
    // (2,2,3) * (2,3,4) -> (2,2,4)
    let x = lit(
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        &[2, 2, 3],
    );
    let y = lit(
        &[
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, //
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ],
        &[2, 3, 4],
    );
    let z = bmm(&x, &y, false, false, 0);
    assert_eq!(z.shape(), [2, 2, 4]);
    assert_eq!(
        z.data(),
        [
            38.0, 44.0, 50.0, 56.0, 83.0, 98.0, 113.0, 128.0, //
            38.0, 44.0, 50.0, 56.0, 83.0, 98.0, 113.0, 128.0,
        ]
    );

    // Test gradient
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 2, 3]);
    assert_eq!(
        x.grad().unwrap().data(),
        [
            10.0, 26.0, 42.0, 10.0, 26.0, 42.0, 10.0, 26.0, 42.0, 10.0, 26.0, 42.0
        ]
    );
    assert_eq!(y.grad().unwrap().shape(), [2, 3, 4]);
    assert_eq!(
        y.grad().unwrap().data(),
        [
            5.0, 5.0, 5.0, 5.0, 7.0, 7.0, 7.0, 7.0, 9.0, 9.0, 9.0, 9.0, //
            5.0, 5.0, 5.0, 5.0, 7.0, 7.0, 7.0, 7.0, 9.0, 9.0, 9.0, 9.0,
        ]
    );

    // (2,3,2).T * (2,3,4) -> (2,2,4)
    let x = rand(None, &[2, 3, 2]);
    let y = rand(None, &[2, 3, 4]);
    let z = bmm(&x, &y, true, false, 0);
    assert_eq!(z.shape(), [2, 2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 3, 2]);

    // (2,2,3) * (2,4,3).T -> (2,2,4)
    let x = rand(None, &[2, 2, 3]);
    let y = rand(None, &[2, 4, 3]);
    let z = bmm(&x, &y, false, true, 0);
    assert_eq!(z.shape(), [2, 2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 2, 3]);

    // (2,3,2).T * (2,4,3).T -> (2,2,4)
    let x = rand(None, &[2, 3, 2]);
    let y = rand(None, &[2, 4, 3]);
    let z = bmm(&x, &y, true, true, 0);
    assert_eq!(z.shape(), [2, 2, 4]);
    z.backward();
    assert_eq!(x.grad().unwrap().shape(), [2, 3, 2]);
}

#[test]
fn test_broadcast() {
    // (2) -> (2,3)
    let x = lit(&[1.0, 2.0], &[2]);
    let y = broadcast(&x, &[3]);
    assert_eq!(y.data(), [1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);

    // Test gradient
    y.backward();
    assert_eq!(x.grad().unwrap().data(), [3.0, 3.0]);
}

#[test]
fn test_embedding() {
    // (2,3) -> (2,3,2)
    let x = lit(&[0.0, 1.0, 0.0, 3.0, 0.0, 5.0], &[2, 3]);
    let w = lit(
        &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0],
        &[6, 2],
    );
    let y = embedding(&w, &x);
    assert_eq!(y.shape(), [2, 3, 2]);
    assert_eq!(
        y.data(),
        [0.0, 1.0, 2.0, 3.0, 0.0, 1.0, 6.0, 7.0, 0.0, 1.0, 10.0, 11.0]
    );

    // Test gradient
    y.backward();
    assert!(x.grad().is_none());
    assert_eq!(
        w.grad().unwrap().data(),
        [3.0, 3.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]
    );

    // (2,3) -> (2,3,1,2)
    let x = lit(&[0.0, 1.0, 0.0, 3.0, 0.0, 5.0], &[2, 3]);
    let w = lit(
        &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0],
        &[6, 1, 2],
    );
    let y = embedding(&w, &x);
    assert_eq!(y.shape(), [2, 3, 1, 2]);
    assert_eq!(
        y.data(),
        [0.0, 1.0, 2.0, 3.0, 0.0, 1.0, 6.0, 7.0, 0.0, 1.0, 10.0, 11.0]
    );

    // Test gradient
    y.backward();
    assert!(x.grad().is_none());
    assert_eq!(
        w.grad().unwrap().data(),
        [3.0, 3.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]
    );
}

#[test]
fn test_sigmoid() {
    // (2,3) -> (2,3)
    let x = lit(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[2, 3]);
    let y = sigmoid(&x);
    assert_in_delta_slice(
        &[
            0.5,
            0.7310585786300049,
            0.8807970779778823,
            0.9525741268224334,
            0.9820137900379085,
            0.9933071490757153,
        ],
        &y.data(),
        1e-6,
    );

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = sigmoid(&x);
    y.backward();
    let dx = numerical_diff(sigmoid, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_re_lu() {
    // (2,3) -> (2,3)
    let x = lit(&[-1.0, 0.0, 1.0, 2.0, 3.0, 4.0], &[2, 3]);
    let y = relu(&x);
    assert_eq!(y.data(), [0.0, 0.0, 1.0, 2.0, 3.0, 4.0]);

    // Test gradient
    let x = rand(None, &[2, 3]);
    let y = relu(&x);
    y.backward();
    let dx = numerical_diff(relu, &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_softmax() {
    // (1,3) -> (1,3)
    let x = lit(&[3.0, 1.0, 0.2], &[1, 3]);
    let y = softmax(&x, 1);
    assert_eq!(y.shape(), [1, 3]);
    assert_in_delta_slice(
        &[0.8360188027814407, 0.11314284146556013, 0.05083835575299916],
        &y.data(),
        1e-6,
    );

    // Test gradient
    y.backward();
    let dx = numerical_diff(|x| softmax(x, 1), &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_flatten() {
    // (2,3) -> (6)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = flatten(&x);
    assert_eq!(y.data(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    // Test gradient
    y.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
}

#[test]
fn test_reshape() {
    // (2,3) -> (3,2)
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = reshape(&x, &[3, 2]);
    assert_eq!(y.data(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    // Test gradient
    y.backward();
    assert_eq!(x.grad().unwrap().data(), [1.0; 6]);
}

#[test]
fn test_softmax_cross_entropy() {
    // (2,3) -> (2,3)
    let x = lit(&[0.3, 2.9, 4.0, 0.2, 1.0, 3.0], &[3, 2]);
    let y = lit(&[1.0, 0.0, 1.0], &[3]);
    let z = softmax_cross_entropy(&x, &y);
    assert!(z.shape().is_empty());
    assert_in_delta(f64::from(z.data()[0]), 0.07356563982184072, 1e-4);

    // Test gradient
    z.backward();
    let dx = numerical_diff(|x| softmax_cross_entropy(x, &y), &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_reuse_leaf() {
    // x + x
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let y = add(&x, &[&x]);
    assert_eq!(y.data(), [2.0, 4.0, 6.0, 8.0, 10.0, 12.0]);

    // Test gradient
    y.backward();
    let dx = numerical_diff(|x| add(x, &[x]), &x);
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_reuse_node() {
    // x^2 + x^2
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let temp = pow(&x, &lit(&[2.0], &[]));
    let y = add(&temp, &[&temp]);
    assert_eq!(y.data(), [2.0, 8.0, 18.0, 32.0, 50.0, 72.0]);

    // Test gradient
    y.backward();
    let dx = numerical_diff(
        |x| {
            let temp = pow(x, &lit(&[2.0], &[]));
            add(&temp, &[&temp])
        },
        &x,
    );
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_dependency() {
    // x^2 + 2x^2
    let x = lit(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]);
    let temp = pow(&x, &lit(&[2.0], &[]));
    let y = add(&temp, &[&mul(&lit(&[2.0], &[]), &temp)]);
    assert_eq!(y.data(), [3.0, 12.0, 27.0, 48.0, 75.0, 108.0]);

    // Test gradient
    y.backward();
    let dx = numerical_diff(
        |x| {
            let temp = pow(x, &lit(&[2.0], &[]));
            add(&temp, &[&mul(&lit(&[2.0], &[]), &temp)])
        },
        &x,
    );
    all_close(&x.grad().unwrap(), &dx);
}

#[test]
fn test_sphere() {
    // x^2 + y^2
    let x = new_scalar(1.0);
    let y = new_scalar(1.0);
    let z = add(&mul(&x, &x), &[&mul(&y, &y)]);
    assert_eq!(z.data(), [2.0]);

    // Test gradient
    z.backward();
    let dx = numerical_diff(|x| add(&mul(x, x), &[&mul(&y, &y)]), &x);
    let dy = numerical_diff(|y| add(&mul(&x, &x), &[&mul(y, y)]), &y);
    all_close(&x.grad().unwrap(), &dx);
    all_close(&y.grad().unwrap(), &dy);
}

#[test]
fn test_matyas() {
    // 0.26 * (x^2 + y^2) - 0.48 * x * y
    let x = new_scalar(1.0);
    let y = new_scalar(1.0);
    let matyas = |x: &Tensor, y: &Tensor| {
        sub(
            &mul(&new_scalar(0.26), &add(&mul(x, x), &[&mul(y, y)])),
            &mul(&new_scalar(0.48), &mul(x, y)),
        )
    };
    let z = matyas(&x, &y);
    assert_in_delta_slice(&[0.04], &z.data(), 1e-6);

    // Test gradient
    z.backward();
    let dx = numerical_diff(|x| matyas(x, &y), &x);
    let dy = numerical_diff(|y| matyas(&x, y), &y);
    all_close(&x.grad().unwrap(), &dx);
    all_close(&y.grad().unwrap(), &dy);
}

#[test]
fn test_goldstein_price() {
    // (1 + (x + y + 1)^2 * (19 - 14x + 3x^2 - 14y + 6xy + 3y^2)) * (30 + (2x - 3y)^2 * (18 - 32x + 12x^2 + 48y - 36xy + 27y^2))
    let goldstein_price = |x: &Tensor, y: &Tensor| {
        mul(
            &add(
                &new_scalar(1.0),
                &[&mul(
                    &pow(&add(x, &[y, &new_scalar(1.0)]), &new_scalar(2.0)), // (x + y + 1)^2
                    &add(
                        &new_scalar(19.0),
                        &[
                            &mul(&new_scalar(-14.0), x),                       // -14x
                            &mul(&new_scalar(3.0), &pow(x, &new_scalar(2.0))), // 3x^2
                            &mul(&new_scalar(-14.0), y),                       // -14y
                            &mul(&new_scalar(6.0), &mul(x, y)),                // 6xy
                            &mul(&new_scalar(3.0), &pow(y, &new_scalar(2.0))), // 3y^2
                        ],
                    ),
                )],
            ),
            &add(
                &new_scalar(30.0),
                &[&mul(
                    &pow(
                        &sub(&mul(&new_scalar(2.0), x), &mul(&new_scalar(3.0), y)),
                        &new_scalar(2.0),
                    ), // (2x - 3y)^2
                    &add(
                        &new_scalar(18.0),
                        &[
                            &mul(&new_scalar(-32.0), x),                        // -32x
                            &mul(&new_scalar(12.0), &pow(x, &new_scalar(2.0))), // 12x^2
                            &mul(&new_scalar(48.0), y),                         // 48y
                            &mul(&new_scalar(-36.0), &mul(x, y)),               // -36xy
                            &mul(&new_scalar(27.0), &pow(y, &new_scalar(2.0))), // 27y^2
                        ],
                    ),
                )],
            ),
        )
    };
    let x = new_scalar(1.0);
    let y = new_scalar(1.0);
    let z = goldstein_price(&x, &y);
    assert_in_delta_slice(&[1876.0], &z.data(), 1e-6);

    // Test gradient
    z.backward();
    let dx = numerical_diff(|x| goldstein_price(x, &y), &x);
    let dy = numerical_diff(|y| goldstein_price(&x, y), &y);
    all_close(&x.grad().unwrap(), &dx);
    all_close(&y.grad().unwrap(), &dy);
}
