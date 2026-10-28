// Checks: model/cf/model.go @ add21b5, against the Go goldens in testdata/cf,
// which contracts/tools/modelreplay writes (D-195, gate 2).

//! The replay gate. It fits BPR and ALS on the synthetic splits the Go generator
//! wrote, once for each epoch count, and compares the score after each count and
//! the final factors with the Go output. The tolerance is 1e-5 absolute.

use std::fs;
use std::path::PathBuf;

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_dataset::dataset::Dataset;
use gorse_model_core::params::{
    ALPHA, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, ParamValue, Params, REG,
};
use gorse_storage::data::types as data;
use serde::Deserialize;

use super::model::{ALS, BPR, FitConfig, Model, Score};

/// The largest absolute difference the replay accepts (D-195, gate 2).
const TOLERANCE: f32 = 1e-5;

/// `golden.json`, as `contracts/tools/modelreplay` writes it.
#[derive(Deserialize)]
struct Golden {
    users: usize,
    items: usize,
    bpr: ModelGolden,
    als: ModelGolden,
}

/// One model's part of the golden output.
#[derive(Deserialize)]
struct ModelGolden {
    /// The score after each epoch count, starting from one epoch.
    epochs: Vec<EpochScore>,
    /// The user factors after the last epoch count.
    user_factor: Vec<Vec<f32>>,
    /// The item factors after the last epoch count.
    item_factor: Vec<Vec<f32>>,
}

/// The metrics of one fit, as the Go run returned them.
#[derive(Deserialize)]
struct EpochScore {
    ndcg: f32,
    precision: f32,
    recall: f32,
}

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("cf")
        .join(name)
}

/// Reads the tab-separated (user, item) index pairs of a split.
fn read_pairs(name: &str) -> Vec<(i32, i32)> {
    let text = fs::read_to_string(testdata(name)).expect("read split");
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (user, item) = line.split_once('\t').expect("tab-separated pair");
            (
                user.parse().expect("user index"),
                item.parse().expect("item index"),
            )
        })
        .collect()
}

/// A split built as the Go generator builds it: every user and item is added in
/// index order, then the feedback, so that every split shares one index mapping.
fn build(users: usize, items: usize, feedback: &[(i32, i32)]) -> Dataset {
    let mut ds = Dataset::new(gotime::Time::default(), 0, 0);
    for user in 0..users {
        ds.add_user(data::User {
            user_id: user.to_string(),
            ..Default::default()
        });
    }
    for item in 0..items {
        ds.add_item(data::Item {
            item_id: item.to_string(),
            ..Default::default()
        });
    }
    for &(user, item) in feedback {
        ds.add_feedback(
            &user.to_string(),
            &item.to_string(),
            gotime::Time::default(),
        );
    }
    ds
}

fn params(entries: &[(&str, ParamValue)]) -> Params {
    let mut params = Params::default();
    for (name, value) in entries {
        params.0.insert((*name).to_owned(), value.clone());
    }
    params
}

/// The generator's BPR hyper-parameters, with `NEpochs` set to `epochs`. The
/// value types match the Go literals: integers are `int`, decimals `float64`.
fn bpr_model(epochs: i64) -> Box<dyn Model> {
    Box::new(BPR::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (REG, ParamValue::Float64(0.01)),
        (LR, ParamValue::Float64(0.05)),
        (N_EPOCHS, ParamValue::Int(epochs)),
        (INIT_MEAN, ParamValue::Int(0)),
        (INIT_STD_DEV, ParamValue::Float64(0.001)),
    ])))
}

/// The generator's ALS hyper-parameters, with `NEpochs` set to `epochs`.
fn als_model(epochs: i64) -> Box<dyn Model> {
    Box::new(ALS::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (REG, ParamValue::Float64(0.015)),
        (N_EPOCHS, ParamValue::Int(epochs)),
        (ALPHA, ParamValue::Float64(0.05)),
    ])))
}

/// The largest absolute difference between a fitted score and the Go score.
fn score_diff(score: Score, expected: &EpochScore) -> f32 {
    (score.ndcg - expected.ndcg)
        .abs()
        .max((score.precision - expected.precision).abs())
        .max((score.recall - expected.recall).abs())
}

/// Replays one model against its golden output and returns the largest difference.
fn replay(
    name: &str,
    golden: &ModelGolden,
    users: usize,
    items: usize,
    make: fn(i64) -> Box<dyn Model>,
) -> f32 {
    let train_pairs = read_pairs("train.tsv");
    let test_pairs = read_pairs("test.tsv");
    let config = FitConfig::new().set_verbose(1).set_jobs(1);
    let mut max_diff = 0f32;
    let mut compared = 0usize;
    let epochs = golden.epochs.len();
    assert!(epochs > 0, "{name}: golden has no epochs");
    for epoch in 1..=epochs {
        let train = build(users, items, &train_pairs);
        let mut test = build(users, items, &test_pairs);
        let mut model = make(epoch as i64);
        let score = model.fit(&Context::background(), &train, &mut test, &config);
        let diff = score_diff(score, &golden.epochs[epoch - 1]);
        println!("{name} epoch {epoch}: score difference {diff:e}");
        max_diff = max_diff.max(diff);
        compared += 3;
        if epoch == epochs {
            assert_eq!(golden.user_factor.len(), users, "{name}: user rows");
            assert_eq!(golden.item_factor.len(), items, "{name}: item rows");
            for (user, expected) in golden.user_factor.iter().enumerate() {
                let actual = model.get_user_factor(user as i32);
                assert_eq!(actual.len(), expected.len(), "{name}: user {user} length");
                for (a, e) in actual.iter().zip(expected) {
                    max_diff = max_diff.max((a - e).abs());
                    compared += 1;
                }
            }
            for (item, expected) in golden.item_factor.iter().enumerate() {
                let actual = model.get_item_factor(item as i32);
                assert_eq!(actual.len(), expected.len(), "{name}: item {item} length");
                for (a, e) in actual.iter().zip(expected) {
                    max_diff = max_diff.max((a - e).abs());
                    compared += 1;
                }
            }
        }
    }
    println!(
        "{name}: largest difference {max_diff:e} over {epochs} epochs, {compared} values compared"
    );
    max_diff
}

#[test]
fn replay_matches_go_goldens() {
    let golden: Golden =
        serde_json::from_str(&fs::read_to_string(testdata("golden.json")).expect("read golden"))
            .expect("parse golden");
    let bpr = replay("bpr", &golden.bpr, golden.users, golden.items, bpr_model);
    let als = replay("als", &golden.als, golden.users, golden.items, als_model);
    let largest = bpr.max(als);
    println!("replay largest difference: {largest:e} (tolerance {TOLERANCE:e})");
    assert!(
        largest <= TOLERANCE,
        "replay difference {largest:e} exceeds {TOLERANCE:e}"
    );
}
