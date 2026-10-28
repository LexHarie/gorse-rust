// Replays: contracts/tools/modelreplay (ctr mode) against model/ctr/fm.go @ add21b5

//! Replays the golden fit of `contracts/tools/modelreplay -mode ctr`: the
//! factorization machine with attention, trained on the golden dataset, must
//! give the scores of every epoch count and the parameters after the last one,
//! within [`TOLERANCE`] of what Go gives (D-195).

use gorse_common::gocontext::Context;
use gorse_dataset::index::Index;
use gorse_dataset::unified_index::UnifiedMapIndexBuilder;
use gorse_model_core::params::{
    AUTO_SCALE, BATCH_SIZE, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, OPTIMIZER,
    ParamValue, Params, REG,
};
use serde::Deserialize;

use super::data::Dataset;
use super::fm::Afm;
use super::model::{FactorizationMachines, FitConfig};

/// The largest absolute difference from Go that the replay accepts (D-195).
const TOLERANCE: f32 = 1e-5;

/// The golden file that `modelreplay` writes, with the dataset it fits.
const GOLDEN: &str = include_str!("../../testdata/ctr/afm_replay.json");

/// The golden file, as `modelreplay` writes it.
#[derive(Deserialize)]
struct Golden {
    params: GoldenParams,
    init_seed: i64,
    names: Names,
    user_labels: Vec<Vec<Feature>>,
    item_labels: Vec<Vec<Feature>>,
    embedding_dimension: Vec<usize>,
    item_embeddings: Vec<Vec<Vec<u16>>>,
    train: Split,
    test: Split,
    epochs: Vec<EpochScore>,
    parameters: Vec<TensorData>,
}

/// The hyper-parameters of the golden fit.
#[derive(Deserialize)]
struct GoldenParams {
    batch_size: i64,
    n_factors: i64,
    n_epochs: i64,
    lr: f32,
    reg: f32,
    init_mean: f32,
    init_std_dev: f32,
    optimizer: String,
    auto_scale: bool,
}

impl GoldenParams {
    /// The hyper-parameters of a fit of `epochs` epochs (Go `model.Params`).
    fn params(&self, epochs: i64) -> Params {
        let entries = [
            (BATCH_SIZE, ParamValue::Int(self.batch_size)),
            (N_FACTORS, ParamValue::Int(self.n_factors)),
            (N_EPOCHS, ParamValue::Int(epochs)),
            (LR, ParamValue::Float32(self.lr)),
            (REG, ParamValue::Float32(self.reg)),
            (INIT_MEAN, ParamValue::Float32(self.init_mean)),
            (INIT_STD_DEV, ParamValue::Float32(self.init_std_dev)),
            (OPTIMIZER, ParamValue::String(self.optimizer.clone())),
            (AUTO_SCALE, ParamValue::Bool(self.auto_scale)),
        ];
        Params(
            entries
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
        )
    }
}

/// The names of the unified index, in the order they were added.
#[derive(Deserialize)]
struct Names {
    users: Vec<String>,
    items: Vec<String>,
    user_labels: Vec<String>,
    item_labels: Vec<String>,
    context_labels: Vec<String>,
    embeddings: Vec<String>,
}

/// A label id and its value.
#[derive(Deserialize)]
struct Feature {
    id: i32,
    value: f32,
}

/// A train or test set, by row.
#[derive(Deserialize)]
struct Split {
    users: Vec<i32>,
    items: Vec<i32>,
    context_labels: Vec<Vec<Feature>>,
    target: Vec<f32>,
}

/// The classification score after an epoch.
#[derive(Deserialize)]
struct EpochScore {
    epoch: i64,
    precision: f32,
    recall: f32,
    accuracy: f32,
    auc: f32,
}

/// A parameter: its shape and its values, row-major.
#[derive(Deserialize)]
struct TensorData {
    shape: Vec<usize>,
    data: Vec<f32>,
}

/// The dataset of a split, with the tables and the index of the golden file.
fn dataset(golden: &Golden, split: &Split) -> Dataset {
    let mut builder = UnifiedMapIndexBuilder::new();
    for name in &golden.names.users {
        builder.add_user(name);
    }
    for name in &golden.names.items {
        builder.add_item(name);
    }
    for name in &golden.names.user_labels {
        builder.add_user_label(name);
    }
    for name in &golden.names.item_labels {
        builder.add_item_label(name);
    }
    for name in &golden.names.context_labels {
        builder.add_ctx_label(name);
    }
    let mut embedding_index = Index::new();
    for name in &golden.names.embeddings {
        embedding_index.add(name);
    }
    let positive_count = split.target.iter().filter(|&&target| target > 0.0).count();
    Dataset {
        index: Some(builder.build()),
        user_labels: pairs(&golden.user_labels),
        item_labels: pairs(&golden.item_labels),
        context_labels: Some(pairs(&split.context_labels)),
        users: split.users.clone(),
        items: split.items.clone(),
        target: split.target.clone(),
        item_embeddings: golden.item_embeddings.clone(),
        item_embedding_dimension: golden.embedding_dimension.clone(),
        item_embedding_index: Some(embedding_index),
        positive_count,
        negative_count: split.target.len() - positive_count,
        ..Dataset::default()
    }
}

/// The label tuples of rows of features.
fn pairs(rows: &[Vec<Feature>]) -> Vec<Vec<(i32, f32)>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|feature| (feature.id, feature.value))
                .collect()
        })
        .collect()
}

/// Asserts that `got` is within [`TOLERANCE`] of Go's `want`, and records the
/// difference in `largest`.
fn compare(largest: &mut f32, what: &str, got: f32, want: f32) {
    let diff = (got - want).abs();
    *largest = largest.max(diff);
    assert!(
        diff <= TOLERANCE,
        "{what}: got {got}, Go has {want}, difference {diff:e}"
    );
}

#[test]
fn afm_replays_the_golden_fit() {
    let golden: Golden = serde_json::from_str(GOLDEN).expect("the golden file parses");
    let train = dataset(&golden, &golden.train);
    let test = dataset(&golden, &golden.test);
    let config = FitConfig {
        jobs: 1,
        verbose: 1,
        patience: 0,
    };
    assert_eq!(golden.epochs.len() as i64, golden.params.n_epochs);

    let mut largest = 0.0f32;
    // Go fits each epoch count in a process of its own, so each count is
    // fitted from scratch here too, with the initial weights of the same seed.
    for expected in &golden.epochs {
        let epoch = expected.epoch;
        let mut fm = Afm::new(golden.params.params(epoch));
        fm.set_init_seed(golden.init_seed);
        let score = fm.fit(&Context::background(), &train, &test, &config);
        for (name, got, want) in [
            ("precision", score.precision, expected.precision),
            ("recall", score.recall, expected.recall),
            ("accuracy", score.accuracy, expected.accuracy),
            ("auc", score.auc, expected.auc),
        ] {
            compare(
                &mut largest,
                &format!("{name} after {epoch} epochs"),
                got,
                want,
            );
        }

        if epoch == golden.params.n_epochs {
            let parameters = fm.parameters();
            assert_eq!(
                parameters.len(),
                golden.parameters.len(),
                "number of parameters"
            );
            for (i, (got, want)) in parameters.iter().zip(&golden.parameters).enumerate() {
                assert_eq!(got.shape(), want.shape, "shape of parameter {i}");
                let values = got.data();
                assert_eq!(values.len(), want.data.len(), "length of parameter {i}");
                for (j, (&value, &expected)) in values.iter().zip(&want.data).enumerate() {
                    compare(
                        &mut largest,
                        &format!("parameter {i} value {j}"),
                        value,
                        expected,
                    );
                }
            }
        }
    }
    eprintln!("largest difference from Go: {largest:e} (tolerance {TOLERANCE:e})");
}
