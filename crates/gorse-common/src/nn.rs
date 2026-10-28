// Ports: common/nn @ add21b5

//! A small neural network library with automatic differentiation (Go
//! package `common/nn`), for the factorization machines of `model/ctr`.
//!
//! [`Tensor`]s are `float32` arrays with a shape. The functions in this
//! module ([`add`], [`mat_mul`], [`softmax_cross_entropy`], ...) compute new
//! tensors and record how, so that [`Tensor::backward`] can compute the
//! gradient of a result with respect to every tensor it came from. Layers
//! ([`Layer`]) hold parameters, optimizers ([`Optimizer`]) update them from
//! their gradients, and [`save`] and [`load`] write and read them as Go's
//! `nn.Save` and `nn.Load` do.
//!
//! Results are those of Go bit for bit on amd64, at each of the kernel
//! levels of [`floats`](crate::floats) that `GODEBUG` selects, including
//! Go's quirks: the shape checks of [`add`] and [`pow`], the broadcasting of
//! [`softmax`], reads past the end of a slice of a tensor in [`embedding`]
//! and [`Tensor::slice_indices`], and data shared between tensors and their
//! [`flatten`]ed and [`reshape`]d forms. Panics carry Go's messages. Where Go
//! itself panics while loading a model, [`load`] returns [`Error::Panic`]
//! with the message instead.

mod codec;
mod deps;
mod functions;
mod golang;
mod layers;
mod op;
mod optimizers;
mod tensor;

pub use functions::{
    abs, add, bce_with_logits, bmm, broadcast, cos, div, embedding, exp, flatten, log, mat_mul,
    mean, mean_square_error, mul, neg, pow, relu, reshape, sigmoid, sin, softmax,
    softmax_cross_entropy, square, sub, sum,
};
pub use layers::{
    Attention, EmbeddingLayer, Error, Field, FlattenLayer, Layer, LinearLayer, Model, ReLULayer,
    Reflect, Reflected, Sequential, SigmoidLayer, load, new_attention, new_embedding, new_flatten,
    new_linear, new_relu, new_sequential, new_sigmoid, save,
};
pub use optimizers::{Adam, Optimizer, SGD, new_adam, new_sgd};
pub use tensor::{
    Tensor, lin_space, new_scalar, new_tensor, normal, normal_init, ones, rand, set_inference_mode,
    uniform, zeros,
};

/// A source of random numbers, as Go's `*rand.Rand` is to [`rand`] and
/// [`normal_init`]: `None` in their place uses the process-wide source of
/// Go's top-level functions.
pub trait Rng {
    /// A value in `[0, 1)` (Go `Float32`).
    fn float32(&mut self) -> f32;

    /// A standard normal value (Go `NormFloat64`).
    fn norm_float64(&mut self) -> f64;
}

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tests;
