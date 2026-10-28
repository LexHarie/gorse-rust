// Ports: errors.go, unstable/parser.go (ParserError, NewParserError), internal/danger/danger.go (SubsliceOffset) (tests: errors_test.go, internal/danger/danger_test.go) @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! go-toml's errors: the parser's `ParserError`, which points at the bytes of
//! the document it is about, and the `DecodeError` `Unmarshal` turns it into,
//! with the line, the column and a human-readable excerpt of the document.

use std::fmt;

use gorse_common::gostrconv;

/// `unstable.ParserError`: a message about a highlighted part of the
/// document. The highlight is a subslice of the document (Go's `Highlight`;
/// the optional `Key` is only set in strict mode, which viper does not use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParserError<'a> {
    /// `Highlight`.
    pub(crate) highlight: &'a [u8],
    /// `Message`.
    pub(crate) message: String,
}

/// `NewParserError`. Go formats the message with `fmt.Errorf`; callers here
/// format it with [`format!`] and the helpers below.
pub(crate) fn new_parser_error(highlight: &[u8], message: impl Into<String>) -> ParserError<'_> {
    ParserError {
        highlight,
        message: message.into(),
    }
}

/// Go's `%c` of a byte: the character with that code point.
pub(crate) fn fmt_c(b: u8) -> char {
    char::from(b)
}

/// Go's `%#U` of a byte: `U+0078 'x'`, without the quoted character when
/// `strconv.IsPrint` says it is not printable.
pub(crate) fn fmt_sharp_u(b: u8) -> String {
    let c = char::from(b);
    if gostrconv::is_print(c) {
        format!("U+{:04X} '{c}'", b)
    } else {
        format!("U+{:04X}", b)
    }
}

/// `danger.SubsliceOffset`: where `subslice` starts in `data`. Like Go it
/// compares addresses, and panics when `subslice` does not lie within `data`.
pub(crate) fn subslice_offset(data: &[u8], subslice: &[u8]) -> usize {
    let datap = data.as_ptr() as usize;
    let hlp = subslice.as_ptr() as usize;
    if hlp < datap {
        panic!("subslice address ({hlp}) is before data address ({datap})");
    }
    let offset = hlp - datap;
    if offset > data.len() {
        panic!(
            "slice offset ({offset}) is farther than data length ({})",
            data.len()
        );
    }
    if offset + subslice.len() > data.len() {
        panic!(
            "slice ends ({offset}+{}) is farther than data length ({})",
            subslice.len(),
            data.len()
        );
    }
    offset
}

/// go-toml's `*DecodeError`: a parser error with its position in the
/// document. Its text (`Error()`) is `toml: ` and the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    message: String,
    line: i64,
    column: i64,
    human: Vec<u8>,
}

impl DecodeError {
    /// The parser's message, without the `toml: ` prefix.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// `Position`: the 1-based line and column where the error is.
    pub fn position(&self) -> (i64, i64) {
        (self.line, self.column)
    }

    /// `String`: the error in context, a few numbered lines of the document
    /// around the highlighted bytes, underlined with `~`. These are the
    /// document's bytes, which need not be UTF-8.
    pub fn human(&self) -> &[u8] {
        &self.human
    }
}

/// `DecodeError.Error`.
impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "toml: {}", self.message)
    }
}

impl std::error::Error for DecodeError {}

/// `wrapDecodeError`: the DecodeError for a ParserError about document.
pub(crate) fn wrap_decode_error(document: &[u8], de: &ParserError<'_>) -> DecodeError {
    let offset = subslice_offset(document, de.highlight);

    let err_message = de.message.clone();
    let (err_line, err_column) = position_at_end(&document[..offset]);
    let (before, after) = lines_of_context(document, de.highlight, offset, 3);

    let mut buf: Vec<u8> = Vec::new();

    let max_line = err_line + after.len() as i64 - 1;
    let line_column_width = max_line.to_string().len();

    // Write the lines of context strictly before the error.
    for i in (1..before.len()).rev() {
        let line = err_line - i as i64;
        buf.extend_from_slice(format_line_number(line, line_column_width).as_bytes());
        buf.push(b'|');

        if !before[i].is_empty() {
            buf.push(b' ');
            buf.extend_from_slice(before[i]);
        }

        buf.push(b'\n');
    }

    // Write the document line that contains the error.

    buf.extend_from_slice(format_line_number(err_line, line_column_width).as_bytes());
    buf.extend_from_slice(b"| ");

    if let Some(first) = before.first() {
        buf.extend_from_slice(first);
    }

    buf.extend_from_slice(de.highlight);

    if let Some(first) = after.first() {
        buf.extend_from_slice(first);
    }

    buf.push(b'\n');

    // Write the line with the error message itself (so it does not have a
    // line number).

    buf.extend(std::iter::repeat_n(b' ', line_column_width));
    buf.extend_from_slice(b"| ");

    if let Some(first) = before.first() {
        buf.extend(std::iter::repeat_n(b' ', first.len()));
    }

    buf.extend(std::iter::repeat_n(b'~', de.highlight.len()));

    if !err_message.is_empty() {
        buf.push(b' ');
        buf.extend_from_slice(err_message.as_bytes());
    }

    // Write the lines of context strictly after the error.

    for (i, line_bytes) in after.iter().enumerate().skip(1) {
        buf.push(b'\n');
        let line = err_line + i as i64;
        buf.extend_from_slice(format_line_number(line, line_column_width).as_bytes());
        buf.push(b'|');

        if !line_bytes.is_empty() {
            buf.push(b' ');
            buf.extend_from_slice(line_bytes);
        }
    }

    DecodeError {
        message: err_message,
        line: err_line,
        column: err_column,
        human: buf,
    }
}

/// `formatLineNumber`: `%<width>d`.
fn format_line_number(line: i64, width: usize) -> String {
    format!("{line:>width$}")
}

/// `linesOfContext`.
fn lines_of_context<'d>(
    document: &'d [u8],
    highlight: &[u8],
    offset: usize,
    lines_around: usize,
) -> (Vec<&'d [u8]>, Vec<&'d [u8]>) {
    (
        before_lines(document, offset, lines_around),
        after_lines(document, highlight, offset, lines_around),
    )
}

/// `beforeLines`: the line of the highlight up to it, then the lines before,
/// walking back.
fn before_lines(document: &[u8], offset: usize, lines_around: usize) -> Vec<&[u8]> {
    let mut before_lines = Vec::new();

    // Walk the document backward from the highlight to find previous lines
    // of context.
    let mut rest = &document[..offset];
    let mut o = rest.len() as i64 - 1;
    while o >= 0 && before_lines.len() <= lines_around && !rest.is_empty() {
        let ou = o as usize;
        if rest[ou] == b'\n' {
            // handle individual lines
            before_lines.push(&rest[ou + 1..]);
            rest = &rest[..ou];
            o = rest.len() as i64 - 1;
        } else if ou == 0 {
            // add the first line only if it's non-empty
            before_lines.push(rest);
            break;
        } else {
            o -= 1;
        }
    }

    before_lines
}

/// `afterLines`: the rest of the highlight's line, then the lines after.
fn after_lines<'d>(
    document: &'d [u8],
    highlight: &[u8],
    offset: usize,
    lines_around: usize,
) -> Vec<&'d [u8]> {
    let mut after_lines = Vec::new();

    // Walk the document forward from the highlight to find the following
    // lines of context.
    let mut rest = &document[offset + highlight.len()..];
    let mut o = 0;
    while o < rest.len() && after_lines.len() <= lines_around {
        if rest[o] == b'\n' {
            // handle individual lines
            after_lines.push(&rest[..o]);
            rest = &rest[o + 1..];
            o = 0;
        } else if o == rest.len() - 1 {
            // add last line only if it's non-empty
            after_lines.push(rest);
            break;
        } else {
            o += 1;
        }
    }

    after_lines
}

/// `positionAtEnd`: the 1-based line and column just after b.
fn position_at_end(b: &[u8]) -> (i64, i64) {
    let mut row = 1;
    let mut column = 1;

    for &c in b {
        if c == b'\n' {
            row += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    (row, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharp_u_matches_go() {
        // Recorded from Go: fmt.Sprintf("%#U", byte(c)).
        assert_eq!(fmt_sharp_u(b'x'), "U+0078 'x'");
        assert_eq!(fmt_sharp_u(b'1'), "U+0031 '1'");
        assert_eq!(fmt_sharp_u(b'\n'), "U+000A");
        assert_eq!(fmt_sharp_u(b' '), "U+0020 ' '");
        assert_eq!(fmt_sharp_u(b'\''), "U+0027 '''");
        assert_eq!(fmt_sharp_u(0x7F), "U+007F");
        assert_eq!(fmt_sharp_u(0xA0), "U+00A0");
        assert_eq!(fmt_sharp_u(0xAD), "U+00AD");
        assert_eq!(fmt_sharp_u(0xEF), "U+00EF 'ï'");
    }

    #[test]
    fn subslice_offset_of_subslices() {
        let doc = b"hello world";
        assert_eq!(subslice_offset(doc, &doc[6..]), 6);
        assert_eq!(subslice_offset(doc, &doc[11..]), 11);
        assert_eq!(subslice_offset(doc, &doc[..0]), 0);
    }

    #[test]
    #[should_panic(expected = "is before data address")]
    fn subslice_offset_panics_outside() {
        let doc = b"hello world";
        subslice_offset(&doc[1..], &doc[..1]);
    }

    // Ported from internal/danger/danger_test.go TestSubsliceOffsetValid.
    #[test]
    fn subslice_offset_valid() {
        let data = b"hello";
        assert_eq!(subslice_offset(data, &data[1..]), 1);
    }

    // Ported from internal/danger/danger_test.go TestSubsliceOffsetInvalid.
    #[test]
    fn subslice_offset_invalid() {
        let one = b"one".to_vec();
        let two = b"two".to_vec();
        let full = b"hello world";
        let examples: [(&str, &[u8], &[u8]); 4] = [
            ("unrelated arrays", &one, &two),
            ("slice starts before data", &full[5..], &full[1..]),
            ("slice starts after data", &full[..3], &full[5..]),
            ("slice ends after data", &full[..5], &full[3..8]),
        ];
        for (desc, d, s) in examples {
            let result = std::panic::catch_unwind(|| subslice_offset(d, s));
            assert!(result.is_err(), "{desc}: no panic");
        }
    }

    /// The `*DecodeError` for a document made of three parts, the middle
    /// one highlighted.
    fn decode_error(doc: [&str; 3], msg: &str) -> DecodeError {
        let document = [doc[0], doc[1], doc[2]].concat();
        let start = doc[0].len();
        let end = start + doc[1].len();
        let document = document.as_bytes();
        wrap_decode_error(document, &new_parser_error(&document[start..end], msg))
    }

    // Ported from errors_test.go TestDecodeError: the human-readable
    // context of a DecodeError (`String()`).
    #[test]
    fn decode_error_context() {
        let examples: [(&str, [&str; 3], &str, &str); 8] = [
            (
                "no context",
                ["", "morning", ""],
                "this is wrong",
                "
1| morning
 | ~~~~~~~ this is wrong",
            ),
            (
                "one line",
                ["good ", "morning", " everyone"],
                "this is wrong",
                "
1| good morning everyone
 |      ~~~~~~~ this is wrong",
            ),
            (
                "exactly 3 lines",
                [
                    "line1
line2
line3
before ",
                    "highlighted",
                    " after
post line 1
post line 2
post line 3",
                ],
                "this is wrong",
                "
1| line1
2| line2
3| line3
4| before highlighted after
 |        ~~~~~~~~~~~ this is wrong
5| post line 1
6| post line 2
7| post line 3",
            ),
            (
                "more than 3 lines",
                [
                    "should not be seen1
should not be seen2
line1
line2
line3
before ",
                    "highlighted",
                    " after
post line 1
post line 2
post line 3
should not be seen3
should not be seen4",
                ],
                "this is wrong",
                "
3| line1
4| line2
5| line3
6| before highlighted after
 |        ~~~~~~~~~~~ this is wrong
7| post line 1
8| post line 2
9| post line 3",
            ),
            (
                "more than 10 total lines",
                [
                    "should not be seen 0
should not be seen1
should not be seen2
should not be seen3
line1
line2
line3
before ",
                    "highlighted",
                    " after
post line 1
post line 2
post line 3
should not be seen3
should not be seen4",
                ],
                "this is wrong",
                "
 5| line1
 6| line2
 7| line3
 8| before highlighted after
  |        ~~~~~~~~~~~ this is wrong
 9| post line 1
10| post line 2
11| post line 3",
            ),
            (
                "last line of more than 10",
                [
                    "should not be seen
should not be seen
should not be seen
should not be seen
should not be seen
should not be seen
should not be seen
line1
line2
line3
before ",
                    "highlighted",
                    "",
                ],
                "this is wrong",
                "
 8| line1
 9| line2
10| line3
11| before highlighted
  |        ~~~~~~~~~~~ this is wrong
",
            ),
            (
                "handle empty lines in the before/after blocks",
                [
                    "line1

line 2
before ",
                    "highlighted",
                    " after
line 3

line 4
line 5",
                ],
                "",
                "1| line1
2|
3| line 2
4| before highlighted after
 |        ~~~~~~~~~~~
5| line 3
6|
7| line 4",
            ),
            (
                "handle remainder of the error line when there is only one line",
                ["P=", "[", "#"],
                "array is incomplete",
                "1| P=[#
 |   ~ array is incomplete",
            ),
        ];
        for (desc, doc, msg, expected) in examples {
            let derr = decode_error(doc, msg);
            assert_eq!(
                String::from_utf8_lossy(derr.human()),
                expected.trim_matches('\n'),
                "{desc}"
            );
        }
    }

    // Ported from errors_test.go TestDecodeError_Accessors, without `Key`
    // (only strict mode sets it, and viper does not use strict mode).
    #[test]
    fn decode_error_accessors() {
        let e = DecodeError {
            message: "foo".to_owned(),
            line: 1,
            column: 2,
            human: b"bar".to_vec(),
        };
        assert_eq!(e.to_string(), "toml: foo");
        assert_eq!(e.position(), (1, 2));
        assert_eq!(e.human(), b"bar");
    }
}
