// Ports: common/nn/layers.go @ add21b5

//! Layers and models, and saving and loading their parameters.

use std::io::{self, Read, Write};

use crate::gostrconv;
use crate::math32;

use super::codec::{self, Message, ReadError};
use super::functions::{add, embedding, flatten, mat_mul, mul, relu, sigmoid, softmax};
use super::golang::{self, AtoiError};
use super::tensor::{Tensor, normal, uniform, zeros};

/// A layer of a model, or a whole model (Go `Layer`).
pub trait Layer: Reflect + Send + Sync {
    /// The tensors training updates.
    fn parameters(&self) -> Vec<Tensor>;

    /// The output for the input `x`.
    fn forward(&self, x: &Tensor) -> Tensor;

    /// Sets the number of threads matrix products use.
    fn set_jobs(&mut self, jobs: usize);
}

/// A model (Go `Model`, the same interface as `Layer`).
pub use self::Layer as Model;

/// A fully connected layer, `x * W + B` (Go `LinearLayer`).
pub struct LinearLayer {
    /// The weights, of shape `(in, out)`.
    pub w: Tensor,
    /// The bias, of shape `(out)`.
    pub b: Tensor,
    jobs: usize,
}

/// A fully connected layer with weights uniform in `±1/sqrt(in)` from Go's
/// global source and zero bias (Go `NewLinear`).
pub fn new_linear(input: usize, output: usize) -> LinearLayer {
    let bound = 1.0 / math32::sqrt(input as f32);
    LinearLayer {
        w: uniform(-bound, bound, &[input, output]),
        b: zeros(&[output]),
        jobs: 0,
    }
}

impl Layer for LinearLayer {
    fn parameters(&self) -> Vec<Tensor> {
        vec![self.w.clone(), self.b.clone()]
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        add(&mat_mul(x, &self.w, false, false, self.jobs), &[&self.b])
    }

    /// Sets the threads, at least 1.
    fn set_jobs(&mut self, jobs: usize) {
        self.jobs = jobs.max(1);
    }
}

impl Reflect for LinearLayer {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(vec![
            Field::exported("W", self.w.reflect()),
            Field::exported("B", self.b.reflect()),
            Field::unexported("jobs"),
        ])
    }
}

/// A layer that flattens its input (Go `NewFlatten`).
pub struct FlattenLayer;

/// A layer that flattens its input (Go `NewFlatten`).
pub fn new_flatten() -> FlattenLayer {
    FlattenLayer
}

impl Layer for FlattenLayer {
    fn parameters(&self) -> Vec<Tensor> {
        Vec::new()
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        flatten(x)
    }

    fn set_jobs(&mut self, _: usize) {}
}

impl Reflect for FlattenLayer {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(Vec::new())
    }
}

/// A table of embeddings, looked up by the indices of its input (Go
/// `EmbeddingLayer`).
pub struct EmbeddingLayer {
    /// The table, one row per index.
    pub w: Tensor,
}

/// An embedding table of `n` rows of the given shape, normal with standard
/// deviation 0.01 from Go's global source (Go `NewEmbedding`).
pub fn new_embedding(n: usize, shape: &[usize]) -> EmbeddingLayer {
    let mut w_shape = vec![n];
    w_shape.extend_from_slice(shape);
    EmbeddingLayer {
        w: normal(0.0, 0.01, &w_shape),
    }
}

impl Layer for EmbeddingLayer {
    fn parameters(&self) -> Vec<Tensor> {
        vec![self.w.clone()]
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        embedding(&self.w, x)
    }

    fn set_jobs(&mut self, _: usize) {}
}

impl Reflect for EmbeddingLayer {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(vec![Field::exported("W", self.w.reflect())])
    }
}

/// The logistic function as a layer (Go `NewSigmoid`).
pub struct SigmoidLayer;

/// The logistic function as a layer (Go `NewSigmoid`).
pub fn new_sigmoid() -> SigmoidLayer {
    SigmoidLayer
}

impl Layer for SigmoidLayer {
    fn parameters(&self) -> Vec<Tensor> {
        Vec::new()
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        sigmoid(x)
    }

    fn set_jobs(&mut self, _: usize) {}
}

impl Reflect for SigmoidLayer {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(Vec::new())
    }
}

/// `max(x, 0)` as a layer (Go `NewReLU`).
pub struct ReLULayer;

/// `max(x, 0)` as a layer (Go `NewReLU`).
pub fn new_relu() -> ReLULayer {
    ReLULayer
}

impl Layer for ReLULayer {
    fn parameters(&self) -> Vec<Tensor> {
        Vec::new()
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        relu(x)
    }

    fn set_jobs(&mut self, _: usize) {}
}

impl Reflect for ReLULayer {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(Vec::new())
    }
}

/// Layers applied in order (Go `Sequential`).
pub struct Sequential {
    /// The layers.
    pub layers: Vec<Box<dyn Layer>>,
}

/// Layers applied in order (Go `NewSequential`).
pub fn new_sequential(layers: Vec<Box<dyn Layer>>) -> Sequential {
    Sequential { layers }
}

impl Layer for Sequential {
    fn parameters(&self) -> Vec<Tensor> {
        self.layers.iter().flat_map(|l| l.parameters()).collect()
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        self.layers.iter().fold(x.clone(), |x, l| l.forward(&x))
    }

    fn set_jobs(&mut self, jobs: usize) {
        for l in &mut self.layers {
            l.set_jobs(jobs);
        }
    }
}

impl Reflect for Sequential {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(vec![Field::exported("Layers", self.layers.reflect())])
    }
}

/// Attention over the features of each row: `softmax(relu(x W) H) * x`
/// (Go `Attention`).
pub struct Attention {
    /// The linear layer, `dimensions` to `k`.
    pub w: Box<dyn Layer>,
    /// The second weights, of shape `(k, dimensions)`.
    pub h: Tensor,
    jobs: usize,
}

/// Attention over `dimensions` features through `k` hidden units, with
/// weights from Go's global source (Go `NewAttention`).
pub fn new_attention(dimensions: usize, k: usize) -> Attention {
    Attention {
        w: Box::new(new_linear(dimensions, k)),
        h: normal(0.0, 0.01, &[k, dimensions]),
        jobs: 0,
    }
}

impl Layer for Attention {
    /// `H`, then the parameters of `W`.
    fn parameters(&self) -> Vec<Tensor> {
        let mut params = vec![self.h.clone()];
        params.extend(self.w.parameters());
        params
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        mul(
            &softmax(
                &mat_mul(&relu(&self.w.forward(x)), &self.h, false, false, self.jobs),
                1,
            ),
            x,
        )
    }

    /// Sets the threads of `W`, and its own, at least 1.
    fn set_jobs(&mut self, jobs: usize) {
        self.w.set_jobs(jobs);
        self.jobs = jobs.max(1);
    }
}

impl Reflect for Attention {
    fn reflect(&self) -> Reflected {
        Reflected::Struct(vec![
            Field::exported("W", self.w.reflect()),
            Field::exported("H", self.h.reflect()),
            Field::unexported("jobs"),
        ])
    }
}

/// The structure [`save`] and [`load`] walk, as Go's reflection sees a
/// value: tensors, structs with their fields in order, and slices.
pub trait Reflect {
    /// The value as Go's reflection sees it.
    fn reflect(&self) -> Reflected;
}

/// A value as Go's reflection sees it.
pub enum Reflected {
    /// A `*Tensor`.
    Tensor(Tensor),
    /// A struct, or a pointer to one, with its fields in order.
    Struct(Vec<Field>),
    /// A slice.
    Slice(Vec<Reflected>),
    /// Anything else, which [`save`] and [`load`] reject.
    Other,
}

/// A field of a struct.
pub struct Field {
    /// The name of the field in Go, which is its name in the saved keys.
    pub name: &'static str,
    /// The value, or `None` for an unexported field, which [`save`] skips
    /// and [`load`] cannot set.
    pub value: Option<Reflected>,
}

impl Field {
    /// An exported field.
    pub fn exported(name: &'static str, value: Reflected) -> Field {
        Field {
            name,
            value: Some(value),
        }
    }

    /// An unexported field.
    pub fn unexported(name: &'static str) -> Field {
        Field { name, value: None }
    }
}

impl Reflect for Tensor {
    fn reflect(&self) -> Reflected {
        Reflected::Tensor(self.clone())
    }
}

impl<T: Reflect> Reflect for [T] {
    fn reflect(&self) -> Reflected {
        Reflected::Slice(self.iter().map(Reflect::reflect).collect())
    }
}

impl<T: Reflect> Reflect for Vec<T> {
    fn reflect(&self) -> Reflected {
        self.as_slice().reflect()
    }
}

impl<T: Reflect + ?Sized> Reflect for Box<T> {
    fn reflect(&self) -> Reflected {
        (**self).reflect()
    }
}

/// Why [`save`] or [`load`] failed. Where Go's `Load` panics on a key that
/// does not fit the value, this one returns [`Error::Panic`] with Go's panic
/// message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error of the writer or reader.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A value that is not a tensor, a struct or a slice.
    #[error("unexpected type")]
    UnexpectedType,
    /// A length prefix that does not end within five bytes.
    #[error("invalid varint32 encountered")]
    InvalidVarint,
    /// The stream ends inside a message.
    #[error("unexpected EOF")]
    UnexpectedEof,
    /// A message that is not valid protobuf.
    #[error("proto: cannot parse invalid wire-format data")]
    InvalidWireFormat,
    /// A key that is not UTF-8.
    #[error("string field contains invalid UTF-8")]
    InvalidUtf8,
    /// A key for a slice that is not a decimal `int`.
    #[error("strconv.Atoi: parsing {}: {}", gostrconv::quote(.input), atoi_reason(.out_of_range))]
    Atoi {
        /// The key.
        input: String,
        /// Whether it is a number out of the range of `int` rather than
        /// not a number.
        out_of_range: bool,
    },
    /// A key that does not fit the value, where Go panics; the message is
    /// Go's panic message.
    #[error("{0}")]
    Panic(String),
    /// A negative dimension, which Go stores in a shape but a `usize`
    /// cannot hold.
    #[error("negative dimension {0} in the shape of a tensor")]
    NegativeShape(i32),
}

/// The text of Go's `strconv.ErrRange` or `strconv.ErrSyntax`.
fn atoi_reason(out_of_range: &bool) -> &'static str {
    if *out_of_range {
        "value out of range"
    } else {
        "invalid syntax"
    }
}

/// Writes every tensor of `o` as a length-delimited `protocol.Tensor` keyed
/// by its path of field names and slice indices, in the order of the fields
/// and elements (Go `Save`). Unexported fields are skipped.
///
/// # Errors
///
/// [`Error::Io`] from the writer, and [`Error::UnexpectedType`] for an
/// exported field that is neither a tensor, a struct nor a slice, after
/// writing what comes before it.
pub fn save(o: &(impl Reflect + ?Sized), w: &mut impl Write) -> Result<(), Error> {
    fn walk(v: &Reflected, key: &mut Vec<String>, w: &mut dyn Write) -> Result<(), Error> {
        match v {
            Reflected::Tensor(t) => {
                let view = t.view();
                let m = Message {
                    key: key.clone(),
                    // Go `int32(i)`.
                    shape: view.shape.iter().map(|&d| d as i32).collect(),
                    data: t.data(),
                };
                codec::write_delimited(w, &m)?;
            }
            Reflected::Struct(fields) => {
                for field in fields {
                    if let Some(value) = &field.value {
                        key.push(field.name.to_owned());
                        walk(value, key, w)?;
                        key.pop();
                    }
                }
            }
            Reflected::Slice(items) => {
                for (i, item) in items.iter().enumerate() {
                    key.push(i.to_string());
                    walk(item, key, w)?;
                    key.pop();
                }
            }
            Reflected::Other => return Err(Error::UnexpectedType),
        }
        Ok(())
    }
    walk(&o.reflect(), &mut Vec::new(), w)
}

/// Reads length-delimited `protocol.Tensor`s until the end of the stream and
/// sets the tensor of `o` each key leads to, its shape and data replaced
/// (Go `Load`). A key naming a field `o` does not have is skipped, and keys
/// past a tensor are ignored. The tensors before an error stay loaded.
///
/// # Errors
///
/// The errors of reading and decoding the stream, as Go's: a stream that
/// ends between messages, or within a length prefix, or before the first
/// byte of a message, ends without an error. And [`Error::Atoi`] for a slice
/// index that is not a number, [`Error::UnexpectedType`] for a key leading
/// into a value that is not a tensor, struct or slice,
/// [`Error::NegativeShape`], and [`Error::Panic`] where Go panics: a key that
/// ends at a struct or slice, an unexported field, and an index out of
/// range.
pub fn load(o: &(impl Reflect + ?Sized), r: &mut impl Read) -> Result<(), Error> {
    fn place(v: Reflected, key: &[String], m: &Message) -> Result<(), Error> {
        let first = || {
            key.first()
                .ok_or_else(|| Error::Panic(golang::index_panic(0, 0)))
        };
        match v {
            Reflected::Tensor(t) => {
                let shape = m
                    .shape
                    .iter()
                    .map(|&d| usize::try_from(d).map_err(|_| Error::NegativeShape(d)))
                    .collect::<Result<Vec<_>, _>>()?;
                t.set_data(m.data.clone(), shape);
                Ok(())
            }
            Reflected::Struct(fields) => {
                let name = first()?;
                match fields.into_iter().find(|f| f.name == name) {
                    None => Ok(()),
                    Some(Field { value: None, .. }) => Err(Error::Panic(
                        "reflect.Value.Interface: cannot return value obtained from unexported field or method"
                            .to_owned(),
                    )),
                    Some(Field { value: Some(value), .. }) => place(value, &key[1..], m),
                }
            }
            Reflected::Slice(items) => {
                let first = first()?;
                let index = golang::atoi(first).map_err(|e| Error::Atoi {
                    input: first.clone(),
                    out_of_range: e == AtoiError::Range,
                })?;
                let item = usize::try_from(index)
                    .ok()
                    .and_then(|i| items.into_iter().nth(i))
                    .ok_or_else(|| Error::Panic("reflect: slice index out of range".to_owned()))?;
                place(item, &key[1..], m)
            }
            Reflected::Other => Err(Error::UnexpectedType),
        }
    }
    loop {
        let m = match codec::read_delimited(r) {
            Ok(m) => m,
            Err(ReadError::Eof) => return Ok(()),
            Err(ReadError::Error(e)) => return Err(e),
        };
        place(o.reflect(), &m.key, &m)?;
    }
}
