// Ports: dataset/dataset.go @ add21b5

//! The dataset: users, items, feedback and the labels of both, with the splits
//! that collaborative filtering and click-through models train and test on, and
//! the loaders of the built-in datasets.
//!
//! Go's `data.User` and `data.Item` hold their labels as `any`, and the labels
//! of a dataset are processed values (label IDs, FP16 vectors), so the dataset
//! has its own [`DatasetUser`] and [`DatasetItem`] with [`Value`] labels.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use gorse_common::floats;
use gorse_common::gosort;
use gorse_common::gostrconv;
use gorse_common::gotime::{self, Time};
use gorse_common::math32;
use gorse_common::util::{self, RandomGenerator};
use gorse_model_core::built_in::{self, DatasetFormat};
use gorse_storage::data::types as data;
use indexmap::IndexMap;

use crate::Error;
use crate::dict::FreqDict;
use crate::index::Index;
use crate::unified_index::UnifiedIndex;

/// Go's `ID`: the ID of a label string in a [`Labels`] dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub i32);

/// The processed form of a label value. `Labels::process_labels` turns a
/// `data` label into one of these.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A value kept as it was: `nil`, a bool, a number, or a list that is not
    /// all numbers or all strings.
    Plain(data::Labels),
    /// A map, with each value processed.
    Object(IndexMap<String, Value>),
    /// A list of numbers as FP16 bits, as `floats.FromAny` gives them.
    Floats(Vec<u16>),
    /// A list of strings, each a label ID.
    Ids(Vec<Id>),
    /// A string, a label ID.
    Id(Id),
}

/// A user of a dataset: Go's `data.User` with processed labels.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetUser {
    /// The user ID (`UserId`).
    pub user_id: String,
    /// The processed labels (`Labels`).
    pub labels: Value,
    /// The comment (`Comment`).
    pub comment: String,
}

/// An item of a dataset: Go's `data.Item` with processed labels.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetItem {
    /// The item ID (`ItemId`).
    pub item_id: String,
    /// Whether the item is hidden (`IsHidden`).
    pub is_hidden: bool,
    /// The categories (`Categories`). `None` is Go's nil slice.
    pub categories: Option<Vec<String>>,
    /// The time stamp (`Timestamp`).
    pub timestamp: Time,
    /// The processed labels (`Labels`).
    pub labels: Value,
    /// The comment (`Comment`).
    pub comment: String,
}

/// Go's `CFSplit`: a split for collaborative filtering. [`Dataset`] implements
/// it, and the splits [`Dataset::split_cf`] and [`Dataset::split_latest`]
/// return are datasets.
pub trait CfSplit {
    /// The number of users (`CountUsers`).
    fn count_users(&self) -> usize;
    /// The number of items (`CountItems`).
    fn count_items(&self) -> usize;
    /// The number of (positive) feedback (`CountFeedback`).
    fn count_feedback(&self) -> usize;
    /// The items (`GetItems`).
    fn get_items(&self) -> &[DatasetItem];
    /// The frequency dictionary of users (`GetUserDict`).
    fn get_user_dict(&self) -> &FreqDict;
    /// The frequency dictionary of items (`GetItemDict`).
    fn get_item_dict(&self) -> &FreqDict;
    /// The (positive) feedback of each user (`GetUserFeedback`).
    fn get_user_feedback(&self) -> &[Vec<i32>];
    /// The (positive) feedback of each item (`GetItemFeedback`).
    fn get_item_feedback(&self) -> &[Vec<i32>];
    /// Samples negative items for each user, not counting `exclude`'s feedback
    /// (`SampleUserNegatives`).
    fn sample_user_negatives(
        &mut self,
        exclude: &dyn CfSplit,
        num_candidates: usize,
    ) -> &[Vec<i32>];
}

/// Go's `CTRSplit`: a split for click-through rate prediction. The split types
/// are in the model packages.
pub trait CtrSplit {
    /// The number of samples (`Count`).
    fn count(&self) -> usize;
    /// The number of users (`CountUsers`).
    fn count_users(&self) -> usize;
    /// The number of items (`CountItems`).
    fn count_items(&self) -> usize;
    /// The number of user labels (`CountUserLabels`).
    fn count_user_labels(&self) -> usize;
    /// The number of item labels (`CountItemLabels`).
    fn count_item_labels(&self) -> usize;
    /// The number of context labels (`CountContextLabels`).
    fn count_context_labels(&self) -> usize;
    /// The number of positive samples (`CountPositive`).
    fn count_positive(&self) -> usize;
    /// The number of negative samples (`CountNegative`).
    fn count_negative(&self) -> usize;
    /// The unified index, or `None` when the split has none (`GetIndex`).
    fn get_index(&self) -> Option<&UnifiedIndex>;
    /// The target of sample `i` (`GetTarget`).
    fn get_target(&self, i: usize) -> f32;
    /// Sample `i`: the feature IDs, the values, the embedding features and the
    /// target (`Get`).
    fn get(&self, i: usize) -> (Vec<i32>, Vec<f32>, Vec<Vec<u16>>, f32);
    /// The dimension of each item embedding (`GetItemEmbeddingDim`).
    fn get_item_embedding_dim(&self) -> Vec<usize>;
    /// The names of the item embedding features (`GetItemEmbeddingIndex`), or
    /// `None` when there are none.
    fn get_item_embedding_index(&self) -> Option<&Index>;
}

/// Go's `Dataset`.
#[derive(Debug, Default, Clone)]
pub struct Dataset {
    timestamp: Time,
    users: Vec<DatasetUser>,
    items: Vec<DatasetItem>,
    user_labels: Labels,
    item_labels: Labels,
    user_feedback: Vec<Vec<i32>>,
    item_feedback: Vec<Vec<i32>>,
    timestamps: Vec<Vec<Time>>,
    negatives: Vec<Vec<i32>>,
    user_dict: FreqDict,
    item_dict: FreqDict,
    num_feedback: usize,
    categories: HashMap<String, usize>,
}

impl Dataset {
    /// `NewDataset`: an empty dataset with room for `user_count` users and
    /// `item_count` items.
    pub fn new(timestamp: Time, user_count: usize, item_count: usize) -> Self {
        Dataset {
            timestamp,
            users: Vec::with_capacity(user_count),
            items: Vec::with_capacity(item_count),
            user_labels: Labels::new(),
            item_labels: Labels::new(),
            user_feedback: vec![Vec::new(); user_count],
            item_feedback: vec![Vec::new(); item_count],
            timestamps: vec![Vec::new(); user_count],
            negatives: Vec::new(),
            user_dict: FreqDict::new(),
            item_dict: FreqDict::new(),
            num_feedback: 0,
            categories: HashMap::new(),
        }
    }

    /// The time stamp the dataset was created with (`GetTimestamp`).
    pub fn get_timestamp(&self) -> Time {
        self.timestamp.clone()
    }

    /// The number of (positive) feedback (`CountFeedback`).
    pub fn count_feedback(&self) -> usize {
        self.num_feedback
    }

    /// The users (`GetUsers`).
    pub fn get_users(&self) -> &[DatasetUser] {
        &self.users
    }

    /// The frequency dictionary of users (`GetUserDict`).
    pub fn get_user_dict(&self) -> &FreqDict {
        &self.user_dict
    }

    /// The number of users (`CountUsers`).
    pub fn count_users(&self) -> usize {
        self.users.len()
    }

    /// The items (`GetItems`).
    pub fn get_items(&self) -> &[DatasetItem] {
        &self.items
    }

    /// The frequency dictionary of items (`GetItemDict`).
    pub fn get_item_dict(&self) -> &FreqDict {
        &self.item_dict
    }

    /// The number of items (`CountItems`).
    pub fn count_items(&self) -> usize {
        self.items.len()
    }

    /// The (positive) feedback of each user (`GetUserFeedback`).
    pub fn get_user_feedback(&self) -> &[Vec<i32>] {
        &self.user_feedback
    }

    /// The (positive) feedback of each item (`GetItemFeedback`).
    pub fn get_item_feedback(&self) -> &[Vec<i32>] {
        &self.item_feedback
    }

    /// How often each category is on an item (`GetCategories`).
    pub fn get_categories(&self) -> &HashMap<String, usize> {
        &self.categories
    }

    /// The IDF of users: `IDF(u) = log(1 + I/freq(u))`, where `I` is the number of
    /// items and `freq(u)` the frequency of user `u` in all feedback
    /// (`GetUserIDF`).
    pub fn get_user_idf(&self) -> Vec<f32> {
        (0..self.user_dict.count())
            .map(|i| math32::log(1.0 + self.items.len() as f32 / self.user_dict.freq(i) as f32))
            .collect()
    }

    /// The IDF of items: `IDF(i) = log(1 + U/freq(i))`, where `U` is the number of
    /// users and `freq(i)` the frequency of item `i` in all feedback
    /// (`GetItemIDF`).
    pub fn get_item_idf(&self) -> Vec<f32> {
        (0..self.item_dict.count())
            .map(|i| math32::log(1.0 + self.users.len() as f32 / self.item_dict.freq(i) as f32))
            .collect()
    }

    /// The IDF of each user label value (`GetUserColumnValuesIDF`).
    pub fn get_user_column_values_idf(&self) -> Vec<f32> {
        let values = &self.user_labels.values;
        (0..values.count())
            .map(|i| math32::log(1.0 + self.users.len() as f32 / values.freq(i) as f32))
            .collect()
    }

    /// The IDF of each item label value (`GetItemColumnValuesIDF`).
    pub fn get_item_column_values_idf(&self) -> Vec<f32> {
        let values = &self.item_labels.values;
        (0..values.count())
            .map(|i| math32::log(1.0 + self.items.len() as f32 / values.freq(i) as f32))
            .collect()
    }

    /// Adds a user, with its labels processed (`AddUser`).
    pub fn add_user(&mut self, user: data::User) {
        let labels = self.user_labels.process_labels(&user.labels, "");
        self.users.push(DatasetUser {
            user_id: user.user_id.clone(),
            labels,
            comment: user.comment,
        });
        self.user_dict.add_no_count(&user.user_id);
        if self.user_feedback.len() < self.users.len() {
            self.user_feedback.push(Vec::new());
        }
        if self.timestamps.len() < self.users.len() {
            self.timestamps.push(Vec::new());
        }
    }

    /// Adds an item, with its labels processed, and counts its categories
    /// (`AddItem`).
    pub fn add_item(&mut self, item: data::Item) {
        let labels = self.item_labels.process_labels(&item.labels, "");
        self.items.push(DatasetItem {
            item_id: item.item_id.clone(),
            is_hidden: item.is_hidden,
            categories: item.categories.clone(),
            timestamp: item.timestamp,
            labels,
            comment: item.comment,
        });
        self.item_dict.add_no_count(&item.item_id);
        if self.item_feedback.len() < self.items.len() {
            self.item_feedback.push(Vec::new());
        }
        for category in item.categories.iter().flatten() {
            *self.categories.entry(category.clone()).or_insert(0) += 1;
        }
    }

    /// Adds a feedback from user `user_id` to item `item_id` at `timestamp`
    /// (`AddFeedback`). The user's feedback list is locked while it grows, as Go
    /// does. Panics if the item was not added first.
    pub fn add_feedback(&mut self, user_id: &str, item_id: &str, timestamp: Time) {
        let user_index = self.user_dict.add(user_id);
        let item_index = self.item_dict.add(item_id);
        self.item_feedback[item_index as usize].push(user_index);
        let _locked = self.user_dict.lock_guard(user_index);
        self.user_feedback[user_index as usize].push(item_index);
        self.timestamps[user_index as usize].push(timestamp);
        self.num_feedback += 1;
    }

    /// A dataset with the users, items and dictionaries of this one, and no
    /// feedback. The splits start from it.
    fn empty_split(&self) -> Dataset {
        Dataset {
            users: self.users.clone(),
            items: self.items.clone(),
            user_feedback: vec![Vec::new(); self.count_users()],
            item_feedback: vec![Vec::new(); self.count_items()],
            timestamps: vec![Vec::new(); self.count_users()],
            user_dict: self.user_dict.clone(),
            item_dict: self.item_dict.clone(),
            ..Dataset::default()
        }
    }

    /// Moves the feedback at position `k` of user `user_index` into `test_set`,
    /// and the other feedback of that user into `train_set`.
    fn split_user_feedback(
        &self,
        user_index: usize,
        k: usize,
        train_set: &mut Dataset,
        test_set: &mut Dataset,
    ) {
        let item_index = self.user_feedback[user_index][k];
        let user = user_index as i32;
        test_set.user_feedback[user_index].push(item_index);
        test_set.item_feedback[item_index as usize].push(user);
        test_set.timestamps[user_index].push(self.timestamps[user_index][k].clone());
        test_set.num_feedback += 1;
        for (i, &other) in self.user_feedback[user_index].iter().enumerate() {
            if i != k {
                train_set.user_feedback[user_index].push(other);
                train_set.item_feedback[other as usize].push(user);
                train_set.timestamps[user_index].push(self.timestamps[user_index][i].clone());
                train_set.num_feedback += 1;
            }
        }
    }

    /// Moves all the feedback of user `user_index` into `train_set`.
    fn move_user_to_train(&self, user_index: usize, train_set: &mut Dataset) {
        let user = user_index as i32;
        for (idx, &item_index) in self.user_feedback[user_index].iter().enumerate() {
            train_set.user_feedback[user_index].push(item_index);
            train_set.item_feedback[item_index as usize].push(user);
            train_set.timestamps[user_index].push(self.timestamps[user_index][idx].clone());
            train_set.num_feedback += 1;
        }
    }

    /// Splits the dataset by leave-one-out: one feedback of each test user goes
    /// to the test set, and the rest of the feedback to the training set.
    /// `num_test_users` is the number of test users. If it is at least the number
    /// of users, or zero or less, every user is a test user (`SplitCF`).
    pub fn split_cf(&self, num_test_users: i64, seed: i64) -> (Dataset, Dataset) {
        let mut train_set = self.empty_split();
        let mut test_set = self.empty_split();
        let mut rng = RandomGenerator::new(seed);
        if num_test_users >= self.count_users() as i64 || num_test_users <= 0 {
            for user_index in 0..self.count_users() {
                if !self.user_feedback[user_index].is_empty() {
                    let k = rng.intn(self.user_feedback[user_index].len() as i64) as usize;
                    self.split_user_feedback(user_index, k, &mut train_set, &mut test_set);
                }
            }
        } else {
            let test_users =
                rng.sample_int32(0, self.count_users() as i32, num_test_users as usize, &[]);
            for &user_index in &test_users {
                let user_index = user_index as usize;
                if !self.user_feedback[user_index].is_empty() {
                    let k = rng.intn(self.user_feedback[user_index].len() as i64) as usize;
                    self.split_user_feedback(user_index, k, &mut train_set, &mut test_set);
                }
            }
            let test_user_set: HashSet<i32> = test_users.iter().copied().collect();
            for user_index in 0..self.count_users() {
                if !test_user_set.contains(&(user_index as i32)) {
                    self.move_user_to_train(user_index, &mut train_set);
                }
            }
        }
        (train_set, test_set)
    }

    /// Splits the dataset by moving the most recent feedback of each user into the
    /// test set, and up to `shots` older feedback of each user into the training
    /// set (`SplitLatest`).
    pub fn split_latest(&self, shots: i64) -> (Dataset, Dataset) {
        let mut train_set = self.empty_split();
        let mut test_set = self.empty_split();
        for user_index in 0..self.count_users() {
            if self.user_feedback[user_index].is_empty() {
                continue;
            }
            let timestamps = &self.timestamps[user_index];
            let mut idxs: Vec<usize> = (0..self.user_feedback[user_index].len()).collect();
            gosort::sort(&mut LatestFirst {
                idxs: &mut idxs,
                timestamps,
            });
            let latest = idxs[0];
            let latest_item = self.user_feedback[user_index][latest];
            test_set.timestamps[user_index].push(timestamps[latest].clone());
            test_set.item_feedback[latest_item as usize].push(user_index as i32);
            test_set.user_feedback[user_index].push(latest_item);
            test_set.num_feedback += 1;
            for (i, &idx) in idxs.iter().enumerate().skip(1) {
                if i as i64 > shots {
                    break;
                }
                let item_index = self.user_feedback[user_index][idx];
                train_set.user_feedback[user_index].push(item_index);
                train_set.item_feedback[item_index as usize].push(user_index as i32);
                train_set.timestamps[user_index].push(timestamps[idx].clone());
                train_set.num_feedback += 1;
            }
        }
        (train_set, test_set)
    }
}

impl CfSplit for Dataset {
    fn count_users(&self) -> usize {
        Dataset::count_users(self)
    }

    fn count_items(&self) -> usize {
        Dataset::count_items(self)
    }

    fn count_feedback(&self) -> usize {
        Dataset::count_feedback(self)
    }

    fn get_items(&self) -> &[DatasetItem] {
        Dataset::get_items(self)
    }

    fn get_user_dict(&self) -> &FreqDict {
        Dataset::get_user_dict(self)
    }

    fn get_item_dict(&self) -> &FreqDict {
        Dataset::get_item_dict(self)
    }

    fn get_user_feedback(&self) -> &[Vec<i32>] {
        Dataset::get_user_feedback(self)
    }

    fn get_item_feedback(&self) -> &[Vec<i32>] {
        Dataset::get_item_feedback(self)
    }

    /// Samples `num_candidates` items per user from the items the user has no
    /// feedback for in either this split or `exclude`. The sample is cached, and a
    /// later call returns the cached samples whatever its arguments (as Go does).
    fn sample_user_negatives(
        &mut self,
        exclude: &dyn CfSplit,
        num_candidates: usize,
    ) -> &[Vec<i32>] {
        if self.negatives.is_empty() {
            let mut rng = RandomGenerator::new(0);
            let count_items = self.count_items() as i32;
            let mut negatives = Vec::with_capacity(self.count_users());
            for user_index in 0..self.count_users() {
                let s1: HashSet<i32> = self.user_feedback[user_index].iter().copied().collect();
                let s2: HashSet<i32> = exclude.get_user_feedback()[user_index]
                    .iter()
                    .copied()
                    .collect();
                negatives.push(rng.sample_int32(0, count_items, num_candidates, &[&s1, &s2]));
            }
            self.negatives = negatives;
        }
        &self.negatives
    }
}

/// `sort.Slice(idxs, func(i, j int) bool { return timestamps[idxs[i]].After(timestamps[idxs[j]]) })`,
/// latest first. Go's `sort.Slice` runs the same pattern-defeating quicksort as
/// `sort.Sort`, so [`gosort::sort`] over this adapter makes Go's swaps in Go's
/// order, and equal timestamps end where Go puts them.
struct LatestFirst<'a> {
    idxs: &'a mut [usize],
    timestamps: &'a [Time],
}

impl gosort::Interface for LatestFirst<'_> {
    fn len(&self) -> usize {
        self.idxs.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        self.timestamps[self.idxs[i]].after(&self.timestamps[self.idxs[j]])
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.idxs.swap(i, j);
    }
}

/// Go's `Labels`: the dictionary of label values, with the count of each value.
#[derive(Debug, Default, Clone)]
pub struct Labels {
    values: FreqDict,
}

impl Labels {
    /// `NewLabels`: an empty set of labels.
    pub fn new() -> Self {
        Self::default()
    }

    /// `processLabels`: turns a label into a [`Value`]. Each string becomes a label
    /// ID in the dictionary, named `parent:string`, where `parent` is the path of
    /// map keys leading to it (`.key.key`). A list of numbers becomes FP16 bits,
    /// and a list of strings becomes label IDs. Anything else is kept as it is.
    fn process_labels(&mut self, labels: &data::Labels, parent: &str) -> Value {
        match labels {
            data::Labels::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        let processed = self.process_labels(value, &format!("{parent}.{key}"));
                        (key.clone(), processed)
                    })
                    .collect(),
            ),
            data::Labels::Array(items) => {
                if let Some(values) = floats_from_any(items) {
                    Value::Floats(values)
                } else if let Some(strings) = strings_of(items) {
                    Value::Ids(
                        strings
                            .iter()
                            .map(|s| Id(self.values.add(&format!("{parent}:{s}"))))
                            .collect(),
                    )
                } else {
                    Value::Plain(labels.clone())
                }
            }
            data::Labels::String(s) => Value::Id(Id(self.values.add(&format!("{parent}:{s}")))),
            _ => Value::Plain(labels.clone()),
        }
    }
}

/// `floats.FromAny` on a list: the FP16 bits of the items when every item is a
/// number, or `None`. An empty list gives empty bits, as Go's does.
///
/// Go's `FromAny` takes any slice by reflection. Here the labels are the only
/// slices and they hold `float64` numbers, so only that case is ported, here and
/// not in `gorse_common::floats`, which Phase 1 owns.
fn floats_from_any(items: &[data::Labels]) -> Option<Vec<u16>> {
    let values = items
        .iter()
        .map(|item| match item {
            data::Labels::Float64(value) => Some(*value as f32),
            _ => None,
        })
        .collect::<Option<Vec<f32>>>()?;
    Some(floats::from_float32(&values))
}

/// The strings of a list, when every item is a string.
fn strings_of(items: &[data::Labels]) -> Option<Vec<&str>> {
    items
        .iter()
        .map(|item| match item {
            data::Labels::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect()
}

/// `LoadDataFromBuiltIn`: the training and test datasets of a built-in dataset in
/// the NCF format. The files are downloaded first when they are missing.
pub fn load_data_from_built_in(data_set_name: &str) -> Result<(Dataset, Dataset), Error> {
    let (train_file_path, test_file_path) =
        built_in::locate_built_in_dataset(data_set_name, DatasetFormat::Ncf)?;
    let mut train = load_train(&train_file_path)?;
    let mut test = Dataset::new(train.get_timestamp(), 0, 0);
    test.users = train.users.clone();
    test.items = train.items.clone();
    test.user_dict = train.user_dict.clone();
    test.item_dict = train.item_dict.clone();
    test.user_feedback = vec![Vec::new(); train.user_feedback.len()];
    test.item_feedback = vec![Vec::new(); train.item_feedback.len()];
    test.timestamps = vec![Vec::new(); train.user_feedback.len()];
    test.negatives = vec![Vec::new(); train.user_feedback.len()];
    load_test(&mut test, &test_file_path)?;
    // Go shares one frequency dictionary between the two sets, so the feedback and
    // negatives of the test set also count in the train set. Copy the final state
    // back, as Go does.
    train.user_dict = test.user_dict.clone();
    train.item_dict = test.item_dict.clone();
    Ok((train, test))
}

/// `loadTrain`: reads tab-separated `user item` lines. Every user and item up to
/// the largest integer ID seen is added first, with its integer as its ID.
fn load_train(path: &Path) -> Result<Dataset, Error> {
    let mut dataset = Dataset::new(gotime::now(), 0, 0);
    let file = File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<&str> = line.split('\t').collect();
        let (user, item) = match (fields.first(), fields.get(1)) {
            (Some(user), Some(item)) => (*user, *item),
            _ => return Err(Error::TooFewFields(line)),
        };
        let user_id = util::parse_int::<i32>(user)?;
        for i in dataset.user_dict.count()..=user_id {
            dataset.add_user(data::User {
                user_id: util::format_int(i),
                ..data::User::default()
            });
        }
        let item_id = util::parse_int::<i32>(item)?;
        for i in dataset.item_dict.count()..=item_id {
            dataset.add_item(data::Item {
                item_id: util::format_int(i),
                ..data::Item::default()
            });
        }
        dataset.add_feedback(user, item, Time::default());
    }
    Ok(dataset)
}

/// `loadTest`: reads lines of the form `(user,item)` followed by tab-separated
/// negative items. Each user's negatives are stored for [`CfSplit::sample_user_negatives`].
fn load_test(dataset: &mut Dataset, path: &Path) -> Result<(), Error> {
    let file = File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<&str> = line.split('\t').collect();
        let (positive, negatives) = (fields[0], &fields[1..]);
        let inner = match positive.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
            Some(inner) => inner,
            None => return Err(Error::WrongFormat(line)),
        };
        let pair: Vec<&str> = inner.split(',').collect();
        let (user, item) = match (pair.first(), pair.get(1)) {
            (Some(user), Some(item)) => (*user, *item),
            _ => return Err(Error::TooFewFields(line)),
        };
        dataset.add_feedback(user, item, Time::default());
        let (user_id, err) = gostrconv::atoi(user);
        if let Some(err) = err {
            return Err(err.into());
        }
        let slot = usize::try_from(user_id)
            .ok()
            .filter(|&slot| slot < dataset.negatives.len())
            .ok_or(Error::UserOutOfRange(user_id))?;
        let ids: Vec<i32> = negatives
            .iter()
            .map(|negative| dataset.item_dict.add(negative))
            .collect();
        dataset.negatives[slot] = ids;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
