// Ports: model/cf/model.go @ add21b5

//! Bayesian personalized ranking (BPR) and alternating least squares (ALS), the
//! base they share, and the marshalling of their parameters and factors.
//!
//! Go's `BaseMatrixFactorization` is embedded in both models. Here it is a
//! field, and [`BPR`] and [`ALS`] delegate to it.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use fixedbitset::FixedBitSet;
use gorse_common::encoding::{self, gob};
use gorse_common::floats;
use gorse_common::gocontext::{Context, ContextError};
use gorse_common::gojson::Value;
use gorse_common::math32;
use gorse_common::monitor;
use gorse_common::parallel;
use gorse_common::util::{self, RandomGenerator};
use gorse_dataset::dataset::CfSplit;
use gorse_dataset::dict::FreqDict;
use gorse_model_core::model::{BaseModel, Model as CoreModel, SuggestTrial};
use gorse_model_core::params::{
    ALPHA, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, ParamValue, Params, REG,
};
use gorse_protocol::LatentFactor;
use prost::Message;

use super::evaluator::{Metric, evaluate, ndcg, precision, recall};
use super::lock;

/// Go's `Score`: the metrics of a fit's last evaluation. Go's struct has no
/// JSON tags, so the stored JSON names the fields as Go does.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Score {
    /// NDCG at the configured top-k.
    #[serde(rename = "NDCG")]
    pub ndcg: f32,
    /// Precision at the configured top-k.
    #[serde(rename = "Precision")]
    pub precision: f32,
    /// Recall at the configured top-k.
    #[serde(rename = "Recall")]
    pub recall: f32,
}

/// Go's `FitConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct FitConfig {
    /// The number of parallel jobs.
    pub jobs: usize,
    /// The number of epochs between evaluations. The last epoch is always evaluated.
    pub verbose: usize,
    /// The number of negative candidates sampled per user for evaluation.
    pub candidates: usize,
    /// The k of the top-k evaluation metrics.
    pub top_k: usize,
    /// Stop after this many evaluated epochs without a better NDCG. 0 never stops early.
    pub patience: usize,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl FitConfig {
    /// Go's `NewFitConfig`.
    pub fn new() -> Self {
        FitConfig {
            jobs: 1,
            verbose: 10,
            candidates: 100,
            top_k: 10,
            patience: 0,
        }
    }

    /// Go's `SetVerbose`.
    pub fn set_verbose(mut self, verbose: usize) -> Self {
        self.verbose = verbose;
        self
    }

    /// Go's `SetJobs`.
    pub fn set_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs;
        self
    }

    /// Go's `SetPatience`.
    pub fn set_patience(mut self, patience: usize) -> Self {
        self.patience = patience;
        self
    }
}

/// Go's `Model` interface: a collaborative-filtering model that is fitted on a
/// train split and evaluated on a validation split.
pub trait Model: CoreModel + Sync {
    /// Go's `Fit`: trains on `train_set`, evaluates on `val_set` and returns the
    /// score of the last evaluation. Returns the zero score when `ctx` is done.
    fn fit(
        &mut self,
        ctx: &Context,
        train_set: &dyn CfSplit,
        val_set: &mut dyn CfSplit,
        config: &FitConfig,
    ) -> Score;

    /// Go's `GetItemIndex`: the item dictionary, or `None` when the model is not fitted.
    fn get_item_index(&self) -> Option<&FreqDict>;

    /// Go's `Marshal`: the parameters, then the factors, without the model name.
    fn marshal(&self, w: &mut dyn Write) -> Result<(), Error>;

    /// Go's `Unmarshal`: reads what [`Model::marshal`] wrote.
    fn unmarshal(&mut self, r: &mut dyn Read) -> Result<(), Error>;

    /// Go's `GetUserFactor`. Panics when the model is not fitted or the index is out of range.
    fn get_user_factor(&self, user_index: i32) -> &[f32];

    /// Go's `GetItemFactor`. Panics when the model is not fitted or the index is out of range.
    fn get_item_factor(&self, item_index: i32) -> &[f32];

    /// The name [`marshal_model`] writes, which Go's `GetModelName` gives by a type switch.
    fn model_name(&self) -> &'static str;
}

/// Go's `MatrixFactorization` interface: a [`Model`] that predicts the rating a
/// user gives an item.
pub trait MatrixFactorization: Model {
    /// Go's `Predict`: predicts from external user and item IDs. Unknown IDs are
    /// logged and predicted as 0.
    fn predict(&self, user_id: &str, item_id: &str) -> f32;

    /// Go's `internalPredict`: predicts from user and item indices. An index
    /// of -1 means unknown, and predicts 0.
    fn internal_predict(&self, user_index: i32, item_index: i32) -> f32;

    /// Go's `GetUserIndex`: the user dictionary, or `None` when the model is not fitted.
    fn get_user_index(&self) -> Option<&FreqDict>;

    /// Go's `IsUserPredictable`: false when the user has no feedback, so its factor was never trained.
    fn is_user_predictable(&self, user_index: i32) -> bool;

    /// Go's `IsItemPredictable`: false when the item has no feedback, so its factor was never trained.
    fn is_item_predictable(&self, item_index: i32) -> bool;
}

/// Errors of [`Model::marshal`], [`Model::unmarshal`] and the model functions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A read or write of the stream failed, or the stream ended early.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The gob-encoded parameters could not be encoded or decoded.
    #[error(transparent)]
    Encoding(#[from] encoding::Error),
    /// A latent factor message could not be decoded.
    #[error(transparent)]
    Decode(#[from] prost::DecodeError),
    /// A parameter has a Go type that gob does not encode.
    #[error("gob: type not registered for interface: {0}")]
    UnregisteredType(&'static str),
    /// A count read from the stream is negative.
    #[error("negative count {0}")]
    NegativeCount(i64),
    /// The model name read by [`unmarshal_model`] is not one of the models.
    #[error("unknown model {0}")]
    UnknownModel(String),
}

/// Go's `GetModelName`.
pub fn get_model_name(m: &dyn Model) -> &'static str {
    m.model_name()
}

/// Go's `MarshalModel`: writes the model name, then the model.
pub fn marshal_model(w: &mut dyn Write, m: &dyn Model) -> Result<(), Error> {
    encoding::write_string(&mut &mut *w, m.model_name())?;
    m.marshal(w)
}

/// Go's `UnmarshalModel`: reads a model name, then the model of that name.
pub fn unmarshal_model(r: &mut dyn Read) -> Result<Box<dyn MatrixFactorization>, Error> {
    let name = encoding::read_string(&mut &mut *r)?;
    match name.as_str() {
        "bpr" => {
            let mut bpr = BPR::default();
            bpr.unmarshal(r)?;
            Ok(Box::new(bpr))
        }
        "als" => {
            let mut als = ALS::default();
            als.unmarshal(r)?;
            Ok(Box::new(als))
        }
        _ => Err(Error::UnknownModel(name)),
    }
}

/// The fitted state of a factorization model: Go's `UserIndex`, `ItemIndex`,
/// `UserPredictable`, `ItemPredictable`, `UserFactor` and `ItemFactor`.
#[derive(Debug)]
struct Fitted {
    user_index: FreqDict,
    item_index: FreqDict,
    user_predictable: FixedBitSet,
    item_predictable: FixedBitSet,
    user_factor: Vec<Vec<f32>>,
    item_factor: Vec<Vec<f32>>,
}

/// Go's `BaseMatrixFactorization`: what BPR and ALS share. The model is
/// fitted from `Init` until `Clear`, and Go's `Invalid` is true when it is not.
#[derive(Default)]
pub struct BaseMatrixFactorization {
    base: BaseModel,
    fitted: Option<Fitted>,
}

impl BaseMatrixFactorization {
    /// The hyper-parameters (Go's `Params`).
    fn params(&self) -> &Params {
        self.base.get_params()
    }

    /// Go's `SetParams`, which is `BaseModel.SetParams`.
    fn set_params(&mut self, params: Params) {
        self.base.set_params(params);
    }

    /// Go's `GetRandomGenerator`.
    fn random_generator(&mut self) -> &mut RandomGenerator {
        self.base.get_random_generator()
    }

    /// The fitted state. Panics when the model is not fitted, as Go's nil
    /// dereference does.
    fn fitted(&self) -> &Fitted {
        self.fitted.as_ref().expect("model is not fitted")
    }

    /// Moves the fitted state out, so that one epoch's jobs can own it under a
    /// lock. [`BaseMatrixFactorization::restore_fitted`] puts it back.
    fn take_fitted(&mut self) -> Fitted {
        self.fitted.take().expect("model is not fitted")
    }

    fn restore_fitted(&mut self, fitted: Fitted) {
        self.fitted = Some(fitted);
    }

    /// Go's `BaseMatrixFactorization.Init`, after the models have set the
    /// factors: the indices and the predictable flags.
    fn init(
        &mut self,
        train_set: &dyn CfSplit,
        user_factor: Vec<Vec<f32>>,
        item_factor: Vec<Vec<f32>>,
    ) {
        let user_index = copy_dict(train_set.get_user_dict());
        let item_index = copy_dict(train_set.get_item_dict());
        self.fitted = Some(Fitted {
            user_predictable: predictable(train_set.get_user_feedback(), user_index.count()),
            item_predictable: predictable(train_set.get_item_feedback(), item_index.count()),
            user_index,
            item_index,
            user_factor,
            item_factor,
        });
    }

    /// Go's `Clear`: drops the fitted state.
    fn clear(&mut self) {
        self.fitted = None;
    }

    /// Go's `Invalid`.
    fn invalid(&self) -> bool {
        self.fitted.is_none()
    }

    /// Go's `GetUserIndex`.
    fn get_user_index(&self) -> Option<&FreqDict> {
        self.fitted.as_ref().map(|fitted| &fitted.user_index)
    }

    /// Go's `GetItemIndex`.
    fn get_item_index(&self) -> Option<&FreqDict> {
        self.fitted.as_ref().map(|fitted| &fitted.item_index)
    }

    /// Go's `GetUserFactor`.
    fn user_factor(&self, user_index: i32) -> &[f32] {
        &self.fitted().user_factor[user_index as usize]
    }

    /// Go's `GetItemFactor`.
    fn item_factor(&self, item_index: i32) -> &[f32] {
        &self.fitted().item_factor[item_index as usize]
    }

    /// Go's `Predict`.
    fn predict(&self, user_id: &str, item_id: &str) -> f32 {
        // Convert sparse Names to dense Names
        let fitted = self.fitted();
        let user_index = fitted.user_index.id(user_id);
        let item_index = fitted.item_index.id(item_id);
        if user_index < 0 {
            tracing::warn!(user_id = user_id, "unknown user");
        }
        if item_index < 0 {
            tracing::warn!(item_id = item_id, "unknown item");
        }
        self.internal_predict(user_index, item_index)
    }

    /// Go's `internalPredict`.
    fn internal_predict(&self, user_index: i32, item_index: i32) -> f32 {
        if item_index >= 0 && user_index >= 0 {
            let fitted = self.fitted();
            floats::dot(
                &fitted.user_factor[user_index as usize],
                &fitted.item_factor[item_index as usize],
            )
        } else {
            tracing::warn!("unknown user or item");
            0.0
        }
    }

    /// Go's `IsUserPredictable`.
    fn is_user_predictable(&self, user_index: i32) -> bool {
        let fitted = self.fitted();
        if user_index >= fitted.user_index.count() || user_index < 0 {
            return false;
        }
        fitted.user_predictable.contains(user_index as usize)
    }

    /// Go's `IsItemPredictable`.
    fn is_item_predictable(&self, item_index: i32) -> bool {
        let fitted = self.fitted();
        if item_index >= fitted.item_index.count() || item_index < 0 {
            return false;
        }
        fitted.item_predictable.contains(item_index as usize)
    }

    /// Go's `BaseMatrixFactorization.Marshal`: the parameters, the user factors
    /// and the item factors.
    fn marshal(&self, w: &mut dyn Write) -> Result<(), Error> {
        let fitted = self.fitted();
        // write params
        write_gob_to(w, &to_gob(&self.base.params)?)?;
        // write user latent factors, then item latent factors
        write_latent_factors(
            w,
            &fitted.user_index,
            &fitted.user_predictable,
            &fitted.user_factor,
        )?;
        write_latent_factors(
            w,
            &fitted.item_index,
            &fitted.item_predictable,
            &fitted.item_factor,
        )
    }

    /// Go's `BaseMatrixFactorization.Unmarshal`. Unlike Go, it leaves the model
    /// as it was when a read fails, and not half-read.
    fn unmarshal(&mut self, r: &mut dyn Read) -> Result<(), Error> {
        // read params
        self.base.params = from_gob(read_gob_from(r)?);
        // read user latent factors, then item latent factors
        let (user_index, user_predictable, user_factor) = read_latent_factors(r)?;
        let (item_index, item_predictable, item_factor) = read_latent_factors(r)?;
        self.fitted = Some(Fitted {
            user_index,
            item_index,
            user_predictable,
            item_predictable,
            user_factor,
            item_factor,
        });
        Ok(())
    }
}

/// Go's `dataset.FreqDict` copy of a split's dictionary. Go shares the pointer;
/// the model keeps its own copy, with each string's count, so that a later change
/// to the split does not change the model.
fn copy_dict(source: &FreqDict) -> FreqDict {
    let mut dict = FreqDict::new();
    for id in 0..source.count() {
        let name = source.string(id).unwrap_or_default();
        match source.freq(id) {
            0 => {
                dict.add_no_count(name);
            }
            freq => {
                for _ in 0..freq {
                    dict.add(name);
                }
            }
        }
    }
    dict
}

/// Go's `IsUserPredictable` and `IsItemPredictable` flags: set for every index
/// in `0..count` whose feedback list is not empty.
fn predictable(feedback: &[Vec<i32>], count: i32) -> FixedBitSet {
    let count = count as usize;
    let mut bits = FixedBitSet::with_capacity(count);
    for (index, items) in feedback.iter().enumerate().take(count) {
        if !items.is_empty() {
            bits.insert(index);
        }
    }
    bits
}

/// Go's `writeLatentFactors` loop in `BaseMatrixFactorization.Marshal`: the count
/// of predictable entries, then each predictable entry as a delimited
/// `protocol.LatentFactor`.
fn write_latent_factors(
    w: &mut dyn Write,
    index: &FreqDict,
    predictable: &FixedBitSet,
    factors: &[Vec<f32>],
) -> Result<(), Error> {
    write_i64(w, predictable.count_ones(..) as i64)?;
    for (i, factor) in factors.iter().enumerate().take(index.count() as usize) {
        if predictable.contains(i) {
            let latent = LatentFactor {
                id: index.string(i as i32).unwrap_or_default().to_owned(),
                data: factor.clone(),
            };
            w.write_all(&latent.encode_length_delimited_to_vec())?;
        }
    }
    Ok(())
}

/// Go's latent factor loop in `BaseMatrixFactorization.Unmarshal`: reads the
/// count, then that many delimited latent factors, and rebuilds the index.
fn read_latent_factors(r: &mut dyn Read) -> Result<(FreqDict, FixedBitSet, Vec<Vec<f32>>), Error> {
    let count = read_i64(r)?;
    let count = usize::try_from(count).map_err(|_| Error::NegativeCount(count))?;
    let mut index = FreqDict::new();
    let mut predictable = FixedBitSet::with_capacity(count);
    let mut factors = vec![Vec::new(); count];
    for _ in 0..count {
        let latent = read_delimited(r)?;
        let i = index.add(&latent.id) as usize;
        predictable.insert(i);
        factors[i] = latent.data;
    }
    Ok((index, predictable, factors))
}

/// `pbutil.WriteDelimited` of a `LatentFactor` is written by
/// [`Message::encode_length_delimited_to_vec`] in the caller.
///
/// `pbutil.ReadDelimited`: an unsigned varint length, then that many bytes of
/// message.
fn read_delimited(r: &mut dyn Read) -> Result<LatentFactor, Error> {
    let len = read_uvarint(r)?;
    let len = usize::try_from(len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message too long"))?;
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    Ok(LatentFactor::decode(buf.as_slice())?)
}

/// Go's `binary.ReadUvarint`.
fn read_uvarint(r: &mut dyn Read) -> io::Result<u64> {
    let mut x = 0u64;
    let mut shift = 0u32;
    for i in 0..10 {
        let mut byte = [0u8; 1];
        r.read_exact(&mut byte)?;
        if byte[0] < 0x80 {
            if i == 9 && byte[0] > 1 {
                break;
            }
            return Ok(x | u64::from(byte[0]) << shift);
        }
        x |= u64::from(byte[0] & 0x7f) << shift;
        shift += 7;
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "varint overflows a 64-bit integer",
    ))
}

/// Go's `binary.Write` of an `int64`, little-endian.
fn write_i64(w: &mut dyn Write, v: i64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

/// Go's `binary.Read` of an `int64`, little-endian.
fn read_i64(r: &mut dyn Read) -> io::Result<i64> {
    let mut bytes = [0u8; 8];
    r.read_exact(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

/// `encoding.WriteGob` through a trait object, which the generic function does
/// not take directly.
fn write_gob_to<T: gob::GobValue>(w: &mut dyn Write, value: &T) -> Result<(), Error> {
    encoding::write_gob(&mut &mut *w, value)?;
    Ok(())
}

/// `encoding.ReadGob` through a trait object.
fn read_gob_from<T: gob::GobValue>(r: &mut dyn Read) -> Result<T, Error> {
    Ok(encoding::read_gob(&mut &mut *r)?)
}

/// `model.Params` in gob form. Go gob-encodes the map as `map[ParamName]any`, so
/// each value must be a type gob has registered.
fn to_gob(params: &Params) -> Result<gob::Params, Error> {
    let mut out = BTreeMap::new();
    for (name, value) in &params.0 {
        let value = match value {
            ParamValue::Nil => gob::Value::Nil,
            ParamValue::Int(v) => gob::Value::Int(*v),
            ParamValue::Int32(v) => gob::Value::Int32(*v),
            ParamValue::Int64(v) => gob::Value::Int64(*v),
            ParamValue::Float32(v) => gob::Value::Float32(*v),
            ParamValue::Float64(v) => gob::Value::Float64(*v),
            ParamValue::Bool(v) => gob::Value::Bool(*v),
            ParamValue::String(v) => gob::Value::String(v.clone()),
            ParamValue::IntSlice(v) => gob::Value::IntSlice(v.clone()),
            ParamValue::Json(Value::Object(_)) => {
                return Err(Error::UnregisteredType("map[string]interface {}"));
            }
            ParamValue::Json(_) => return Err(Error::UnregisteredType("[]interface {}")),
            ParamValue::Other(name) => return Err(Error::UnregisteredType(name)),
        };
        out.insert(name.clone(), value);
    }
    Ok(gob::Params(out))
}

/// The inverse of [`to_gob`], for the parameters that [`BaseMatrixFactorization::unmarshal`] reads.
fn from_gob(params: gob::Params) -> Params {
    Params(
        params
            .0
            .into_iter()
            .map(|(name, value)| {
                let value = match value {
                    gob::Value::Nil => ParamValue::Nil,
                    gob::Value::Int(v) => ParamValue::Int(v),
                    gob::Value::Int32(v) => ParamValue::Int32(v),
                    gob::Value::Int64(v) => ParamValue::Int64(v),
                    gob::Value::Float32(v) => ParamValue::Float32(v),
                    gob::Value::Float64(v) => ParamValue::Float64(v),
                    gob::Value::Bool(v) => ParamValue::Bool(v),
                    gob::Value::String(v) => ParamValue::String(v),
                    gob::Value::IntSlice(v) => ParamValue::IntSlice(v),
                };
                (name, value)
            })
            .collect(),
    )
}

/// A count parameter as Go reads it. A negative value panics in Go when it sizes
/// a slice; here it counts as 0 (`porting/notes/WP-3-1.md`).
fn count_param(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

/// Go's `ScoreFor` pair of the evaluation block: NDCG, precision and recall at the
/// configured top-k, computed on the validation split.
fn evaluate_score(
    estimator: &dyn MatrixFactorization,
    val_set: &mut dyn CfSplit,
    train_set: &dyn CfSplit,
    config: &FitConfig,
) -> Score {
    let scorers: [Metric; 3] = [ndcg, precision, recall];
    let s = evaluate(
        estimator,
        val_set,
        train_set,
        config.top_k,
        config.candidates,
        config.jobs,
        &scorers,
    );
    Score {
        ndcg: s[0],
        precision: s[1],
        recall: s[2],
    }
}

/// The epoch history of a fit, which early stopping reads.
struct History {
    /// The (epoch, NDCG) of every evaluation, starting with epoch 0.
    scores: Vec<(usize, f32)>,
    /// The last evaluation.
    score: Score,
}

impl History {
    fn new(initial: Score) -> Self {
        History {
            scores: vec![(0, initial.ndcg)],
            score: initial,
        }
    }

    /// Go's `lo.MaxBy` over the history: the first entry with the largest NDCG.
    fn best(&self) -> (usize, f32) {
        let Some(&first) = self.scores.first() else {
            return (0, 0.0);
        };
        let mut best = first;
        for &entry in &self.scores[1..] {
            if entry.1 > best.1 {
                best = entry;
            }
        }
        best
    }

    /// Go's early stopping check after an evaluated epoch: true when the best
    /// NDCG is at least `patience` epochs old.
    fn should_stop(&self, epoch: usize, patience: usize) -> bool {
        if patience == 0 || epoch <= patience {
            return false;
        }
        let (best_epoch, _) = self.best();
        best_epoch <= epoch - patience
    }
}

/// Ends a monitor span when dropped, as Go's `defer span.End()` does.
struct SpanGuard(Arc<monitor::Span>);

impl Drop for SpanGuard {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// The training data one epoch's jobs read.
struct Interactions<'a> {
    /// Each user's positive items, indexed by user.
    user_feedback: &'a [Vec<i32>],
    /// The same items as sets, for the negative sampling of BPR.
    user_sets: &'a [HashSet<i32>],
    /// The number of users.
    n_users: usize,
    /// The number of items.
    n_items: usize,
}

/// One pairwise update of BPR (the body of Go's epoch job): samples a user with
/// feedback, one of that user's items as the positive, and an item the user has
/// not interacted with as the negative, then moves the three factors along the
/// gradient of the pairwise loss.
fn bpr_update(
    fitted: &mut Fitted,
    rng: &mut RandomGenerator,
    data: &Interactions<'_>,
    reg: f32,
    lr: f32,
) {
    // Select a user
    let (user, rating_count) = loop {
        let user = rng.int31n(data.n_users as i32) as usize;
        let rating_count = data.user_feedback[user].len();
        if rating_count > 0 {
            break (user, rating_count);
        }
    };
    let pos = data.user_feedback[user][rng.intn(rating_count as i64) as usize] as usize;
    // Select a negative sample
    let neg = loop {
        let temp = rng.int31n(data.n_items as i32);
        if !data.user_sets[user].contains(&temp) {
            break temp as usize;
        }
    };
    let diff = floats::dot(&fitted.user_factor[user], &fitted.item_factor[pos])
        - floats::dot(&fitted.user_factor[user], &fitted.item_factor[neg]);
    let grad = math32::exp(-diff) / (1.0 + math32::exp(-diff));
    // Pairwise update
    let user_copy = fitted.user_factor[user].clone();
    let pos_copy = fitted.item_factor[pos].clone();
    let neg_copy = fitted.item_factor[neg].clone();
    let mut temp = vec![0f32; user_copy.len()];
    // Update positive item latent factor: +w_u
    floats::mul_const_to(&user_copy, grad, &mut temp);
    floats::mul_const_add(&pos_copy, -reg, &mut temp);
    floats::mul_const_add(&temp, lr, &mut fitted.item_factor[pos]);
    // Update negative item latent factor: -w_u
    floats::mul_const_to(&user_copy, -grad, &mut temp);
    floats::mul_const_add(&neg_copy, -reg, &mut temp);
    floats::mul_const_add(&temp, lr, &mut fitted.item_factor[neg]);
    // Update user latent factor: h_i-h_j
    floats::sub_to(&pos_copy, &neg_copy, &mut temp);
    floats::mul_const(&mut temp, grad);
    floats::mul_const_add(&user_copy, -reg, &mut temp);
    floats::mul_const_add(&temp, lr, &mut fitted.user_factor[user]);
}

/// Bayesian personalized ranking.
///
/// BPR is a pairwise learning algorithm for matrix factorization with implicit
/// feedback. The pairwise ranking between item i and j for user u is estimated
/// by:
///
/// ```text
/// p(i >_u j) = \sigma( p_u^T (q_i - q_j) )
/// ```
///
/// Hyper-parameters:
///
/// - `Reg`: the regularization parameter of the cost function that is
///   optimized. Default is 0.01.
/// - `Lr`: the learning rate of SGD. Default is 0.05.
/// - `NFactors`: the number of latent factors. Default is 16.
/// - `NEpochs`: the number of iterations of the SGD procedure. Default is 100.
/// - `InitMean`: the mean of initial random latent factors. Default is 0.
/// - `InitStdDev`: the standard deviation of initial random latent factors.
///   Default is 0.001.
#[derive(Default)]
pub struct BPR {
    base: BaseMatrixFactorization,
    n_factors: usize,
    n_epochs: usize,
    lr: f32,
    reg: f32,
    init_mean: f32,
    init_std_dev: f32,
}

impl BPR {
    /// Go's `NewBPR`: a BPR model with the given hyper-parameters.
    pub fn new(params: Params) -> Self {
        let mut bpr = BPR::default();
        bpr.set_params(params);
        bpr
    }

    /// Go's `BPR.Init`: draws the initial factors from the model's generator,
    /// then sets up the index.
    fn init(&mut self, train_set: &dyn CfSplit) {
        let rng = self.base.random_generator();
        let user_factor = rng.normal_matrix(
            train_set.count_users(),
            self.n_factors,
            self.init_mean,
            self.init_std_dev,
        );
        let item_factor = rng.normal_matrix(
            train_set.count_items(),
            self.n_factors,
            self.init_mean,
            self.init_std_dev,
        );
        self.base.init(train_set, user_factor, item_factor);
    }
}

impl CoreModel for BPR {
    fn set_params(&mut self, params: Params) {
        self.base.set_params(params);
        let params = self.base.params();
        self.n_factors = count_param(params.get_int(N_FACTORS, 16));
        self.n_epochs = count_param(params.get_int(N_EPOCHS, 100));
        self.lr = params.get_float32(LR, 0.05);
        self.reg = params.get_float32(REG, 0.01);
        self.init_mean = params.get_float32(INIT_MEAN, 0.0);
        self.init_std_dev = params.get_float32(INIT_STD_DEV, 0.001);
    }

    fn get_params(&self) -> &Params {
        self.base.params()
    }

    /// Go's `BPR.SuggestParams`.
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

    fn clear(&mut self) {
        self.base.clear();
    }

    fn invalid(&self) -> bool {
        self.base.invalid()
    }
}

impl Model for BPR {
    /// Go's `BPR.Fit`. Its task complexity is O(bpr.nEpochs).
    fn fit(
        &mut self,
        ctx: &Context,
        train_set: &dyn CfSplit,
        val_set: &mut dyn CfSplit,
        config: &FitConfig,
    ) -> Score {
        tracing::info!(
            train_set_size = train_set.count_feedback(),
            test_set_size = val_set.count_feedback(),
            params = ?self.base.params(),
            config = ?config,
            "fit bpr"
        );
        self.init(train_set);
        let n_users = train_set.count_users();
        let n_items = train_set.count_items();
        let feedback = train_set.get_user_feedback();
        // One generator per job, seeded in order from the model's generator.
        let rngs: Vec<Mutex<RandomGenerator>> = (0..config.jobs)
            .map(|_| Mutex::new(RandomGenerator::new(self.base.random_generator().int63())))
            .collect();
        // Convert array to hashmap
        let user_sets: Vec<HashSet<i32>> = feedback
            .iter()
            .map(|items| items.iter().copied().collect())
            .collect();
        let data = Interactions {
            user_feedback: feedback,
            user_sets: &user_sets,
            n_users,
            n_items,
        };
        let (reg, lr) = (self.reg, self.lr);

        let eval_start = Instant::now();
        let initial = evaluate_score(&*self, val_set, train_set, config);
        let mut history = History::new(initial);
        tracing::debug!(
            eval_time = ?eval_start.elapsed(),
            ndcg = initial.ndcg,
            precision = initial.precision,
            recall = initial.recall,
            "fit bpr 0/{}",
            self.n_epochs
        );
        // Training
        let (_, span) = monitor::start(ctx, "BPR.Fit", self.n_epochs as i64);
        let span = SpanGuard(span);
        for epoch in 1..=self.n_epochs {
            let fit_start = Instant::now();
            // Go's jobs update the factors without a lock. Here the epoch's jobs
            // share one lock, so each update runs whole, one at a time.
            let state = Mutex::new(self.base.take_fitted());
            let result = parallel::parallel::<ContextError, _>(
                ctx,
                train_set.count_feedback(),
                config.jobs,
                |worker_id, _| {
                    let mut fitted = lock(&state);
                    let mut rng = lock(&rngs[worker_id]);
                    bpr_update(&mut fitted, &mut rng, &data, reg, lr);
                    Ok(())
                },
            );
            self.base
                .restore_fitted(state.into_inner().unwrap_or_else(PoisonError::into_inner));
            let fit_time = fit_start.elapsed();
            if result.is_err() {
                tracing::info!(epoch, "fit bpr canceled");
                return Score::default();
            }
            // Cross validation
            if epoch % config.verbose == 0 || epoch == self.n_epochs {
                let eval_start = Instant::now();
                history.score = evaluate_score(&*self, val_set, train_set, config);
                history.scores.push((epoch, history.score.ndcg));
                tracing::info!(
                    fit_time = ?fit_time,
                    eval_time = ?eval_start.elapsed(),
                    ndcg = history.score.ndcg,
                    precision = history.score.precision,
                    recall = history.score.recall,
                    "fit bpr {}/{}",
                    epoch,
                    self.n_epochs
                );
                // early stopping if no improvement in last `patience` epochs
                if history.should_stop(epoch, config.patience) {
                    let (best_epoch, best_ndcg) = history.best();
                    tracing::info!(
                        best_epoch,
                        best_ndcg,
                        patience = config.patience,
                        "early stopping"
                    );
                    break;
                }
            }
            span.0.add(1);
        }
        let score = history.score;
        tracing::info!(
            ndcg = score.ndcg,
            precision = score.precision,
            recall = score.recall,
            "fit bpr complete"
        );
        score
    }

    fn get_item_index(&self) -> Option<&FreqDict> {
        self.base.get_item_index()
    }

    fn marshal(&self, w: &mut dyn Write) -> Result<(), Error> {
        self.base.marshal(w)
    }

    fn unmarshal(&mut self, r: &mut dyn Read) -> Result<(), Error> {
        self.base.unmarshal(r)?;
        let params = self.base.params().clone();
        self.set_params(params);
        Ok(())
    }

    fn get_user_factor(&self, user_index: i32) -> &[f32] {
        self.base.user_factor(user_index)
    }

    fn get_item_factor(&self, item_index: i32) -> &[f32] {
        self.base.item_factor(item_index)
    }

    fn model_name(&self) -> &'static str {
        "bpr"
    }
}

impl MatrixFactorization for BPR {
    fn predict(&self, user_id: &str, item_id: &str) -> f32 {
        self.base.predict(user_id, item_id)
    }

    fn internal_predict(&self, user_index: i32, item_index: i32) -> f32 {
        self.base.internal_predict(user_index, item_index)
    }

    fn get_user_index(&self) -> Option<&FreqDict> {
        self.base.get_user_index()
    }

    fn is_user_predictable(&self, user_index: i32) -> bool {
        self.base.is_user_predictable(user_index)
    }

    fn is_item_predictable(&self, item_index: i32) -> bool {
        self.base.is_item_predictable(item_index)
    }
}

/// Alternating least squares, the eALS of Go's `ALS` (weighted matrix
/// factorization with the whole-data update of Hu, Koren and Volinsky).
///
/// Hyper-parameters:
///
/// - `Reg`: the regularization parameter. Default is 0.06.
/// - `Alpha`: the weight of the unobserved entries. Default is 0.001.
/// - `NFactors`: the number of latent factors. Default is 16.
/// - `NEpochs`: the number of sweeps. Default is 50.
/// - `InitMean`: the mean of initial random latent factors. Default is 0.
/// - `InitStdDev`: the standard deviation of initial random latent factors.
///   Default is 0.1.
#[derive(Default)]
pub struct ALS {
    base: BaseMatrixFactorization,
    n_factors: usize,
    n_epochs: usize,
    init_mean: f32,
    init_std_dev: f32,
    reg: f32,
    weight: f32,
}

impl ALS {
    /// Go's `NewALS`: an ALS model with the given hyper-parameters.
    pub fn new(params: Params) -> Self {
        let mut als = ALS::default();
        als.set_params(params);
        als
    }

    /// Go's `ALS.Init`: draws the initial factors from the model's generator,
    /// then sets up the index.
    fn init(&mut self, train_set: &dyn CfSplit) {
        let rng = self.base.random_generator();
        let user_factor = rng.normal_matrix(
            train_set.count_users(),
            self.n_factors,
            self.init_mean,
            self.init_std_dev,
        );
        let item_factor = rng.normal_matrix(
            train_set.count_items(),
            self.n_factors,
            self.init_mean,
            self.init_std_dev,
        );
        self.base.init(train_set, user_factor, item_factor);
    }
}

impl CoreModel for ALS {
    fn set_params(&mut self, params: Params) {
        self.base.set_params(params);
        let params = self.base.params();
        self.n_factors = count_param(params.get_int(N_FACTORS, 16));
        self.n_epochs = count_param(params.get_int(N_EPOCHS, 50));
        self.init_mean = params.get_float32(INIT_MEAN, 0.0);
        self.init_std_dev = params.get_float32(INIT_STD_DEV, 0.1);
        self.reg = params.get_float32(REG, 0.06);
        self.weight = params.get_float32(ALPHA, 0.001);
    }

    fn get_params(&self) -> &Params {
        self.base.params()
    }

    /// Go's `ALS.SuggestParams`.
    fn suggest_params(&self, trial: &mut dyn SuggestTrial) -> Params {
        Params(BTreeMap::from([
            (N_FACTORS.to_owned(), ParamValue::Int(16)),
            (INIT_MEAN.to_owned(), ParamValue::Int(0)),
            (
                INIT_STD_DEV.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(INIT_STD_DEV, 0.001, 0.1)),
            ),
            (
                REG.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(REG, 0.001, 0.1)),
            ),
            (
                ALPHA.to_owned(),
                ParamValue::Float64(trial.suggest_log_float(ALPHA, 0.001, 0.1)),
            ),
        ]))
    }

    fn clear(&mut self) {
        self.base.clear();
    }

    fn invalid(&self) -> bool {
        self.base.invalid()
    }
}

/// The hyper-parameters one ALS solve reads.
#[derive(Clone, Copy)]
struct AlsHyper {
    n_factors: usize,
    reg: f32,
    weight: f32,
}

/// The scratch space of one ALS job, sized to the other side of the factorization.
struct Scratch {
    /// The current prediction for each other-side index with feedback.
    predictions: Vec<f32>,
    /// The residual of each prediction, for the factor being solved.
    res: Vec<f32>,
}

/// One side of an ALS sweep: the factors that are solved, and what they are solved against.
struct Phase<'a> {
    /// The factors to re-solve, one row per index.
    factors: &'a [Vec<f32>],
    /// The other side's factors, held fixed during the sweep.
    others: &'a [Vec<f32>],
    /// The feedback of each row: the other-side indices it interacted with.
    feedback: &'a [Vec<i32>],
    /// The gram matrix of the other side, `sum of x x^T` over the rows with feedback.
    s: &'a [Vec<f32>],
}

/// Go's solve step of an ALS job: re-solves one row, factor by factor, with the
/// other side and `s` fixed. The row is updated in place, so each factor uses
/// the values already solved for the earlier factors, as Go's in-place update does.
fn solve_row(
    row: &mut [f32],
    others: &[Vec<f32>],
    feedback: &[i32],
    s: &[Vec<f32>],
    scratch: &mut Scratch,
    hyper: AlsHyper,
) {
    let AlsHyper {
        n_factors,
        reg,
        weight,
    } = hyper;
    let Scratch { predictions, res } = scratch;
    for &i in feedback {
        predictions[i as usize] = floats::dot(row, &others[i as usize]);
    }
    for f in 0..n_factors {
        for &i in feedback {
            let i = i as usize;
            res[i] = predictions[i] - row[f] * others[i][f];
        }
        let (mut a, mut b, mut c) = (0f32, 0f32, 0f32);
        for &i in feedback {
            let i = i as usize;
            a += (1.0 - (1.0 - weight) * res[i]) * others[i][f];
            c += (1.0 - weight) * others[i][f] * others[i][f];
        }
        for k in (0..n_factors).filter(|&k| k != f) {
            b += weight * row[k] * s[k][f];
        }
        row[f] = (a - b) / (c + weight * s[f][f] + reg);
        for &i in feedback {
            let i = i as usize;
            predictions[i] = res[i] + row[f] * others[i][f];
        }
    }
}

/// The gram matrix Go builds for one ALS sweep: `sum over the rows with
/// feedback of x x^T`, where `x` is the row's factor. `None` when `ctx` is done.
fn gram(
    ctx: &Context,
    factors: &[Vec<f32>],
    feedback: &[Vec<i32>],
    n_factors: usize,
) -> Option<Vec<Vec<f32>>> {
    let mut s = util::new_matrix32(n_factors, n_factors);
    for (index, items) in feedback.iter().enumerate() {
        if ctx.err().is_some() {
            return None;
        }
        if !items.is_empty() {
            let factor = &factors[index];
            for (s_row, &x_i) in s.iter_mut().zip(factor) {
                for (cell, &x_j) in s_row.iter_mut().zip(factor) {
                    *cell += x_i * x_j;
                }
            }
        }
    }
    Some(s)
}

/// One ALS sweep over the rows of `phase`, in parallel, returning the new rows.
/// `None` when a job failed or `ctx` is done. Rows are independent within a
/// sweep, so the result does not depend on how the jobs are scheduled.
fn solve_phase(
    ctx: &Context,
    jobs: usize,
    phase: Phase<'_>,
    hyper: AlsHyper,
) -> Option<Vec<Vec<f32>>> {
    let scratch: Vec<Mutex<Scratch>> = (0..jobs)
        .map(|_| {
            Mutex::new(Scratch {
                predictions: vec![0.0; phase.others.len()],
                res: vec![0.0; phase.others.len()],
            })
        })
        .collect();
    let rows: Vec<Mutex<Vec<f32>>> = (0..phase.factors.len())
        .map(|_| Mutex::new(Vec::new()))
        .collect();
    let result = parallel::parallel::<ContextError, _>(
        ctx,
        phase.factors.len(),
        jobs,
        |worker_id, index| {
            let mut scratch = lock(&scratch[worker_id]);
            let mut row = phase.factors[index].clone();
            solve_row(
                &mut row,
                phase.others,
                &phase.feedback[index],
                phase.s,
                &mut scratch,
                hyper,
            );
            *lock(&rows[index]) = row;
            Ok(())
        },
    );
    if result.is_err() {
        return None;
    }
    Some(
        rows.into_iter()
            .map(|row| row.into_inner().unwrap_or_else(PoisonError::into_inner))
            .collect(),
    )
}

impl Model for ALS {
    /// Go's `ALS.Fit`. Its task complexity is O(als.nEpochs).
    fn fit(
        &mut self,
        ctx: &Context,
        train_set: &dyn CfSplit,
        val_set: &mut dyn CfSplit,
        config: &FitConfig,
    ) -> Score {
        tracing::info!(
            train_set_size = train_set.count_feedback(),
            test_set_size = val_set.count_feedback(),
            params = ?self.base.params(),
            config = ?config,
            "fit als"
        );
        self.init(train_set);
        let hyper = AlsHyper {
            n_factors: self.n_factors,
            reg: self.reg,
            weight: self.weight,
        };
        let user_feedback = train_set.get_user_feedback();
        let item_feedback = train_set.get_item_feedback();

        // evaluate initial model
        let eval_start = Instant::now();
        let initial = evaluate_score(&*self, val_set, train_set, config);
        let mut history = History::new(initial);
        tracing::debug!(
            eval_time = ?eval_start.elapsed(),
            ndcg = initial.ndcg,
            precision = initial.precision,
            recall = initial.recall,
            "fit als 0/{}",
            self.n_epochs
        );

        let (_, span) = monitor::start(ctx, "ALS.Fit", self.n_epochs as i64);
        let span = SpanGuard(span);
        for epoch in 1..=self.n_epochs {
            let fit_start = Instant::now();
            // Update user factors
            // S^q <- \sum^N_{itemIndex=1} c_i q_i q_i^T
            let Some(s) = gram(
                ctx,
                &self.base.fitted().item_factor,
                item_feedback,
                self.n_factors,
            ) else {
                tracing::info!(epoch, "fit als canceled");
                return Score::default();
            };
            let Some(rows) = solve_phase(
                ctx,
                config.jobs,
                Phase {
                    factors: &self.base.fitted().user_factor,
                    others: &self.base.fitted().item_factor,
                    feedback: user_feedback,
                    s: &s,
                },
                hyper,
            ) else {
                tracing::info!(epoch, "fit als canceled");
                return Score::default();
            };
            self.base
                .fitted
                .as_mut()
                .expect("model is not fitted")
                .user_factor = rows;

            // Update item factors
            // S^p <- P^T P
            let Some(s) = gram(
                ctx,
                &self.base.fitted().user_factor,
                user_feedback,
                self.n_factors,
            ) else {
                tracing::info!(epoch, "fit als canceled");
                return Score::default();
            };
            let Some(rows) = solve_phase(
                ctx,
                config.jobs,
                Phase {
                    factors: &self.base.fitted().item_factor,
                    others: &self.base.fitted().user_factor,
                    feedback: item_feedback,
                    s: &s,
                },
                hyper,
            ) else {
                tracing::info!(epoch, "fit als canceled");
                return Score::default();
            };
            self.base
                .fitted
                .as_mut()
                .expect("model is not fitted")
                .item_factor = rows;

            let fit_time = fit_start.elapsed();
            // Cross validation
            if epoch % config.verbose == 0 || epoch == self.n_epochs {
                let eval_start = Instant::now();
                history.score = evaluate_score(&*self, val_set, train_set, config);
                history.scores.push((epoch, history.score.ndcg));
                tracing::debug!(
                    fit_time = ?fit_time,
                    eval_time = ?eval_start.elapsed(),
                    ndcg = history.score.ndcg,
                    precision = history.score.precision,
                    recall = history.score.recall,
                    "fit als {}/{}",
                    epoch,
                    self.n_epochs
                );
                // early stopping if no improvement in last `patience` epochs
                if history.should_stop(epoch, config.patience) {
                    let (best_epoch, best_ndcg) = history.best();
                    tracing::info!(
                        best_epoch,
                        best_ndcg,
                        patience = config.patience,
                        "early stopping"
                    );
                    break;
                }
            }
            span.0.add(1);
        }
        let score = history.score;
        tracing::info!(
            ndcg = score.ndcg,
            precision = score.precision,
            recall = score.recall,
            "fit als complete"
        );
        score
    }

    fn get_item_index(&self) -> Option<&FreqDict> {
        self.base.get_item_index()
    }

    fn marshal(&self, w: &mut dyn Write) -> Result<(), Error> {
        self.base.marshal(w)
    }

    fn unmarshal(&mut self, r: &mut dyn Read) -> Result<(), Error> {
        self.base.unmarshal(r)?;
        let params = self.base.params().clone();
        self.set_params(params);
        Ok(())
    }

    fn get_user_factor(&self, user_index: i32) -> &[f32] {
        self.base.user_factor(user_index)
    }

    fn get_item_factor(&self, item_index: i32) -> &[f32] {
        self.base.item_factor(item_index)
    }

    fn model_name(&self) -> &'static str {
        "als"
    }
}

impl MatrixFactorization for ALS {
    fn predict(&self, user_id: &str, item_id: &str) -> f32 {
        self.base.predict(user_id, item_id)
    }

    fn internal_predict(&self, user_index: i32, item_index: i32) -> f32 {
        self.base.internal_predict(user_index, item_index)
    }

    fn get_user_index(&self) -> Option<&FreqDict> {
        self.base.get_user_index()
    }

    fn is_user_predictable(&self, user_index: i32) -> bool {
        self.base.is_user_predictable(user_index)
    }

    fn is_item_predictable(&self, item_index: i32) -> bool {
        self.base.is_item_predictable(item_index)
    }
}

#[cfg(test)]
mod tests {
    // Ports: model/cf/model_test.go @ add21b5. Both tests need the ml-1m dataset
    // from cdn.gorse.io, which the container cannot reach (O-1).

    use std::io::Cursor;

    use gorse_common::gocontext::Context;
    use gorse_dataset::dataset::load_data_from_built_in;
    use gorse_dataset::dict::FreqDict;
    use gorse_model_core::params::{
        ALPHA, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, ParamValue, Params, REG,
    };

    use super::*;
    use crate::cf::model::{FitConfig, Model};

    const BENCH_DELTA: f32 = 0.01;

    /// The Go test's `newFitConfig`, with one job per CPU.
    fn new_fit_config(_: usize) -> FitConfig {
        let jobs = std::thread::available_parallelism().map_or(1, usize::from);
        FitConfig::new().set_verbose(1).set_jobs(jobs)
    }

    /// Whether two dictionaries hold the same strings, in the same order, with the same counts.
    fn same_dict(a: &FreqDict, b: Option<&FreqDict>) -> bool {
        let Some(b) = b else { return false };
        a.count() == b.count()
            && (0..a.count()).all(|id| a.string(id) == b.string(id) && a.freq(id) == b.freq(id))
    }

    fn params(entries: &[(&str, ParamValue)]) -> Params {
        let mut params = Params::default();
        for (name, value) in entries {
            params.0.insert((*name).to_owned(), value.clone());
        }
        params
    }

    #[test]
    #[ignore = "O-1: dataset from cdn.gorse.io"]
    fn test_bpr_movielens() {
        let (train_set, mut test_set) = load_data_from_built_in("ml-1m").expect("ml-1m");
        let mut m = BPR::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.01)),
            (LR, ParamValue::Float64(0.05)),
            (N_EPOCHS, ParamValue::Int(30)),
            (INIT_MEAN, ParamValue::Int(0)),
            (INIT_STD_DEV, ParamValue::Float64(0.001)),
        ]));
        let fit_config = new_fit_config(30);
        let score = m.fit(
            &Context::background(),
            &train_set,
            &mut test_set,
            &fit_config,
        );
        assert!((0.36 - score.ndcg).abs() <= BENCH_DELTA);
        assert!(same_dict(train_set.get_user_dict(), m.get_user_index()));
        assert!(same_dict(test_set.get_item_dict(), m.get_item_index()));

        // test predict
        assert_eq!(m.predict("1", "1"), m.internal_predict(1, 1));
        assert_eq!(
            m.internal_predict(1, 1),
            floats::dot(m.get_user_factor(1), m.get_item_factor(1))
        );
        assert!(m.is_user_predictable(1));
        assert!(m.is_item_predictable(1));
        assert!(!m.is_user_predictable(i32::MAX));
        assert!(!m.is_item_predictable(i32::MAX));

        // test encode/decode model and increment training
        let mut buf = Vec::new();
        marshal_model(&mut buf, &m).expect("marshal");
        let tmp = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");
        assert_eq!(m.get_params(), tmp.get_params());
        assert_eq!(m.predict("1", "1"), tmp.predict("1", "1"));
        assert!(m.is_user_predictable(1));
        assert!(m.is_item_predictable(1));
        assert!(!m.is_user_predictable(i32::MAX));
        assert!(!m.is_item_predictable(i32::MAX));

        // test clear
        m.clear();
        assert!(m.invalid());
    }

    #[test]
    #[ignore = "O-1: dataset from cdn.gorse.io"]
    fn test_ccd_movielens() {
        let (train_set, mut test_set) = load_data_from_built_in("ml-1m").expect("ml-1m");
        let mut m = ALS::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.015)),
            (N_EPOCHS, ParamValue::Int(30)),
            (ALPHA, ParamValue::Float64(0.05)),
        ]));
        let fit_config = new_fit_config(30);
        let score = m.fit(
            &Context::background(),
            &train_set,
            &mut test_set,
            &fit_config,
        );
        assert!((0.36 - score.ndcg).abs() <= BENCH_DELTA);

        // test predict
        assert_eq!(m.predict("1", "1"), m.internal_predict(1, 1));
        assert_eq!(
            m.internal_predict(1, 1),
            floats::dot(m.get_user_factor(1), m.get_item_factor(1))
        );

        // test encode/decode model and increment training
        let mut buf = Vec::new();
        marshal_model(&mut buf, &m).expect("marshal");
        let tmp = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");
        assert_eq!(m.get_params(), tmp.get_params());
        assert_eq!(m.predict("1", "1"), tmp.predict("1", "1"));

        // test clear
        m.clear();
        assert!(m.invalid());
    }
}
