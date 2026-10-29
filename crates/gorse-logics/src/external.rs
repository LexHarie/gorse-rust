// Ports: logics/external.go @ add21b5

//! External recommenders: a script run in QuickJS returns the item ids a user
//! should be recommended. The script can call `fetch`, which sends an HTTP
//! request with the Fetch API's shape and answers with a response object.
//!
//! Go runs `modernc.org/quickjs`, which is QuickJS too (D-196 names
//! `rquickjs` 0.14.0 for it). A Go panic in `fetch` becomes an exception here,
//! which `pull` returns as an error, as Go's `recover` does.

use std::collections::HashMap;

use gorse_config::config::schema::ExternalConfig;
use rquickjs::function::Rest;
use rquickjs::{Context, Ctx, Exception, Function, Object, Runtime, Value};
use thiserror::Error;

/// The errors of [`External::pull`].
#[derive(Debug, Error)]
pub enum PullError {
    /// The script threw, or the QuickJS engine failed.
    #[error("{0}")]
    Script(String),
    /// The script returned a string or object that is not a JSON list of strings.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The script returned something other than a string or an object.
    #[error("script must return string or object")]
    NotStringOrObject,
}

/// A recommender whose script produces the items: `External` in Go.
pub struct External {
    // The runtime is kept with the context it made, as QuickJS needs it alive.
    _runtime: Runtime,
    context: Context,
    script: String,
    name: String,
}

impl External {
    /// Go's `NewExternal`: makes a QuickJS context with `fetch` registered.
    pub fn new(cfg: &ExternalConfig) -> Result<External, rquickjs::Error> {
        let runtime = Runtime::new()?;
        let context = Context::full(&runtime)?;
        let client = reqwest::blocking::Client::new();
        context.with(|ctx| register_fetch(ctx, client))?;
        Ok(External {
            _runtime: runtime,
            context,
            script: cfg.script.clone(),
            name: cfg.name.clone(),
        })
    }

    /// The recommender's name, from its config.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Go's `Pull`: sets `user_id` to `user_id`, runs the script and returns
    /// the items it gives, which must be a JSON list of strings, as text or as
    /// an object. The items are `None` when the script's list is `null`: Go
    /// unmarshals `null` into a nil slice, and the master answers that as `null`.
    pub fn pull(&self, user_id: &str) -> Result<Option<Vec<String>>, PullError> {
        self.context.with(|ctx| {
            ctx.globals()
                .set("user_id", user_id)
                .map_err(|err| script_error(&ctx, err))?;
            let result: Value = ctx
                .eval(self.script.as_bytes())
                .map_err(|err| script_error(&ctx, err))?;
            let text = if let Some(text) = result.as_string() {
                text.to_string().map_err(|err| script_error(&ctx, err))?
            } else if result.is_object() {
                ctx.json_stringify(&result)
                    .map_err(|err| script_error(&ctx, err))?
                    .map(|text| text.to_string())
                    .transpose()
                    .map_err(|err| script_error(&ctx, err))?
                    .unwrap_or_default()
            } else {
                return Err(PullError::NotStringOrObject);
            };
            // Go unmarshals `null` into a nil slice without an error.
            Ok(serde_json::from_str(&text)?)
        })
    }
}

/// Registers `fetch` as a global function of the context.
fn register_fetch<'js>(ctx: Ctx<'js>, client: reqwest::blocking::Client) -> rquickjs::Result<()> {
    let fetch = Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
        fetch(&ctx, &client, args.0)
    })?;
    ctx.globals().set("fetch", fetch)
}

/// The message of an error the script raised: the exception the engine holds,
/// or the error itself.
fn script_error(ctx: &Ctx<'_>, err: rquickjs::Error) -> PullError {
    if let rquickjs::Error::Exception = err {
        let exception = Exception::from_value(ctx.catch())
            .map(|exception| {
                let message = exception.message().unwrap_or_default();
                match exception.stack() {
                    Some(stack) if !stack.is_empty() => format!("{message}\n{stack}"),
                    _ => message,
                }
            })
            .unwrap_or_else(|_| "exception".to_owned());
        return PullError::Script(exception);
    }
    PullError::Script(err.to_string())
}

/// Go's `fetch`: one argument, a URL or a request object, or two, a URL and a
/// request object. Any other shape throws, as the Go panic does.
fn fetch<'js>(
    ctx: &Ctx<'js>,
    client: &reqwest::blocking::Client,
    args: Vec<Value<'js>>,
) -> rquickjs::Result<Object<'js>> {
    let (url, req) = match args.as_slice() {
        [first] => match first.as_string() {
            Some(url) => (url.to_string()?, None),
            None if first.is_object() => (String::new(), Some(first.clone())),
            None => {
                return Err(Exception::throw_type(
                    ctx,
                    "fetch requires first argument to be string or object",
                ));
            }
        },
        [first, second] => {
            let Some(url) = first.as_string() else {
                return Err(Exception::throw_type(
                    ctx,
                    "fetch requires first argument to be string",
                ));
            };
            if !second.is_object() {
                return Err(Exception::throw_type(
                    ctx,
                    "fetch requires second argument to be object",
                ));
            }
            (url.to_string()?, Some(second.clone()))
        }
        _ => {
            return Err(Exception::throw_type(
                ctx,
                "fetch requires 1 or 2 arguments",
            ));
        }
    };
    let request = parse_request(ctx, client, url, req)?;
    let response = match request.send() {
        Ok(response) => response,
        Err(err) => return Err(Exception::throw_type(ctx, &err.to_string())),
    };
    new_response(ctx, response)
}

/// Go's `parseRequest`: reads the method, headers, URL and body of a request
/// object. Its fields are optional, and each has the type Go asserts.
fn parse_request<'js>(
    ctx: &Ctx<'js>,
    client: &reqwest::blocking::Client,
    mut url: String,
    req: Option<Value<'js>>,
) -> rquickjs::Result<reqwest::blocking::RequestBuilder> {
    let mut method = String::from("GET");
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut body = String::new();
    if let Some(req) = req {
        let Some(req) = req.as_object() else {
            return Err(Exception::throw_type(ctx, "request is not an object"));
        };
        let value = req.get::<_, Value>("method")?;
        if !value.is_undefined() {
            method = expect_string(ctx, &value)?;
        }
        let value = req.get::<_, Value>("headers")?;
        if !value.is_undefined() && value.is_object() {
            let text = ctx
                .json_stringify(&value)?
                .map(|text| text.to_string())
                .transpose()?
                .unwrap_or_default();
            headers = serde_json::from_str::<Option<HashMap<String, String>>>(&text)
                .map_err(|err| Exception::throw_type(ctx, &err.to_string()))?
                .unwrap_or_default();
        }
        let value = req.get::<_, Value>("url")?;
        if !value.is_undefined() {
            url = expect_string(ctx, &value)?;
        }
        let value = req.get::<_, Value>("body")?;
        if !value.is_undefined() {
            body = expect_string(ctx, &value)?;
        }
    }
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|err| Exception::throw_type(ctx, &err.to_string()))?;
    let mut builder = client.request(method, url).body(body);
    for (key, value) in headers {
        builder = builder.header(key, value);
    }
    Ok(builder)
}

/// The value of a request field, which Go asserts to be a string.
fn expect_string(ctx: &Ctx<'_>, value: &Value<'_>) -> rquickjs::Result<String> {
    match value.as_string() {
        Some(text) => text.to_string(),
        None => Err(Exception::throw_type(ctx, "value is not a string")),
    }
}

/// Go's `newResponse`: the response object of the Fetch API, with `ok`,
/// `status`, `statusText`, `body` and `headers`.
fn new_response<'js>(
    ctx: &Ctx<'js>,
    response: reqwest::blocking::Response,
) -> rquickjs::Result<Object<'js>> {
    let status = response.status();
    let code = status.as_u16();
    let reason = status.canonical_reason().unwrap_or_default();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for (name, value) in response.headers() {
        let key = canonical_mime_header_key(name.as_str());
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match groups.iter_mut().find(|(known, _)| *known == key) {
            Some((_, values)) => values.push(value),
            None => groups.push((key, vec![value])),
        }
    }
    let body = response
        .bytes()
        .map_err(|err| Exception::throw_type(ctx, &err.to_string()))?;

    let object = Object::new(ctx.clone())?;
    object.set("ok", (200..300).contains(&code))?;
    object.set("status", i32::from(code))?;
    object.set("statusText", format!("{code} {reason}"))?;
    object.set("body", String::from_utf8_lossy(&body).into_owned())?;
    let headers = Object::new(ctx.clone())?;
    for (key, values) in groups {
        headers.set(key, values.join(", "))?;
    }
    object.set("headers", headers)?;
    Ok(object)
}

/// Go's `textproto.CanonicalMIMEHeaderKey`: upper case at the start of each
/// hyphen-separated word and lower case elsewhere. A key with a character a
/// header name cannot hold is returned as it is.
fn canonical_mime_header_key(key: &str) -> String {
    if !key.bytes().all(is_token_byte) {
        return key.to_owned();
    }
    let mut upper = true;
    key.bytes()
        .map(|byte| {
            let mapped = if upper {
                byte.to_ascii_uppercase()
            } else {
                byte.to_ascii_lowercase()
            };
            upper = byte == b'-';
            char::from(mapped)
        })
        .collect()
}

/// Whether `byte` may appear in a header field name (an RFC 7230 token).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[cfg(test)]
mod tests;
