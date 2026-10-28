// Ports: logics/non_personalized.go @ add21b5

//! Non-personalised recommenders: each scores every item with an expression,
//! keeps the items that pass a filter expression, and lists the best scores
//! overall and per category.
//!
//! Go runs the expressions on `expr-lang`; here they run on the `gorse-expr`
//! VM. The variables `item` and `feedback` are typed as Go's `data.Item` and
//! `[]data.Feedback`, the types of the values Go's `expr.Env` gives them, so
//! the checker types the expressions as Go does.
//!
//! The recommender holds compiled programs, which use `Rc`s, so it is not
//! `Send`: it is used on one thread at a time. Go's `Push` is safe to call from
//! many goroutines; the port's `push` takes `&mut self` instead (D-WP36-3 in
//! `porting/notes/WP-3-6.md`).

use std::collections::BTreeMap;
use std::rc::Rc;

use gorse_common::gotime::Time;
use gorse_common::heap::TopKFilter;
use gorse_config::config::schema::NonPersonalizedConfig;
use gorse_expr::conf::Config;
use gorse_expr::file;
use gorse_expr::nature;
use gorse_expr::ty::{Field, Kind, Ty};
use gorse_expr::value::{self, Value};
use gorse_expr::vm::{self, Program};
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Feedback, FeedbackKey, Item, Labels};
use thiserror::Error;

/// The errors of [`NonPersonalized::new`].
#[derive(Debug, Error)]
pub enum Error {
    /// An expression does not compile.
    #[error(transparent)]
    Compile(#[from] file::Error),
    /// The score expression is not a number.
    #[error("score function must return float64")]
    ScoreType,
    /// The filter expression is not a bool.
    #[error("filter function must return bool")]
    FilterType,
}

/// The Go types of the variables an expression can use: `item` is a
/// `data.Item`, and `feedback` is a `[]data.Feedback`.
#[derive(Clone)]
struct EnvTypes {
    item: Ty,
    feedback: Ty,
    feedback_key: Ty,
}

impl EnvTypes {
    /// The types `data.Item`, `data.Feedback` and `data.FeedbackKey`, with
    /// their fields in declaration order.
    fn new() -> EnvTypes {
        let feedback_key = Ty::named_struct(
            "data.FeedbackKey",
            vec![
                field("FeedbackType", Ty::string()),
                field("UserId", Ty::string()),
                field("ItemId", Ty::string()),
            ],
            Vec::new(),
        );
        let item = Ty::named_struct(
            "data.Item",
            vec![
                field("ItemId", Ty::string()),
                field("IsHidden", Ty::bool()),
                field("Categories", Ty::slice(Ty::string())),
                field("Timestamp", value::time_type()),
                field("UpdateAt", value::time_type()),
                field("Labels", Ty::any()),
                field("Comment", Ty::string()),
            ],
            Vec::new(),
        );
        // `FeedbackKey` is embedded, so its fields are promoted, as in Go.
        let feedback = Ty::named_struct(
            "data.Feedback",
            vec![
                Field {
                    name: "FeedbackKey".to_owned(),
                    ty: feedback_key.clone(),
                    embedded: true,
                },
                field("Value", Ty::float64()),
                field("Timestamp", value::time_type()),
                field("Updated", value::time_type()),
                field("Labels", Ty::any()),
                field("Comment", Ty::string()),
            ],
            Vec::new(),
        );
        EnvTypes {
            item,
            feedback,
            feedback_key,
        }
    }

    /// The compile-time environment: the nature of `item` and `feedback`.
    fn config(&self) -> Config {
        let mut config = Config::create_new();
        config.with_env(nature::map_env_of([
            ("item".to_owned(), nature::from_type(&self.item)),
            (
                "feedback".to_owned(),
                nature::from_type(&Ty::slice(self.feedback.clone())),
            ),
        ]));
        config
    }
}

/// A field of a struct type that is not embedded.
fn field(name: &str, ty: Ty) -> Field {
    Field {
        name: name.to_owned(),
        ty,
        embedded: false,
    }
}

/// A non-personalised recommender: `logics`' `NonPersonalized`.
pub struct NonPersonalized {
    name: String,
    timestamp: Time,
    types: EnvTypes,
    score: Program,
    filter: Option<Program>,
    heap_size: usize,
    /// The best items of each group, by group name. The group `""` holds the
    /// best items overall, so every item is in it.
    heaps: BTreeMap<String, TopKFilter<String, f64>>,
}

impl NonPersonalized {
    /// Go's `NewNonPersonalized`: compiles the score and filter expressions,
    /// which must give a number and a bool, and keeps the `n` best items of
    /// each group.
    pub fn new(
        config: &NonPersonalizedConfig,
        n: usize,
        timestamp: Time,
    ) -> Result<NonPersonalized, Error> {
        let types = EnvTypes::new();
        let env = types.config();
        let score = gorse_expr::compile(&config.score, &env)?;
        if !matches!(
            score.root().kind(),
            Kind::Float64 | Kind::Int | Kind::Int8 | Kind::Int16 | Kind::Int32 | Kind::Int64
        ) {
            return Err(Error::ScoreType);
        }
        let filter = if config.filter.is_empty() {
            None
        } else {
            let filter = gorse_expr::compile(&config.filter, &env)?;
            if filter.root().kind() != Kind::Bool {
                return Err(Error::FilterType);
            }
            Some(filter)
        };
        let mut heaps = BTreeMap::new();
        heaps.insert(String::new(), TopKFilter::new(n));
        Ok(NonPersonalized {
            name: config.name.clone(),
            timestamp,
            types,
            score,
            filter,
            heap_size: n,
            heaps,
        })
    }

    /// Go's `Push`: scores `item` with its feedback and keeps it in the best
    /// items overall and in those of each of its categories. Hidden items, and
    /// items the filter rejects, are not kept. An expression that fails is
    /// logged and the item is skipped.
    pub fn push(&mut self, item: &Item, feedback: &[Feedback]) {
        if item.is_hidden {
            return;
        }
        let env = self.environment(item, feedback);
        if let Some(filter) = &self.filter {
            let keep = match vm::run(filter, &env) {
                Ok(Value::Bool(keep)) => keep,
                Ok(_) => unreachable!("the filter returns a bool, as checked when compiled"),
                Err(err) => {
                    tracing::error!(error = %err, "evaluate filter function");
                    return;
                }
            };
            if !keep {
                return;
            }
        }
        let result = match vm::run(&self.score, &env) {
            Ok(result) => result,
            Err(err) => {
                tracing::error!(error = %err, "evaluate score function");
                return;
            }
        };
        // Go's type switch: the score is a float or one of the integer types
        // `int` and `int64`. Other types, such as a `time.Duration`, are logged.
        let score = match result {
            Value::Float(score) => score,
            Value::Int(score) => score as f64,
            Value::Integer(ty, score) if ty.name() == "int64" => score as f64,
            result => {
                tracing::error!(result = ?result, "score function must return float64");
                return;
            }
        };
        let size = self.heap_size;
        let groups =
            std::iter::once("").chain(item.categories.iter().flatten().map(String::as_str));
        for group in groups {
            self.heaps
                .entry(group.to_owned())
                .or_insert_with(|| TopKFilter::new(size))
                .push(item.item_id.clone(), score);
        }
    }

    /// Go's `PopAll`: empties the heaps and returns the items of all of them,
    /// each once, with the groups it was kept under and the best score first.
    pub fn pop_all(&mut self) -> Vec<Score> {
        let timestamp = self.timestamp.clone();
        let mut scores: BTreeMap<String, Score> = BTreeMap::new();
        for (category, heap) in &mut self.heaps {
            for elem in heap.pop_all() {
                scores
                    .entry(elem.value.clone())
                    .and_modify(|score| score.categories.push(category.clone()))
                    .or_insert_with(|| Score {
                        id: elem.value,
                        score: elem.weight,
                        is_hidden: false,
                        categories: vec![category.clone()],
                        timestamp: timestamp.clone(),
                    });
            }
        }
        let mut result: Vec<Score> = scores.into_values().collect();
        cache::sort_documents(&mut result);
        result
    }

    /// The recommender's name, as configured.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The time the recommender was created at; its scores carry it.
    pub fn timestamp(&self) -> &Time {
        &self.timestamp
    }

    /// The variables an expression sees for `item` and `feedback`: Go's
    /// `map[string]any{"item": item, "feedback": feedback}`.
    fn environment(&self, item: &Item, feedback: &[Feedback]) -> Value {
        let item = Value::Struct(
            self.types.item.clone(),
            Rc::new(vec![
                Value::str(&item.item_id),
                Value::Bool(item.is_hidden),
                string_slice(item.categories.as_deref()),
                Value::Time(item.timestamp.clone()),
                Value::Time(item.update_at.clone()),
                labels_value(&item.labels),
                Value::str(&item.comment),
            ]),
        );
        let feedback = feedback
            .iter()
            .map(|feedback| self.feedback_value(feedback))
            .collect();
        Value::Map(
            Ty::map(Ty::string(), Ty::any()),
            Rc::new(vec![
                (Value::str("item"), item),
                (
                    Value::str("feedback"),
                    Value::Array(Ty::slice(self.types.feedback.clone()), Rc::new(feedback)),
                ),
            ]),
        )
    }

    /// A `data.Feedback` as an expression value.
    fn feedback_value(&self, feedback: &Feedback) -> Value {
        let FeedbackKey {
            feedback_type,
            user_id,
            item_id,
        } = &feedback.feedback_key;
        let key = Value::Struct(
            self.types.feedback_key.clone(),
            Rc::new(vec![
                Value::str(feedback_type),
                Value::str(user_id),
                Value::str(item_id),
            ]),
        );
        Value::Struct(
            self.types.feedback.clone(),
            Rc::new(vec![
                key,
                Value::Float(feedback.value),
                Value::Time(feedback.timestamp.clone()),
                Value::Time(feedback.updated.clone()),
                labels_value(&feedback.labels),
                Value::str(&feedback.comment),
            ]),
        )
    }
}

/// A `[]string` as an expression value. Go's nil slice is an empty one here,
/// which has the same length and ranges the same way.
fn string_slice(items: Option<&[String]>) -> Value {
    let items = items.unwrap_or(&[]);
    Value::Array(
        Ty::slice(Ty::string()),
        Rc::new(items.iter().map(|item| Value::str(item)).collect()),
    )
}

/// Labels as the `any` values Go's expression sees: a `json.Number` becomes a
/// float, since expr-lang has no type for it (D-WP36-4 in `porting/notes/WP-3-6.md`).
fn labels_value(labels: &Labels) -> Value {
    match labels {
        Labels::Nil => Value::Nil,
        Labels::Bool(value) => Value::Bool(*value),
        Labels::Float64(value) => Value::Float(*value),
        Labels::Number(text) => {
            Value::Float(text.parse().expect("a json.Number is a valid number"))
        }
        Labels::String(text) => Value::str(text),
        Labels::Array(items) => Value::any_array(items.iter().map(labels_value).collect()),
        Labels::Object(entries) => Value::Map(
            Ty::map(Ty::string(), Ty::any()),
            Rc::new(
                entries
                    .iter()
                    .map(|(name, value)| (Value::str(name), labels_value(value)))
                    .collect(),
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gorse_common::gotime::{self, Duration, HOUR, SECOND};

    /// `n` seconds, as Go's `time.Duration(n) * time.Second`.
    fn seconds(n: i64) -> Duration {
        Duration(n * SECOND.0)
    }

    #[test]
    fn latest() {
        let timestamp = gotime::now();
        let mut latest = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "latest".to_owned(),
                score: "item.Timestamp.Unix()".to_owned(),
                ..Default::default()
            },
            10,
            timestamp.clone(),
        )
        .expect("valid");
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(seconds(-i)),
                ..Default::default()
            };
            latest.push(&item, &[]);
        }
        let scores = latest.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            let i = i as i64;
            assert_eq!(score.id, i.to_string());
            assert_eq!(score.score, timestamp.add(seconds(-i)).unix() as f64);
            assert_eq!(score.timestamp, timestamp);
        }
    }

    #[test]
    fn popular() {
        let timestamp = gotime::now();
        let mut popular = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "popular".to_owned(),
                score: "len(feedback)".to_owned(),
                ..Default::default()
            },
            10,
            timestamp,
        )
        .expect("valid");
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                ..Default::default()
            };
            let feedback = vec![Feedback::default(); i];
            popular.push(&item, &feedback);
        }
        let scores = popular.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            assert_eq!(score.id, (99 - i).to_string());
            assert_eq!(score.score, (99 - i) as f64);
        }
    }

    #[test]
    fn popular_window() {
        // Create popular recommender
        let timestamp = gotime::now();
        let mut popular = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "popular".to_owned(),
                score: "len(feedback)".to_owned(),
                filter: format!(
                    "(now() - item.Timestamp).Nanoseconds() < {}",
                    HOUR.nanoseconds()
                ),
            },
            10,
            timestamp.clone(),
        )
        .expect("valid");
        // Add items
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(Duration(SECOND.0 - HOUR.0)),
                ..Default::default()
            };
            let feedback = vec![Feedback::default(); i];
            popular.push(&item, &feedback);
        }
        // Add outdated items
        for i in 100..110 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(Duration(-HOUR.0)),
                ..Default::default()
            };
            let feedback = vec![Feedback::default(); i];
            popular.push(&item, &feedback);
        }
        // Check result
        let scores = popular.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            assert_eq!(score.id, (99 - i).to_string());
            assert_eq!(score.score, (99 - i) as f64);
        }
    }

    #[test]
    fn filter() {
        let timestamp = gotime::now();
        let mut latest = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "latest".to_owned(),
                score: "item.Timestamp.Unix()".to_owned(),
                filter: "!item.IsHidden".to_owned(),
            },
            10,
            timestamp.clone(),
        )
        .expect("valid");
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(seconds(-i)),
                is_hidden: i < 10,
                ..Default::default()
            };
            latest.push(&item, &[]);
        }
        let scores = latest.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            let i = i as i64;
            assert_eq!(score.id, (i + 10).to_string());
            assert_eq!(score.score, timestamp.add(seconds(-i - 10)).unix() as f64);
            assert_eq!(score.timestamp, timestamp);
        }
    }

    #[test]
    fn hidden() {
        let timestamp = gotime::now();
        let mut latest = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "latest".to_owned(),
                score: "item.Timestamp.Unix()".to_owned(),
                ..Default::default()
            },
            10,
            timestamp.clone(),
        )
        .expect("valid");
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(seconds(-i)),
                is_hidden: i < 10,
                ..Default::default()
            };
            latest.push(&item, &[]);
        }
        let scores = latest.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            let i = i as i64;
            assert_eq!(score.id, (i + 10).to_string());
            assert_eq!(score.score, timestamp.add(seconds(-i - 10)).unix() as f64);
            assert_eq!(score.timestamp, timestamp);
        }
    }

    #[test]
    fn most_starred_weekly() {
        // Create non-personalized recommender
        let timestamp = gotime::now();
        let mut most_starred_weekly = NonPersonalized::new(
            &NonPersonalizedConfig {
                name: "most_starred_weekly".to_owned(),
                score: "count(feedback, .FeedbackType == 'star')".to_owned(),
                filter: "(now() - item.Timestamp).Hours() < 168".to_owned(),
            },
            10,
            timestamp.clone(),
        )
        .expect("valid");

        // Add items
        for i in 0..100 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(Duration(-167 * HOUR.0)),
                ..Default::default()
            };
            let mut feedback = Vec::new();
            for j in 0..i {
                feedback.push(feedback_of("star", j, i, timestamp.clone()));
                feedback.push(feedback_of("like", j, i, timestamp.clone()));
            }
            most_starred_weekly.push(&item, &feedback);
        }

        // Add outdated items
        for i in 100..110 {
            let item = Item {
                item_id: i.to_string(),
                timestamp: timestamp.add(Duration(-168 * HOUR.0)),
                ..Default::default()
            };
            let mut feedback = Vec::new();
            for j in 0..i {
                feedback.push(feedback_of(
                    "star",
                    j,
                    i,
                    timestamp.add(Duration(-169 * HOUR.0)),
                ));
            }
            most_starred_weekly.push(&item, &feedback);
        }

        // Check result
        let scores = most_starred_weekly.pop_all();
        assert_eq!(scores.len(), 10);
        for (i, score) in scores.iter().enumerate() {
            assert_eq!(score.id, (99 - i).to_string());
            assert_eq!(score.score, (99 - i) as f64);
        }
    }

    /// A feedback of `feedback_type` from user `user` on item `item`.
    fn feedback_of(feedback_type: &str, user: i64, item: i64, timestamp: Time) -> Feedback {
        Feedback {
            feedback_key: FeedbackKey {
                feedback_type: feedback_type.to_owned(),
                user_id: user.to_string(),
                item_id: item.to_string(),
            },
            timestamp,
            ..Default::default()
        }
    }
}
