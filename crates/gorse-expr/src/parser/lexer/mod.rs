// Ports: github.com/expr-lang/expr/parser/lexer/lexer.go @ v1.17.8

//! The lexer: a state machine that queues tokens as the parser asks for
//! them, as Go's does, so an error late in the source is only met when the
//! parser gets there.

mod state;
mod token;
mod utils;

use std::collections::VecDeque;

pub use token::{Kind, Token};

use crate::file::{Error, Location, Source};
use crate::gostr;
use crate::parser::utils as chars;
use state::State;

/// `Lex`: all the tokens of `source`, ending with [`Kind::Eof`].
pub fn lex(source: &Source) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    let mut l = Lexer::new(source);
    while let Some(t) = l.next_token()? {
        tokens.push(t);
    }
    Ok(tokens)
}

/// A position in the source.
#[derive(Debug, Clone, Copy, Default)]
struct Pos {
    byte: usize,
    rune: usize,
}

/// The lexer of one source.
///
/// Go's lexer is reusable through `Reset`, which keeps the positions of the
/// previous source; one lexer per source keeps clear of that (Q-WP119-9).
#[derive(Debug)]
pub struct Lexer<'a> {
    state: Option<State>,
    source: &'a Source,
    tokens: VecDeque<Token>,
    err: Option<Error>,
    start: Pos,
    end: Pos,
    eof: bool,
    /// When true, keywords `if`/`else` are not treated as operators and
    /// will be emitted as identifiers instead (for compatibility with custom
    /// if()).
    pub disable_if_operator: bool,
}

impl<'a> Lexer<'a> {
    /// A lexer at the start of `source`.
    pub fn new(source: &'a Source) -> Self {
        Self {
            state: Some(State::Root),
            source,
            tokens: VecDeque::new(),
            err: None,
            start: Pos::default(),
            end: Pos::default(),
            eof: false,
            disable_if_operator: false,
        }
    }

    /// `Next`: the next token, `None` after [`Kind::Eof`], or the first
    /// error, which every later call returns again.
    pub fn next_token(&mut self) -> Result<Option<Token>, Error> {
        while let Some(state) = self.state
            && self.err.is_none()
            && self.tokens.is_empty()
        {
            self.state = state.run(self);
        }
        if let Some(err) = &self.err {
            return Err(err.clone().bind(self.source));
        }
        Ok(self.tokens.pop_front())
    }

    fn src(&self) -> &'a str {
        self.source.as_str()
    }

    fn commit(&mut self) {
        self.start = self.end;
    }

    /// The next character, `None` (Go: `eof`) at the end.
    fn next(&mut self) -> Option<char> {
        let Some(r) = self.src()[self.end.byte..].chars().next() else {
            self.eof = true;
            return None;
        };
        self.end.rune += 1;
        self.end.byte += r.len_utf8();
        Some(r)
    }

    fn peek(&self) -> Option<char> {
        self.src()[self.end.byte..].chars().next()
    }

    /// Undoes one [`Lexer::next`]; after the end, only forgets having
    /// reached it.
    fn backup(&mut self) {
        if self.eof {
            self.eof = false;
        } else if self.end.rune > 0
            && let Some(r) = self.src()[..self.end.byte].chars().next_back()
        {
            self.end.byte -= r.len_utf8();
            self.end.rune -= 1;
        }
    }

    fn emit(&mut self, kind: Kind) {
        let value = self.word().as_bytes().to_vec();
        self.emit_value(kind, value);
    }

    fn emit_value(&mut self, kind: Kind, value: Vec<u8>) {
        self.tokens.push_back(Token {
            location: Location {
                from: self.start.rune,
                to: self.end.rune,
            },
            kind,
            value,
        });
        self.commit();
    }

    fn emit_eof(&mut self) {
        self.tokens.push_back(Token {
            location: Location {
                from: self.end.rune.saturating_sub(1),
                to: self.end.rune,
            },
            kind: Kind::Eof,
            value: Vec::new(),
        });
        self.commit();
    }

    fn skip(&mut self) {
        self.commit();
    }

    fn word(&self) -> &'a str {
        &self.src()[self.start.byte..self.end.byte]
    }

    fn accept(&mut self, valid: &str) -> bool {
        if self.peek().is_some_and(|r| valid.contains(r)) {
            self.next();
            return true;
        }
        false
    }

    fn accept_run(&mut self, valid: &str) {
        while self.accept(valid) {}
    }

    fn skip_spaces(&mut self) {
        self.accept_run(" ");
        self.skip();
    }

    /// Records the first error, at the last character read (or just past
    /// the end), and stops the lexer.
    fn error(&mut self, message: impl FnOnce() -> String) -> Option<State> {
        if self.err.is_none() {
            // show first error
            let mut end = self.end.rune;
            if self.eof {
                end += 1;
            }
            self.err = Some(Error::new(
                Location {
                    // An error always follows a character read or the end.
                    from: end.saturating_sub(1),
                    to: end,
                },
                message(),
            ));
        }
        None
    }

    fn scan_digits(&mut self, mut ch: Option<char>, base: u32, mut n: u32) -> Option<char> {
        while n > 0 && digit_val(ch) < base {
            ch = self.next();
            n -= 1;
        }
        if n > 0 {
            self.error(|| "invalid char escape".to_owned());
        }
        ch
    }

    fn scan_escape(&mut self, quote: char) -> Option<char> {
        let ch = self.next(); // read character after '/'
        match ch {
            Some('a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\') => self.next(),
            Some(c) if c == quote => self.next(),
            Some(c @ '0'..='7') => self.scan_digits(Some(c), 8, 3),
            Some('x') => {
                let ch = self.next();
                self.scan_digits(ch, 16, 2)
            }
            Some('u') => {
                // Support variable-length form: \u{XXXXXX}
                if self.peek() == Some('{') {
                    // consume '{'
                    self.next();
                    // read 1-6 hex digits
                    let mut digits = 0;
                    loop {
                        let p = self.peek();
                        if p == Some('}') {
                            break;
                        }
                        if digit_val(p) >= 16 || digits >= 6 {
                            self.error(|| "invalid char escape".to_owned());
                            return None;
                        }
                        self.next();
                        digits += 1;
                    }
                    if digits == 0 {
                        self.error(|| "invalid char escape".to_owned());
                        return None;
                    }
                    // consume '}' and continue
                    self.next();
                    return self.next();
                }
                let ch = self.next();
                self.scan_digits(ch, 16, 4)
            }
            Some('U') => {
                let ch = self.next();
                self.scan_digits(ch, 16, 8)
            }
            _ => {
                self.error(|| "invalid char escape".to_owned());
                ch
            }
        }
    }

    fn scan_string(&mut self, quote: char) {
        let mut ch = self.next(); // read character after quote
        while ch != Some(quote) {
            if matches!(ch, Some('\n') | None) {
                self.error(|| "literal not terminated".to_owned());
                return;
            }
            if ch == Some('\\') {
                ch = self.scan_escape(quote);
            } else {
                ch = self.next();
            }
        }
    }

    fn scan_raw_string(&mut self, quote: char) {
        let mut escaped_quotes = 0;
        loop {
            let mut ch = self.next();
            while ch == Some(quote) && self.peek() == Some(quote) {
                // skip current and next char which are the quote escape
                // sequence
                self.next();
                ch = self.next();
                escaped_quotes += 1;
            }
            match ch {
                Some(c) if c == quote => break,
                None => {
                    self.error(|| "literal not terminated".to_owned());
                    return;
                }
                Some(_) => {}
            }
        }
        let s = &self.src()[self.start.byte + 1..self.end.byte - 1];

        // handle simple case where no quoted backtick was found
        if escaped_quotes == 0 {
            self.emit_value(Kind::String, s.as_bytes().to_vec());
            return;
        }

        let mut b = String::with_capacity(s.len() - escaped_quotes);
        let mut skipped = false;
        for r in s.chars() {
            if r == quote {
                if !skipped {
                    skipped = true;
                    continue;
                }
                skipped = false;
            }
            b.push(r);
        }
        self.emit_value(Kind::String, b.into_bytes());
    }

    fn unrecognized(&mut self, r: char) -> Option<State> {
        self.error(|| format!("unrecognized character: {}", gostr::format_unicode_sharp(r)))
    }
}

/// `digitVal`: the value of a hex digit, 16 for anything else.
fn digit_val(ch: Option<char>) -> u32 {
    match ch {
        Some(c @ '0'..='9') => c as u32 - '0' as u32,
        Some(c @ ('a'..='f' | 'A'..='F')) => (c as u32 | 0x20) - 'a' as u32 + 10,
        _ => 16, // larger than any legal digit val
    }
}

fn is_alpha_numeric(r: Option<char>) -> bool {
    r.is_some_and(chars::is_alpha_numeric)
}
