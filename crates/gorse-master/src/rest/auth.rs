// Ports: master/rest.go (`login`, `logout`, `checkLogin`, `cookieHandler`, `handleOAuth2Callback`, the redirects of `dashboard`) @ add21b5

//! Login, logout and the session check (`master/rest.go`).
//!
//! A session is a cookie that holds the dashboard user name and password,
//! encrypted with a key generated when the process starts, so a session never
//! outlives a restart. The OIDC login stores the ID token in a second cookie and
//! keeps its claims in a token cache for an hour.

use std::sync::{Arc, LazyLock};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::Response;
use cookie::{Cookie, CookieJar, Key};
use gorse_server::rest::response::Query;
use serde::{Deserialize, Serialize};

use crate::Master;
use crate::dashboard;
use crate::rest::write_error;

/// The key the session and ID token cookies are encrypted with. Go's
/// `cookieHandler` draws its keys at random when the package starts.
static COOKIE_KEY: LazyLock<Key> = LazyLock::new(Key::generate);

/// The cookie that holds the dashboard session.
const SESSION: &str = "session";

/// The cookie that holds the OIDC ID token.
const ID_TOKEN: &str = "id_token";

/// The session's content: the dashboard user name and password (Go's
/// `cookieValue` map).
#[derive(Debug, Serialize, Deserialize)]
struct Session {
    user_name: String,
    password: String,
}

/// Encrypts `value` under `name`, as `securecookie.Encode` does.
fn encode_cookie(name: &str, value: &str) -> String {
    let mut jar = CookieJar::new();
    jar.private_mut(&COOKIE_KEY)
        .add(Cookie::new(name.to_owned(), value.to_owned()));
    jar.get(name)
        .map_or_else(String::new, |cookie| cookie.value().to_owned())
}

/// Decrypts the cookie `encoded` that [`encode_cookie`] wrote under `name`.
/// None when it does not decrypt, which is what `securecookie.Decode` reports.
pub(crate) fn decode_cookie(name: &str, encoded: &str) -> Option<String> {
    let mut jar = CookieJar::new();
    jar.add_original(Cookie::new(name.to_owned(), encoded.to_owned()));
    jar.private(&COOKIE_KEY)
        .get(name)
        .map(|cookie| cookie.value().to_owned())
}

/// The value of the cookie `name` in the request, or None.
pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| Cookie::parse(pair.trim().to_owned()).ok())
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
}

/// Go's `checkLogin`: whether the request may use the dashboard. The admin API
/// key logs a request in, and so does a valid session (or ID token with OIDC).
/// With no dashboard credentials and no OIDC, every request is logged in.
pub(crate) fn check_login(master: &Master, headers: &HeaderMap) -> bool {
    let cfg = master.config();
    if !cfg.master.admin_api_key.is_empty()
        && headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            == Some(cfg.master.admin_api_key.as_str())
    {
        return true;
    }
    if cfg.oidc.enable {
        return cookie_value(headers, ID_TOKEN)
            .and_then(|encoded| decode_cookie(ID_TOKEN, &encoded))
            .is_some_and(|token| master.oidc_token_is_known(&token));
    }
    if !cfg.master.dashboard_user_name.is_empty() || !cfg.master.dashboard_password.is_empty() {
        return cookie_value(headers, SESSION)
            .and_then(|encoded| decode_cookie(SESSION, &encoded))
            .and_then(|json| serde_json::from_str::<Session>(&json).ok())
            .is_some_and(|session| {
                session.user_name == cfg.master.dashboard_user_name
                    && session.password == cfg.master.dashboard_password
            });
    }
    true
}

/// Go's `http.Redirect`: a `Location` and, for `GET` and `HEAD`, the small HTML
/// body Go writes with it. The location is made absolute from the request path
/// when it is relative, which the callers' locations never are.
pub(crate) fn redirect(method: &Method, location: &str, status: StatusCode) -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = status;
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    if *method == Method::GET || *method == Method::HEAD {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        let text = format!(
            "<a href=\"{}\">{}</a>.\n\n",
            html_escape(location),
            status.canonical_reason().unwrap_or("")
        );
        *response.body_mut() = axum::body::Body::from(text);
    }
    response
}

/// The HTML escape `http.Redirect` applies to the location in its body.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

/// Go's redirect to the login page, or to the OIDC provider when OIDC is on
/// (the `dashboard` handler's branch for a request that is not logged in).
pub(crate) fn redirect_to_login(master: &Master, method: &Method) -> Response {
    if master.config().oidc.enable
        && let Some(url) = master.oidc_auth_code_url()
    {
        return redirect(method, &url, StatusCode::FOUND);
    }
    redirect(method, "/login", StatusCode::FOUND)
}

/// `GET /login`: the dashboard page.
pub(crate) async fn login_get() -> Response {
    dashboard::serve_file(&Method::GET, "/login")
}

/// The value of a form field, as Go's `FormValue` reads it: the first value
/// the body holds, then the first the query holds, and `""` when neither has it.
pub(crate) fn form_value(body: &Query, query: &Query, name: &str) -> String {
    let mut values = body.get_all(name);
    values.extend(query.get_all(name));
    values
        .first()
        .map_or_else(String::new, |value| (*value).to_owned())
}

/// `POST /login`: checks the credentials and sets the session cookie. Without
/// dashboard credentials every login succeeds and sets no cookie.
pub(crate) async fn login_post(
    State(master): State<Arc<Master>>,
    uri: Uri,
    body: Bytes,
) -> Response {
    let posted = Query::parse(std::str::from_utf8(&body).ok());
    let queried = Query::parse(uri.query());
    let name = form_value(&posted, &queried, "user_name");
    let pass = form_value(&posted, &queried, "password");
    let cfg = master.config();
    let dashboard_user = cfg.master.dashboard_user_name.clone();
    let dashboard_pass = cfg.master.dashboard_password.clone();
    if dashboard_user.is_empty() && dashboard_pass.is_empty() {
        return redirect(&Method::POST, "/", StatusCode::FOUND);
    }
    if name != dashboard_user || pass != dashboard_pass {
        return redirect(&Method::POST, "/login?msg=incorrect", StatusCode::FOUND);
    }
    let session = Session {
        user_name: name,
        password: pass,
    };
    let json = serde_json::to_string(&session).unwrap_or_default();
    let mut response = redirect(&Method::POST, "/", StatusCode::FOUND);
    let cookie = Cookie::build((SESSION, encode_cookie(SESSION, &json)))
        .path("/")
        .build();
    set_cookie(&mut response, &cookie);
    response
}

/// Appends a `Set-Cookie` header for `cookie`.
pub(crate) fn set_cookie(response: &mut Response, cookie: &Cookie<'_>) {
    if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// `ANY /logout`: clears the session cookie and redirects to the login page.
pub(crate) async fn logout(request: Request) -> Response {
    let mut response = redirect(request.method(), "/login", StatusCode::FOUND);
    let expired = Cookie::build((SESSION, ""))
        .path("/")
        .max_age(cookie::time::Duration::seconds(-1))
        .build();
    set_cookie(&mut response, &expired);
    response
}

/// `GET /callback/oauth2`: the OIDC code exchange. The ID token goes in a
/// cookie, and its claims in the token cache.
pub(crate) async fn oauth2_callback(State(master): State<Arc<Master>>, uri: Uri) -> Response {
    let code = Query::parse(uri.query()).get("code").to_owned();
    match master.oidc_exchange(&code).await {
        Ok(login) => {
            let mut response = redirect(&Method::GET, "/", StatusCode::FOUND);
            let expires = cookie::time::OffsetDateTime::from_unix_timestamp(login.expires_at);
            let cookie = Cookie::build((ID_TOKEN, encode_cookie(ID_TOKEN, &login.raw_id_token)))
                .path("/")
                .expires(expires.ok())
                .build();
            set_cookie(&mut response, &cookie);
            response
        }
        Err(err) => write_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

/// The `id_token` cookie's ID token, when the cookie is set and decodes.
pub(crate) fn id_token_cookie(headers: &HeaderMap) -> Option<String> {
    let encoded = cookie_value(headers, ID_TOKEN)?;
    decode_cookie(ID_TOKEN, &encoded)
}
