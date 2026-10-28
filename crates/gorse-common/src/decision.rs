// Ports: common/decision/client.go @ add21b5

//! A client for the Jev decisions API: it asks yes/no questions about a state,
//! and returns the probability of "yes" for each of them.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::gocontext;
use crate::http;

/// The questions of one request, by the caller's ids. A `BTreeMap` so that the
/// request lists them in key order, as Go's encoder sorts map keys.
pub type Questions = BTreeMap<String, NoulQuestion>;

/// The yes and no outcomes of a Noul question. Either may be absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// What the question means when the answer is yes.
    #[serde(rename = "true", default, skip_serializing_if = "is_absent")]
    pub when_true: Option<Value>,
    /// What the question means when the answer is no.
    #[serde(rename = "false", default, skip_serializing_if = "is_absent")]
    pub when_false: Option<Value>,
}

/// A question that asks for the probability of "yes".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoulQuestion {
    /// The question the model answers, as JSON.
    pub instructions: Value,
    /// The yes and no outcomes, if the question gives them.
    pub criteria: Option<NoulCriteria>,
}

impl Serialize for NoulQuestion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            instructions: &'a Value,
            #[serde(skip_serializing_if = "Option::is_none")]
            criteria: Option<&'a NoulCriteria>,
        }
        Wire {
            kind: "noul",
            instructions: &self.instructions,
            criteria: self.criteria.as_ref(),
        }
        .serialize(serializer)
    }
}

/// The probability a Noul question returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The answer's type, `noul` for a Noul question.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// The probability of "yes"; `null` when the provider gave none.
    pub noul: Option<f64>,
}

/// Token usage, and the cost when the provider reports one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the request.
    #[serde(default)]
    pub input_tokens: i64,
    /// Tokens in the reply.
    #[serde(default)]
    pub output_tokens: i64,
    /// The cost of the request, when the provider reports one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cost: f64,
}

/// A Jev decisions response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// The model that answered.
    #[serde(default)]
    pub model: String,
    /// The answers, by the question ids of the request.
    #[serde(default)]
    pub answers: BTreeMap<String, Answer>,
    /// Token usage of the request.
    #[serde(default)]
    pub usage: Usage,
    /// The provider's id for the request.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The provider that served the request.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

/// A reply with a status outside 2xx from the Jev API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The HTTP status of the reply.
    pub status_code: u16,
    /// The body of the reply, as it came.
    pub body: Vec<u8>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "jev request failed with status {}: {}",
            self.status_code,
            String::from_utf8_lossy(&self.body).trim()
        )
    }
}

impl std::error::Error for ApiError {}

/// The errors of [`Client::evaluate`].
#[derive(Debug, Error)]
pub enum Error {
    /// The request could not be encoded, or the reply could not be decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The context was done before the reply arrived.
    #[error(transparent)]
    Context(#[from] gocontext::ContextError),
    /// The request could not be sent, or its reply could not be read.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    /// The API replied with a status outside 2xx.
    #[error(transparent)]
    Api(#[from] ApiError),
}

/// A client that calls one Jev decisions endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    api_key: String,
    endpoint: String,
    model: String,
}

#[derive(Serialize)]
struct EvaluateRequest<'a> {
    model: &'a str,
    state: &'a Value,
    questions: &'a Questions,
}

impl Client {
    /// Go's `NewClient`: a client with an explicit endpoint and model.
    pub fn new(api_key: &str, endpoint: &str, model: &str) -> Client {
        Client {
            api_key: api_key.to_owned(),
            endpoint: endpoint.to_owned(),
            model: model.to_owned(),
        }
    }

    /// Go's `Evaluate`: evaluates all `questions` against the same `state` in
    /// one request.
    pub fn evaluate(
        &self,
        ctx: &gocontext::Context,
        state: &Value,
        questions: &Questions,
    ) -> Result<Response, Error> {
        let body = serde_json::to_vec(&EvaluateRequest {
            model: &self.model,
            state,
            questions,
        })?;
        let reply = http::post(
            ctx,
            &self.endpoint,
            vec![
                ("Authorization", format!("Bearer {}", self.api_key)),
                ("Content-Type", "application/json".to_owned()),
            ],
            body,
        )
        .map_err(|err| match err {
            http::PostError::Context(err) => Error::Context(err),
            http::PostError::Transport(err) => Error::Transport(err),
        })?;
        if !(200..300).contains(&reply.status) {
            return Err(ApiError {
                status_code: reply.status,
                body: reply.body,
            }
            .into());
        }
        Ok(serde_json::from_slice(&reply.body)?)
    }
}

/// Whether an optional outcome is missing, which Go's `omitempty` skips for an
/// `any` field holding nil.
fn is_absent(value: &Option<Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

#[cfg(test)]
mod tests {
    // Ports: common/decision/client_test.go @ add21b5

    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::thread;

    use serde_json::json;

    use super::*;

    /// What the test server saw of one request.
    #[derive(Debug, Default)]
    struct Seen {
        method: String,
        authorization: Option<String>,
        content_type: Option<String>,
        body: Value,
    }

    /// Serves `reply` for one test, and closes the server when `test` returns,
    /// or unwinds.
    fn with_server(
        reply: impl Fn(&http::Request) -> http::Response + Sync,
        test: impl FnOnce(&str, &Mutex<Option<Seen>>),
    ) {
        struct Close<'a>(&'a http::Server);
        impl Drop for Close<'_> {
            fn drop(&mut self) {
                self.0.close();
            }
        }
        let server = http::Server::bind().expect("bind");
        let url = format!("http://{}/", server.addr());
        let seen = Mutex::new(None);
        let handler = |request: http::Request| {
            *lock(&seen) = Some(Seen {
                method: request.method.clone(),
                authorization: request.header("authorization").map(str::to_owned),
                content_type: request.header("content-type").map(str::to_owned),
                body: serde_json::from_slice(&request.body).unwrap_or(Value::Null),
            });
            reply(&request)
        };
        thread::scope(|scope| {
            scope.spawn(|| server.serve(&handler).expect("serve"));
            let _close = Close(&server);
            test(&url, &seen);
        });
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn json_reply(status: u16, body: &str) -> http::Response {
        http::Response {
            status,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn client_evaluate() {
        with_server(
            |_| {
                json_reply(
                    200,
                    r#"{
                        "model":"typesafe/jev-1.13-20260917",
                        "answers":{"is_urgent":{"type":"noul","noul":0.96}},
                        "usage":{"input_tokens":427,"output_tokens":73,"cost":0.000017934},
                        "id":"gen-dec-123",
                        "provider":"TypeSafe"
                    }"#,
                )
            },
            |url, seen| {
                let client = Client::new("test-key", url, "test-model");
                let questions = Questions::from([(
                    "is_urgent".to_owned(),
                    NoulQuestion {
                        instructions: json!("Does this message convey urgency?"),
                        criteria: Some(NoulCriteria {
                            when_true: Some(json!("Explicitly time-sensitive")),
                            when_false: Some(json!("No urgency expressed")),
                        }),
                    },
                )]);
                let response = client
                    .evaluate(
                        &gocontext::Context::background(),
                        &json!({"message": "Help! My payouts have been failing for 3 days."}),
                        &questions,
                    )
                    .expect("evaluate");

                let request = lock(seen).take().expect("the server saw the request");
                assert_eq!(request.method, "POST");
                assert_eq!(request.authorization.as_deref(), Some("Bearer test-key"));
                assert_eq!(request.content_type.as_deref(), Some("application/json"));
                assert_eq!(request.body["model"], "test-model");
                assert_eq!(
                    request.body["state"]["message"],
                    "Help! My payouts have been failing for 3 days."
                );
                assert_eq!(
                    request.body["questions"]["is_urgent"],
                    json!({
                        "type": "noul",
                        "instructions": "Does this message convey urgency?",
                        "criteria": {
                            "true": "Explicitly time-sensitive",
                            "false": "No urgency expressed"
                        }
                    })
                );

                assert_eq!(response.model, "typesafe/jev-1.13-20260917");
                assert_eq!(response.answers["is_urgent"].noul, Some(0.96));
                assert_eq!(response.usage.input_tokens, 427);
                assert_eq!(response.usage.cost, 0.000017934);
                assert_eq!(response.id, "gen-dec-123");
                assert_eq!(response.provider, "TypeSafe");
            },
        );
    }

    #[test]
    fn client_evaluate_with_model() {
        with_server(
            |_| json_reply(200, r#"{"model":"jev-1.13.0","answers":{},"usage":{}}"#),
            |url, seen| {
                let client = Client::new("test-key", url, "jev-1.13.0");
                client
                    .evaluate(
                        &gocontext::Context::background(),
                        &json!("state"),
                        &Questions::new(),
                    )
                    .expect("evaluate");
                let request = lock(seen).take().expect("the server saw the request");
                assert_eq!(request.body["model"], "jev-1.13.0");
            },
        );
    }

    #[test]
    fn client_evaluate_error() {
        with_server(
            |_| json_reply(422, r#"{"error":"invalid question"}"#),
            |url, _| {
                let client = Client::new("test-key", url, "test-model");
                let err = client
                    .evaluate(
                        &gocontext::Context::background(),
                        &json!("state"),
                        &Questions::new(),
                    )
                    .expect_err("422 is an error");
                let Error::Api(api) = &err else {
                    panic!("want an API error, got {err:?}");
                };
                assert_eq!(api.status_code, 422);
                assert_eq!(
                    serde_json::from_slice::<Value>(&api.body).expect("JSON body"),
                    json!({"error": "invalid question"})
                );
                assert_eq!(
                    err.to_string(),
                    r#"jev request failed with status 422: {"error":"invalid question"}"#
                );
            },
        );
    }

    #[test]
    fn noul_question_serializes_as_go_does() {
        let question = NoulQuestion {
            instructions: json!("Is it urgent?"),
            criteria: None,
        };
        assert_eq!(
            serde_json::to_value(&question).expect("encode"),
            json!({"type": "noul", "instructions": "Is it urgent?"})
        );
        let with_nil_outcomes = NoulQuestion {
            instructions: Value::Null,
            criteria: Some(NoulCriteria::default()),
        };
        assert_eq!(
            serde_json::to_value(&with_nil_outcomes).expect("encode"),
            json!({"type": "noul", "instructions": null, "criteria": {}})
        );
    }
}
