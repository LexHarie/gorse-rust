// Ports: common/nn/tensor.go @ add21b5

//! The tensor type, its constructors and the in-place arithmetic the
//! operators are built from.

use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::floats;
use crate::math32;

use super::golang;
use super::op::Node;
use super::{Rng, deps};

/// Disables gradient computation when enabled, for model evaluation.
static INFERENCE_MODE: AtomicBool = AtomicBool::new(false);

/// Enables or disables inference mode, in which operators record no graph,
/// so that `backward` cannot run through them (Go `SetInferenceMode`). The
/// mode is process-wide, as in Go.
pub fn set_inference_mode(enabled: bool) {
    INFERENCE_MODE.store(enabled, Ordering::SeqCst);
}

pub(crate) fn inference_mode() -> bool {
    INFERENCE_MODE.load(Ordering::SeqCst)
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

/// A Go slice of `float32`: a window of `len` elements at `offset` in a
/// shared buffer. As in Go, slicing beyond `len` up to the end of the buffer
/// (the capacity) is allowed where Go code does it.
#[derive(Clone)]
pub(crate) struct View {
    buf: Arc<RwLock<Vec<f32>>>,
    offset: usize,
    len: usize,
    cap: usize,
    pub(crate) shape: Vec<usize>,
}

impl View {
    fn new(data: Vec<f32>, shape: Vec<usize>) -> View {
        let len = data.len();
        View {
            buf: Arc::new(RwLock::new(data)),
            offset: 0,
            len,
            cap: len,
            shape,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// `data[low:high]` as a new window of the same buffer.
    fn reslice(&self, low: usize, high: usize, shape: Vec<usize>) -> View {
        let r = golang::slice(golang::int(low), golang::int(high), self.cap);
        View {
            buf: Arc::clone(&self.buf),
            offset: self.offset + r.start,
            len: r.len(),
            cap: self.cap - r.start,
            shape,
        }
    }

    fn read<R>(&self, f: impl FnOnce(&[f32]) -> R) -> R {
        f(&read(&self.buf)[self.offset..self.offset + self.len])
    }

    /// `data[low:high]`, which may reach past `len` up to the capacity.
    fn read_slice<R>(&self, low: i64, high: i64, f: impl FnOnce(&[f32]) -> R) -> R {
        let r = golang::slice(low, high, self.cap);
        f(&read(&self.buf)[self.offset + r.start..self.offset + r.end])
    }

    fn write<R>(&self, f: impl FnOnce(&mut [f32]) -> R) -> R {
        f(&mut write(&self.buf)[self.offset..self.offset + self.len])
    }

    fn to_vec(&self) -> Vec<f32> {
        self.read(<[f32]>::to_vec)
    }
}

pub(crate) struct Inner {
    view: RwLock<View>,
    grad: Mutex<Option<Tensor>>,
    op: Mutex<Option<Arc<Node>>>,
}

/// A tensor (Go `*nn.Tensor`): `float32` data with a shape, the gradient that
/// `backward` accumulates, and the operator that produced it.
///
/// A `Tensor` is a handle: cloning it gives another handle to the same
/// tensor, as copying a Go pointer does. Tensors made by [`Tensor::slice`],
/// [`flatten`](super::flatten) and [`reshape`](super::reshape) share their
/// data with the tensor they come from, as in Go, so writes through one (an
/// optimizer step, say) show through the others. Handles can be sent between
/// threads; the data is behind a lock, so concurrent use is safe where Go's
/// would be a data race.
#[derive(Clone)]
pub struct Tensor(pub(crate) Arc<Inner>);

impl Tensor {
    pub(crate) fn from_view(view: View) -> Tensor {
        Tensor(Arc::new(Inner {
            view: RwLock::new(view),
            grad: Mutex::new(None),
            op: Mutex::new(None),
        }))
    }

    /// `&Tensor{data: data, shape: shape}`, without checking the size.
    pub(crate) fn raw(data: Vec<f32>, shape: Vec<usize>) -> Tensor {
        Tensor::from_view(View::new(data, shape))
    }

    /// `NewTensor(data, shape...)` of the data of `view`, shared.
    ///
    /// # Panics
    ///
    /// With Go's message when the data does not have the shape's size.
    pub(crate) fn share(view: View, shape: &[usize]) -> Tensor {
        check_size(view.len, shape);
        Tensor::from_view(View { shape: shape.to_vec(), ..view })
    }

    /// Go `fromPB`: new data and shape, not checked against each other.
    pub(crate) fn set_data(&self, data: Vec<f32>, shape: Vec<usize>) {
        self.set_view(View::new(data, shape));
    }

    pub(crate) fn view(&self) -> View {
        read(&self.0.view).clone()
    }

    pub(crate) fn set_view(&self, view: View) {
        *write(&self.0.view) = view;
    }

    /// A key for the identity of the tensor, as a Go pointer is one.
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    pub(crate) fn from_inner(inner: Arc<Inner>) -> Tensor {
        Tensor(inner)
    }

    pub(crate) fn len(&self) -> usize {
        self.view().len
    }

    pub(crate) fn read<R>(&self, f: impl FnOnce(&[f32]) -> R) -> R {
        self.view().read(f)
    }

    pub(crate) fn write<R>(&self, f: impl FnOnce(&mut [f32]) -> R) -> R {
        self.view().write(f)
    }

    /// `t.data[low:high]`, which Go allows up to the capacity.
    pub(crate) fn read_slice<R>(&self, low: i64, high: i64, f: impl FnOnce(&[f32]) -> R) -> R {
        self.view().read_slice(low, high, f)
    }

    /// Reads two tensors at once, locking their buffers in address order.
    pub(crate) fn read2<R>(a: &Tensor, b: &Tensor, f: impl FnOnce(&[f32], &[f32]) -> R) -> R {
        let (va, vb) = (a.view(), b.view());
        let (ra, rb) = (va.offset..va.offset + va.len, vb.offset..vb.offset + vb.len);
        if Arc::ptr_eq(&va.buf, &vb.buf) {
            let g = read(&va.buf);
            return f(&g[ra], &g[rb]);
        }
        if Arc::as_ptr(&va.buf) < Arc::as_ptr(&vb.buf) {
            let ga = read(&va.buf);
            let gb = read(&vb.buf);
            f(&ga[ra], &gb[rb])
        } else {
            let gb = read(&vb.buf);
            let ga = read(&va.buf);
            f(&ga[ra], &gb[rb])
        }
    }

    /// Runs `f` on the data up to the capacity, written: Go's `t.data[i:j]`
    /// may reach past the length.
    pub(crate) fn write_cap<R>(&self, f: impl FnOnce(&mut [f32]) -> R) -> R {
        let view = self.view();
        f(&mut write(&view.buf)[view.offset..view.offset + view.cap])
    }

    /// Runs `f` on this tensor's data, written, and on `other`'s up to its
    /// capacity, read, as Go slices `other.data[i:j]` for `j` up to the
    /// length of this one. Where both share a buffer, `other` is read before
    /// the write.
    pub(crate) fn update_from<R>(&self, other: &Tensor, f: impl FnOnce(&mut [f32], &[f32]) -> R) -> R {
        let (va, vb) = (self.view(), other.view());
        let (ra, rb) = (va.offset..va.offset + va.len, vb.offset..vb.offset + vb.cap);
        if Arc::ptr_eq(&va.buf, &vb.buf) {
            let snapshot = read(&vb.buf)[rb].to_vec();
            return f(&mut write(&va.buf)[ra], &snapshot);
        }
        Tensor::locked(&va, ra, &vb, rb, f)
    }

    /// Runs `f` on `a`'s buffer over `ra`, written, and `b`'s over `rb`, read:
    /// two different buffers, locked in address order.
    fn locked<R>(
        a: &View,
        ra: Range<usize>,
        b: &View,
        rb: Range<usize>,
        f: impl FnOnce(&mut [f32], &[f32]) -> R,
    ) -> R {
        if Arc::as_ptr(&a.buf) < Arc::as_ptr(&b.buf) {
            let mut ga = write(&a.buf);
            let gb = read(&b.buf);
            f(&mut ga[ra], &gb[rb])
        } else {
            let gb = read(&b.buf);
            let mut ga = write(&a.buf);
            f(&mut ga[ra], &gb[rb])
        }
    }

    /// `t.data[i] = f(t.data[i], other.data[index(i)])` for each element in
    /// order, as Go's loops run, with Go's message for an index past
    /// `other`. Where both share a buffer, each element of `other` is read as
    /// the loop reaches it, so earlier writes show through as in Go.
    pub(crate) fn combine(&self, other: &Tensor, index: impl Fn(usize) -> usize, f: impl Fn(f32, f32) -> f32) {
        let (va, vb) = (self.view(), other.view());
        if Arc::ptr_eq(&va.buf, &vb.buf) {
            let mut g = write(&va.buf);
            for i in 0..va.len {
                let o = g[vb.offset + go_index(index(i), vb.len)];
                let x = &mut g[va.offset + i];
                *x = f(*x, o);
            }
            return;
        }
        let (ra, rb) = (va.offset..va.offset + va.len, vb.offset..vb.offset + vb.len);
        Tensor::locked(&va, ra, &vb, rb, |t, o| {
            for (i, x) in t.iter_mut().enumerate() {
                *x = f(*x, o[go_index(index(i), o.len())]);
            }
        });
    }

    pub(crate) fn op(&self) -> Option<Arc<Node>> {
        lock(&self.0.op).clone()
    }

    pub(crate) fn set_op(&self, op: Option<Arc<Node>>) {
        *lock(&self.0.op) = op;
    }

    pub(crate) fn set_grad(&self, grad: Option<Tensor>) {
        *lock(&self.0.grad) = grad;
    }

    /// The generation of the operator that produced this tensor, 0 for a
    /// leaf.
    pub(crate) fn generation(&self) -> usize {
        self.op().map_or(0, |op| op.generation)
    }

    /// Whether this tensor has no dimensions (Go `IsScalar`).
    pub fn is_scalar(&self) -> bool {
        self.view().shape.is_empty()
    }

    /// Makes this tensor a leaf, cutting it from the operator that produced
    /// it, and returns it (Go `NoGrad`).
    pub fn no_grad(&self) -> Tensor {
        self.set_op(None);
        self.clone()
    }

    /// The shape (Go `Shape`).
    pub fn shape(&self) -> Vec<usize> {
        self.view().shape
    }

    /// Rows `start..end` of the first dimension, sharing this tensor's data
    /// (Go `Slice`).
    ///
    /// # Panics
    ///
    /// With Go's messages, on a tensor without dimensions, when `end` is past
    /// the first dimension, and when `start > end`.
    pub fn slice(&self, start: usize, end: usize) -> Tensor {
        let view = self.view();
        if view.shape.is_empty() {
            panic!("slice requires at least 1-D tensor");
        }
        if end > view.shape[0] {
            panic!("slice out of range");
        }
        let sub_size: usize = view.shape[1..].iter().product();
        let (low, high) = (start * sub_size, end * sub_size);
        let mut shape = vec![end - start];
        shape.extend_from_slice(&view.shape[1..]);
        Tensor::from_view(view.reslice(low, high, shape))
    }

    /// The rows at `indices` of the first dimension, copied (Go
    /// `SliceIndices`). As in Go, a row past the end of a slice is read from
    /// the tensor it was sliced from, up to the end of the shared data.
    ///
    /// # Panics
    ///
    /// With Go's messages, on a tensor without dimensions and on a row past
    /// the shared data.
    pub fn slice_indices(&self, indices: &[usize]) -> Tensor {
        let view = self.view();
        if view.shape.is_empty() {
            panic!("runtime error: slice bounds out of range [1:0]");
        }
        let mut shape = vec![indices.len()];
        shape.extend_from_slice(&view.shape[1..]);
        let sub_size: usize = view.shape[1..].iter().product();
        let mut data = vec![0.0; indices.len() * sub_size];
        for (i, &index) in indices.iter().enumerate() {
            let low = golang::int(index * sub_size);
            view.read_slice(low, low + golang::int(sub_size), |row| {
                data[i * sub_size..(i + 1) * sub_size].copy_from_slice(row);
            });
        }
        Tensor::raw(data, shape)
    }

    /// The element at `indices` (Go `Get`).
    ///
    /// # Panics
    ///
    /// With Go's messages, when the number of indices differs from the
    /// number of dimensions or an index is out of range.
    pub fn get(&self, indices: &[usize]) -> f32 {
        let indices: Vec<i64> = indices.iter().map(|&i| golang::int(i)).collect();
        self.get_go(&indices)
    }

    /// `Get` with Go `int` indices, which may be negative.
    pub(crate) fn get_go(&self, indices: &[i64]) -> f32 {
        let view = self.view();
        if indices.len() != view.shape.len() {
            panic!("the number of indices does not match the shape of the tensor");
        }
        let mut index = 0i64;
        for (&i, &dim) in indices.iter().zip(&view.shape) {
            if i < 0 || i >= golang::int(dim) {
                panic!("index out of range");
            }
            index = index * golang::int(dim) + i;
        }
        view.read(|d| d[golang::index(index, d.len())])
    }

    /// Computes the gradients of this tensor with respect to every tensor it
    /// was computed from, adding them to their gradients (Go `Backward`).
    ///
    /// The gradient of this tensor starts as ones. Operators run from the
    /// highest generation down, in the order of Go's `container/heap`, so
    /// that gradients add up in the same order. Every operator's output
    /// gradient is cleared after it runs, this tensor's included; leaves keep
    /// theirs.
    ///
    /// # Panics
    ///
    /// As Go does: on a tensor no operator produced (after setting its
    /// gradient), and when a gradient's shape differs from its input's.
    pub fn backward(&self) {
        super::op::backward(self);
    }

    /// The gradient, if `backward` has computed one (Go `Grad`).
    pub fn grad(&self) -> Option<Tensor> {
        lock(&self.0.grad).clone()
    }

    /// A copy of the data (Go `Data`, which returns the data itself).
    pub fn data(&self) -> Vec<f32> {
        self.view().to_vec()
    }

    /// Go `clone`: a new leaf tensor with a copy of the data.
    pub(crate) fn clone_tensor(&self) -> Tensor {
        let view = self.view();
        let data = view.to_vec();
        Tensor::raw(data, view.shape)
    }

    /// `t += other`, `other`'s shape a suffix of `t`'s: `floats.AddConst` of
    /// `other.data[0]` when `other` has one element, otherwise `floats.Add`
    /// of `other.data` to each `t.data[i:i+size]`, which Go takes past the
    /// length of the data up to its capacity.
    ///
    /// # Panics
    ///
    /// With Go's messages, and where Go would loop forever: when `other` has
    /// no elements and `t` has some.
    pub(crate) fn add(&self, other: &Tensor) -> Tensor {
        let w_size: usize = other.shape().iter().product();
        let (va, vb) = (self.view(), other.view());
        if w_size == 1 {
            // AddConst takes its constant by value, before any write.
            let c = vb.read(|o| o[go_index(0, o.len())]);
            va.write(|t| floats::add_const(t, c));
            return self.clone();
        }
        assert!(
            w_size > 0 || va.len == 0,
            "nn: adding a tensor without elements, which never ends in Go"
        );
        let chunk = |i: usize| golang::slice(golang::int(i), golang::int(i + w_size), va.cap);
        if Arc::ptr_eq(&va.buf, &vb.buf) {
            // floats.Add is a plain loop, `dst[i] += s[i]`.
            let mut g = write(&va.buf);
            for i in (0..va.len).step_by(w_size) {
                let r = chunk(i);
                assert!(r.len() == vb.len, "floats: slice lengths do not match");
                for j in 0..w_size {
                    let s = g[vb.offset + j];
                    g[va.offset + r.start + j] += s;
                }
            }
        } else {
            let (ra, rb) = (va.offset..va.offset + va.cap, vb.offset..vb.offset + vb.len);
            Tensor::locked(&va, ra, &vb, rb, |t, o| {
                for i in (0..va.len).step_by(w_size) {
                    floats::add(&mut t[chunk(i)], o);
                }
            });
        }
        self.clone()
    }

    /// `t -= other`, `other` repeated along `t` (a suffix of its shape).
    pub(crate) fn sub(&self, other: &Tensor) -> Tensor {
        let w_size: usize = other.shape().iter().product();
        self.combine(other, |i| modulo(i, w_size), |x, o| x - o);
        self.clone()
    }

    /// `t -= other`, each element of `other` spread over a block of `t` (a
    /// prefix of its shape).
    pub(crate) fn b_sub(&self, other: &Tensor) -> Tensor {
        let b_size = block_size(&self.shape(), &other.shape());
        self.combine(other, |i| divide(i, b_size), |x, o| x - o);
        self.clone()
    }

    /// `t *= other`, `other` repeated along `t`.
    pub(crate) fn mul(&self, other: &Tensor) -> Tensor {
        let w_size: usize = other.shape().iter().product();
        self.combine(other, |i| modulo(i, w_size), |x, o| x * o);
        self.clone()
    }

    /// `t /= other`, `other` repeated along `t`.
    pub(crate) fn div(&self, other: &Tensor) -> Tensor {
        let w_size: usize = other.shape().iter().product();
        self.combine(other, |i| modulo(i, w_size), |x, o| x / o);
        self.clone()
    }

    /// `t /= other`, each element of `other` spread over a block of `t`.
    pub(crate) fn b_div(&self, other: &Tensor) -> Tensor {
        let b_size = block_size(&self.shape(), &other.shape());
        self.combine(other, |i| divide(i, b_size), |x, o| x / o);
        self.clone()
    }

    /// `floats.MulTo(t.data, t.data, t.data)`.
    pub(crate) fn square(&self) -> Tensor {
        self.write(|t| {
            let a = t.to_vec();
            floats::mul_to(&a, &a, t);
        });
        self.clone()
    }

    /// `t.data[i] = math32.Pow(t.data[i], other.data[i % size])`.
    pub(crate) fn pow(&self, other: &Tensor) -> Tensor {
        let w_size: usize = other.shape().iter().product();
        self.combine(other, |i| modulo(i, w_size), math32::pow);
        self.clone()
    }

    /// `float32(math.Exp(float64(x)))`: Go's float64 `Exp`, not math32's.
    pub(crate) fn exp(&self) -> Tensor {
        self.map(|x| deps::exp(f64::from(x)) as f32)
    }

    pub(crate) fn log(&self) -> Tensor {
        self.map(math32::log)
    }

    pub(crate) fn sin(&self) -> Tensor {
        self.map(math32::sin)
    }

    pub(crate) fn cos(&self) -> Tensor {
        self.map(math32::cos)
    }

    pub(crate) fn tanh(&self) -> Tensor {
        self.map(math32::tanh)
    }

    pub(crate) fn neg(&self) -> Tensor {
        self.map(|x| -x)
    }

    fn map(&self, f: impl Fn(f32) -> f32) -> Tensor {
        self.write(|t| t.iter_mut().for_each(|x| *x = f(*x)));
        self.clone()
    }

    /// The matrix product of two 2-D tensors, either transposed, with the rows
    /// of the result split into `jobs` parts computed in parallel.
    pub(crate) fn mat_mul(&self, other: &Tensor, transpose1: bool, transpose2: bool, jobs: usize) -> Tensor {
        let (xs, ys) = (self.shape(), other.shape());
        if xs.len() != 2 || ys.len() != 2 {
            panic!("matMul requires 2-D tensors");
        }
        let compatible = match (transpose1, transpose2) {
            (false, false) => xs[1] == ys[0],
            (true, false) => xs[0] == ys[0],
            (false, true) => xs[1] == ys[1],
            (true, true) => xs[0] == ys[1],
        };
        if !compatible {
            panic!(
                "matMul requires the shapes of tensors are compatible, but got {} and {}",
                golang::format_ints(&xs),
                golang::format_ints(&ys)
            );
        }
        let (m, k) = if transpose1 { (xs[1], xs[0]) } else { (xs[0], xs[1]) };
        let n = if transpose2 { ys[0] } else { ys[1] };
        // A part from row `a` reads `t.data[a*k:]`, or `t.data[a:]` with the
        // leading dimension `m` when `t` is transposed.
        let (row_step, lda) = if transpose1 { (1, m) } else { (k, k) };
        let ldb = if transpose2 { k } else { n };
        let mut result = vec![0.0; m * n];
        let parts = partition(golang::int(m), golang::int(jobs));
        Tensor::read2(self, other, |a, b| {
            let mut rest = result.as_mut_slice();
            let mut chunks = Vec::with_capacity(parts.len());
            for &(start, end) in &parts {
                let (c, tail) = std::mem::take(&mut rest).split_at_mut((end - start) * n);
                chunks.push((start, end, c));
                rest = tail;
            }
            run_parts(chunks, |(start, end, c)| {
                floats::mm(transpose1, transpose2, end - start, n, k, &a[start * row_step..], lda, b, ldb, c, n);
            });
        });
        Tensor::raw(result, vec![m, n])
    }

    /// The matrix products of the matrices of two 3-D tensors, either
    /// transposed, the batch spread over `jobs` workers.
    pub(crate) fn batch_mat_mul(&self, other: &Tensor, transpose1: bool, transpose2: bool, jobs: usize) -> Tensor {
        let (xs, ys) = (self.shape(), other.shape());
        if xs.len() != 3 || ys.len() != 3 {
            panic!("BatchMatMul requires 3-D tensors");
        }
        let compatible = match (transpose1, transpose2) {
            (false, false) => xs[0] == ys[0] && xs[2] == ys[1],
            (true, false) => xs[0] == ys[0] && xs[1] == ys[1],
            (false, true) => xs[0] == ys[0] && xs[2] == ys[2],
            (true, true) => xs[0] == ys[0] && xs[1] == ys[2],
        };
        if !compatible {
            if !transpose1 && !transpose2 {
                panic!("BatchMatMul requires the shapes of tensors are compatible");
            }
            panic!("batchMatMul requires the shapes of tensors are compatible");
        }
        let b = xs[0];
        let (m, k) = if transpose1 { (xs[2], xs[1]) } else { (xs[1], xs[2]) };
        let n = if transpose2 { ys[1] } else { ys[2] };
        let lda = if transpose1 { m } else { k };
        let ldb = if transpose2 { k } else { n };
        let mut result = vec![0.0; b * m * n];
        Tensor::read2(self, other, |x, y| {
            let mut rest = result.as_mut_slice();
            let mut chunks = Vec::with_capacity(b);
            for _ in 0..b {
                let (c, tail) = std::mem::take(&mut rest).split_at_mut(m * n);
                chunks.push(Mutex::new(c));
                rest = tail;
            }
            deps::parallel_for(b, jobs, |i| {
                let a = &x[i * m * k..(i + 1) * m * k];
                let bb = &y[i * n * k..(i + 1) * n * k];
                floats::mm(transpose1, transpose2, m, n, k, a, lda, bb, ldb, &mut lock(&chunks[i]), n);
            });
        });
        Tensor::raw(result, vec![b, m, n])
    }

    /// `t.data[i] = max(t.data[i], other.data[0 or i])` with Go's builtin
    /// `max`.
    pub(crate) fn maximum(&self, other: &Tensor) {
        let scalar = other.is_scalar();
        self.combine(other, |i| if scalar { 0 } else { i }, golang::max32);
    }

    /// `t.data[i] = t.data[i] > other.data[0 or i] ? 1 : 0`.
    pub(crate) fn gt(&self, other: &Tensor) -> Tensor {
        let scalar = other.is_scalar();
        self.combine(other, |i| if scalar { 0 } else { i }, |x, o| if x > o { 1.0 } else { 0.0 });
        self.clone()
    }

    /// Swaps the last two dimensions.
    #[cfg_attr(not(test), expect(dead_code, reason = "Go uses it only in tests"))]
    pub(crate) fn transpose(&self) -> Tensor {
        let view = self.view();
        if view.shape.len() < 2 {
            panic!("transpose requires at least 2-D tensor");
        }
        let rank = view.shape.len();
        let batch_size: usize = view.shape[..rank - 2].iter().product();
        let (m, n) = (view.shape[rank - 2], view.shape[rank - 1]);
        let mut shape = view.shape[..rank - 2].to_vec();
        shape.extend([n, m]);
        let mut data = vec![0.0; batch_size * m * n];
        view.read(|t| {
            for b in 0..batch_size {
                for i in 0..m {
                    for j in 0..n {
                        data[b * m * n + j * m + i] = t[b * m * n + i * n + j];
                    }
                }
            }
        });
        Tensor::raw(data, shape)
    }

    /// The maximum along `axis`. A 1-D tensor gives a scalar by `lo.Max`,
    /// which skips a NaN after the first element; otherwise Go's builtin
    /// `max`, which returns NaN for any NaN.
    pub(crate) fn max(&self, axis: isize, keep_dim: bool) -> Tensor {
        self.reduce(axis, keep_dim, lo_max, golang::max32)
    }

    /// The sum along `axis`. A 1-D tensor gives a scalar by `lo.Sum`, which
    /// starts from +0.
    pub(crate) fn sum(&self, axis: isize, keep_dim: bool) -> Tensor {
        self.reduce(axis, keep_dim, |d| d.iter().fold(0.0, |s, &x| s + x), |s, x| s + x)
    }

    fn reduce(
        &self,
        axis: isize,
        keep_dim: bool,
        vector: impl Fn(&[f32]) -> f32,
        combine: impl Fn(f32, f32) -> f32,
    ) -> Tensor {
        let view = self.view();
        let axis = match usize::try_from(axis) {
            Ok(a) if a < view.shape.len() => a,
            _ => panic!("axis out of range"),
        };
        if view.shape.len() == 1 {
            return new_scalar(view.read(vector));
        }
        let mut shape = Vec::with_capacity(view.shape.len());
        let (mut a, mut b, mut c) = (1, 1, 1);
        for (i, &dim) in view.shape.iter().enumerate() {
            if i < axis {
                shape.push(dim);
                a *= dim;
            } else if i == axis {
                if keep_dim {
                    shape.push(1);
                }
                b = dim;
            } else {
                shape.push(dim);
                c *= dim;
            }
        }
        let mut data = vec![0.0; a * c];
        view.read(|t| {
            for i in 0..a {
                for j in 0..c {
                    let mut value = t[go_index(i * b * c + j, t.len())];
                    for k in 1..b {
                        value = combine(value, t[i * b * c + j + k * c]);
                    }
                    data[i * c + j] = value;
                }
            }
        });
        Tensor::raw(data, shape)
    }

    /// The indices of the first maximum, or `None` for no data.
    #[cfg_attr(not(test), expect(dead_code, reason = "Go uses it only in tests"))]
    pub(crate) fn argmax(&self) -> Option<Vec<usize>> {
        let view = self.view();
        let mut max_index = view.read(|t| {
            let first = *t.first()?;
            let mut max_value = first;
            let mut max_index = 0;
            for (i, &x) in t.iter().enumerate().skip(1) {
                if x > max_value {
                    max_value = x;
                    max_index = i;
                }
            }
            Some(max_index)
        })?;
        let mut indices = vec![0; view.shape.len()];
        for (index, &dim) in indices.iter_mut().zip(&view.shape).rev() {
            *index = max_index % dim;
            max_index /= dim;
        }
        Some(indices)
    }
}

/// `lo.Max`: the first element, replaced by every later one that is greater;
/// 0 for none.
fn lo_max(d: &[f32]) -> f32 {
    let Some((&first, rest)) = d.split_first() else {
        return 0.0;
    };
    rest.iter().fold(first, |m, &x| if x > m { x } else { m })
}

/// `s[i]`'s bounds check, with Go's message.
pub(crate) fn go_index(i: usize, len: usize) -> usize {
    if i < len {
        i
    } else {
        panic!("{}", golang::index_panic(golang::int(i), len))
    }
}

/// `i % n`, which Go rejects for `n == 0`.
pub(crate) fn modulo(i: usize, n: usize) -> usize {
    i.checked_rem(n).unwrap_or_else(|| panic!("runtime error: integer divide by zero"))
}

/// `i / n`, which Go rejects for `n == 0`.
fn divide(i: usize, n: usize) -> usize {
    i.checked_div(n).unwrap_or_else(|| panic!("runtime error: integer divide by zero"))
}

/// Go's `bSize`: the size of `t` divided by each dimension of `other` in
/// turn.
fn block_size(t: &[usize], other: &[usize]) -> usize {
    other.iter().fold(t.iter().product(), |b, &d| divide(b, d))
}

/// Splits `0..n` into at most `p` contiguous parts whose sizes differ by at
/// most one, the larger first (Go `partition`).
pub(crate) fn partition(n: i64, p: i64) -> Vec<(usize, usize)> {
    if n <= 0 {
        return Vec::new();
    }
    let len = n.cast_unsigned() as usize;
    if p <= 1 {
        return vec![(0, len)];
    }
    if n <= p {
        return (0..len).map(|i| (i, i + 1)).collect();
    }
    let p = p.cast_unsigned() as usize;
    let min_part_size = len / p;
    let mut max_part_count = len % p;
    let mut parts = Vec::with_capacity(p);
    let mut i = 0;
    while i < len {
        let mut part_size = min_part_size;
        if max_part_count > 0 {
            part_size += 1;
            max_part_count -= 1;
        }
        let end = (i + part_size).min(len);
        parts.push((i, end));
        i = end;
    }
    parts
}

/// Runs `f` on each job, each on its own thread when there are several, as
/// Go starts a goroutine for each and waits for them all.
pub(crate) fn run_parts<T: Send>(jobs: Vec<T>, f: impl Fn(T) + Sync) {
    if jobs.len() <= 1 {
        jobs.into_iter().for_each(f);
        return;
    }
    let f = &f;
    std::thread::scope(|s| {
        for job in jobs {
            s.spawn(move || f(job));
        }
    });
}

/// The nil pointer dereference Go panics with on a missing gradient.
pub(crate) fn nil_dereference() -> ! {
    panic!("runtime error: invalid memory address or nil pointer dereference")
}

impl fmt::Display for Tensor {
    /// Go `String`: a scalar's value, or the values in brackets, the first
    /// and last five of more than ten.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let view = self.view();
        let data = view.to_vec();
        if view.shape.is_empty() {
            return f.write_str(&golang::format_float32(data[0]));
        }
        let items: Vec<String> = if data.len() <= 10 {
            data.iter().map(|&x| golang::format_float32(x)).collect()
        } else {
            let head = data[..5].iter().map(|&x| golang::format_float32(x));
            let tail = data[data.len() - 5..].iter().map(|&x| golang::format_float32(x));
            head.chain(std::iter::once("...".to_owned())).chain(tail).collect()
        };
        write!(f, "[{}]", items.join(", "))
    }
}

impl fmt::Debug for Tensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tensor({} {})", golang::format_ints(&self.shape()), self)
    }
}

/// A tensor of the given data and shape (Go `NewTensor`).
///
/// # Panics
///
/// With Go's message when the data does not have the shape's size.
pub fn new_tensor(data: Vec<f32>, shape: &[usize]) -> Tensor {
    check_size(data.len(), shape);
    Tensor::raw(data, shape.to_vec())
}

fn check_size(len: usize, shape: &[usize]) {
    let size: usize = shape.iter().product();
    if len != size {
        panic!("shape {} does not match data size {}", golang::format_ints(shape), len);
    }
}

/// A tensor without dimensions holding `data` (Go `NewScalar`).
pub fn new_scalar(data: f32) -> Tensor {
    Tensor::raw(vec![data], Vec::new())
}

/// Values from `start` in steps of `(end - start) / (n - 1)`, `n` the size of
/// the shape (Go `LinSpace`). As in Go, a size of 1 divides by zero.
pub fn lin_space(start: f32, end: f32, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let delta = (end - start) / (golang::int(n) - 1) as f32;
    let data = (0..n).map(|i| start + delta * i as f32).collect();
    Tensor::raw(data, shape.to_vec())
}

/// Values of `rng.Float32()`, or of Go's global source for `None` (Go
/// `Rand`).
pub fn rand(rng: Option<&mut dyn Rng>, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let data = match rng {
        Some(rng) => (0..n).map(|_| rng.float32()).collect(),
        None => deps::global(|rng| (0..n).map(|_| rng.float32()).collect()),
    };
    Tensor::raw(data, shape.to_vec())
}

/// Uniform values in `[low, high)` from Go's global source (Go `Uniform`).
pub fn uniform(low: f32, high: f32, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let data = deps::global(|rng| (0..n).map(|_| rng.float32() * (high - low) + low).collect());
    Tensor::raw(data, shape.to_vec())
}

/// Normal values from Go's global source (Go `Normal`).
pub fn normal(mean: f32, std: f32, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let data = deps::global(|rng| (0..n).map(|_| rng.norm_float64() as f32 * std + mean).collect());
    Tensor::raw(data, shape.to_vec())
}

/// A tensor of ones (Go `Ones`).
pub fn ones(shape: &[usize]) -> Tensor {
    Tensor::raw(vec![1.0; shape.iter().product()], shape.to_vec())
}

/// A tensor of zeros (Go `Zeros`).
pub fn zeros(shape: &[usize]) -> Tensor {
    Tensor::raw(vec![0.0; shape.iter().product()], shape.to_vec())
}

/// Overwrites the data of `t` with normal values of `rng`, or of Go's global
/// source for `None` (Go `NormalInit`).
pub fn normal_init(rng: Option<&mut dyn Rng>, t: &Tensor, mean: f32, std: f32) {
    let fill = |rng: &mut dyn Rng| {
        t.write(|d| d.iter_mut().for_each(|x| *x = rng.norm_float64() as f32 * std + mean));
    };
    match rng {
        Some(rng) => fill(rng),
        None => deps::global(fill),
    }
}
