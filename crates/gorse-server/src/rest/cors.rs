// Ports: go-restful v3.13.0 `CrossOriginResourceSharing` (the container filter that `server/rest.go` installs on the containers of both nodes) and the `Ok`, `Text` and error helpers of `server/rest.go` @ add21b5

//! The CORS filter of the WebService routes (X-214, X-254).
//!
//! Go installs go-restful's CORS filter on the container of each node. A
//! container filter runs only for the WebService routes: the server routes under
//! `/api/` and `/apidocs.json`, and on the master also the dashboard routes under
//! `/api/dashboard/`. The plain handlers the container also registers (the
//! Swagger page, the pprof handlers, `/metrics`, and on the master the dashboard
//! page, the login, the bulk import and export, dump and restore, and the chat
//! proxy) never see it. [`Cors::is_web_service`] leaves them alone.
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
//!
//! Go copies `master.http_cors_domains` and `master.http_cors_methods` into the
//! filter when the HTTP server starts. The master's copy is taken once from its
//! config, [`Source::Fixed`]. The server's copy starts from Go's default lists,
//! because Go's `Sync` runs in a goroutine and the filter is set before the
//! master's config arrives. Each `Sync` then writes into those lists the way Go's
//! JSON decoder does ([`GoList`]), and the golden checks show the result.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use gorse_config::config::schema::Config;

use super::RestServer;

/// The request headers a preflight may name, as go-restful's filter allows them.
const ALLOWED_HEADERS: [&str; 3] = ["Content-Type", "Accept", "X-API-Key"];

/// The `/api/` paths the master's plain handlers take. Go registers each with
/// `ServeMux` as an exact pattern, so they are not WebService routes.
const MASTER_PLAIN_PATHS: [&str; 7] = [
    "/api/purge",
    "/api/bulk/users",
    "/api/bulk/items",
    "/api/bulk/feedback",
    "/api/dump",
    "/api/restore",
    "/api/chat/completions",
];

/// The OpenAPI document's route. Both nodes register it as a WebService, so the
/// filter covers it.
const OPENAPI_ROUTE: (&str, &str) = ("GET", "/apidocs.json");

/// The WebService routes of the server node, with their methods, in the order
/// Go declares them in `server/rest.go`. A `{name}` segment matches one path
/// segment. Go's `computeAllowedMethods` lists the methods of the routes a path
/// matches, in this order ([`route_methods`]). Generated from the `ws.GET`,
/// `ws.POST`, `ws.PUT`, `ws.PATCH` and `ws.DELETE` declarations @ add21b5, in
/// file order.
const SERVER_ROUTES: [(&str, &str); 42] = [
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
];

const DASHBOARD_ROUTES: [(&str, &str); 23] = [
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
];

const ACCESS_CONTROL_REQUEST_METHOD: HeaderName = header::ACCESS_CONTROL_REQUEST_METHOD;
const ACCESS_CONTROL_REQUEST_HEADERS: HeaderName = header::ACCESS_CONTROL_REQUEST_HEADERS;

/// go-restful's CORS filter, with the config its origins and methods come from.
pub struct Cors {
    /// Where the allowed origins and the configured methods are read from.
    source: Source,
    /// The WebService routes, with their methods, in Go's declaration order.
    routes: Vec<(&'static str, &'static str)>,
    /// The `/api/` paths of plain handlers, which are not WebService routes.
    plain_paths: &'static [&'static str],
}

/// Go's `GetDefaultConfig` lists (`config/config.go`), which the server's filter
/// holds until its `Sync` writes into them.
const DEFAULT_DOMAINS: [&str; 1] = [".*"];
const DEFAULT_METHODS: [&str; 5] = ["GET", "POST", "PUT", "DELETE", "PATCH"];

/// Where the filter reads its origins and methods from.
enum Source {
    /// The master's lists, which Go copies once when its HTTP server starts.
    Fixed(Arc<CorsCopy>),
    /// The server's lists, which Go's `Sync` writes into ([`CorsCopy`]).
    Synced(Arc<RestServer>),
}

/// go-restful's copy of one config list, and the writes Go's `Sync` makes into it.
///
/// `StartHttpServer` copies `s.Config.Master.HttpCorsDomains` into the filter, and
/// the filter keeps that slice. `Sync` then decodes the master's JSON into
/// `s.Config` with `json.Unmarshal`. The decoder writes each element into the
/// array the config's list has room for, and grows the list into a new array when
/// it has none left. So the filter sees the synced elements that land in its
/// array, and the start-time elements that no write reached. An empty list makes
/// the config a fresh slice that the filter does not share.
#[derive(Clone)]
struct GoList {
    /// The array the filter reads. Its length is the start-time length.
    array: Vec<String>,
    /// Whether the config's list still shares `array`.
    shared: bool,
    /// The capacity of the config's list. While it shares `array`, this is
    /// `array.len()`.
    capacity: usize,
}

impl GoList {
    /// The list `StartHttpServer` copies from the start config's values.
    fn start(values: &[&str]) -> GoList {
        GoList {
            array: values.iter().map(|value| (*value).to_owned()).collect(),
            shared: true,
            capacity: values.len(),
        }
    }

    /// The list of a config that is never synced: the master's copy.
    fn fixed(values: &[String]) -> GoList {
        GoList {
            array: values.to_vec(),
            shared: false,
            capacity: values.len(),
        }
    }

    /// Decodes `synced` into the config's list, as `json.Unmarshal` does.
    fn sync(&mut self, synced: &[String]) {
        if synced.is_empty() {
            self.shared = false;
            self.capacity = 0;
            return;
        }
        for (index, value) in synced.iter().enumerate() {
            if index >= self.capacity {
                // The decoder grows the list into a new array, and writes the rest
                // there, so the filter's array gets no more.
                self.capacity = (self.capacity + self.capacity / 2).max(4);
                self.shared = false;
            }
            if self.shared {
                self.array[index] = value.clone();
            }
        }
    }
}

/// The domains and methods of one node's CORS filter, as go-restful holds them.
#[derive(Clone)]
pub struct CorsCopy {
    domains: GoList,
    methods: GoList,
}

impl CorsCopy {
    /// The lists `StartHttpServer` copies from the server's start config, which is
    /// Go's `GetDefaultConfig` until `Sync` has loaded the master's.
    pub fn start() -> CorsCopy {
        CorsCopy {
            domains: GoList::start(&DEFAULT_DOMAINS),
            methods: GoList::start(&DEFAULT_METHODS),
        }
    }

    /// The lists of the master's config, which it copies once and never syncs.
    pub fn of(config: &Config) -> CorsCopy {
        CorsCopy {
            domains: GoList::fixed(
                config
                    .master
                    .http_cors_domains
                    .as_deref()
                    .unwrap_or_default(),
            ),
            methods: GoList::fixed(
                config
                    .master
                    .http_cors_methods
                    .as_deref()
                    .unwrap_or_default(),
            ),
        }
    }

    /// Applies the decode of the master's config that `Sync` makes.
    pub fn sync(&mut self, config: &Config) {
        self.domains.sync(
            config
                .master
                .http_cors_domains
                .as_deref()
                .unwrap_or_default(),
        );
        self.methods.sync(
            config
                .master
                .http_cors_methods
                .as_deref()
                .unwrap_or_default(),
        );
    }

    /// The allowed origins, as the filter reads them.
    fn domains(&self) -> &[String] {
        &self.domains.array
    }

    /// The configured methods, as the filter reads them.
    fn methods(&self) -> &[String] {
        &self.methods.array
    }
}

impl Cors {
    /// The filter of the server node: the server routes and the OpenAPI document.
    pub fn server(rest: Arc<RestServer>) -> Cors {
        let routes = SERVER_ROUTES
            .iter()
            .copied()
            .chain([OPENAPI_ROUTE])
            .collect();
        Cors::new(Source::Synced(rest), routes, &[])
    }

    /// The filter of the master node: the server routes, the dashboard routes and
    /// the OpenAPI document, and the paths of the master's plain handlers.
    pub fn master(config: &Config) -> Cors {
        let routes = SERVER_ROUTES
            .iter()
            .chain(&DASHBOARD_ROUTES)
            .copied()
            .chain([OPENAPI_ROUTE])
            .collect();
        let copy = Arc::new(CorsCopy::of(config));
        Cors::new(Source::Fixed(copy), routes, &MASTER_PLAIN_PATHS)
    }

    fn new(
        source: Source,
        routes: Vec<(&'static str, &'static str)>,
        plain_paths: &'static [&'static str],
    ) -> Cors {
        Cors {
            source,
            routes,
            plain_paths,
        }
    }

    /// The lists the filter reads on this request.
    fn copy(&self) -> Arc<CorsCopy> {
        match &self.source {
            Source::Fixed(copy) => Arc::clone(copy),
            Source::Synced(rest) => rest.cors_copy(),
        }
    }

    /// Whether the container filter sees `path`: a WebService route, which is an
    /// `/api/` path no plain handler takes, or the OpenAPI document.
    fn is_web_service(&self, path: &str) -> bool {
        path == OPENAPI_ROUTE.1 || (path.starts_with("/api/") && !self.plain_paths.contains(&path))
    }
}

/// Runs go-restful's CORS filter on a WebService route, and go-restful's own
/// not-found and not-allowed answers for it.
pub async fn filter(State(cors): State<Arc<Cors>>, request: Request, next: Next) -> Response {
    if !cors.is_web_service(request.uri().path()) {
        return next.run(request).await;
    }
    let copy = cors.copy();
    let Some(origin) = allowed_origin(copy.domains(), request.headers()) else {
        return restful_errors(next.run(request).await);
    };
    let is_preflight = request.method() == Method::OPTIONS
        && header_text(request.headers(), ACCESS_CONTROL_REQUEST_METHOD)
            .is_some_and(|method| !method.is_empty());
    if is_preflight {
        let methods = allowed_methods(copy.methods(), &cors.routes, request.uri().path());
        return preflight(&methods, &origin, request.headers());
    }
    merge_origin(restful_errors(next.run(request).await), &origin)
}

/// The methods a preflight lists: the configured ones, or, when the config lists
/// none, the methods of the routes the path matches (Go's `computeAllowedMethods`,
/// which the shipped config relies on).
fn allowed_methods(
    configured: &[String],
    routes: &[(&'static str, &'static str)],
    path: &str,
) -> Vec<String> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    route_methods(routes, path)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Go's `computeAllowedMethods`: the methods of the WebService routes that
/// `path` matches, in declaration order, duplicates kept.
fn route_methods(routes: &[(&'static str, &'static str)], path: &str) -> Vec<&'static str> {
    routes
        .iter()
        .filter(|(_, template)| matches_template(template, path))
        .map(|(method, _)| *method)
        .collect()
}

/// Whether a route template matches a request path. The segments must agree,
/// with a `{name}` segment matching any non-empty one. One trailing slash is
/// allowed on either side: go-restful drops the empty segment a slash leaves.
fn matches_template(template: &str, path: &str) -> bool {
    let template = template.strip_suffix('/').unwrap_or(template);
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

    /// The server node's routes, as [`Cors::server`] takes them.
    fn server_routes() -> Vec<(&'static str, &'static str)> {
        SERVER_ROUTES
            .iter()
            .copied()
            .chain([OPENAPI_ROUTE])
            .collect()
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
    fn a_template_with_a_trailing_slash_matches_without_it() {
        assert!(matches_template(
            "/api/item/{item-id}/feedback/",
            "/api/item/x/feedback"
        ));
    }

    #[test]
    fn route_methods_are_listed_in_declaration_order() {
        assert_eq!(
            route_methods(&server_routes(), "/api/items"),
            ["GET", "POST"]
        );
        assert!(route_methods(&server_routes(), "/api/nothing").is_empty());
    }

    #[test]
    fn the_server_node_takes_no_dashboard_route() {
        assert!(route_methods(&server_routes(), "/api/dashboard/cluster").is_empty());
        let master: Vec<_> = SERVER_ROUTES
            .iter()
            .chain(&DASHBOARD_ROUTES)
            .copied()
            .collect();
        assert_eq!(route_methods(&master, "/api/dashboard/cluster"), ["GET"]);
    }

    #[test]
    fn a_configured_method_list_is_used_as_it_is() {
        let configured = domains(&["PUT"]);
        assert_eq!(
            allowed_methods(&configured, &server_routes(), "/api/items"),
            ["PUT"]
        );
        assert_eq!(
            allowed_methods(&[], &server_routes(), "/api/items"),
            ["GET", "POST"]
        );
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

    #[test]
    fn the_start_lists_are_go_s_defaults() {
        let copy = CorsCopy::start();
        assert_eq!(copy.domains(), [".*"]);
        assert_eq!(copy.methods(), ["GET", "POST", "PUT", "DELETE", "PATCH"]);
    }

    #[test]
    fn a_synced_list_that_fits_is_seen_by_the_filter() {
        let mut copy = CorsCopy::start();
        copy.domains.sync(&domains(&["https://allowed.example"]));
        assert_eq!(copy.domains(), ["https://allowed.example"]);
        assert_eq!(copy.methods(), ["GET", "POST", "PUT", "DELETE", "PATCH"]);
    }

    #[test]
    fn an_empty_synced_list_leaves_the_start_lists_in_place() {
        let mut copy = CorsCopy::start();
        copy.domains.sync(&[]);
        copy.methods.sync(&[]);
        assert_eq!(copy.domains(), [".*"]);
        assert_eq!(copy.methods(), ["GET", "POST", "PUT", "DELETE", "PATCH"]);
    }

    #[test]
    fn a_shorter_synced_list_is_written_over_the_front_of_the_defaults() {
        let mut copy = CorsCopy::start();
        copy.methods.sync(&domains(&["HEAD", "OPTIONS"]));
        assert_eq!(
            copy.methods(),
            ["HEAD", "OPTIONS", "PUT", "DELETE", "PATCH"]
        );
    }

    #[test]
    fn a_longer_synced_list_writes_what_fits_and_then_detaches() {
        let mut copy = CorsCopy::start();
        copy.domains.sync(&domains(&["a", "b"]));
        assert_eq!(copy.domains(), ["a"]);
        copy.methods.sync(&domains(&["A", "B", "C", "D", "E", "F"]));
        assert_eq!(copy.methods(), ["A", "B", "C", "D", "E"]);
    }

    #[test]
    fn a_list_detached_by_an_empty_sync_keeps_what_the_filter_saw() {
        // Go's decoder, run on the same sequence: `[a] [] [b]` leaves `[a]`.
        let mut copy = CorsCopy::start();
        copy.domains.sync(&domains(&["a"]));
        copy.domains.sync(&[]);
        copy.domains.sync(&domains(&["b"]));
        assert_eq!(copy.domains(), ["a"]);
    }

    #[test]
    fn a_list_still_shared_follows_the_next_sync() {
        // Go's decoder: `[a] [b]` leaves `[b]`.
        let mut copy = CorsCopy::start();
        copy.domains.sync(&domains(&["a"]));
        copy.domains.sync(&domains(&["b"]));
        assert_eq!(copy.domains(), ["b"]);
    }

    #[test]
    fn an_empty_sync_before_a_list_leaves_the_defaults() {
        // Go's decoder: `[] [a]` leaves `[.*]`, because the list grows into a new array.
        let mut copy = CorsCopy::start();
        copy.domains.sync(&[]);
        copy.domains.sync(&domains(&["a"]));
        assert_eq!(copy.domains(), [".*"]);
    }
}
