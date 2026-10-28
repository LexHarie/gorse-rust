// Ports: github.com/subosito/gotenv v1.6.0 gotenv_test.go, github.com/spf13/viper v1.21.0 internal/encoding/dotenv/codec_test.go @ add21b5
//
// Derived from gotenv: Copyright (c) 2013 Alif Rachmawadi. MIT license,
// crates/gorse-config/licenses/GOTENV-LICENSE. Derived from viper: Copyright
// (c) 2014 Steve Francia. MIT license, crates/gorse-config/licenses/VIPER-LICENSE.

//! gotenv's and viper's tests of the dotenv parser, with their cases and
//! expectations. The fixtures in `tests/data/gotenv/` are gotenv's files.
//! Tests of `Load`, `OverLoad`, `Apply`, `Must`, `Marshal` and viper's
//! `Encode` are not ported: the codec only parses (see
//! `PORTING_NOTES.md` for the list).

use std::collections::BTreeMap;

use gorse_config::formats::dotenv::{decode, strict_parse};
use gorse_config::value::Value;

/// An environment with no variables.
fn no_env(_: &str) -> Option<String> {
    None
}

/// The parsed variables as text.
fn env_map(
    input: &[u8],
    env: &dyn Fn(&str) -> Option<String>,
) -> (BTreeMap<String, String>, Option<String>) {
    let (parsed, err) = strict_parse(input, env);
    let map = parsed
        .iter()
        .map(|(k, v)| {
            (
                String::from_utf8(k.to_vec()).expect("keys are UTF-8"),
                String::from_utf8(v.to_vec()).expect("values are UTF-8"),
            )
        })
        .collect();
    (
        map,
        err.map(|e| String::from_utf8(e).expect("errors are UTF-8")),
    )
}

/// Variables as gotenv's tests write them, `gotenv.Env{...}`.
type Entries<'a> = &'a [(&'a str, &'a str)];

fn map(entries: Entries<'_>) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/gotenv/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).expect("the fixture reads")
}

/// `TestParse`: `gotenv.Parse`, which is `strictParse` with the error
/// dropped, so a row whose input fails (the `export OPTION_A` row) checks
/// the variables set before the error.
#[test]
fn test_parse() {
    let formats: &[(&str, Entries<'_>, bool)] = &[
        // parses unquoted values
        ("FOO=bar", &[("FOO", "bar")], false),
        // parses values with spaces around equal sign
        ("FOO =bar", &[("FOO", "bar")], false),
        ("FOO= bar", &[("FOO", "bar")], false),
        // parses values with leading spaces
        ("  FOO=bar", &[("FOO", "bar")], false),
        // parses values with following spaces
        ("FOO=bar  ", &[("FOO", "bar")], false),
        // parses double quoted values
        (r#"FOO="bar""#, &[("FOO", "bar")], false),
        // parses double quoted values with following spaces
        (r#"FOO="bar"  "#, &[("FOO", "bar")], false),
        // parses single quoted values
        ("FOO='bar'", &[("FOO", "bar")], false),
        // parses single quoted values with following spaces
        ("FOO='bar'  ", &[("FOO", "bar")], false),
        // parses escaped double quotes
        (r#"FOO="escaped\"bar""#, &[("FOO", r#"escaped"bar"#)], false),
        // parses empty values
        ("FOO=", &[("FOO", "")], false),
        // expands variables found in values
        (
            "FOO=test\nBAR=$FOO",
            &[("FOO", "test"), ("BAR", "test")],
            false,
        ),
        // parses variables wrapped in brackets
        (
            "FOO=test\nBAR=${FOO}bar",
            &[("FOO", "test"), ("BAR", "testbar")],
            false,
        ),
        // reads variables from ENV when expanding if not found in local env
        ("BAR=$FOO", &[("BAR", "test")], true),
        // expands undefined variables to an empty string
        ("BAR=$FOO", &[("BAR", "")], false),
        // expands variables in quoted strings
        (
            "FOO=test\nBAR=\"quote $FOO\"",
            &[("FOO", "test"), ("BAR", "quote test")],
            false,
        ),
        // does not expand variables in single quoted strings
        ("BAR='quote $FOO'", &[("BAR", "quote $FOO")], false),
        // does not expand escaped variables
        (r#"FOO="foo\$BAR""#, &[("FOO", "foo$BAR")], false),
        (r#"FOO="foo\${BAR}""#, &[("FOO", "foo${BAR}")], false),
        (
            "FOO=test\nBAR=\"foo\\${FOO} ${FOO}\"",
            &[("FOO", "test"), ("BAR", "foo${FOO} test")],
            false,
        ),
        // parses yaml style options
        ("OPTION_A: 1", &[("OPTION_A", "1")], false),
        // parses export keyword
        ("export OPTION_A=2", &[("OPTION_A", "2")], false),
        // allows export line if you want to do it that way
        ("OPTION_A=2\nexport OPTION_A", &[("OPTION_A", "2")], false),
        // expands newlines in quoted strings
        (r#"FOO="bar\nbaz""#, &[("FOO", "bar\nbaz")], false),
        // parses variables with "." in the name
        ("FOO.BAR=foobar", &[("FOO.BAR", "foobar")], false),
        // strips unquoted values
        ("foo=bar ", &[("foo", "bar")], false), // not 'bar '
        // ignores empty lines
        (
            "\n \t  \nfoo=bar\n \nfizz=buzz",
            &[("foo", "bar"), ("fizz", "buzz")],
            false,
        ),
        // ignores inline comments
        ("foo=bar # this is foo", &[("foo", "bar")], false),
        // allows # in quoted value
        (r#"foo="bar#baz" # comment"#, &[("foo", "bar#baz")], false),
        // ignores comment lines
        ("\n\n\n # HERE GOES FOO \nfoo=bar", &[("foo", "bar")], false),
        // parses # in quoted values
        (r#"foo="ba#r""#, &[("foo", "ba#r")], false),
        ("foo='ba#r'", &[("foo", "ba#r")], false),
        // parses # in quoted values with following spaces
        (r#"foo="ba#r"  "#, &[("foo", "ba#r")], false),
        ("foo='ba#r'  ", &[("foo", "ba#r")], false),
        // supports carriage return
        ("FOO=bar\rbaz=fbb", &[("FOO", "bar"), ("baz", "fbb")], false),
        // supports carriage return combine with new line
        (
            "FOO=bar\r\nbaz=fbb",
            &[("FOO", "bar"), ("baz", "fbb")],
            false,
        ),
        // expands carriage return in quoted strings
        (r#"FOO="bar\rbaz""#, &[("FOO", "bar\rbaz")], false),
        // escape $ properly when no alphabets/numbers/_  are followed by it
        (r#"FOO="bar\\$ \\$\\$""#, &[("FOO", "bar$ $$")], false),
        // ignore $ when it is not escaped and no variable is followed by it
        (r#"FOO="bar $ ""#, &[("FOO", "bar $ ")], false),
        // parses unquoted values with spaces after separator
        ("FOO= bar", &[("FOO", "bar")], false),
        // allows # in quoted value with spaces after separator
        (r#"foo= "bar#baz" # comment"#, &[("foo", "bar#baz")], false),
        // allows = in double quoted values with newlines (typically base64 padding)
        (r#"foo="---\na==\n---""#, &[("foo", "---\na==\n---")], false),
    ];
    for (input, want, preset) in formats {
        let env = |name: &str| (*preset && name == "FOO").then(|| "test".to_owned());
        let (got, _) = env_map(input.as_bytes(), &env);
        assert_eq!(got, map(want), "{input:?}");
    }
}

/// `TestStrictParse`.
#[test]
fn test_strict_parse() {
    let error_formats: &[(&str, Entries<'_>, &str)] = &[
        // allows export line if you want to do it that way and checks for unset variables
        (
            "OPTION_A=2\nexport OH_NO_NOT_SET",
            &[("OPTION_A", "2")],
            "line `export OH_NO_NOT_SET` has an unset variable",
        ),
        // throws an error if line format is incorrect
        ("lol$wut", &[], "line `lol$wut` doesn't match format"),
    ];
    for (input, want, err) in error_formats {
        let (got, got_err) = env_map(input.as_bytes(), &no_env);
        assert_eq!(got_err.as_deref(), Some(*err), "{input:?}");
        assert_eq!(got, map(want), "{input:?}");
    }
}

/// `TestStrictParse_NoTokenPassThroughErrors`: the test's reader returns
/// `len(p)` zero bytes on every read, so no line ever ends and the scanner
/// fails with `bufio.ErrTooLong`. Here the input is 64 KiB and one zero
/// bytes, which fill the scanner's largest buffer the same way.
/// (`TestStrictParse_PassThroughErrors` needs a reader that fails; viper
/// passes a `bytes.Buffer`, which cannot, so it is not ported.)
#[test]
fn test_strict_parse_no_token_pass_through_errors() {
    let input = vec![0u8; 64 * 1024 + 1];
    let (_, err) = env_map(&input, &no_env);
    assert_eq!(err.as_deref(), Some("bufio.Scanner: token too long"));
}

/// `TestRead`: `gotenv.Read` is `strictParse` on the file.
#[test]
fn test_read() {
    let mit = "The MIT License (MIT)

Copyright (c) 2013 Alif Rachmawadi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.";
    let fixtures: &[(&str, Entries<'_>)] = &[
        (
            "fixtures/exported.env",
            &[("OPTION_A", "2"), ("OPTION_B", r"\n"), ("OPTION_C", mit)],
        ),
        (
            "fixtures/plain.env",
            &[
                ("OPTION_A", "1"),
                ("OPTION_B", "2"),
                ("OPTION_C", "3"),
                ("OPTION_D", "4"),
                ("OPTION_E", "5"),
            ],
        ),
        (
            "fixtures/quoted.env",
            &[
                ("OPTION_A", "1"),
                ("OPTION_B", "2"),
                ("OPTION_C", ""),
                ("OPTION_D", r"\n"),
                ("OPTION_E", "1"),
                ("OPTION_F", "2"),
                ("OPTION_G", ""),
                ("OPTION_H", "\n"),
                (
                    "OPTION_I",
                    "some multi-line text\nwith \"escaped quotes\" and 1 variable",
                ),
                ("OPTION_J", r#"some$pecial$1$2!*chars=qweq""e$$\$"""#),
                ("OPTION_K", "\n"),
                (
                    "OPTION_L",
                    "some multi-line text\nwith \"escaped quotes\"\nempty lines\n\nand 1 variable\n",
                ),
            ],
        ),
        (
            "fixtures/yaml.env",
            &[
                ("OPTION_A", "1"),
                ("OPTION_B", "2"),
                ("OPTION_C", ""),
                ("OPTION_D", r"\n"),
            ],
        ),
    ];
    for (name, want) in fixtures {
        let (got, err) = env_map(&fixture(name), &no_env);
        assert_eq!(err, None, "{name}");
        for (key, value) in *want {
            assert_eq!(
                got.get(*key).map(String::as_str),
                Some(*value),
                "{name} {key}"
            );
        }
    }
}

/// `TestLoad_default` and `TestMust_Load_default` read `.env`; the part
/// that parses it.
#[test]
fn test_load_default() {
    let (got, err) = env_map(&fixture(".env"), &no_env);
    assert_eq!(err, None);
    assert_eq!(got, map(&[("HELLO", "world")]));
}

/// `TestLoad_overrideVars`: with `A=fromEnv` in the environment, `B="$A"`
/// reads the environment.
#[test]
fn test_load_override_vars() {
    let env = |name: &str| (name == "A").then(|| "fromEnv".to_owned());
    let (got, err) = env_map(&fixture("fixtures/vars.env"), &env);
    assert_eq!(err, None);
    assert_eq!(got["B"], "fromEnv");
}

/// `TestLoad_overrideVars2`: `D="$C"` comes before `C` in the file, so it
/// reads the environment.
#[test]
fn test_load_override_vars2() {
    let env = |name: &str| (name == "C").then(|| "fromEnv".to_owned());
    let (got, err) = env_map(&fixture("fixtures/vars.env"), &env);
    assert_eq!(err, None);
    assert_eq!(got["D"], "fromEnv");
}

/// `TestLoad_Env`: `.env.invalid` does not parse.
#[test]
fn test_load_env() {
    let (_, err) = env_map(&fixture(".env.invalid"), &no_env);
    assert_eq!(err.as_deref(), Some("line `lol$wut` doesn't match format"));
}

/// `TestLoad_BOM_UTF8`, `TestLoad_BOM_UTF16_LE` and `TestLoad_BOM_UTF16_BE`.
#[test]
fn test_load_bom() {
    for (name, want) in [
        ("fixtures/utf8_bom.env", "UTF-8"),
        ("fixtures/utf16le_bom.env", "UTF-16 LE"),
        ("fixtures/utf16be_bom.env", "UTF-16 BE"),
    ] {
        let (got, err) = env_map(&fixture(name), &no_env);
        assert_eq!(err, None, "{name}");
        assert_eq!(got["BOM"], want, "{name}");
    }
}

/// `TestMarshal`, its `Unmarshal` half: the text `Marshal` writes parses
/// back to the map.
#[test]
fn test_marshal_unmarshal() {
    let expected = "EMPTY=\"\"
FOO=\"BAR\"
ONE=1
QUOTED=\"some \\\"quoted\\\" text\"";
    let (got, err) = env_map(expected.as_bytes(), &no_env);
    assert_eq!(err, None);
    assert_eq!(
        got,
        map(&[
            ("FOO", "BAR"),
            ("ONE", "1"),
            ("QUOTED", r#"some "quoted" text"#),
            ("EMPTY", ""),
        ])
    );
}

/// viper's `TestCodec_Decode`.
#[test]
fn test_codec_decode() {
    // OK
    let original = "# key-value pair\nKEY=value\n";
    let got = decode(original.as_bytes(), &no_env).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got.get("KEY"), Some(&Value::String("value".to_owned())));

    // InvalidData
    let err = decode(b"invalid data", &no_env).unwrap_err();
    assert_eq!(err, "line `invalid data` doesn't match format");
}
