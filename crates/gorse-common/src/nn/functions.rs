// Ports: common/nn/functions.go @ add21b5

//! The differentiable functions: each applies an operator and records it in
//! the graph for `backward`.

use super::golang;
use super::op::{Op, apply};
use super::tensor::{Tensor, new_scalar};

/// `-x` (Go `Neg`).
pub fn neg(x: &Tensor) -> Tensor {
    apply(Op::Neg, vec![x.clone()])
}

/// `x[i]` of a shape, Go's index check included: the dimension `len(x0) -
/// len(x1) + i` of the longer shape that the suffix check compares.
fn dim(shape: &[usize], i: i64) -> usize {
    shape[golang::index(i, shape.len())]
}

/// Whether `short` matches the end of `long`, the dimensions compared as Go
/// does, `long[len(long) - len(short) + i]` for each `i` of `short`.
fn is_suffix(long: &[usize], short: &[usize]) -> bool {
    let offset = golang::int(long.len()) - golang::int(short.len());
    short
        .iter()
        .enumerate()
        .all(|(i, &d)| dim(long, offset + golang::int(i)) == d)
}

/// The element-wise sum of tensors, each shape a suffix of the other's, from
/// left to right (Go `Add`). The shorter of each pair is repeated along the
/// longer.
///
/// # Panics
///
/// When the shapes do not match, with Go's message. As in Go, the check
/// compares each tensor with `x0`, not with the sum so far, so that a third
/// tensor longer than `x0` panics with an index out of range.
pub fn add(x0: &Tensor, x: &[&Tensor]) -> Tensor {
    let mut output = x0.clone();
    let x0_shape = x0.shape();
    for &x1 in x {
        let mut x1 = x1.clone();
        if x0_shape.len() < x1.shape().len() {
            std::mem::swap(&mut output, &mut x1);
        }
        let x1_shape = x1.shape();
        if !is_suffix(&x0_shape, &x1_shape) {
            panic!(
                "the shape of one tensor {} must be a suffix sequence of the shape of the other tensor {}",
                golang::format_ints(&x0_shape),
                golang::format_ints(&x1_shape)
            );
        }
        output = apply(Op::Add, vec![output, x1]);
    }
    output
}

/// `x0 - x1`, `x1` repeated along `x0` (Go `Sub`).
///
/// # Panics
///
/// With Go's messages when the shape of `x1` is not a suffix of the shape
/// of `x0`.
pub fn sub(x0: &Tensor, x1: &Tensor) -> Tensor {
    let (s0, s1) = (x0.shape(), x1.shape());
    if s0.len() < s1.len() {
        panic!(
            "the shape of the second tensor {} must be a suffix sequence of the shape of the first tensor {}",
            golang::format_ints(&s1),
            golang::format_ints(&s0)
        );
    }
    if !is_suffix(&s0, &s1) {
        panic!("the shape of the second tensor must be a suffix sequence of the shape of the first tensor");
    }
    apply(Op::Sub, vec![x0.clone(), x1.clone()])
}

/// The element-wise product, the shorter tensor repeated along the longer
/// (Go `Mul`).
///
/// # Panics
///
/// With Go's message when the shorter shape is not a suffix of the longer.
pub fn mul(x0: &Tensor, x1: &Tensor) -> Tensor {
    let (mut x0, mut x1) = (x0.clone(), x1.clone());
    if x0.shape().len() < x1.shape().len() {
        std::mem::swap(&mut x0, &mut x1);
    }
    let (s0, s1) = (x0.shape(), x1.shape());
    if !is_suffix(&s0, &s1) {
        panic!(
            "the shape of the second tensor {} must be a suffix sequence of the shape of the first tensor {}",
            golang::format_ints(&s1),
            golang::format_ints(&s0)
        );
    }
    apply(Op::Mul, vec![x0, x1])
}

/// `x0 / x1`, `x1` repeated along `x0` (Go `Div`).
///
/// # Panics
///
/// With Go's messages when the shape of `x1` is not a suffix of the shape
/// of `x0`.
pub fn div(x0: &Tensor, x1: &Tensor) -> Tensor {
    let (s0, s1) = (x0.shape(), x1.shape());
    if s0.len() < s1.len() {
        panic!(
            "the shape of the second tensor {} must be a suffix sequence of the shape of the first tensor {}",
            golang::format_ints(&s1),
            golang::format_ints(&s0)
        );
    }
    if !is_suffix(&s0, &s1) {
        panic!("the shape of the second tensor must be a suffix sequence of the shape of the first tensor");
    }
    apply(Op::Div, vec![x0.clone(), x1.clone()])
}

/// The element-wise square (Go `Square`).
pub fn square(x: &Tensor) -> Tensor {
    apply(Op::Square, vec![x.clone()])
}

/// The element-wise power `x^n`, `n` repeated along `x` (Go `Pow`).
///
/// # Panics
///
/// With Go's messages when `n` has more dimensions than `x`, or when the
/// check fails. As in Go, the check compares `n.shape[len(n) - len(x) +
/// i]` with `x.shape[i]`, so a shorter `n` panics with an index out of range
/// rather than checking that it is a suffix.
pub fn pow(x: &Tensor, n: &Tensor) -> Tensor {
    let (sx, sn) = (x.shape(), n.shape());
    if sx.len() < sn.len() {
        panic!("the shape of the second tensor must be a suffix sequence of the shape of the first tensor");
    }
    let offset = golang::int(sn.len()) - golang::int(sx.len());
    for i in 0..sn.len() {
        if dim(&sn, offset + golang::int(i)) != sx[i] {
            panic!("the shape of the second tensor must be a suffix sequence of the shape of the first tensor");
        }
    }
    apply(Op::Pow, vec![x.clone(), n.clone()])
}

/// The element-wise exponential, by Go's float64 `math.Exp` (Go `Exp`).
pub fn exp(x: &Tensor) -> Tensor {
    apply(Op::Exp, vec![x.clone()])
}

/// The element-wise natural logarithm (Go `Log`).
pub fn log(x: &Tensor) -> Tensor {
    apply(Op::Log, vec![x.clone()])
}

/// The element-wise sine (Go `Sin`).
pub fn sin(x: &Tensor) -> Tensor {
    apply(Op::Sin, vec![x.clone()])
}

/// The element-wise cosine (Go `Cos`).
pub fn cos(x: &Tensor) -> Tensor {
    apply(Op::Cos, vec![x.clone()])
}

/// The element-wise absolute value (Go `Abs`).
pub fn abs(x: &Tensor) -> Tensor {
    apply(Op::Abs, vec![x.clone()])
}

/// The sum of all elements, a scalar, or with one `along` the sums along
/// that dimension (Go `Sum`). As in Go, an `along` outside the dimensions
/// removes none and sums nothing.
///
/// # Panics
///
/// With Go's message for more than one `along`.
pub fn sum(x: &Tensor, along: &[isize]) -> Tensor {
    match along {
        [] => apply(Op::Sum, vec![x.clone()]),
        &[along] => apply(Op::PartialSum { along: along as i64 }, vec![x.clone()]),
        _ => panic!("only one along is allowed"),
    }
}

/// The mean of all elements, a scalar (Go `Mean`).
pub fn mean(x: &Tensor) -> Tensor {
    apply(Op::Mean, vec![x.clone()])
}

/// The matrix product of 2-D tensors, either transposed, with the rows of
/// the result computed by `jobs` threads (Go `MatMul`).
///
/// # Panics
///
/// With Go's messages for tensors that are not 2-D or do not match.
pub fn mat_mul(x: &Tensor, y: &Tensor, transpose1: bool, transpose2: bool, jobs: usize) -> Tensor {
    apply(Op::MatMul { transpose1, transpose2, jobs }, vec![x.clone(), y.clone()])
}

/// The matrix products of the matrices of 3-D tensors, either transposed,
/// the batch spread over `jobs` threads (Go `BMM`).
///
/// # Panics
///
/// With Go's messages for tensors that are not 3-D or do not match.
pub fn bmm(x: &Tensor, y: &Tensor, transpose1: bool, transpose2: bool, jobs: usize) -> Tensor {
    apply(Op::BatchMatMul { transpose1, transpose2, jobs }, vec![x.clone(), y.clone()])
}

/// Each element of `x` repeated over the dimensions `shape` appended to its
/// shape (Go `Broadcast`).
pub fn broadcast(x: &Tensor, shape: &[usize]) -> Tensor {
    apply(Op::Broadcast { shape: shape.to_vec() }, vec![x.clone()])
}

/// The tensor as one dimension, sharing its data (Go `Flatten`).
pub fn flatten(x: &Tensor) -> Tensor {
    apply(Op::Flatten, vec![x.clone()])
}

/// The tensor with another shape of the same size, sharing its data (Go
/// `Reshape`).
///
/// # Panics
///
/// With Go's message when the sizes differ.
pub fn reshape(x: &Tensor, shape: &[usize]) -> Tensor {
    let size1: usize = x.shape().iter().product();
    let size2: usize = shape.iter().product();
    if size1 != size2 {
        panic!("the size of the tensor must be equal to the size of the new shape");
    }
    apply(Op::Reshape { shape: shape.to_vec() }, vec![x.clone()])
}

/// The rows of `w` at the indices in `x`, converted to integers as Go does
/// (Go `Embedding`). Only `w` gets a gradient.
///
/// # Panics
///
/// With Go's messages for an index out of range. As in Go, the rows of a
/// slice of a larger table can be read past its end, up to the end of the
/// table, and on amd64 a NaN or out of range index becomes `i64::MIN`.
pub fn embedding(w: &Tensor, x: &Tensor) -> Tensor {
    apply(Op::Embedding, vec![w.clone(), x.clone()])
}

/// The logistic function, computed as `tanh(x / 2) / 2 + 1/2` (Go
/// `Sigmoid`).
pub fn sigmoid(x: &Tensor) -> Tensor {
    apply(Op::Sigmoid, vec![x.clone()])
}

/// `max(x, 0)` by Go's builtin `max` (Go `ReLu`).
pub fn relu(x: &Tensor) -> Tensor {
    apply(Op::ReLU, vec![x.clone()])
}

/// The softmax along `axis` (Go `Softmax`). As in Go, the maximum and the
/// sum along `axis` are subtracted and divided element `i` by element `i %
/// size` of their own, as for a suffix of the shape, which lines up with
/// `axis` only when it is the first dimension or the others are all 1; and
/// the backward pass multiplies the result in place.
pub fn softmax(x: &Tensor, axis: isize) -> Tensor {
    apply(Op::Softmax { axis }, vec![x.clone()])
}

/// The mean of the squared differences (Go `MeanSquareError`).
pub fn mean_square_error(x: &Tensor, y: &Tensor) -> Tensor {
    mean(&square(&sub(x, y)))
}

/// The mean cross entropy of the softmax of each row of `x` against the
/// class indices in `y` (Go `SoftmaxCrossEntropy`).
///
/// # Panics
///
/// With Go's messages when `x` is not 2-D, `y` not 1-D, or their lengths
/// differ, and for a class index out of range.
pub fn softmax_cross_entropy(x: &Tensor, y: &Tensor) -> Tensor {
    let (sx, sy) = (x.shape(), y.shape());
    if sx.len() != 2 {
        panic!("the shape of the first tensor must be 2-D");
    }
    if sy.len() != 1 {
        panic!("the shape of the second tensor must be 1-D");
    }
    if sx[0] != sy[0] {
        panic!("the size of the first tensor must be equal to the size of the second tensor");
    }
    apply(Op::SoftmaxCrossEntropy, vec![x.clone(), y.clone()])
}

/// The mean binary cross entropy of logits `prediction` against `target`
/// values of -1 or 1, each weighted by `weights` if given (Go
/// `BCEWithLogits`). It is computed in the stable form
///
/// ```text
/// max(prediction, 0) - prediction * y + log(1 + exp(-|prediction|))
/// ```
///
/// where `y = (target + 1) / 2`.
pub fn bce_with_logits(target: &Tensor, prediction: &Tensor, weights: Option<&Tensor>) -> Tensor {
    // term1 = max(prediction, 0)
    let term1 = relu(prediction);
    // y = (target + 1) / 2
    let y = div(&add(&new_scalar(1.0), &[target]), &new_scalar(2.0));
    // term2 = prediction * y
    let term2 = mul(prediction, &y);
    // term3 = log(1 + exp(-|prediction|))
    let abs_prediction = abs(prediction);
    let exp_term = exp(&neg(&abs_prediction));
    let log_term = log(&add(&new_scalar(1.0), &[&exp_term]));
    // loss = max(prediction, 0) - prediction*y + log(1 + exp(-|prediction|))
    let mut loss = add(&sub(&term1, &term2), &[&log_term]);
    if let Some(weights) = weights {
        loss = mul(&loss, weights);
    }
    mean(&loss)
}
