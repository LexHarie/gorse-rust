// Ports: model/ctr/data.go @ add21b5

//! Datasets for click-through-rate models: the sparse features of each sample,
//! the libFM loader, the splits, and the conversion of labels and embeddings
//! from the values a request or a stored item carries.
//!
//! Go's `any` values are modelled by [`Value`], which holds the types the
//! conversions switch on.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use gorse_common::floats::from_float32;
use gorse_common::gosort;
use gorse_common::gotime::Time;
use gorse_common::util::parse_float;
use gorse_common::util::random::RandomGenerator;
use gorse_common::util::{NumError, parse_int};
use gorse_dataset::dataset::CtrSplit;
use gorse_dataset::index::Index;
use gorse_dataset::unified_index::{UnifiedDirectIndex, UnifiedIndex};
use gorse_model_core::built_in::{DatasetFormat, locate_built_in_dataset};

#[cfg(test)]
mod tests;

/// Errors of the libFM loader and the built-in dataset loader.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be opened or read (`errors.WithStack` of the Go
    /// error).
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A target or a feature value or index is not a number (the `strconv`
    /// error Go returns).
    #[error(transparent)]
    Parse(#[from] NumError),
    /// A feature has no `:` separator. Go indexes `kv[1]` and panics; the
    /// port returns this error instead.
    #[error("malformed feature {0:?}: missing ':'")]
    MalformedFeature(String),
    /// The built-in dataset could not be located.
    #[error(transparent)]
    BuiltIn(#[from] gorse_model_core::built_in::Error),
}

/// The features of the samples of a libFM file: each sample's feature ids and
/// values, as `lo.Tuple2[int32, float32]` pairs.
pub type Features = Vec<Vec<(i32, f32)>>;

/// A categorical or numerical feature, named (Go `Label`).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    /// The feature name, with the prefixes of the nesting it came from.
    pub name: String,
    /// The value: 1 for a categorical feature, the number for a numerical one.
    pub value: f32,
}

/// An embedding vector, named, stored as FP16 bits (Go `Embedding`).
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    /// The embedding feature name, with the prefixes of the nesting it came
    /// from.
    pub name: String,
    /// The FP16 bits of the vector.
    pub value: Vec<u16>,
}

/// A Go `any` holding a value of one of the types the label and embedding
/// conversions switch on. A `json.Number` decoded from a request is
/// [`Value::Number`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A nil interface.
    Nil,
    /// A `bool`.
    Bool(bool),
    /// A `string`.
    String(String),
    /// A `json.Number`, the text of a JSON number.
    Number(String),
    /// A `float32`.
    Float32(f32),
    /// A `float64`.
    Float64(f64),
    /// An `int`.
    Int(i64),
    /// A `[]any`.
    Slice(Vec<Value>),
    /// A `[]float32`.
    Float32s(Vec<f32>),
    /// A `[]float64`.
    Float64s(Vec<f64>),
    /// A `[]uint16`, FP16 bits.
    Uint16s(Vec<u16>),
    /// A `map[string]any`.
    Map(BTreeMap<String, Value>),
}

/// Go's `ConvertLabels`: the labels of a value, the categorical and numerical
/// features of a nested JSON value. A nil value gives no labels.
pub fn convert_labels(o: &Value) -> Vec<Label> {
    convert_labels_into(Vec::new(), "", o)
}

/// Go's `convertLabels`. A nil value and an empty list return nil in Go, which
/// drops the labels collected so far, so both return an empty list here. Go
/// visits map keys in random order; this port visits them in sorted order.
fn convert_labels_into(mut result: Vec<Label>, prefix: &str, o: &Value) -> Vec<Label> {
    match o {
        Value::Nil => Vec::new(),
        Value::Slice(labels) => {
            if labels.is_empty() {
                return Vec::new();
            }
            if let Value::String(_) = labels[0] {
                for val in labels {
                    match val {
                        Value::String(s) => result.push(Label {
                            name: format!("{prefix}{s}"),
                            value: 1.0,
                        }),
                        _ => panic!("unsupported labels: {labels:?}"),
                    }
                }
            }
            result
        }
        Value::Map(labels) => {
            for (key, val) in labels {
                result = convert_labels_into(result, &format!("{prefix}{key}."), val);
            }
            result
        }
        Value::String(s) => {
            result.push(Label {
                name: format!("{prefix}{s}"),
                value: 1.0,
            });
            result
        }
        Value::Number(n) => {
            // Go discards the error of `json.Number.Float64`, which leaves 0 for
            // text that does not parse.
            let value = n.parse::<f64>().unwrap_or(0.0);
            result.push(Label {
                name: prefix.to_string(),
                value: value as f32,
            });
            result
        }
        _ => result,
    }
}

/// Go's `ConvertEmbeddingLabels`: replaces each embedding vector in `o`, at
/// any depth, with its FP16 bits, and keeps the other labels. Existing FP16
/// slices are kept as they are.
pub fn convert_embedding_labels(o: Value) -> Value {
    match o {
        Value::Map(labels) => Value::Map(
            labels
                .into_iter()
                .map(|(key, value)| (key, convert_embedding_labels(value)))
                .collect(),
        ),
        Value::Slice(_) | Value::Float32s(_) | Value::Float64s(_) => {
            let mut embeddings = convert_embeddings(&o);
            if embeddings.len() == 1 {
                return Value::Uint16s(embeddings.remove(0).value);
            }
            o
        }
        _ => o,
    }
}

/// Go's `ConvertEmbeddings`: the embedding vectors of a value, named by the
/// keys that lead to them.
pub fn convert_embeddings(o: &Value) -> Vec<Embedding> {
    convert_embeddings_into(Vec::new(), "", o)
}

fn convert_embeddings_into(mut result: Vec<Embedding>, prefix: &str, o: &Value) -> Vec<Embedding> {
    match o {
        Value::Nil => Vec::new(),
        Value::Slice(embeddings) => {
            if embeddings.is_empty() {
                return Vec::new();
            }
            let mut value = Vec::new();
            for val in embeddings {
                match val {
                    Value::Float64(v) => value.push(*v as f32),
                    Value::Float32(v) => value.push(*v),
                    Value::Int(v) => value.push(*v as f32),
                    // Go's `case json.Number:` has no body, so the number is
                    // skipped.
                    Value::Number(_) => {}
                    _ => return result,
                }
            }
            result.push(Embedding {
                name: prefix.to_string(),
                value: from_float32(&value),
            });
            result
        }
        Value::Float64s(embeddings) => {
            let value: Vec<f32> = embeddings.iter().map(|f| *f as f32).collect();
            result.push(Embedding {
                name: prefix.to_string(),
                value: from_float32(&value),
            });
            result
        }
        Value::Float32s(embeddings) => {
            result.push(Embedding {
                name: prefix.to_string(),
                value: from_float32(embeddings),
            });
            result
        }
        Value::Uint16s(embeddings) => {
            result.push(Embedding {
                name: prefix.to_string(),
                value: embeddings.clone(),
            });
            result
        }
        Value::Map(embeddings) => {
            for (key, val) in embeddings {
                let name = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                result = convert_embeddings_into(result, &name, val);
            }
            result
        }
        _ => result,
    }
}

/// A sample set for click-through-rate models (Go `Dataset`). Samples are the
/// parallel vectors `users`, `items`, `context_labels`, `target` and
/// `timestamps`; a sample is a user, an item, their labels and a context.
#[derive(Debug, Default, Clone)]
pub struct Dataset {
    /// The unified index of the names the ids refer to. `None` only for a
    /// dataset that was never given one.
    pub index: Option<UnifiedIndex>,
    /// The labels of each user, by user id: the label id and its value.
    pub user_labels: Vec<Vec<(i32, f32)>>,
    /// The labels of each item, by item id: the label id and its value.
    pub item_labels: Vec<Vec<(i32, f32)>>,
    /// The context labels of each sample. `None` when the dataset has no
    /// context (Go's nil slice, which the code tests for).
    pub context_labels: Option<Vec<Vec<(i32, f32)>>>,
    /// The user id of each sample. Empty when the dataset has no users.
    pub users: Vec<i32>,
    /// The item id of each sample. Empty when the dataset has no items.
    pub items: Vec<i32>,
    /// The target of each sample.
    pub target: Vec<f32>,
    /// The time of each sample.
    pub timestamps: Vec<Time>,
    /// The FP16 embeddings of each item, by item id: the embedding id, then
    /// the dimensions.
    pub item_embeddings: Vec<Vec<Vec<u16>>>,
    /// The dimension of each embedding feature.
    pub item_embedding_dimension: Vec<usize>,
    /// The names of the embedding features.
    pub item_embedding_index: Option<Index>,
    /// The number of samples with a positive target.
    pub positive_count: usize,
    /// The number of samples with a target that is not positive.
    pub negative_count: usize,
}

impl Dataset {
    /// The unified index, panicking when there is none (Go calls a method on a
    /// nil interface and panics).
    fn unified_index(&self) -> &UnifiedIndex {
        self.index
            .as_ref()
            .expect("invalid memory address or nil pointer dereference: no unified index")
    }

    /// Go's `Count`: the number of samples. Panics when the parallel vectors
    /// disagree, as Go does.
    pub fn count(&self) -> usize {
        if self.users.len() != self.items.len() {
            panic!("len(dataset.Users) != len(dataset.Items)");
        }
        if !self.users.is_empty() && self.users.len() != self.target.len() {
            panic!("len(dataset.Users) != len(dataset.Target)");
        }
        if let Some(context_labels) = &self.context_labels
            && context_labels.len() != self.target.len()
        {
            panic!("len(dataset.ContextLabels) != len(dataset.Target)");
        }
        self.target.len()
    }

    /// Go's `Get`: the feature ids and values of sample `i`, the item
    /// embeddings and the target. The ids are offset by the sizes of the
    /// index before them: users, items, user labels, then item labels. The
    /// context labels are not offset, as in Go.
    pub fn get(&self, i: usize) -> (Vec<i32>, Vec<f32>, Vec<Vec<u16>>, f32) {
        let mut indices = Vec::new();
        let mut values = Vec::new();
        let mut embedding = Vec::new();
        let mut position: i32 = 0;
        // append user id
        if !self.users.is_empty() {
            indices.push(self.users[i]);
            values.push(1.0);
            position += self.unified_index().count_users();
        }
        // append item id
        if !self.items.is_empty() {
            indices.push(position + self.items[i]);
            values.push(1.0);
            position += self.unified_index().count_items();
            if !self.item_embeddings.is_empty() {
                embedding = self.item_embeddings[self.items[i] as usize].clone();
            }
        }
        // append user indices
        if !self.users.is_empty() {
            for &(index, value) in &self.user_labels[self.users[i] as usize] {
                indices.push(position + index);
                values.push(value);
            }
            position += self.unified_index().count_user_labels();
        }
        // append item indices
        if !self.items.is_empty() {
            for &(index, value) in &self.item_labels[self.items[i] as usize] {
                indices.push(position + index);
                values.push(value);
            }
        }
        // append context indices
        if let Some(context_labels) = &self.context_labels {
            for &(index, value) in &context_labels[i] {
                indices.push(index);
                values.push(value);
            }
        }
        (indices, values, embedding, self.target[i])
    }

    /// Go's `Split`: a random split. Each sample lands in the test set with
    /// probability `ratio`, drawn without replacement from `seed`.
    pub fn split(&self, ratio: f32, seed: i64) -> (Dataset, Dataset) {
        let mut train_set = self.empty_like();
        let mut test_set = self.empty_like();
        // split by random
        let num_test_size = (self.count() as f32 * ratio) as usize;
        let sampled: std::collections::HashSet<i64> = RandomGenerator::new(seed)
            .sample(0, self.count() as i64, num_test_size, &[])
            .into_iter()
            .collect();
        for i in 0..self.target.len() {
            if sampled.contains(&(i as i64)) {
                self.append_sample(&mut test_set, i);
            } else {
                self.append_sample(&mut train_set, i);
            }
        }
        (train_set, test_set)
    }

    /// Go's `SplitByUserTime`: within each user, the latest samples are the
    /// test set. Earlier samples are used for training. At least one sample is
    /// kept in train for every user with two or more samples.
    pub fn split_by_user_time(&self, ratio: f32) -> (Dataset, Dataset) {
        let mut train_set = self.empty_like();
        let mut test_set = self.empty_like();

        let mut user_samples: Vec<Vec<usize>> =
            vec![Vec::new(); self.unified_index().count_users() as usize];
        for (i, &user_index) in self.users.iter().enumerate() {
            user_samples[user_index as usize].push(i);
        }
        for samples in &mut user_samples {
            if samples.is_empty() {
                continue;
            }
            gosort::sort(&mut ByTimestamp {
                samples,
                timestamps: &self.timestamps,
            });
            let mut num_test = (samples.len() as f32 * ratio) as usize;
            if num_test == 0 && samples.len() > 1 {
                num_test = 1;
            }
            if num_test >= samples.len() {
                num_test = samples.len() - 1;
            }
            let split_index = samples.len() - num_test;
            for (i, &sample_index) in samples.iter().enumerate() {
                if i >= split_index {
                    self.append_sample(&mut test_set, sample_index);
                } else {
                    self.append_sample(&mut train_set, sample_index);
                }
            }
        }
        (train_set, test_set)
    }

    /// A dataset with the index, labels and embedding features of `self` and
    /// no samples, as the splits start from.
    fn empty_like(&self) -> Dataset {
        Dataset {
            index: self.index.clone(),
            user_labels: self.user_labels.clone(),
            item_labels: self.item_labels.clone(),
            item_embeddings: self.item_embeddings.clone(),
            item_embedding_index: self.item_embedding_index.clone(),
            item_embedding_dimension: self.item_embedding_dimension.clone(),
            ..Dataset::default()
        }
    }

    /// Go's `appendSample`: copies sample `i` of `self` to the end of `dst`,
    /// and counts it as positive or negative.
    fn append_sample(&self, dst: &mut Dataset, i: usize) {
        dst.users.push(self.users[i]);
        dst.items.push(self.items[i]);
        if let Some(context_labels) = &self.context_labels {
            dst.context_labels
                .get_or_insert_with(Vec::new)
                .push(context_labels[i].clone());
        }
        dst.target.push(self.target[i]);
        dst.timestamps.push(self.timestamps[i].clone());
        if self.target[i] > 0.0 {
            dst.positive_count += 1;
        } else {
            dst.negative_count += 1;
        }
    }
}

/// Sorts a user's sample indices by timestamp, as Go's `sort.Slice` does.
struct ByTimestamp<'a> {
    samples: &'a mut Vec<usize>,
    timestamps: &'a [Time],
}

impl gosort::Interface for ByTimestamp<'_> {
    fn len(&self) -> usize {
        self.samples.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.timestamps[self.samples[i]].before(&self.timestamps[self.samples[j]])
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.samples.swap(i, j);
    }
}

impl CtrSplit for Dataset {
    fn count(&self) -> usize {
        Dataset::count(self)
    }

    fn count_users(&self) -> usize {
        self.unified_index().count_users() as usize
    }

    fn count_items(&self) -> usize {
        self.unified_index().count_items() as usize
    }

    fn count_user_labels(&self) -> usize {
        self.unified_index().count_user_labels() as usize
    }

    fn count_item_labels(&self) -> usize {
        self.unified_index().count_item_labels() as usize
    }

    fn count_context_labels(&self) -> usize {
        self.unified_index().count_context_labels() as usize
    }

    fn count_positive(&self) -> usize {
        self.positive_count
    }

    fn count_negative(&self) -> usize {
        self.negative_count
    }

    fn get_index(&self) -> Option<&UnifiedIndex> {
        self.index.as_ref()
    }

    fn get_target(&self, i: usize) -> f32 {
        self.target[i]
    }

    fn get(&self, i: usize) -> (Vec<i32>, Vec<f32>, Vec<Vec<u16>>, f32) {
        Dataset::get(self, i)
    }

    fn get_item_embedding_dim(&self) -> Vec<usize> {
        self.item_embedding_dimension.clone()
    }

    fn get_item_embedding_index(&self) -> Option<&Index> {
        self.item_embedding_index.as_ref()
    }
}

/// Go's `LoadLibFMFile`: the features and targets of a libFM file, and the
/// largest feature id in it. Each line is a target followed by `id:value`
/// pairs.
pub fn load_lib_fm_file(path: &Path) -> Result<(Features, Vec<f32>, i32), Error> {
    let file = File::open(path)?;
    let mut features = Vec::new();
    let mut targets = Vec::new();
    let mut max_label: i32 = 0;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<&str> = line.split(' ').collect();
        // fetch target
        let target = parse_float::<f32>(fields[0])?;
        targets.push(target);
        // fetch features
        let mut line_features = Vec::with_capacity(fields.len() - 1);
        for field in &fields[1..] {
            if !field.trim().is_empty() {
                let (k, v) = field
                    .split_once(':')
                    .ok_or_else(|| Error::MalformedFeature((*field).to_string()))?;
                // append feature
                let feature = parse_int::<i64>(k)? as i32;
                let value = parse_float::<f32>(v)?;
                line_features.push((feature, value));
                max_label = max_label.max(feature);
            }
        }
        features.push(line_features);
    }
    Ok((features, targets, max_label))
}

/// Go's `LoadDataFromBuiltIn`: the train and test sets of a built-in libFM
/// dataset, which share one unified index.
pub fn load_data_from_built_in(name: &str) -> Result<(Dataset, Dataset), Error> {
    let (train_file_path, test_file_path) = locate_built_in_dataset(name, DatasetFormat::LibFm)?;
    let (train_context_labels, train_target, train_max_label) = load_lib_fm_file(&train_file_path)?;
    let (test_context_labels, test_target, test_max_label) = load_lib_fm_file(&test_file_path)?;
    let unified_index = UnifiedIndex::Direct(UnifiedDirectIndex::new(
        train_max_label.max(test_max_label) + 1,
    ));
    let train = Dataset {
        index: Some(unified_index.clone()),
        context_labels: Some(train_context_labels),
        target: train_target,
        ..Dataset::default()
    };
    let test = Dataset {
        index: Some(unified_index),
        context_labels: Some(test_context_labels),
        target: test_target,
        ..Dataset::default()
    };
    Ok((train, test))
}
