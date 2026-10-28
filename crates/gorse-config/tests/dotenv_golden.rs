// Ports: the golden data of contracts/tools/dotenvgolden (github.com/subosito/gotenv v1.6.0 StrictParse through spf13/viper v1.21.0's dotenv codec, golang.org/x/text v0.36.0, go1.27.0) @ add21b5

//! Replays every case `contracts/tools/dotenvgolden` recorded from Go.
//!
//! `strict_parse` must give Go's bytes exactly for every case. `decode` must
//! give the same map or error text, except that Rust strings cannot hold
//! bytes that are not UTF-8: for the cases listed under X-40 below,
//! and only those, it gives the text Go's `for range` reads (each byte that
//! does not start a valid sequence becomes U+FFFD).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gorse_config::formats::dotenv::{decode, strict_parse};
use gorse_config::value::Value;
use serde_json::Value as Json;

/// A Go string from the golden: a JSON string, `{"text", "hex"}` or
/// `{"len", "rle"}`.
fn go_bytes(j: &Json) -> Vec<u8> {
    match j {
        Json::String(s) => s.as_bytes().to_vec(),
        Json::Object(o) => {
            if let Some(h) = o.get("hex") {
                return hex::decode(h.as_str().expect("hex is a string")).expect("hex decodes");
            }
            let mut out = Vec::new();
            for run in o["rle"].as_array().expect("rle is an array") {
                let piece =
                    hex::decode(run[0].as_str().expect("a hex piece")).expect("hex decodes");
                for _ in 0..run[1].as_u64().expect("a count") {
                    out.extend_from_slice(&piece);
                }
            }
            assert_eq!(Some(out.len() as u64), o["len"].as_u64());
            out
        }
        other => panic!("not a Go string: {other}"),
    }
}

/// An environment of the golden: variables set, and the ones unset (every
/// other variable is unset too: the tool clears the environment).
struct Environment {
    set: HashMap<String, String>,
}

/// What Go gave: the variables sorted by key, or the error text.
type Outcome = Result<Vec<(Vec<u8>, Vec<u8>)>, Vec<u8>>;

/// A golden case.
struct Case {
    name: String,
    input: Vec<u8>,
    env: String,
    /// The variables sorted by key, or the error text.
    want: Outcome,
}

fn load() -> (HashMap<String, Environment>, Vec<Case>) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/dotenv_golden.json");
    let text = std::fs::read_to_string(path).expect("the golden file reads");
    let golden: Json = serde_json::from_str(&text).expect("the golden is JSON");
    assert_eq!(golden["go"], "go1.27.0");
    assert_eq!(golden["github.com/subosito/gotenv"], "v1.6.0");
    assert_eq!(golden["golang.org/x/text"], "v0.36.0");
    assert_eq!(golden["github.com/spf13/viper"], "v1.21.0");

    let mut envs = HashMap::new();
    for (name, e) in golden["environments"]
        .as_object()
        .expect("environments is an object")
    {
        let set: HashMap<String, String> = e
            .get("set")
            .and_then(Json::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().expect("a value").to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        for unset in e
            .get("unset")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            assert!(!set.contains_key(unset.as_str().expect("a name")));
        }
        envs.insert(name.clone(), Environment { set });
    }

    let cases = golden["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .map(|c| {
            let want = match (c.get("result"), c.get("error")) {
                (Some(r), None) => Ok(r
                    .as_array()
                    .expect("result is an array")
                    .iter()
                    .map(|e| (go_bytes(&e["key"]), go_bytes(&e["value"])))
                    .collect()),
                (None, Some(e)) => Err(go_bytes(e)),
                _ => panic!("case {c} has neither or both of result and error"),
            };
            Case {
                name: c["name"].as_str().expect("a name").to_owned(),
                input: go_bytes(&c["input"]),
                env: c["env"].as_str().expect("an environment").to_owned(),
                want,
            }
        })
        .collect();
    (envs, cases)
}

/// `os.LookupEnv` in a case's environment.
fn lookup<'a>(
    envs: &'a HashMap<String, Environment>,
    case: &Case,
) -> impl Fn(&str) -> Option<String> + 'a {
    let env = &envs[&case.env];
    move |name: &str| env.set.get(name).cloned()
}

/// Go's `for range` reading of a string, written independently of the
/// crate's: valid UTF-8 is kept, each byte of an invalid sequence becomes
/// U+FFFD.
fn go_text(b: &[u8]) -> String {
    let mut out = String::new();
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push('\u{fffd}');
        }
    }
    out
}

#[test]
fn strict_parse_gives_go_bytes_for_every_case() {
    let (envs, cases) = load();
    let mut failures = Vec::new();
    for case in &cases {
        let env = lookup(&envs, case);
        let (parsed, err) = strict_parse(&case.input, &env);
        let got = match err {
            Some(err) => Err(err),
            None => {
                let mut entries: Vec<(Vec<u8>, Vec<u8>)> = parsed
                    .iter()
                    .map(|(k, v)| (k.to_vec(), v.to_vec()))
                    .collect();
                entries.sort();
                Ok(entries)
            }
        };
        if got != case.want {
            failures.push(format!(
                "{}: input {:?}\n  got  {:?}\n  want {:?}",
                case.name,
                String::from_utf8_lossy(&case.input)
                    .chars()
                    .take(200)
                    .collect::<String>(),
                show(&got),
                show(&case.want),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn show(r: &Outcome) -> String {
    let cut = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        if s.chars().count() > 120 {
            format!(
                "{}... ({} bytes)",
                s.chars().take(120).collect::<String>(),
                b.len()
            )
        } else {
            format!("{s:?}")
        }
    };
    match r {
        Ok(entries) => entries
            .iter()
            .map(|(k, v)| format!("{}={}", cut(k), cut(v)))
            .collect::<Vec<_>>()
            .join(", "),
        Err(e) => format!("error {}", cut(e)),
    }
}

/// X-40: the cases whose Go result holds a value or error text that
/// is not valid UTF-8, outside the mutation corpus.
const X_40_CASES: &[&str] = &[
    "bom/utf8 partial bom",
    "bom/utf8 partial bom 1",
    "bom/not a bom fe",
    "bom/not a bom ff",
    "bom/ff ff",
    "bom/fe fe",
    "bom/fffe after newline",
    "invalid/value ff",
    "invalid/value quoted ff",
    "invalid/value single ff",
    "invalid/value truncated",
    "invalid/value truncated quoted",
    "invalid/value surrogate bytes",
    "invalid/value overlong",
    "invalid/value latin1",
    "invalid/value continuation",
    "invalid/value five bytes",
    "invalid/value f4 90",
    "invalid/value escape ff",
    "invalid/value escape truncated",
    "invalid/value ff space",
    "invalid/value ff nbsp",
    "invalid/value nbsp truncated",
    "invalid/value multi-line",
    "invalid/key ff",
    "invalid/key ff after",
    "invalid/line ff",
    "invalid/line ff spaces",
    "invalid/export ff",
    "invalid/export ff set",
    "invalid/env ff",
];

/// X-40: the same for the mutation corpus, by number
/// (`mutation/NNNN`).
const X_40_MUTATIONS: &[u16] = &[
    10, 15, 16, 20, 51, 59, 71, 86, 88, 106, 115, 119, 126, 128, 129, 132, 135, 141, 152, 161, 169,
    171, 178, 190, 191, 220, 232, 247, 254, 255, 259, 270, 279, 283, 300, 307, 319, 322, 323, 325,
    334, 347, 349, 350, 351, 360, 364, 365, 379, 381, 393, 397, 410, 414, 423, 427, 428, 430, 434,
    458, 461, 471, 475, 482, 499, 505, 514, 517, 522, 523, 528, 540, 544, 549, 550, 555, 561, 567,
    580, 581, 584, 585, 588, 595, 596, 602, 604, 607, 614, 622, 624, 626, 631, 638, 648, 649, 650,
    660, 665, 666, 670, 690, 691, 692, 709, 710, 711, 714, 717, 723, 726, 727, 737, 738, 748, 751,
    760, 765, 768, 776, 786, 799, 810, 811, 815, 821, 833, 845, 847, 850, 861, 866, 871, 878, 882,
    886, 896, 927, 930, 932, 943, 949, 960, 964, 966, 976, 992, 1004, 1007, 1008, 1016, 1033, 1041,
    1056, 1071, 1078, 1086, 1092, 1095, 1097, 1099, 1104, 1105, 1113, 1115, 1118, 1128, 1131, 1157,
    1166, 1170, 1171, 1173, 1175, 1179, 1181, 1182, 1184, 1199, 1210, 1211, 1228, 1232, 1236, 1237,
    1244, 1247, 1250, 1254, 1262, 1263, 1265, 1267, 1278, 1285, 1289, 1297, 1300, 1311, 1316, 1327,
    1353, 1357, 1360, 1372, 1378, 1382, 1384, 1387, 1400, 1404, 1410, 1416, 1423, 1426, 1431, 1432,
    1434, 1435, 1436, 1452, 1477, 1488, 1491, 1503, 1504, 1507, 1508, 1509, 1517, 1533, 1540, 1553,
    1562, 1568, 1577, 1585, 1586, 1588, 1589, 1592, 1593, 1602, 1611, 1613, 1616, 1620, 1624, 1633,
    1644, 1653, 1665, 1674, 1680, 1700, 1705, 1715, 1721, 1727, 1748, 1752, 1765, 1767, 1778, 1787,
    1792, 1797, 1798, 1806, 1807, 1821, 1831, 1834, 1840, 1843, 1854, 1859, 1863, 1866, 1870, 1873,
    1874, 1885, 1894, 1895, 1914, 1915, 1918, 1923, 1929, 1933, 1939, 1949, 1953, 1968, 1971, 1974,
    1998, 2005, 2007, 2010, 2022, 2026, 2031, 2037, 2051, 2065, 2072, 2096, 2098, 2108, 2115, 2116,
    2125, 2127, 2135, 2150, 2167, 2176, 2186, 2187, 2188, 2190, 2195, 2196, 2197, 2211, 2235, 2249,
    2251, 2252, 2260, 2262, 2268, 2272, 2275, 2280, 2285, 2311, 2327, 2357, 2364, 2367, 2368, 2373,
    2375, 2387, 2391, 2393, 2404, 2415, 2417, 2426, 2430, 2432, 2471, 2473, 2478, 2479, 2490, 2495,
    2498, 2500, 2501, 2513, 2521, 2523, 2527, 2530, 2540, 2546, 2551, 2552, 2567, 2577, 2579, 2582,
    2588, 2589, 2593, 2603, 2614, 2624, 2626, 2638, 2657, 2660, 2671, 2676, 2692, 2693, 2699, 2701,
    2711, 2716, 2717, 2721, 2727, 2747, 2748, 2750, 2753, 2783, 2787, 2790, 2801, 2819, 2827, 2832,
    2834, 2835, 2840, 2845, 2846, 2851, 2852, 2854, 2857, 2864, 2865, 2866, 2873, 2874, 2885, 2915,
    2930, 2935, 2944, 2952, 2954, 2957, 2976, 2983, 2993, 3002, 3003, 3006, 3015, 3020, 3022, 3024,
    3034, 3038, 3039, 3074, 3090, 3091, 3095, 3107, 3108, 3120, 3128, 3133, 3137, 3141, 3142, 3152,
    3158, 3163, 3164, 3165, 3169, 3175, 3201, 3203, 3205, 3207, 3225, 3234, 3245, 3247, 3258, 3262,
    3300, 3301, 3308, 3310, 3318, 3320, 3323, 3327, 3358, 3361, 3364, 3371, 3373, 3383, 3400, 3402,
    3416, 3418, 3423, 3424, 3433, 3448, 3452, 3464, 3469, 3482, 3500, 3501, 3503, 3507, 3510, 3518,
    3529, 3548, 3552, 3554, 3555, 3556, 3568, 3578, 3579, 3586, 3601, 3602, 3604, 3605, 3606, 3608,
    3630, 3637, 3639, 3645, 3647, 3653, 3664, 3670, 3673, 3684, 3687, 3695, 3696, 3697, 3705, 3706,
    3707, 3710, 3721, 3723, 3735, 3739, 3752, 3756, 3760, 3775, 3777, 3785, 3786, 3789, 3791, 3796,
    3799, 3819, 3832, 3835, 3837, 3847, 3851, 3858, 3859, 3861, 3864, 3869, 3875, 3903, 3905, 3907,
    3909, 3911, 3933, 3937, 3938, 3943, 3945, 3953, 3959, 3979, 3995, 3996, 3998,
];

#[test]
fn decode_matches_go_except_listed_invalid_utf8() {
    let (envs, cases) = load();
    let listed: BTreeSet<String> = X_40_CASES
        .iter()
        .map(|s| (*s).to_owned())
        .chain(X_40_MUTATIONS.iter().map(|n| format!("mutation/{n:04}")))
        .collect();
    assert_eq!(listed.len(), X_40_CASES.len() + X_40_MUTATIONS.len());

    let mut not_utf8 = BTreeSet::new();
    let mut failures = Vec::new();
    for case in &cases {
        let env = lookup(&envs, case);
        let got = decode(&case.input, &env);
        let valid = match &case.want {
            Ok(entries) => entries
                .iter()
                .all(|(k, v)| std::str::from_utf8(k).is_ok() && std::str::from_utf8(v).is_ok()),
            Err(e) => std::str::from_utf8(e).is_ok(),
        };
        if !valid {
            not_utf8.insert(case.name.clone());
        }
        // Keys are ASCII, so converting them never merges two keys.
        let want: Result<BTreeMap<String, String>, String> = match &case.want {
            Ok(entries) => Ok(entries
                .iter()
                .map(|(k, v)| (go_text(k), go_text(v)))
                .collect()),
            Err(e) => Err(go_text(e)),
        };
        let got: Result<BTreeMap<String, String>, String> = got.map(|m| {
            m.into_iter()
                .map(|(k, v)| match &v {
                    Value::String(s) => (k, s.clone()),
                    other => panic!("{}: {k} is not a string: {other:?}", case.name),
                })
                .collect()
        });
        if got != want {
            failures.push(format!("{}: got {got:?}\n  want {want:?}", case.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The deviation applies to exactly the listed cases.
    let unlisted: Vec<_> = not_utf8.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&not_utf8).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "not UTF-8 but not listed: {unlisted:?}\nlisted but UTF-8: {stale:?}"
    );
}

/// The quirks `PORTING_NOTES.md` names, as Go recorded them:
/// each named case must be in the golden with this outcome, and the port
/// must give it too.
#[test]
fn quirks_recorded_by_go() {
    let (envs, cases) = load();
    let by_name: HashMap<&str, &Case> = cases.iter().map(|c| (c.name.as_str(), c)).collect();
    let x = |n: usize| "x".repeat(n);
    let expect_value = |name: &str, key: &str, value: &str| {
        let case = by_name[name];
        let want = case
            .want
            .as_ref()
            .unwrap_or_else(|e| panic!("{name}: Go failed: {}", String::from_utf8_lossy(e)));
        let got = want
            .iter()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v.as_slice());
        assert_eq!(got, Some(value.as_bytes()), "{name}: Go's {key}");
        let env = lookup(&envs, case);
        let (parsed, err) = strict_parse(&case.input, &env);
        assert_eq!(err, None, "{name}");
        assert_eq!(parsed.get(key.as_bytes()), Some(value.as_bytes()), "{name}");
    };
    let expect_error = |name: &str, text: &str| {
        let case = by_name[name];
        assert_eq!(
            case.want,
            Err(text.as_bytes().to_vec()),
            "{name}: Go's error"
        );
        let env = lookup(&envs, case);
        assert_eq!(
            strict_parse(&case.input, &env).1,
            Some(text.as_bytes().to_vec()),
            "{name}"
        );
    };

    // Q-126: a "\r" that is the last buffered byte ends the line; its
    // "\n" is an empty line, which a quoted value keeps.
    expect_value(
        "boundary/prefix 0 quoted crlf 4096+0",
        "A",
        &(x(4092) + "\n\ny"),
    );
    expect_value(
        "boundary/prefix 0 quoted crlf 4096-1",
        "A",
        &(x(4091) + "\ny"),
    );
    expect_value(
        "boundary/prefix 0 quoted crlf 4096+1",
        "A",
        &(x(4093) + "\ny"),
    );
    expect_value(
        "boundary/prefix 1 quoted crlf 8192+0",
        "A",
        &(x(8188) + "\n\ny"),
    );
    expect_value(
        "boundary/prefix 0 quoted crlf 65536+0",
        "A",
        &(x(65532) + "\n\ny"),
    );
    expect_value(
        "boundary/utf16le quoted crlf 2048+0",
        "A",
        &(x(2044) + "\n\ny"),
    );
    expect_value(
        "boundary/utf8 quoted crlf 4096+0",
        "A",
        &(x(4092) + "\n\ny"),
    );
    // Q-127: the index into the trimmed val[1:] is used on val.
    expect_value("branch/quote with space after opening", "A", r" x\");
    expect_error(
        "branch/quote with spaces after opening",
        "line `y\"` doesn't match format",
    );
    expect_error("branch/quote index quirk 2", "missing quotes");
    // Q-128: a backslash before the closing quote opens a multi-line
    // value, also in single quotes.
    expect_error("branch/escape in single", "missing quotes");
    expect_value("branch/single quote escaped at end", "A", "x\\'\ny");
    // Q-129: StrictParse rejects every `export NAME` line.
    expect_error("branch/export set", "line `export B` doesn't match format");
    expect_error(
        "branch/export unset",
        "line `export B` has an unset variable",
    );
    expect_error(
        "branch/export prefix word",
        "line `exporter B` has an unset variable",
    );
    expect_error("branch/export tab", "line `export\tB` doesn't match format");
    // Q-130: \n and \r are replaced before other escapes.
    expect_value("branch/escape backslash n", "A", "x\ny");
    expect_value("branch/escape t", "A", "xty");
    // Q-131: a brace on one side is part of the variable.
    expect_value("branch/var open brace", "A", "test");
    expect_value("branch/var close brace", "A", "test");
    expect_value("branch/var self", "A", "env-a2");
    // Q-132: a line too long inside a quoted value is "missing quotes".
    expect_error("long/continuation 65536", "missing quotes");
    expect_error("long/line 65536 lf", "bufio.Scanner: token too long");
    // Q-133: x/text reads two trail surrogates as one U+FFFD.
    expect_value("bom/utf16le low low", "A", "\u{fffd}x");
    expect_value("bom/utf16le high high", "A", "\u{fffd}\u{fffd}x");
    // Q-134: a "\r" ends a line inside quotes too.
    expect_value("branch/multi-line cr", "A", "x\ny\n");
    // Q-135: `:` needs white space after it, `=` does not.
    expect_error("branch/colon no space", "line `A:1` doesn't match format");
    expect_value("branch/equals before colon", "A", "\"x=y");
}
