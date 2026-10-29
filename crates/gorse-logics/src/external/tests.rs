// Ports: logics/external_test.go @ add21b5

use gorse_config::config::schema::ExternalConfig;
use rquickjs::Value;
use serde_json::Value as Json;

use super::External;
use crate::testing::{Reply, serve};

/// Evaluates `script` in the recommender's context and returns the result as
/// JSON text, as Go's `quickjs.Object.String()` does for an object.
fn eval_json(external: &External, script: &str) -> Result<String, String> {
    external.context.with(|ctx| {
        let value: Value = ctx.eval(script.as_bytes()).map_err(|err| err.to_string())?;
        ctx.json_stringify(&value)
            .map_err(|err| err.to_string())?
            .map(|text| text.to_string())
            .transpose()
            .map_err(|err| err.to_string())
            .map(|text| text.unwrap_or_default())
    })
}

#[test]
fn fetch() {
    let (url, requests) = serve(|_| Reply::ok("Hello, client\n"));
    let external = External::new(&ExternalConfig::default()).expect("the VM starts");

    let response = eval_json(&external, &format!(r#"fetch("{url}")"#));
    let request = requests.recv().expect("the server got a request");
    assert_eq!(request.method, "GET");
    let response: Json = serde_json::from_str(&response.expect("the fetch succeeds"))
        .expect("the response is an object");
    assert_eq!(response["ok"], true);
    assert_eq!(response["status"], 200.0);
    assert_eq!(response["statusText"], "200 OK");
    assert_eq!(response["body"], "Hello, client\n");
    let headers = response["headers"]
        .as_object()
        .expect("headers are an object");
    assert!(headers.contains_key("Content-Length"));
    assert!(headers.contains_key("Date"));

    eval_json(
        &external,
        &format!(r#"fetch({{method: "POST", url: "{url}"}})"#),
    )
    .expect("the fetch succeeds");
    let request = requests.recv().expect("the server got a request");
    assert_eq!(request.method, "POST");

    eval_json(
        &external,
        &format!(
            r#"fetch("{url}", {{
	method: "PUT",
	headers: {{
		"Content-Type": "application/json"
	}},
	body: JSON.stringify({{message: "Hello, server"}})
	}})"#
        ),
    )
    .expect("the fetch succeeds");
    let request = requests.recv().expect("the server got a request");
    assert_eq!(request.method, "PUT");
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    assert_eq!(request.body, br#"{"message":"Hello, server"}"#);
}

#[test]
fn external() {
    let (url, _requests) = serve(|request| {
        let user_id = request
            .target
            .split_once('?')
            .and_then(|(_, query)| {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("user_id="))
            })
            .unwrap_or_default();
        if user_id == "1" {
            Reply::ok("[\"item_1\", \"item_2\", \"item_3\"]\n")
        } else {
            Reply::not_found()
        }
    });
    let external = External::new(&ExternalConfig {
        script: format!(r#"fetch("{url}?user_id=1").body"#),
        name: "test".to_owned(),
    })
    .expect("the VM starts");

    let items = external.pull("1").expect("the pull succeeds");
    assert_eq!(
        items,
        Some(vec![
            "item_1".to_owned(),
            "item_2".to_owned(),
            "item_3".to_owned()
        ])
    );
}

#[test]
fn exception() {
    let external = External::new(&ExternalConfig {
        script: r#"throw new Error("test error")"#.to_owned(),
        name: "test".to_owned(),
    })
    .expect("the VM starts");

    assert!(external.pull("1").is_err());
}

#[test]
fn panic() {
    let external = External::new(&ExternalConfig {
        script: "fetch({}, {})".to_owned(),
        name: "test".to_owned(),
    })
    .expect("the VM starts");

    assert!(external.pull("1").is_err());
}

/// Go unmarshals a script's `null` into a nil slice without an error, so the
/// pull has no items, and the master answers `null`.
#[test]
fn null_result_has_no_items() {
    let external = External::new(&ExternalConfig {
        script: "'null'".to_owned(),
        name: "test".to_owned(),
    })
    .expect("the VM starts");

    assert_eq!(external.pull("1").expect("the pull succeeds"), None);
}

/// An empty list is a list with no items, which Go answers as `[]`.
#[test]
fn empty_list_result_has_no_items() {
    let external = External::new(&ExternalConfig {
        script: "'[]'".to_owned(),
        name: "test".to_owned(),
    })
    .expect("the VM starts");

    assert_eq!(
        external.pull("1").expect("the pull succeeds"),
        Some(Vec::new())
    );
}
