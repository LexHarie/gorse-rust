// Ports: model/cf/model_test.go and model/ctr/model_test.go @ add21b5 (the dataset assertions), and
// contracts/tools/modelreplay (the replay). The Go side of the dataset runs is conformance/oracle/metrics.

//! WP-3-8, metric parity (D-195), the Phase 3 exit gate.
//!
//! Gate 2, the replay. `contracts/tools/modelreplay` fits BPR, ALS and the
//! attention factorization machine on small synthetic datasets, with one job and
//! fixed seeds, and this test runs it live into a temporary directory. The Rust
//! fit of every epoch count must give the same metrics as Go within
//! [`REPLAY_TOLERANCE`]. The committed goldens are checked against live Go by
//! `cf_golden.rs` and `ctr_replay.rs`.
//!
//! Gate 1, the datasets. Each Go model test that loads a built-in dataset is
//! ported here with the Go test's own parameters. Its assertion is checked with
//! the Go test's own delta, never a wider one. The metrics the Go test does not
//! assert (precision, recall and AUC) are checked against a Go run of the same
//! configuration on the same file, within the same band. The Go run is
//! `conformance/oracle/metrics`, which this test runs itself.
//!
//! The datasets are read from `~/.gorse/dataset`, where the Go loader reads them.
//! Go must be on PATH, at the version in `go.mod`. A test that cannot run Go, or
//! that finds no dataset, fails with the reason. None is ignored.

use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

use gorse_common::gocontext::Context;
use gorse_common::gotime;
use gorse_dataset::dataset::{Dataset as CfDataset, load_data_from_built_in as load_cf_data};
use gorse_dataset::index::Index;
use gorse_dataset::unified_index::UnifiedMapIndexBuilder;
use gorse_model::cf::model::{ALS, BPR, FitConfig as CfFitConfig, Model, Score as CfScore};
use gorse_model::ctr::data::{Dataset as CtrDataset, load_data_from_built_in as load_ctr_data};
use gorse_model::ctr::evaluator::evaluate_classification;
use gorse_model::ctr::fm::Afm;
use gorse_model::ctr::model::{
    FactorizationMachines, FitConfig as CtrFitConfig, Score as CtrScore, marshal_model,
    unmarshal_model,
};
use gorse_model_core::params::{
    ALPHA, AUTO_SCALE, BATCH_SIZE, INIT_MEAN, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, OPTIMIZER,
    ParamValue, Params, REG,
};
use gorse_storage::data::types as data;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// The largest absolute difference from Go that the replay accepts (D-195, gate 2).
const REPLAY_TOLERANCE: f32 = 1e-5;

/// The band of the Go CF tests (`benchDelta` in `model/cf/model_test.go`). It also
/// bounds the metrics the Go tests do not assert, against a Go run (D-195).
const BENCH_DELTA: f32 = 0.01;

/// The band of the Go CTR tests (`classificationDelta` in `model/ctr/model_test.go`).
const CLASSIFICATION_DELTA: f32 = 0.01;

/// The Criteo band of the Go test (`InDelta(t, 0.77, score.Accuracy, 0.025)`).
const CRITEO_DELTA: f32 = 0.025;

/// The Criteo band of the Go test after the marshal round trip (`0.02`).
const CRITEO_CLONE_DELTA: f32 = 0.02;

/// The number of threads that the Go tests use (`runtime.NumCPU()`).
fn jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The repository root: the parent of this crate.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the conformance crate sits in the repository root")
}

/// Runs `go run` with `args` from the repository root, with `envs` set, and
/// panics with the reason when Go is missing or fails.
fn go_run(envs: &[(&str, &str)], args: &[&str]) {
    let mut command = Command::new("go");
    command.current_dir(repo_root()).arg("run").args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().unwrap_or_else(|e| {
        panic!("go is not on PATH ({e}): this test runs the Go oracle (conformance/README.md)")
    });
    assert!(
        output.status.success(),
        "go run {} failed ({}):\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs the Go dataset oracle for one configuration, and returns its metrics.
fn oracle<T: DeserializeOwned>(config: &str) -> T {
    let dir = tempfile::tempdir().expect("temporary directory");
    let out = dir.path().join("metrics.json");
    go_run(
        &[],
        &[
            "./conformance/oracle/metrics",
            "-config",
            config,
            "-out",
            out.to_str().expect("UTF-8 path"),
        ],
    );
    let text = fs::read_to_string(&out).expect("the oracle writes its metrics");
    serde_json::from_str(&text).expect("the oracle writes valid JSON")
}

/// The metrics of a Go CF run, as `conformance/oracle/metrics` writes them.
#[derive(Deserialize)]
struct GoCf {
    ndcg: f32,
    precision: f32,
    recall: f32,
}

/// The classification metrics of a Go run: the fit, or the same model after the
/// marshal round trip.
#[derive(Deserialize)]
struct GoClassification {
    accuracy: f32,
    precision: f32,
    recall: f32,
    auc: f32,
}

/// The output of a Go CTR run, as `conformance/oracle/metrics` writes it: the fit's
/// metrics at the top level, and the clone's under `clone`.
#[derive(Deserialize)]
struct GoCtr {
    #[serde(flatten)]
    fit: GoClassification,
    clone: GoClassification,
}

/// Asserts that `got` is within `delta` of `want`, as Go's `assert.InDelta` does,
/// and prints both values for the record.
fn in_delta(what: &str, got: f32, want: f32, delta: f32) {
    let diff = (got - want).abs();
    eprintln!("  {what}: Rust {got}, reference {want}, difference {diff:e} (band {delta})");
    assert!(
        diff <= delta,
        "{what}: Rust {got}, reference {want}, difference {diff:e} is outside the band {delta}"
    );
}

/// Asserts that `got` is at least `want`, as Go's `assert.GreaterOrEqual` does.
fn at_least(what: &str, got: f32, want: f32) {
    eprintln!("  {what}: Rust {got}, must be at least {want}");
    assert!(got >= want, "{what}: Rust {got} is below {want}");
}

/// Checks the precision, recall and AUC of a CTR score against a Go run, within
/// the classification band (D-195: Go does not assert these metrics). Prints the
/// Go accuracy for the record; accuracy is checked against the Go test's own
/// assertion by each test.
fn against_go(label: &str, rust: CtrScore, go: &GoClassification) {
    eprintln!("  {label} accuracy of the Go run: {}", go.accuracy);
    in_delta(
        &format!("{label} precision against the Go run"),
        rust.precision,
        go.precision,
        CLASSIFICATION_DELTA,
    );
    in_delta(
        &format!("{label} recall against the Go run"),
        rust.recall,
        go.recall,
        CLASSIFICATION_DELTA,
    );
    in_delta(
        &format!("{label} AUC against the Go run"),
        rust.auc,
        go.auc,
        CLASSIFICATION_DELTA,
    );
}

/// Builds a parameter set from `(name, value)` entries.
fn params(entries: &[(&str, ParamValue)]) -> Params {
    let mut params = Params::default();
    for (name, value) in entries {
        params.0.insert((*name).to_owned(), value.clone());
    }
    params
}

/// The BPR parameters of `TestBPR_MovieLens` (`model/cf/model_test.go`), with
/// `NEpochs` set to `epochs`.
fn bpr_movielens(epochs: i64) -> BPR {
    BPR::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (REG, ParamValue::Float64(0.01)),
        (LR, ParamValue::Float64(0.05)),
        (N_EPOCHS, ParamValue::Int(epochs)),
        (INIT_MEAN, ParamValue::Int(0)),
        (INIT_STD_DEV, ParamValue::Float64(0.001)),
    ]))
}

/// The ALS parameters of `TestCCD_MovieLens` (`model/cf/model_test.go`).
fn als_movielens() -> ALS {
    ALS::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (REG, ParamValue::Float64(0.015)),
        (N_EPOCHS, ParamValue::Int(30)),
        (ALPHA, ParamValue::Float64(0.05)),
    ]))
}

/// Fits a CF model on the train and test sets of a built-in dataset, with the
/// Go tests' fit configuration, and returns the score of the last evaluation.
fn fit_cf(dataset: &str, mut model: impl Model) -> CfScore {
    let (train, mut test) = load_cf_data(dataset).expect("the dataset loads");
    let config = CfFitConfig::new().set_verbose(1).set_jobs(jobs());
    model.fit(&Context::background(), &train, &mut test, &config)
}

/// The metrics of a CTR fit, and of the same model after a marshal round trip.
struct CtrRun {
    fit: CtrScore,
    clone: CtrScore,
}

/// Fits an attention factorization machine on a built-in dataset, with the Go
/// tests' fit configuration, then evaluates the model after a round trip.
fn fit_ctr(dataset: &str, mut fm: Afm) -> CtrRun {
    let (train, test) = load_ctr_data(dataset).expect("the dataset loads");
    let mut config = CtrFitConfig::new();
    config.set_verbose(1).set_jobs(jobs());
    let fit = fm.fit(&Context::background(), &train, &test, &config);
    let mut buf = Vec::new();
    marshal_model(&mut buf, &fm).expect("the model marshals");
    let tmp = unmarshal_model(&mut Cursor::new(buf)).expect("the model unmarshals");
    let clone = evaluate_classification(&*tmp, &test, config.jobs);
    CtrRun { fit, clone }
}

/// Go's `Afm` parameters for the Frappe test (`model/ctr/model_test.go`).
fn afm_frappe() -> Afm {
    Afm::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.01)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
    ]))
}

/// Go's `Afm` parameters for the MovieLens (ml-tag) test.
fn afm_ml_tag() -> Afm {
    Afm::new(params(&[
        (INIT_STD_DEV, ParamValue::Float64(0.01)),
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.001)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
    ]))
}

/// Go's `Afm` parameters for the Criteo test, which turns auto-scaling off.
fn afm_criteo() -> Afm {
    Afm::new(params(&[
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.01)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
        (AUTO_SCALE, ParamValue::Bool(false)),
    ]))
}

// ---------------------------------------------------------------------------
// Gate 1: the built-in datasets.
// ---------------------------------------------------------------------------

/// Go `TestBPR_MovieLens`: NDCG 0.36 within 0.01 of Go's assertion.
#[test]
fn ml_1m_bpr() {
    let go: GoCf = oracle("bpr-ml-1m");
    let score = fit_cf("ml-1m", bpr_movielens(30));
    eprintln!("ml-1m BPR");
    in_delta(
        "NDCG against the Go test's 0.36",
        score.ndcg,
        0.36,
        BENCH_DELTA,
    );
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// Go `TestCCD_MovieLens`: NDCG 0.36 within 0.01 of Go's assertion.
#[test]
fn ml_1m_als() {
    let go: GoCf = oracle("als-ml-1m");
    let score = fit_cf("ml-1m", als_movielens());
    eprintln!("ml-1m ALS");
    in_delta(
        "NDCG against the Go test's 0.36",
        score.ndcg,
        0.36,
        BENCH_DELTA,
    );
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// ml-100k: no Go model test uses it, so the reference is a Go run of the
/// ml-1m configuration on the same file (D-195).
#[test]
fn ml_100k_bpr() {
    let go: GoCf = oracle("bpr-ml-100k");
    let score = fit_cf("ml-100k", bpr_movielens(30));
    eprintln!("ml-100k BPR");
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// ml-100k: as `ml_100k_bpr`, for ALS.
#[test]
fn ml_100k_als() {
    let go: GoCf = oracle("als-ml-100k");
    let score = fit_cf("ml-100k", als_movielens());
    eprintln!("ml-100k ALS");
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// The commented-out `TestBPR_Pinterest` (`model/cf/model_test.go`): its NDCG
/// assertion 0.53 within 0.01. Go does not run it, so the value is the one the
/// commented code asserts, checked the same way.
#[test]
fn pinterest_20_bpr() {
    let go: GoCf = oracle("bpr-pinterest-20");
    let score = fit_cf(
        "pinterest-20",
        BPR::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.005)),
            (LR, ParamValue::Float64(0.05)),
            (N_EPOCHS, ParamValue::Int(50)),
            (INIT_MEAN, ParamValue::Int(0)),
            (INIT_STD_DEV, ParamValue::Float64(0.001)),
        ])),
    );
    eprintln!("pinterest-20 BPR");
    in_delta(
        "NDCG against the commented assertion 0.53",
        score.ndcg,
        0.53,
        BENCH_DELTA,
    );
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// The commented-out `TestCCD_Pinterest` (`model/cf/model_test.go`): its NDCG
/// assertion 0.52 within 0.01, checked as `pinterest_20_bpr`.
#[test]
fn pinterest_20_als() {
    let go: GoCf = oracle("als-pinterest-20");
    let score = fit_cf(
        "pinterest-20",
        ALS::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.01)),
            (N_EPOCHS, ParamValue::Int(20)),
            (INIT_STD_DEV, ParamValue::Float64(0.01)),
            (ALPHA, ParamValue::Float64(0.001)),
        ])),
    );
    eprintln!("pinterest-20 ALS");
    in_delta(
        "NDCG against the commented assertion 0.52",
        score.ndcg,
        0.52,
        BENCH_DELTA,
    );
    in_delta("NDCG against the Go run", score.ndcg, go.ndcg, BENCH_DELTA);
    in_delta(
        "precision against the Go run",
        score.precision,
        go.precision,
        BENCH_DELTA,
    );
    in_delta(
        "recall against the Go run",
        score.recall,
        go.recall,
        BENCH_DELTA,
    );
}

/// Go `TestFactorizationMachines_Classification_Frappe`: accuracy at least
/// 0.919 - 0.01, before and after the marshal round trip.
#[test]
fn frappe_afm() {
    let go: GoCtr = oracle("afm-frappe");
    let run = fit_ctr("frappe", afm_frappe());
    eprintln!("frappe AFM");
    at_least(
        "accuracy against the Go test",
        run.fit.accuracy,
        0.919 - CLASSIFICATION_DELTA,
    );
    at_least(
        "clone accuracy against the Go test",
        run.clone.accuracy,
        0.919 - CLASSIFICATION_DELTA,
    );
    against_go("fit", run.fit, &go.fit);
    against_go("clone", run.clone, &go.clone);
}

/// Go `TestFactorizationMachines_Classification_MovieLens`. Go skips it as
/// time-consuming (`t.Skip`). It runs here, so that no check is skipped.
#[test]
fn ml_tag_afm() {
    let go: GoCtr = oracle("afm-ml-tag");
    let run = fit_ctr("ml-tag", afm_ml_tag());
    eprintln!("ml-tag AFM");
    in_delta(
        "accuracy against the Go test's 0.815",
        run.fit.accuracy,
        0.815,
        CLASSIFICATION_DELTA,
    );
    against_go("fit", run.fit, &go.fit);
}

/// Go `TestFactorizationMachines_Classification_Criteo`: accuracy 0.77 within
/// 0.025, and 0.02 after the marshal round trip.
#[test]
fn criteo_afm() {
    let go: GoCtr = oracle("afm-criteo");
    let run = fit_ctr("criteo", afm_criteo());
    eprintln!("criteo AFM");
    in_delta(
        "accuracy against the Go test's 0.77",
        run.fit.accuracy,
        0.77,
        CRITEO_DELTA,
    );
    in_delta(
        "clone accuracy against the Go test's 0.77",
        run.clone.accuracy,
        0.77,
        CRITEO_CLONE_DELTA,
    );
    against_go("fit", run.fit, &go.fit);
    against_go("clone", run.clone, &go.clone);
}

// ---------------------------------------------------------------------------
// Gate 2: the replay of the synthetic goldens.
// ---------------------------------------------------------------------------

/// Runs `contracts/tools/modelreplay` in `mode` into `out`, as the golden checks do.
fn generate(mode: &str, out: &Path, envs: &[(&str, &str)]) {
    go_run(
        envs,
        &[
            "./contracts/tools/modelreplay",
            "-mode",
            mode,
            "-epochs",
            "5",
            "-out",
            out.to_str().expect("UTF-8 path"),
        ],
    );
}

/// `golden.json` for the CF replay, as `contracts/tools/modelreplay` writes it.
#[derive(Deserialize)]
struct CfGolden {
    users: usize,
    items: usize,
    bpr: CfModelGolden,
    als: CfModelGolden,
}

/// One model's part of the CF golden.
#[derive(Deserialize)]
struct CfModelGolden {
    epochs: Vec<CfEpochScore>,
    user_factor: Vec<Vec<f32>>,
    item_factor: Vec<Vec<f32>>,
}

/// The metrics after one epoch count.
#[derive(Deserialize)]
struct CfEpochScore {
    ndcg: f32,
    precision: f32,
    recall: f32,
}

/// Reads the tab-separated (user, item) index pairs of a split.
fn read_pairs(path: &Path) -> Vec<(i32, i32)> {
    let text = fs::read_to_string(path).expect("the generator writes the split");
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
fn build_cf(users: usize, items: usize, feedback: &[(i32, i32)]) -> CfDataset {
    let mut ds = CfDataset::new(gotime::Time::default(), 0, 0);
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

/// Fits BPR and ALS for every epoch count of the golden, each from scratch with
/// one job, and compares the metrics and the final factors with Go's. Returns the
/// largest difference.
fn replay_cf_model(
    name: &str,
    dir: &Path,
    golden: &CfGolden,
    model: &CfModelGolden,
    make: impl Fn(i64) -> Box<dyn Model>,
) -> f32 {
    let train_pairs = read_pairs(&dir.join("train.tsv"));
    let test_pairs = read_pairs(&dir.join("test.tsv"));
    let config = CfFitConfig::new().set_verbose(1).set_jobs(1);
    let mut largest = 0f32;
    let mut check = |what: String, got: f32, want: f32| {
        let diff = (got - want).abs();
        largest = largest.max(diff);
        assert!(
            diff <= REPLAY_TOLERANCE,
            "{name} {what}: Rust {got}, Go {want}, difference {diff:e}"
        );
    };
    for (index, expected) in model.epochs.iter().enumerate() {
        let epoch = index + 1;
        let train = build_cf(golden.users, golden.items, &train_pairs);
        let mut test = build_cf(golden.users, golden.items, &test_pairs);
        let mut fitted = make(epoch as i64);
        let score = fitted.fit(&Context::background(), &train, &mut test, &config);
        check(
            format!("NDCG after {epoch} epochs"),
            score.ndcg,
            expected.ndcg,
        );
        check(
            format!("precision after {epoch} epochs"),
            score.precision,
            expected.precision,
        );
        check(
            format!("recall after {epoch} epochs"),
            score.recall,
            expected.recall,
        );
        if epoch == model.epochs.len() {
            for (u, want) in model.user_factor.iter().enumerate() {
                for (k, (&got, &expected)) in fitted
                    .get_user_factor(u as i32)
                    .iter()
                    .zip(want)
                    .enumerate()
                {
                    check(format!("user factor {u}.{k}"), got, expected);
                }
            }
            for (i, want) in model.item_factor.iter().enumerate() {
                for (k, (&got, &expected)) in fitted
                    .get_item_factor(i as i32)
                    .iter()
                    .zip(want)
                    .enumerate()
                {
                    check(format!("item factor {i}.{k}"), got, expected);
                }
            }
        }
    }
    eprintln!("{name}: largest difference from Go {largest:e} (tolerance {REPLAY_TOLERANCE:e})");
    largest
}

/// BPR and ALS on the synthetic split of `contracts/tools/modelreplay`: the
/// metrics of every epoch count and the final factors, within 1e-5 of Go.
#[test]
fn replay_cf_bpr_and_als_match_go() {
    let dir = tempfile::tempdir().expect("temporary directory");
    generate("cf", dir.path(), &[]);
    let golden: CfGolden = serde_json::from_str(
        &fs::read_to_string(dir.path().join("golden.json"))
            .expect("the generator writes golden.json"),
    )
    .expect("golden.json parses");
    replay_cf_model("bpr", dir.path(), &golden, &golden.bpr, |epochs| {
        Box::new(BPR::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.01)),
            (LR, ParamValue::Float64(0.05)),
            (N_EPOCHS, ParamValue::Int(epochs)),
            (INIT_MEAN, ParamValue::Int(0)),
            (INIT_STD_DEV, ParamValue::Float64(0.001)),
        ])))
    });
    replay_cf_model("als", dir.path(), &golden, &golden.als, |epochs| {
        Box::new(ALS::new(params(&[
            (N_FACTORS, ParamValue::Int(8)),
            (REG, ParamValue::Float64(0.015)),
            (N_EPOCHS, ParamValue::Int(epochs)),
            (ALPHA, ParamValue::Float64(0.05)),
        ])))
    });
}

/// `afm_replay.json` for the CTR replay, as `contracts/tools/modelreplay` writes it.
#[derive(Deserialize)]
struct CtrGolden {
    params: CtrGoldenParams,
    init_seed: i64,
    names: CtrNames,
    user_labels: Vec<Vec<CtrFeature>>,
    item_labels: Vec<Vec<CtrFeature>>,
    embedding_dimension: Vec<usize>,
    item_embeddings: Vec<Vec<Vec<u16>>>,
    train: CtrSplit,
    test: CtrSplit,
    epochs: Vec<CtrEpochScore>,
}

/// The hyper-parameters of the golden fit.
#[derive(Deserialize)]
struct CtrGoldenParams {
    batch_size: i64,
    n_factors: i64,
    lr: f32,
    reg: f32,
    init_mean: f32,
    init_std_dev: f32,
    optimizer: String,
    auto_scale: bool,
}

impl CtrGoldenParams {
    /// The hyper-parameters of a fit of `epochs` epochs.
    fn params(&self, epochs: i64) -> Params {
        params(&[
            (BATCH_SIZE, ParamValue::Int(self.batch_size)),
            (N_FACTORS, ParamValue::Int(self.n_factors)),
            (N_EPOCHS, ParamValue::Int(epochs)),
            (LR, ParamValue::Float32(self.lr)),
            (REG, ParamValue::Float32(self.reg)),
            (INIT_MEAN, ParamValue::Float32(self.init_mean)),
            (INIT_STD_DEV, ParamValue::Float32(self.init_std_dev)),
            (OPTIMIZER, ParamValue::String(self.optimizer.clone())),
            (AUTO_SCALE, ParamValue::Bool(self.auto_scale)),
        ])
    }
}

/// The names of the unified index, in the order they were added.
#[derive(Deserialize)]
struct CtrNames {
    users: Vec<String>,
    items: Vec<String>,
    user_labels: Vec<String>,
    item_labels: Vec<String>,
    context_labels: Vec<String>,
    embeddings: Vec<String>,
}

/// A label id and its value.
#[derive(Deserialize)]
struct CtrFeature {
    id: i32,
    value: f32,
}

/// A train or test set, by row.
#[derive(Deserialize)]
struct CtrSplit {
    users: Vec<i32>,
    items: Vec<i32>,
    context_labels: Vec<Vec<CtrFeature>>,
    target: Vec<f32>,
}

/// The classification metrics after one epoch count.
#[derive(Deserialize)]
struct CtrEpochScore {
    epoch: i64,
    precision: f32,
    recall: f32,
    accuracy: f32,
    auc: f32,
}

/// The label tuples of rows of features.
fn label_pairs(rows: &[Vec<CtrFeature>]) -> Vec<Vec<(i32, f32)>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|feature| (feature.id, feature.value))
                .collect()
        })
        .collect()
}

/// A split of the golden dataset, built as the Go generator builds it.
fn build_ctr(golden: &CtrGolden, split: &CtrSplit) -> CtrDataset {
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
    CtrDataset {
        index: Some(builder.build()),
        user_labels: label_pairs(&golden.user_labels),
        item_labels: label_pairs(&golden.item_labels),
        context_labels: Some(label_pairs(&split.context_labels)),
        users: split.users.clone(),
        items: split.items.clone(),
        target: split.target.clone(),
        item_embeddings: golden.item_embeddings.clone(),
        item_embedding_dimension: golden.embedding_dimension.clone(),
        item_embedding_index: Some(embedding_index),
        positive_count,
        negative_count: split.target.len() - positive_count,
        ..CtrDataset::default()
    }
}

/// The attention factorization machine on the synthetic dataset of
/// `contracts/tools/modelreplay -mode ctr`: the metrics of every epoch count,
/// within 1e-5 of Go. As the Go generator does, each epoch count is fitted from
/// scratch with the initial weights of the golden's seed.
#[test]
fn replay_ctr_afm_matches_go() {
    let dir = tempfile::tempdir().expect("temporary directory");
    generate("ctr", dir.path(), &[("GODEBUG", "randautoseed=0")]);
    let golden: CtrGolden = serde_json::from_str(
        &fs::read_to_string(dir.path().join("afm_replay.json"))
            .expect("the generator writes afm_replay.json"),
    )
    .expect("afm_replay.json parses");
    let train = build_ctr(&golden, &golden.train);
    let test = build_ctr(&golden, &golden.test);
    let mut config = CtrFitConfig::new();
    config.set_verbose(1).set_jobs(1);
    let mut largest = 0f32;
    for expected in &golden.epochs {
        let mut fm = Afm::new(golden.params.params(expected.epoch));
        fm.set_init_seed(golden.init_seed);
        let score = fm.fit(&Context::background(), &train, &test, &config);
        for (name, got, want) in [
            ("precision", score.precision, expected.precision),
            ("recall", score.recall, expected.recall),
            ("accuracy", score.accuracy, expected.accuracy),
            ("auc", score.auc, expected.auc),
        ] {
            let diff = (got - want).abs();
            largest = largest.max(diff);
            assert!(
                diff <= REPLAY_TOLERANCE,
                "{name} after {} epochs: Rust {got}, Go {want}, difference {diff:e}",
                expected.epoch
            );
        }
    }
    eprintln!("ctr: largest difference from Go {largest:e} (tolerance {REPLAY_TOLERANCE:e})");
}
