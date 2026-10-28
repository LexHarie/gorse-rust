//! `gourl` against `tests/data/gourl_golden.json`, which
//! `contracts/tools/urlgolden` wrote by running Go's net/url: `Parse` and
//! `ParseRequestURI` with every URL field and the results of `String`,
//! `Redacted`, `RequestURI`, `EscapedPath`, `EscapedFragment`, `Hostname`,
//! `Port`, `IsAbs`, `Query` and `User.String`, or the error text; the four
//! escaping functions; `ParseQuery` and `Values.Encode`; the same URL methods
//! on URLs built field by field; `netip.ParseAddr`; and `Parse` and
//! `ParseQuery` under `GODEBUG` settings, which run in child processes with
//! `GODEBUG` set. Each case must match exactly.

mod common;

mod gourl {
    use std::collections::BTreeSet;
    use std::fmt;

    use gorse_common::gostrconv::quote;
    use gorse_common::gourl::{
        Url, Userinfo, Values, internal, parse, parse_query, parse_request_uri, path_escape,
        path_unescape, query_escape, query_unescape, user, user_password,
    };
    use serde::Deserialize;

    use crate::common::{is_child, load_golden, run_in_child};

    const GOLDEN: &str = "gourl_golden.json";

    /// A Go string: a JSON string when it is valid UTF-8, else its bytes in
    /// hex.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum GoString {
        Str(String),
        Hex { hex: String },
    }

    impl Default for GoString {
        fn default() -> Self {
            GoString::Str(String::new())
        }
    }

    impl GoString {
        fn bytes(&self) -> Vec<u8> {
            match self {
                GoString::Str(s) => s.as_bytes().to_vec(),
                GoString::Hex { hex } => hex::decode(hex).expect("hex"),
            }
        }
    }

    /// Bytes that print as Go's `%q`, so that a mismatch reads like Go.
    #[derive(PartialEq, Eq)]
    struct B(Vec<u8>);

    impl fmt::Debug for B {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&quote(&self.0))
        }
    }

    impl From<&GoString> for B {
        fn from(s: &GoString) -> Self {
            B(s.bytes())
        }
    }

    impl From<&[u8]> for B {
        fn from(s: &[u8]) -> Self {
            B(s.to_vec())
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Golden {
        go_version: String,
        parse: Vec<ParseCase>,
        escape: Vec<EscapeCase>,
        query: Vec<QueryCase>,
        methods: Vec<MethodsCase>,
        parse_addr: Vec<AddrCase>,
        godebug_parse: Vec<ParseCase>,
        query_limits: Vec<LimitCase>,
    }

    /// The `GODEBUG` sections, which the child processes read.
    #[derive(Deserialize)]
    struct GodebugGolden {
        godebug_parse: Vec<ParseCase>,
        query_limits: Vec<LimitCase>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct UserinfoFields {
        username: GoString,
        password: GoString,
        password_set: bool,
    }

    #[derive(Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct UrlFields {
        scheme: GoString,
        opaque: GoString,
        user: Option<UserinfoFields>,
        host: GoString,
        path: GoString,
        raw_path: GoString,
        force_query: bool,
        raw_query: GoString,
        fragment: GoString,
        raw_fragment: GoString,
        omit_host: bool,
    }

    impl UrlFields {
        fn url(&self) -> Url {
            Url {
                scheme: self.scheme.bytes(),
                opaque: self.opaque.bytes(),
                user: self.user.as_ref().map(|u| {
                    if u.password_set {
                        user_password(u.username.bytes(), u.password.bytes())
                    } else {
                        user(u.username.bytes())
                    }
                }),
                host: self.host.bytes(),
                path: self.path.bytes(),
                raw_path: self.raw_path.bytes(),
                force_query: self.force_query,
                raw_query: self.raw_query.bytes(),
                fragment: self.fragment.bytes(),
                raw_fragment: self.raw_fragment.bytes(),
                omit_host: self.omit_host,
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct KeyValues {
        key: GoString,
        values: Vec<GoString>,
    }

    #[derive(Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Methods {
        string: GoString,
        redacted: GoString,
        request_uri: GoString,
        escaped_path: GoString,
        escaped_fragment: GoString,
        hostname: GoString,
        port: GoString,
        is_abs: bool,
        query: Vec<KeyValues>,
        query_encode: GoString,
        user_string: GoString,
    }

    /// What the URL methods return, in a form that compares and prints.
    #[derive(Debug, PartialEq, Eq)]
    struct MethodResults {
        string: B,
        redacted: B,
        request_uri: B,
        escaped_path: B,
        escaped_fragment: B,
        hostname: B,
        port: B,
        is_abs: bool,
        query: Vec<(B, Vec<B>)>,
        query_encode: B,
        user_string: B,
    }

    impl From<&Methods> for MethodResults {
        fn from(m: &Methods) -> Self {
            MethodResults {
                string: (&m.string).into(),
                redacted: (&m.redacted).into(),
                request_uri: (&m.request_uri).into(),
                escaped_path: (&m.escaped_path).into(),
                escaped_fragment: (&m.escaped_fragment).into(),
                hostname: (&m.hostname).into(),
                port: (&m.port).into(),
                is_abs: m.is_abs,
                query: key_values(&m.query),
                query_encode: (&m.query_encode).into(),
                user_string: (&m.user_string).into(),
            }
        }
    }

    fn key_values(kvs: &[KeyValues]) -> Vec<(B, Vec<B>)> {
        kvs.iter()
            .map(|kv| (B::from(&kv.key), kv.values.iter().map(B::from).collect()))
            .collect()
    }

    /// The keys of v with their values, in key order (Go's `Encode` order).
    fn values_list(v: &Values) -> Vec<(B, Vec<B>)> {
        v.0.iter()
            .map(|(k, vs)| (B(k.clone()), vs.iter().map(|x| B(x.clone())).collect()))
            .collect()
    }

    fn method_results(u: &Url) -> MethodResults {
        let q = u.query();
        MethodResults {
            string: B(u.string()),
            redacted: B(u.redacted()),
            request_uri: B(u.request_uri()),
            escaped_path: B(u.escaped_path()),
            escaped_fragment: B(u.escaped_fragment()),
            hostname: u.hostname().into(),
            port: u.port().into(),
            is_abs: u.is_abs(),
            query: values_list(&q),
            query_encode: B(q.encode().into_bytes()),
            // Go's nil *Userinfo String returns "".
            user_string: B(u
                .user
                .as_ref()
                .map_or(String::new(), Userinfo::string)
                .into_bytes()),
        }
    }

    #[derive(Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Parsed {
        err: Option<String>,
        url: Option<UrlFields>,
        methods: Option<Methods>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParseCase {
        #[serde(default)]
        godebug: String,
        #[serde(rename = "in")]
        input: GoString,
        parse: Parsed,
        request: Parsed,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EscapeCase {
        #[serde(rename = "in")]
        input: GoString,
        query_escape: GoString,
        path_escape: GoString,
        query_unescape: GoString,
        query_unescape_err: String,
        path_unescape: GoString,
        path_unescape_err: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct QueryCase {
        #[serde(rename = "in")]
        input: GoString,
        values: Vec<KeyValues>,
        err: String,
        encode: GoString,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MethodsCase {
        url: UrlFields,
        methods: Methods,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AddrCase {
        #[serde(rename = "in")]
        input: GoString,
        err: String,
        is4: bool,
        zone: GoString,
        bytes: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LimitCase {
        godebug: String,
        params: usize,
        err: String,
        len: usize,
    }

    /// Collects mismatches, so a failing run reports many cases at once.
    #[derive(Default)]
    struct Failures {
        count: usize,
        shown: Vec<String>,
    }

    impl Failures {
        fn check(&mut self, ok: bool, msg: impl FnOnce() -> String) {
            if !ok {
                self.count += 1;
                if self.shown.len() < 20 {
                    self.shown.push(msg());
                }
            }
        }

        fn finish(self, what: &str, total: usize) {
            assert!(
                self.count == 0,
                "{what}: {} of {total} cases differ from Go:\n{}",
                self.count,
                self.shown.join("\n")
            );
        }
    }

    fn check_parsed(
        f: &mut Failures,
        what: &str,
        input: &[u8],
        got: Result<Url, gorse_common::gourl::Error>,
        want: &Parsed,
    ) {
        match (got, &want.err) {
            (Err(err), Some(want_err)) => {
                let err = err.to_string();
                f.check(&err == want_err, || {
                    format!(
                        "{what}({}) error = {err:?}, want {want_err:?}",
                        quote(input)
                    )
                });
            }
            (Err(err), None) => {
                f.check(false, || {
                    format!("{what}({}) error = {err}, want none", quote(input))
                });
            }
            (Ok(u), Some(want_err)) => {
                f.check(false, || {
                    format!("{what}({}) = {u:?}, want error {want_err:?}", quote(input))
                });
            }
            (Ok(u), None) => {
                let want_url = want.url.as_ref().expect("url").url();
                f.check(u == want_url, || {
                    format!("{what}({}) = {u:?}\n\twant {want_url:?}", quote(input))
                });
                let got_m = method_results(&u);
                let want_m = MethodResults::from(want.methods.as_ref().expect("methods"));
                f.check(got_m == want_m, || {
                    format!(
                        "{what}({}) methods = {got_m:?}\n\twant {want_m:?}",
                        quote(input)
                    )
                });
            }
        }
    }

    fn check_parse_cases<'a>(f: &mut Failures, cases: impl Iterator<Item = &'a ParseCase>) {
        for c in cases {
            let input = c.input.bytes();
            check_parsed(f, "Parse", &input, parse(&input), &c.parse);
            check_parsed(
                f,
                "ParseRequestURI",
                &input,
                parse_request_uri(&input),
                &c.request,
            );
        }
    }

    /// Runs the test in a child process without `GODEBUG` when it is set
    /// here, as the golden tool ran without it. Returns whether it did.
    fn without_godebug(name: &str) -> bool {
        if is_child() || std::env::var_os("GODEBUG").is_none() {
            return false;
        }
        run_in_child(name, &[("GODEBUG", None)]);
        true
    }

    #[test]
    fn test_golden_go_version() {
        let g: Golden = load_golden(GOLDEN);
        assert_eq!(g.go_version, "go1.27.0");
        // Every section has cases.
        for (name, len) in [
            ("parse", g.parse.len()),
            ("escape", g.escape.len()),
            ("query", g.query.len()),
            ("methods", g.methods.len()),
            ("parse_addr", g.parse_addr.len()),
            ("godebug_parse", g.godebug_parse.len()),
            ("query_limits", g.query_limits.len()),
        ] {
            assert!(len > 0, "{name} is empty");
        }
    }

    #[test]
    fn test_golden_parse() {
        if without_godebug("gourl::test_golden_parse") {
            return;
        }
        let g: Golden = load_golden(GOLDEN);
        let mut f = Failures::default();
        check_parse_cases(&mut f, g.parse.iter());
        f.finish("parse", g.parse.len());
    }

    #[test]
    fn test_golden_escape() {
        let g: Golden = load_golden(GOLDEN);
        let mut f = Failures::default();
        for c in &g.escape {
            let input = c.input.bytes();
            let got = query_escape(&input);
            let want = c.query_escape.bytes();
            f.check(got.as_bytes() == want, || {
                format!(
                    "QueryEscape({}) = {}, want {}",
                    quote(&input),
                    quote(&got),
                    quote(&want)
                )
            });
            let got = path_escape(&input);
            let want = c.path_escape.bytes();
            f.check(got.as_bytes() == want, || {
                format!(
                    "PathEscape({}) = {}, want {}",
                    quote(&input),
                    quote(&got),
                    quote(&want)
                )
            });
            for (name, got, want, want_err) in [
                (
                    "QueryUnescape",
                    query_unescape(&input),
                    &c.query_unescape,
                    &c.query_unescape_err,
                ),
                (
                    "PathUnescape",
                    path_unescape(&input),
                    &c.path_unescape,
                    &c.path_unescape_err,
                ),
            ] {
                // Go returns "" with the error.
                let (got, got_err) = match got {
                    Ok(v) => (v, String::new()),
                    Err(e) => (Vec::new(), e.to_string()),
                };
                let want = want.bytes();
                f.check(got == want && &got_err == want_err, || {
                    format!(
                        "{name}({}) = {}, {got_err:?}; want {}, {want_err:?}",
                        quote(&input),
                        quote(&got),
                        quote(&want)
                    )
                });
            }
        }
        f.finish("escape", g.escape.len());
    }

    #[test]
    fn test_golden_query() {
        if without_godebug("gourl::test_golden_query") {
            return;
        }
        let g: Golden = load_golden(GOLDEN);
        let mut f = Failures::default();
        for c in &g.query {
            let input = c.input.bytes();
            let (v, err) = parse_query(&input);
            let err = err.map_or(String::new(), |e| e.to_string());
            let got = values_list(&v);
            let want = key_values(&c.values);
            f.check(got == want && err == c.err, || {
                format!(
                    "ParseQuery({}) = {got:?}, {err:?}; want {want:?}, {:?}",
                    quote(&input),
                    c.err
                )
            });
            let got = v.encode();
            let want = c.encode.bytes();
            f.check(got.as_bytes() == want, || {
                format!(
                    "ParseQuery({}).Encode() = {}, want {}",
                    quote(&input),
                    quote(&got),
                    quote(&want)
                )
            });
        }
        f.finish("query", g.query.len());
    }

    #[test]
    fn test_golden_methods() {
        let g: Golden = load_golden(GOLDEN);
        let mut f = Failures::default();
        for c in &g.methods {
            let u = c.url.url();
            let got = method_results(&u);
            let want = MethodResults::from(&c.methods);
            f.check(got == want, || {
                format!("{u:?} methods = {got:?}\n\twant {want:?}")
            });
        }
        f.finish("methods", g.methods.len());
    }

    #[test]
    fn test_golden_parse_addr() {
        let g: Golden = load_golden(GOLDEN);
        let mut f = Failures::default();
        for c in &g.parse_addr {
            let input = c.input.bytes();
            match internal::parse_addr(&input) {
                Ok(a) => {
                    let (is4, zone, bytes) =
                        (a.is4(), B(a.zone().to_vec()), hex::encode(a.as_slice()));
                    let want_zone = B::from(&c.zone);
                    f.check(
                        c.err.is_empty() && is4 == c.is4 && zone == want_zone && bytes == c.bytes,
                        || {
                            format!(
                                "ParseAddr({}) = is4 {is4}, zone {zone:?}, {bytes}; want is4 {}, zone {want_zone:?}, {}, err {:?}",
                                quote(&input),
                                c.is4,
                                c.bytes,
                                c.err
                            )
                        },
                    );
                }
                Err(err) => {
                    let err = err.to_string();
                    f.check(err == c.err, || {
                        format!(
                            "ParseAddr({}) error = {err:?}, want {:?}",
                            quote(&input),
                            c.err
                        )
                    });
                }
            }
        }
        f.finish("parse_addr", g.parse_addr.len());
    }

    /// The `GODEBUG` cases: each setting runs in a child process with
    /// `GODEBUG` set to it, which checks the cases recorded under it.
    #[test]
    fn test_golden_godebug() {
        let g: GodebugGolden = load_golden(GOLDEN);
        if is_child() {
            let godebug = std::env::var("GODEBUG").expect("GODEBUG");
            let mut f = Failures::default();
            let cases: Vec<&ParseCase> = g
                .godebug_parse
                .iter()
                .filter(|c| c.godebug == godebug)
                .collect();
            check_parse_cases(&mut f, cases.iter().copied());
            let limits: Vec<&LimitCase> = g
                .query_limits
                .iter()
                .filter(|c| c.godebug == godebug)
                .collect();
            for c in &limits {
                let query = (0..c.params)
                    .map(|i| format!("p{i}"))
                    .collect::<Vec<_>>()
                    .join("&");
                let (v, err) = parse_query(&query);
                let err = err.map_or(String::new(), |e| e.to_string());
                f.check(err == c.err && v.len() == c.len, || {
                    format!(
                        "GODEBUG={godebug} ParseQuery({} params) = {} keys, {err:?}; want {}, {:?}",
                        c.params,
                        v.len(),
                        c.len,
                        c.err
                    )
                });
            }
            assert!(
                !cases.is_empty() || !limits.is_empty(),
                "no cases for GODEBUG={godebug}"
            );
            f.finish(&format!("GODEBUG={godebug}"), cases.len() + limits.len());
            return;
        }
        let settings: BTreeSet<&str> = g
            .godebug_parse
            .iter()
            .map(|c| c.godebug.as_str())
            .chain(g.query_limits.iter().map(|c| c.godebug.as_str()))
            .collect();
        for godebug in settings {
            run_in_child("gourl::test_golden_godebug", &[("GODEBUG", Some(godebug))]);
        }
    }
}
