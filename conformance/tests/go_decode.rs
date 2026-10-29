//! The request-body decoder against Go's `ReadEntity`. The Go probe in
//! `porting/evidence/wp-4-2/go-readentity-probe` calls go-restful's `ReadEntity`
//! on each body, with the Go type of the route the body is for. This runs the
//! probe on every body of its corpus and on the bodies built here, and it
//! compares each reply with the Rust decoder: the error text, or the value as
//! `json.Marshal` writes it. Nothing is read from a saved oracle, so the check
//! runs against the Go 1.27.0 toolchain each time. It fails, and does not skip,
//! when Go is missing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::{Compression, Crc};
use serde::Deserialize;

/// One body of the corpus, and the request it came with.
#[derive(Debug, Clone)]
struct Case {
    /// The Go type of the route: `user`, `users`, `user-patch`, `item`,
    /// `items`, `item-patch` or `feedback`.
    target: String,
    /// The `Content-Type` header, or empty when there is none.
    content_type: String,
    /// The `Content-Encoding` header, or empty when there is none.
    content_encoding: String,
    /// The body's bytes.
    body: Vec<u8>,
}

/// A row of the probe's corpus, as the probe reads it.
#[derive(Deserialize)]
struct CorpusRow {
    target: String,
    ct: String,
    #[serde(default)]
    ce: String,
    body: Option<String>,
    hex: Option<String>,
}

/// The probe's reply for one body.
#[derive(Deserialize)]
struct Reply {
    err: Option<String>,
    value: Option<String>,
}

fn probe_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../porting/evidence/wp-4-2/go-readentity-probe")
}

/// The decoded bytes of a hex string.
fn from_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("hex is ASCII");
            u8::from_str_radix(digits, 16).expect("hex digits")
        })
        .collect()
}

/// The corpus the probe was given when the decoder was written, which is every
/// row of the file, with none left out.
fn corpus_cases() -> Vec<Case> {
    let text = std::fs::read_to_string(probe_dir().join("corpus4.jsonl"))
        .expect("read the probe's corpus");
    text.lines()
        .map(|line| {
            let row: CorpusRow = serde_json::from_str(line).expect("a corpus row");
            let body = match (row.body, row.hex) {
                (Some(text), None) => text.into_bytes(),
                (None, Some(hex)) => from_hex(&hex),
                _ => panic!("a corpus row has a body or a hex body, not both"),
            };
            Case {
                target: row.target,
                content_type: if row.ct == "<none>" {
                    String::new()
                } else {
                    row.ct
                },
                content_encoding: row.ce,
                body,
            }
        })
        .collect()
}

/// The JSON bodies the generated cases are built from, one per route.
fn sample_bodies() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "user",
            r#"{"UserId":"u1","Labels":["a","b"],"Comment":"c"}"#,
        ),
        (
            "users",
            r#"[{"UserId":"u1"},{"UserId":"u2","Labels":[1,2.5]}]"#,
        ),
        ("user-patch", r#"{"Labels":["a"],"Comment":"x"}"#),
        (
            "item",
            r#"{"ItemId":"i1","IsHidden":true,"Categories":["c"],"Timestamp":"2020-01-01T00:00:00Z","Labels":["l"],"Comment":"x"}"#,
        ),
        (
            "items",
            r#"[{"ItemId":"i1"},{"ItemId":"i2","Categories":["a","b"]}]"#,
        ),
        (
            "item-patch",
            r#"{"IsHidden":false,"Categories":["x"],"Timestamp":"2021-02-03T04:05:06+02:00","Labels":[1,2],"Comment":"c"}"#,
        ),
        (
            "feedback",
            r#"[{"FeedbackType":"like","UserId":"u","ItemId":"i","Value":1.5,"Timestamp":"2020-01-01T00:00:00Z"}]"#,
        ),
    ]
}

/// A raw deflate stream of `data`, compressed with `level`.
fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(level));
    encoder.write_all(data).expect("compress");
    encoder.finish().expect("finish")
}

/// The CRC-32 of `data`, as a gzip trailer and header check write it.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

/// A gzip member of `data` with the header flags `flags` and the extra fields
/// the flags name, built by hand so that every flag is exercised.
fn gzip_member(data: &[u8], flags: u8, extra: &[u8]) -> Vec<u8> {
    let mut member = vec![0x1f, 0x8b, 8, flags, 0, 0, 0, 0, 0, 255];
    member.extend_from_slice(extra);
    let header_len = member.len();
    member.extend(deflate(data, 6));
    member.extend(crc32(data).to_le_bytes());
    member.extend((data.len() as u32).to_le_bytes());
    if flags & 2 != 0 {
        // The header check: the low 16 bits of the CRC of the header bytes.
        let check = crc32(&member[..header_len]) as u16;
        member.splice(header_len..header_len, check.to_le_bytes());
    }
    member
}

/// The generated cases: every route with its sample body, compressed in every
/// way go-restful can read, and every truncation and single-byte corruption of
/// those streams, which exercise the offsets Go reports.
fn generated_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |target: &str, content_type: &str, encoding: &str, body: Vec<u8>| {
        cases.push(Case {
            target: target.to_owned(),
            content_type: content_type.to_owned(),
            content_encoding: encoding.to_owned(),
            body,
        });
    };
    for (target, json) in sample_bodies() {
        let data = json.as_bytes();
        push(target, "application/json", "", data.to_vec());
        push(target, "application/json", "identity", data.to_vec());
        push(target, "application/json", "GZIP", data.to_vec());
        for level in [0, 1, 9] {
            push(target, "application/json", "deflate", zlib(data, level));
            push(target, "application/json", "gzip", gzip_stream(data, level));
        }
        push(
            target,
            "application/json",
            "gzip",
            gzip_member(data, 0, &[]),
        );
        push(
            target,
            "application/json",
            "gzip",
            gzip_member(data, 4, &[3, 0, 9, 9, 9]),
        );
        push(
            target,
            "application/json",
            "gzip",
            gzip_member(data, 8, b"name\0"),
        );
        push(
            target,
            "application/json",
            "gzip",
            gzip_member(data, 16, b"note\0"),
        );
        push(
            target,
            "application/json",
            "gzip",
            gzip_member(data, 2, &[]),
        );
        let two_members = [gzip_member(data, 0, &[]), gzip_member(data, 0, &[])].concat();
        push(target, "application/json", "gzip", two_members);
        push(target, "application/json", "gzip", Vec::new());
        push(target, "", "gzip", gzip_stream(data, 6));
        push(target, "text/xml", "gzip", gzip_stream(data, 6));

        // Every truncation and single-byte corruption of a gzip and a zlib stream.
        for stream in [gzip_stream(data, 6), zlib(data, 6)] {
            for end in 0..stream.len() {
                push(target, "application/json", "gzip", stream[..end].to_vec());
                push(
                    target,
                    "application/json",
                    "deflate",
                    stream[..end].to_vec(),
                );
            }
            for index in 0..stream.len() {
                for mask in [0x01, 0x80, 0xff] {
                    let mut corrupt = stream.clone();
                    corrupt[index] ^= mask;
                    push(target, "application/json", "gzip", corrupt.clone());
                    push(target, "application/json", "deflate", corrupt);
                }
            }
        }
    }
    cases
}

/// A gzip stream of `data`, with the default header.
fn gzip_stream(data: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(level));
    encoder.write_all(data).expect("compress");
    encoder.finish().expect("finish")
}

/// A zlib stream of `data`.
fn zlib(data: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(level));
    encoder.write_all(data).expect("compress");
    encoder.finish().expect("finish")
}

/// The cases the decoder's own tests named: labels past `float64`, names that
/// need escaping, the headers, and the zlib header and dictionary checks.
fn named_cases() -> Vec<Case> {
    let user = |body: &str| Case {
        target: "user".to_owned(),
        content_type: "application/json".to_owned(),
        content_encoding: String::new(),
        body: body.as_bytes().to_vec(),
    };
    let mut cases = vec![
        user(r#"{"Labels":[1e400]}"#),
        user(r#"{"Labels":[1E+2, 2.50, -0, 1e-400]}"#),
        user(r#"{"Labels":{"a":1e400}}"#),
        user(r#"{"Labels":[12345678901234567890123456789012345678901234567890]}"#),
        user(&format!(
            "{{\"Labels\":{}{}}}",
            "[".repeat(130),
            "]".repeat(130)
        )),
        user(&format!(
            "{{\"Labels\":{}{}}}",
            "[".repeat(9990),
            "]".repeat(9990)
        )),
        user(r#"{"Labels":[1,"a",true,null,{"k":[2]}]}"#),
        user(r#"{"Labels":{"b":{"c":[1.0]},"a":"<&>"}}"#),
    ];
    for code_point in [
        0x378, 0x200b, 0xe000, 0xa0, 0x1f600, 0x85, 0xfffd, 0x1ee00, 0xfffe,
    ] {
        let character = char::from_u32(code_point).expect("a code point");
        cases.push(user(&format!("{{{character}")));
    }
    // A zlib header with a dictionary: the check value must be the empty one's.
    let flags = (0..32u8)
        .find(|flags| (0x78u16 << 8 | u16::from(0x20 | flags)).is_multiple_of(31))
        .map_or(0x20, |flags| 0x20 | flags);
    let header = [0x78, 0x20 | flags];
    let stream = deflate(br#"{"UserId":"u"}"#, 6);
    for check in [[0, 0, 0, 1], [0, 0, 0, 2]] {
        let mut body = header.to_vec();
        body.extend(check);
        body.extend(&stream);
        cases.push(Case {
            target: "user".to_owned(),
            content_type: "application/json".to_owned(),
            content_encoding: "deflate".to_owned(),
            body,
        });
    }
    cases.push(Case {
        target: "user".to_owned(),
        content_type: "application/json".to_owned(),
        content_encoding: "deflate".to_owned(),
        body: vec![0x00, 0x00],
    });
    cases
}

/// Runs the probe on `cases`, one body per line, and returns its replies.
fn run_probe(cases: &[Case]) -> Vec<Reply> {
    let mut input = String::new();
    for case in cases {
        let mut row = serde_json::json!({
            "target": case.target,
            "ct": if case.content_type.is_empty() { "<none>".to_owned() } else { case.content_type.clone() },
            "ce": case.content_encoding,
        });
        match std::str::from_utf8(&case.body) {
            Ok(text) => row["body"] = text.into(),
            Err(_) => row["hex"] = hex_of(&case.body).into(),
        }
        input.push_str(&row.to_string());
        input.push('\n');
    }
    let mut child = Command::new("go")
        .args(["run", "."])
        .current_dir(probe_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run Go, which the test needs");
    let mut stdin = child.stdin.take().expect("the probe's stdin");
    let writer = std::thread::spawn(move || {
        stdin.write_all(input.as_bytes()).expect("write the corpus");
    });
    let output = child.wait_with_output().expect("the probe finishes");
    writer.join().expect("the writer finishes");
    assert!(
        output.status.success(),
        "the probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("the probe writes UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a probe reply"))
        .collect()
}

/// The hex of `bytes`.
fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Labels nested to Go's limit of 10000 are printed by recursive serialization,
/// which needs more than a test thread's default stack. The check therefore
/// runs on a thread with a large one. The server's own threads need the same
/// (see porting/notes/WP-4-2.md, open items).
/// The XML bodies that each route takes, one per route, as XML.
fn xml_sample_bodies() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "user",
            "<User><UserId>u1</UserId><Labels><a>x</a><b>y</b></Labels><Comment>c &amp; d</Comment><UpdateAt>2020-01-01T00:00:00Z</UpdateAt></User>",
        ),
        (
            "users",
            "<Users><UserId>u1</UserId><Comment><![CDATA[<raw>]]></Comment></Users>",
        ),
        (
            "user-patch",
            "<Patch><Labels>a</Labels><Comment>x</Comment></Patch>",
        ),
        (
            "item",
            "<Item><ItemId>i1</ItemId><IsHidden>true</IsHidden><Categories>c</Categories><Categories>d</Categories><Timestamp>2020-01-01T00:00:00Z</Timestamp><Labels>l</Labels><Comment>x</Comment></Item>",
        ),
        ("items", "<Items><ItemId>i1</ItemId><Categories/></Items>"),
        (
            "item-patch",
            "<P><IsHidden>0</IsHidden><Categories>x</Categories><Timestamp>2021-02-03T04:05:06+02:00</Timestamp><Labels/><Comment>c</Comment></P>",
        ),
        (
            "feedback",
            "<F><FeedbackType>like</FeedbackType><UserId>u</UserId><ItemId>i</ItemId><Value>1.5</Value><Timestamp>2020-01-01T00:00:00Z</Timestamp></F>",
        ),
    ]
}

/// XML bodies that name each error class of `encoding/xml`'s tokenizer, its
/// entities and its struct mapping, and the field kinds of each route.
fn xml_named_bodies() -> Vec<(&'static str, Vec<u8>)> {
    let deep = format!(
        "<User><Labels>{}{}</Labels></User>",
        "<a>".repeat(5000),
        "</a>".repeat(5000)
    );
    let unclosed = format!("<User><Labels>{}", "<a>".repeat(5000));
    vec![
        ("user", "".into()),
        ("user", "   \n".into()),
        ("user", "text only".into()),
        ("user", "<User/>".into()),
        ("user", "<a/>".into()),
        (
            "user",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><User><UserId>x</UserId></User>".into(),
        ),
        (
            "user",
            "<?xml version='1.0' encoding='utf-8'?><User/>".into(),
        ),
        (
            "user",
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><User/>".into(),
        ),
        ("user", "<?xml version=\"1.1\"?><User/>".into()),
        ("user", "<?xml version=\"2.0\"?><User/>".into()),
        ("user", "<?xml?><User/>".into()),
        ("user", "<?xml version=1.0?><User/>".into()),
        ("user", "<?target data?><User/>".into()),
        ("user", "<?".into()),
        ("user", "<? x?>".into()),
        ("user", "<?a:b:c?><User/>".into()),
        ("user", "<!-- c --><User/>".into()),
        ("user", "<!-- a -- b --><User/>".into()),
        ("user", "<!-x--><User/>".into()),
        ("user", "<!--".into()),
        ("user", "<!---->".into()),
        ("user", "<![CDATA[x<y]]><User/>".into()),
        ("user", "<![CDAT[x]]><User/>".into()),
        ("user", "<![CDATA[x".into()),
        ("user", "<![CDATA[]]><User/>".into()),
        ("user", "<!DOCTYPE User [<!ENTITY x \"y\">]><User/>".into()),
        (
            "user",
            "<!DOCTYPE User [<!-- c --><!ENTITY x \"y\">]><User/>".into(),
        ),
        ("user", "<!DOCTYPE User \"<User>\"><User/>".into()),
        ("user", "<!DOCTYPE User [<!ELEMENT a (#PCDATA)>".into()),
        ("user", "<!DOCTYPE User [<!-- c ><User/>".into()),
        ("user", "<User><UserId>a&bogus;</UserId></User>".into()),
        (
            "user",
            "<User><UserId>a&#x41;&#65;&#xD800;&#x1F600;</UserId></User>".into(),
        ),
        ("user", "<User><UserId>&#0;</UserId></User>".into()),
        ("user", "<User><UserId>&#x110000;</UserId></User>".into()),
        ("user", "<User><UserId>&#;</UserId></User>".into()),
        ("user", "<User><UserId>&#x;</UserId></User>".into()),
        ("user", "<User><UserId>& </UserId></User>".into()),
        ("user", "<User><UserId>&amp</UserId></User>".into()),
        (
            "user",
            "<User><UserId>&lt;&gt;&quot;&apos;</UserId></User>".into(),
        ),
        ("user", "<User><UserId>a]]>b</UserId></User>".into()),
        ("user", "<User><UserId>a<b</UserId></User>".into()),
        ("user", "<User><UserId>\u{1}</UserId></User>".into()),
        ("user", "<User><UserId>\u{fffe}</UserId></User>".into()),
        ("user", b"<User><UserId>\xff</UserId></User>".to_vec()),
        ("user", b"<User><UserId>\xe2\x82</UserId></User>".to_vec()),
        (
            "user",
            b"<User><UserId>\xed\xa0\x80</UserId></User>".to_vec(),
        ),
        ("user", "<User><UserId>x</User>".into()),
        ("user", "<User><UserId>x</UserId>".into()),
        ("user", "</User>".into()),
        ("user", "<User a=b/>".into()),
        ("user", "<User a/>".into()),
        ("user", "<User a=\"<\"/>".into()),
        ("user", "<User a=\"x\" b='y' c=\"&amp;\"></User>".into()),
        ("user", "<User / >".into()),
        ("user", "<User/ >".into()),
        ("user", "<User></p:User>".into()),
        (
            "user",
            "<p:User xmlns:p=\"u\"><p:UserId>x</p:UserId></p:User>".into(),
        ),
        ("user", "<p:User xmlns:p=\"u\"><UserId>x</p:User>".into()),
        ("user", "<p:User></User>".into()),
        ("user", "<a:b:c/>".into()),
        ("user", "<1a/>".into()),
        ("user", "<User></User>trailing <<<".into()),
        ("user", "text<User/>".into()),
        ("user", "<User><UserId>x</UserId></User><".into()),
        (
            "user",
            "<User><Labels><a><b>x</b></a></Labels></User>".into(),
        ),
        ("user", "<User><UpdateAt>bad</UpdateAt></User>".into()),
        ("user", "<User><UpdateAt/></User>".into()),
        (
            "user",
            "<User><UpdateAt>2020-01-01T00:00:00+24:00</UpdateAt></User>".into(),
        ),
        (
            "user",
            "<User><UpdateAt> 2020-01-01T00:00:00Z </UpdateAt></User>".into(),
        ),
        (
            "user",
            "<User><UpdateAt><x/>2020-01-01T00:00:00Z</UpdateAt></User>".into(),
        ),
        (
            "user",
            "<User><UserId>a<!-- c -->b<![CDATA[c]]>d</UserId></User>".into(),
        ),
        (
            "user",
            "<User><UserId>x</UserId><UserId>y</UserId></User>".into(),
        ),
        ("user", "<User><UserId><x>a</x>b</UserId></User>".into()),
        ("user", "<User><UserId>a\r\nb\rc</UserId></User>".into()),
        ("user", "<User>\n\t<UserId>x</UserId>\n</User>".into()),
        ("user", "<User><UserId>x</UserId></User>\u{0}".into()),
        ("user", deep.into_bytes()),
        ("user", unclosed.into_bytes()),
        ("users", "<Users><UserId>u1</UserId></Users><Users/>".into()),
        ("users", "<Users/>".into()),
        ("users", "".into()),
        (
            "user-patch",
            "<P><Labels>a</Labels><Comment>x</Comment></P>".into(),
        ),
        ("user-patch", "<P><Comment/></P>".into()),
        ("item", "<Item><IsHidden>  true\n</IsHidden></Item>".into()),
        ("item", "<Item><IsHidden>1</IsHidden></Item>".into()),
        ("item", "<Item><IsHidden>maybe</IsHidden></Item>".into()),
        ("item", "<Item><IsHidden></IsHidden></Item>".into()),
        ("item", "<Item><IsHidden> </IsHidden></Item>".into()),
        (
            "item",
            "<Item><Categories/><Categories>a</Categories></Item>".into(),
        ),
        (
            "item",
            "<Item><Categories><x>y</x>z</Categories></Item>".into(),
        ),
        (
            "item",
            "<Item><Labels>x</Labels><Labels><a/></Labels></Item>".into(),
        ),
        (
            "item",
            "<Item><Timestamp>2020-01-01</Timestamp></Item>".into(),
        ),
        ("item", "<Item><ItemId>a\u{1}</ItemId></Item>".into()),
        ("items", "<Items><ItemId>i1</ItemId></Items>".into()),
        ("items", "<Items/>".into()),
        (
            "item-patch",
            "<P><Timestamp>2021-13-01T00:00:00Z</Timestamp></P>".into(),
        ),
        (
            "item-patch",
            "<P><Timestamp>2021-02-03T04:05:06+02:00</Timestamp></P>".into(),
        ),
        ("item-patch", "<P><Timestamp></Timestamp></P>".into()),
        (
            "item-patch",
            "<P><IsHidden>false</IsHidden><Comment/></P>".into(),
        ),
        (
            "item-patch",
            "<P><Categories>a</Categories><Categories>b</Categories></P>".into(),
        ),
        ("feedback", "<F><Value>abc</Value></F>".into()),
        ("feedback", "<F><Value>1e400</Value></F>".into()),
        ("feedback", "<F><Value></Value></F>".into()),
        ("feedback", "<F><Value> 0x1p-2 </Value></F>".into()),
        ("feedback", "<F><Value>NaN</Value></F>".into()),
        ("feedback", "<F><Value>-Inf</Value></F>".into()),
        ("feedback", "<F><Value>1_000</Value></F>".into()),
        (
            "feedback",
            "<F><FeedbackType>like</FeedbackType><Other><FeedbackType>x</FeedbackType></Other></F>"
                .into(),
        ),
    ]
}

/// The XML cases: the named bodies, each sample with the content types and
/// encodings XML bodies come in, and every truncation and corruption of a sample.
fn xml_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |target: &str, content_type: &str, encoding: &str, body: Vec<u8>| {
        cases.push(Case {
            target: target.to_owned(),
            content_type: content_type.to_owned(),
            content_encoding: encoding.to_owned(),
            body,
        });
    };
    for (target, body) in xml_named_bodies() {
        push(target, "application/xml", "", body);
    }
    for (target, xml) in xml_sample_bodies() {
        let data = xml.as_bytes();
        push(target, "application/xml; charset=utf-8", "", data.to_vec());
        push(target, "text/xml", "", data.to_vec());
        push(target, "application/xml", "gzip", gzip_stream(data, 6));
        push(target, "application/xml", "deflate", zlib(data, 6));
        for end in 0..data.len() {
            push(target, "application/xml", "", data[..end].to_vec());
        }
        for index in 0..data.len() {
            for mask in [0x01, 0x20, 0x80, 0xff] {
                let mut corrupt = data.to_vec();
                corrupt[index] ^= mask;
                push(target, "application/xml", "", corrupt);
            }
        }
    }
    cases
}

#[test]
fn request_bodies_decode_as_go_reads_them() {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(check_request_bodies)
        .expect("spawn the check")
        .join()
        .expect("the check passes");
}

/// Whether a route's type is a list, which Go decodes to a nil slice for `null`.
fn is_list(target: &str) -> bool {
    matches!(target, "users" | "items" | "feedback")
}

fn check_request_bodies() {
    let mut cases = corpus_cases();
    cases.extend(named_cases());
    cases.extend(generated_cases());
    cases.extend(xml_cases());
    let replies = run_probe(&cases);
    assert_eq!(
        replies.len(),
        cases.len(),
        "the probe replies once per body"
    );

    let mut failures = Vec::new();
    for (case, reply) in cases.iter().zip(&replies) {
        let got = gorse_server::rest::decode_body_json(
            &case.target,
            &case.content_type,
            &case.content_encoding,
            &case.body,
        );
        let problem = match (&reply.err, &reply.value) {
            (Some(want), _) => match &got {
                Err(text) if text == want => None,
                _ => Some(format!("Go says error {want:?}, the decoder says {got:?}")),
            },
            // Go decodes the value but cannot write it back; the decoder must
            // still accept the body.
            (None, Some(value)) if value.starts_with("MARSHAL ERROR") => match &got {
                Ok(_) => None,
                Err(text) => Some(format!("Go decodes it, the decoder says error {text:?}")),
            },
            // A nil list and an empty one are the same to the handlers, so the
            // `null` Go writes for a nil list matches the empty list the decoder gives.
            (None, Some(want)) => match &got {
                Ok(text) if text == want => None,
                Ok(text) if want == "null" && text == "[]" && is_list(&case.target) => None,
                _ => Some(format!("Go says value {want}, the decoder says {got:?}")),
            },
            (None, None) => panic!("the probe gave neither an error nor a value"),
        };
        if let Some(problem) = problem {
            failures.push(format!(
                "{} {:?} {:?} {}: {problem}",
                case.target,
                case.content_type,
                case.content_encoding,
                hex_of(&case.body)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} bodies differ from Go:\n{}",
        failures.len(),
        cases.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
