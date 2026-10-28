// Ports: common/reranker/client.go @ add21b5

//! A client for a reranking API, which orders documents by their relevance to a
//! query, and the mock server that answers that API in tests.

use std::fmt;
use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::gocontext::{self, ContextError};
use crate::http;

/// A rerank request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RerankRequest {
    /// The rerank model to use.
    pub model: String,
    /// The text the documents are ranked against.
    pub query: String,
    /// How many results to return; zero asks for all of them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub top_n: i64,
    /// The documents to rank.
    pub documents: Vec<String>,
}

/// A rerank response: one result per document, in the order the API chose.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RerankResponse {
    /// The model that ranked the documents.
    #[serde(default)]
    pub model: String,
    /// Token usage of the request.
    #[serde(default)]
    pub usage: Usage,
    /// The ranking, most relevant first.
    #[serde(default)]
    pub results: Vec<RerankResult>,
}

/// One document's place in the ranking and its relevance score.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RerankResult {
    /// The position of the document in the request.
    #[serde(default)]
    pub index: i64,
    /// How relevant the document is to the query.
    #[serde(default)]
    pub relevance_score: f64,
}

/// Token usage of a rerank request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the request and the reply.
    #[serde(default)]
    pub total_tokens: i64,
}

/// The errors of [`Client::rerank`].
#[derive(Debug, Error)]
pub enum Error {
    /// The request could not be encoded, or the reply could not be decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The context was done before the reply arrived.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// The request could not be sent, or its reply could not be read.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    /// The API replied with a status other than 200 OK.
    #[error("{0}")]
    Status(StatusError),
}

/// A reply that was not 200 OK, with its body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusError {
    /// The HTTP status of the reply.
    pub status: u16,
    /// The body of the reply, as text.
    pub body: String,
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rerank request failed with status: {}, body: {}",
            self.status, self.body
        )
    }
}

impl std::error::Error for StatusError {}

/// A client for one rerank endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    auth_token: String,
    url: String,
}

impl Client {
    /// Go's `NewClient`.
    pub fn new(api_key: &str, endpoint: &str) -> Client {
        Client {
            auth_token: api_key.to_owned(),
            url: endpoint.to_owned(),
        }
    }

    /// Go's `Rerank`: sends `req` and returns the ranking.
    pub fn rerank(
        &self,
        ctx: &gocontext::Context,
        req: RerankRequest,
    ) -> Result<RerankResponse, Error> {
        let body = serde_json::to_vec(&req)?;
        let reply = http::post(
            ctx,
            &self.url,
            vec![
                ("Authorization", format!("Bearer {}", self.auth_token)),
                ("Content-Type", "application/json".to_owned()),
            ],
            body,
        )
        .map_err(|err| match err {
            http::PostError::Context(err) => Error::Context(err),
            http::PostError::Transport(err) => Error::Transport(err),
        })?;
        if reply.status != 200 {
            return Err(Error::Status(StatusError {
                status: reply.status,
                body: String::from_utf8_lossy(&reply.body).into_owned(),
            }));
        }
        Ok(serde_json::from_slice(&reply.body)?)
    }
}

/// A mock of the rerank API on a local port. It scores document `i` as
/// `1 / (i + 1)`, so the first document is the most relevant.
#[derive(Debug)]
pub struct MockServer {
    lifecycle: http::Lifecycle,
    api_key: String,
}

impl Default for MockServer {
    fn default() -> Self {
        Self::new()
    }
}

impl MockServer {
    /// Go's `NewMockServer`. Nothing listens until [`MockServer::start`].
    pub fn new() -> MockServer {
        MockServer {
            lifecycle: http::Lifecycle::default(),
            api_key: "dashscope".to_owned(),
        }
    }

    /// Go's `Start`: listens and serves until [`MockServer::close`]. Go returns
    /// `http.ErrServerClosed` after a close; here that is `Ok(())`.
    pub fn start(&self) -> io::Result<()> {
        let server = self.lifecycle.bind()?;
        server.serve(&rerank)
    }

    /// Go's `URL`: the rerank endpoint. Waits until the server is listening.
    pub fn url(&self) -> String {
        let server = self.lifecycle.wait_ready();
        format!("http://{}/v1/rerank", server.addr())
    }

    /// The key the mock accepts; Go's `AuthToken`.
    pub fn auth_token(&self) -> &str {
        &self.api_key
    }

    /// Go's `Ready`: waits until [`MockServer::start`] is listening.
    pub fn ready(&self) {
        self.lifecycle.wait_ready();
    }

    /// Go's `Close`: stops serving. Calling it before `start` does nothing.
    pub fn close(&self) -> io::Result<()> {
        if let Some(server) = self.lifecycle.get() {
            server.close();
        }
        Ok(())
    }
}

/// Answers `POST /v1/rerank`, as Go's go-restful route does.
fn rerank(request: http::Request) -> http::Response {
    if request.method != "POST" || request.path != "/v1/rerank" {
        return http::Response {
            status: 404,
            content_type: "text/plain; charset=utf-8",
            body: b"404: Page Not Found".to_vec(),
        };
    }
    let req: RerankRequest = match serde_json::from_slice(&request.body) {
        Ok(req) => req,
        Err(err) => {
            return http::Response {
                status: 400,
                content_type: "text/plain; charset=utf-8",
                body: err.to_string().into_bytes(),
            };
        }
    };
    let results = (0..req.documents.len())
        .map(|i| RerankResult {
            index: i as i64,
            relevance_score: 1.0 / (i as f64 + 1.0),
        })
        .collect();
    let response = RerankResponse {
        model: req.model,
        usage: Usage { total_tokens: 100 },
        results,
    };
    http::Response {
        status: 200,
        content_type: "application/json",
        body: serde_json::to_vec(&response).expect("a rerank response encodes"),
    }
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    // Ports: common/reranker/client_test.go @ add21b5

    use std::thread;

    use super::*;

    /// Closes the mock when dropped, so a failing test does not leave the
    /// serving thread waiting.
    struct CloseOnDrop<'a>(&'a MockServer);

    impl Drop for CloseOnDrop<'_> {
        fn drop(&mut self) {
            let _ = self.0.close();
        }
    }

    #[test]
    fn rerank() {
        let server = MockServer::new();
        thread::scope(|scope| {
            scope.spawn(|| server.start().expect("start"));
            let _close = CloseOnDrop(&server);
            server.ready();

            let client = Client::new(server.auth_token(), &server.url());
            let response = client
                .rerank(
                    &gocontext::Context::background(),
                    RerankRequest {
                        model: "jina-reranker-v2-base-multilingual".to_owned(),
                        query: "What is the capital of France?".to_owned(),
                        top_n: 0,
                        documents: vec![
                            "Paris is the capital of France.".to_owned(),
                            "Lyon is a city in France.".to_owned(),
                        ],
                    },
                )
                .expect("rerank");
            assert_eq!(response.results.len(), 2);
            assert_eq!(response.results[0].index, 0);
            assert_eq!(response.results[0].relevance_score, 1.0);
            assert_eq!(response.results[1].index, 1);
            assert_eq!(response.results[1].relevance_score, 0.5);
        });
    }

    #[test]
    fn top_n_is_omitted_when_zero() {
        let req = RerankRequest {
            model: "m".to_owned(),
            query: "q".to_owned(),
            top_n: 0,
            documents: vec!["d".to_owned()],
        };
        assert_eq!(
            serde_json::to_string(&req).expect("encode"),
            r#"{"model":"m","query":"q","documents":["d"]}"#
        );
    }
}
