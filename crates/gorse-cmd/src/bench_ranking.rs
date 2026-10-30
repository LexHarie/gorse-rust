// Ports: cmd/gorse-bench/main.go (runRankingBenchmark, EvaluateAFM, EvaluateReranker, EvaluateDecision, evaluateRanker) @ add21b5

//! The `reranker` and `decision` benchmarks. Each rates the candidate items of
//! every user with a model and reports the grouped AUC (GAUC) of the ranking,
//! averaged over the users with the number of their candidates as the weight.
//! The AFM model is rated alongside the ranker, on the same split.
//!
//! The users are visited in sorted order, where Go visits them in map order,
//! so the sums are the same on every run. The GAUC table is sorted by ranker
//! name for the same reason; Go's `sync.Map` lists its rows in an order that
//! changes between runs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;

use gorse_common::gocontext::Context;
use gorse_common::gotime::Time;
use gorse_common::parallel;
use gorse_config::config::schema::Config;
use gorse_dataset::dataset::{CtrSplit, DatasetItem, Value as ProcessedLabels};
use gorse_logics::reranker::{Decision, FeedbackItem, RankError, Reranker};
use gorse_model::ctr::data::Dataset as CtrDataset;
use gorse_model::ctr::evaluator::auc;
use gorse_model::ctr::fm::Afm;
use gorse_model::ctr::model::{BatchInference, FactorizationMachines, FitConfig};
use gorse_model_core::params::Params;
use gorse_storage::cache::Score;
use gorse_storage::data::types::{Item, Labels, User};

use crate::bench;
use crate::bench_table;
use crate::cli;

/// Runs the ranking benchmark. `decision` selects the decision ranker over the
/// reranker, and `user_auc` writes the per-user AUC of each model to a CSV file
/// in the working directory.
pub fn run(config_path: &str, decision: bool, user_auc: bool) {
    let loaded = bench::load(config_path);
    let cfg = &loaded.config;
    let (train, test) = loaded.ctr.split_by_user_time(0.2);

    let rows = vec![
        count_row("total", &loaded.ctr),
        count_row("train", &train),
        count_row("test", &test),
    ];
    print!(
        "{}",
        bench_table::render(&["", "#interactions", "#positive", "#negative"], &rows)
    );

    let items = loaded.cf.get_items();
    let mut scores: Vec<(String, f64)> = std::thread::scope(|scope| {
        let afm = scope.spawn(|| ("AFM".to_owned(), evaluate_afm(cfg, &train, &test, user_auc)));
        let ranker = if decision {
            evaluate_decision(cfg, &train, &test, items, user_auc)
        } else {
            evaluate_reranker(cfg, &train, &test, items, user_auc)
        };
        let afm = afm.join().expect("the AFM benchmark did not panic");
        vec![afm, ranker]
    });
    scores.sort_by(|a, b| a.0.cmp(&b.0));

    let rows: Vec<Vec<String>> = scores
        .into_iter()
        .map(|(name, score)| vec![name, format!("{:.4}", score as f32)])
        .collect();
    print!("{}", bench_table::render(&["Ranker", "GAUC"], &rows));
}

/// One row of the interaction table: the users' feedback counts for a split.
fn count_row(name: &str, dataset: &CtrDataset) -> Vec<String> {
    vec![
        name.to_owned(),
        dataset.count().to_string(),
        dataset.positive_count.to_string(),
        dataset.negative_count.to_string(),
    ]
}

/// Go's `EvaluateAFM`: fits the AFM model on `train`, and rates each user's
/// test samples by AUC, weighted by how many samples the user has. The score is
/// the weighted mean.
fn evaluate_afm(cfg: &Config, train: &CtrDataset, test: &CtrDataset, user_auc: bool) -> f64 {
    let mut model = Afm::new(Params::default());
    if let Some(seed) = go_global_seed() {
        model.set_init_seed(seed);
    }
    let mut fit = FitConfig::new();
    fit.set_verbose(10)
        .set_jobs(bench::num_cpu())
        .set_patience(10);
    model.fit(&Context::background(), train, test, &fit);

    let mut feedback_count: HashMap<i32, usize> = HashMap::new();
    for i in 0..train.count() {
        let (indices, _, _, target) = train.get(i);
        if target > 0.0 {
            *feedback_count.entry(indices[0]).or_default() += 1;
        }
    }

    let mut features = Vec::with_capacity(test.count());
    let mut embeddings = Vec::with_capacity(test.count());
    let mut positives: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    let mut negatives: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for i in 0..test.count() {
        let (indices, values, embedding, target) = test.get(i);
        let user = indices[0];
        features.push((indices, values));
        embeddings.push(embedding);
        if target > 0.0 {
            positives.entry(user).or_default().push(i);
        } else {
            negatives.entry(user).or_default().push(i);
        }
    }
    let predictions = model.batch_internal_predict(&features, &embeddings, bench::num_cpu());

    let mut csv = if user_auc { open_csv("AFM.csv") } else { None };
    let mut sum = 0f32;
    let mut count = 0f32;
    for (user, pos_indices) in &positives {
        let neg_indices = negatives.get(user).map_or(&[][..], Vec::as_slice);
        let user_feedback = feedback_count.get(user).copied().unwrap_or(0);
        if pos_indices.is_empty()
            || neg_indices.is_empty()
            || user_feedback == 0
            || user_feedback as i64 > cfg.recommend.context_size
        {
            continue;
        }
        let mut pos_predictions: Vec<f32> = pos_indices.iter().map(|&i| predictions[i]).collect();
        let mut neg_predictions: Vec<f32> = neg_indices.iter().map(|&i| predictions[i]).collect();
        let user_auc = auc(&mut pos_predictions, &mut neg_predictions);
        let candidates = pos_indices.len() + neg_indices.len();
        if let Some(csv) = csv.as_mut() {
            writeln!(csv, "{user_feedback},{candidates},{user_auc:.4}")
                .expect("write the AFM CSV file");
        }
        let user_count = candidates as f32;
        sum += user_auc * user_count;
        count += user_count;
    }
    if count > 0.0 {
        (sum / count) as f64
    } else {
        0.0
    }
}

/// Go's `EvaluateReranker`: builds the reranker from the config, and scores it
/// under the name of its model.
fn evaluate_reranker(
    cfg: &Config,
    train: &CtrDataset,
    test: &CtrDataset,
    items: &[DatasetItem],
    user_auc: bool,
) -> (String, f64) {
    let api = &cfg.recommend.ranker.reranker_api;
    let ranker = match Reranker::new(api) {
        Ok(ranker) => ranker,
        Err(err) => cli::fatal("failed to create reranker", err),
    };
    let score = evaluate_ranker(
        cfg,
        train,
        test,
        items,
        user_auc,
        &api.model,
        |feedback, candidates| {
            ranker.rank(
                &Context::background(),
                Some(&User::default()),
                feedback,
                candidates,
            )
        },
    );
    (api.model.clone(), score)
}

/// Go's `EvaluateDecision`: builds the decision ranker from the config, and
/// scores it under the name of its model.
fn evaluate_decision(
    cfg: &Config,
    train: &CtrDataset,
    test: &CtrDataset,
    items: &[DatasetItem],
    user_auc: bool,
) -> (String, f64) {
    let api = &cfg.recommend.ranker.decision_api;
    let ranker = match Decision::new(api) {
        Ok(ranker) => ranker,
        Err(err) => cli::fatal("failed to create decision ranker", err),
    };
    let score = evaluate_ranker(
        cfg,
        train,
        test,
        items,
        user_auc,
        &api.model,
        |feedback, candidates| {
            ranker.rank(
                &Context::background(),
                Some(&User::default()),
                feedback,
                candidates,
            )
        },
    );
    (api.model.clone(), score)
}

/// Go's `evaluateRanker`. Each user's feedback in the training split is the
/// ranker's context. The user's test items are the candidates, and the ranked
/// list is scored by AUC on the positive and the negative items. `rank` is the
/// ranker, which calls its API over a blocking client, so the caller runs this
/// outside the async runtime.
fn evaluate_ranker<R>(
    cfg: &Config,
    train: &CtrDataset,
    test: &CtrDataset,
    items: &[DatasetItem],
    user_auc: bool,
    model: &str,
    rank: R,
) -> f64
where
    R: Fn(&[FeedbackItem], &[Item]) -> Result<Vec<Score>, RankError> + Sync,
{
    let raw: Vec<Item> = items.iter().map(to_item).collect();

    let mut feedbacks: HashMap<i32, Vec<FeedbackItem>> = HashMap::new();
    for i in 0..train.count() {
        let (indices, _, _, target) = train.get(i);
        if target <= 0.0 {
            continue;
        }
        let item = indices[1] - train.count_users() as i32;
        feedbacks.entry(indices[0]).or_default().push(FeedbackItem {
            item: raw[item as usize].clone(),
            ..Default::default()
        });
    }

    let mut positives: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    let mut negatives: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for i in 0..test.count() {
        let (indices, _, _, target) = test.get(i);
        let user = indices[0];
        let item = indices[1] - test.count_users() as i32;
        if target > 0.0 {
            positives.entry(user).or_default().push(item);
        } else {
            negatives.entry(user).or_default().push(item);
        }
    }

    let csv = Mutex::new(if user_auc {
        open_csv(&format!("{model}.csv"))
    } else {
        None
    });
    let results: Mutex<Vec<(i32, f32, f32)>> = Mutex::new(Vec::new());
    let users: Vec<i32> = positives.keys().copied().collect();
    let ctx = Context::background();
    parallel::for_each(&ctx, &users, bench::num_cpu(), |_, &user| {
        let pos_indices = &positives[&user];
        let neg_indices = negatives.get(&user).map_or(&[][..], Vec::as_slice);
        if pos_indices.is_empty() || neg_indices.is_empty() {
            return;
        }
        let mut candidates = Vec::with_capacity(pos_indices.len() + neg_indices.len());
        let mut negative_items = HashSet::new();
        let mut positive_items = HashSet::new();
        for &neg in neg_indices {
            let item = &raw[neg as usize];
            candidates.push(item.clone());
            negative_items.insert(item.item_id.clone());
        }
        for &pos in pos_indices {
            let item = &raw[pos as usize];
            candidates.push(item.clone());
            positive_items.insert(item.item_id.clone());
        }
        let feedback = feedbacks.get(&user).map_or(&[][..], Vec::as_slice);
        if feedback.is_empty() || feedback.len() as i64 > cfg.recommend.context_size {
            return;
        }
        let ranked = match rank(feedback, &candidates) {
            Ok(ranked) => ranked,
            Err(err) => {
                tracing::error!(user_index = user, error = %err, "failed to rank items for user");
                return;
            }
        };
        let mut pos_predictions = Vec::new();
        let mut neg_predictions = Vec::new();
        for item in &ranked {
            if positive_items.contains(&item.id) {
                pos_predictions.push(item.score as f32);
            } else if negative_items.contains(&item.id) {
                neg_predictions.push(item.score as f32);
            }
        }
        let user_auc = auc(&mut pos_predictions, &mut neg_predictions);
        let candidate_count = pos_indices.len() + neg_indices.len();
        if let Some(csv) = csv.lock().unwrap_or_else(|err| err.into_inner()).as_mut() {
            writeln!(csv, "{},{candidate_count},{user_auc:.4}", feedback.len())
                .expect("write the ranker CSV file");
        }
        let user_count = candidate_count as f32;
        results
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push((user, user_auc, user_count));
    })
    .expect("the background context is never done");

    // Summed in user order, so the score does not depend on the order the
    // workers finished in.
    let mut results = results.into_inner().unwrap_or_else(|err| err.into_inner());
    results.sort_by_key(|(user, _, _)| *user);
    let mut sum = 0f32;
    let mut count = 0f32;
    for (_, user_auc, user_count) in results {
        sum += user_auc * user_count;
        count += user_count;
    }
    if count > 0.0 {
        (sum / count) as f64
    } else {
        0.0
    }
}

/// Creates a per-user AUC CSV file with its header, as Go's `csv.Writer` on the
/// file does. A file that cannot be created is logged, and the export is off.
fn open_csv(path: &str) -> Option<BufWriter<File>> {
    match File::create(path) {
        Ok(file) => {
            let mut writer = BufWriter::new(file);
            writeln!(writer, "Feedback,Candidates,AUC").expect("write the CSV header");
            Some(writer)
        }
        Err(err) => {
            tracing::error!(error = %err, path, "failed to create ranker csv");
            None
        }
    }
}

/// The seed of Go's global random source in this process. Go's runtime seeds it
/// with 1 when `GODEBUG` has `randautoseed=0`, and otherwise at random, so the
/// AFM weights are random too.
fn go_global_seed() -> Option<i64> {
    std::env::var("GODEBUG")
        .unwrap_or_default()
        .split(',')
        .any(|setting| setting == "randautoseed=0")
        .then_some(1)
}

/// The storage item of a dataset item, as Go's `GetItems` gives it: its labels
/// are the processed ones. A processed label has no raw form, so its numbers
/// are written as the numbers the dataset holds.
fn to_item(item: &DatasetItem) -> Item {
    Item {
        item_id: item.item_id.clone(),
        is_hidden: item.is_hidden,
        categories: item.categories.clone(),
        timestamp: item.timestamp.clone(),
        update_at: Time::default(),
        labels: labels_of(&item.labels),
        comment: item.comment.clone(),
    }
}

/// The labels of a processed value, as the templates of a reranker see them.
fn labels_of(value: &ProcessedLabels) -> Labels {
    match value {
        ProcessedLabels::Plain(labels) => labels.clone(),
        ProcessedLabels::Object(entries) => Labels::Object(
            entries
                .iter()
                .map(|(name, value)| (name.clone(), labels_of(value)))
                .collect(),
        ),
        ProcessedLabels::Floats(bits) => Labels::Array(
            bits.iter()
                .map(|bits| Labels::Number(bits.to_string()))
                .collect(),
        ),
        ProcessedLabels::Ids(ids) => Labels::Array(
            ids.iter()
                .map(|id| Labels::Number(id.0.to_string()))
                .collect(),
        ),
        ProcessedLabels::Id(id) => Labels::Number(id.0.to_string()),
    }
}
