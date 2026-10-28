// Ports: unmarshaler_test.go, fast_test.go, internal/imported_tests/unmarshal_imported_test.go @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! go-toml v2.2.4's own tests of `Unmarshal`, ported where they decode into
//! a `map[string]interface{}` (or an `interface{}`, which go-toml fills with
//! one), as viper does. Tests that decode into typed maps or structs are
//! ported with a `map[string]interface{}` target where the TOML values keep
//! their meaning, and say so; the others are listed in
//! `porting/notes/WP-1-15-toml.md`. Every input here is also a case of
//! `toml_golden.json`, which records Go's exact result.

use gorse_config::formats::toml::{self, Error};
use gorse_config::value::{LocalDate, LocalTime, Map, Value};

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn int(v: i64) -> Value {
    Value::Int64(v)
}

fn arr(items: Vec<Value>) -> Value {
    Value::Slice(items)
}

fn map(entries: Vec<(&str, Value)>) -> Map {
    Map::from(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<Vec<_>>(),
    )
}

fn table(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(map(entries))
}

fn decode(doc: &str) -> Result<Map, Error> {
    toml::unmarshal(doc.as_bytes())
}

/// `assert.NoError` and `assert.Equal`: the map, in document order.
fn assert_decodes(doc: &str, expected: Map) {
    match decode(doc) {
        Ok(m) => assert_eq!(m, expected, "{doc:?}"),
        Err(err) => panic!("{doc:?}: {err}"),
    }
}

/// `assert.Error`.
fn assert_fails(doc: &str) {
    assert!(decode(doc).is_err(), "{doc:?} decoded");
}

/// `assert.Error` and `errors.As(err, &de)` with de a `*toml.DecodeError`.
fn assert_decode_error(doc: &[u8]) -> toml::DecodeError {
    match toml::unmarshal(doc) {
        Err(Error::Decode(de)) => de,
        other => panic!(
            "{:?}: err should have been a *toml.DecodeError, but got {other:?}",
            String::from_utf8_lossy(doc)
        ),
    }
}

// unmarshaler_test.go TestUnmarshal_Integers, decoding into a map instead
// of a struct with an `int64` field: the values are the same `int64`s.
#[test]
fn unmarshal_integers() {
    let examples: [(&str, &str, Option<i64>); 11] = [
        ("integer just digits", "1234", Some(1234)),
        ("integer zero", "0", Some(0)),
        ("integer sign", "+99", Some(99)),
        ("integer decimal underscore", "123_456", Some(123456)),
        ("integer hex uppercase", "0xDEADBEEF", Some(0xDEADBEEF)),
        ("integer hex lowercase", "0xdead_beef", Some(0xDEADBEEF)),
        ("integer octal", "0o01234567", Some(0o01234567)),
        ("integer binary", "0b11010110", Some(0b11010110)),
        ("double underscore", "12__3", None),
        ("starts with underscore", "_1", None),
        ("ends with underscore", "1_", None),
    ];
    for (_desc, input, expected) in examples {
        let doc = format!("A = {input}");
        match expected {
            Some(v) => assert_decodes(&doc, map(vec![("A", int(v))])),
            None => assert_fails(&doc),
        }
    }
}

// unmarshaler_test.go TestUnmarshal_Floats, decoding into a map instead of
// a struct with a `float64` field. One case differs because of the target:
// `0` ("float zero without decimals") is an integer, so the map holds
// `int64(0)` where the struct field converts it to `0.0`.
#[test]
#[expect(clippy::approx_constant, reason = "go-toml's test value 3.1415, not π")]
fn unmarshal_floats() {
    let examples: [(&str, &str, Option<f64>); 18] = [
        ("float pi", "3.1415", Some(3.1415)),
        ("float negative", "-0.01", Some(-0.01)),
        ("float signed exponent", "5e+22", Some(5e+22)),
        ("float exponent lowercase", "1e06", Some(1e06)),
        ("float exponent uppercase", "-2E-2", Some(-2e-2)),
        ("float exponent zero", "0e0", Some(0.0)),
        ("float upper exponent zero", "0E0", Some(0.0)),
        (
            "float fractional with exponent",
            "6.626e-34",
            Some(6.626e-34),
        ),
        (
            "float underscores",
            "224_617.445_991_228",
            Some(224_617.445_991_228),
        ),
        ("inf", "inf", Some(f64::INFINITY)),
        ("inf negative", "-inf", Some(f64::NEG_INFINITY)),
        ("inf positive", "+inf", Some(f64::INFINITY)),
        ("nan", "nan", Some(f64::NAN)),
        ("nan negative", "-nan", Some(f64::NAN)),
        ("nan positive", "+nan", Some(f64::NAN)),
        ("underscore after integer part", "1_e2", None),
        ("underscore after integer part", "1.0_e2", None),
        ("leading zero in positive float", "+0_0.0", None),
    ];
    for (desc, input, expected) in examples {
        let doc = format!("A = {input}");
        match expected {
            Some(want) => {
                let m = decode(&doc).unwrap_or_else(|err| panic!("{desc}: {err}"));
                match m.get("A") {
                    Some(Value::Float64(got)) if want.is_nan() => assert!(got.is_nan(), "{desc}"),
                    Some(Value::Float64(got)) => assert_eq!(*got, want, "{desc}"),
                    other => panic!("{desc}: {other:?}"),
                }
            }
            None => assert_fails(&doc),
        }
    }
    // float zero without decimals
    assert_decodes("A = 0", map(vec![("A", int(0))]));
}

// unmarshaler_test.go TestUnmarshal: the examples whose target is a
// `map[string]interface{}` (or holds one).
#[test]
fn unmarshal_into_maps() {
    let lt = |hour, minute, second, nanosecond, precision| {
        Value::LocalTime(LocalTime {
            hour,
            minute,
            second,
            nanosecond,
            precision,
        })
    };
    assert_decodes(
        "a = 12:08:05.666666666",
        map(vec![("a", lt(12, 8, 5, 666666666, 9))]),
    );
    assert_decodes("a = 12:08:05", map(vec![("a", lt(12, 8, 5, 0, 0))]));
    assert_fails("a = 12:08:0"); // local-time missing digit
    assert_fails("a = 12:08:000"); // local-time extra digit

    // issue 475 - space between dots in key
    assert_decodes(
        "fruit. color = \"yellow\"\n\t\t\t\t\tfruit . flavor = \"banana\"",
        map(vec![(
            "fruit",
            table(vec![("color", s("yellow")), ("flavor", s("banana"))]),
        )]),
    );
    // issue 427 - quotation marks in key
    assert_decodes(
        "'\"a\"' = 1\n\t\t\t\t\t\"\\\"b\\\"\" = 2",
        map(vec![("\"a\"", int(1)), ("\"b\"", int(2))]),
    );
    // issue 739 - table redefinition
    assert_decodes(
        "\n[foo.bar.baz]\nwibble = 'wobble'\n\n[foo]\n\n[foo.bar]\nhuey = 'dewey'\n\t\t\t",
        map(vec![(
            "foo",
            table(vec![(
                "bar",
                table(vec![
                    ("baz", table(vec![("wibble", s("wobble"))])),
                    ("huey", s("dewey")),
                ]),
            )]),
        )]),
    );
    // spaces around dotted keys (a map[string]map[string]interface{} in Go)
    assert_decodes("a . b = 1", map(vec![("a", table(vec![("b", int(1))]))]));
    // standard empty table
    assert_decodes("[A]", map(vec![("A", table(vec![]))]));
    // inline empty table
    assert_decodes("A = {}", map(vec![("A", table(vec![]))]));
    // into map[string]interface{}
    assert_decodes("A = \"foo\"", map(vec![("A", s("foo"))]));
    // multi keys of different types into map[string]interface{}
    assert_decodes(
        "A = \"foo\"\n\t\t\t\t\tB = 42",
        map(vec![("A", s("foo")), ("B", int(42))]),
    );
    // slice in a map[string]interface{}
    assert_decodes(
        "A = [\"foo\", \"bar\"]",
        map(vec![("A", arr(vec![s("foo"), s("bar")]))]),
    );
    // one-level multi-element array table to map
    assert_decodes(
        "[[Products]]
\t\t\t\t\tName = \"Hammer\"
\t\t\t\t\tSku = 738594937

\t\t\t\t\t[[Products]]  # empty table within the array

\t\t\t\t\t[[Products]]
\t\t\t\t\tName = \"Nail\"
\t\t\t\t\tSku = 284758393

\t\t\t\t\tColor = \"gray\"",
        map(vec![(
            "Products",
            arr(vec![
                table(vec![("Name", s("Hammer")), ("Sku", int(738594937))]),
                table(vec![]),
                table(vec![
                    ("Name", s("Nail")),
                    ("Sku", int(284758393)),
                    ("Color", s("gray")),
                ]),
            ]),
        )]),
    );
    // sub-table in array table
    assert_decodes(
        "[[Fruits]]
\t\t\t\t\tName = \"apple\"

\t\t\t\t\t[Fruits.Physical]  # subtable
\t\t\t\t\tColor = \"red\"
\t\t\t\t\tShape = \"round\"",
        map(vec![(
            "Fruits",
            arr(vec![table(vec![
                ("Name", s("apple")),
                (
                    "Physical",
                    table(vec![("Color", s("red")), ("Shape", s("round"))]),
                ),
            ])]),
        )]),
    );
    // multiple sub-table in array tables
    assert_decodes(
        "[[Fruits]]
\t\t\t\t\tName = \"apple\"

\t\t\t\t\t[[Fruits.Varieties]]  # nested array of tables
\t\t\t\t\tName = \"red delicious\"

\t\t\t\t\t[[Fruits.Varieties]]
\t\t\t\t\tName = \"granny smith\"

\t\t\t\t\t[[Fruits]]
\t\t\t\t\tName = \"banana\"

\t\t\t\t\t[[Fruits.Varieties]]
\t\t\t\t\tName = \"plantain\"",
        map(vec![(
            "Fruits",
            arr(vec![
                table(vec![
                    ("Name", s("apple")),
                    (
                        "Varieties",
                        arr(vec![
                            table(vec![("Name", s("red delicious"))]),
                            table(vec![("Name", s("granny smith"))]),
                        ]),
                    ),
                ]),
                table(vec![
                    ("Name", s("banana")),
                    ("Varieties", arr(vec![table(vec![("Name", s("plantain"))])])),
                ]),
            ]),
        )]),
    );
    // empty array table in interface{}
    assert_decodes(
        "[[products]]",
        map(vec![("products", arr(vec![table(vec![])]))]),
    );
    // array of int in struct (a *map[string]interface{} field in Go)
    assert_decodes(
        "[A]\n\t\t\tb = 42",
        map(vec![("A", table(vec![("b", int(42))]))]),
    );
    // windows line endings
    assert_decodes(
        "A = 1\r\n\r\nB = 2",
        map(vec![("A", int(1)), ("B", int(2))]),
    );
    assert_fails("A = 1\r"); // dangling CR
    assert_fails("A = 1\rB = 2"); // missing NL after CR
    assert_fails("a = 1z = 2"); // no newline (#526)
    // comment with CRLF
    assert_decodes("# foo\r\na=2", map(vec![("a", int(2))]));
    // comment that looks like a date
    assert_decodes("a=19#9-", map(vec![("a", int(19))]));
    assert_decodes("a=199#-", map(vec![("a", int(199))]));
}

// unmarshaler_test.go TestUnmarshalDecodeErrors.
#[test]
fn unmarshal_decode_errors() {
    let examples: &[(&str, &str, &str)] = &[
        ("local date with invalid digit", "a = 20x1-05-21", ""),
        ("local time with fractional", "a = 11:22:33.x", ""),
        (
            "wrong time offset separator",
            "a = 1979-05-27T00:32:00.-07:00",
            "",
        ),
        (
            "wrong time offset separator",
            "a = 1979-05-27T00:32:00Z07:00",
            "",
        ),
        ("float with double _", "flt8 = 224_617.445_991__228", ""),
        ("float with double .", "flt8 = 1..2", ""),
        (
            "number with plus sign and leading underscore",
            "a = +_0",
            "",
        ),
        (
            "number with negative sign and leading underscore",
            "a = -_0",
            "",
        ),
        (
            "exponent with plus sign and leading underscore",
            "a = 0e+_0",
            "",
        ),
        (
            "exponent with negative sign and leading underscore",
            "a = 0e-_0",
            "",
        ),
        ("int with wrong base", "a = 0f2", ""),
        ("int hex with double underscore", "a = 0xFFF__FFF", ""),
        ("int hex very large", "a = 0xFFFFFFFFFFFFFFFFF", ""),
        ("int oct with double underscore", "a = 0o777__77", ""),
        ("int oct very large", "a = 0o77777777777777777777777", ""),
        ("int bin with double underscore", "a = 0b111__111", ""),
        (
            "int bin very large",
            "a = 0b11111111111111111111111111111111111111111111111111111111111111111111111111111",
            "",
        ),
        ("int dec very large", "a = 999999999999999999999999", ""),
        (
            "literal string with new lines",
            "a = 'hello\nworld'",
            "literal strings cannot have new lines",
        ),
        (
            "unterminated literal string",
            "a = 'hello",
            "unterminated literal string",
        ),
        (
            "unterminated multiline literal string",
            "a = '''hello",
            "multiline literal string not terminated by '''",
        ),
        (
            "basic string with new lines",
            "a = \"hello\n\"",
            "basic strings cannot have new lines",
        ),
        (
            "basic string with unfinished escape",
            "a = \"hello \\",
            "need a character after \\",
        ),
        (
            "basic unfinished multiline string",
            "a = \"\"\"hello",
            "multiline basic string not terminated by \"\"\"",
        ),
        (
            "basic unfinished escape in multiline string",
            "a = \"\"\"hello \\",
            "need a character after \\",
        ),
        (
            "malformed local date",
            "a = 2021-033-0",
            "dates are expected to have the format YYYY-MM-DD",
        ),
        (
            "malformed tz",
            "a = 2021-03-30 21:31:00+1",
            "invalid date-time timezone",
        ),
        (
            "malformed tz first char",
            "a = 2021-03-30 21:31:00:1",
            "extra characters at the end of a local date time",
        ),
        (
            "bad char between hours and minutes",
            "a = 2021-03-30 213:1:00",
            "expecting colon between hours and minutes",
        ),
        (
            "bad char between minutes and seconds",
            "a = 2021-03-30 21:312:0",
            "expecting colon between minutes and seconds",
        ),
        (
            "invalid hour value",
            "a=1979-05-27T90:+2:99",
            "hour cannot be greater 23",
        ),
        (
            "invalid minutes value",
            "a=1979-05-27T23:+2:99",
            "expected digit (0-9)",
        ),
        (
            "invalid seconds value",
            "a=1979-05-27T12:45:99",
            "seconds cannot be greater 60",
        ),
        ("binary with invalid digit", "a = 0bf", ""),
        ("invalid i in dec", "a = 0i", ""),
        ("invalid n in dec", "a = 0n", ""),
        ("invalid unquoted key", "a", ""),
        ("dt with tz has no time", "a = 2021-03-30TZ", ""),
        ("invalid end of array table", "[[a}", ""),
        ("invalid end of array table two", "[[a]}", ""),
        ("eof after equal", "a =", ""),
        ("invalid true boolean", "a = trois", ""),
        ("invalid false boolean", "a = faux", ""),
        ("inline table with incorrect separator", "a = {b=1;}", ""),
        ("inline table with invalid value", "a = {b=faux}", ""),
        ("incomplete array after whitespace", "a = [ ", ""),
        ("array with comma first", "a = [ ,]", ""),
        ("array staring with incomplete newline", "a = [\r]", ""),
        (
            "array with incomplete newline after comma",
            "a = [1,\r]",
            "",
        ),
        ("array with incomplete newline after value", "a = [1\r]", ""),
        (
            "invalid unicode in basic multiline string",
            "A = \"\"\"\\u123\"\"\"",
            "",
        ),
        (
            "invalid long unicode in basic multiline string",
            "A = \"\"\"\\U0001D11\"\"\"",
            "",
        ),
        ("invalid unicode in basic string", "A = \"\\u123\"", ""),
        (
            "invalid long unicode in basic string",
            "A = \"\\U0001D11\"",
            "",
        ),
        (
            "invalid escape char basic multiline string",
            "A = \"\"\"\\z\"\"\"",
            "",
        ),
        ("invalid inf", "A = ick", ""),
        ("invalid nan", "A = non", ""),
        ("invalid character in comment in array", "A = [#\x00\n]", ""),
        (
            "basic string with newline after the first escape code",
            "a = \"\\t\n\"",
            "",
        ),
        (
            "basic string with unfinished escape sequence after the first escape code",
            "a = \"\\t\\",
            "",
        ),
        (
            "basic string with unfinished after the first escape code",
            "a = \"\\t",
            "",
        ),
        (
            "multiline basic string with unfinished escape sequence after the first escape code",
            "a = \"\"\"\\t\\",
            "",
        ),
        (
            "impossible date-day",
            "A = 2021-03-40T23:59:00",
            "impossible date",
        ),
        (
            "leap day in non-leap year",
            "A = 2021-02-29T23:59:00",
            "impossible date",
        ),
        ("missing minute digit", "a=17:4::01", ""),
        ("invalid space in year", "i=19 7-12-21T10:32:00", ""),
        ("missing nanoseconds digits", "a=17:45:56.", ""),
        ("minutes over 60", "a=17:99:00", ""),
        ("invalid second", "a=17:00::0", ""),
        ("invalid hour", "a=1::00:00", ""),
        ("invalid month", "a=2021-0--29", ""),
        ("zero is an invalid day", "a=2021-11-00", ""),
        ("zero is an invalid month", "a=2021-00-11", ""),
        (
            "invalid number of seconds digits with trailing digit",
            "a=0000-01-01 00:00:000000Z3",
            "",
        ),
        (
            "invalid zone offset hours",
            "a=0000-01-01 00:00:00+24:00",
            "",
        ),
        (
            "invalid zone offset minutes",
            "a=0000-01-01 00:00:00+00:60",
            "",
        ),
        (
            "invalid character in zone offset hours",
            "a=0000-01-01 00:00:00+0Z:00",
            "",
        ),
        (
            "invalid character in zone offset minutes",
            "a=0000-01-01 00:00:00+00:0Z",
            "",
        ),
        (
            "invalid number of seconds",
            "a=0000-01-01 00:00:00+27000",
            "",
        ),
        ("carriage return inside basic key", "\"\r\"=42", ""),
        ("carriage return inside literal key", "'\r'=42", ""),
        ("carriage return inside basic string", "A = \"\r\"", ""),
        (
            "carriage return inside basic multiline string",
            "a=\"\"\"\r\"\"\"",
            "",
        ),
        (
            "carriage return at the trail of basic multiline string",
            "a=\"\"\"\r",
            "",
        ),
        ("carriage return inside literal string", "A = '\r'", ""),
        (
            "carriage return inside multiline literal string",
            "a='''\r'''",
            "",
        ),
        (
            "carriage return at trail of multiline literal string",
            "a='''\r",
            "",
        ),
        ("carriage return in comment", "# this is a test\ra=1", ""),
        ("backspace in comment", "# this is a test\x08a=1", ""),
    ];
    // The cases with bytes that are not UTF-8.
    let byte_examples: &[(&str, &[u8])] = &[
        (
            "invalid utf8 character in long string with no escape sequence",
            b"a = \"aaaa\x80aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
        ),
        (
            "invalid ascii character in long string with no escape sequence",
            b"a = \"aaaa\x00aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"",
        ),
        (
            "unfinished 2-byte utf8 character in string with no escape sequence",
            b"a = \"aaaa\xC2\"",
        ),
        (
            "unfinished 3-byte utf8 character in string with no escape sequence",
            b"a = \"aaaa\xE2\x00\x00\"",
        ),
        (
            "invalid 3rd byte of 3-byte utf8 character in string with no escape sequence",
            b"a = \"aaaa\xE2\x80\x00\"",
        ),
        (
            "invalid 4th byte of 4-byte utf8 character in string with no escape sequence",
            b"a = \"aaaa\xF2\x81\x81\x00\"",
        ),
        (
            "unfinished 2-byte utf8 character in literal string",
            b"a = 'aaa\xC2'",
        ),
        (
            "unfinished 3-byte utf8 character in literal string",
            b"a = 'aaaa\xE2\x00\x00'",
        ),
        (
            "invalid 3rd byte of 3-byte utf8 character in literal string",
            b"a = 'aaaa\xE2\x80\x00'",
        ),
        (
            "invalid 4th byte of 4-byte utf8 character in literal string",
            b"a = 'aaaa\xF2\x81\x81\x00'",
        ),
        (
            "invalid start utf8 character in literal string",
            b"a = '\x80'",
        ),
        (
            "utf8 character with not enough bytes before end in literal string",
            b"a = '\xEF'",
        ),
    ];
    for (desc, data, msg) in examples {
        let de = assert_decode_error(data.as_bytes());
        if !msg.is_empty() {
            assert_eq!(de.to_string(), format!("toml: {msg}"), "{desc}");
        }
    }
    for (_desc, data) in byte_examples {
        assert_decode_error(data);
    }
}

// unmarshaler_test.go TestASCIIControlCharacters.
#[test]
fn ascii_control_characters() {
    let mut invalid_characters = vec![0x7F_u8];
    invalid_characters.extend(0x00..=0x08);
    invalid_characters.extend(0x0B..=0x0C);
    invalid_characters.extend(0x0E..=0x1F);

    let string_types = [
        ("basic", "\"", true),
        ("basicMultiline", "\"\"\"", true),
        ("literal", "'", false),
        ("literalMultiline", "'''", false),
    ];

    for (_name, delimiter, can_escape) in string_types {
        for &c in &invalid_characters {
            let c = char::from(c);
            assert_decode_error(format!("A = {delimiter}{c}{delimiter}").as_bytes());
            if can_escape {
                assert_decode_error(format!("A = {delimiter}\\t{c}{delimiter}").as_bytes());
                assert_decode_error(format!("A = {delimiter}{c}\\t{delimiter}").as_bytes());
            }
        }
    }
}

// unmarshaler_test.go TestLocalDateTime, decoding into a
// `map[string]interface{}` instead of a `map[string]toml.LocalDateTime`
// (the values are the same). Go computes the expected fields with
// `time.Parse`; they are written out here.
#[test]
fn local_date_time() {
    let examples = [
        (
            "9 digits zero nanoseconds",
            "2006-01-02T15:04:05.000000000",
            0,
            9,
        ),
        ("9 digits", "2006-01-02T15:04:05.123456789", 123456789, 9),
        ("8 digits", "2006-01-02T15:04:05.12345678", 123456780, 8),
        ("7 digits", "2006-01-02T15:04:05.1234567", 123456700, 7),
        ("6 digits", "2006-01-02T15:04:05.123456", 123456000, 6),
        ("5 digits", "2006-01-02T15:04:05.12345", 123450000, 5),
        ("4 digits", "2006-01-02T15:04:05.1234", 123400000, 4),
        ("3 digits", "2006-01-02T15:04:05.123", 123000000, 3),
        ("2 digits", "2006-01-02T15:04:05.12", 120000000, 2),
        ("1 digit", "2006-01-02T15:04:05.1", 100000000, 1),
        ("0 digit", "2006-01-02T15:04:05", 0, 0),
    ];
    for (_desc, input, nanosecond, precision) in examples {
        let expected = Value::LocalDateTime(
            LocalDate {
                year: 2006,
                month: 1,
                day: 2,
            },
            LocalTime {
                hour: 15,
                minute: 4,
                second: 5,
                nanosecond,
                precision,
            },
        );
        assert_decodes(&format!("a = {input}"), map(vec![("a", expected)]));
    }
}

// unmarshaler_test.go TestIssue287.
#[test]
fn issue_287() {
    assert_decodes(
        "y=[[{}]]",
        map(vec![("y", arr(vec![arr(vec![table(vec![])])]))]),
    );
}

// unmarshaler_test.go TestIssue458 (a named map[string]interface{} type).
#[test]
fn issue_458() {
    assert_decodes(
        "[[package]]
dependencies = [\"regex\"]
name = \"decode\"
version = \"0.1.0\"",
        map(vec![(
            "package",
            arr(vec![table(vec![
                ("dependencies", arr(vec![s("regex")])),
                ("name", s("decode")),
                ("version", s("0.1.0")),
            ])]),
        )]),
    );
}

// unmarshaler_test.go TestIssue507, 579, 581, 585, 586, 588, 596, 631,
// 658, 662, 666, 701, 703, 714 and 715: documents go-toml rejects.
#[test]
fn issues_rejected() {
    assert_fails("0=\n0ame"); // 507
    assert_fails("[foo"); // 579
    assert_fails("P=[#"); // 581
    assert_fails("a=1979-05127T 0"); // 585
    assert_fails("a={ "); // 586
    assert_fails("a=[1#"); // 588
    assert_fails("a=1979-05-27T90:+2:99"); // 596
    assert_fails("\"\\b\u{7f}\"= 2"); // 631
    assert_fails("e={b=1,b=4}"); // 658
    assert_fails("a=[{b=1,b=2}]"); // 662
    assert_fails("a={}\na={}"); // 666
    assert_fails("\na={}\n[a.b]\nz=0\n"); // 701
    assert_fails("\na={}\n[[a.b]]\nz=0\n"); // 701
    assert_fails("[a]\nx.y=0\n[a.x]"); // 703
    assert_fails("0."); // 714
    assert_fails("0={0=0,"); // 714
    assert_fails("0=+"); // 715
    assert_fails("0=-"); // 715
    assert_fails("0=+A"); // 715
}

// unmarshaler_test.go TestIssue600: lowercase 't' and 'z'.
#[test]
fn issue_600() {
    assert!(decode("a=1979-05-27t00:32:00z").is_ok());
}

// unmarshaler_test.go TestIssue602: an empty document is an empty map.
#[test]
fn issue_602() {
    assert_decodes("", Map::new());
}

// unmarshaler_test.go TestIssue708, into a `map[string]interface{}`
// instead of a `map[string]string`.
#[test]
fn issue_708() {
    assert_decodes("0=\"\"\"\\\r\n\"\"\"", map(vec![("0", s(""))]));
}

// unmarshaler_test.go TestIssue710, into a `map[string]interface{}`
// instead of a `map[string]toml.LocalTime`: more than nine fractional
// digits are cut, not rounded.
#[test]
fn issue_710() {
    let lt = |nanosecond| {
        Value::LocalTime(LocalTime {
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond,
            precision: 9,
        })
    };
    assert_decodes("0=00:00:00.0000000000", map(vec![("0", lt(0))]));
    assert_decodes("0=00:00:00.0000000001", map(vec![("0", lt(0))]));
    assert_decodes("0=00:00:00.1111111119", map(vec![("0", lt(111111111))]));
}

// fast_test.go TestFastSimpleInt (a `map[string]int64` in Go) and
// TestFastSimpleString (a `map[string]string`).
#[test]
fn fast_simple() {
    assert_decodes("a = 42", map(vec![("a", int(42))]));
    assert_decodes("a = \"hello\"", map(vec![("a", s("hello"))]));
}

// fast_test.go TestFastSimpleInterface.
#[test]
fn fast_simple_interface() {
    assert_decodes(
        "\n\ta = \"hello\"\n\tb = 42",
        map(vec![("a", s("hello")), ("b", int(42))]),
    );
}

// fast_test.go TestFastMultipartKeyInterface.
#[test]
fn fast_multipart_key_interface() {
    assert_decodes(
        "\n\ta.interim = \"test\"\n\ta.b.c = \"hello\"\n\tb = 42",
        map(vec![
            (
                "a",
                table(vec![
                    ("interim", s("test")),
                    ("b", table(vec![("c", s("hello"))])),
                ]),
            ),
            ("b", int(42)),
        ]),
    );
}

// fast_test.go TestFastArrayTable.
#[test]
fn fast_array_table() {
    assert_decodes(
        "\n\t[root]\n\t[[root.nested]]\n\tname = 'Bob'\n\t[[root.nested]]\n\tname = 'Alice'\n\t",
        map(vec![(
            "root",
            table(vec![(
                "nested",
                arr(vec![
                    table(vec![("name", s("Bob"))]),
                    table(vec![("name", s("Alice"))]),
                ]),
            )]),
        )]),
    );
}

// errors_test.go ExampleDecodeError.
#[test]
fn example_decode_error() {
    let de = assert_decode_error(b"name = 123__456");
    assert_eq!(
        de.to_string(),
        "toml: number must have at least one digit between underscores"
    );
    assert_eq!(
        String::from_utf8_lossy(de.human()),
        "1| name = 123__456\n |           ~~ number must have at least one digit between underscores"
    );
    assert_eq!(de.position(), (1, 11));
}

// internal/imported_tests TestUnmarshalEmptyInterface.
#[test]
fn imported_empty_interface() {
    assert_decodes("User = \"pelletier\"", map(vec![("User", s("pelletier"))]));
}

// internal/imported_tests TestInterface: the `interface{}` field becomes a
// `map[string]interface{}`; here the whole document is one.
#[test]
fn imported_interface() {
    assert_decodes(
        "Name = \"rui\"
Age = 18

[Inter]
  FirstName = \"wang\"
  LastName = \"jl\"
  Age = 100",
        map(vec![
            ("Name", s("rui")),
            ("Age", int(18)),
            (
                "Inter",
                table(vec![
                    ("FirstName", s("wang")),
                    ("LastName", s("jl")),
                    ("Age", int(100)),
                ]),
            ),
        ]),
    );
}

// internal/imported_tests TestUnmarshalTabInStringAndQuotedKey, into a map
// instead of a struct with string fields.
#[test]
fn imported_tab_in_string_and_quoted_key() {
    assert_decodes(
        "Field2 = \"\"\"\nhello\tworld\"\"\"",
        map(vec![("Field2", s("hello\tworld"))]),
    );
    assert_decodes(
        "\"Fie\tld1\" = \"key with tab\"",
        map(vec![("Fie\tld1", s("key with tab"))]),
    );
    assert_decodes(
        "Field2 = \"hello\tworld\"",
        map(vec![("Field2", s("hello\tworld"))]),
    );
}

// internal/imported_tests TestUnmarshalMap, into a
// `map[string]interface{}` instead of a `map[string]int`.
#[test]
fn imported_map() {
    assert_decodes(
        "\n\t\ta = 1\n\t\tb = 2\n\t\tc = 3\n\t\t",
        map(vec![("a", int(1)), ("b", int(2)), ("c", int(3))]),
    );
}

// internal/imported_tests TestUnmarshalToNilInterface: the struct's
// `interface{}` fields hold what a `map[string]interface{}` holds.
#[test]
fn imported_to_nil_interface() {
    assert_decodes(
        "
PrimitiveField = \"Hello\"
ArrayField = [1,2,3]
InterfacePointerField = \"World\"

[StructField]
Field1 = 123
Field2 = \"Field2\"

[MapField]
MapField1 = [4,5,6]
MapField2 = {A = \"A\"}
MapField3 = false

[[StructArrayField]]
Name = \"Allen\"
Age = 20

[[StructArrayField]]
Name = \"Jack\"
Age = 23
",
        map(vec![
            ("PrimitiveField", s("Hello")),
            ("ArrayField", arr(vec![int(1), int(2), int(3)])),
            ("InterfacePointerField", s("World")),
            (
                "StructField",
                table(vec![("Field1", int(123)), ("Field2", s("Field2"))]),
            ),
            (
                "MapField",
                table(vec![
                    ("MapField1", arr(vec![int(4), int(5), int(6)])),
                    ("MapField2", table(vec![("A", s("A"))])),
                    ("MapField3", Value::Bool(false)),
                ]),
            ),
            (
                "StructArrayField",
                arr(vec![
                    table(vec![("Name", s("Allen")), ("Age", int(20))]),
                    table(vec![("Name", s("Jack")), ("Age", int(23))]),
                ]),
            ),
        ]),
    );
}

// internal/imported_tests TestUnmarshalMixedTypeSlice: an `[]interface{}`
// field; here the map's value.
#[test]
fn imported_mixed_type_slice() {
    assert_decodes(
        "ArrayField = [{Field = \"inner1\"},[{Field = \"inner2\"},{Field = \"inner3\"}]]\n",
        map(vec![(
            "ArrayField",
            arr(vec![
                table(vec![("Field", s("inner1"))]),
                arr(vec![
                    table(vec![("Field", s("inner2"))]),
                    table(vec![("Field", s("inner3"))]),
                ]),
            ]),
        )]),
    );
}
