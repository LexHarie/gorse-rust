// Ports: github.com/expr-lang/expr/parser/lexer/state.go @ v1.17.8

//! The states of the lexer. Go's `stateFn` values are functions returning
//! the next state; here they are the variants of [`State`].

use super::utils::{unescape, unescape_bytes};
use super::{Kind, Lexer, is_alpha_numeric};
use crate::gostr;
use crate::parser::utils as chars;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Root,
    Number,
    Dot,
    Identifier,
    Not,
    QuestionMark,
    Slash,
    SingleLineComment,
    MultiLineComment,
    Pointer,
}

impl State {
    /// Runs the state; `None` stops the lexer.
    pub(super) fn run(self, l: &mut Lexer<'_>) -> Option<State> {
        match self {
            Self::Root => root(l),
            Self::Number => number(l),
            Self::Dot => dot(l),
            Self::Identifier => identifier(l),
            Self::Not => not(l),
            Self::QuestionMark => question_mark(l),
            Self::Slash => slash(l),
            Self::SingleLineComment => single_line_comment(l),
            Self::MultiLineComment => multi_line_comment(l),
            Self::Pointer => pointer(l),
        }
    }
}

fn root(l: &mut Lexer<'_>) -> Option<State> {
    let Some(r) = l.next() else {
        l.emit_eof();
        return None;
    };
    match r {
        _ if chars::is_space(r) => {
            l.skip();
            return Some(State::Root);
        }
        '\'' | '"' => {
            l.scan_string(r);
            match unescape(l.word()) {
                Ok(s) => l.emit_value(Kind::String, s),
                Err(err) => {
                    l.error(|| err.to_owned());
                    // Go queues the token anyway; the error comes first.
                    l.emit_value(Kind::String, Vec::new());
                }
            }
        }
        '`' => l.scan_raw_string(r),
        'b' | 'B' if matches!(l.peek(), Some('\'' | '"')) => {
            if let Some(quote) = l.next() {
                l.scan_string(quote);
            }
            match unescape_bytes(&l.word()[1..]) {
                // skip 'b'
                Ok(s) => l.emit_value(Kind::Bytes, s),
                Err(err) => {
                    l.error(|| err.to_owned());
                    l.emit_value(Kind::Bytes, Vec::new());
                }
            }
        }
        '0'..='9' => {
            l.backup();
            return Some(State::Number);
        }
        '?' => return Some(State::QuestionMark),
        '/' => return Some(State::Slash),
        '#' => return Some(State::Pointer),
        '|' => {
            l.accept("|");
            l.emit(Kind::Operator);
        }
        ':' => {
            l.accept(":");
            l.emit(Kind::Operator);
        }
        '(' | '[' | '{' | ')' | ']' | '}' => l.emit(Kind::Bracket),
        // single rune operator
        ',' | ';' | '%' | '+' | '-' | '^' => l.emit(Kind::Operator),
        // possible double rune operator
        '&' | '!' | '=' | '*' | '<' | '>' => {
            l.accept("&=*");
            l.emit(Kind::Operator);
        }
        '.' => {
            l.backup();
            return Some(State::Dot);
        }
        _ if chars::is_alpha_numeric(r) => {
            l.backup();
            return Some(State::Identifier);
        }
        _ => return l.unrecognized(r),
    }
    Some(State::Root)
}

fn number(l: &mut Lexer<'_>) -> Option<State> {
    if !scan_number(l) {
        let message = format!("bad number syntax: {}", gostr::quote(l.word().as_bytes()));
        return l.error(|| message);
    }
    l.emit(Kind::Number);
    Some(State::Root)
}

fn scan_number(l: &mut Lexer<'_>) -> bool {
    let mut digits = "0123456789_";
    // Is it hex?
    if l.accept("0") {
        // Note: Leading 0 does not mean octal in floats.
        if l.accept("xX") {
            digits = "0123456789abcdefABCDEF_";
        } else if l.accept("oO") {
            digits = "01234567_";
        } else if l.accept("bB") {
            digits = "01_";
        }
    }
    l.accept_run(digits);
    let end = l.end;
    if l.accept(".") {
        // Lookup for .. operator: if after dot there is another dot (1..2),
        // it maybe a range operator.
        if l.peek() == Some('.') {
            // We can't backup() here, as it would require two backups, and
            // backup() func supports only one for now. So, save and restore
            // it here.
            l.end = end;
            return true;
        }
        l.accept_run(digits);
    }
    if l.accept("eE") {
        l.accept("+-");
        l.accept_run(digits);
    }
    // Next thing mustn't be alphanumeric.
    if is_alpha_numeric(l.peek()) {
        l.next();
        return false;
    }
    true
}

fn dot(l: &mut Lexer<'_>) -> Option<State> {
    l.next();
    if l.accept("0123456789") {
        l.backup();
        return Some(State::Number);
    }
    l.accept(".");
    l.emit(Kind::Operator);
    Some(State::Root)
}

fn identifier(l: &mut Lexer<'_>) -> Option<State> {
    while is_alpha_numeric(l.next()) {
        // absorb
    }
    l.backup();
    match l.word() {
        "not" => return Some(State::Not),
        "in" | "or" | "and" | "matches" | "contains" | "startsWith" | "endsWith" | "let" => {
            l.emit(Kind::Operator);
        }
        "if" | "else" if !l.disable_if_operator => l.emit(Kind::Operator),
        _ => l.emit(Kind::Identifier),
    }
    Some(State::Root)
}

fn not(l: &mut Lexer<'_>) -> Option<State> {
    l.emit(Kind::Operator);

    l.skip_spaces();

    let end = l.end;

    // Get the next word.
    while is_alpha_numeric(l.next()) {
        // absorb
    }
    l.backup();

    match l.word() {
        "in" | "matches" | "contains" | "startsWith" | "endsWith" => l.emit(Kind::Operator),
        _ => l.end = end,
    }
    Some(State::Root)
}

fn question_mark(l: &mut Lexer<'_>) -> Option<State> {
    l.accept(".?");
    l.emit(Kind::Operator);
    Some(State::Root)
}

fn slash(l: &mut Lexer<'_>) -> Option<State> {
    if l.accept("/") {
        return Some(State::SingleLineComment);
    }
    if l.accept("*") {
        return Some(State::MultiLineComment);
    }
    l.emit(Kind::Operator);
    Some(State::Root)
}

fn single_line_comment(l: &mut Lexer<'_>) -> Option<State> {
    while !matches!(l.next(), None | Some('\n')) {}
    l.skip();
    Some(State::Root)
}

fn multi_line_comment(l: &mut Lexer<'_>) -> Option<State> {
    loop {
        match l.next() {
            None => return l.error(|| "unclosed comment".to_owned()),
            Some('*') if l.accept("/") => break,
            Some(_) => {}
        }
    }
    l.skip();
    Some(State::Root)
}

fn pointer(l: &mut Lexer<'_>) -> Option<State> {
    l.accept("#");
    l.emit(Kind::Operator);
    while is_alpha_numeric(l.next()) {
        // absorb
    }
    l.backup();
    if !l.word().is_empty() {
        l.emit(Kind::Identifier);
    }
    Some(State::Root)
}
