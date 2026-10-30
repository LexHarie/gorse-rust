// Ports: cmd/gorse-bench/main.go (benchEmbeddingCmd, EvaluateEmbedding) @ add21b5

//! The `embedding` benchmark. It rates how well the embedding of each item
//! finds the items a user goes on to read. An item's embedding comes from
//! `--embedding-column`, an expression over the item, or from `--text-column`,
//! an expression that gives the text the OpenAI embeddings API embeds.
//!
//! For each user, the candidates are the user's test items and 99 sampled
//! negatives. Each candidate scores the negative sum of its Euclidean distances
//! to the user's training items, and the top `k` by score are ranked. The NDCG,
//! precision and recall of that ranking are averaged over the users.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use gorse_common::floats::euclidean;
use gorse_common::gocontext::Context;
use gorse_common::heap::TopKFilter;
use gorse_common::parallel;
use gorse_config::config::schema::Config;
use gorse_dataset::dataset::{CfSplit, Dataset, DatasetItem};
use gorse_expr::ty::{Kind, Ty};
use gorse_expr::value::Value;
use gorse_expr::vm::{self, Program};
use gorse_logics::item_to_item::{compile_column, item_type, item_value};
use gorse_model::cf::evaluator::{ndcg, precision, recall};
use serde::{Deserialize, Serialize};

use crate::bench::{self, Common, EmbeddingFlags};
use crate::bench_table;
use crate::cli;

/// The number of sampled negatives each user's candidates are padded with.
const NEGATIVE_SAMPLES: usize = 99;

/// Runs the embedding benchmark. The output is the dataset's sizes, then the
/// model's metrics at `k`.
pub fn run(common: &Common, flags: &EmbeddingFlags) {
    if flags.embedding_column.is_empty() && flags.text_column.is_empty() {
        bench::fatal("one of embedding-column or text-column is required");
    }
    let loaded = bench::load(&common.config);
    let mut cfg = loaded.config;
    if let Some(model) = &flags.model {
        cfg.openai.embedding_model = model.clone();
    }
    cfg.openai.embedding_dimensions = flags.dimensions;

    let dataset = loaded.cf;
    let (train, mut test) = dataset.split_latest(flags.shots);
    // Go calls this for its side effect, on the test split, before the
    // benchmark samples the negatives again against the training split.
    test.sample_user_negatives(&dataset, NEGATIVE_SAMPLES);

    let rows = vec![
        size_row("total", &dataset),
        size_row("train", &train),
        size_row("test", &test),
    ];
    print!(
        "{}",
        bench_table::render(&["", "#users", "#items", "#interactions"], &rows)
    );

    let top = flags.top as usize;
    let (model, metrics) = evaluate(
        &cfg,
        &train,
        &mut test,
        &flags.embedding_column,
        &flags.text_column,
        top,
        common.jobs,
    );
    let headers = [
        "Embedding Model".to_owned(),
        format!("NDCG@{}", flags.top),
        format!("Precision@{}", flags.top),
        format!("Recall@{}", flags.top),
    ];
    let header: Vec<&str> = headers.iter().map(String::as_str).collect();
    let rows = vec![vec![
        model,
        format!("{:.4}", metrics.ndcg),
        format!("{:.4}", metrics.precision),
        format!("{:.4}", metrics.recall),
    ]];
    print!("{}", bench_table::render(&header, &rows));
}

/// One row of the size table: the users, items and feedback of a split.
fn size_row(name: &str, dataset: &Dataset) -> Vec<String> {
    vec![
        name.to_owned(),
        dataset.count_users().to_string(),
        dataset.count_items().to_string(),
        dataset.count_feedback().to_string(),
    ]
}

/// The mean metrics over the users, as Go's `cf.Score`.
#[derive(Default)]
struct Metrics {
    ndcg: f32,
    precision: f32,
    recall: f32,
}

/// Where an item's embedding comes from: an expression over the item that gives
/// the embedding, or one that gives the text to embed.
enum Source {
    Embedding(Program),
    Text(Program),
}

/// Go's `EvaluateEmbedding`. It returns the model's name with the dimensions of
/// its embeddings, and the mean metrics over the users.
fn evaluate(
    cfg: &Config,
    train: &Dataset,
    test: &mut Dataset,
    embedding_column: &str,
    text_column: &str,
    top: usize,
    jobs: usize,
) -> (String, Metrics) {
    let source = if !embedding_column.is_empty() {
        Source::Embedding(compile_or_fatal(
            embedding_column,
            "failed to compile embedding expression",
        ))
    } else if !text_column.is_empty() {
        Source::Text(compile_or_fatal(
            text_column,
            "failed to compile text expression",
        ))
    } else {
        bench::fatal("one of embedding-column or text-column is required");
    };

    let items: &[DatasetItem] = test.get_items();
    let slots: Vec<Mutex<Option<Vec<f32>>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let dimensions = AtomicI64::new(0);
    let ctx = Context::background();
    match &source {
        Source::Text(program) => {
            let client = EmbeddingClient::new(&cfg.openai.base_url, &cfg.openai.auth_token);
            parallel::for_(&ctx, items.len(), jobs, |i| {
                let item = &items[i];
                let Some(text) = text_of(&run_on(program, item)) else {
                    return;
                };
                match client.create_embedding(
                    &text,
                    &cfg.openai.embedding_model,
                    cfg.openai.embedding_dimensions,
                ) {
                    Ok(embedding) => {
                        let len = embedding.len() as i64;
                        *slots[i].lock().unwrap_or_else(|err| err.into_inner()) = Some(embedding);
                        let _ = dimensions.compare_exchange(0, len, Ordering::SeqCst, Ordering::SeqCst);
                    }
                    Err(err) => {
                        tracing::error!(item_id = %item.item_id, error = %err, "failed to create embeddings");
                    }
                }
            })
            .expect("the background context is never done");
        }
        Source::Embedding(program) => {
            parallel::for_(&ctx, items.len(), jobs, |i| {
                let item = &items[i];
                let Some(embedding) = float32s_of(&run_on(program, item)) else {
                    return;
                };
                let previous = dimensions.swap(embedding.len() as i64, Ordering::SeqCst);
                if previous != 0 && previous != embedding.len() as i64 {
                    cli::fatal(
                        "inconsistent embedding dimensions",
                        format!("expected {previous}, got {}", embedding.len()),
                    );
                }
                *slots[i].lock().unwrap_or_else(|err| err.into_inner()) = Some(embedding);
            })
            .expect("the background context is never done");
        }
    }
    let embeddings: Vec<Option<Vec<f32>>> = slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or_else(|err| err.into_inner()))
        .collect();

    let negatives = test.sample_user_negatives(train, NEGATIVE_SAMPLES).to_vec();
    let results: Mutex<Vec<(usize, Metrics)>> = Mutex::new(Vec::new());
    parallel::for_(&ctx, test.count_users(), jobs, |user| {
        let target_items = &test.get_user_feedback()[user];
        if target_items.is_empty() {
            return;
        }
        let target_set: HashSet<i32> = target_items.iter().copied().collect();
        let mut candidates = target_items.clone();
        candidates.extend_from_slice(&negatives[user]);
        let feedback = &train.get_user_feedback()[user];
        if feedback.is_empty() {
            return;
        }

        let mut filter = TopKFilter::new(top);
        for &candidate in &candidates {
            let Some(candidate_embedding) = &embeddings[candidate as usize] else {
                continue;
            };
            let mut total_distance = 0f32;
            let mut valid_shots = 0;
            for &shot in feedback {
                if let Some(shot_embedding) = &embeddings[shot as usize] {
                    total_distance -= euclidean(candidate_embedding, shot_embedding);
                    valid_shots += 1;
                }
            }
            if valid_shots > 0 {
                filter.push(candidate, total_distance);
            }
        }
        if filter.is_empty() {
            return;
        }

        let rank_list = filter.pop_all_values();
        results.lock().unwrap_or_else(|err| err.into_inner()).push((
            user,
            Metrics {
                ndcg: ndcg(&target_set, &rank_list),
                precision: precision(&target_set, &rank_list),
                recall: recall(&target_set, &rank_list),
            },
        ));
    })
    .expect("the background context is never done");

    // Summed in user order, so the means do not depend on the order the workers
    // finished in.
    let mut results = results.into_inner().unwrap_or_else(|err| err.into_inner());
    results.sort_by_key(|(user, _)| *user);
    let mut sum = Metrics::default();
    let mut count = 0f32;
    for (_, metrics) in &results {
        sum.ndcg += metrics.ndcg;
        sum.precision += metrics.precision;
        sum.recall += metrics.recall;
        count += 1.0;
    }
    let metrics = if count > 0.0 {
        Metrics {
            ndcg: sum.ndcg / count,
            precision: sum.precision / count,
            recall: sum.recall / count,
        }
    } else {
        Metrics::default()
    };
    let name = format!(
        "{} ({})",
        cfg.openai.embedding_model,
        dimensions.load(Ordering::SeqCst)
    );
    (name, metrics)
}

/// Compiles an item expression, as Go's `expr.Compile` over `item: data.Item`.
/// A failure is fatal with the message `message`.
fn compile_or_fatal(column: &str, message: &str) -> Program {
    match compile_column(column, "item", &item_type()) {
        Ok(program) => program,
        Err(err) => cli::fatal(message, err),
    }
}

/// Runs an item expression on `item`. `None` when the expression fails, which Go
/// skips silently.
fn run_on(program: &Program, item: &DatasetItem) -> Option<Value> {
    let environment = Value::string_map(vec![(Value::str("item"), item_value(item))]);
    vm::run(program, &environment).ok()
}

/// The text of a string result, or `None` for any other type.
fn text_of(value: &Option<Value>) -> Option<String> {
    match value {
        Some(Value::Str(bytes)) => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

/// The floats of a `[]float32` result, or `None` for any other type. Go's
/// `result.([]float32)` matches the type exactly, so a list of float64 does not.
fn float32s_of(value: &Option<Value>) -> Option<Vec<f32>> {
    match value {
        Some(Value::Array(ty, items)) if *ty == Ty::slice(Ty::basic(Kind::Float32)) => items
            .iter()
            .map(|item| match item {
                Value::Float(number) => Some(*number as f32),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// The OpenAI embeddings client, as go-openai's `CreateEmbeddings` calls the
/// API: a `POST` to `{base_url}/embeddings` with the token as a bearer
/// credential. The request carries the input and the model, and the dimensions
/// only when they are set, as go-openai's `omitempty` does.
struct EmbeddingClient {
    http: reqwest::blocking::Client,
    base_url: String,
    token: String,
}

/// The body of an embeddings request.
#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a str,
    model: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    dimensions: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// The body of an embeddings response: one entry per input.
#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// One embedding of a response.
#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

impl EmbeddingClient {
    fn new(base_url: &str, token: &str) -> EmbeddingClient {
        EmbeddingClient {
            http: reqwest::blocking::Client::new(),
            base_url: base_url.to_owned(),
            token: token.to_owned(),
        }
    }

    /// The embedding of `input`. It is the first embedding of the response, as Go's
    /// `resp.Data[0]` reads it. A response without one is an error here, where Go
    /// panics.
    fn create_embedding(
        &self,
        input: &str,
        model: &str,
        dimensions: i64,
    ) -> Result<Vec<f32>, String> {
        let request = EmbeddingRequest {
            input,
            model,
            dimensions,
        };
        let response = self
            .http
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .map_err(|err| err.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the API answered {status}"));
        }
        let body: EmbeddingResponse = response.json().map_err(|err| err.to_string())?;
        body.data
            .into_iter()
            .next()
            .map(|data| data.embedding)
            .ok_or_else(|| "the response has no embedding".to_owned())
    }
}
