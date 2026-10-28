// Ports: scannerc.go (the token queue of yaml_parser_fetch_more_tokens, the depth limits of yaml_parser_roll_indent and yaml_parser_increase_flow_level, the scanner error texts and marks) @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3, a Go port of libyaml: Copyright (c)
// 2006-2011 Kirill Simonov, Copyright (c) 2011-2019 Canonical Ltd. MIT
// licensed; see crates/gorse-config/licenses/YAML-V3-LICENSE.

//! libyaml's tokens, scanned by `yaml-rust2`.
//!
//! The scanner of `yaml-rust2` is itself a port of libyaml's, updated for
//! YAML 1.2. This adapter gives the parser ([`super::parser`]) the tokens
//! libyaml would: it drops the implicit flow mappings `yaml-rust2` adds
//! around `[a: b]` entries (libyaml's parser makes those), marks block
//! scalars at their indicator, checks anchor names and the block depth limit
//! as libyaml does, keeps yaml.v3's lookahead of two tokens, and turns the
//! scanner's errors into libyaml's problem texts and marks. Where the two
//! scanners accept different inputs, the difference is a deviation listed in
//! `porting/notes/WP-1-15-yaml.md`.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use yaml_rust2::scanner::{Scanner as Yr2Scanner, TScalarStyle, TokenType};

use super::reader::{Chars, Decoded};

/// libyaml's `max_indents` and `max_flow_level`.
const MAX_DEPTH: usize = 10000;

/// The deepest flow nesting `yaml-rust2` scans (its flow level is a `u8`).
pub(crate) const MAX_FLOW_DEPTH: usize = 255;

/// A position. Only the line is used: yaml.v3 reports lines, never columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mark {
    /// The line, from 0.
    pub(crate) line: usize,
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
        let problem = if self.problem.is_empty() { "unknown problem parsing YAML content" } else { &self.problem };
        if line != 0 { format!("line {line}: {problem}") } else { problem.to_owned() }
    }
}

/// Line numbers of character positions, by libyaml's line breaks: `\r\n`,
/// `\r`, `\n` (NEL arrives as `\n`), U+2028 and U+2029.
struct Lines {
    /// The index of the first character of each line.
    starts: Vec<usize>,
}

impl Lines {
    fn new(chars: &[char]) -> Self {
        let mut starts = vec![0];
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '\r' if chars.get(i + 1) == Some(&'\n') => {
                    i += 1;
                    starts.push(i + 1);
                }
                '\r' | '\n' | '\u{2028}' | '\u{2029}' => starts.push(i + 1),
                _ => {}
            }
            i += 1;
        }
        Lines { starts }
    }

    /// The line of the character at `index`.
    fn line(&self, index: usize) -> usize {
        self.starts.partition_point(|&s| s <= index).saturating_sub(1)
    }

    /// The column of the character at `index`.
    fn column(&self, index: usize) -> usize {
        index - self.starts[self.line(index)]
    }
}

/// The token source of the parser: `peek` and `skip` as libyaml's
/// `peek_token` and `skip_token`.
pub(crate) struct Scanner {
    inner: Yr2Scanner<Chars>,
    chars: Rc<[char]>,
    lines: Lines,
    reader_failed: Rc<Cell<bool>>,
    reader_problem: Option<&'static str>,
    started: bool,
    /// Whether `yaml-rust2` has returned its STREAM-END token (or the token
    /// stream ended early, see [`Self::fetch`]).
    stream_ended: bool,
    queue: VecDeque<Token>,
    error: Option<SyntaxError>,
    /// For each open flow collection: whether it is an implicit mapping
    /// `yaml-rust2` added.
    flows: Vec<bool>,
    /// Open block collections (libyaml's `indents`).
    block_depth: usize,
    /// The kind and character index of the last token `yaml-rust2` returned.
    previous: Option<(TokenType, usize)>,
}

impl Scanner {
    /// A scanner over the decoded input.
    pub(crate) fn new(decoded: Decoded) -> Self {
        let chars: Rc<[char]> = decoded.chars.into();
        let failed = Rc::new(Cell::new(false));
        let error_at = decoded.error.map(|(_, at)| at);
        let inner = Yr2Scanner::new(Chars::new(Rc::clone(&chars), error_at, Rc::clone(&failed)));
        let lines = Lines::new(&chars);
        Scanner {
            inner,
            chars,
            lines,
            reader_failed: failed,
            reader_problem: decoded.error.map(|(problem, _)| problem),
            started: false,
            stream_ended: false,
            queue: VecDeque::new(),
            error: None,
            flows: Vec::new(),
            block_depth: 0,
            previous: None,
        }
    }

    /// `peek_token`: the next token. yaml.v3 keeps two more tokens in the
    /// queue (`yaml_parser_fetch_more_tokens`), so an error in them is
    /// reported here.
    pub(crate) fn peek(&mut self) -> Result<&Token, SyntaxError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        while self.queue.len() < 3 && !self.stream_ended {
            if let Err(e) = self.fetch() {
                self.error = Some(e.clone());
                return Err(e);
            }
        }
        // After STREAM-END, libyaml appends further STREAM-END tokens.
        self.queue.front().ok_or_else(|| self.parser_error_at_end())
    }

    fn parser_error_at_end(&self) -> SyntaxError {
        // Unreachable: the queue always ends with STREAM-END.
        SyntaxError {
            kind: ErrorKind::Parser,
            problem: "did not find expected <stream-end>".to_owned(),
            context_mark: Mark::default(),
            problem_mark: Mark::default(),
        }
    }

    /// `skip_token`.
    pub(crate) fn skip(&mut self) {
        if self.queue.len() > 1 || !matches!(self.queue.front().map(|t| &t.kind), Some(TokenKind::StreamEnd)) {
            self.queue.pop_front();
        }
    }

    fn mark(&self, index: usize) -> Mark {
        Mark { line: self.lines.line(index) }
    }

    fn reader_error(&self) -> SyntaxError {
        SyntaxError {
            kind: ErrorKind::Reader,
            problem: self.reader_problem.unwrap_or_default().to_owned(),
            context_mark: Mark::default(),
            problem_mark: Mark::default(),
        }
    }

    fn scanner_error(&self, problem: impl Into<String>, context: Option<usize>, problem_at: usize) -> SyntaxError {
        SyntaxError {
            kind: ErrorKind::Scanner,
            problem: problem.into(),
            context_mark: context.map(|i| self.mark(i)).unwrap_or_default(),
            problem_mark: self.mark(problem_at),
        }
    }

    /// Fetches the next token from `yaml-rust2` into the queue.
    fn fetch(&mut self) -> Result<(), SyntaxError> {
        if !self.started {
            self.started = true;
            // yaml_parser_fetch_next_token fills the buffer before
            // STREAM-START, so an error in the first window comes first.
            if self.reader_problem.is_some() && self.reader_failed_at_start() {
                return Err(self.reader_error());
            }
        }
        let next = self.inner.next_token();
        if self.reader_failed.get() {
            return Err(self.reader_error());
        }
        let token = match next {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.stream_ended = true;
                return Ok(());
            }
            Err(e) => return self.scan_error(e.info(), e.marker().index()),
        };
        let index = token.0.index();
        let kind = token.1;
        let previous = self.previous.replace((kind.clone(), index));
        let start = self.mark(index);
        let kind = match kind {
            TokenType::StreamStart(_) => TokenKind::StreamStart,
            TokenType::StreamEnd => {
                self.stream_ended = true;
                // yaml_parser_fetch_stream_end forces a new line.
                let line = if self.lines.column(index) != 0 { start.line + 1 } else { start.line };
                let mark = Mark { line };
                self.queue.push_back(Token { kind: TokenKind::StreamEnd, start: mark, end: mark });
                return Ok(());
            }
            TokenType::VersionDirective(major, minor) => TokenKind::VersionDirective(major, minor),
            TokenType::TagDirective(handle, prefix) => {
                if handle.is_empty() && prefix.is_empty() {
                    // yaml-rust2 skips a directive it does not know.
                    return Err(self.scanner_error("found unknown directive name", Some(index), index));
                }
                TokenKind::TagDirective(handle, prefix)
            }
            TokenType::DocumentStart => TokenKind::DocumentStart,
            TokenType::DocumentEnd => TokenKind::DocumentEnd,
            TokenType::BlockSequenceStart | TokenType::BlockMappingStart => {
                self.block_depth += 1;
                if self.block_depth > MAX_DEPTH {
                    let context = matches!(kind, TokenType::BlockMappingStart).then_some(index);
                    return Err(self.scanner_error(format!("exceeded max depth of {MAX_DEPTH}"), context, index));
                }
                if matches!(kind, TokenType::BlockSequenceStart) {
                    TokenKind::BlockSequenceStart
                } else {
                    TokenKind::BlockMappingStart
                }
            }
            TokenType::BlockEnd => {
                self.block_depth = self.block_depth.saturating_sub(1);
                TokenKind::BlockEnd
            }
            TokenType::FlowSequenceStart => {
                self.flows.push(false);
                TokenKind::FlowSequenceStart
            }
            TokenType::FlowSequenceEnd => {
                self.flows.pop();
                TokenKind::FlowSequenceEnd
            }
            TokenType::FlowMappingStart => {
                let implicit = self.chars.get(index) != Some(&'{');
                self.flows.push(implicit);
                if implicit {
                    return Ok(());
                }
                TokenKind::FlowMappingStart
            }
            TokenType::FlowMappingEnd => {
                if self.flows.pop() == Some(true) {
                    return Ok(());
                }
                TokenKind::FlowMappingEnd
            }
            TokenType::BlockEntry => TokenKind::BlockEntry,
            TokenType::FlowEntry => TokenKind::FlowEntry,
            TokenType::Key => TokenKind::Key,
            TokenType::Value => TokenKind::Value,
            TokenType::Alias(name) => {
                self.check_anchor(&name, index)?;
                TokenKind::Alias(name)
            }
            TokenType::Anchor(name) => {
                self.check_anchor(&name, index)?;
                TokenKind::Anchor(name)
            }
            TokenType::Tag(handle, suffix) => TokenKind::Tag(handle, suffix),
            TokenType::Scalar(style, value) => {
                let style = match style {
                    TScalarStyle::Plain => ScalarStyle::Plain,
                    TScalarStyle::SingleQuoted => ScalarStyle::SingleQuoted,
                    TScalarStyle::DoubleQuoted => ScalarStyle::DoubleQuoted,
                    TScalarStyle::Literal => ScalarStyle::Literal,
                    TScalarStyle::Folded => ScalarStyle::Folded,
                };
                if matches!(style, ScalarStyle::Literal | ScalarStyle::Folded) {
                    // libyaml marks a block scalar at its indicator;
                    // yaml-rust2 at its content.
                    if let Some(at) = self.block_scalar_indicator(previous) {
                        let mark = self.mark(at);
                        let value = if value.chars().all(|c| c == '\n') { self.empty_block_scalar(at) } else { value };
                        self.queue.push_back(Token { kind: TokenKind::Scalar(value, style), start: mark, end: start });
                        return Ok(());
                    }
                }
                TokenKind::Scalar(value, style)
            }
        };
        self.queue.push_back(Token { kind, start, end: start });
        Ok(())
    }

    fn reader_failed_at_start(&self) -> bool {
        // The window holding the error is the first one when no character
        // precedes it.
        self.chars.is_empty()
    }

    /// The index of the `|` or `>` of a block scalar, searched for after the
    /// token before it.
    fn block_scalar_indicator(&self, previous: Option<(TokenType, usize)>) -> Option<usize> {
        let (kind, index) = previous?;
        let mut i = match kind {
            TokenType::StreamStart(_) | TokenType::BlockEntry => index,
            TokenType::Value | TokenType::Key => index + 1,
            TokenType::DocumentStart => index + 3,
            TokenType::Tag(..) | TokenType::Anchor(..) => {
                let mut i = index;
                while self.chars.get(i).is_some_and(|c| !is_blank_or_break(*c)) {
                    i += 1;
                }
                i
            }
            _ => return None,
        };
        while let Some(&c) = self.chars.get(i) {
            match c {
                '|' | '>' => return Some(i),
                '#' => {
                    while self.chars.get(i).is_some_and(|c| !is_break(*c)) {
                        i += 1;
                    }
                }
                c if is_blank_or_break(c) || c == '\u{feff}' => i += 1,
                _ => return None,
            }
        }
        None
    }

    /// The value libyaml gives a block scalar without content lines, whose
    /// indicator is at `at`: nothing, or with the keep indicator (`+`) one
    /// line break per empty line after the header (`yaml-rust2` adds a line
    /// break at the end of the stream).
    fn empty_block_scalar(&self, at: usize) -> String {
        let header: Vec<char> = self.chars[at + 1..].iter().take(2).copied().collect();
        let keep = header.first() == Some(&'+') || (header.first().is_some_and(char::is_ascii_digit) && header.get(1) == Some(&'+'));
        if !keep {
            return String::new();
        }
        // Skip the header line; each later line of spaces ending in a line
        // break adds one.
        let mut i = at;
        while self.chars.get(i).is_some_and(|c| !is_break(*c)) {
            i += 1;
        }
        let mut breaks = String::new();
        loop {
            match self.chars.get(i) {
                Some('\r') if self.chars.get(i + 1) == Some(&'\n') => i += 2,
                Some(c) if is_break(*c) => i += 1,
                _ => break,
            }
            while self.chars.get(i) == Some(&' ') {
                i += 1;
            }
            if !self.chars.get(i).is_some_and(|c| is_break(*c)) {
                break;
            }
            breaks.push('\n');
        }
        breaks
    }

    /// libyaml's anchor names are ASCII letters, digits, `_` and `-`
    /// (`yaml_parser_scan_anchor`); `yaml-rust2` takes any character but
    /// blanks and flow indicators.
    fn check_anchor(&self, name: &str, index: usize) -> Result<(), SyntaxError> {
        let valid = name.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').count();
        let rest = name.chars().nth(valid);
        // libyaml ends the name before these and scans them as tokens of
        // their own; the port keeps yaml-rust2's longer name (a deviation).
        let ends_name = |c: char| matches!(c, '?' | ':' | ',' | ']' | '}' | '%' | '@' | '`');
        if valid == 0 || rest.is_some_and(|c| !ends_name(c)) {
            let problem_at = index + 1 + valid;
            return Err(self.scanner_error("did not find expected alphabetic or numeric character", Some(index), problem_at));
        }
        Ok(())
    }

    /// Maps an error of `yaml-rust2` to libyaml's text and marks.
    fn scan_error(&mut self, info: &str, at: usize) -> Result<(), SyntaxError> {
        let current = self.inner.mark().index();
        match info {
            "invalid content after document end marker" => {
                // libyaml scans the content after `...` as tokens of the next
                // document, which the first document never reads; the
                // stream ends here for the parser's lookahead.
                self.stream_ended = true;
                let mark = self.mark(at);
                self.queue.push_back(Token { kind: TokenKind::StreamEnd, start: mark, end: mark });
                return Ok(());
            }
            "simple key expect ':'" | "simple key expected" => {
                let key = self.simple_key_line(at);
                return Err(SyntaxError {
                    kind: ErrorKind::Scanner,
                    problem: "could not find expected ':'".to_owned(),
                    context_mark: Mark { line: key },
                    problem_mark: self.mark(at),
                });
            }
            "recursion limit exceeded" => {
                return Err(self.scanner_error(format!("exceeded max depth of {MAX_FLOW_DEPTH}"), None, at));
            }
            "mapping values are not allowed in this context"
            | "block sequence entries are not allowed in this context"
            | "mapping keys are not allowed in this context" => {
                return Err(self.scanner_error(info, Some(at), at));
            }
            "did not find expected next token" => {
                return Err(self.scanner_error("found character that cannot start any token", Some(at), at));
            }
            _ => {}
        }
        if info.starts_with("unexpected character: ") {
            return Err(self.scanner_error("found character that cannot start any token", Some(at), at));
        }
        if let Some(rest) = info.strip_prefix("while ") {
            if let Some((_, problem)) = rest.split_once(", ") {
                let problem = match problem {
                    "did not find expected hexadecimal number" => "did not find expected hexdecimal number",
                    "found a tab" => "found a tab character that violates indentation",
                    p => p,
                };
                return Err(self.scanner_error(problem, Some(at), current.max(at)));
            }
        }
        // A YAML 1.2 rule of yaml-rust2 that libyaml does not have.
        Err(self.scanner_error(info, Some(at), at))
    }

    /// The line of the simple key `yaml-rust2` gave up on: the last line
    /// before the error position that holds something other than blanks
    /// and comments (the key is the token after the last one returned).
    fn simple_key_line(&self, at: usize) -> usize {
        let line = self.lines.line(at);
        let start = self.lines.starts[line];
        if self.chars[start..at.min(self.chars.len())].iter().any(|c| !is_blank_or_break(*c)) {
            return line;
        }
        let mut l = line;
        while l > 0 {
            l -= 1;
            let from = self.lines.starts[l];
            let to = self.lines.starts.get(l + 1).copied().unwrap_or(self.chars.len());
            let first = self.chars[from..to].iter().find(|c| !is_blank_or_break(**c) && **c != '\u{feff}');
            if first.is_some_and(|c| *c != '#') {
                return l;
            }
        }
        0
    }
}

fn is_break(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}')
}

fn is_blank_or_break(c: char) -> bool {
    c == ' ' || c == '\t' || is_break(c)
}
