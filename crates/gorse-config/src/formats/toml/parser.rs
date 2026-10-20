// Ports: unstable/parser.go, unstable/scanner.go (tests: unstable/parser_test.go) @ github.com/pelletier/go-toml/v2 v2.2.4
//
// Derived from go-toml v2: Copyright (c) 2021 - 2023 Thomas Pelletier. Use of
// this source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/GO-TOML-LICENSE.

//! go-toml's parser: it scans one top-level expression at a time
//! ([`Parser::next_expression`]) into an AST of [`ast::Node`]s.
//!
//! As in Go, every function takes the rest of the document as a byte slice
//! and returns what remains after what it read; errors highlight a subslice
//! of the document. Node data borrows from the document, except for strings
//! with escape sequences, which are decoded into a buffer of their own.
//! Comments are scanned and dropped: `Unmarshal` never sets `KeepComments`.

use std::borrow::Cow;

use super::ast::{Builder, Kind, Node, Range, Reference};
use super::characters::{utf8_toml_valid_already_escaped, utf8_valid_next};
use super::errors::{ParserError, fmt_c, fmt_sharp_u, new_parser_error, subslice_offset};
use super::with_stack;

/// What most parse functions return: a node and the rest of the document.
type Parsed<'a, T> = Result<(T, &'a [u8]), ParserError<'a>>;

/// A string's raw token, its value and the rest of the document.
type ParsedString<'a> = Result<(&'a [u8], Cow<'a, [u8]>, &'a [u8]), ParserError<'a>>;

/// `Parser`.
#[derive(Debug)]
pub(crate) struct Parser<'a> {
    data: &'a [u8],
    builder: Builder<'a>,
    ref_: Reference,
    left: &'a [u8],
    err: Option<ParserError<'a>>,
    first: bool,
}

impl<'a> Parser<'a> {
    /// `Reset`: a parser at the start of b.
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Parser {
            data: b,
            builder: Builder::default(),
            ref_: None,
            left: b,
            err: None,
            first: true,
        }
    }

    /// `Data`.
    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The nodes of the last expression.
    pub(crate) fn builder(&self) -> &Builder<'a> {
        &self.builder
    }

    /// `Range`: the range of b, a subslice of the document.
    fn range(&self, b: &[u8]) -> Range {
        Range {
            offset: subslice_offset(self.data, b) as u32,
            length: b.len() as u32,
        }
    }

    /// `Raw`: the bytes of a range.
    pub(crate) fn raw(&self, raw: Range) -> &'a [u8] {
        let start = raw.offset as usize;
        &self.data[start..start + raw.length as usize]
    }

    /// `NextExpression`: parses the next top-level expression and says
    /// whether there is one. False at the end of the document or on an
    /// error ([`Parser::error`]).
    pub(crate) fn next_expression(&mut self) -> bool {
        if self.left.is_empty() || self.err.is_some() {
            return false;
        }

        self.builder.reset();
        self.ref_ = None;

        loop {
            if self.left.is_empty() || self.err.is_some() {
                return false;
            }

            if !self.first {
                match self.parse_newline(self.left) {
                    Ok(rest) => self.left = rest,
                    Err(err) => {
                        self.left = &[];
                        self.err = Some(err);
                    }
                }
            }

            if self.left.is_empty() || self.err.is_some() {
                return false;
            }

            match self.parse_expression(self.left) {
                Ok((r, rest)) => {
                    self.ref_ = r;
                    self.left = rest;
                }
                Err(err) => {
                    self.left = &[];
                    self.err = Some(err);
                    return false;
                }
            }

            self.first = false;

            if self.ref_.is_some() {
                return true;
            }
        }
    }

    /// `Expression`: the index of the last parsed expression's node.
    pub(crate) fn expression(&self) -> usize {
        match self.ref_ {
            Some(r) => r,
            None => panic!("runtime error: index out of range [-1]"),
        }
    }

    /// `Error`.
    pub(crate) fn error(&self) -> Option<&ParserError<'a>> {
        self.err.as_ref()
    }

    fn parse_newline(&self, b: &'a [u8]) -> Result<&'a [u8], ParserError<'a>> {
        if b[0] == b'\n' {
            return Ok(&b[1..]);
        }

        if b[0] == b'\r' {
            let (_, rest) = scan_windows_newline(b)?;
            return Ok(rest);
        }

        Err(new_parser_error(
            &b[0..1],
            format!("expected newline but got {}", fmt_sharp_u(b[0])),
        ))
    }

    /// `parseComment`. Without `KeepComments` no node is made.
    fn parse_comment(&self, b: &'a [u8]) -> Parsed<'a, Reference> {
        let (_, rest) = scan_comment(b)?;
        Ok((None, rest))
    }

    fn parse_expression(&mut self, b: &'a [u8]) -> Parsed<'a, Reference> {
        // expression =  ws [ comment ]
        // expression =/ ws keyval ws [ comment ]
        // expression =/ ws table ws [ comment ]
        let b = parse_whitespace(b);

        if b.is_empty() {
            return Ok((None, b));
        }

        if b[0] == b'#' {
            return self.parse_comment(b);
        }

        if b[0] == b'\n' || b[0] == b'\r' {
            return Ok((None, b));
        }

        let (r, b) = if b[0] == b'[' {
            self.parse_table(b)?
        } else {
            self.parse_keyval(b)?
        };

        let b = parse_whitespace(b);

        if !b.is_empty() && b[0] == b'#' {
            // The comment would be chained to the expression with
            // KeepComments.
            let (_, rest) = self.parse_comment(b)?;
            return Ok((Some(r), rest));
        }

        Ok((Some(r), b))
    }

    fn parse_table(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // table = std-table / array-table
        if b.len() > 1 && b[1] == b'[' {
            return self.parse_array_table(b);
        }

        self.parse_std_table(b)
    }

    fn parse_array_table(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // array-table = array-table-open key array-table-close
        // array-table-open  = %x5B.5B ws  ; [[ Double left square bracket
        // array-table-close = ws %x5D.5D  ; ]] Double right square bracket
        let r = self.builder.push(Node::new(
            Kind::ArrayTable,
            Range::default(),
            Cow::Borrowed(&[]),
        ));

        let b = parse_whitespace(&b[2..]);

        let (k, b) = self.parse_key(b)?;

        self.builder.attach_child(r, k);
        let b = parse_whitespace(b);

        let b = expect(b']', b)?;

        let b = expect(b']', b)?;

        Ok((r, b))
    }

    fn parse_std_table(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // std-table = std-table-open key std-table-close
        // std-table-open  = %x5B ws     ; [ Left square bracket
        // std-table-close = ws %x5D     ; ] Right square bracket
        let r = self
            .builder
            .push(Node::new(Kind::Table, Range::default(), Cow::Borrowed(&[])));

        let b = parse_whitespace(&b[1..]);

        let (key, b) = self.parse_key(b)?;

        self.builder.attach_child(r, key);

        let b = parse_whitespace(b);

        let b = expect(b']', b)?;

        Ok((r, b))
    }

    fn parse_keyval(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // keyval = key keyval-sep val
        let r = self.builder.push(Node::new(
            Kind::KeyValue,
            Range::default(),
            Cow::Borrowed(&[]),
        ));

        let (key, b) = self.parse_key(b)?;

        // keyval-sep = ws %x3D ws ; =

        let b = parse_whitespace(b);

        if b.is_empty() {
            return Err(new_parser_error(
                b,
                "expected = after a key, but the document ends there",
            ));
        }

        let b = expect(b'=', b)?;

        let b = parse_whitespace(b);

        let (val_ref, b) = self.parse_val(b)?;

        self.builder.chain(val_ref, key);
        self.builder.attach_child(r, val_ref);

        Ok((r, b))
    }

    /// `parseVal`, on a stack with room for nested arrays and inline tables.
    fn parse_val(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        with_stack(|| self.parse_val_inner(b))
    }

    fn parse_val_inner(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // val = string / boolean / array / inline-table / date-time / float / integer
        if b.is_empty() {
            return Err(new_parser_error(b, "expected value, not eof"));
        }

        match b[0] {
            b'"' => {
                let (raw, v, b) = if scan_follows_multiline_basic_string_delimiter(b) {
                    self.parse_multiline_basic_string(b)?
                } else {
                    self.parse_basic_string(b)?
                };

                let r = self
                    .builder
                    .push(Node::new(Kind::String, self.range(raw), v));

                Ok((r, b))
            }
            b'\'' => {
                let (raw, v, b) = if scan_follows_multiline_literal_string_delimiter(b) {
                    self.parse_multiline_literal_string(b)?
                } else {
                    self.parse_literal_string(b)?
                };

                let r = self
                    .builder
                    .push(Node::new(Kind::String, self.range(raw), v));

                Ok((r, b))
            }
            b't' => {
                if !scan_follows_true(b) {
                    return Err(new_parser_error(atmost(b, 4), "expected 'true'"));
                }

                let r = self.builder.push(Node::new(
                    Kind::Bool,
                    Range::default(),
                    Cow::Borrowed(&b[..4]),
                ));

                Ok((r, &b[4..]))
            }
            b'f' => {
                if !scan_follows_false(b) {
                    return Err(new_parser_error(atmost(b, 5), "expected 'false'"));
                }

                let r = self.builder.push(Node::new(
                    Kind::Bool,
                    Range::default(),
                    Cow::Borrowed(&b[..5]),
                ));

                Ok((r, &b[5..]))
            }
            b'[' => self.parse_val_array(b),
            b'{' => self.parse_inline_table(b),
            _ => self.parse_int_or_float_or_date_time(b),
        }
    }

    fn parse_literal_string(&self, b: &'a [u8]) -> ParsedString<'a> {
        let (v, rest) = scan_literal_string(b)?;

        Ok((v, Cow::Borrowed(&v[1..v.len() - 1]), rest))
    }

    fn parse_inline_table(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // inline-table = inline-table-open [ inline-table-keyvals ] inline-table-close
        // inline-table-open  = %x7B ws     ; {
        // inline-table-close = ws %x7D     ; }
        // inline-table-sep   = ws %x2C ws  ; , Comma
        // inline-table-keyvals = keyval [ inline-table-sep inline-table-keyvals ]
        let parent = self.builder.push(Node::new(
            Kind::InlineTable,
            self.range(&b[..1]),
            Cow::Borrowed(&[]),
        ));

        let mut first = true;

        // Go's `var child reference` starts at 0; it is read only after the
        // first key-value set it.
        let mut child = 0;

        let mut b = &b[1..];

        while !b.is_empty() {
            let previous_b = b;
            b = parse_whitespace(b);

            if b.is_empty() {
                return Err(new_parser_error(
                    &previous_b[..1],
                    "inline table is incomplete",
                ));
            }

            if b[0] == b'}' {
                break;
            }

            if !first {
                b = expect(b',', b)?;
                b = parse_whitespace(b);
            }

            let (kv, rest) = self.parse_keyval(b)?;
            b = rest;

            if first {
                self.builder.attach_child(parent, kv);
            } else {
                self.builder.chain(child, kv);
            }
            child = kv;

            first = false;
        }

        let rest = expect(b'}', b)?;

        Ok((parent, rest))
    }

    fn parse_val_array(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // array = array-open [ array-values ] ws-comment-newline array-close
        // array-open =  %x5B ; [
        // array-close = %x5D ; ]
        // array-values =  ws-comment-newline val ws-comment-newline array-sep array-values
        // array-values =/ ws-comment-newline val ws-comment-newline [ array-sep ]
        // array-sep = %x2C  ; , Comma
        // ws-comment-newline = *( wschar / [ comment ] newline )
        let array_start = b;
        let mut b = &b[1..];

        let parent =
            self.builder
                .push(Node::new(Kind::Array, Range::default(), Cow::Borrowed(&[])));

        // First indicates whether the parser is looking for the first element
        // (non-comment) of the array.
        let mut first = true;

        let mut last_child: Reference = None;

        // (With KeepComments, the comments would be children too.)
        while !b.is_empty() {
            b = self.parse_optional_whitespace_comment_newline(b)?;

            if b.is_empty() {
                return Err(new_parser_error(&array_start[..1], "array is incomplete"));
            }

            if b[0] == b']' {
                break;
            }

            if b[0] == b',' {
                if first {
                    return Err(new_parser_error(&b[0..1], "array cannot start with comma"));
                }
                b = &b[1..];

                b = self.parse_optional_whitespace_comment_newline(b)?;
            } else if !first {
                return Err(new_parser_error(
                    &b[0..1],
                    "array elements must be separated by commas",
                ));
            }

            // TOML allows trailing commas in arrays.
            if !b.is_empty() && b[0] == b']' {
                break;
            }

            let (value_ref, rest) = self.parse_val(b)?;
            b = rest;

            match last_child {
                None => self.builder.attach_child(parent, value_ref),
                Some(last) => self.builder.chain(last, value_ref),
            }
            last_child = Some(value_ref);

            b = self.parse_optional_whitespace_comment_newline(b)?;

            first = false;
        }

        let rest = expect(b']', b)?;

        Ok((parent, rest))
    }

    /// `parseOptionalWhitespaceCommentNewline`, which would also return the
    /// comments it read with KeepComments.
    fn parse_optional_whitespace_comment_newline(
        &self,
        mut b: &'a [u8],
    ) -> Result<&'a [u8], ParserError<'a>> {
        while !b.is_empty() {
            b = parse_whitespace(b);

            if !b.is_empty() && b[0] == b'#' {
                let (_, rest) = self.parse_comment(b)?;
                b = rest;
            }

            if b.is_empty() {
                break;
            }

            if b[0] == b'\n' || b[0] == b'\r' {
                b = self.parse_newline(b)?;
            } else {
                break;
            }
        }

        Ok(b)
    }

    fn parse_multiline_literal_string(&self, b: &'a [u8]) -> ParsedString<'a> {
        let (token, rest) = scan_multiline_literal_string(b)?;

        let mut i = 3;

        // skip the immediate new line
        if token[i] == b'\n' {
            i += 1;
        } else if token[i] == b'\r' && token[i + 1] == b'\n' {
            i += 2;
        }

        Ok((token, Cow::Borrowed(&token[i..token.len() - 3]), rest))
    }

    fn parse_multiline_basic_string(&self, b: &'a [u8]) -> ParsedString<'a> {
        // ml-basic-string = ml-basic-string-delim [ newline ] ml-basic-body
        // ml-basic-string-delim
        // ml-basic-string-delim = 3quotation-mark
        // ml-basic-body = *mlb-content *( mlb-quotes 1*mlb-content ) [ mlb-quotes ]
        //
        // mlb-content = mlb-char / newline / mlb-escaped-nl
        // mlb-char = mlb-unescaped / escaped
        // mlb-quotes = 1*2quotation-mark
        // mlb-unescaped = wschar / %x21 / %x23-5B / %x5D-7E / non-ascii
        // mlb-escaped-nl = escape ws newline *( wschar / newline )
        let (token, escaped, rest) = scan_multiline_basic_string(b)?;

        let mut i = 3;

        // skip the immediate new line
        if token[i] == b'\n' {
            i += 1;
        } else if token[i] == b'\r' && token[i + 1] == b'\n' {
            i += 2;
        }

        // fast path
        let start_idx = i;
        let end_idx = token.len() - 3;

        if !escaped {
            let s = &token[start_idx..end_idx];
            let verr = utf8_toml_valid_already_escaped(s);
            if verr.zero() {
                return Ok((token, Cow::Borrowed(s), rest));
            }
            return Err(new_parser_error(
                &s[verr.index..verr.index + verr.size],
                "invalid UTF-8",
            ));
        }

        let mut builder: Vec<u8> = Vec::new();

        // The scanner ensures that the token starts and ends with quotes and
        // that escapes are balanced.
        while i < token.len() - 3 {
            let c = token[i];

            if c == b'\\' {
                // When the last non-whitespace character on a line is an
                // unescaped \, it will be trimmed along with all whitespace
                // (including newlines) up to the next non-whitespace
                // character or closing delimiter.

                let mut is_last_non_whitespace_on_line = false;
                let mut j = 1;
                while j < token.len() - 3 - i {
                    match token[i + j] {
                        b' ' | b'\t' => {
                            j += 1;
                            continue;
                        }
                        b'\r' => {
                            if token[i + j + 1] == b'\n' {
                                j += 1;
                                continue;
                            }
                        }
                        b'\n' => is_last_non_whitespace_on_line = true,
                        _ => {}
                    }
                    break;
                }
                if is_last_non_whitespace_on_line {
                    i += j;
                    while i < token.len() - 3 {
                        let c = token[i];
                        if !(c == b'\n' || c == b'\r' || c == b' ' || c == b'\t') {
                            i -= 1;
                            break;
                        }
                        i += 1;
                    }
                    i += 1;
                    continue;
                }

                // handle escaping
                i += 1;
                let c = token[i];

                match c {
                    b'"' | b'\\' => builder.push(c),
                    b'b' => builder.push(b'\x08'),
                    b'f' => builder.push(b'\x0c'),
                    b'n' => builder.push(b'\n'),
                    b'r' => builder.push(b'\r'),
                    b't' => builder.push(b'\t'),
                    b'e' => builder.push(0x1B),
                    b'u' => {
                        let x = hex_to_rune(atmost(&token[i + 1..], 4), 4)?;
                        push_rune(&mut builder, x);
                        i += 4;
                    }
                    b'U' => {
                        let x = hex_to_rune(atmost(&token[i + 1..], 8), 8)?;

                        push_rune(&mut builder, x);
                        i += 8;
                    }
                    _ => {
                        return Err(new_parser_error(
                            &token[i..i + 1],
                            format!("invalid escaped character {}", fmt_sharp_u(c)),
                        ));
                    }
                }
                i += 1;
            } else {
                let size = utf8_valid_next(&token[i..]);
                if size == 0 {
                    return Err(new_parser_error(
                        &token[i..i + 1],
                        format!("invalid character {}", fmt_sharp_u(c)),
                    ));
                }
                builder.extend_from_slice(&token[i..i + size]);
                i += size;
            }
        }

        Ok((token, Cow::Owned(builder), rest))
    }

    fn parse_key(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        // key = simple-key / dotted-key
        // simple-key = quoted-key / unquoted-key
        //
        // unquoted-key = 1*( ALPHA / DIGIT / %x2D / %x5F ) ; A-Z / a-z / 0-9 / - / _
        // quoted-key = basic-string / literal-string
        // dotted-key = simple-key 1*( dot-sep simple-key )
        //
        // dot-sep   = ws %x2E ws  ; . Period
        let (raw, key, mut b) = self.parse_simple_key(b)?;

        let r = self
            .builder
            .push(Node::new(Kind::Key, self.range(raw), key));

        loop {
            b = parse_whitespace(b);
            if !b.is_empty() && b[0] == b'.' {
                b = parse_whitespace(&b[1..]);

                let (raw, key, rest) = self.parse_simple_key(b)?;
                b = rest;

                let node = Node::new(Kind::Key, self.range(raw), key);
                self.builder.push_and_chain(node);
            } else {
                break;
            }
        }

        Ok((r, b))
    }

    fn parse_simple_key(&self, b: &'a [u8]) -> ParsedString<'a> {
        if b.is_empty() {
            return Err(new_parser_error(b, "expected key but found none"));
        }

        // simple-key = quoted-key / unquoted-key
        // unquoted-key = 1*( ALPHA / DIGIT / %x2D / %x5F ) ; A-Z / a-z / 0-9 / - / _
        // quoted-key = basic-string / literal-string
        match b[0] {
            b'\'' => self.parse_literal_string(b),
            b'"' => self.parse_basic_string(b),
            c if is_unquoted_key_char(c) => {
                let (key, rest) = scan_unquoted_key(b);
                Ok((key, Cow::Borrowed(key), rest))
            }
            c => Err(new_parser_error(
                &b[0..1],
                format!("invalid character at start of key: {}", fmt_c(c)),
            )),
        }
    }

    fn parse_basic_string(&self, b: &'a [u8]) -> ParsedString<'a> {
        // basic-string = quotation-mark *basic-char quotation-mark
        // quotation-mark = %x22            ; "
        // basic-char = basic-unescaped / escaped
        // basic-unescaped = wschar / %x21 / %x23-5B / %x5D-7E / non-ascii
        // escaped = escape escape-seq-char
        // escape-seq-char =  %x22         ; "    quotation mark  U+0022
        // escape-seq-char =/ %x5C         ; \    reverse solidus U+005C
        // escape-seq-char =/ %x62         ; b    backspace       U+0008
        // escape-seq-char =/ %x66         ; f    form feed       U+000C
        // escape-seq-char =/ %x6E         ; n    line feed       U+000A
        // escape-seq-char =/ %x72         ; r    carriage return U+000D
        // escape-seq-char =/ %x74         ; t    tab             U+0009
        // escape-seq-char =/ %x75 4HEXDIG ; uXXXX                U+XXXX
        // escape-seq-char =/ %x55 8HEXDIG ; UXXXXXXXX            U+XXXXXXXX
        let (token, escaped, rest) = scan_basic_string(b)?;

        let start_idx = 1;
        let end_idx = token.len() - 1;

        // Fast path. If there is no escape sequence, the string should just
        // be an UTF-8 encoded string, which is the same as Go. In that case,
        // validate the string and return a direct reference to the buffer.
        if !escaped {
            let s = &token[start_idx..end_idx];
            let verr = utf8_toml_valid_already_escaped(s);
            if verr.zero() {
                return Ok((token, Cow::Borrowed(s), rest));
            }
            return Err(new_parser_error(
                &s[verr.index..verr.index + verr.size],
                "invalid UTF-8",
            ));
        }

        let mut i = start_idx;

        let mut builder: Vec<u8> = Vec::new();

        // The scanner ensures that the token starts and ends with quotes and
        // that escapes are balanced.
        while i < token.len() - 1 {
            let c = token[i];
            if c == b'\\' {
                i += 1;
                let c = token[i];

                match c {
                    b'"' | b'\\' => builder.push(c),
                    b'b' => builder.push(b'\x08'),
                    b'f' => builder.push(b'\x0c'),
                    b'n' => builder.push(b'\n'),
                    b'r' => builder.push(b'\r'),
                    b't' => builder.push(b'\t'),
                    b'e' => builder.push(0x1B),
                    b'u' => {
                        let x = hex_to_rune(&token[i + 1..token.len() - 1], 4)?;

                        push_rune(&mut builder, x);
                        i += 4;
                    }
                    b'U' => {
                        let x = hex_to_rune(&token[i + 1..token.len() - 1], 8)?;

                        push_rune(&mut builder, x);
                        i += 8;
                    }
                    _ => {
                        return Err(new_parser_error(
                            &token[i..i + 1],
                            format!("invalid escaped character {}", fmt_sharp_u(c)),
                        ));
                    }
                }
                i += 1;
            } else {
                let size = utf8_valid_next(&token[i..]);
                if size == 0 {
                    return Err(new_parser_error(
                        &token[i..i + 1],
                        format!("invalid character {}", fmt_sharp_u(c)),
                    ));
                }
                builder.extend_from_slice(&token[i..i + size]);
                i += size;
            }
        }

        Ok((token, Cow::Owned(builder), rest))
    }

    fn parse_int_or_float_or_date_time(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        match b[0] {
            b'i' => {
                if !scan_follows_inf(b) {
                    return Err(new_parser_error(atmost(b, 3), "expected 'inf'"));
                }

                let r = self.builder.push(Node::new(
                    Kind::Float,
                    self.range(&b[..3]),
                    Cow::Borrowed(&b[..3]),
                ));
                return Ok((r, &b[3..]));
            }
            b'n' => {
                if !scan_follows_nan(b) {
                    return Err(new_parser_error(atmost(b, 3), "expected 'nan'"));
                }

                let r = self.builder.push(Node::new(
                    Kind::Float,
                    self.range(&b[..3]),
                    Cow::Borrowed(&b[..3]),
                ));
                return Ok((r, &b[3..]));
            }
            b'+' | b'-' => return self.scan_int_or_float(b),
            _ => {}
        }

        if b.len() < 3 {
            return self.scan_int_or_float(b);
        }

        let s = b.len().min(5);

        for (idx, &c) in b[..s].iter().enumerate() {
            if is_digit(c) {
                continue;
            }

            if idx == 2 && c == b':' || (idx == 4 && c == b'-') {
                return Ok(self.scan_date_time(b));
            }

            break;
        }

        self.scan_int_or_float(b)
    }

    /// `scanDateTime`: scans for contiguous characters in [0-9T:Z.+-], and up
    /// to one space if followed by a digit. Never fails.
    fn scan_date_time(&mut self, b: &'a [u8]) -> (usize, &'a [u8]) {
        let mut has_date = false;
        let mut has_time = false;
        let mut has_tz = false;
        let mut seen_space = false;

        let mut i = 0;
        while i < b.len() {
            let c = b[i];

            match c {
                c if is_digit(c) => {}
                b'-' => {
                    has_date = true;
                    const MIN_OFFSET_OF_TZ: usize = 8;
                    if i >= MIN_OFFSET_OF_TZ {
                        has_tz = true;
                    }
                }
                b'T' | b't' | b':' | b'.' => has_time = true,
                // ('-' never gets here: the case above takes it.)
                b'+' | b'Z' | b'z' => has_tz = true,
                b' ' => {
                    if !seen_space && i + 1 < b.len() && is_digit(b[i + 1]) {
                        i += 2;
                        // Avoid reaching past the end of the document in
                        // case the time is malformed. See TestIssue585.
                        if i >= b.len() {
                            i -= 1;
                        }
                        seen_space = true;
                        has_time = true;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
            i += 1;
        }

        let kind = if has_time {
            if has_date {
                if has_tz {
                    Kind::DateTime
                } else {
                    Kind::LocalDateTime
                }
            } else {
                Kind::LocalTime
            }
        } else {
            Kind::LocalDate
        };

        let r = self
            .builder
            .push(Node::new(kind, Range::default(), Cow::Borrowed(&b[..i])));
        (r, &b[i..])
    }

    fn scan_int_or_float(&mut self, b: &'a [u8]) -> Parsed<'a, usize> {
        let mut i = 0;

        if b.len() > 2 && b[0] == b'0' && b[1] != b'.' && b[1] != b'e' && b[1] != b'E' {
            let is_valid_rune: Option<fn(u8) -> bool> = match b[1] {
                b'x' => Some(is_valid_hex_rune),
                b'o' => Some(is_valid_octal_rune),
                b'b' => Some(is_valid_binary_rune),
                _ => {
                    i += 1;
                    None
                }
            };

            if let Some(is_valid_rune) = is_valid_rune {
                i += 2;
                while i < b.len() {
                    if !is_valid_rune(b[i]) {
                        break;
                    }
                    i += 1;
                }
            }

            let r = self.builder.push(Node::new(
                Kind::Integer,
                self.range(&b[..i]),
                Cow::Borrowed(&b[..i]),
            ));
            return Ok((r, &b[i..]));
        }

        let mut is_float = false;

        while i < b.len() {
            let c = b[i];

            if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'_' {
                i += 1;
                continue;
            }

            if c == b'.' || c == b'e' || c == b'E' {
                is_float = true;

                i += 1;
                continue;
            }

            if c == b'i' {
                if scan_follows_inf(&b[i..]) {
                    let r = self.builder.push(Node::new(
                        Kind::Float,
                        self.range(&b[..i + 3]),
                        Cow::Borrowed(&b[..i + 3]),
                    ));
                    return Ok((r, &b[i + 3..]));
                }

                return Err(new_parser_error(
                    &b[i..i + 1],
                    "unexpected character 'i' while scanning for a number",
                ));
            }

            if c == b'n' {
                if scan_follows_nan(&b[i..]) {
                    let r = self.builder.push(Node::new(
                        Kind::Float,
                        self.range(&b[..i + 3]),
                        Cow::Borrowed(&b[..i + 3]),
                    ));
                    return Ok((r, &b[i + 3..]));
                }

                return Err(new_parser_error(
                    &b[i..i + 1],
                    "unexpected character 'n' while scanning for a number",
                ));
            }

            break;
        }

        if i == 0 {
            return Err(new_parser_error(b, "incomplete number"));
        }

        let kind = if is_float { Kind::Float } else { Kind::Integer };

        let r = self
            .builder
            .push(Node::new(kind, self.range(&b[..i]), Cow::Borrowed(&b[..i])));
        Ok((r, &b[i..]))
    }
}

/// Go's `bytes.Buffer.WriteRune` of a valid rune.
fn push_rune(builder: &mut Vec<u8>, r: char) {
    let mut tmp = [0; 4];
    builder.extend_from_slice(r.encode_utf8(&mut tmp).as_bytes());
}

fn atmost(b: &[u8], n: usize) -> &[u8] {
    if n >= b.len() {
        return b;
    }

    &b[..n]
}

/// `hexToRune`: the code point of the first `length` hex digits of b.
fn hex_to_rune(b: &[u8], length: usize) -> Result<char, ParserError<'_>> {
    if b.len() < length {
        return Err(new_parser_error(
            b,
            format!("unicode point needs {length} character, not {}", b.len()),
        ));
    }
    let b = &b[..length];

    let mut r: u32 = 0;
    for (i, &c) in b.iter().enumerate() {
        let d = match c {
            b'0'..=b'9' => u32::from(c - b'0'),
            b'a'..=b'f' => u32::from(c - b'a' + 10),
            b'A'..=b'F' => u32::from(c - b'A' + 10),
            _ => return Err(new_parser_error(&b[i..i + 1], "non-hex character")),
        };
        // Eight hex digits fit in Go's uint32.
        r = r * 16 + d;
    }

    // `r > unicode.MaxRune || 0xD800 <= r && r < 0xE000`
    match char::from_u32(r) {
        Some(c) => Ok(c),
        None => Err(new_parser_error(
            b,
            "escape sequence is invalid Unicode code point",
        )),
    }
}

/// `parseWhitespace`: skips spaces and tabs.
fn parse_whitespace(b: &[u8]) -> &[u8] {
    // ws = *wschar
    // wschar =  %x20  ; Space
    // wschar =/ %x09  ; Horizontal tab
    let (_, rest) = scan_whitespace(b);

    rest
}

fn is_digit(r: u8) -> bool {
    r.is_ascii_digit()
}

fn is_valid_hex_rune(r: u8) -> bool {
    r.is_ascii_hexdigit() || r == b'_'
}

fn is_valid_octal_rune(r: u8) -> bool {
    (b'0'..=b'7').contains(&r) || r == b'_'
}

fn is_valid_binary_rune(r: u8) -> bool {
    r == b'0' || r == b'1' || r == b'_'
}

fn expect(x: u8, b: &[u8]) -> Result<&[u8], ParserError<'_>> {
    if b.is_empty() {
        return Err(new_parser_error(
            b,
            format!(
                "expected character {} but the document ended here",
                fmt_c(x)
            ),
        ));
    }

    if b[0] != x {
        return Err(new_parser_error(
            &b[0..1],
            format!("expected character {}", fmt_c(x)),
        ));
    }

    Ok(&b[1..])
}

// scanner.go

fn scan_follows(b: &[u8], pattern: &[u8]) -> bool {
    b.starts_with(pattern)
}

fn scan_follows_multiline_basic_string_delimiter(b: &[u8]) -> bool {
    scan_follows(b, b"\"\"\"")
}

fn scan_follows_multiline_literal_string_delimiter(b: &[u8]) -> bool {
    scan_follows(b, b"'''")
}

fn scan_follows_true(b: &[u8]) -> bool {
    scan_follows(b, b"true")
}

fn scan_follows_false(b: &[u8]) -> bool {
    scan_follows(b, b"false")
}

fn scan_follows_inf(b: &[u8]) -> bool {
    scan_follows(b, b"inf")
}

fn scan_follows_nan(b: &[u8]) -> bool {
    scan_follows(b, b"nan")
}

fn scan_unquoted_key(b: &[u8]) -> (&[u8], &[u8]) {
    // unquoted-key = 1*( ALPHA / DIGIT / %x2D / %x5F ) ; A-Z / a-z / 0-9 / - / _
    for i in 0..b.len() {
        if !is_unquoted_key_char(b[i]) {
            return (&b[..i], &b[i..]);
        }
    }

    (b, &b[b.len()..])
}

fn is_unquoted_key_char(r: u8) -> bool {
    r.is_ascii_alphanumeric() || r == b'-' || r == b'_'
}

fn scan_literal_string(b: &[u8]) -> Result<(&[u8], &[u8]), ParserError<'_>> {
    // literal-string = apostrophe *literal-char apostrophe
    // apostrophe = %x27 ; ' apostrophe
    // literal-char = %x09 / %x20-26 / %x28-7E / non-ascii
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\'' => return Ok((&b[..i + 1], &b[i + 1..])),
            b'\n' | b'\r' => {
                return Err(new_parser_error(
                    &b[i..i + 1],
                    "literal strings cannot have new lines",
                ));
            }
            _ => {}
        }
        let size = utf8_valid_next(&b[i..]);
        if size == 0 {
            return Err(new_parser_error(&b[i..i + 1], "invalid character"));
        }
        i += size;
    }

    Err(new_parser_error(
        &b[b.len()..],
        "unterminated literal string",
    ))
}

fn scan_multiline_literal_string(b: &[u8]) -> Result<(&[u8], &[u8]), ParserError<'_>> {
    // ml-literal-string = ml-literal-string-delim [ newline ] ml-literal-body
    // ml-literal-string-delim
    // ml-literal-string-delim = 3apostrophe
    // ml-literal-body = *mll-content *( mll-quotes 1*mll-content ) [ mll-quotes ]
    //
    // mll-content = mll-char / newline
    // mll-char = %x09 / %x20-26 / %x28-7E / non-ascii
    // mll-quotes = 1*2apostrophe
    let mut i = 3;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                if scan_follows_multiline_literal_string_delimiter(&b[i..]) {
                    i += 3;

                    // At that point we found 3 apostrophe, and i is the
                    // index of the byte after the third one. The scanner
                    // needs to be eager, because there can be an extra 2
                    // apostrophe that can be accepted at the end of the
                    // string.

                    if i >= b.len() || b[i] != b'\'' {
                        return Ok((&b[..i], &b[i..]));
                    }
                    i += 1;

                    if i >= b.len() || b[i] != b'\'' {
                        return Ok((&b[..i], &b[i..]));
                    }
                    i += 1;

                    if i < b.len() && b[i] == b'\'' {
                        return Err(new_parser_error(
                            &b[i - 3..i + 1],
                            "''' not allowed in multiline literal string",
                        ));
                    }

                    return Ok((&b[..i], &b[i..]));
                }
            }
            b'\r' => {
                if b.len() < i + 2 {
                    return Err(new_parser_error(&b[b.len()..], r"need a \n after \r"));
                }
                if b[i + 1] != b'\n' {
                    return Err(new_parser_error(&b[i..i + 2], r"need a \n after \r"));
                }
                i += 2; // skip the \n
                continue;
            }
            _ => {}
        }
        let size = utf8_valid_next(&b[i..]);
        if size == 0 {
            return Err(new_parser_error(&b[i..i + 1], "invalid character"));
        }
        i += size;
    }

    Err(new_parser_error(
        &b[b.len()..],
        "multiline literal string not terminated by '''",
    ))
}

fn scan_windows_newline(b: &[u8]) -> Result<(&[u8], &[u8]), ParserError<'_>> {
    const LEN_CRLF: usize = 2;
    if b.len() < LEN_CRLF {
        return Err(new_parser_error(b, "windows new line expected"));
    }

    if b[1] != b'\n' {
        return Err(new_parser_error(b, r"windows new line should be \r\n"));
    }

    Ok((&b[..LEN_CRLF], &b[LEN_CRLF..]))
}

fn scan_whitespace(b: &[u8]) -> (&[u8], &[u8]) {
    for i in 0..b.len() {
        match b[i] {
            b' ' | b'\t' => continue,
            _ => return (&b[..i], &b[i..]),
        }
    }

    (b, &b[b.len()..])
}

fn scan_comment(b: &[u8]) -> Result<(&[u8], &[u8]), ParserError<'_>> {
    // comment-start-symbol = %x23 ; #
    // non-ascii = %x80-D7FF / %xE000-10FFFF
    // non-eol = %x09 / %x20-7F / non-ascii
    //
    // comment = comment-start-symbol *non-eol

    let mut i = 1;
    while i < b.len() {
        if b[i] == b'\n' {
            return Ok((&b[..i], &b[i..]));
        }
        if b[i] == b'\r' {
            if i + 1 < b.len() && b[i + 1] == b'\n' {
                return Ok((&b[..i + 1], &b[i + 1..]));
            }
            return Err(new_parser_error(
                &b[i..i + 1],
                "invalid character in comment",
            ));
        }
        let size = utf8_valid_next(&b[i..]);
        if size == 0 {
            return Err(new_parser_error(
                &b[i..i + 1],
                "invalid character in comment",
            ));
        }

        i += size;
    }

    Ok((b, &b[b.len()..]))
}

/// `scanBasicString`: the token (quotes included), whether it has escape
/// sequences, and the rest.
fn scan_basic_string(b: &[u8]) -> Result<(&[u8], bool, &[u8]), ParserError<'_>> {
    // basic-string = quotation-mark *basic-char quotation-mark
    // quotation-mark = %x22            ; "
    // basic-char = basic-unescaped / escaped
    // basic-unescaped = wschar / %x21 / %x23-5B / %x5D-7E / non-ascii
    // escaped = escape escape-seq-char
    let mut escaped = false;
    let mut i = 1;

    while i < b.len() {
        match b[i] {
            b'"' => return Ok((&b[..i + 1], escaped, &b[i + 1..])),
            b'\n' | b'\r' => {
                return Err(new_parser_error(
                    &b[i..i + 1],
                    "basic strings cannot have new lines",
                ));
            }
            b'\\' => {
                if b.len() < i + 2 {
                    return Err(new_parser_error(&b[i..i + 1], "need a character after \\"));
                }
                escaped = true;
                i += 1; // skip the next character
            }
            _ => {}
        }
        i += 1;
    }

    Err(new_parser_error(
        &b[b.len()..],
        "basic string not terminated by \"",
    ))
}

/// `scanMultilineBasicString`: the token (delimiters included), whether it
/// has escape sequences, and the rest.
fn scan_multiline_basic_string(b: &[u8]) -> Result<(&[u8], bool, &[u8]), ParserError<'_>> {
    // ml-basic-string = ml-basic-string-delim [ newline ] ml-basic-body
    // ml-basic-string-delim
    // ml-basic-string-delim = 3quotation-mark
    // ml-basic-body = *mlb-content *( mlb-quotes 1*mlb-content ) [ mlb-quotes ]
    //
    // mlb-content = mlb-char / newline / mlb-escaped-nl
    // mlb-char = mlb-unescaped / escaped
    // mlb-quotes = 1*2quotation-mark
    // mlb-unescaped = wschar / %x21 / %x23-5B / %x5D-7E / non-ascii
    // mlb-escaped-nl = escape ws newline *( wschar / newline )

    let mut escaped = false;
    let mut i = 3;

    while i < b.len() {
        match b[i] {
            b'"' => {
                if scan_follows_multiline_basic_string_delimiter(&b[i..]) {
                    i += 3;

                    // At that point we found 3 apostrophe, and i is the
                    // index of the byte after the third one. The scanner
                    // needs to be eager, because there can be an extra 2
                    // apostrophe that can be accepted at the end of the
                    // string.

                    if i >= b.len() || b[i] != b'"' {
                        return Ok((&b[..i], escaped, &b[i..]));
                    }
                    i += 1;

                    if i >= b.len() || b[i] != b'"' {
                        return Ok((&b[..i], escaped, &b[i..]));
                    }
                    i += 1;

                    if i < b.len() && b[i] == b'"' {
                        return Err(new_parser_error(
                            &b[i - 3..i + 1],
                            "\"\"\" not allowed in multiline basic string",
                        ));
                    }

                    return Ok((&b[..i], escaped, &b[i..]));
                }
            }
            b'\\' => {
                if b.len() < i + 2 {
                    return Err(new_parser_error(&b[b.len()..], "need a character after \\"));
                }
                escaped = true;
                i += 1; // skip the next character
            }
            b'\r' => {
                if b.len() < i + 2 {
                    return Err(new_parser_error(&b[b.len()..], r"need a \n after \r"));
                }
                if b[i + 1] != b'\n' {
                    return Err(new_parser_error(&b[i..i + 2], r"need a \n after \r"));
                }
                i += 1; // skip the \n
            }
            _ => {}
        }
        i += 1;
    }

    Err(new_parser_error(
        &b[b.len()..],
        "multiline basic string not terminated by \"\"\"",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `astNode`: the expected shape of a node.
    struct AstNode {
        kind: Kind,
        data: &'static str,
        children: Vec<AstNode>,
    }

    fn leaf(kind: Kind, data: &'static str) -> AstNode {
        AstNode {
            kind,
            data,
            children: Vec::new(),
        }
    }

    fn node(kind: Kind, children: Vec<AstNode>) -> AstNode {
        AstNode {
            kind,
            data: "",
            children,
        }
    }

    /// `compareNode` and `compareIterator`.
    fn compare_node(b: &Builder<'_>, e: &AstNode, n: usize) {
        let actual = b.node_at(n);
        assert_eq!(e.kind, actual.kind);
        assert_eq!(e.data.as_bytes(), &*actual.data);

        let mut it = b.children(n);
        let mut idx = 0;
        while it.next(b) {
            assert!(idx < e.children.len(), "extra child in actual tree");
            compare_node(b, &e.children[idx], it.node());
            idx += 1;
        }
        assert_eq!(idx, e.children.len(), "missing children in actual");
    }

    /// The first expression of input, compared with expected.
    fn check(input: &str, expected: &AstNode) {
        let mut p = Parser::new(input.as_bytes());
        p.next_expression();
        assert!(p.error().is_none(), "{input}: {:?}", p.error());
        compare_node(p.builder(), expected, p.expression());
    }

    // Ported from unstable/parser_test.go TestParser_AST_Numbers.
    #[test]
    fn ast_numbers() {
        let examples = [
            ("integer just digits", "1234", Kind::Integer),
            ("integer zero", "0", Kind::Integer),
            ("integer sign", "+99", Kind::Integer),
            ("integer hex uppercase", "0xDEADBEEF", Kind::Integer),
            ("integer hex lowercase", "0xdead_beef", Kind::Integer),
            ("integer octal", "0o01234567", Kind::Integer),
            ("integer binary", "0b11010110", Kind::Integer),
            ("float zero", "0.0", Kind::Float),
            ("float positive zero", "+0.0", Kind::Float),
            ("float negative zero", "-0.0", Kind::Float),
            ("float pi", "3.1415", Kind::Float),
            ("float negative", "-0.01", Kind::Float),
            ("float signed exponent", "5e+22", Kind::Float),
            ("float exponent lowercase", "1e06", Kind::Float),
            ("float exponent uppercase", "-2E-2", Kind::Float),
            ("float fractional with exponent", "6.626e-34", Kind::Float),
            ("float underscores", "224_617.445_991_228", Kind::Float),
            ("inf", "inf", Kind::Float),
            ("inf negative", "-inf", Kind::Float),
            ("inf positive", "+inf", Kind::Float),
            ("nan", "nan", Kind::Float),
            ("nan negative", "-nan", Kind::Float),
            ("nan positive", "+nan", Kind::Float),
        ];
        for (_desc, input, kind) in examples {
            let expected = node(
                Kind::KeyValue,
                vec![leaf(kind, input), leaf(Kind::Key, "A")],
            );
            check(&format!("A = {input}"), &expected);
        }
    }

    // Ported from unstable/parser_test.go TestParser_AST.
    #[test]
    fn ast() {
        let examples = [
            (
                "simple string assignment",
                r#"A = "hello""#,
                node(
                    Kind::KeyValue,
                    vec![leaf(Kind::String, "hello"), leaf(Kind::Key, "A")],
                ),
            ),
            (
                "simple bool assignment",
                "A = true",
                node(
                    Kind::KeyValue,
                    vec![leaf(Kind::Bool, "true"), leaf(Kind::Key, "A")],
                ),
            ),
            (
                "array of strings",
                r#"A = ["hello", ["world", "again"]]"#,
                node(
                    Kind::KeyValue,
                    vec![
                        node(
                            Kind::Array,
                            vec![
                                leaf(Kind::String, "hello"),
                                node(
                                    Kind::Array,
                                    vec![leaf(Kind::String, "world"), leaf(Kind::String, "again")],
                                ),
                            ],
                        ),
                        leaf(Kind::Key, "A"),
                    ],
                ),
            ),
            (
                "array of arrays of strings",
                r#"A = ["hello", "world"]"#,
                node(
                    Kind::KeyValue,
                    vec![
                        node(
                            Kind::Array,
                            vec![leaf(Kind::String, "hello"), leaf(Kind::String, "world")],
                        ),
                        leaf(Kind::Key, "A"),
                    ],
                ),
            ),
            (
                "inline table",
                r#"name = { first = "Tom", last = "Preston-Werner" }"#,
                node(
                    Kind::KeyValue,
                    vec![
                        node(
                            Kind::InlineTable,
                            vec![
                                node(
                                    Kind::KeyValue,
                                    vec![leaf(Kind::String, "Tom"), leaf(Kind::Key, "first")],
                                ),
                                node(
                                    Kind::KeyValue,
                                    vec![
                                        leaf(Kind::String, "Preston-Werner"),
                                        leaf(Kind::Key, "last"),
                                    ],
                                ),
                            ],
                        ),
                        leaf(Kind::Key, "name"),
                    ],
                ),
            ),
        ];
        for (_desc, input, expected) in &examples {
            check(input, expected);
        }
    }

    // Ported from unstable/parser_test.go TestParser_AST_DateTimes.
    #[test]
    fn ast_date_times() {
        let examples = [
            (
                "offset-date-time with delim 'T' and UTC offset",
                "2021-07-21T12:08:05Z",
                Kind::DateTime,
            ),
            (
                "offset-date-time with space delim and +8hours offset",
                "2021-07-21 12:08:05+08:00",
                Kind::DateTime,
            ),
            (
                "local-date-time with nano second",
                "2021-07-21T12:08:05.666666666",
                Kind::LocalDateTime,
            ),
            (
                "local-date-time",
                "2021-07-21T12:08:05",
                Kind::LocalDateTime,
            ),
            ("local-date", "2021-07-21", Kind::LocalDate),
        ];
        for (_desc, input, kind) in examples {
            let expected = node(
                Kind::KeyValue,
                vec![leaf(kind, input), leaf(Kind::Key, "A")],
            );
            check(&format!("A = {input}"), &expected);
        }
    }

    // Ported from unstable/parser_test.go ExampleParser.
    #[test]
    fn example_parser() {
        let doc = "
\thello = \"world\"
\tvalue = 42
\t";
        let mut p = Parser::new(doc.as_bytes());
        let mut out = String::new();
        while p.next_expression() {
            let b = p.builder();
            let e = p.expression();
            out.push_str(&format!("Expression: {}\n", b.node_at(e).kind));
            let value = b.node_at(b.value(e));
            let mut it = b.key(e);
            it.next(b);
            let k = b.node_at(it.node());
            out.push_str(&format!(
                "{} -> ({}) {}\n",
                String::from_utf8_lossy(&k.data),
                value.kind,
                String::from_utf8_lossy(&value.data)
            ));
        }
        assert!(p.error().is_none());
        assert_eq!(
            out,
            "Expression: KeyValue
hello -> (String) world
Expression: KeyValue
value -> (Integer) 42
"
        );
    }
}
