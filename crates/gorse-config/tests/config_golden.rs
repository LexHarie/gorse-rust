// Checks config.LoadConfig, Config.Validate, Config's JSON and the recommender hashes against contracts/tools/configgolden

//! What the Go config package does with the corpus of `config_golden.json`,
//! written by `contracts/tools/configgolden`: every load case (files in each
//! format, environment variables, missing files, directories), with the
//! loaded config's `json.Marshal` form, its feedback type expressions and its
//! recommender hashes, or the error text; the validate cases (a JSON patch
//! over the default config, then `Validate`); the hash cases; and the default
//! config's JSON.
//!
//! Load cases whose file is YAML are run by `load_yaml_cases`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gorse_common::expression::{ExprType, FeedbackTypeExpression};
use gorse_config::config::{
    Config, LoadError, RecommendConfig, get_default_config, load_config_with_env, set_root_dir,
};
use gorse_config::viper::ReadError;
use serde_json::Value as Json;

const GOLDEN: &str = include_str!("data/config_golden.json");

/// One feedback type expression as the golden file writes it: the feedback
/// type, the expression type's number and the value's bits in hex.
type ExprRow = (String, i64, String);

/// What a case records, the fields the Go program writes for one call.
#[derive(Debug, PartialEq)]
struct Outcome {
    error: Option<String>,
    parse_error: bool,
    meta: Option<String>,
    meta_error: Option<String>,
    exprs: Option<BTreeMap<String, Vec<ExprRow>>>,
    hashes: Option<Hashes>,
}

#[derive(Debug, PartialEq)]
struct Hashes {
    list: Vec<String>,
    hash: String,
    recommenders: Vec<(String, String)>,
}

fn str_field(case: &Json, key: &str) -> Option<String> {
    case.get(key).and_then(Json::as_str).map(str::to_owned)
}

fn outcome_of(case: &Json) -> Outcome {
    let exprs = case.get("exprs").and_then(Json::as_object).map(|map| {
        map.iter()
            .map(|(name, list)| {
                let list = list
                    .as_array()
                    .expect("expression list")
                    .iter()
                    .map(|e| {
                        (
                            e["feedback_type"]
                                .as_str()
                                .expect("feedback type")
                                .to_owned(),
                            e["expr_type"].as_i64().expect("expr type"),
                            e["value"].as_str().expect("value").to_owned(),
                        )
                    })
                    .collect();
                (name.clone(), list)
            })
            .collect()
    });
    let hashes = case.get("hashes").and_then(|h| {
        Some(Hashes {
            list: h["list"]
                .as_array()?
                .iter()
                .map(|s| s.as_str().expect("name").to_owned())
                .collect(),
            hash: h["hash"].as_str()?.to_owned(),
            recommenders: h["recommenders"]
                .as_array()?
                .iter()
                .map(|r| {
                    (
                        r["name"].as_str().expect("name").to_owned(),
                        r["hash"].as_str().expect("hash").to_owned(),
                    )
                })
                .collect(),
        })
    });
    Outcome {
        error: str_field(case, "error"),
        parse_error: case
            .get("parse_error")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        meta: str_field(case, "meta"),
        meta_error: str_field(case, "meta_error"),
        exprs,
        hashes,
    }
}

/// Go's `ExprType` number for an expression's comparison.
fn expr_type_number(t: ExprType) -> i64 {
    match t {
        ExprType::None => 0,
        ExprType::Less => 1,
        ExprType::LessOrEqual => 2,
        ExprType::Greater => 3,
        ExprType::GreaterOrEqual => 4,
    }
}

/// `clean` in the Go program: the temporary directories become placeholders.
fn clean(s: &str, dir: &Path, home: &Path) -> String {
    s.replace(&*dir.to_string_lossy(), "${DIR}")
        .replace(&*home.to_string_lossy(), "${HOME}")
}

/// `hashes` in the Go program.
fn hashes(r: &RecommendConfig) -> Hashes {
    let mut recommenders = Vec::new();
    for c in r.non_personalized.iter().flatten() {
        recommenders.push((c.full_name(), c.hash()));
    }
    for c in r.item_to_item.iter().flatten() {
        recommenders.push((c.full_name(), c.hash(Some(r))));
    }
    for c in r.user_to_user.iter().flatten() {
        recommenders.push((c.full_name(), c.hash(Some(r))));
    }
    for c in r.external.iter().flatten() {
        recommenders.push((c.full_name(), c.hash()));
    }
    recommenders.push((r.collaborative.full_name(), r.collaborative.hash(r)));
    Hashes {
        list: r.list_recommenders(),
        hash: r.hash(),
        recommenders,
    }
}

/// The directories the corpus runs in: `home` is `$HOME` for every test, so
/// the root directory is set once; each test has its own `dir`, which holds
/// one directory per load case.
struct Sandbox {
    home: PathBuf,
    dir: PathBuf,
}

/// The `$HOME` of every test in this binary, created on first use.
fn shared_home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("gorse-config-golden-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let home = base.join("home");
        set_root_dir(home.join(".gorse").join("var").join("lib"));
        home
    })
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let home = shared_home().clone();
        let dir = home.parent().expect("base").join("cases").join(name);
        fs::create_dir_all(&dir).expect("create the case directory");
        Sandbox { home, dir }
    }
}

/// Runs one load case as the Go program does and records its outcome.
fn run_load_case(sandbox: &Sandbox, index: usize, case: &Json) -> Outcome {
    let case_dir = sandbox.dir.join(format!("case{index}"));
    fs::create_dir_all(&case_dir).expect("create the case directory");

    let mut path = String::new();
    if let Some(file) = str_field(case, "file") {
        let target = case_dir.join(&file);
        if case.get("dir").and_then(Json::as_bool).unwrap_or(false) {
            fs::create_dir(&target).expect("create a directory");
        } else if !case.get("missing").and_then(Json::as_bool).unwrap_or(false) {
            let bytes = match str_field(case, "text_hex") {
                Some(hex) => unhex(&hex),
                None => str_field(case, "text").unwrap_or_default().into_bytes(),
            };
            fs::write(&target, bytes).expect("write the case file");
        }
        path = target.to_string_lossy().into_owned();
    }

    // The Go harness sets HOME for the whole process before any case runs, so
    // the case's own variables are layered over it.
    let mut env: HashMap<String, String> = HashMap::from([(
        "HOME".to_owned(),
        sandbox.home.to_string_lossy().into_owned(),
    )]);
    if let Some(map) = case.get("env").and_then(Json::as_object) {
        for (k, v) in map {
            env.insert(k.clone(), v.as_str().expect("env value").to_owned());
        }
    }
    let lookup = |k: &str| env.get(k).cloned();

    match load_config_with_env(&path, &lookup) {
        Err(err) => Outcome {
            error: Some(clean(
                &err.to_string()
                    .replace(&*case_dir.to_string_lossy(), "${DIR}"),
                &sandbox.dir,
                &sandbox.home,
            )),
            parse_error: matches!(err, LoadError::Read(ReadError::Parse(_))),
            meta: None,
            meta_error: None,
            exprs: None,
            hashes: None,
        },
        Ok(config) => outcome_of_config(&config, sandbox),
    }
}

fn outcome_of_config(config: &Config, sandbox: &Sandbox) -> Outcome {
    let (meta, meta_error) = match config.marshal_json() {
        Ok(json) => (Some(clean(&json, &sandbox.dir, &sandbox.home)), None),
        Err(err) => (None, Some(err)),
    };
    let expr_list = |list: &Option<Vec<FeedbackTypeExpression>>| {
        list.iter()
            .flatten()
            .map(|e| {
                (
                    e.feedback_type.clone(),
                    expr_type_number(e.expr_type),
                    format!("{:016x}", e.value.to_bits()),
                )
            })
            .collect::<Vec<_>>()
    };
    let ds = &config.recommend.data_source;
    let exprs = BTreeMap::from([
        (
            "positive_feedback_types".to_owned(),
            expr_list(&ds.positive_feedback_types),
        ),
        (
            "negative_feedback_types".to_owned(),
            expr_list(&ds.negative_feedback_types),
        ),
        (
            "read_feedback_types".to_owned(),
            expr_list(&ds.read_feedback_types),
        ),
    ]);
    Outcome {
        error: None,
        parse_error: false,
        meta,
        meta_error,
        exprs: Some(exprs),
        hashes: Some(hashes(&config.recommend)),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn golden() -> Json {
    serde_json::from_str(GOLDEN).expect("parse the golden file")
}

/// Runs the load cases whose file has `yaml` as its extension (`yaml` for
/// `true`), or the others, and compares each outcome with the golden's.
///
/// A case the Go program ran 64 times may have several results. The Rust
/// port's result must be one of them, and [`pinned_alternatives`] pins the
/// one it picks.
fn check_load_cases(want_yaml: bool) -> Vec<String> {
    let golden = golden();
    let sandbox = Sandbox::new(if want_yaml { "yaml" } else { "load" });
    let mut failures = Vec::new();
    for (index, case) in golden["load"]
        .as_array()
        .expect("load cases")
        .iter()
        .enumerate()
    {
        let is_yaml =
            str_field(case, "file").is_some_and(|f| f.ends_with(".yaml") || f.ends_with(".yml"));
        if is_yaml != want_yaml {
            continue;
        }
        let name = case["name"].as_str().expect("name");
        let expected = outcome_of(case);
        let mut results = vec![expected];
        results.extend(
            case.get("alternatives")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .map(outcome_of),
        );
        let actual = run_load_case(&sandbox, index, case);
        if !results.contains(&actual) {
            failures.push(format!(
                "{name}:\n  want {:?}\n  got  {actual:?}",
                results[0]
            ));
        }
    }
    failures
}

#[test]
fn load_cases_match_go() {
    let failures = check_load_cases(false);
    assert!(
        failures.is_empty(),
        "{} load cases differ from Go:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn load_yaml_cases() {
    let failures = check_load_cases(true);
    assert!(
        failures.is_empty(),
        "{} YAML load cases differ from Go:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn validate_cases_match_go() {
    let golden = golden();
    let mut failures = Vec::new();
    let _sandbox = Sandbox::new("validate");
    for case in golden["validate"].as_array().expect("validate cases") {
        let name = case["name"].as_str().expect("name");
        let mut config = get_default_config();
        if case["base"].as_str() == Some("suite") {
            config.database.cache_store = "redis://localhost:6379/0".into();
            config.database.data_store =
                "mysql://gorse:gorse_pass@tcp(localhost:3306)/gorse".into();
        }
        let patch = case["patch"].as_str().expect("patch");
        config
            .unmarshal_json(patch)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        let got = config.validate().err().map(|e| e.to_string());
        let want = str_field(case, "error");
        if got != want {
            failures.push(format!("{name}: want {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn hash_cases_match_go() {
    let golden = golden();
    let mut failures = Vec::new();
    for case in golden["hash"].as_array().expect("hash cases") {
        let name = case["name"].as_str().expect("name");
        let mut recommend = RecommendConfig::default();
        recommend
            .unmarshal_json(case["recommend"].as_str().expect("recommend"))
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        let got = hashes(&recommend);
        let want = Hashes {
            list: case["list"]
                .as_array()
                .expect("list")
                .iter()
                .map(|s| s.as_str().expect("name").to_owned())
                .collect(),
            hash: case["hash"].as_str().expect("hash").to_owned(),
            recommenders: case["recommenders"]
                .as_array()
                .expect("recommenders")
                .iter()
                .map(|r| {
                    (
                        r["name"].as_str().expect("name").to_owned(),
                        r["hash"].as_str().expect("hash").to_owned(),
                    )
                })
                .collect(),
        };
        if got != want {
            failures.push(format!("{name}: want {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn default_config_matches_go() {
    let sandbox = Sandbox::new("default");
    let want = golden()["default"].as_str().expect("default").to_owned();
    let got = clean(
        &get_default_config()
            .marshal_json()
            .expect("marshal the default config"),
        &sandbox.dir,
        &sandbox.home,
    );
    assert_eq!(got, want);
}

/// The two `Meta.config` contracts the generator writes from the defaults and
/// from the shipped `config.toml` (contracts/tools/configschema). This is the
/// WP-1-15 acceptance check that the port produces them.
#[test]
fn meta_config_matches_contracts() {
    let sandbox = Sandbox::new("meta-config");
    let rendered = |config: &Config| -> Json {
        let json = config.marshal_json().expect("marshal the config");
        serde_json::from_str(&clean(&json, &sandbox.dir, &sandbox.home)).expect("JSON")
    };
    let contract = |name: &str| -> Json {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/config")
            .join(name);
        let text = fs::read_to_string(path).expect("read the contract");
        serde_json::from_str(&text).expect("contract JSON")
    };

    assert_eq!(
        rendered(&get_default_config()),
        contract("meta-config.default.json")
    );

    let home = sandbox.home.to_string_lossy().into_owned();
    let lookup = |k: &str| (k == "HOME").then(|| home.clone());
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/config.toml");
    let sample = load_config_with_env(path, &lookup).expect("load the shipped config.toml");
    assert_eq!(rendered(&sample), contract("meta-config.sample.json"));
}

/// Where Go's output depends on its random map order, the port takes the
/// second recorded outcome every time. The choice and its reason are Q-145
/// and Q-146 in `PORTING_NOTES.md`.
#[test]
fn pinned_alternatives() {
    let golden = golden();
    let sandbox = Sandbox::new("pinned");
    for name in [
        "two upper-case spellings of a key",
        "variable under a file value of another type",
    ] {
        let (index, case) = golden["load"]
            .as_array()
            .expect("load cases")
            .iter()
            .enumerate()
            .find(|(_, c)| c["name"] == name)
            .expect("case");
        let actual = run_load_case(&sandbox, index, case);
        let chosen = &case["alternatives"].as_array().expect("alternatives")[0];
        assert_eq!(actual, outcome_of(chosen), "{name}");
    }
}
