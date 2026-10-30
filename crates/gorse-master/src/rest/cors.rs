// Ports: go-restful v3.13.0 `CrossOriginResourceSharing` (the container filter that `server/rest.go` installs on the master's container) and the `Ok`, `Text` and error helpers of `server/rest.go` @ add21b5

//! The CORS filter of the master's WebServices (X-254).
//!
//! Go installs go-restful's CORS filter on the master's container, and a
//! container filter runs only for the WebService routes: the server routes and
//! the dashboard routes under `/api/`, and `/apidocs.json`. The plain handlers
//! the container also registers (the dashboard page, the login, the bulk import
//! and export, dump and restore, the chat proxy, the Swagger page and the pprof
//! handlers) never see it, so [`is_web_service`] leaves them alone.
//!
//! The filter runs before the route. For an origin the config allows, it adds
//! `Access-Control-Allow-Origin` with the origin, and the route's own headers
//! follow it. Go's `Ok` then adds its `*` after the origin, so both are sent. Its
//! error helpers and `Text` replace the origin with `*`, so only `*` is sent. The
//! Rust helpers always set `*`, so [`merge_origin`] tells the two apart by the
//! content type each Go helper writes: `Ok` writes JSON, the others write text.
//!
//! The preflight is answered without the route, as go-restful answers it. Go's
//! own answers to a request no route takes, a 404 or a 405 with no body, are
//! given their Go text by [`restful_errors`].

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::Master;

/// The request headers a preflight may name, as go-restful's filter allows them.
const ALLOWED_HEADERS: [&str; 3] = ["Content-Type", "Accept", "X-API-Key"];

/// The `/api/` paths the plain handlers take. Go registers each with `ServeMux`
/// as an exact pattern, so they are not WebService routes.
const PLAIN_API_PATHS: [&str; 7] = [
    "/api/purge",
    "/api/bulk/users",
    "/api/bulk/items",
    "/api/bulk/feedback",
    "/api/dump",
    "/api/restore",
    "/api/chat/completions",
];

/// The routes of the WebServices, with their methods, in the order Go declares
/// them: the server routes, then the dashboard routes, then the OpenAPI document.
/// A `{name}` segment matches one path segment. Go's `computeAllowedMethods`
/// lists the methods of the routes a path matches, in this order (`cors.rs`
/// [`route_methods`]). Generated from the `ws.GET`, `ws.POST`, `ws.PUT`,
/// `ws.PATCH` and `ws.DELETE` declarations of `server/rest.go` and `master/rest.go`
/// @ add21b5, in file order.
const ROUTES: [(&str, &str); 66] = [
    ("GET", "/api/health/live"),
    ("GET", "/api/health/ready"),
    ("POST", "/api/user"),
    ("PATCH", "/api/user/{user-id}"),
    ("GET", "/api/user/{user-id}"),
    ("POST", "/api/users"),
    ("GET", "/api/users"),
    ("DELETE", "/api/user/{user-id}"),
    ("POST", "/api/item"),
    ("PATCH", "/api/item/{item-id}"),
    ("GET", "/api/items"),
    ("GET", "/api/item/{item-id}"),
    ("POST", "/api/items"),
    ("DELETE", "/api/item/{item-id}"),
    ("PUT", "/api/item/{item-id}/category/{category}"),
    ("DELETE", "/api/item/{item-id}/category/{category}"),
    ("POST", "/api/feedback"),
    ("PUT", "/api/feedback"),
    ("GET", "/api/feedback"),
    ("GET", "/api/feedback/{user-id}/{item-id}"),
    ("DELETE", "/api/feedback/{user-id}/{item-id}"),
    ("GET", "/api/feedback/{feedback-type}"),
    ("GET", "/api/feedback/{feedback-type}/{user-id}/{item-id}"),
    (
        "DELETE",
        "/api/feedback/{feedback-type}/{user-id}/{item-id}",
    ),
    ("GET", "/api/user/{user-id}/feedback/{feedback-type}"),
    ("GET", "/api/user/{user-id}/feedback"),
    ("GET", "/api/item/{item-id}/feedback/{feedback-type}"),
    ("GET", "/api/item/{item-id}/feedback/"),
    ("GET", "/api/collaborative-filtering/{user-id}"),
    ("GET", "/api/collaborative-filtering/{user-id}/{category}"),
    ("GET", "/api/latest"),
    ("GET", "/api/latest/{category}"),
    ("GET", "/api/non-personalized/{name}"),
    ("GET", "/api/item-to-item/{name}/{item-id}"),
    ("GET", "/api/user-to-user/{name}/{user-id}"),
    ("GET", "/api/item/{item-id}/neighbors/"),
    ("GET", "/api/item/{item-id}/neighbors/{category}"),
    ("GET", "/api/user/{user-id}/neighbors/"),
    ("GET", "/api/recommend/{user-id}"),
    ("GET", "/api/recommend/{user-id}/{category}"),
    ("POST", "/api/session/recommend"),
    ("POST", "/api/session/recommend/{category}"),
    ("GET", "/api/dashboard/userinfo"),
    ("GET", "/api/dashboard/cluster"),
    ("GET", "/api/dashboard/categories"),
    ("POST", "/api/dashboard/config"),
    ("GET", "/api/dashboard/config"),
    ("DELETE", "/api/dashboard/config"),
    ("GET", "/api/dashboard/config/schema"),
    ("GET", "/api/dashboard/stats"),
    ("GET", "/api/dashboard/tasks"),
    ("GET", "/api/dashboard/timeseries/{name}"),
    ("GET", "/api/dashboard/user/{user-id}"),
    ("GET", "/api/dashboard/user/{user-id}/feedback/"),
    (
        "GET",
        "/api/dashboard/user/{user-id}/feedback/{feedback-type}",
    ),
    ("GET", "/api/dashboard/users"),
    ("GET", "/api/dashboard/latest"),
    ("GET", "/api/dashboard/non-personalized/{name}"),
    ("GET", "/api/dashboard/recommend/{user-id}"),
    ("GET", "/api/dashboard/recommend/{user-id}/{recommender}"),
    (
        "GET",
        "/api/dashboard/recommend/{user-id}/{recommender}/{name}",
    ),
    ("GET", "/api/dashboard/item-to-item/{name}/{item-id}"),
    ("GET", "/api/dashboard/user-to-user/{name}/{user-id}"),
    ("GET", "/api/dashboard/external"),
    ("GET", "/api/dashboard/ranker/prompt"),
    ("GET", "/apidocs.json"),
];

const ACCESS_CONTROL_REQUEST_METHOD: HeaderName = header::ACCESS_CONTROL_REQUEST_METHOD;
const ACCESS_CONTROL_REQUEST_HEADERS: HeaderName = header::ACCESS_CONTROL_REQUEST_HEADERS;

/// Runs go-restful's CORS filter for the WebService routes, and go-restful's
/// own not-found and not-allowed answers for them.
pub(super) async fn filter(
    State(master): State<Arc<Master>>,
    request: Request,
    next: Next,
) -> Response {
    if !is_web_service(request.uri().path()) {
        return next.run(request).await;
    }
    let config = master.config();
    let domains = config
        .master
        .http_cors_domains
        .as_deref()
        .unwrap_or_default();
    let Some(origin) = allowed_origin(domains, request.headers()) else {
        return restful_errors(next.run(request).await);
    };
    let is_preflight = request.method() == Method::OPTIONS
        && header_text(request.headers(), ACCESS_CONTROL_REQUEST_METHOD)
            .is_some_and(|method| !method.is_empty());
    if is_preflight {
        let configured = config
            .master
            .http_cors_methods
            .as_deref()
            .unwrap_or_default();
        let methods = allowed_methods(configured, request.uri().path());
        return preflight(&methods, &origin, request.headers());
    }
    merge_origin(restful_errors(next.run(request).await), &origin)
}

/// The methods a preflight lists: the configured ones, or, when the config lists
/// none, the methods of the routes the path matches (Go's `computeAllowedMethods`,
/// which the shipped config relies on).
fn allowed_methods(configured: &[String], path: &str) -> Vec<String> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    route_methods(path).into_iter().map(str::to_owned).collect()
}

/// Go's `computeAllowedMethods`: the methods of the WebService routes that
/// `path` matches, in declaration order, duplicates kept.
fn route_methods(path: &str) -> Vec<&'static str> {
    ROUTES
        .iter()
        .filter(|(_, template)| matches_template(template, path))
        .map(|(method, _)| *method)
        .collect()
}

/// Whether a route template matches a request path. The segments must agree,
/// with a `{name}` segment matching any non-empty one, and one trailing slash is
/// allowed, as go-restful's matching allows it.
fn matches_template(template: &str, path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    let template: Vec<&str> = template.split('/').collect();
    let segments: Vec<&str> = path.split('/').collect();
    template.len() == segments.len()
        && template.iter().zip(&segments).all(|(part, segment)| {
            if part.starts_with('{') && part.ends_with('}') {
                !segment.is_empty()
            } else {
                part == segment
            }
        })
}

/// Whether go-restful's container filter sees `path`: the WebService routes,
/// which are the `/api/` paths the plain handlers do not take, and
/// `/apidocs.json`.
pub(super) fn is_web_service(path: &str) -> bool {
    path == "/apidocs.json" || (path.starts_with("/api/") && !PLAIN_API_PATHS.contains(&path))
}

/// The request's origin, when the configured domains allow it. An absent or an
/// empty origin, and a refused one, get no CORS headers.
fn allowed_origin(domains: &[String], headers: &HeaderMap) -> Option<String> {
    let origin = header_text(headers, header::ORIGIN).filter(|origin| !origin.is_empty())?;
    allows(domains, origin).then(|| origin.to_owned())
}

/// go-restful's `isOriginAllowed`: an empty list allows every origin, the entry
/// `.*` allows every origin, and any other entry must equal the origin, ignoring
/// case.
fn allows(domains: &[String], origin: &str) -> bool {
    if domains.is_empty() {
        return true;
    }
    let origin = origin.to_lowercase();
    domains
        .iter()
        .any(|domain| domain == ".*" || domain.to_lowercase() == origin)
}

/// go-restful's preflight answer. The methods and headers the request names are
/// checked against the config, and the answer lists the configured methods and
/// echoes the requested headers. A request the config refuses gets an empty 200
/// with no CORS headers, as go-restful gives it.
fn preflight(methods: &[String], origin: &str, headers: &HeaderMap) -> Response {
    let mut response = Response::new(Body::empty());
    let requested_method = header_text(headers, ACCESS_CONTROL_REQUEST_METHOD).unwrap_or_default();
    if !methods.iter().any(|method| method == requested_method) {
        return response;
    }
    let requested_headers =
        header_text(headers, ACCESS_CONTROL_REQUEST_HEADERS).unwrap_or_default();
    if !requested_headers.is_empty()
        && !requested_headers
            .split(',')
            .all(|name| allowed_header(name.trim_matches(' ')))
    {
        return response;
    }
    let response_headers = response.headers_mut();
    insert(
        response_headers,
        header::ACCESS_CONTROL_ALLOW_METHODS,
        &methods.join(","),
    );
    insert(
        response_headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        requested_headers,
    );
    insert(
        response_headers,
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        origin,
    );
    response
}

/// Whether the config allows a request header, ignoring case, as go-restful's
/// `isValidAccessControlRequestHeader` checks it.
fn allowed_header(name: &str) -> bool {
    ALLOWED_HEADERS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// Adds the origin to a route's response, as go-restful's filter does before
/// the route runs. A response that carries `*` from a Go helper that sets it
/// (an error, or `Text`) keeps only that `*`. A JSON response, which is Go's
/// `Ok` adding its `*`, keeps the origin before it.
fn merge_origin(mut response: Response, origin: &str) -> Response {
    let Ok(origin) = HeaderValue::from_str(origin) else {
        return response;
    };
    let is_ok = response.status() == StatusCode::OK && is_json(&response);
    let headers = response.headers_mut();
    if !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    } else if is_ok {
        let mut values = vec![origin];
        values.extend(
            headers
                .get_all(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .iter()
                .cloned(),
        );
        headers.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
        for value in values {
            headers.append(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        }
    }
    response
}

/// Whether a response is JSON, which is what Go's `Ok` writes.
fn is_json(response: &Response) -> bool {
    header_text(response.headers(), header::CONTENT_TYPE)
        .is_some_and(|content_type| content_type.starts_with("application/json"))
}

/// The text go-restful writes when no route takes a request: a 404 or a 405
/// with no body of its own. Go's status text is the body, as plain text.
fn restful_errors(response: Response) -> Response {
    let message = match response.status() {
        StatusCode::NOT_FOUND => "404: Page Not Found",
        StatusCode::METHOD_NOT_ALLOWED => "405: Method Not Allowed",
        _ => return response,
    };
    if response.headers().contains_key(header::CONTENT_TYPE) {
        return response;
    }
    let status = response.status();
    let mut written = (status, message).into_response();
    for (name, value) in response.headers() {
        // The length is the new body's, not the empty one's.
        if name != header::CONTENT_LENGTH {
            written.headers_mut().append(name.clone(), value.clone());
        }
    }
    written
}

/// The value of a header, when it is present and ASCII.
fn header_text(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

/// Inserts a header, skipping a value that is not a valid header value.
fn insert(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains(list: &[&str]) -> Vec<String> {
        list.iter().map(|domain| (*domain).to_owned()).collect()
    }

    /// A response with the given status and content type, and the `*` a Go
    /// helper sets when `acao` is given.
    fn response(status: StatusCode, content_type: &str, acao: Option<&str>) -> Response {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = status;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).unwrap(),
        );
        if let Some(acao) = acao {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(acao).unwrap(),
            );
        }
        response
    }

    /// Every `Access-Control-Allow-Origin` value, in the order sent.
    fn allow_origin(response: &Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn an_empty_list_allows_every_origin() {
        assert!(allows(&[], "https://any.example"));
    }

    #[test]
    fn a_listed_origin_is_allowed_and_another_is_refused() {
        let list = domains(&["https://allowed.example"]);
        assert!(allows(&list, "https://allowed.example"));
        assert!(allows(&list, "HTTPS://ALLOWED.EXAMPLE"));
        assert!(!allows(&list, "https://denied.example"));
    }

    #[test]
    fn the_wildcard_entry_allows_every_origin() {
        assert!(allows(&domains(&[".*"]), "https://any.example"));
    }

    #[test]
    fn an_entry_is_compared_as_text_not_as_a_pattern() {
        // go-restful compares the entries exactly; `.*` is its only wildcard.
        assert!(!allows(
            &domains(&["https://allowed"]),
            "https://allowed.example"
        ));
    }

    #[test]
    fn a_path_matches_a_template_segment_by_segment() {
        assert!(matches_template("/api/user/{user-id}", "/api/user/alice"));
        assert!(matches_template("/api/user/{user-id}", "/api/user/alice/"));
        assert!(!matches_template("/api/user/{user-id}", "/api/user/"));
        assert!(!matches_template("/api/user/{user-id}", "/api/user/a/b"));
    }

    #[test]
    fn route_methods_are_listed_in_declaration_order() {
        assert_eq!(route_methods("/api/items"), ["GET", "POST"]);
        assert!(route_methods("/api/nothing").is_empty());
    }

    #[test]
    fn a_configured_method_list_is_used_as_it_is() {
        let configured = domains(&["PUT"]);
        assert_eq!(allowed_methods(&configured, "/api/items"), ["PUT"]);
        assert_eq!(allowed_methods(&[], "/api/items"), ["GET", "POST"]);
    }

    #[test]
    fn go_ok_keeps_the_origin_before_its_wildcard() {
        let merged = merge_origin(
            response(StatusCode::OK, "application/json", Some("*")),
            "https://allowed.example",
        );
        assert_eq!(allow_origin(&merged), ["https://allowed.example", "*"]);
    }

    #[test]
    fn go_error_and_text_replace_the_origin_with_their_wildcard() {
        let error = merge_origin(
            response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Some("*"),
            ),
            "https://allowed.example",
        );
        assert_eq!(allow_origin(&error), ["*"]);
        let text = merge_origin(
            response(StatusCode::OK, "text/plain; charset=utf-8", Some("*")),
            "https://allowed.example",
        );
        assert_eq!(allow_origin(&text), ["*"]);
    }

    #[test]
    fn a_response_without_a_wildcard_gets_the_origin() {
        let merged = merge_origin(
            response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", None),
            "https://allowed.example",
        );
        assert_eq!(allow_origin(&merged), ["https://allowed.example"]);
    }

    #[test]
    fn a_route_the_path_does_not_take_gets_go_s_text() {
        let mut bare = Response::new(Body::empty());
        *bare.status_mut() = StatusCode::NOT_FOUND;
        let answer = restful_errors(bare);
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            answer.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
    }
}
