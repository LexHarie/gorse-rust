// Ports: common/nn/optimizers.go @ add21b5

//! Optimizers: they update parameters in place from their gradients.

use std::collections::HashMap;

use crate::floats;
use crate::math32;

use super::golang;
use super::tensor::{Tensor, nil_dereference, run_parts, zeros};

/// An optimizer (Go `Optimizer`).
pub trait Optimizer: Send {
    /// Sets the weight decay, added to each gradient times the parameter.
    fn set_weight_decay(&mut self, rate: f32);

    /// Sets the number of threads a step splits each parameter between.
    fn set_jobs(&mut self, jobs: usize);

    /// Clears the gradients of the parameters.
    fn zero_grad(&mut self);

    /// Updates the parameters from their gradients.
    ///
    /// # Panics
    ///
    /// When a parameter with elements has no gradient, with the message of
    /// the nil dereference that crashes a Go program there (in a goroutine,
    /// which no `recover` can catch).
    fn step(&mut self);
}

/// Go `baseOptimizer`.
struct Base {
    params: Vec<Tensor>,
    wd: f32,
    jobs: usize,
}

impl Base {
    fn new(params: Vec<Tensor>) -> Base {
        Base {
            params,
            wd: 0.0,
            jobs: 0,
        }
    }

    /// The size of the buffers: the largest parameter.
    fn buf_size(&self) -> usize {
        self.params.iter().map(Tensor::len).max().unwrap_or(0)
    }

    fn zero_grad(&self) {
        for p in &self.params {
            p.set_grad(None);
        }
    }
}

/// Splits `parts` of `0..n` from mutable slices of length `n`.
fn split<'a>(parts: &[(usize, usize)], mut rest: &'a mut [f32]) -> Vec<&'a mut [f32]> {
    let mut start = 0;
    parts
        .iter()
        .map(|&(i, j)| {
            debug_assert_eq!(i, start);
            let (part, tail) = std::mem::take(&mut rest).split_at_mut(j - start);
            rest = tail;
            start = j;
            part
        })
        .collect()
}

/// `s[i:j]` of each part, mutable, with Go's bounds checks against the
/// length of `s` as its capacity: the parts are contiguous from 0.
fn split_go<'a>(parts: &[(usize, usize)], s: &'a mut [f32]) -> Vec<&'a mut [f32]> {
    if let Some(&(_, end)) = parts.last() {
        golang::slice(0, golang::int(end), s.len());
    }
    split(parts, s)
}

/// `s[i:j]` of each part, with Go's bounds checks against the length of `s`
/// as its capacity.
fn slices<'a>(parts: &[(usize, usize)], s: &'a [f32]) -> Vec<&'a [f32]> {
    parts
        .iter()
        .map(|&(i, j)| &s[golang::slice(golang::int(i), golang::int(j), s.len())])
        .collect()
}

/// Go's `s.b[:len(p.data)]` of a buffer.
fn prefix(b: &mut [f32], n: usize) -> &mut [f32] {
    let len = b.len();
    &mut b[golang::slice(0, golang::int(n), len)]
}

/// Stochastic gradient descent (Go `SGD`).
pub struct SGD {
    base: Base,
    lr: f32,
    b: Vec<f32>,
}

/// Stochastic gradient descent with learning rate `lr` (Go `NewSGD`).
pub fn new_sgd(params: Vec<Tensor>, lr: f32) -> SGD {
    let base = Base::new(params);
    let b = vec![0.0; base.buf_size()];
    SGD { base, lr, b }
}

impl Optimizer for SGD {
    fn set_weight_decay(&mut self, rate: f32) {
        self.base.wd = rate;
    }

    fn set_jobs(&mut self, jobs: usize) {
        self.base.jobs = jobs;
    }

    fn zero_grad(&mut self) {
        self.base.zero_grad();
    }

    /// `p -= lr * (grad + wd * p)` for each parameter, in parts of multiples
    /// of 32 elements on separate threads.
    fn step(&mut self) {
        let (wd, lr) = (self.base.wd, self.lr);
        for p in &self.base.params {
            let n = p.len();
            let b = prefix(&mut self.b, n);
            let parts = partition_aligned(golang::int(n), golang::int(self.base.jobs), 32);
            if parts.is_empty() {
                continue;
            }
            let grad = p.grad().unwrap_or_else(|| nil_dereference());
            p.update_from(&grad, |p, g| {
                let jobs = split(&parts, p)
                    .into_iter()
                    .zip(slices(&parts, g))
                    .zip(split(&parts, b));
                run_parts(jobs.collect(), |((p, g), b)| {
                    floats::mul_const_add_to(p, wd, g, b);
                    floats::mul_const_add(b, -lr, p);
                });
            });
        }
    }
}

/// The moments of a parameter, which is kept so that its address names it.
struct Moments {
    _param: Tensor,
    m: Tensor,
    v: Tensor,
}

/// Adam (Go `Adam`).
pub struct Adam {
    base: Base,
    alpha: f32,
    beta1: f32,
    beta2: f32,
    eps: f32,
    moments: HashMap<usize, Moments>,
    t: f32,
    b1: Vec<f32>,
    b2: Vec<f32>,
}

/// Adam with learning rate `alpha`, β₁ 0.9, β₂ 0.999 and ε 1e-8 (Go
/// `NewAdam`).
pub fn new_adam(params: Vec<Tensor>, alpha: f32) -> Adam {
    let base = Base::new(params);
    let size = base.buf_size();
    Adam {
        base,
        alpha,
        beta1: 0.9,
        beta2: 0.999,
        eps: 1e-8,
        moments: HashMap::new(),
        t: 0.0,
        b1: vec![0.0; size],
        b2: vec![0.0; size],
    }
}

impl Optimizer for Adam {
    fn set_weight_decay(&mut self, rate: f32) {
        self.base.wd = rate;
    }

    fn set_jobs(&mut self, jobs: usize) {
        self.base.jobs = jobs;
    }

    fn zero_grad(&mut self) {
        self.base.zero_grad();
    }

    /// One Adam step for each parameter, in parts of multiples of 32
    /// elements on separate threads. A parameter listed twice has one pair
    /// of moments, updated twice.
    fn step(&mut self) {
        self.t += 1.0;
        let fix1 = 1.0 - math32::pow(self.beta1, self.t);
        let fix2 = 1.0 - math32::pow(self.beta2, self.t);
        let lr = self.alpha * math32::sqrt(fix2) / fix1;
        let (wd, beta1, beta2, eps) = (self.base.wd, self.beta1, self.beta2, self.eps);
        for p in &self.base.params {
            let moments = self.moments.entry(p.id()).or_insert_with(|| Moments {
                _param: p.clone(),
                m: zeros(&p.shape()),
                v: zeros(&p.shape()),
            });
            let (m, v) = (&moments.m, &moments.v);
            let n = p.len();
            let (b1, b2) = (prefix(&mut self.b1, n), prefix(&mut self.b2, n));
            let parts = partition_aligned(golang::int(n), golang::int(self.base.jobs), 32);
            if parts.is_empty() {
                continue;
            }
            let grad = p.grad().unwrap_or_else(|| nil_dereference());
            m.write_cap(|m| {
                v.write_cap(|v| {
                    p.update_from(&grad, |p, g| {
                        let jobs = split(&parts, p)
                            .into_iter()
                            .zip(slices(&parts, g))
                            .zip(split_go(&parts, m))
                            .zip(split_go(&parts, v))
                            .zip(split(&parts, b1))
                            .zip(split(&parts, b2));
                        run_parts(jobs.collect(), |(((((p, g), m), v), b1), b2)| {
                            // grad = grad + wd * param.data
                            floats::mul_const_add_to(p, wd, g, b1);
                            // m += (1 - beta1) * (grad - m)
                            floats::sub_to(b1, m, b2);
                            floats::mul_const_add(b2, 1.0 - beta1, m);
                            // v += (1 - beta2) * (grad * grad - v)
                            floats::mul_to(b1, b1, b2);
                            floats::sub(b2, v);
                            floats::mul_const_add(b2, 1.0 - beta2, v);
                            // param.data -= self.lr * m / (xp.sqrt(v) + eps)
                            floats::sqrt_to(v, b2);
                            floats::add_const(b2, eps);
                            floats::div_to(m, b2, b1);
                            floats::mul_const_add(b1, -lr, p);
                        });
                    });
                });
            });
        }
    }
}

/// Splits `0..n` into parts of `n / m` elements rounded up to a multiple of
/// `k`, the last part shorter (Go `partitionAligned`): `(10, 3, 2)` gives
/// `[(0, 4), (4, 8), (8, 10)]`. Go `int`s, negative ones included.
pub(crate) fn partition_aligned(n: i64, m: i64, mut k: i64) -> Vec<(usize, usize)> {
    if n <= 0 {
        return Vec::new();
    }
    let len = n.cast_unsigned() as usize;
    if m <= 0 {
        return vec![(0, len)];
    }
    if k <= 0 {
        k = 1;
    }
    // The size of each part.
    let mut part_size = n / m;
    if part_size % k != 0 {
        part_size += k - part_size % k;
    }
    if part_size == 0 {
        part_size = k;
    }
    let part_size = part_size.cast_unsigned() as usize;
    let mut parts = Vec::new();
    let mut start = 0;
    while start < len {
        let end = (start + part_size).min(len);
        parts.push((start, end));
        start = end;
    }
    parts
}
