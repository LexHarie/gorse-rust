// Ports: common/mock/openai.go @ add21b5

//! A mock of the OpenAI chat and embeddings APIs, for tests. A chat completion
//! answers with its first message, streamed in 8-byte pieces when asked; an
//! embedding is the MD-5 digest of its input, one float per byte.
//!
//! The JSON it writes has the field names and the values go-openai reads, and
//! the zero values go-openai writes for the rest. It leaves out
//! `content_filter_results` and the unnamed `MultiContent` field, which no
//! Gorse code reads (`porting/notes/WP-1-12.md`, X-WP12-2).

use std::io;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http;

/// A mock of the OpenAI API on a local port.
#[derive(Debug)]
pub struct OpenAiServer {
    lifecycle: http::Lifecycle,
    auth_token: String,
}

impl Default for OpenAiServer {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiServer {
    /// Go's `NewOpenAIServer`. Nothing listens until [`OpenAiServer::start`].
    pub fn new() -> OpenAiServer {
        OpenAiServer {
            lifecycle: http::Lifecycle::default(),
            auth_token: "ollama".to_owned(),
        }
    }

    /// Go's `Start`: listens and serves until [`OpenAiServer::close`]. Go
    /// returns `http.ErrServerClosed` after a close; here that is `Ok(())`.
    pub fn start(&self) -> io::Result<()> {
        let server = self.lifecycle.bind()?;
        server.serve(&route)
    }

    /// Go's `BaseURL`: the `/v1` URL. Waits until the server is listening.
    pub fn base_url(&self) -> String {
        let server = self.lifecycle.wait_ready();
        format!("http://{}/v1", server.addr())
    }

    /// The key the mock expects; Go's `AuthToken`.
    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    /// Go's `Ready`: waits until [`OpenAiServer::start`] is listening.
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

/// Go's `Hash`: the 16 bytes of the MD-5 digest of `input`, as float32s.
pub fn hash(input: &str) -> Vec<f32> {
    Md5::digest(input.as_bytes())
        .iter()
        .map(|&byte| f32::from(byte))
        .collect()
}

#[derive(Deserialize)]
struct ChatRequest {
    #[serde(default)]
    model: String,
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct EmbeddingRequest {
    #[serde(default)]
    input: Value,
}

/// go-openai's `Usage`, all zero.
#[derive(Serialize)]
struct Usage {
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    prompt_tokens_details: Option<Value>,
    completion_tokens_details: Option<Value>,
}

impl Usage {
    fn zero() -> Usage {
        Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }
    }
}

#[derive(Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<ChatCompletionChoice>,
    usage: Usage,
    system_fingerprint: String,
}

#[derive(Serialize)]
struct ChatCompletionChoice {
    index: i64,
    message: ChatCompletionMessage,
    finish_reason: Option<String>,
}

#[derive(Serialize)]
struct ChatCompletionMessage {
    role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    content: String,
}

#[derive(Serialize)]
struct ChatCompletionStreamResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<ChatCompletionStreamChoice>,
    system_fingerprint: String,
}

#[derive(Serialize)]
struct ChatCompletionStreamChoice {
    index: i64,
    delta: ChatCompletionStreamChoiceDelta,
    finish_reason: Option<String>,
}

#[derive(Serialize)]
struct ChatCompletionStreamChoiceDelta {
    #[serde(skip_serializing_if = "String::is_empty")]
    content: String,
}

#[derive(Serialize)]
struct EmbeddingResponse {
    object: String,
    data: Vec<Embedding>,
    model: String,
    usage: Usage,
}

#[derive(Serialize)]
struct Embedding {
    object: String,
    embedding: Vec<f32>,
    index: i64,
}

/// Routes one request as go-restful does: the two `POST` routes under `/v1`.
fn route(request: http::Request) -> http::Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/v1/chat/completions") => chat_completion(&request.body),
        ("POST", "/v1/embeddings") => embeddings(&request.body),
        _ => text(404, "404: Page Not Found"),
    }
}

fn text(status: u16, message: &str) -> http::Response {
    http::Response {
        status,
        content_type: "text/plain; charset=utf-8",
        body: message.as_bytes().to_vec(),
    }
}

fn json<T: Serialize>(value: &T) -> http::Response {
    http::Response {
        status: 200,
        content_type: "application/json",
        body: serde_json::to_vec(value).expect("a mock response encodes"),
    }
}

fn chat_completion(body: &[u8]) -> http::Response {
    let req: ChatRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(err) => return text(400, &err.to_string()),
    };
    // Go indexes the first message without a check, and its server then
    // drops the connection. This mock answers 500 instead.
    let Some(first) = req.messages.first() else {
        return text(500, "index out of range [0] with length 0");
    };
    let mut content = first.content.clone();
    if req.model == "deepseek-r1" {
        content = format!("<think>To be or not to be, that is the question.</think>{content}");
    }
    if req.stream {
        let mut events = Vec::new();
        // Go slices the content by bytes, so a piece can end inside a character.
        for piece in content.as_bytes().chunks(8) {
            let event = ChatCompletionStreamResponse {
                id: String::new(),
                object: String::new(),
                created: 0,
                model: String::new(),
                choices: vec![ChatCompletionStreamChoice {
                    index: 0,
                    delta: ChatCompletionStreamChoiceDelta {
                        content: go_string(piece),
                    },
                    finish_reason: None,
                }],
                system_fingerprint: String::new(),
            };
            events.extend_from_slice(b"data: ");
            events.extend(serde_json::to_vec(&event).expect("a stream event encodes"));
            events.extend_from_slice(b"\n\n");
        }
        return http::Response {
            status: 200,
            content_type: "text/event-stream",
            body: events,
        };
    }
    json(&ChatCompletionResponse {
        id: String::new(),
        object: String::new(),
        created: 0,
        model: String::new(),
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatCompletionMessage {
                role: String::new(),
                content,
            },
            finish_reason: None,
        }],
        usage: Usage::zero(),
        system_fingerprint: String::new(),
    })
}

fn embeddings(body: &[u8]) -> http::Response {
    let req: EmbeddingRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(err) => return text(400, &err.to_string()),
    };
    let Value::String(input) = req.input else {
        return text(400, "invalid input type");
    };
    json(&EmbeddingResponse {
        object: String::new(),
        data: vec![Embedding {
            object: String::new(),
            embedding: hash(&input),
            index: 0,
        }],
        model: String::new(),
        usage: Usage::zero(),
    })
}

/// Decodes `bytes` as Go's JSON encoder reads a string: each byte that does
/// not start a valid UTF-8 sequence becomes U+FFFD, and decoding goes on at
/// the next byte.
fn go_string(mut bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                return text;
            }
            Err(err) => {
                let (valid, rest) = bytes.split_at(err.valid_up_to());
                text.push_str(std::str::from_utf8(valid).expect("valid up to the error"));
                text.push('\u{FFFD}');
                bytes = &rest[1..];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Ports: common/mock/openai_test.go @ add21b5

    use std::thread;

    use serde_json::json;

    use super::*;
    use crate::gocontext;

    /// Closes the mock when dropped, so a failing test does not leave the
    /// serving thread waiting.
    struct CloseOnDrop<'a>(&'a OpenAiServer);

    impl Drop for CloseOnDrop<'_> {
        fn drop(&mut self) {
            let _ = self.0.close();
        }
    }

    /// Sends `body` to `path` under `base` with the mock's key, and returns the reply.
    fn post(server: &OpenAiServer, path: &str, body: &Value) -> http::Reply {
        http::post(
            &gocontext::Context::background(),
            &format!("{}{path}", server.base_url()),
            vec![
                ("Authorization", format!("Bearer {}", server.auth_token())),
                ("Content-Type", "application/json".to_owned()),
            ],
            serde_json::to_vec(body).expect("encode"),
        )
        .expect("post")
    }

    /// Runs `test` against a started mock.
    fn with_mock(test: impl FnOnce(&OpenAiServer)) {
        let server = OpenAiServer::new();
        thread::scope(|scope| {
            scope.spawn(|| server.start().expect("start"));
            let _close = CloseOnDrop(&server);
            server.ready();
            test(&server);
        });
    }

    #[test]
    fn chat_completion() {
        with_mock(|server| {
            let reply = post(
                server,
                "/chat/completions",
                &json!({
                    "model": "qwen2.5",
                    "messages": [{"role": "user", "content": "Hello"}]
                }),
            );
            assert_eq!(reply.status, 200);
            let response: Value = serde_json::from_slice(&reply.body).expect("JSON");
            assert_eq!(response["choices"][0]["message"]["content"], "Hello");
        });
    }

    #[test]
    fn chat_completion_stream() {
        let content = "In my younger and more vulnerable years my father gave me some advice that I've been turning over in \
            my mind ever since. Whenever you feel like criticizing anyone, he told me, just remember that all the \
            people in this world haven't had the advantages that you've had.";
        with_mock(|server| {
            let reply = post(
                server,
                "/chat/completions",
                &json!({
                    "model": "qwen2.5",
                    "messages": [{"role": "user", "content": content}],
                    "stream": true
                }),
            );
            assert_eq!(reply.status, 200);
            let text = String::from_utf8(reply.body).expect("UTF-8 stream");
            let mut received = String::new();
            let mut events = 0;
            for line in text.lines().filter(|line| !line.is_empty()) {
                let event = line
                    .strip_prefix("data: ")
                    .expect("each event is a data line");
                let event: Value = serde_json::from_str(event).expect("JSON event");
                received.push_str(
                    event["choices"][0]["delta"]["content"]
                        .as_str()
                        .unwrap_or(""),
                );
                events += 1;
            }
            assert_eq!(received, content);
            assert_eq!(events, content.len().div_ceil(8));
        });
    }

    #[test]
    fn embeddings() {
        with_mock(|server| {
            let reply = post(
                server,
                "/embeddings",
                &json!({"model": "mxbai-embed-large", "input": "Hello"}),
            );
            assert_eq!(reply.status, 200);
            let response: Value = serde_json::from_slice(&reply.body).expect("JSON");
            let embedding: Vec<f32> = response["data"][0]["embedding"]
                .as_array()
                .expect("an array")
                .iter()
                .map(|value| value.as_f64().expect("a number") as f32)
                .collect();
            assert_eq!(
                embedding,
                vec![
                    139.0, 26.0, 153.0, 83.0, 196.0, 97.0, 18.0, 150.0, 168.0, 39.0, 171.0, 248.0,
                    196.0, 120.0, 4.0, 215.0
                ]
            );
        });
    }

    #[test]
    fn hash_is_the_md5_digest() {
        assert_eq!(
            hash("Hello"),
            vec![
                139.0, 26.0, 153.0, 83.0, 196.0, 97.0, 18.0, 150.0, 168.0, 39.0, 171.0, 248.0,
                196.0, 120.0, 4.0, 215.0
            ]
        );
    }

    #[test]
    fn go_string_replaces_each_invalid_byte() {
        // "é" is two bytes; the first byte alone is invalid, as is each byte of a truncated run.
        assert_eq!(go_string("é".as_bytes()), "é");
        assert_eq!(go_string(&"é".as_bytes()[..1]), "\u{FFFD}");
        assert_eq!(go_string(&[0xE4, 0xB8]), "\u{FFFD}\u{FFFD}");
        assert_eq!(go_string(b"a\xffb"), "a\u{FFFD}b");
    }
}
