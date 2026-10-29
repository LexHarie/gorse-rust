// Ports: logics/reranker.go @ add21b5

//! Rerankers: they render a query and one document per item from two templates,
//! then have a rerank API or a decisions API score the documents.
//!
//! Go renders the templates with `gonja`; here they are `minijinja` (D-196).
//! Both read Jinja syntax, and the values a template sees carry the Go field
//! names, as gonja reads a struct. The environment's formatter prints each value
//! as gonja does (`go_text`), so `{{ }}` gives the same text in both.

use std::collections::BTreeMap;

use gorse_common::decision::{self, NoulQuestion, Questions};
use gorse_common::gocontext;
use gorse_common::gostrconv;
use gorse_common::reranker;
use gorse_config::config::schema::{DecisionAPIConfig, RerankerAPIConfig};
use gorse_storage::cache::{self, Score};
use gorse_storage::data::types::{Item, Labels, User};
use minijinja::value::ValueKind;
use minijinja::{Environment, Output, State, Value};
use serde_json::Value as Json;
use thiserror::Error;

/// A feedback of a user and the item it is about: `logics`' `FeedbackItem`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedbackItem {
    /// The feedback type (`FeedbackType`).
    pub feedback_type: String,
    /// The item the feedback is about (`data.Item`, embedded in Go).
    pub item: Item,
}

/// The errors of [`Reranker::rank`] and [`Decision::rank`].
#[derive(Debug, Error)]
pub enum RankError {
    /// A template does not parse, or does not render.
    #[error(transparent)]
    Template(#[from] minijinja::Error),
    /// The rerank API failed.
    #[error(transparent)]
    Rerank(#[from] reranker::Error),
    /// The decisions API failed.
    #[error(transparent)]
    Decision(#[from] decision::Error),
    /// The rerank API ranked a document that is not in the request. Go would
    /// index past the end of the items and panic; this port returns an error.
    #[error("rerank response index {0} is out of range")]
    IndexOutOfRange(i64),
    /// The decisions API gave no Noul probability for an item.
    #[error("decision response is missing a noul probability for item {}", gostrconv::quote(.0))]
    MissingProbability(String),
    /// The decisions API gave a Noul probability outside [0, 1].
    #[error("decision response has an invalid noul probability for item {}: {}", gostrconv::quote(.item), gostrconv::format_float(*.score, b'g', -1, 64))]
    InvalidProbability {
        /// The item the probability is for.
        item: String,
        /// The probability the API gave.
        score: f64,
    },
}

/// The query and document templates of a ranker: `rankerTemplates` in Go.
struct RankerTemplates {
    env: Environment<'static>,
    query_name: &'static str,
    document_name: &'static str,
}

impl RankerTemplates {
    /// Go's `newRankerTemplates`: parses both templates.
    fn new(
        query_name: &'static str,
        query_template: &str,
        document_name: &'static str,
        document_template: &str,
    ) -> Result<RankerTemplates, minijinja::Error> {
        let mut env = Environment::new();
        env.set_formatter(go_formatter);
        env.add_template_owned(query_name, query_template.to_owned())?;
        env.add_template_owned(document_name, document_template.to_owned())?;
        Ok(RankerTemplates {
            env,
            query_name,
            document_name,
        })
    }

    /// Go's `render`: the query from the user and the feedback, and one
    /// document per item.
    fn render(
        &self,
        user: Option<&User>,
        feedback: &[FeedbackItem],
        items: &[Item],
    ) -> Result<(String, Vec<String>), minijinja::Error> {
        let query = self
            .env
            .get_template(self.query_name)?
            .render(Value::from(BTreeMap::from([
                ("user", user_value(user)),
                (
                    "feedback",
                    Value::from(feedback.iter().map(feedback_value).collect::<Vec<_>>()),
                ),
            ])))?;
        let documents = items
            .iter()
            .map(|item| {
                self.env
                    .get_template(self.document_name)?
                    .render(Value::from(BTreeMap::from([("item", item_value(item))])))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((query, documents))
    }
}

/// The template value of a user; a nil user is a nil value, as in Go.
fn user_value(user: Option<&User>) -> Value {
    match user {
        None => Value::from(()),
        Some(user) => Value::from(BTreeMap::from([
            ("UserId", Value::from(user.user_id.as_str())),
            ("Labels", labels_value(&user.labels)),
            ("Comment", Value::from(user.comment.as_str())),
            ("UpdateAt", Value::from(user.update_at.to_string())),
        ])),
    }
}

/// The formatter of the templates: prints a value as [`go_text`] gives it.
fn go_formatter(out: &mut Output, _: &mut State, value: &Value) -> Result<(), minijinja::Error> {
    out.write_str(&go_text(value))?;
    Ok(())
}

/// The text gonja gives a value, which `{{ }}` prints. A float has 11 fixed
/// decimals with the trailing zeros trimmed ([`go_float`]); a bool is `True` or
/// `False`; nil is empty. A list is `[a, b]`, with a string element in single
/// quotes, as gonja's `String` prints it. Other values print as minijinja does.
fn go_text(value: &Value) -> String {
    match value.kind() {
        ValueKind::Undefined | ValueKind::None => String::new(),
        ValueKind::Bool => String::from(if value.is_true() { "True" } else { "False" }),
        ValueKind::Number if value.is_integer() => value.to_string(),
        ValueKind::Number => {
            f64::try_from(value.clone()).map_or_else(|_| value.to_string(), go_float)
        }
        ValueKind::String => value.as_str().unwrap_or_default().to_owned(),
        ValueKind::Seq => {
            let elements: Vec<String> = value
                .try_iter()
                .map(|elements| {
                    elements
                        .map(|element| match element.as_str() {
                            Some(text) => format!("'{text}'"),
                            None => go_text(&element),
                        })
                        .collect()
                })
                .unwrap_or_default();
            format!("[{}]", elements.join(", "))
        }
        _ => value.to_string(),
    }
}

/// gonja's float text: `strconv.FormatFloat(f, 'f', 11, 64)` with the trailing
/// zeros trimmed, and a `0` after a point that is left bare. So `1` is `1.0`,
/// `1e-12` is `0.0`, and `0.123456789012345` is `0.12345678901`.
fn go_float(float: f64) -> String {
    let fixed = format!("{float:.11}");
    let trimmed = fixed.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_owned()
    }
}

/// The template value of an item, with the field names of `data.Item`.
fn item_value(item: &Item) -> Value {
    Value::from(item_fields(item))
}

/// The fields of an item, by their Go names.
fn item_fields(item: &Item) -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("ItemId", Value::from(item.item_id.as_str())),
        ("IsHidden", Value::from(item.is_hidden)),
        (
            "Categories",
            match &item.categories {
                None => Value::from(()),
                Some(categories) => Value::from(categories.clone()),
            },
        ),
        ("Timestamp", Value::from(item.timestamp.to_string())),
        ("UpdateAt", Value::from(item.update_at.to_string())),
        ("Labels", labels_value(&item.labels)),
        ("Comment", Value::from(item.comment.as_str())),
    ])
}

/// The template value of a feedback: its type and, promoted from the embedded
/// item, the item's fields.
fn feedback_value(feedback: &FeedbackItem) -> Value {
    let mut fields = item_fields(&feedback.item);
    fields.insert("FeedbackType", Value::from(feedback.feedback_type.as_str()));
    Value::from(fields)
}

/// A labels value as a template value. A JSON number keeps its text, which
/// gonja prints as it is.
fn labels_value(labels: &Labels) -> Value {
    match labels {
        Labels::Nil => Value::from(()),
        Labels::Bool(value) => Value::from(*value),
        Labels::Float64(value) => Value::from(*value),
        Labels::Number(text) | Labels::String(text) => Value::from(text.as_str()),
        Labels::Array(items) => Value::from(items.iter().map(labels_value).collect::<Vec<_>>()),
        Labels::Object(map) => Value::from(
            map.iter()
                .map(|(key, value)| (key.as_str(), labels_value(value)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// A ranker that scores items with a rerank API: `Reranker` in Go.
pub struct Reranker {
    templates: RankerTemplates,
    client: reranker::Client,
    model: String,
}

impl Reranker {
    /// Go's `NewReranker`: parses the query and document templates.
    pub fn new(cfg: &RerankerAPIConfig) -> Result<Reranker, minijinja::Error> {
        Ok(Reranker {
            templates: RankerTemplates::new(
                "query",
                &cfg.query_template,
                "document",
                &cfg.document_template,
            )?,
            client: reranker::Client::new(&cfg.auth_token, &cfg.url),
            model: cfg.model.clone(),
        })
    }

    /// Go's `Rank`: the documents, most relevant first, scored by the API.
    pub fn rank(
        &self,
        ctx: &gocontext::Context,
        user: Option<&User>,
        feedback: &[FeedbackItem],
        items: &[Item],
    ) -> Result<Vec<Score>, RankError> {
        let (query, documents) = self.templates.render(user, feedback, items)?;
        let resp = self.client.rerank(
            ctx,
            reranker::RerankRequest {
                model: self.model.clone(),
                query,
                top_n: 0,
                documents,
            },
        )?;
        resp.results
            .iter()
            .map(|result| {
                let item = usize::try_from(result.index)
                    .ok()
                    .and_then(|index| items.get(index))
                    .ok_or(RankError::IndexOutOfRange(result.index))?;
                Ok(new_score(item.item_id.clone(), result.relevance_score))
            })
            .collect()
    }
}

/// A ranker that scores items with the decisions API: `Decision` in Go.
pub struct Decision {
    templates: RankerTemplates,
    client: decision::Client,
}

impl Decision {
    /// Go's `NewDecision`: parses the state and instructions templates.
    pub fn new(cfg: &DecisionAPIConfig) -> Result<Decision, minijinja::Error> {
        Ok(Decision {
            templates: RankerTemplates::new(
                "state",
                &cfg.state_template,
                "instructions",
                &cfg.instructions_template,
            )?,
            client: decision::Client::new(&cfg.auth_token, &cfg.url, &cfg.model),
        })
    }

    /// Go's `Rank`: the probability that each item is a Noul answer to the
    /// state, most likely first. Nothing is asked when there are no items.
    pub fn rank(
        &self,
        ctx: &gocontext::Context,
        user: Option<&User>,
        feedback: &[FeedbackItem],
        items: &[Item],
    ) -> Result<Vec<Score>, RankError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let (query, documents) = self.templates.render(user, feedback, items)?;
        let mut questions = Questions::new();
        for (item, document) in items.iter().zip(documents) {
            questions.insert(
                item.item_id.clone(),
                NoulQuestion {
                    instructions: Json::String(document),
                    criteria: None,
                },
            );
        }
        let response = self
            .client
            .evaluate(ctx, &Json::String(query), &questions)?;
        let mut scores = Vec::with_capacity(items.len());
        for item in items {
            let answer = response.answers.get(&item.item_id);
            let noul = match answer {
                Some(answer) if answer.kind == "noul" => answer.noul,
                _ => None,
            };
            let Some(score) = noul else {
                return Err(RankError::MissingProbability(item.item_id.clone()));
            };
            if score.is_nan() || score.is_infinite() || !(0.0..=1.0).contains(&score) {
                return Err(RankError::InvalidProbability {
                    item: item.item_id.clone(),
                    score,
                });
            }
            scores.push(new_score(item.item_id.clone(), score));
        }
        cache::sort_documents(&mut scores);
        Ok(scores)
    }
}

/// A score of an item with the fields a ranker leaves unset: not hidden, no
/// categories, and the zero time, as Go's zero `cache.Score`.
fn new_score(id: String, score: f64) -> Score {
    Score {
        id,
        score,
        is_hidden: false,
        categories: Vec::new(),
        timestamp: Default::default(),
    }
}

#[cfg(test)]
mod tests;
