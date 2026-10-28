// Ports: model/ctr/model.go @ add21b5

//! The click-through-rate model interface, its scores and fit configuration,
//! and the framing that writes a model with a type header and reads it back.

use std::io::{Read, Write};

use gorse_common::encoding::{self, read_string, write_string};
use gorse_common::gocontext::Context;
use gorse_common::nn;
use gorse_dataset::dataset::CtrSplit;
use gorse_model_core::model::Model;

use super::data::{Embedding, Label};
use super::fm::Afm;

/// Errors of marshalling and unmarshalling a model.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A gob or string value failed to encode or decode.
    #[error(transparent)]
    Encoding(#[from] encoding::Error),
    /// The tensors of a model failed to save or load.
    #[error(transparent)]
    Nn(#[from] nn::Error),
    /// The header names a model this port does not read (Go `unknown model`).
    #[error("unknown model: {0}")]
    UnknownModel(String),
    /// A hyper-parameter has a type that the gob form does not carry.
    #[error("unsupported parameter {0}")]
    UnsupportedParam(String),
    /// The model has a feature with embeddings but no index to name them.
    #[error("embedding index is missing")]
    MissingEmbeddingIndex,
    /// The dataset index is missing.
    #[error("unified index is missing")]
    MissingIndex,
    /// A unified index or an embedding index did not decode.
    #[error(transparent)]
    Dataset(#[from] gorse_dataset::Error),
}

/// The scores of a model on a test set (Go `Score`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Score {
    /// The root mean square error of a regression.
    pub rmse: f32,
    /// The precision of a classification.
    pub precision: f32,
    /// The recall of a classification.
    pub recall: f32,
    /// The accuracy of a classification.
    pub accuracy: f32,
    /// The area under the ROC curve of a classification.
    pub auc: f32,
}

impl Score {
    /// The value a fit is judged by, which is the precision (Go `GetValue`).
    pub fn get_value(&self) -> f32 {
        self.precision
    }

    /// Reports whether this score is better than `other`, by AUC (Go
    /// `BetterThan`).
    pub fn better_than(&self, other: &Score) -> bool {
        self.auc > other.auc
    }
}

/// The settings of a fit (Go `FitConfig`).
#[derive(Debug, Clone, PartialEq)]
pub struct FitConfig {
    /// The number of threads the fit uses.
    pub jobs: usize,
    /// The number of epochs between two evaluations of the test set.
    pub verbose: i64,
    /// The number of epochs without an improvement after which the fit stops.
    /// A value of 0 or less disables early stopping.
    pub patience: i64,
}

impl Default for FitConfig {
    /// Go's `NewFitConfig`: one job, evaluation every 10 epochs, patience 10.
    fn default() -> Self {
        Self {
            jobs: 1,
            verbose: 10,
            patience: 10,
        }
    }
}

impl FitConfig {
    /// Go's `NewFitConfig`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the number of epochs between two evaluations.
    pub fn set_verbose(&mut self, verbose: i64) -> &mut Self {
        self.verbose = verbose;
        self
    }

    /// Sets the number of threads.
    pub fn set_jobs(&mut self, jobs: usize) -> &mut Self {
        self.jobs = jobs;
        self
    }

    /// Sets the patience of early stopping.
    pub fn set_patience(&mut self, patience: i64) -> &mut Self {
        self.patience = patience;
        self
    }
}

/// A factorization machine model (Go `FactorizationMachines`).
pub trait FactorizationMachines: Model + Send + Sync {
    /// The header [`marshal_model`] writes before the model.
    fn header(&self) -> &'static str;

    /// Predicts the target of one user-item pair with their labels (Go
    /// `Predict`).
    fn predict(
        &self,
        user_id: &str,
        item_id: &str,
        user_features: &[Label],
        item_features: &[Label],
    ) -> f32;

    /// Predicts the target of one sample given its feature ids and values (Go
    /// `InternalPredict`).
    fn internal_predict(&self, x: &[i32], values: &[f32]) -> f32;

    /// Trains the model on `train_set`, evaluating on `test_set` (Go `Fit`).
    /// Returns the last score, or the default score when the context is
    /// cancelled.
    fn fit(
        &mut self,
        ctx: &Context,
        train_set: &dyn CtrSplit,
        test_set: &dyn CtrSplit,
        config: &FitConfig,
    ) -> Score;

    /// Writes the model without its header (Go `Marshal`). Generic over the
    /// writer, so the trait stays object-safe: a boxed model is read, and
    /// written through [`marshal_model`] with its concrete type.
    fn marshal<W: Write>(&self, w: &mut W) -> Result<(), Error>
    where
        Self: Sized;

    /// The batch inference of this model, when it has one (Go's
    /// `estimator.(BatchInference)` type assertion).
    fn as_batch_inference(&self) -> Option<&dyn BatchInference> {
        None
    }
}

/// Predicts many samples at once (Go `BatchInference`).
pub trait BatchInference {
    /// Predicts the user-item pairs with their labels and their embeddings
    /// (Go `BatchPredict`).
    fn batch_predict(
        &self,
        inputs: &[(String, String, Vec<Label>, Vec<Label>)],
        embeddings: &[Vec<Embedding>],
        jobs: usize,
    ) -> Vec<f32>;

    /// Predicts the samples given their feature ids, values and embeddings
    /// (Go `BatchInternalPredict`).
    fn batch_internal_predict(
        &self,
        x: &[(Vec<i32>, Vec<f32>)],
        e: &[Vec<Vec<u16>>],
        jobs: usize,
    ) -> Vec<f32>;
}

/// Writes a model with its header, so [`unmarshal_model`] can read it (Go
/// `MarshalModel`).
pub fn marshal_model<M: FactorizationMachines, W: Write>(w: &mut W, m: &M) -> Result<(), Error> {
    // write header
    write_string(w, m.header())?;
    m.marshal(w)
}

/// Reads a model written by [`marshal_model`] (Go `UnmarshalModel`).
pub fn unmarshal_model<R: Read>(r: &mut R) -> Result<Box<dyn FactorizationMachines>, Error> {
    // read header
    let header = read_string(r)?;
    match header.as_str() {
        "AFM" | "AFM2" => {
            let fm = Afm::unmarshal(r, header == "AFM2")?;
            Ok(Box::new(fm))
        }
        _ => Err(Error::UnknownModel(header)),
    }
}
