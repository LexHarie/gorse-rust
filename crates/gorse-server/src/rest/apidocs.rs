// Ports: server/rest.go (`/apidocs.json` and `/apidocs/` through `swgui/v5emb`) @ add21b5, with swgui v1.8.5

//! The OpenAPI document and the Swagger UI page (X-214, X-254).
//!
//! `/apidocs.json` is the node's OpenAPI document, the frozen contract file that
//! the pinned Go node serves: `contracts/rest/server.openapi.json` for the server
//! and `contracts/rest/master.openapi.json` for the master. It is a WebService
//! route, so it goes through the CORS filter in `cors.rs`. `/apidocs/` is Swagger
//! UI, the page that loads the document from `/apidocs.json` and the files it
//! names beside it. Those are plain handlers, outside the filter.
//!
//! The page and its files are the swgui v1.8.5 bundle that Go embeds. They are
//! vendored in `swagger/`, decompressed, and `swagger/MANIFEST.sha256` checks
//! them. Go's `ServeMux` answers `/apidocs` with a temporary redirect to
//! `/apidocs/`, and the Go file server answers a name it does not hold with a
//! plain 404.

use axum::Router;
use axum::extract::Path;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

use super::response::temporary_redirect;

/// The vendored Swagger UI bundle. The checksums are not served.
#[derive(RustEmbed)]
#[folder = "swagger/"]
#[exclude = "MANIFEST.sha256"]
struct Swagger;

/// The page, which Go serves at `/apidocs/` and never as a file.
const PAGE: &str = "index.html";

/// The files Go's Swagger UI serves beside the page.
const FILES: [&str; 7] = [
    "favicon-16x16.png",
    "favicon-32x32.png",
    "oauth2-redirect.html",
    "swagger-ui-bundle.js",
    "swagger-ui-standalone-preset.js",
    "swagger-ui.css",
    "swagger-ui.js",
];

/// The routes of the Swagger UI page: `/apidocs`, the page at `/apidocs/`, and
/// its files.
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/apidocs", get(redirect_to_page))
        .route("/apidocs/", get(page))
        .route("/apidocs/{*file}", get(file))
}

/// Go's `/apidocs.json`: the node's OpenAPI document, `document`. A WebService
/// route, so the CORS filter sees it.
pub fn document_route<S>(document: &'static str) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/apidocs.json",
        get(move || async move { ([(header::CONTENT_TYPE, "application/json")], document) }),
    )
}

/// Go's `ServeMux` redirect from `/apidocs` to `/apidocs/`.
async fn redirect_to_page(method: Method) -> Response {
    temporary_redirect(&method, "/apidocs/")
}

/// Go's `/apidocs/`: the Swagger UI page. Go's template writes it as `text/html`,
/// with no charset.
async fn page() -> Response {
    match embedded(PAGE) {
        Some(body) => ([(header::CONTENT_TYPE, "text/html")], body).into_response(),
        None => not_found(),
    }
}

/// A Swagger UI file, by its name in the bundle. A name that is not one of the
/// files Go serves is a 404.
async fn file(Path(name): Path<String>) -> Response {
    if !FILES.contains(&name.as_str()) {
        return not_found();
    }
    match embedded(&name) {
        Some(body) => ([(header::CONTENT_TYPE, content_type(&name))], body).into_response(),
        None => not_found(),
    }
}

/// The bytes of a vendored file.
fn embedded(name: &str) -> Option<Vec<u8>> {
    Swagger::get(name).map(|file| file.data.into_owned())
}

/// The media type Go's file server gives a file, from its extension.
fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js") => "application/javascript",
        Some("css") => "text/css; charset=utf-8",
        Some("png") => "image/png",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Go's `http.NotFound`: a 404 with the text `404 page not found`.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "404 page not found\n").into_response()
}
