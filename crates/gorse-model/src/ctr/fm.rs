// Ports: model/ctr/fm.go @ add21b5, except fm_xla.go and model.py (D-8, D-196)

//! The factorization machine with attention (AFM), the default build of Go's
//! `model/ctr` (`fm.go`): sparse features and item embeddings go through an
//! embedding table, a second-order interaction, and an attention layer per
//! embedding feature. Training is binary cross-entropy with Adam or SGD.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::{Read, Write};
use std::time::Instant;

use gorse_common::encoding::gob::{Params as GobParams, Value as GobValue};
use gorse_common::encoding::{read_gob, write_gob};
use gorse_common::floats::to_float32;
use gorse_common::gocontext::Context;
use gorse_common::math32;
use gorse_common::monitor::{self, Span};
use gorse_common::nn::{
    self, Optimizer, Tensor, add, bce_with_logits, bmm, embedding, flatten, mat_mul, mul, new_adam,
    new_scalar, new_sgd, new_tensor, relu, reshape, softmax, square, sub, sum, zeros,
};
use gorse_common::util::rand::Rand;
use gorse_dataset::dataset::CtrSplit;
use gorse_dataset::index::{Index, NOT_ID};
use gorse_dataset::unified_index::{UnifiedIndex, marshal_unified_index, unmarshal_unified_index};
use gorse_model_core::model::{BaseModel, Model, SuggestTrial};
use gorse_model_core::params::{
    ADAM, AUTO_SCALE, BATCH_SIZE, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, OPTIMIZER,
    ParamValue, Params, REG, SGD,
};
use tracing::{info, warn};

use super::data::{Embedding, Label};
use super::evaluator::evaluate_classification;
use super::model::{BatchInference, Error, FactorizationMachines, FitConfig, Score};
use super::transformer::AutoScaler;

#[cfg(test)]
mod tests;

/// The header of a model without automatic scaling (`headerAFM`).
const HEADER_AFM: &str = "AFM";
/// The header of a model with automatic scaling (`headerAFM2`).
const HEADER_AFM2: &str = "AFM2";

/// A fully connected layer, `x * W + B` (Go `nn.LinearLayer`).
struct Linear {
    w: Tensor,
    b: Tensor,
    jobs: usize,
}

impl Linear {
    /// Go's `nn.NewLinear`: weights uniform in `±1/sqrt(in)`, zero bias.
    fn new(rng: &mut Rand, input: usize, output: usize) -> Self {
        let bound = 1.0 / math32::sqrt(input as f32);
        Self {
            w: uniform(rng, -bound, bound, &[input, output]),
            b: zeros(&[output]),
            jobs: 0,
        }
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        add(&mat_mul(x, &self.w, false, false, self.jobs), &[&self.b])
    }

    fn parameters(&self) -> Vec<Tensor> {
        vec![self.w.clone(), self.b.clone()]
    }

    fn set_jobs(&mut self, jobs: usize) {
        self.jobs = jobs.max(1);
    }
}

/// A table of embeddings, looked up by the indices of its input (Go
/// `nn.EmbeddingLayer`).
struct Lookup {
    w: Tensor,
}

impl Lookup {
    /// Go's `nn.NewEmbedding`: `n` rows of `shape`, normal with standard
    /// deviation 0.01.
    fn new(rng: &mut Rand, n: usize, shape: &[usize]) -> Self {
        let mut w_shape = vec![n];
        w_shape.extend_from_slice(shape);
        Self {
            w: normal(rng, 0.0, 0.01, &w_shape),
        }
    }

    fn forward(&self, x: &Tensor) -> Tensor {
        embedding(&self.w, x)
    }

    fn parameters(&self) -> Vec<Tensor> {
        vec![self.w.clone()]
    }
}

/// Attention over the features of an embedding (Go `nn.Attention`).
struct Attention {
    w: Linear,
    h: Tensor,
    jobs: usize,
}

impl Attention {
    /// Go's `nn.NewAttention`. The linear layer is drawn before `H`, as the
    /// struct literal of Go draws its fields in order.
    fn new(rng: &mut Rand, dimensions: usize, k: usize) -> Self {
        let w = Linear::new(rng, dimensions, k);
        let h = normal(rng, 0.0, 0.01, &[k, dimensions]);
        Self { w, h, jobs: 0 }
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

    fn parameters(&self) -> Vec<Tensor> {
        let mut params = vec![self.h.clone()];
        params.extend(self.w.parameters());
        params
    }

    fn set_jobs(&mut self, jobs: usize) {
        self.w.set_jobs(jobs);
        self.jobs = jobs.max(1);
    }
}

/// Go's `nn.Uniform`: each element `rand.Float32()*(high-low) + low`, drawn
/// from `rng`.
fn uniform(rng: &mut Rand, low: f32, high: f32, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let data = (0..n).map(|_| rng.float32() * (high - low) + low).collect();
    new_tensor(data, shape)
}

/// Go's `nn.Normal`: each element `float32(rand.NormFloat64())*std + mean`,
/// drawn from `rng`.
fn normal(rng: &mut Rand, mean: f32, std: f32, shape: &[usize]) -> Tensor {
    let n: usize = shape.iter().product();
    let data = (0..n)
        .map(|_| (rng.norm_float64() as f32) * std + mean)
        .collect();
    new_tensor(data, shape)
}

/// A seed for the weights of a new model, from the process's random state,
/// as Go's global source is randomly seeded.
fn random_seed() -> i64 {
    RandomState::new().hash_one(0x6e6e_u64) as i64
}

/// The factorization machine with attention (Go `AFM`).
pub struct Afm {
    base: BaseModel,
    index: Option<UnifiedIndex>,
    // parameters
    b: Tensor,
    w: Lookup,
    v: Lookup,
    a: Vec<Attention>,
    e: Vec<Linear>,
    // hyper parameters
    batch_size: usize,
    n_factors: usize,
    n_epochs: i64,
    lr: f32,
    reg: f32,
    init_mean: f32,
    init_std_dev: f32,
    optimizer: String,
    auto_scale: bool,
    // dataset stats
    num_features: usize,
    num_dimension: usize,
    embedding_dim: Vec<usize>,
    embedding_index: Option<Index>,
    // numerical feature scalers: feature_index -> AutoScaler. Go keeps this as
    // a map that is nil until a fit; an empty map here behaves the same.
    scalers: BTreeMap<i32, AutoScaler>,
    // The source the weights are drawn from. Go draws them from the process
    // global source, which Go seeds randomly; this port seeds a source per
    // model the same way. A test seeds it to reproduce a run (see
    // `set_init_seed`).
    init_rng: Rand,
}

impl Afm {
    /// Go's `NewAFM`: a model with the hyper-parameters of `params`.
    pub fn new(params: Params) -> Self {
        let mut fm = Self {
            base: BaseModel::default(),
            index: None,
            b: zeros(&[]),
            w: Lookup { w: zeros(&[]) },
            v: Lookup { w: zeros(&[]) },
            a: Vec::new(),
            e: Vec::new(),
            batch_size: 1024,
            n_factors: 16,
            n_epochs: 50,
            lr: 0.001,
            reg: 0.0002,
            init_mean: 0.0,
            init_std_dev: 0.01,
            optimizer: ADAM.to_string(),
            auto_scale: true,
            num_features: 0,
            num_dimension: 0,
            embedding_dim: Vec::new(),
            embedding_index: None,
            scalers: BTreeMap::new(),
            init_rng: Rand::new(random_seed()),
        };
        fm.set_params(params);
        fm
    }

    /// Seeds the source the weights of the next [`Afm::init`] are drawn from.
    /// Go's global source reproduces its draws only when
    /// `GODEBUG=randautoseed=0` seeds it with 1, so a replay seeds with 1 too.
    pub fn set_init_seed(&mut self, seed: i64) {
        self.init_rng = Rand::new(seed);
    }

    /// Go's `Init`: creates the layers for `train_set`, fits the scalers when
    /// they are automatic, and keeps the index.
    pub fn init(&mut self, train_set: &dyn CtrSplit) {
        let index = train_set.get_index().expect("train set has no index");
        self.num_features = index.len() as usize;
        self.num_dimension = 0;
        for i in 0..train_set.count() {
            let (_, x, _, _) = train_set.get(i);
            self.num_dimension = self.num_dimension.max(x.len());
        }
        let (num_features, n_factors) = (self.num_features, self.n_factors);
        self.b = zeros(&[]);
        self.w = Lookup::new(&mut self.init_rng, num_features, &[1]);
        self.v = Lookup::new(&mut self.init_rng, num_features, &[n_factors]);
        self.embedding_dim = train_set.get_item_embedding_dim();
        self.embedding_index = train_set.get_item_embedding_index().cloned();
        self.a = Vec::with_capacity(self.embedding_dim.len());
        self.e = Vec::with_capacity(self.embedding_dim.len());
        for &dim in &self.embedding_dim {
            self.a
                .push(Attention::new(&mut self.init_rng, dim, n_factors));
            self.e.push(Linear::new(&mut self.init_rng, dim, n_factors));
        }
        // Collect numerical features and fit scalers if enabled
        if self.auto_scale {
            self.fit_scalers(train_set);
        }
        self.index = Some(index.clone());
    }

    /// Go's `fitScalers`: collects the values of each feature, and fits an
    /// [`AutoScaler`] to each feature that has a value other than 1.
    fn fit_scalers(&mut self, train_set: &dyn CtrSplit) {
        self.scalers = BTreeMap::new();

        // Collect values for each feature index
        let mut feature_values: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
        for i in 0..train_set.count() {
            let (indices, values, _, _) = train_set.get(i);
            for (j, &idx) in indices.iter().enumerate() {
                feature_values.entry(idx).or_default().push(values[j]);
            }
        }

        // Identify numerical features (values not all equal to 1) and fit scalers
        for (idx, values) in feature_values {
            let is_numerical = values.iter().any(|&v| v != 1.0);
            if is_numerical {
                let mut scaler = AutoScaler::new();
                scaler.fit(&values);
                self.scalers.insert(idx, scaler);
            }
        }

        if !self.scalers.is_empty() {
            info!(
                num_numerical_features = self.scalers.len(),
                "fitted scalers for numerical features"
            );
        }
    }

    /// Go's `applyScalers`: a copy of `x` whose numerical values are scaled.
    fn apply_scalers(&self, x: &[(Vec<i32>, Vec<f32>)]) -> Vec<(Vec<i32>, Vec<f32>)> {
        x.iter()
            .map(|(indices, values)| {
                let mut scaled = values.clone();
                for (j, idx) in indices.iter().enumerate() {
                    if let Some(scaler) = self.scalers.get(idx) {
                        scaled[j] = scaler.transform(values[j]);
                    }
                }
                (indices.clone(), scaled)
            })
            .collect()
    }

    /// Go's `convertToTensors`: pads the samples to one shape and makes the
    /// tensors. The target is `None` when `y` is.
    fn convert_to_tensors(
        &self,
        x: &[(Vec<i32>, Vec<f32>)],
        e: &[Vec<Vec<u16>>],
        y: Option<&[f32]>,
    ) -> (Tensor, Tensor, Vec<Tensor>, Option<Tensor>) {
        if let Some(y) = y
            && x.len() != y.len()
        {
            panic!("length of x and y must be equal");
        }

        let mut num_dimension = self.num_dimension;
        for (indices, _) in x {
            num_dimension = num_dimension.max(indices.len());
        }
        let mut aligned_indices = vec![0.0f32; x.len() * num_dimension];
        let mut aligned_values = vec![0.0f32; x.len() * num_dimension];
        let mut aligned_embeddings: Vec<Vec<f32>> = self
            .embedding_dim
            .iter()
            .map(|dim| Vec::with_capacity(x.len() * dim))
            .collect();
        let mut aligned_target = vec![0.0f32; x.len()];
        for (i, (indices, values)) in x.iter().enumerate() {
            if indices.len() != values.len() {
                panic!("length of indices and values must be equal");
            }
            for (j, (&index, &value)) in indices.iter().zip(values).enumerate() {
                aligned_indices[i * num_dimension + j] = index as f32;
                aligned_values[i * num_dimension + j] = value;
            }
            for (j, &dim) in self.embedding_dim.iter().enumerate() {
                match e[i].get(j) {
                    Some(embedding) if embedding.len() == dim => {
                        aligned_embeddings[j].extend(to_float32(embedding));
                    }
                    _ => aligned_embeddings[j].extend(std::iter::repeat_n(0.0f32, dim)),
                }
            }
            if let Some(y) = y {
                aligned_target[i] = y[i];
            }
        }

        let indices_tensor = new_tensor(aligned_indices, &[x.len(), num_dimension]);
        let values_tensor = new_tensor(aligned_values, &[x.len(), num_dimension]);
        let embedding_tensor = aligned_embeddings
            .into_iter()
            .zip(&self.embedding_dim)
            .map(|(data, &dim)| new_tensor(data, &[x.len(), dim]))
            .collect();
        let target_tensor = y.map(|_| new_tensor(aligned_target, &[x.len()]));
        (
            indices_tensor,
            values_tensor,
            embedding_tensor,
            target_tensor,
        )
    }

    /// Go's `Forward`: the logits of a batch, from the feature indices and
    /// values of shape `(batch, dimension)` and the embeddings of each feature.
    fn forward(
        &self,
        indices: &Tensor,
        values: &Tensor,
        embeddings: &[Tensor],
        jobs: usize,
    ) -> Tensor {
        let shape = indices.shape();
        let (batch_size, num_dimension) = (shape[0], shape[1]);
        let v = self.v.forward(indices);
        let x = reshape(values, &[batch_size, num_dimension, 1]);
        let vx = bmm(&v, &x, true, false, jobs);
        let sum_square = square(&vx);
        let e2 = square(&v);
        let x2 = square(&x);
        let square_sum = bmm(&e2, &x2, true, false, jobs);
        let pairwise = sub(&sum_square, &square_sum);
        let pairwise = sum(&pairwise, &[1]);
        let pairwise = mul(&pairwise, &new_scalar(0.5));
        let w = self.w.forward(indices);
        let linear = bmm(&w, &x, true, false, jobs);
        let mut fm_output = add(
            &reshape(&linear, &[batch_size]),
            &[&reshape(&pairwise, &[batch_size]), &self.b],
        );
        // Encode the embedding
        for (i, embedding) in embeddings.iter().enumerate() {
            let encoded_norm = self.e[i].forward(&self.a[i].forward(embedding));
            let encoded_norm = reshape(&encoded_norm, &[batch_size, self.n_factors, 1]);
            let interaction = reshape(&bmm(&vx, &encoded_norm, true, false, jobs), &[batch_size]);
            fm_output = add(&fm_output, &[&interaction]);
        }
        flatten(&fm_output)
    }

    /// Go's `Parameters`: `B`, the parameters of `V` and `W`, then for each
    /// embedding feature the parameters of its attention and its linear layer.
    pub(crate) fn parameters(&self) -> Vec<Tensor> {
        let mut params = vec![self.b.clone()];
        params.extend(self.v.parameters());
        params.extend(self.w.parameters());
        for i in 0..self.embedding_dim.len() {
            params.extend(self.a[i].parameters());
            params.extend(self.e[i].parameters());
        }
        params
    }

    /// Go's `BatchInternalPredict`, in batches of `batch_size`.
    fn batch_internal_predict_impl(
        &self,
        x: &[(Vec<i32>, Vec<f32>)],
        e: &[Vec<Vec<u16>>],
        jobs: usize,
    ) -> Vec<f32> {
        let mut predictions = Vec::with_capacity(x.len());
        for i in (0..x.len()).step_by(self.batch_size) {
            let j = (i + self.batch_size).min(x.len());
            let scaled;
            let batch_x: &[(Vec<i32>, Vec<f32>)] = if self.auto_scale {
                scaled = self.apply_scalers(&x[i..j]);
                &scaled
            } else {
                &x[i..j]
            };
            let batch_e: &[Vec<Vec<u16>>] = if self.embedding_dim.is_empty() {
                &[]
            } else {
                &e[i..j]
            };
            let (indices, values, embeddings, _) = self.convert_to_tensors(batch_x, batch_e, None);
            let output = self.forward(&indices, &values, &embeddings, jobs);
            predictions.extend(output.data());
        }
        predictions.truncate(x.len());
        predictions
    }

    /// Go's `Fit` loop, after the layers are set up. Returns the last score.
    fn run_epochs(
        &mut self,
        ctx: &Context,
        train_set: &dyn CtrSplit,
        test_set: &dyn CtrSplit,
        config: &FitConfig,
        span: &Span,
        score: &mut Score,
    ) -> Option<Score> {
        let mut scores: Vec<(i64, f32)> = vec![(0, score.auc)];
        let mut optimizer: Box<dyn Optimizer> = match self.optimizer.as_str() {
            SGD => Box::new(new_sgd(self.parameters(), self.lr)),
            ADAM => Box::new(new_adam(self.parameters(), self.lr)),
            _ => panic!("unknown optimizer"),
        };
        optimizer.set_weight_decay(self.reg);
        optimizer.set_jobs(config.jobs);
        for epoch in 1..=self.n_epochs {
            let fit_start = Instant::now();
            let mut cost = 0.0f32;
            let count = train_set.count();
            for i in (0..count).step_by(self.batch_size) {
                if ctx.err().is_some() {
                    info!("fit AFM canceled");
                    return None;
                }
                let j = (i + self.batch_size).min(count);
                // Materialize only this batch; repeated item embeddings must not be
                // expanded into FP32 tensors for the entire training set.
                let mut x = Vec::with_capacity(j - i);
                let mut e = Vec::with_capacity(j - i);
                let mut y = Vec::with_capacity(j - i);
                for k in i..j {
                    let (indices, values, embedding, target) = train_set.get(k);
                    x.push((indices, values));
                    e.push(embedding);
                    y.push(target);
                }
                if !self.scalers.is_empty() {
                    x = self.apply_scalers(&x);
                }
                let (batch_indices, batch_values, batch_embedding, batch_target) =
                    self.convert_to_tensors(&x, &e, Some(&y));
                let batch_target = batch_target.expect("training batches have targets");
                let batch_output =
                    self.forward(&batch_indices, &batch_values, &batch_embedding, config.jobs);
                let batch_loss = bce_with_logits(&batch_target, &batch_output, None);
                cost += batch_loss.data()[0];
                optimizer.zero_grad();
                batch_loss.backward();
                optimizer.step();
            }

            let fit_time = fit_start.elapsed();
            // Cross validation
            if epoch % config.verbose == 0 || epoch == self.n_epochs {
                let eval_start = Instant::now();
                *score = evaluate_classification(&*self, test_set, config.jobs);
                scores.push((epoch, score.auc));
                let eval_time = eval_start.elapsed();
                info!(
                    fit_time = ?fit_time,
                    eval_time = ?eval_time,
                    loss = cost,
                    accuracy = score.accuracy,
                    precision = score.precision,
                    recall = score.recall,
                    auc = score.auc,
                    "fit AFM {epoch}/{}",
                    self.n_epochs
                );
                // check NaN
                if math32::is_nan(cost) || math32::is_nan(score.get_value()) {
                    warn!(lr = self.lr, "model diverged");
                    break;
                }
                // early stopping if no improvement in last `patience` epochs
                if config.patience > 0 && epoch > config.patience {
                    let epoch_score = best_epoch(&scores);
                    if epoch_score.0 <= epoch - config.patience {
                        info!(
                            best_epoch = epoch_score.0,
                            best_auc = epoch_score.1,
                            patience = config.patience,
                            "early stopping"
                        );
                        break;
                    }
                }
            }
            span.add(1);
        }
        Some(*score)
    }
}

/// Go's `lo.MaxBy` over the scores by AUC: the first score with the greatest
/// AUC, since a later score replaces it only when strictly greater.
fn best_epoch(scores: &[(i64, f32)]) -> (i64, f32) {
    let mut best = scores[0];
    for &item in &scores[1..] {
        if item.1 > best.1 {
            best = item;
        }
    }
    best
}

/// Converts model parameters to their gob form (Go encodes a `model.Params`
/// map). A value gob cannot carry is an error.
fn to_gob_params(params: &Params) -> Result<GobParams, Error> {
    let mut out = BTreeMap::new();
    for (name, value) in &params.0 {
        let gob_value = match value {
            ParamValue::Nil => GobValue::Nil,
            ParamValue::Int(v) => GobValue::Int(*v),
            ParamValue::Int32(v) => GobValue::Int32(*v),
            ParamValue::Int64(v) => GobValue::Int64(*v),
            ParamValue::Float32(v) => GobValue::Float32(*v),
            ParamValue::Float64(v) => GobValue::Float64(*v),
            ParamValue::Bool(v) => GobValue::Bool(*v),
            ParamValue::String(v) => GobValue::String(v.clone()),
            ParamValue::IntSlice(v) => GobValue::IntSlice(v.clone()),
            ParamValue::Json(_) | ParamValue::Other(_) => {
                return Err(Error::UnsupportedParam(name.clone()));
            }
        };
        out.insert(name.clone(), gob_value);
    }
    Ok(GobParams(out))
}

/// The inverse of [`to_gob_params`].
fn from_gob_params(params: GobParams) -> Params {
    Params(
        params
            .0
            .into_iter()
            .map(|(name, value)| {
                let value = match value {
                    GobValue::Nil => ParamValue::Nil,
                    GobValue::Int(v) => ParamValue::Int(v),
                    GobValue::Int32(v) => ParamValue::Int32(v),
                    GobValue::Int64(v) => ParamValue::Int64(v),
                    GobValue::Float32(v) => ParamValue::Float32(v),
                    GobValue::Float64(v) => ParamValue::Float64(v),
                    GobValue::Bool(v) => ParamValue::Bool(v),
                    GobValue::String(v) => ParamValue::String(v),
                    GobValue::IntSlice(v) => ParamValue::IntSlice(v),
                };
                (name, value)
            })
            .collect(),
    )
}

impl Model for Afm {
    /// Go's `SetParams`: stores the hyper-parameters, with Go's defaults for
    /// those that are missing.
    fn set_params(&mut self, params: Params) {
        self.base.set_params(params);
        let p = self.base.get_params();
        self.batch_size = p.get_int(BATCH_SIZE, 1024) as usize;
        self.n_factors = p.get_int(N_FACTORS, 16) as usize;
        self.n_epochs = p.get_int(N_EPOCHS, 50);
        self.lr = p.get_float32(LR, 0.001);
        self.reg = p.get_float32(REG, 0.0002);
        self.init_mean = p.get_float32(INIT_MEAN, 0.0);
        self.init_std_dev = p.get_float32(INIT_STD_DEV, 0.01);
        self.optimizer = p.get_string(OPTIMIZER, ADAM);
        self.auto_scale = p.get_bool(AUTO_SCALE, true);
    }

    fn get_params(&self) -> &Params {
        self.base.get_params()
    }

    /// Go's `AFM.SuggestParams`.
    fn suggest_params(&self, trial: &mut dyn SuggestTrial) -> Params {
        Params(BTreeMap::from([
            (N_FACTORS.to_owned(), ParamValue::Int(16)),
            (
                LR.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(LR, 0.001, 0.1)),
            ),
            (
                REG.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(REG, 0.001, 0.1)),
            ),
            (INIT_MEAN.to_owned(), ParamValue::Int(0)),
            (
                INIT_STD_DEV.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(INIT_STD_DEV, 0.001, 0.1)),
            ),
        ]))
    }

    /// Go's `Clear`: drops the index and the scalers. The weights stay.
    fn clear(&mut self) {
        self.index = None;
        self.scalers = BTreeMap::new();
    }

    /// Go's `Invalid`: true until the model has an index.
    fn invalid(&self) -> bool {
        self.index.is_none()
    }
}

impl FactorizationMachines for Afm {
    fn header(&self) -> &'static str {
        if self.auto_scale {
            HEADER_AFM2
        } else {
            HEADER_AFM
        }
    }

    /// Go's `Predict`, which panics for a deep learning model.
    fn predict(
        &self,
        _user_id: &str,
        _item_id: &str,
        _user_features: &[Label],
        _item_features: &[Label],
    ) -> f32 {
        panic!("Predict is unsupported for deep learning models");
    }

    /// Go's `InternalPredict`, which panics for a deep learning model.
    fn internal_predict(&self, _x: &[i32], _values: &[f32]) -> f32 {
        panic!("InternalPredict is unsupported for deep learning models");
    }

    /// Go's `Fit`: trains with binary cross-entropy in batches, evaluates the
    /// AUC on `test_set` every `verbose` epochs, and stops early after
    /// `patience` epochs without an improvement.
    fn fit(
        &mut self,
        ctx: &Context,
        train_set: &dyn CtrSplit,
        test_set: &dyn CtrSplit,
        config: &FitConfig,
    ) -> Score {
        info!(
            train_set_size = train_set.count(),
            test_set_size = test_set.count(),
            params = ?self.base.get_params(),
            config = ?config,
            "fit AFM"
        );
        self.init(train_set);
        for i in 0..self.embedding_dim.len() {
            self.a[i].set_jobs(config.jobs);
            self.e[i].set_jobs(config.jobs);
        }

        let eval_start = Instant::now();
        let mut score = evaluate_classification(&*self, test_set, config.jobs);
        let eval_time = eval_start.elapsed();
        info!(
            eval_time = ?eval_time,
            accuracy = score.accuracy,
            precision = score.precision,
            recall = score.recall,
            auc = score.auc,
            "fit AFM 0/{}",
            self.n_epochs
        );

        let (_, span) = monitor::start(ctx, "FM.Fit", self.n_epochs);
        let result = self.run_epochs(ctx, train_set, test_set, config, &span, &mut score);
        span.end();
        result.unwrap_or_default()
    }

    fn marshal<W: Write>(&self, w: &mut W) -> Result<(), Error>
    where
        Self: Sized,
    {
        // write params
        write_gob(w, &to_gob_params(self.base.get_params())?)?;
        // write index
        marshal_unified_index(w, self.index.as_ref())?;
        // write dataset stats
        write_gob(w, &(self.num_features as i64))?;
        write_gob(w, &(self.num_dimension as i64))?;
        let embedding_dim: Vec<i64> = self.embedding_dim.iter().map(|&d| d as i64).collect();
        write_gob(w, &embedding_dim)?;
        if !self.embedding_dim.is_empty() {
            let index = self
                .embedding_index
                .as_ref()
                .ok_or(Error::MissingEmbeddingIndex)?;
            gorse_dataset::index::marshal_index(w, index)?;
        }
        // write scalers
        if self.auto_scale {
            write_gob(w, &(self.scalers.len() as i64))?;
            for (idx, scaler) in &self.scalers {
                write_gob(w, idx)?;
                scaler.marshal(w)?;
            }
        }
        // write parameters
        nn::save(&self.parameters(), w)?;
        Ok(())
    }

    fn as_batch_inference(&self) -> Option<&dyn BatchInference> {
        Some(self)
    }
}

impl Afm {
    /// Reads a model after its header. `auto_scale` is what the header says
    /// (Go sets `autoScale` from the header before `Unmarshal`). The
    /// parameters then set it again, as Go's `SetParams` does.
    pub fn unmarshal<R: Read>(r: &mut R, auto_scale: bool) -> Result<Self, Error> {
        let mut fm = Afm::new(Params::default());
        fm.auto_scale = auto_scale;
        // read params
        let params: GobParams = read_gob(r)?;
        fm.set_params(from_gob_params(params));
        // read index
        fm.index = unmarshal_unified_index(r)?;
        // read dataset stats
        fm.num_features = read_gob::<i64>(r)? as usize;
        fm.num_dimension = read_gob::<i64>(r)? as usize;
        let embedding_dim: Vec<i64> = read_gob(r)?;
        fm.embedding_dim = embedding_dim.into_iter().map(|d| d as usize).collect();
        if !fm.embedding_dim.is_empty() {
            fm.embedding_index = Some(gorse_dataset::index::unmarshal_index(r)?);
        }
        // read scalers
        if fm.auto_scale {
            let num_scalers: i64 = read_gob(r)?;
            fm.scalers = BTreeMap::new();
            for _ in 0..num_scalers {
                let idx: i32 = read_gob(r)?;
                let mut scaler = AutoScaler::new();
                scaler.unmarshal(r)?;
                fm.scalers.insert(idx, scaler);
            }
        }
        // read parameters. The layers are created with the values that
        // `nn.Load` then replaces, so their draws do not matter.
        let (num_features, n_factors) = (fm.num_features, fm.n_factors);
        let mut rng = Rand::new(0);
        fm.b = zeros(&[]);
        fm.w = Lookup::new(&mut rng, num_features, &[1]);
        fm.v = Lookup::new(&mut rng, num_features, &[n_factors]);
        fm.a = Vec::with_capacity(fm.embedding_dim.len());
        fm.e = Vec::with_capacity(fm.embedding_dim.len());
        for &dim in &fm.embedding_dim {
            fm.a.push(Attention::new(&mut rng, dim, n_factors));
            fm.e.push(Linear::new(&mut rng, dim, n_factors));
        }
        nn::load(&fm.parameters(), r)?;
        Ok(fm)
    }
}

impl BatchInference for Afm {
    /// Go's `BatchPredict`: encodes the names of the inputs with the index,
    /// then predicts them in batches.
    fn batch_predict(
        &self,
        inputs: &[(String, String, Vec<Label>, Vec<Label>)],
        embeddings: &[Vec<Embedding>],
        jobs: usize,
    ) -> Vec<f32> {
        let index = self.index.as_ref().expect("model has no index");
        let mut x: Vec<(Vec<i32>, Vec<f32>)> = vec![(Vec::new(), Vec::new()); inputs.len()];
        for (i, (user, item, user_labels, item_labels)) in inputs.iter().enumerate() {
            // encode user
            let user_index = index.encode_user(user);
            if user_index != NOT_ID {
                x[i].0.push(user_index);
                x[i].1.push(1.0);
            }
            // encode item
            let item_index = index.encode_item(item);
            if item_index != NOT_ID {
                x[i].0.push(item_index);
                x[i].1.push(1.0);
            }
            // encode user labels
            for user_feature in user_labels {
                let feature_index = index.encode_user_label(&user_feature.name);
                if feature_index != NOT_ID {
                    x[i].0.push(feature_index);
                    x[i].1.push(user_feature.value);
                }
            }
            // encode item labels
            for item_feature in item_labels {
                let feature_index = index.encode_item_label(&item_feature.name);
                if feature_index != NOT_ID {
                    x[i].0.push(feature_index);
                    x[i].1.push(item_feature.value);
                }
            }
        }
        let mut e: Vec<Vec<Vec<u16>>> = Vec::with_capacity(inputs.len());
        for embedding_list in &embeddings[..inputs.len()] {
            let mut row = vec![Vec::new(); self.embedding_dim.len()];
            if let Some(embedding_index) = &self.embedding_index {
                for embedding in embedding_list {
                    let item_index = embedding_index.to_number(&embedding.name);
                    if item_index == NOT_ID {
                        // unknown embedding
                        continue;
                    }
                    let index = item_index as usize;
                    if embedding.value.len() != self.embedding_dim[index] {
                        // dimension mismatch
                        continue;
                    }
                    row[index] = embedding.value.clone();
                }
            }
            e.push(row);
        }
        self.batch_internal_predict_impl(&x, &e, jobs)
    }

    fn batch_internal_predict(
        &self,
        x: &[(Vec<i32>, Vec<f32>)],
        e: &[Vec<Vec<u16>>],
        jobs: usize,
    ) -> Vec<f32> {
        self.batch_internal_predict_impl(x, e, jobs)
    }
}
