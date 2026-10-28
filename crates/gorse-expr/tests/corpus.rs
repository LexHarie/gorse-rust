//! The expression corpus: what the checker, the compiler and the VM do with
//! each expression must be what expr-lang v1.17.8 does, as the golden file
//! records it. The golden file is written by contracts/tools/exprevalgolden
//! from the Go module; this test never writes it.
//!
//! For each case the compile verdict must match (the error text, or the root
//! type and kind). A case that compiles and is not volatile must then run to
//! the same result: the error text, or the type and the tagged result, encoded
//! as the oracle encodes it.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use gorse_common::gotime;
use gorse_expr::compile;
use gorse_expr::conf::Config;
use gorse_expr::nature::{self, MapFields, Nature};
use gorse_expr::ty::{Field, Kind, Ty};
use gorse_expr::value::{self, Value};
use gorse_expr::vm;
use serde_json::{Map, Value as Json};

/// `data.Item`, the item of the environments that have one.
fn item_ty() -> Ty {
    Ty::named_struct(
        "data.Item",
        vec![
            field("ItemId", Ty::string()),
            field("IsHidden", Ty::bool()),
            field("Categories", Ty::slice(Ty::string())),
            field("Timestamp", value::time_type()),
            field("UpdateAt", value::time_type()),
            field("Labels", Ty::any()),
            field("Comment", Ty::string()),
        ],
        Vec::new(),
    )
}

/// `data.FeedbackKey`, embedded in `data.Feedback`.
fn feedback_key_ty() -> Ty {
    Ty::named_struct(
        "data.FeedbackKey",
        vec![
            field("FeedbackType", Ty::string()),
            field("UserId", Ty::string()),
            field("ItemId", Ty::string()),
        ],
        Vec::new(),
    )
}

/// `data.Feedback`.
fn feedback_ty() -> Ty {
    Ty::named_struct(
        "data.Feedback",
        vec![
            Field {
                name: "FeedbackKey".to_owned(),
                ty: feedback_key_ty(),
                embedded: true,
            },
            field("Value", Ty::float64()),
            field("Timestamp", value::time_type()),
            field("Updated", value::time_type()),
            field("Labels", Ty::any()),
            field("Comment", Ty::string()),
        ],
        Vec::new(),
    )
}

/// `data.User`.
fn user_ty() -> Ty {
    Ty::named_struct(
        "data.User",
        vec![
            field("UserId", Ty::string()),
            field("Labels", Ty::any()),
            field("Comment", Ty::string()),
            field("UpdateAt", value::time_type()),
        ],
        Vec::new(),
    )
}

fn field(name: &str, ty: Ty) -> Field {
    Field {
        name: name.to_owned(),
        ty,
        embedded: false,
    }
}

/// The environment of a site: a `map[string]any` with the named entries.
fn env_nature(entries: Vec<(&str, Nature)>) -> Nature {
    let fields = entries
        .into_iter()
        .map(|(name, nt)| (name.to_owned(), nt))
        .collect();
    Nature {
        ty: Some(Ty::map(Ty::string(), Ty::any())),
        map: Some(Arc::new(MapFields { fields })),
        ..Nature::default()
    }
}

/// The configuration `expr.Compile` gets at each site of the corpus.
fn config_for(env: &str) -> Config {
    let mut config = Config::create_new();
    let entries = match env {
        "non_personalized" => vec![
            ("item", nature::from_type(&item_ty())),
            ("feedback", nature::from_type(&Ty::slice(feedback_ty()))),
        ],
        "item" => vec![("item", nature::from_type(&item_ty()))],
        "user" => vec![("user", nature::from_type(&user_ty()))],
        // expr.Compile without an Env option: nothing is strict.
        "none" => return config,
        other => panic!("unknown environment {other}"),
    };
    config.with_env(env_nature(entries));
    config
}

/// A JSON document as `encoding/json` decodes it into an `any`.
fn json_any(j: &Json) -> Value {
    match j {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Float(n.as_f64().expect("a finite number")),
        Json::String(s) => Value::str(s),
        Json::Array(items) => Value::any_array(items.iter().map(json_any).collect()),
        Json::Object(members) => Value::string_map(
            members
                .iter()
                .map(|(k, v)| (Value::str(k), json_any(v)))
                .collect(),
        ),
    }
}

fn json_time(j: &Json) -> Value {
    match j {
        Json::Null => Value::Time(gotime::Time::default()),
        Json::String(s) => {
            Value::Time(gotime::parse(gotime::RFC3339_NANO, s).expect("an RFC 3339 time"))
        }
        other => panic!("not a time: {other}"),
    }
}

fn json_str(j: &Json) -> Value {
    Value::str(j.as_str().unwrap_or(""))
}

fn json_bool(j: &Json) -> Value {
    Value::Bool(j.as_bool().unwrap_or(false))
}

fn json_float(j: &Json) -> Value {
    Value::Float(j.as_f64().unwrap_or(0.0))
}

/// The fixtures of the corpus, as the oracle decodes them into Gorse's types.
struct Fixtures {
    item: Value,
    feedback: Value,
    user: Value,
}

impl Fixtures {
    fn load() -> Self {
        let json = read_json("testdata/corpus/fixtures.json");
        let item = &json["item"];
        let item = Value::Struct(
            item_ty(),
            Arc::new(vec![
                json_str(&item["ItemId"]),
                json_bool(&item["IsHidden"]),
                Value::Array(
                    Ty::slice(Ty::string()),
                    Arc::new(
                        item["Categories"]
                            .as_array()
                            .map_or(Vec::new(), |a| a.iter().map(json_str).collect()),
                    ),
                ),
                json_time(&item["Timestamp"]),
                json_time(&item["UpdateAt"]),
                json_any(&item["Labels"]),
                json_str(&item["Comment"]),
            ]),
        );
        let feedback = json["feedback"]
            .as_array()
            .expect("feedback")
            .iter()
            .map(|f| {
                Value::Struct(
                    feedback_ty(),
                    Arc::new(vec![
                        Value::Struct(
                            feedback_key_ty(),
                            Arc::new(vec![
                                json_str(&f["FeedbackType"]),
                                json_str(&f["UserId"]),
                                json_str(&f["ItemId"]),
                            ]),
                        ),
                        json_float(&f["Value"]),
                        json_time(&f["Timestamp"]),
                        json_time(&f["Updated"]),
                        json_any(&f["Labels"]),
                        json_str(&f["Comment"]),
                    ]),
                )
            })
            .collect();
        let feedback = Value::Array(Ty::slice(feedback_ty()), Arc::new(feedback));
        let user = &json["user"];
        let user = Value::Struct(
            user_ty(),
            Arc::new(vec![
                json_str(&user["UserId"]),
                json_any(&user["Labels"]),
                json_str(&user["Comment"]),
                Value::Time(gotime::Time::default()),
            ]),
        );
        Fixtures {
            item,
            feedback,
            user,
        }
    }

    /// The value map a site passes to `expr.Run`.
    fn environment(&self, env: &str) -> Value {
        let entries = match env {
            "non_personalized" => vec![
                ("item", self.item.clone()),
                ("feedback", self.feedback.clone()),
            ],
            "item" => vec![("item", self.item.clone())],
            "user" => vec![("user", self.user.clone())],
            _ => Vec::new(),
        };
        Value::string_map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::str(k), v))
                .collect(),
        )
    }
}

/// `encode` of the oracle: a result as JSON, with its Go type made explicit.
fn encode(v: &Value) -> Json {
    fn tag(name: &str, value: Json) -> Json {
        let mut m = Map::new();
        m.insert(name.to_owned(), value);
        Json::Object(m)
    }
    match v {
        Value::Nil => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Str(s) => match std::str::from_utf8(s) {
            Ok(text) => Json::String(text.to_owned()),
            Err(_) => tag("bytes_hex", Json::String(hex::encode(s))),
        },
        Value::Bytes(b) => tag("bytes_hex", Json::String(hex::encode(b))),
        Value::Int(i) => tag("int", Json::String(i.to_string())),
        Value::Float(f) => tag("float", Json::String(format!("{:016x}", f.to_bits()))),
        Value::Time(t) => tag("time", Json::String(t.format(gotime::RFC3339_NANO))),
        Value::Duration(d) => tag("duration_ns", Json::String(d.0.to_string())),
        Value::Array(_, items) => tag("list", Json::Array(items.iter().map(encode).collect())),
        Value::Map(ty, entries) if ty.key().is_some_and(|k| k.kind() == Kind::String) => {
            let mut members = Map::new();
            for (key, value) in entries.iter() {
                let name = key
                    .as_str()
                    .map_or_else(String::new, |s| String::from_utf8_lossy(s).into_owned());
                members.insert(name, encode(value));
            }
            tag("map", Json::Object(members))
        }
        Value::Integer(ty, n) if ty.name() == "int64" => tag("int", Json::String(n.to_string())),
        other => tag("go", Json::String(other.type_name())),
    }
}

fn read_json(relative: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Checks one case; the problem found, if any.
fn check_case(case: &Json, fixtures: &Fixtures) -> Option<String> {
    let source = case["source"].as_str().expect("source");
    let env = case["env"].as_str().expect("env");
    let verdict = &case["compile"];
    let compiled = compile(source, &config_for(env));
    if verdict["ok"] == Json::Bool(false) {
        let want = verdict["error"].as_str().unwrap_or("");
        return match compiled {
            Err(err) if err.to_string() == want => None,
            Err(err) => Some(format!("compile error: want {want:?}, got {err}")),
            Ok(_) => Some(format!("compiled, but Go rejects it: {want:?}")),
        };
    }
    let program = match compiled {
        Ok(program) => program,
        Err(err) => {
            return Some(format!("rejected, but Go compiles it: {err}"));
        }
    };
    let want_type = verdict["type"].as_str().unwrap_or("");
    let want_kind = verdict["kind"].as_str().unwrap_or("");
    let got_type = program
        .root_type()
        .map_or_else(String::new, |t| t.name().to_owned());
    let got_kind = program.root().kind().name();
    if got_type != want_type || got_kind != want_kind {
        return Some(format!(
            "root type: want {want_type} ({want_kind}), got {got_type} ({got_kind})"
        ));
    }
    if case["volatile"] == Json::Bool(true) {
        return None;
    }
    let run = &case["run"];
    if run.is_null() {
        return None;
    }
    match vm::run(&program, &fixtures.environment(env)) {
        Err(err) => {
            let want = run["error"].as_str().unwrap_or("");
            (err.to_string() != want).then(|| format!("run error: want {want:?}, got {err}"))
        }
        Ok(value) => {
            let want_type = run["type"].as_str().unwrap_or("");
            if value.type_name() != want_type {
                return Some(format!(
                    "run type: want {want_type}, got {}",
                    value.type_name()
                ));
            }
            let want = run.get("result").cloned().unwrap_or(Json::Null);
            let got = encode(&value);
            let same = if reads_map_order(source) {
                same_up_to_map_order(&want, &got)
            } else {
                got == want
            };
            (!same).then(|| format!("run result: want {want}, got {got}"))
        }
    }
}

/// Whether the expression reads a map in the order Go iterates it: `keys`,
/// `values` and `toPairs` list a map's entries, and Go leaves that order
/// unspecified (O-WP36).
fn reads_map_order(source: &str) -> bool {
    let calls =
        regex::Regex::new(r"(^|[^A-Za-z0-9_.])(keys|values|toPairs)\(").expect("a valid pattern");
    calls.is_match(source)
}

/// Whether a result of an order-dependent expression agrees with Go's. Go's
/// answer is one sample of the orders a map can be read in: a list must hold
/// the same items in any order, and a bool that follows from the order is not
/// compared.
fn same_up_to_map_order(want: &Json, got: &Json) -> bool {
    match want.get("list") {
        Some(want) => got
            .get("list")
            .is_some_and(|got| sorted_items(want) == sorted_items(got)),
        None => true,
    }
}

/// The items of a list as JSON texts, in sorted order.
fn sorted_items(list: &Json) -> Vec<String> {
    let mut items: Vec<String> = list.as_array().map_or_else(Vec::new, |items| {
        items.iter().map(Json::to_string).collect()
    });
    items.sort();
    items
}

#[test]
fn golden_matches_go() {
    let golden = read_json("testdata/corpus/golden.json");
    let cases = golden.as_array().expect("the golden file is a list");
    let fixtures = Fixtures::load();
    let mut failures = Vec::new();
    for case in cases {
        if let Some(problem) = check_case(case, &fixtures) {
            failures.push(format!(
                "{} [{}] {}: {problem}",
                case["origin"].as_str().unwrap_or(""),
                case["env"].as_str().unwrap_or(""),
                case["source"].as_str().unwrap_or(""),
            ));
        }
    }
    if let Ok(path) = std::env::var("CORPUS_REPORT") {
        fs::write(path, failures.join("\n")).expect("write the report");
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Go:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
