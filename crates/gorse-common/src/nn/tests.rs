// Ports: common/nn/nn_test.go, op_test.go, tensor_test.go @ add21b5

//! Tests ported from `common/nn/*_test.go`, one submodule per Go file. The
//! helpers below mirror the Go helpers of `op_test.go` (`numericalDiff`,
//! `allClose`) and the testify assertions the Go tests use.
//!
//! The Go tests that read `iris` and `mnist` need datasets that Go downloads
//! from `cdn.gorse.io`; they are ignored here, as the `ann` tests are
//! (`PORTING_NOTES.md` O-1), and read the same files from `~/.gorse/dataset`
//! when those are present.

mod nn_test;
mod op_test;
mod tensor_test;

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use crate::util;
use crate::util::rand::Rand;

use super::deps::global;
use super::*;

/// `eps` of `op_test.go`.
pub(super) const EPS: f32 = 1e-4;
/// `rtol` of `op_test.go`.
pub(super) const RTOL: f32 = 1e-2;
/// `atol` of `op_test.go`.
pub(super) const ATOL: f32 = 5e-3;

/// A tensor of `shape` holding `data`, written out literally in a test.
pub(super) fn lit(data: &[f32], shape: &[usize]) -> Tensor {
    new_tensor(data.to_vec(), shape)
}

/// `numericalDiff`: the central differences of `f` at `x`, summed over the
/// elements of each output, as the gradient of `f` with respect to `x`.
pub(super) fn numerical_diff(f: impl Fn(&Tensor) -> Tensor, x: &Tensor) -> Tensor {
    let v = x.data();
    let shape = x.shape();
    let mut dx = vec![0.0; v.len()];
    for (i, &vi) in v.iter().enumerate() {
        let mut lo = v.clone();
        lo[i] = vi - EPS;
        let mut hi = v.clone();
        hi[i] = vi + EPS;
        let y0 = f(&new_tensor(lo, &shape)).data();
        let y1 = f(&new_tensor(hi, &shape)).data();
        for (a, b) in y0.iter().zip(&y1) {
            dx[i] += (b - a) / (2.0 * EPS);
        }
    }
    new_tensor(dx, &shape)
}

/// `allClose`: the shapes match, and every element of `a` is within
/// `atol + rtol * |b|` of the element of `b`. A NaN passes, as in Go, where
/// the comparison is false.
pub(super) fn all_close(a: &Tensor, b: &Tensor) {
    assert_eq!(a.shape(), b.shape());
    let (da, db) = (a.data(), b.data());
    for (i, (x, y)) in da.iter().zip(&db).enumerate() {
        if (x - y).abs() > ATOL + RTOL * y.abs() {
            panic!("a.data[{i}] = {x}, b.data[{i}] = {y}");
        }
    }
}

/// testify's `InDeltaSlice`: the slices have the same length, and each pair
/// of elements differs by at most `delta`.
pub(super) fn assert_in_delta_slice(expected: &[f32], actual: &[f32], delta: f64) {
    assert_eq!(expected.len(), actual.len());
    for (e, a) in expected.iter().zip(actual) {
        assert_in_delta(f64::from(*e), f64::from(*a), delta);
    }
}

/// testify's `InDelta`.
pub(super) fn assert_in_delta(expected: f64, actual: f64, delta: f64) {
    let dt = expected - actual;
    if dt < -delta || dt > delta {
        panic!("expected {expected} and {actual} differ by more than {delta}");
    }
}

/// Whether `f` panics, as testify's `assert.Panics` checks.
pub(super) fn panics(f: impl FnOnce()) -> bool {
    panic::catch_unwind(AssertUnwindSafe(f)).is_err()
}

/// Where Go's `datautil` keeps a dataset: `~/.gorse/dataset/<name>`.
pub(super) fn dataset(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".gorse")
        .join("dataset")
        .join(name)
}

/// The Go `*rand.Rand` of `rand.New(rand.NewSource(seed))`, as the random
/// source of [`rand`] and [`normal_init`].
pub(super) struct GoRand(Rand);

impl GoRand {
    pub(super) fn new(seed: i64) -> GoRand {
        GoRand(Rand::new(seed))
    }
}

impl Rng for GoRand {
    fn float32(&mut self) -> f32 {
        self.0.float32()
    }

    fn norm_float64(&mut self) -> f64 {
        self.0.norm_float64()
    }
}

/// `rand.Perm` of Go's top-level source, which Go seeds randomly, so no
/// test can depend on which permutation comes out.
pub(super) fn global_perm(n: usize) -> Vec<usize> {
    let mut p: Vec<usize> = (0..n).collect();
    global(|r| {
        for i in (1..n).rev() {
            let j = ((r.float32() * (i + 1) as f32) as usize).min(i);
            p.swap(i, j);
        }
    });
    p
}

/// `iris.data`: the four features of each row as a `(rows, 4)` tensor, and the
/// class of each row, numbered in order of first appearance in the fifth
/// column, as a `(rows)` tensor.
pub(super) fn load_iris(path: &Path) -> (Tensor, Tensor) {
    let text = std::fs::read_to_string(path).expect("iris.data");
    let mut data = Vec::new();
    let mut target = Vec::new();
    let mut types: HashMap<&str, usize> = HashMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let cells: Vec<&str> = line.split(',').collect();
        for cell in &cells[..4] {
            data.push(util::parse_float::<f32>(cell).expect("iris feature"));
        }
        let next = types.len();
        let class = *types.entry(cells[4]).or_insert(next);
        target.push(class as f32);
    }
    let rows = target.len();
    (new_tensor(data, &[rows, 4]), new_tensor(target, &[rows]))
}

/// `openMNISTFile`: the 784 pixels of each libfm line as a row of a
/// `(rows, 784)` tensor and its label as a `(rows)` tensor.
pub(super) fn open_mnist_file(path: &Path) -> (Tensor, Tensor) {
    let text = std::fs::read_to_string(path).expect("libfm file");
    let mut images = Vec::new();
    let mut labels = Vec::new();
    for line in text.lines() {
        let splits: Vec<&str> = line.split(' ').collect();
        labels.push(util::parse_float::<f32>(splits[0]).expect("label"));
        let mut image = vec![0.0; 784];
        for split in &splits[1..] {
            let (index, value) = split.split_once(':').expect("index:value");
            let index: usize = index.parse().expect("pixel index");
            image[index] = util::parse_float::<f32>(value).expect("pixel");
        }
        images.extend(image);
    }
    let rows = labels.len();
    (
        new_tensor(images, &[rows, 784]),
        new_tensor(labels, &[rows]),
    )
}
