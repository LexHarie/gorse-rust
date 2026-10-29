// Ports: the `CreateEmbeddings` call of go-openai v1.41.2 that `server/rest.go`
// `searchEmbedding` makes, with the error texts go-openai builds (`APIError`,
// `RequestError`) @ add21b5

//! The embedding request an item search with `index` makes to the OpenAI API.
//!
//! go-openai builds the request as `POST {base_url}/embeddings` with a bearer
//! token, and its JSON body has `input`, `model`, and `dimensions` when it is
//! not zero. Gorse uses no other request field, so this client sends no others.

use serde::{Deserialize, Serialize};

/// The body of `POST /embeddings`, with go-openai's field names and
/// `omitempty` on `dimensions`.
#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a str,
    model: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    dimensions: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// The part of the response this client reads.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    #[serde(default)]
    data: Vec<Embedding>,
}

#[derive(Debug, Deserialize)]
struct Embedding {
    #[serde(default)]
    embedding: Vec<f32>,
}

/// The error body the API sends on failure: `{"error": {"message": ...}}`.
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    message: String,
}

/// Requests the embedding of `input` from `base_url` and returns the first
/// embedding of the response, as `openai.Client.CreateEmbeddings` does.
///
/// # Errors
///
/// Fails with go-openai's error text when the request fails or the API answers
/// with a failure status. The caller writes the text as the response body.
pub(crate) async fn create_embedding(
    base_url: &str,
    auth_token: &str,
    model: &str,
    dimensions: i64,
    input: &str,
) -> Result<Vec<f32>, String> {
    let body = serde_json::to_vec(&EmbeddingRequest {
        input,
        model,
        dimensions,
    })
    .map_err(|err| err.to_string())?;
    let url = format!("{base_url}/embeddings");
    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {auth_token}"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(failure_text(status, &bytes));
    }
    let decoded: EmbeddingResponse =
        serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
    Ok(decoded
        .data
        .into_iter()
        .next()
        .map(|embedding| embedding.embedding)
        .unwrap_or_default())
}

/// The text go-openai's `handleErrorResp` leads to. A body with an `error`
/// object gives an `APIError`; anything else gives a `RequestError`, whose
/// `Err` is nil when the body parsed without an error object, and whose text
/// Go's `%s` prints as `%!s(<nil>)`.
fn failure_text(status: reqwest::StatusCode, body: &[u8]) -> String {
    let status_line = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or_default()
    );
    let code = status.as_u16();
    match serde_json::from_slice::<ErrorResponse>(body) {
        Ok(ErrorResponse { error: Some(error) }) => format!(
            "error, status code: {code}, status: {status_line}, message: {}",
            error.message
        ),
        _ => format!(
            "error, status code: {code}, status: {status_line}, message: %!s(<nil>), body: {}",
            String::from_utf8_lossy(body)
        ),
    }
}
