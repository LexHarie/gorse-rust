// Ports: common/nn/op.go @ add21b5
// Ports: common/nn/tensor.go (Backward) @ add21b5
// Ports: container/heap/heap.go (Push, Pop) @ go1.27.0

//! The operators of the computation graph, their gradients, and the
//! backward pass.

use std::collections::HashSet;
use std::sync::{Arc, Weak};

use crate::floats;
use crate::math32;

use super::golang;
use super::tensor::{
    Inner, Tensor, go_index, inference_mode, modulo, new_scalar, new_tensor, nil_dereference, ones, zeros,
};

/// An operator, with what its forward and backward passes need besides its
/// inputs.
pub(crate) enum Op {
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Sin,
    Cos,
    Square,
    Pow,
    Exp,
    Log,
    Abs,
    Sum,
    PartialSum { along: i64 },
    Mean,
    MatMul { transpose1: bool, transpose2: bool, jobs: usize },
    BatchMatMul { transpose1: bool, transpose2: bool, jobs: usize },
    Broadcast { shape: Vec<usize> },
    Flatten,
    Reshape { shape: Vec<usize> },
    Embedding,
    Sigmoid,
    ReLU,
    Softmax { axis: isize },
    SoftmaxCrossEntropy,
}

impl Op {
    /// Go `String`.
    fn name(&self) -> &'static str {
        match self {
            Op::Neg => "Neg",
            Op::Add => "Add",
            Op::Sub => "Sub",
            Op::Mul => "Mul",
            Op::Div => "Div",
            Op::Sin => "Sin",
            Op::Cos => "Cos",
            Op::Square => "Square",
            Op::Pow => "Pow",
            Op::Exp => "Exp",
            Op::Log => "Log",
            Op::Abs => "Abs",
            Op::Sum | Op::PartialSum { .. } => "Sum",
            Op::Mean => "Mean",
            Op::MatMul { .. } => "MatMul",
            Op::BatchMatMul { .. } => "BatchMatMul",
            Op::Broadcast { .. } => "Broadcast",
            Op::Flatten => "Flatten",
            Op::Reshape { .. } => "Reshape",
            Op::Embedding => "Embedding",
            Op::Sigmoid => "Sigmoid",
            Op::ReLU => "ReLU",
            Op::Softmax { .. } => "Softmax",
            Op::SoftmaxCrossEntropy => "SoftmaxCrossEntropy",
        }
    }
}

/// An operator applied to its inputs: a node of the graph. The output is
/// held weakly, as in Go, so that a graph does not keep its intermediate
/// results alive; the inputs are held.
pub(crate) struct Node {
    op: Op,
    inputs: Vec<Tensor>,
    output: Weak<Inner>,
    pub(crate) generation: usize,
}

/// Go `apply`: runs the operator, and outside inference mode records it as
/// the operator of the result, one generation after its latest input.
pub(crate) fn apply(op: Op, inputs: Vec<Tensor>) -> Tensor {
    let y = forward(&op, &inputs);
    if inference_mode() {
        y.set_op(None);
    } else {
        let generation = inputs.iter().map(Tensor::generation).max().unwrap_or(0) + 1;
        let output = Arc::downgrade(&y.0);
        y.set_op(Some(Arc::new(Node { op, inputs, output, generation })));
    }
    y
}

/// The product of a shape.
fn size(shape: &[usize]) -> usize {
    shape.iter().product()
}

/// Squashes `shape` around `along` into the sizes before it, of it, and
/// after it, as `partialSum` does.
fn squash(shape: &[usize], along: i64) -> (usize, usize, usize) {
    let (mut s1, mut s2, mut s3) = (1, 1, 1);
    for (i, &dim) in shape.iter().enumerate() {
        let i = golang::int(i);
        if i == along {
            s2 = dim;
        } else if i < along {
            s1 *= dim;
        } else {
            s3 *= dim;
        }
    }
    (s1, s2, s3)
}

/// The size of the rows of an embedding table.
fn embedding_dim(w: &Tensor) -> usize {
    let shape = w.shape();
    // `w.shape[1:]` of a scalar.
    golang::slice(1, golang::int(shape.len()), shape.len());
    size(&shape[1..])
}

/// `int(x)` of a `float32` index, as Go converts it on amd64.
fn index_of(x: f32) -> i64 {
    math32::go_f32_to_i64(x)
}

fn forward(op: &Op, inputs: &[Tensor]) -> Tensor {
    let x = &inputs[0];
    match op {
        Op::Neg => x.clone_tensor().neg(),
        Op::Add => x.clone_tensor().add(&inputs[1]),
        Op::Sub => x.clone_tensor().sub(&inputs[1]),
        Op::Mul => x.clone_tensor().mul(&inputs[1]),
        Op::Div => x.clone_tensor().div(&inputs[1]),
        Op::Sin => x.clone_tensor().sin(),
        Op::Cos => x.clone_tensor().cos(),
        Op::Square => x.clone_tensor().square(),
        Op::Pow => x.clone_tensor().pow(&inputs[1]),
        Op::Exp => x.clone_tensor().exp(),
        Op::Log => x.clone_tensor().log(),
        Op::Abs => {
            let y = x.clone_tensor();
            y.write(|d| {
                for v in d {
                    if *v < 0.0 {
                        *v = -*v;
                    }
                }
            });
            y
        }
        Op::Sum => new_tensor(vec![x.read(|d| d.iter().fold(0.0, |s, &v| s + v))], &[]),
        Op::PartialSum { along } => {
            let shape = x.shape();
            let (s1, s2, s3) = squash(&shape, *along);
            let out_shape: Vec<usize> = shape
                .iter()
                .enumerate()
                .filter(|&(i, _)| golang::int(i) != *along)
                .map(|(_, &d)| d)
                .collect();
            let mut data = vec![0.0; s1 * s3];
            x.read(|d| {
                for i in 0..s1 {
                    for j in 0..s2 {
                        for k in 0..s3 {
                            data[i * s3 + k] += d[i * s2 * s3 + j * s3 + k];
                        }
                    }
                }
            });
            new_tensor(data, &out_shape)
        }
        Op::Mean => {
            let (sum, n) = x.read(|d| (d.iter().fold(0.0, |s, &v| s + v), d.len()));
            new_tensor(vec![sum / n as f32], &[])
        }
        Op::MatMul { transpose1, transpose2, jobs } => x.mat_mul(&inputs[1], *transpose1, *transpose2, *jobs),
        Op::BatchMatMul { transpose1, transpose2, jobs } => {
            x.batch_mat_mul(&inputs[1], *transpose1, *transpose2, *jobs)
        }
        Op::Broadcast { shape } => {
            let mut out_shape = x.shape();
            out_shape.extend_from_slice(shape);
            let mut data = vec![0.0; size(&out_shape)];
            let w_size = size(shape);
            x.read(|d| {
                for (i, &v) in d.iter().enumerate() {
                    for y in &mut data[i * w_size..(i + 1) * w_size] {
                        *y = v;
                    }
                }
            });
            new_tensor(data, &out_shape)
        }
        Op::Flatten => {
            let view = x.view();
            let len = view.len();
            Tensor::share(view, &[len])
        }
        Op::Reshape { shape } => Tensor::share(x.view(), shape),
        Op::Embedding => {
            let (w, x) = (&inputs[0], &inputs[1]);
            let dim = embedding_dim(w);
            let mut shape = x.shape();
            shape.extend_from_slice(&w.shape()[1..]);
            let mut data = vec![0.0; size(&shape)];
            let size = data.len();
            let dim_go = golang::int(dim);
            for (i, &v) in x.data().iter().enumerate() {
                let index = index_of(v);
                let i = golang::int(i);
                let dst = golang::slice(i * dim_go, (i + 1) * dim_go, size);
                let low = index.wrapping_mul(dim_go);
                let high = index.wrapping_add(1).wrapping_mul(dim_go);
                w.read_slice(low, high, |src| {
                    // `copy` copies as many as both have.
                    let n = dst.len().min(src.len());
                    data[dst.start..dst.start + n].copy_from_slice(&src[..n]);
                });
            }
            new_tensor(data, &shape)
        }
        Op::Sigmoid => {
            // y = tanh(x * 0.5) * 0.5 + 0.5
            let y = x.clone_tensor();
            y.mul(&new_scalar(0.5));
            y.tanh();
            y.mul(&new_scalar(0.5));
            y.add(&new_scalar(0.5))
        }
        Op::ReLU => {
            let y = x.clone_tensor();
            y.maximum(&new_scalar(0.0));
            y
        }
        Op::Softmax { axis } => {
            let y = x.clone_tensor();
            y.sub(&x.max(*axis, true));
            y.exp();
            y.div(&y.sum(*axis, true))
        }
        Op::SoftmaxCrossEntropy => {
            let t = &inputs[1];
            let m = x.max(1, true);
            let s = x.clone_tensor().b_sub(&m); // x - m
            let s = s.exp(); // exp(x - m)
            let s = s.sum(1, true); // sum(exp(x - m))
            s.log(); // log(sum(exp(x - m)))
            m.add(&s); // m + log(sum(exp(x - m)))
            let log_p = x.clone_tensor().b_sub(&m); // x - (m + log(sum(exp(x - m))))
            let labels = t.data();
            let mut cross_entropy = 0.0f32;
            for (i, &label) in labels.iter().enumerate() {
                cross_entropy -= log_p.get_go(&[golang::int(i), index_of(label)]);
            }
            cross_entropy /= labels.len() as f32;
            new_scalar(cross_entropy)
        }
    }
}

impl Node {
    fn output(&self) -> Tensor {
        self.output.upgrade().map_or_else(|| nil_dereference(), Tensor::from_inner)
    }

    /// The gradients of the inputs, from the gradient `dy` of the output.
    fn backward(&self, dy: &Tensor) -> Vec<Tensor> {
        let inputs = &self.inputs;
        let x = &inputs[0];
        match &self.op {
            Op::Neg => vec![dy.clone_tensor().neg()],
            Op::Add | Op::Sub => {
                let gx0 = dy.clone_tensor();
                let gx1 = zeros(&inputs[1].shape());
                let w_size = gx1.len();
                let sub = matches!(self.op, Op::Sub);
                let dy = dy.data();
                gx1.write(|g| {
                    for (i, &d) in dy.iter().enumerate() {
                        let j = modulo(i, w_size);
                        if sub {
                            g[j] -= d;
                        } else {
                            g[j] += d;
                        }
                    }
                });
                vec![gx0, gx1]
            }
            Op::Mul => {
                let gx0 = dy.clone_tensor();
                gx0.mul(&inputs[1]);
                let gx1 = zeros(&inputs[1].shape());
                let w_size = gx1.len();
                let (dy, x0) = (dy.data(), x.data());
                gx1.write(|g| {
                    for (i, &d) in dy.iter().enumerate() {
                        g[modulo(i, w_size)] += d * x0[go_index(i, x0.len())];
                    }
                });
                vec![gx0, gx1]
            }
            Op::Div => {
                let w_size = size(&inputs[1].shape());
                let (dy, x0, x1) = (dy.data(), x.data(), inputs[1].data());
                let gx0 = zeros(&x.shape());
                gx0.write(|g| {
                    for (i, &d) in dy.iter().enumerate() {
                        g[go_index(i, g.len())] = d / x1[go_index(modulo(i, w_size), x1.len())];
                    }
                });
                let gx1 = zeros(&inputs[1].shape());
                gx1.write(|g| {
                    for (i, &d) in dy.iter().enumerate() {
                        let j = modulo(i, w_size);
                        let x1j = x1[go_index(j, x1.len())];
                        g[go_index(j, g.len())] -= d * x0[go_index(i, x0.len())] / x1j / x1j;
                    }
                });
                vec![gx0, gx1]
            }
            Op::Sin => vec![x.clone_tensor().cos().mul(dy)],
            Op::Cos => vec![x.clone_tensor().sin().neg().mul(dy)],
            Op::Square => {
                let dx = x.clone_tensor();
                let dy = dy.data();
                dx.write(|d| {
                    let a = d.to_vec();
                    floats::mul_to(&a, &dy, d);
                    floats::mul_const(d, 2.0);
                });
                vec![dx]
            }
            Op::Pow => {
                let n = &inputs[1];
                let dx0 = x.clone_tensor();
                dx0.pow(n);
                dx0.mul(n);
                dx0.div(x);
                dx0.mul(dy);
                let w_size = size(&n.shape());
                let dx1 = zeros(&n.shape());
                let (dy, y, x0) = (dy.data(), self.output().data(), x.data());
                dx1.write(|g| {
                    for (i, &d) in dy.iter().enumerate() {
                        let j = go_index(modulo(i, w_size), g.len());
                        g[j] += d * y[go_index(i, y.len())] * math32::log(x0[go_index(i, x0.len())]);
                    }
                });
                vec![dx0, dx1]
            }
            Op::Exp => vec![x.clone_tensor().exp().mul(dy)],
            Op::Log => vec![dy.clone_tensor().div(x)],
            Op::Abs => {
                let dx = dy.clone_tensor();
                let x = x.data();
                dx.write(|d| {
                    for (i, v) in d.iter_mut().enumerate() {
                        if x[go_index(i, x.len())] < 0.0 {
                            *v = -*v;
                        }
                    }
                });
                vec![dx]
            }
            Op::Sum | Op::Mean => {
                let dx = zeros(&x.shape());
                let d0 = dy.read(|d| d[go_index(0, d.len())]);
                dx.write(|d| {
                    let v = if matches!(self.op, Op::Mean) { d0 / d.len() as f32 } else { d0 };
                    d.fill(v);
                });
                vec![dx]
            }
            Op::PartialSum { along } => {
                let (s1, s2, s3) = squash(&x.shape(), *along);
                let dx = zeros(&x.shape());
                let dy = dy.data();
                dx.write(|d| {
                    for i in 0..s1 {
                        for j in 0..s2 {
                            for k in 0..s3 {
                                d[i * s2 * s3 + j * s3 + k] = dy[i * s3 + k];
                            }
                        }
                    }
                });
                vec![dx]
            }
            Op::MatMul { transpose1, transpose2, jobs } => {
                let (x0, x1, jobs) = (x, &inputs[1], *jobs);
                let (dx0, dx1) = match (transpose1, transpose2) {
                    // y = x0 * x1: dx0 = dy * x1^T, dx1 = x0^T * dy
                    (false, false) => (dy.mat_mul(x1, false, true, jobs), x0.mat_mul(dy, true, false, jobs)),
                    // y = x0^T * x1: dx0 = x1 * dy^T, dx1 = x0 * dy
                    (true, false) => (x1.mat_mul(dy, false, true, jobs), x0.mat_mul(dy, false, false, jobs)),
                    // y = x0 * x1^T: dx0 = dy * x1, dx1 = dy^T * x0
                    (false, true) => (dy.mat_mul(x1, false, false, jobs), dy.mat_mul(x0, true, false, jobs)),
                    // y = x0^T * x1^T: dx0 = x1^T * dy^T, dx1 = dy^T * x0^T
                    (true, true) => (x1.mat_mul(dy, true, true, jobs), dy.mat_mul(x0, true, true, jobs)),
                };
                vec![dx0, dx1]
            }
            Op::BatchMatMul { transpose1, transpose2, jobs } => {
                let (x0, x1, jobs) = (x, &inputs[1], *jobs);
                let (dx0, dx1) = match (transpose1, transpose2) {
                    (false, false) => (
                        dy.batch_mat_mul(x1, false, true, jobs),
                        x0.batch_mat_mul(dy, true, false, jobs),
                    ),
                    (true, false) => (
                        x1.batch_mat_mul(dy, false, true, jobs),
                        x0.batch_mat_mul(dy, false, false, jobs),
                    ),
                    (false, true) => (
                        dy.batch_mat_mul(x1, false, false, jobs),
                        dy.batch_mat_mul(x0, true, false, jobs),
                    ),
                    (true, true) => (
                        x1.batch_mat_mul(dy, true, true, jobs),
                        dy.batch_mat_mul(x0, true, true, jobs),
                    ),
                };
                vec![dx0, dx1]
            }
            Op::Broadcast { shape } => {
                let gx = zeros(&x.shape());
                let w_size = size(shape);
                let dy = dy.data();
                gx.write(|g| {
                    for (i, v) in g.iter_mut().enumerate() {
                        for &d in &dy[i * w_size..(i + 1) * w_size] {
                            *v += d;
                        }
                    }
                });
                vec![gx]
            }
            Op::Flatten | Op::Reshape { .. } => vec![Tensor::share(dy.view(), &x.shape())],
            Op::Embedding => {
                let (w, x) = (&inputs[0], &inputs[1]);
                let dim = embedding_dim(w);
                let dim_go = golang::int(dim);
                let dw = zeros(&w.shape());
                let dy = dy.data();
                let x = x.data();
                dw.write(|g| {
                    for (i, &v) in x.iter().enumerate() {
                        let index = index_of(v).wrapping_mul(dim_go);
                        for j in 0..dim {
                            let k = index.wrapping_add(golang::int(j));
                            g[golang::index(k, g.len())] += dy[go_index(i * dim + j, dy.len())];
                        }
                    }
                });
                vec![dw]
            }
            Op::Sigmoid => {
                // dx = dy * y * (1 - y)
                let y = self.output();
                let dx = y.clone_tensor();
                dx.neg();
                dx.add(&new_scalar(1.0));
                dx.mul(&y);
                dx.mul(dy);
                vec![dx]
            }
            Op::ReLU => vec![x.clone_tensor().gt(&new_scalar(0.0)).mul(dy)],
            Op::Softmax { axis } => {
                let y = self.output();
                let gx = y.clone_tensor();
                gx.mul(dy);
                let sum_dx = gx.sum(*axis, true);
                // Go multiplies the output in place.
                y.mul(&sum_dx);
                gx.sub(&y);
                vec![gx]
            }
            Op::SoftmaxCrossEntropy => {
                let t = &inputs[1];
                let labels = t.data();
                // gy *= 1/N
                let gy = dy.clone_tensor().mul(&new_scalar(1.0 / labels.len() as f32));
                // y = softmax(x)
                let y = x.clone_tensor();
                y.b_sub(&x.max(1, true));
                y.exp();
                y.b_div(&y.sum(1, true));
                // convert to one-hot
                let shape = x.shape();
                let one_hot = zeros(&shape);
                let cols = golang::int(shape[1]);
                one_hot.write(|d| {
                    for (i, &label) in labels.iter().enumerate() {
                        let k = golang::int(i).wrapping_mul(cols).wrapping_add(index_of(label));
                        d[golang::index(k, d.len())] = 1.0;
                    }
                });
                // y = (y - t_onehot) * gy
                let y = y.sub(&one_hot).mul(&gy);
                vec![y, zeros(&t.shape())]
            }
        }
    }
}

/// Go's `opHeap` under `container/heap`: a max-heap of nodes by generation.
struct Heap(Vec<Option<Arc<Node>>>);

impl Heap {
    fn generation(&self, i: usize) -> usize {
        self.0[i].as_ref().map_or(0, |node| node.generation)
    }

    /// `opHeap.Less`.
    fn less(&self, i: usize, j: usize) -> bool {
        self.generation(i) > self.generation(j)
    }

    fn up(&mut self, mut j: usize) {
        while j > 0 {
            let i = (j - 1) / 2;
            if !self.less(j, i) {
                break;
            }
            self.0.swap(i, j);
            j = i;
        }
    }

    fn down(&mut self, i0: usize, n: usize) {
        let mut i = i0;
        loop {
            let j1 = 2 * i + 1;
            if j1 >= n {
                break;
            }
            let mut j = j1;
            let j2 = j1 + 1;
            if j2 < n && self.less(j2, j1) {
                j = j2;
            }
            if !self.less(j, i) {
                break;
            }
            self.0.swap(i, j);
            i = j;
        }
    }

    /// `heap.Push`.
    fn push(&mut self, node: Arc<Node>) {
        self.0.push(Some(node));
        self.up(self.0.len() - 1);
    }

    /// `heap.Pop` of a non-empty heap.
    fn pop(&mut self) -> Option<Arc<Node>> {
        let n = self.0.len() - 1;
        self.0.swap(0, n);
        self.down(0, n);
        self.0.pop().flatten()
    }
}

/// A key for the identity of a node.
fn key(node: &Arc<Node>) -> usize {
    Arc::as_ptr(node) as usize
}

/// Go `(*Tensor).Backward`.
pub(crate) fn backward(t: &Tensor) {
    t.set_grad(Some(ones(&t.shape())));
    let root = t.op();
    let mut seen: HashSet<usize> = root.iter().map(key).collect();
    // Holds what `seen` names, so that no address is reused meanwhile.
    let mut visited: Vec<Arc<Node>> = root.iter().cloned().collect();
    let mut ops = Heap(vec![root]);
    while !ops.0.is_empty() {
        let Some(node) = ops.pop() else {
            // The type assertion `heap.Pop(ops).(op)` of the nil operator of
            // a leaf.
            panic!("interface conversion: interface is nil, not nn.op");
        };
        let output = node.output();
        let dy = output.grad().unwrap_or_else(|| nil_dereference());
        let grads = node.backward(&dy);
        for (input, grad) in node.inputs.iter().zip(grads) {
            let (input_shape, grad_shape) = (input.shape(), grad.shape());
            if input_shape != grad_shape {
                panic!(
                    "{}: shape {} does not match shape {}",
                    node.op.name(),
                    golang::format_ints(&input_shape),
                    golang::format_ints(&grad_shape)
                );
            }
            match input.grad() {
                None => input.set_grad(Some(grad)),
                Some(g) => {
                    g.add(&grad);
                }
            }
            if let Some(op) = input.op()
                && seen.insert(key(&op))
            {
                visited.push(Arc::clone(&op));
                ops.push(op);
            }
        }
        output.set_grad(None);
    }
}

