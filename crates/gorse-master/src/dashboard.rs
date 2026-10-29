// Ports: master/rest.go (`staticFileSystem`, `SinglePageAppFileSystem`, `noCache`, `dashboard` and `login`'s file serving) @ add21b5

//! The embedded dashboard (D-240).
//!
//! The Vue bundle is the zip that the Go module `github.com/gorse-io/dashboard`
//! embeds. `contracts/tools/dashboard` extracts it into `dashboard/`, and
//! [`Dashboard`] embeds that tree with rust-embed. The files are served as they
//! are, and every path that names no file gets `index.html`, as Go's
//! `SinglePageAppFileSystem` does.

use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use rust_embed::RustEmbed;

/// The vendored dashboard tree. The README and the manifest are not served.
#[derive(RustEmbed)]
#[folder = "dashboard/"]
#[exclude = "README.md"]
#[exclude = "MANIFEST.sha256"]
pub struct Dashboard;

/// The name of the page every unknown path falls back to.
const INDEX: &str = "index.html";

/// The `Expires` header of [`no_cache`]: Go's `time.Unix(0, 0).Format(time.RFC1123)`
/// in the UTC zone the nodes run in.
const EPOCH: &str = "Thu, 01 Jan 1970 00:00:00 UTC";

/// Go's `mime` table for the types the bundle holds. Go also reads
/// `/etc/mime.types`, which the vendored types here do not depend on.
fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    match extension {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "text/plain; charset=utf-8",
    }
}

/// Go's `SinglePageAppFileSystem.Open` followed by `http.FileServer`: the file
/// at `path`, or `index.html` when no file has that name. A request for
/// `.../index.html` is redirected to its directory, as `http.FileServer` does.
pub fn serve_file(method: &Method, path: &str) -> Response {
    let name = path.trim_start_matches('/');
    let name = if name.is_empty() { INDEX } else { name };
    if path.ends_with("/index.html") {
        return redirect_to_directory();
    }
    let (name, file) = match Dashboard::get(name) {
        Some(file) => (name, file),
        None => match Dashboard::get(INDEX) {
            Some(file) => (INDEX, file),
            None => return not_found(),
        },
    };
    let len = file.data.len();
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(file.data.into_owned())
    };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(name)),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

/// `http.FileServer`'s redirect for a request that ends in `index.html`.
fn redirect_to_directory() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("./"));
    response
}

/// `http.NotFound`, for a bundle that has no `index.html`. The embedded tree
/// always has one, so this is not reached in practice.
fn not_found() -> Response {
    let mut response = Response::new(Body::from("404 page not found\n"));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Go's `noCache`: the headers that stop a proxy or a client caching the page.
pub fn no_cache(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::EXPIRES, HeaderValue::from_static(EPOCH));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(
            "no-cache, no-store, no-transform, must-revalidate, private, max-age=0",
        ),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        axum::http::HeaderName::from_static("x-accel-expires"),
        HeaderValue::from_static("0"),
    );
}
