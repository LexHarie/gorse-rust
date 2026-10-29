// Ports: master/rest.go (`chatCompletions`) @ add21b5

//! The chat completions proxy (`master/rest.go`). It forwards a request to the
//! OpenAI-compatible server in `openai.base_url`, and answers with that server's
//! reply: as JSON, or as a server-sent event stream.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gorse_server::rest::response::{ApiError, escape_html};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Master;
use crate::rest::{auth, method_not_allowed, write_error};

/// A chat request. The fields the proxy reads are typed; every other field is
/// forwarded as the client sent it.
#[derive(Debug, Default, Deserialize, Serialize)]
struct ChatRequest {
    /// The model (`model`). Empty means the configured default.
    #[serde(default)]
    model: String,
    /// Whether the reply is a stream (`stream`).
    #[serde(default)]
    stream: bool,
    /// The remaining fields of the request.
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// `ANY /api/chat/completions`: forwards a chat request with `POST`.
pub(crate) async fn chat_completions(State(master): State<Arc<Master>>, request: Request) -> Response {
    if *request.method() != Method::POST {
        return method_not_allowed();
    }
    if !auth::check_login(&master, request.headers()) {
        return write_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let body = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(err) => return write_error(StatusCode::BAD_REQUEST, err.to_string()),
    };
    let mut chat: ChatRequest = match serde_json::from_slice(&body) {
        Ok(chat) => chat,
        Err(err) => return write_error(StatusCode::BAD_REQUEST, err.to_string()),
    };
    let cfg = master.config();
    if chat.model.is_empty() {
        chat.model = cfg.openai.chat_completion_model.clone();
    }
    if chat.model.is_empty() {
        return write_error(StatusCode::BAD_REQUEST, "missing chat completion model");
    }
    let stream = chat.stream;
    let upstream = match send(&cfg.openai.base_url, &cfg.openai.auth_token, &chat).await {
        Ok(upstream) => upstream,
        Err(err) => return write_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    if !stream {
        return json_reply(&upstream);
    }
    event_stream_reply(&upstream)
}

/// Posts `chat` to `{base_url}/chat/completions` with the bearer token, and
/// returns the body of a successful reply. The error text is Go's
/// `go-openai` error text: the status and the server's message.
async fn send(base_url: &str, token: &str, chat: &ChatRequest) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(chat)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "error, status code: {}, status: {}, message: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            text
        ));
    }
    Ok(text)
}

/// The JSON reply: the server's JSON, encoded as Go's `json.Encoder` encodes
/// it, with the `<`, `>` and `&` escapes and a newline.
fn json_reply(upstream: &str) -> Response {
    let value: Value = match serde_json::from_str(upstream) {
        Ok(value) => value,
        Err(err) => return ApiError::internal(err).into_response(),
    };
    let encoded = match serde_json::to_vec(&value) {
        Ok(encoded) => encoded,
        Err(err) => return ApiError::internal(err).into_response(),
    };
    let mut body = escape_html(&encoded);
    body.push(b'\n');
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// The stream reply: each event of the server's stream as `data: <json>` and a
/// blank line, then `data: [DONE]`. A `[DONE]` from the server ends the stream
/// early, as `go-openai` reads it as the end.
fn event_stream_reply(upstream: &str) -> Response {
    let mut body = Vec::new();
    for event in upstream.split("\n\n") {
        let Some(payload) = event.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        body.extend_from_slice(b"data: ");
        body.extend_from_slice(payload.as_bytes());
        body.extend_from_slice(b"\n\n");
    }
    body.extend_from_slice(b"data: [DONE]\n\n");
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    response
}
