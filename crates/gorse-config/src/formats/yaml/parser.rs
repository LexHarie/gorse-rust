// Ports: parserc.go @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3, a Go port of libyaml: Copyright (c)
// 2006-2011 Kirill Simonov, Copyright (c) 2011-2019 Canonical Ltd. MIT
// licensed; see crates/gorse-config/licenses/YAML-V3-LICENSE.

//! libyaml's parser: the state machine that turns tokens into events, with
//! its error texts and marks.
//!
//! yaml.v3's parser also attaches comments to events; comments never change
//! what the decoder produces, so the port leaves them out (the scanner of
//! `yaml-rust2` drops them). yaml.v3 emits a TAIL-COMMENT event only for
//! comments, so it never appears here either.

use super::scanner::{ErrorKind, Mark, ScalarStyle, Scanner, SyntaxError, Token, TokenKind};

/// libyaml's `yaml_event_type_t`, with the event's values.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EventKind {
    StreamStart,
    StreamEnd,
    DocumentStart,
    DocumentEnd,
    Alias(String),
    Scalar {
        anchor: Option<String>,
        tag: String,
        value: String,
        style: ScalarStyle,
    },
    SequenceStart {
        anchor: Option<String>,
        tag: String,
        flow: bool,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<String>,
        tag: String,
        flow: bool,
    },
    MappingEnd,
}

/// An event and its start mark.
#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub(crate) kind: EventKind,
    pub(crate) start: Mark,
}

/// `yaml_parser_state_t`, without the two states libyaml declares but never
/// enters (`yaml_PARSE_BLOCK_NODE_OR_INDENTLESS_SEQUENCE_STATE`,
/// `yaml_PARSE_FLOW_NODE_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    StreamStart,
    ImplicitDocumentStart,
    DocumentStart,
    DocumentContent,
    DocumentEnd,
    BlockNode,
    BlockSequenceFirstEntry,
    BlockSequenceEntry,
    IndentlessSequenceEntry,
    BlockMappingFirstKey,
    BlockMappingKey,
    BlockMappingValue,
    FlowSequenceFirstEntry,
    FlowSequenceEntry,
    FlowSequenceEntryMappingKey,
    FlowSequenceEntryMappingValue,
    FlowSequenceEntryMappingEnd,
    FlowMappingFirstKey,
    FlowMappingKey,
    FlowMappingValue,
    FlowMappingEmptyValue,
    End,
}

/// `yaml_parser_t`'s parsing half.
pub(crate) struct Parser {
    scanner: Scanner,
    state: State,
    states: Vec<State>,
    marks: Vec<Mark>,
    tag_directives: Vec<(String, String)>,
    error: Option<SyntaxError>,
}

type ParseResult = Result<Event, SyntaxError>;

fn parser_error(problem: &str, problem_mark: Mark) -> SyntaxError {
    SyntaxError { kind: ErrorKind::Parser, problem: problem.to_owned(), context_mark: Mark::default(), problem_mark }
}

fn parser_error_context(context_mark: Mark, problem: &str, problem_mark: Mark) -> SyntaxError {
    SyntaxError { kind: ErrorKind::Parser, problem: problem.to_owned(), context_mark, problem_mark }
}

fn event(kind: EventKind, start: Mark) -> Event {
    Event { kind, start }
}

/// `yaml_parser_process_empty_scalar`.
fn empty_scalar(mark: Mark) -> ParseResult {
    Ok(event(
        EventKind::Scalar { anchor: None, tag: String::new(), value: String::new(), style: ScalarStyle::Plain },
        mark,
    ))
}

impl Parser {
    /// A parser reading tokens from `scanner`.
    pub(crate) fn new(scanner: Scanner) -> Self {
        Parser {
            scanner,
            state: State::StreamStart,
            states: Vec::new(),
            marks: Vec::new(),
            tag_directives: Vec::new(),
            error: None,
        }
    }

    /// `yaml_parser_parse`: the next event, or `None` after the end of the
    /// stream. An error is returned again on every later call.
    pub(crate) fn parse(&mut self) -> Result<Option<Event>, SyntaxError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.state == State::End {
            return Ok(None);
        }
        match self.state_machine() {
            Ok(e) => Ok(Some(e)),
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }

    fn peek(&mut self) -> Result<Token, SyntaxError> {
        self.scanner.peek().cloned()
    }

    fn skip(&mut self) {
        self.scanner.skip();
    }

    fn pop_state(&mut self) {
        // The states stack is never empty where libyaml pops it.
        self.state = self.states.pop().unwrap_or(State::End);
    }

    fn pop_mark(&mut self) -> Mark {
        self.marks.pop().unwrap_or_default()
    }

    /// `yaml_parser_state_machine`.
    fn state_machine(&mut self) -> ParseResult {
        match self.state {
            State::StreamStart => self.parse_stream_start(),
            State::ImplicitDocumentStart => self.parse_document_start(true),
            State::DocumentStart => self.parse_document_start(false),
            State::DocumentContent => self.parse_document_content(),
            State::DocumentEnd => self.parse_document_end(),
            State::BlockNode => self.parse_node(true, false),
            State::BlockSequenceFirstEntry => self.parse_block_sequence_entry(true),
            State::BlockSequenceEntry => self.parse_block_sequence_entry(false),
            State::IndentlessSequenceEntry => self.parse_indentless_sequence_entry(),
            State::BlockMappingFirstKey => self.parse_block_mapping_key(true),
            State::BlockMappingKey => self.parse_block_mapping_key(false),
            State::BlockMappingValue => self.parse_block_mapping_value(),
            State::FlowSequenceFirstEntry => self.parse_flow_sequence_entry(true),
            State::FlowSequenceEntry => self.parse_flow_sequence_entry(false),
            State::FlowSequenceEntryMappingKey => self.parse_flow_sequence_entry_mapping_key(),
            State::FlowSequenceEntryMappingValue => self.parse_flow_sequence_entry_mapping_value(),
            State::FlowSequenceEntryMappingEnd => self.parse_flow_sequence_entry_mapping_end(),
            State::FlowMappingFirstKey => self.parse_flow_mapping_key(true),
            State::FlowMappingKey => self.parse_flow_mapping_key(false),
            State::FlowMappingValue => self.parse_flow_mapping_value(false),
            State::FlowMappingEmptyValue => self.parse_flow_mapping_value(true),
            // parse() returns before.
            State::End => Err(parser_error("invalid parser state", Mark::default())),
        }
    }

    fn parse_stream_start(&mut self) -> ParseResult {
        let token = self.peek()?;
        if token.kind != TokenKind::StreamStart {
            return Err(parser_error("did not find expected <stream-start>", token.start));
        }
        self.state = State::ImplicitDocumentStart;
        self.skip();
        Ok(event(EventKind::StreamStart, token.start))
    }

    fn parse_document_start(&mut self, implicit: bool) -> ParseResult {
        let mut token = self.peek()?;
        // Parse extra document end indicators.
        if !implicit {
            while token.kind == TokenKind::DocumentEnd {
                self.skip();
                token = self.peek()?;
            }
        }
        let directive_or_start = matches!(
            token.kind,
            TokenKind::VersionDirective(..) | TokenKind::TagDirective(..) | TokenKind::DocumentStart | TokenKind::StreamEnd
        );
        if implicit && !directive_or_start {
            // Parse an implicit document.
            self.process_directives()?;
            self.states.push(State::DocumentEnd);
            self.state = State::BlockNode;
            Ok(event(EventKind::DocumentStart, token.start))
        } else if token.kind != TokenKind::StreamEnd {
            // Parse an explicit document.
            let start = token.start;
            self.process_directives()?;
            let token = self.peek()?;
            if token.kind != TokenKind::DocumentStart {
                return Err(parser_error("did not find expected <document start>", token.start));
            }
            self.states.push(State::DocumentEnd);
            self.state = State::DocumentContent;
            self.skip();
            Ok(event(EventKind::DocumentStart, start))
        } else {
            // Parse the stream end.
            self.state = State::End;
            self.skip();
            Ok(event(EventKind::StreamEnd, token.start))
        }
    }

    fn parse_document_content(&mut self) -> ParseResult {
        let token = self.peek()?;
        if matches!(
            token.kind,
            TokenKind::VersionDirective(..)
                | TokenKind::TagDirective(..)
                | TokenKind::DocumentStart
                | TokenKind::DocumentEnd
                | TokenKind::StreamEnd
        ) {
            self.pop_state();
            return empty_scalar(token.start);
        }
        self.parse_node(true, false)
    }

    fn parse_document_end(&mut self) -> ParseResult {
        let token = self.peek()?;
        if token.kind == TokenKind::DocumentEnd {
            self.skip();
        }
        self.tag_directives.clear();
        self.state = State::DocumentStart;
        Ok(event(EventKind::DocumentEnd, token.start))
    }

    fn parse_node(&mut self, block: bool, indentless_sequence: bool) -> ParseResult {
        let mut token = self.peek()?;
        if let TokenKind::Alias(name) = &token.kind {
            self.pop_state();
            let e = event(EventKind::Alias(name.clone()), token.start);
            self.skip();
            return Ok(e);
        }
        let mut start_mark = token.start;
        let mut tag_token: Option<(String, String)> = None;
        let mut tag_mark = Mark::default();
        let mut anchor: Option<String> = None;
        if let TokenKind::Anchor(name) = &token.kind {
            anchor = Some(name.clone());
            start_mark = token.start;
            self.skip();
            token = self.peek()?;
            if let TokenKind::Tag(handle, suffix) = &token.kind {
                tag_token = Some((handle.clone(), suffix.clone()));
                tag_mark = token.start;
                self.skip();
                token = self.peek()?;
            }
        } else if let TokenKind::Tag(handle, suffix) = &token.kind {
            tag_token = Some((handle.clone(), suffix.clone()));
            start_mark = token.start;
            tag_mark = token.start;
            self.skip();
            token = self.peek()?;
            if let TokenKind::Anchor(name) = &token.kind {
                anchor = Some(name.clone());
                self.skip();
                token = self.peek()?;
            }
        }

        let mut tag = String::new();
        if let Some((handle, suffix)) = tag_token {
            if handle.is_empty() {
                tag = suffix;
            } else {
                if let Some((_, prefix)) = self.tag_directives.iter().find(|(h, _)| *h == handle) {
                    tag = format!("{prefix}{suffix}");
                }
                if tag.is_empty() {
                    return Err(parser_error_context(start_mark, "found undefined tag handle", tag_mark));
                }
            }
        }

        if indentless_sequence && token.kind == TokenKind::BlockEntry {
            self.state = State::IndentlessSequenceEntry;
            return Ok(event(EventKind::SequenceStart { anchor, tag, flow: false }, start_mark));
        }
        match token.kind {
            TokenKind::Scalar(value, style) => {
                self.pop_state();
                self.skip();
                Ok(event(EventKind::Scalar { anchor, tag, value, style }, start_mark))
            }
            TokenKind::FlowSequenceStart => {
                self.state = State::FlowSequenceFirstEntry;
                Ok(event(EventKind::SequenceStart { anchor, tag, flow: true }, start_mark))
            }
            TokenKind::FlowMappingStart => {
                self.state = State::FlowMappingFirstKey;
                Ok(event(EventKind::MappingStart { anchor, tag, flow: true }, start_mark))
            }
            TokenKind::BlockSequenceStart if block => {
                self.state = State::BlockSequenceFirstEntry;
                Ok(event(EventKind::SequenceStart { anchor, tag, flow: false }, start_mark))
            }
            TokenKind::BlockMappingStart if block => {
                self.state = State::BlockMappingFirstKey;
                Ok(event(EventKind::MappingStart { anchor, tag, flow: false }, start_mark))
            }
            _ if anchor.is_some() || !tag.is_empty() => {
                self.pop_state();
                Ok(event(
                    EventKind::Scalar { anchor, tag, value: String::new(), style: ScalarStyle::Plain },
                    start_mark,
                ))
            }
            _ => Err(parser_error_context(start_mark, "did not find expected node content", token.start)),
        }
    }

    fn parse_block_sequence_entry(&mut self, first: bool) -> ParseResult {
        if first {
            let token = self.peek()?;
            self.marks.push(token.start);
            self.skip();
        }
        let token = self.peek()?;
        if token.kind == TokenKind::BlockEntry {
            let mark = token.end;
            self.skip();
            let token = self.peek()?;
            if token.kind != TokenKind::BlockEntry && token.kind != TokenKind::BlockEnd {
                self.states.push(State::BlockSequenceEntry);
                return self.parse_node(true, false);
            }
            self.state = State::BlockSequenceEntry;
            return empty_scalar(mark);
        }
        if token.kind == TokenKind::BlockEnd {
            self.pop_state();
            self.pop_mark();
            self.skip();
            return Ok(event(EventKind::SequenceEnd, token.start));
        }
        let context_mark = self.pop_mark();
        Err(parser_error_context(context_mark, "did not find expected '-' indicator", token.start))
    }

    fn parse_indentless_sequence_entry(&mut self) -> ParseResult {
        let token = self.peek()?;
        if token.kind == TokenKind::BlockEntry {
            let mark = token.end;
            self.skip();
            let token = self.peek()?;
            if !matches!(token.kind, TokenKind::BlockEntry | TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd) {
                self.states.push(State::IndentlessSequenceEntry);
                return self.parse_node(true, false);
            }
            self.state = State::IndentlessSequenceEntry;
            return empty_scalar(mark);
        }
        self.pop_state();
        Ok(event(EventKind::SequenceEnd, token.start))
    }

    fn parse_block_mapping_key(&mut self, first: bool) -> ParseResult {
        if first {
            let token = self.peek()?;
            self.marks.push(token.start);
            self.skip();
        }
        let token = self.peek()?;
        if token.kind == TokenKind::Key {
            let mark = token.end;
            self.skip();
            let token = self.peek()?;
            if !matches!(token.kind, TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd) {
                self.states.push(State::BlockMappingValue);
                return self.parse_node(true, true);
            }
            self.state = State::BlockMappingValue;
            return empty_scalar(mark);
        }
        if token.kind == TokenKind::BlockEnd {
            self.pop_state();
            self.pop_mark();
            self.skip();
            return Ok(event(EventKind::MappingEnd, token.start));
        }
        let context_mark = self.pop_mark();
        Err(parser_error_context(context_mark, "did not find expected key", token.start))
    }

    fn parse_block_mapping_value(&mut self) -> ParseResult {
        let token = self.peek()?;
        if token.kind == TokenKind::Value {
            let mark = token.end;
            self.skip();
            let token = self.peek()?;
            if !matches!(token.kind, TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd) {
                self.states.push(State::BlockMappingKey);
                return self.parse_node(true, true);
            }
            self.state = State::BlockMappingKey;
            return empty_scalar(mark);
        }
        self.state = State::BlockMappingKey;
        empty_scalar(token.start)
    }

    fn parse_flow_sequence_entry(&mut self, first: bool) -> ParseResult {
        if first {
            let token = self.peek()?;
            self.marks.push(token.start);
            self.skip();
        }
        let mut token = self.peek()?;
        if token.kind != TokenKind::FlowSequenceEnd {
            if !first {
                if token.kind == TokenKind::FlowEntry {
                    self.skip();
                    token = self.peek()?;
                } else {
                    let context_mark = self.pop_mark();
                    return Err(parser_error_context(context_mark, "did not find expected ',' or ']'", token.start));
                }
            }
            if token.kind == TokenKind::Key {
                self.state = State::FlowSequenceEntryMappingKey;
                self.skip();
                return Ok(event(EventKind::MappingStart { anchor: None, tag: String::new(), flow: true }, token.start));
            } else if token.kind != TokenKind::FlowSequenceEnd {
                self.states.push(State::FlowSequenceEntry);
                return self.parse_node(false, false);
            }
        }
        self.pop_state();
        self.pop_mark();
        self.skip();
        Ok(event(EventKind::SequenceEnd, token.start))
    }

    fn parse_flow_sequence_entry_mapping_key(&mut self) -> ParseResult {
        let token = self.peek()?;
        if !matches!(token.kind, TokenKind::Value | TokenKind::FlowEntry | TokenKind::FlowSequenceEnd) {
            self.states.push(State::FlowSequenceEntryMappingValue);
            return self.parse_node(false, false);
        }
        let mark = token.end;
        self.skip();
        self.state = State::FlowSequenceEntryMappingValue;
        empty_scalar(mark)
    }

    fn parse_flow_sequence_entry_mapping_value(&mut self) -> ParseResult {
        let mut token = self.peek()?;
        if token.kind == TokenKind::Value {
            self.skip();
            token = self.peek()?;
            if !matches!(token.kind, TokenKind::FlowEntry | TokenKind::FlowSequenceEnd) {
                self.states.push(State::FlowSequenceEntryMappingEnd);
                return self.parse_node(false, false);
            }
        }
        self.state = State::FlowSequenceEntryMappingEnd;
        empty_scalar(token.start)
    }

    fn parse_flow_sequence_entry_mapping_end(&mut self) -> ParseResult {
        let token = self.peek()?;
        self.state = State::FlowSequenceEntry;
        Ok(event(EventKind::MappingEnd, token.start))
    }

    fn parse_flow_mapping_key(&mut self, first: bool) -> ParseResult {
        if first {
            let token = self.peek()?;
            self.marks.push(token.start);
            self.skip();
        }
        let mut token = self.peek()?;
        if token.kind != TokenKind::FlowMappingEnd {
            if !first {
                if token.kind == TokenKind::FlowEntry {
                    self.skip();
                    token = self.peek()?;
                } else {
                    let context_mark = self.pop_mark();
                    return Err(parser_error_context(context_mark, "did not find expected ',' or '}'", token.start));
                }
            }
            if token.kind == TokenKind::Key {
                self.skip();
                token = self.peek()?;
                if !matches!(token.kind, TokenKind::Value | TokenKind::FlowEntry | TokenKind::FlowMappingEnd) {
                    self.states.push(State::FlowMappingValue);
                    return self.parse_node(false, false);
                }
                self.state = State::FlowMappingValue;
                return empty_scalar(token.start);
            } else if token.kind != TokenKind::FlowMappingEnd {
                self.states.push(State::FlowMappingEmptyValue);
                return self.parse_node(false, false);
            }
        }
        self.pop_state();
        self.pop_mark();
        self.skip();
        Ok(event(EventKind::MappingEnd, token.start))
    }

    fn parse_flow_mapping_value(&mut self, empty: bool) -> ParseResult {
        let mut token = self.peek()?;
        if empty {
            self.state = State::FlowMappingKey;
            return empty_scalar(token.start);
        }
        if token.kind == TokenKind::Value {
            self.skip();
            token = self.peek()?;
            if !matches!(token.kind, TokenKind::FlowEntry | TokenKind::FlowMappingEnd) {
                self.states.push(State::FlowMappingKey);
                return self.parse_node(false, false);
            }
        }
        self.state = State::FlowMappingKey;
        empty_scalar(token.start)
    }

    /// `yaml_parser_process_directives`.
    fn process_directives(&mut self) -> Result<(), SyntaxError> {
        let mut version_seen = false;
        let mut token = self.peek()?;
        loop {
            match &token.kind {
                TokenKind::VersionDirective(major, minor) => {
                    if version_seen {
                        return Err(parser_error("found duplicate %YAML directive", token.start));
                    }
                    if *major != 1 || *minor != 1 {
                        return Err(parser_error("found incompatible YAML document", token.start));
                    }
                    version_seen = true;
                }
                TokenKind::TagDirective(handle, prefix) => {
                    self.append_tag_directive(handle, prefix, false, token.start)?;
                }
                _ => break,
            }
            self.skip();
            token = self.peek()?;
        }
        self.append_tag_directive("!", "!", true, token.start)?;
        self.append_tag_directive("!!", "tag:yaml.org,2002:", true, token.start)?;
        Ok(())
    }

    /// `yaml_parser_append_tag_directive`.
    fn append_tag_directive(&mut self, handle: &str, prefix: &str, allow_duplicates: bool, mark: Mark) -> Result<(), SyntaxError> {
        if self.tag_directives.iter().any(|(h, _)| h == handle) {
            if allow_duplicates {
                return Ok(());
            }
            return Err(parser_error("found duplicate %TAG directive", mark));
        }
        self.tag_directives.push((handle.to_owned(), prefix.to_owned()));
        Ok(())
    }
}
