// Ports: scannerc.go (yaml_parser_fetch_more_tokens, yaml_parser_fetch_next_token, the simple key, indent and flow level bookkeeping, the token fetchers, yaml_parser_scan_to_next_token, the scanner error texts and marks) @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3, a Go port of libyaml: Copyright (c)
// 2006-2011 Kirill Simonov, Copyright (c) 2011-2019 Canonical Ltd. MIT
// licensed; see crates/gorse-config/licenses/YAML-V3-LICENSE.

//! libyaml's scanner: the tokens of a YAML stream, from the characters the
//! reader decoded.
//!
//! This is yaml.v3's `scannerc.go`, ported with the same lookahead (a token is
//! fetched while the queue holds at most two tokens or a possible simple key
//! at its head needs disambiguating), the same marks and the same error
//! texts. The submodules hold the lexemes ([`lexemes`]), the scalars
//! ([`scalars`]) and the comments ([`comments`]), which yaml.v3 scans as
//! well: their reader errors and the marks of block-end tokens are observable.
//!
//! Comment text is not kept, since the decoder never reads it. The token queue
//! is never compacted, where yaml.v3 compacts it when its backing array is
//! full; see `PORTING_NOTES.md` for what that changes.

use std::collections::HashMap;

use super::reader::Decoded;

mod comments;
mod lexemes;
mod scalars;

/// libyaml's `max_flow_level`.
const MAX_FLOW_LEVEL: usize = 10000;

/// libyaml's `max_indents`.
const MAX_INDENTS: usize = 10000;

/// A position: libyaml's `yaml_mark_t`. The index counts characters, a CRLF
/// two of them; the line and the column count from 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    /// The character index, from 0.
    pub(crate) index: usize,
    /// The line, from 0.
    pub(crate) line: usize,
    /// The column, from 0.
    pub(crate) column: usize,
}

/// A scalar's style, as libyaml's scanner reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}

/// libyaml's `yaml_token_type_t`, with the token's value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TokenKind {
    StreamStart,
    StreamEnd,
    VersionDirective(u32, u32),
    TagDirective(String, String),
    DocumentStart,
    DocumentEnd,
    BlockSequenceStart,
    BlockMappingStart,
    BlockEnd,
    FlowSequenceStart,
    FlowSequenceEnd,
    FlowMappingStart,
    FlowMappingEnd,
    BlockEntry,
    FlowEntry,
    Key,
    Value,
    Alias(String),
    Anchor(String),
    /// The handle and the suffix.
    Tag(String, String),
    Scalar(String, ScalarStyle),
}

/// A token and its marks.
#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) start: Mark,
    pub(crate) end: Mark,
}

/// Which part of libyaml failed (`yaml_error_type_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Reader,
    Scanner,
    Parser,
}

/// A reader, scanner or parser error, with libyaml's marks.
#[derive(Clone, Debug)]
pub(crate) struct SyntaxError {
    pub(crate) kind: ErrorKind,
    pub(crate) problem: String,
    pub(crate) context_mark: Mark,
    pub(crate) problem_mark: Mark,
}

impl SyntaxError {
    /// yaml.v3's `(*parser).fail` text, without the `yaml: ` prefix.
    pub(crate) fn message(&self) -> String {
        let mut line = 0;
        if self.context_mark.line != 0 {
            line = self.context_mark.line;
            if self.kind == ErrorKind::Scanner {
                line += 1;
            }
        } else if self.problem_mark.line != 0 {
            line = self.problem_mark.line;
            if self.kind == ErrorKind::Scanner {
                line += 1;
            }
        }
        let problem = if self.problem.is_empty() {
            "unknown problem parsing YAML content"
        } else {
            &self.problem
        };
        if line != 0 {
            format!("line {line}: {problem}")
        } else {
            problem.to_owned()
        }
    }
}

/// `yaml_simple_key_t`: a token that may be the key of a mapping.
#[derive(Clone, Copy, Debug, Default)]
struct SimpleKey {
    possible: bool,
    required: bool,
    token_number: usize,
    mark: Mark,
}

/// The marks of a comment, which decide where block-end tokens are placed
/// (`yaml_comment_t`). Line comments have zero scan and end marks, as in Go.
#[derive(Clone, Copy, Debug)]
struct Comment {
    scan_mark: Mark,
    start_mark: Mark,
    end_mark: Mark,
}

/// The token source of the parser: [`Scanner::peek`] and [`Scanner::skip`]
/// as libyaml's `peek_token` and `skip_token`.
pub(crate) struct Scanner {
    /// The characters the reader decoded, up to the window holding its error.
    chars: Vec<char>,
    /// The reader's problem, when the input has one past `chars`.
    reader_problem: Option<&'static str>,
    /// libyaml's `parser.mark`: the position of the next character. Its index
    /// is also the position in `chars`.
    mark: Mark,
    /// The index in `chars` of libyaml's `buffer[0]`, which `is_bom` reads.
    buffer_start: usize,
    /// libyaml's `newlines`: the line breaks since the last character that is
    /// not blank.
    newlines: usize,
    /// An error the scanner latched without returning it (libyaml sets
    /// `parser.error` when a lookahead fails and carries on).
    error: Option<SyntaxError>,
    stream_start_produced: bool,
    token_available: bool,
    /// The token queue. Tokens are not removed: `head` is the index of the
    /// next token to parse, and a token's number is its index here.
    tokens: Vec<Token>,
    head: usize,
    flow_level: usize,
    indent: isize,
    indents: Vec<isize>,
    simple_key_allowed: bool,
    simple_keys: Vec<SimpleKey>,
    /// The possible simple keys, by token number (`simple_keys_by_tok`).
    simple_keys_by_tok: HashMap<usize, usize>,
    comments: Vec<Comment>,
}

impl Scanner {
    /// A scanner over the reader's output.
    pub(crate) fn new(decoded: Decoded) -> Self {
        Scanner {
            chars: decoded.chars,
            reader_problem: decoded.error.map(|(problem, _)| problem),
            mark: Mark::default(),
            buffer_start: 0,
            newlines: 0,
            error: None,
            stream_start_produced: false,
            token_available: false,
            tokens: Vec::new(),
            head: 0,
            flow_level: 0,
            indent: -1,
            indents: Vec::new(),
            simple_key_allowed: false,
            simple_keys: Vec::new(),
            simple_keys_by_tok: HashMap::new(),
            comments: Vec::new(),
        }
    }

    /// The next token, fetching tokens as needed (`peek_token`).
    pub(crate) fn peek(&mut self) -> Result<&Token, SyntaxError> {
        if !self.token_available {
            self.fetch_more_tokens()?;
        }
        match self.tokens.get(self.head) {
            Some(token) => Ok(token),
            None => Err(SyntaxError {
                kind: ErrorKind::Scanner,
                problem: "invalid scanner state".to_owned(),
                context_mark: Mark::default(),
                problem_mark: self.mark,
            }),
        }
    }

    /// Consumes the next token (`skip_token`).
    pub(crate) fn skip(&mut self) {
        self.token_available = false;
        self.head += 1;
    }

    /// The error the scanner latched, if any (libyaml's `parser.error`).
    pub(crate) fn latched_error(&self) -> Option<&SyntaxError> {
        self.error.as_ref()
    }

    /// The character `k` places after the next one, or NUL past the end of
    /// the input. Callers make the lookahead available first, as libyaml's
    /// `cache` does.
    fn at(&self, k: usize) -> char {
        self.chars.get(self.mark.index + k).copied().unwrap_or('\0')
    }

    /// The characters from the next one to the end of the decoded input.
    fn unread(&self) -> usize {
        self.chars.len().saturating_sub(self.mark.index)
    }

    /// `yaml_parser_update_buffer` for `n` characters: fails with the
    /// reader's error when the input cannot provide them. Like libyaml, the
    /// refill moves the next character to the start of the buffer.
    fn ensure(&mut self, n: usize) -> Result<(), SyntaxError> {
        if self.unread() >= n {
            return Ok(());
        }
        self.buffer_start = self.mark.index;
        match self.reader_problem {
            Some(problem) => Err(self.reader_error(problem)),
            None => Ok(()),
        }
    }

    /// [`Self::ensure`] for a lookahead the scanner can do without: the
    /// reader's error is latched and `false` returned, and the caller goes
    /// on.
    fn ensure_soft(&mut self, n: usize) -> bool {
        match self.ensure(n) {
            Ok(()) => true,
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn reader_error(&self, problem: &str) -> SyntaxError {
        SyntaxError {
            kind: ErrorKind::Reader,
            problem: problem.to_owned(),
            context_mark: Mark::default(),
            problem_mark: Mark::default(),
        }
    }

    /// `yaml_parser_set_scanner_error`: a scanner error at the current mark,
    /// with `context_mark` as the context.
    fn scanner_error(&self, context_mark: Mark, problem: &str) -> SyntaxError {
        SyntaxError {
            kind: ErrorKind::Scanner,
            problem: problem.to_owned(),
            context_mark,
            problem_mark: self.mark,
        }
    }

    /// `is_bom` of yaml.v3, which tests the first three bytes of libyaml's
    /// buffer rather than the position it is given.
    fn is_bom(&self) -> bool {
        self.chars.get(self.buffer_start) == Some(&'\u{feff}')
    }

    /// `yaml_parser_fetch_more_tokens`: fetches tokens until the head of the
    /// queue is settled.
    fn fetch_more_tokens(&mut self) -> Result<(), SyntaxError> {
        loop {
            if self.tokens.len() - self.head > 2 {
                let Some(&key) = self.simple_keys_by_tok.get(&self.head) else {
                    break;
                };
                if !self.simple_key_is_valid(key)? {
                    break;
                }
            }
            self.fetch_next_token()?;
        }
        self.token_available = true;
        Ok(())
    }

    /// `yaml_parser_fetch_next_token`: scans the next token and queues it,
    /// then the comment on its line.
    fn fetch_next_token(&mut self) -> Result<(), SyntaxError> {
        self.ensure(1)?;
        if !self.stream_start_produced {
            self.fetch_stream_start();
            return Ok(());
        }
        let scan_mark = self.mark;
        self.scan_to_next_token()?;
        self.unroll_indent(self.mark.column as isize, scan_mark);
        self.ensure(4)?;
        if self.at(0) == '\0' {
            return self.fetch_stream_end();
        }
        let column_zero = self.mark.column == 0;
        let c = self.at(0);
        if column_zero && c == '%' {
            return self.fetch_directive();
        }
        if column_zero
            && self.at(0) == '-'
            && self.at(1) == '-'
            && self.at(2) == '-'
            && is_blankz(self.at(3))
        {
            return self.fetch_document_indicator(TokenKind::DocumentStart);
        }
        if column_zero
            && self.at(0) == '.'
            && self.at(1) == '.'
            && self.at(2) == '.'
            && is_blankz(self.at(3))
        {
            return self.fetch_document_indicator(TokenKind::DocumentEnd);
        }
        self.fetch_token()?;
        if matches!(
            self.tokens.last(),
            Some(Token {
                kind: TokenKind::BlockEntry,
                ..
            })
        ) {
            return Ok(());
        }
        self.scan_line_comment()
    }

    /// The branches of `yaml_parser_fetch_next_token` after the stream end,
    /// directive and document indicator checks.
    fn fetch_token(&mut self) -> Result<(), SyntaxError> {
        let c = self.at(0);
        if c == '[' {
            return self.fetch_flow_collection_start(TokenKind::FlowSequenceStart);
        }
        if c == '{' {
            return self.fetch_flow_collection_start(TokenKind::FlowMappingStart);
        }
        if c == ']' {
            return self.fetch_flow_collection_end(TokenKind::FlowSequenceEnd);
        }
        if c == '}' {
            return self.fetch_flow_collection_end(TokenKind::FlowMappingEnd);
        }
        if c == ',' {
            return self.fetch_flow_entry();
        }
        if c == '-' && is_blankz(self.at(1)) {
            return self.fetch_block_entry();
        }
        if c == '?' && (self.flow_level > 0 || is_blankz(self.at(1))) {
            return self.fetch_key();
        }
        if c == ':' && (self.flow_level > 0 || is_blankz(self.at(1))) {
            return self.fetch_value();
        }
        if c == '*' {
            return self.fetch_anchor(true);
        }
        if c == '&' {
            return self.fetch_anchor(false);
        }
        if c == '!' {
            return self.fetch_tag();
        }
        if c == '|' && self.flow_level == 0 {
            return self.fetch_block_scalar(true);
        }
        if c == '>' && self.flow_level == 0 {
            return self.fetch_block_scalar(false);
        }
        if c == '\'' {
            return self.fetch_flow_scalar(true);
        }
        if c == '"' {
            return self.fetch_flow_scalar(false);
        }
        if self.starts_plain_scalar() {
            return self.fetch_plain_scalar();
        }
        Err(self.scanner_error(self.mark, "found character that cannot start any token"))
    }

    /// Whether the character at the mark starts a plain scalar.
    fn starts_plain_scalar(&self) -> bool {
        let c = self.at(0);
        let indicator = matches!(
            c,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        );
        !(is_blankz(c) || indicator)
            || (c == '-' && !is_blank(self.at(1)))
            || (self.flow_level == 0 && (c == '?' || c == ':') && !is_blankz(self.at(1)))
    }

    /// `yaml_simple_key_is_valid`: whether the key may still be a simple key,
    /// dropping it when it has run past the line or 1024 characters.
    fn simple_key_is_valid(&mut self, key: usize) -> Result<bool, SyntaxError> {
        let simple_key = self.simple_keys[key];
        if !simple_key.possible {
            return Ok(false);
        }
        if simple_key.mark.line < self.mark.line || simple_key.mark.index + 1024 < self.mark.index {
            if simple_key.required {
                return Err(self.scanner_error(simple_key.mark, "could not find expected ':'"));
            }
            self.simple_keys[key].possible = false;
            return Ok(false);
        }
        Ok(true)
    }

    /// `yaml_parser_save_simple_key`: the next token may be a simple key.
    fn save_simple_key(&mut self) -> Result<(), SyntaxError> {
        let required = self.flow_level == 0 && self.indent == self.mark.column as isize;
        if self.simple_key_allowed {
            let key = SimpleKey {
                possible: true,
                required,
                token_number: self.tokens.len(),
                mark: self.mark,
            };
            self.remove_simple_key()?;
            let top = self.simple_keys.len() - 1;
            self.simple_keys[top] = key;
            self.simple_keys_by_tok.insert(key.token_number, top);
        }
        Ok(())
    }

    /// `yaml_parser_remove_simple_key`: the innermost simple key cannot be a
    /// key any more.
    fn remove_simple_key(&mut self) -> Result<(), SyntaxError> {
        let top = self.simple_keys.len() - 1;
        let key = self.simple_keys[top];
        if key.possible {
            if key.required {
                return Err(self.scanner_error(key.mark, "could not find expected ':'"));
            }
            self.simple_keys[top].possible = false;
            self.simple_keys_by_tok.remove(&key.token_number);
        }
        Ok(())
    }

    /// `yaml_parser_increase_flow_level`.
    fn increase_flow_level(&mut self) -> Result<(), SyntaxError> {
        self.simple_keys.push(SimpleKey {
            possible: false,
            required: false,
            token_number: self.tokens.len(),
            mark: self.mark,
        });
        self.flow_level += 1;
        if self.flow_level > MAX_FLOW_LEVEL {
            let context = self.simple_keys[self.simple_keys.len() - 1].mark;
            return Err(
                self.scanner_error(context, &format!("exceeded max depth of {MAX_FLOW_LEVEL}"))
            );
        }
        Ok(())
    }

    /// `yaml_parser_decrease_flow_level`.
    fn decrease_flow_level(&mut self) {
        if self.flow_level > 0 {
            self.flow_level -= 1;
            if let Some(key) = self.simple_keys.pop() {
                self.simple_keys_by_tok.remove(&key.token_number);
            }
        }
    }

    /// `yaml_parser_roll_indent`: opens a block collection at `column`,
    /// queuing its start token at token number `at` (or at the end).
    fn roll_indent(
        &mut self,
        column: usize,
        at: Option<usize>,
        kind: TokenKind,
        mark: Mark,
    ) -> Result<(), SyntaxError> {
        if self.flow_level > 0 {
            return Ok(());
        }
        if self.indent < column as isize {
            self.indents.push(self.indent);
            self.indent = column as isize;
            if self.indents.len() > MAX_INDENTS {
                let context = self.simple_keys[self.simple_keys.len() - 1].mark;
                return Err(
                    self.scanner_error(context, &format!("exceeded max depth of {MAX_INDENTS}"))
                );
            }
            self.insert_token(
                at,
                Token {
                    kind,
                    start: mark,
                    end: mark,
                },
            );
        }
        Ok(())
    }

    /// `yaml_parser_unroll_indent`: closes the block collections deeper than
    /// `column`, with block-end tokens placed at the marks of the comments
    /// that belong to them.
    fn unroll_indent(&mut self, column: isize, scan_mark: Mark) {
        if self.flow_level > 0 {
            return;
        }
        let mut block_mark = scan_mark;
        block_mark.index = block_mark.index.saturating_sub(1);
        while self.indent > column {
            let mut stop_index = block_mark.index;
            for comment in self.comments.iter().rev() {
                if comment.end_mark.index < stop_index {
                    break;
                }
                if comment.start_mark.column as isize == self.indent + 1 {
                    block_mark = comment.start_mark;
                }
                stop_index = comment.scan_mark.index;
            }
            self.insert_token(
                None,
                Token {
                    kind: TokenKind::BlockEnd,
                    start: block_mark,
                    end: block_mark,
                },
            );
            self.indent = self.indents.pop().unwrap_or(-1);
        }
    }

    /// `yaml_insert_token`: queues `token` at token number `at`, or at the end.
    fn insert_token(&mut self, at: Option<usize>, token: Token) {
        match at {
            Some(number) => self.tokens.insert(number, token),
            None => self.tokens.push(token),
        }
    }

    /// Queues a token that spans `start..end`, with `kind`, at the end.
    fn push_token(&mut self, kind: TokenKind, start: Mark, end: Mark) {
        self.insert_token(None, Token { kind, start, end });
    }

    fn fetch_stream_start(&mut self) {
        self.indent = -1;
        self.simple_keys.push(SimpleKey::default());
        self.simple_keys_by_tok = HashMap::new();
        self.simple_key_allowed = true;
        self.stream_start_produced = true;
        self.push_token(TokenKind::StreamStart, self.mark, self.mark);
    }

    fn fetch_stream_end(&mut self) -> Result<(), SyntaxError> {
        if self.mark.column != 0 {
            self.mark.column = 0;
            self.mark.line += 1;
        }
        self.unroll_indent(-1, self.mark);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        self.push_token(TokenKind::StreamEnd, self.mark, self.mark);
        Ok(())
    }

    fn fetch_directive(&mut self) -> Result<(), SyntaxError> {
        self.unroll_indent(-1, self.mark);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_directive()?;
        self.insert_token(None, token);
        Ok(())
    }

    fn fetch_document_indicator(&mut self, kind: TokenKind) -> Result<(), SyntaxError> {
        self.unroll_indent(-1, self.mark);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        self.skip_char();
        self.skip_char();
        self.skip_char();
        self.push_token(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_collection_start(&mut self, kind: TokenKind) -> Result<(), SyntaxError> {
        self.save_simple_key()?;
        self.increase_flow_level()?;
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip_char();
        self.push_token(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_collection_end(&mut self, kind: TokenKind) -> Result<(), SyntaxError> {
        self.remove_simple_key()?;
        self.decrease_flow_level();
        self.simple_key_allowed = false;
        let start = self.mark;
        self.skip_char();
        self.push_token(kind, start, self.mark);
        Ok(())
    }

    fn fetch_flow_entry(&mut self) -> Result<(), SyntaxError> {
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip_char();
        self.push_token(TokenKind::FlowEntry, start, self.mark);
        Ok(())
    }

    fn fetch_block_entry(&mut self) -> Result<(), SyntaxError> {
        if self.flow_level == 0 {
            if !self.simple_key_allowed {
                return Err(self.scanner_error(
                    self.mark,
                    "block sequence entries are not allowed in this context",
                ));
            }
            self.roll_indent(
                self.mark.column,
                None,
                TokenKind::BlockSequenceStart,
                self.mark,
            )?;
        }
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let start = self.mark;
        self.skip_char();
        self.push_token(TokenKind::BlockEntry, start, self.mark);
        Ok(())
    }

    fn fetch_key(&mut self) -> Result<(), SyntaxError> {
        if self.flow_level == 0 {
            if !self.simple_key_allowed {
                return Err(
                    self.scanner_error(self.mark, "mapping keys are not allowed in this context")
                );
            }
            self.roll_indent(
                self.mark.column,
                None,
                TokenKind::BlockMappingStart,
                self.mark,
            )?;
        }
        self.remove_simple_key()?;
        self.simple_key_allowed = self.flow_level == 0;
        let start = self.mark;
        self.skip_char();
        self.push_token(TokenKind::Key, start, self.mark);
        Ok(())
    }

    fn fetch_value(&mut self) -> Result<(), SyntaxError> {
        let top = self.simple_keys.len() - 1;
        if self.simple_key_is_valid(top)? {
            let key = self.simple_keys[top];
            self.insert_token(
                Some(key.token_number),
                Token {
                    kind: TokenKind::Key,
                    start: key.mark,
                    end: key.mark,
                },
            );
            self.roll_indent(
                key.mark.column,
                Some(key.token_number),
                TokenKind::BlockMappingStart,
                key.mark,
            )?;
            self.simple_keys[top].possible = false;
            self.simple_keys_by_tok.remove(&key.token_number);
            self.simple_key_allowed = false;
        } else {
            if self.flow_level == 0 {
                if !self.simple_key_allowed {
                    return Err(self.scanner_error(
                        self.mark,
                        "mapping values are not allowed in this context",
                    ));
                }
                self.roll_indent(
                    self.mark.column,
                    None,
                    TokenKind::BlockMappingStart,
                    self.mark,
                )?;
            }
            self.simple_key_allowed = self.flow_level == 0;
        }
        let start = self.mark;
        self.skip_char();
        self.push_token(TokenKind::Value, start, self.mark);
        Ok(())
    }

    fn fetch_anchor(&mut self, alias: bool) -> Result<(), SyntaxError> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_anchor(alias)?;
        self.insert_token(None, token);
        Ok(())
    }

    fn fetch_tag(&mut self) -> Result<(), SyntaxError> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_tag()?;
        self.insert_token(None, token);
        Ok(())
    }

    fn fetch_block_scalar(&mut self, literal: bool) -> Result<(), SyntaxError> {
        self.remove_simple_key()?;
        self.simple_key_allowed = true;
        let token = self.scan_block_scalar(literal)?;
        self.insert_token(None, token);
        Ok(())
    }

    fn fetch_flow_scalar(&mut self, single: bool) -> Result<(), SyntaxError> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_flow_scalar(single)?;
        self.insert_token(None, token);
        Ok(())
    }

    fn fetch_plain_scalar(&mut self) -> Result<(), SyntaxError> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let token = self.scan_plain_scalar()?;
        self.insert_token(None, token);
        Ok(())
    }

    /// `yaml_parser_scan_to_next_token`: skips blanks, comments and line
    /// breaks up to the next token.
    fn scan_to_next_token(&mut self) -> Result<(), SyntaxError> {
        let scan_mark = self.mark;
        loop {
            self.ensure(1)?;
            if self.mark.column == 0 && self.is_bom() {
                self.skip_char();
            }
            self.ensure(1)?;
            while self.at(0) == ' '
                || ((self.flow_level > 0 || !self.simple_key_allowed) && self.at(0) == '\t')
            {
                self.skip_char();
                self.ensure(1)?;
            }
            if self.at(0) == '#' {
                self.scan_comments(scan_mark)?;
            }
            if is_break(self.at(0)) {
                self.ensure(2)?;
                self.skip_break();
                if self.flow_level == 0 {
                    self.simple_key_allowed = true;
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    /// `skip`: the character at the mark is consumed.
    fn skip_char(&mut self) {
        self.advance_char();
    }

    /// `is_z` at the mark: the end of the input.
    fn at_end(&self) -> bool {
        self.at(0) == '\0'
    }

    /// `read` without the text: the character at the mark is consumed, and
    /// returned.
    fn advance_char(&mut self) -> char {
        let c = self.at(0);
        if !is_blank(c) {
            self.newlines = 0;
        }
        self.mark.index += 1;
        self.mark.column += 1;
        c
    }

    /// `skip_line`: the line break at the mark is consumed.
    fn skip_break(&mut self) {
        if self.at(0) == '\r' && self.at(1) == '\n' {
            self.mark.index += 2;
        } else if is_break(self.at(0)) {
            self.mark.index += 1;
        } else {
            return;
        }
        self.mark.column = 0;
        self.mark.line += 1;
        self.newlines += 1;
    }

    /// `read`: the character at the mark is consumed and appended to `s`.
    fn take_char(&mut self, s: &mut String) {
        let c = self.advance_char();
        s.push(c);
    }

    /// `read_line`: the line break at the mark is consumed and appended to `s`
    /// as a line feed (LS and PS are kept). Nothing happens at other characters.
    fn take_break(&mut self, s: &mut String) {
        match self.at(0) {
            '\r' if self.at(1) == '\n' => {
                s.push('\n');
                self.mark.index += 1;
            }
            '\r' | '\n' => s.push('\n'),
            c @ ('\u{2028}' | '\u{2029}') => s.push(c),
            _ => return,
        }
        self.mark.index += 1;
        self.mark.column = 0;
        self.mark.line += 1;
        self.newlines += 1;
    }
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// `is_alpha`: the characters an anchor, alias, tag handle or directive name
/// may hold.
fn is_alpha(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}')
}

fn is_breakz(c: char) -> bool {
    is_break(c) || c == '\0'
}

fn is_blankz(c: char) -> bool {
    is_blank(c) || is_breakz(c)
}
