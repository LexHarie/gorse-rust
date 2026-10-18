// Ports: github.com/expr-lang/expr/parser/lexer/token.go @ v1.17.8

//! Tokens.

use std::fmt;

use crate::file::Location;
use crate::gostr;

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A name, like `foo`, or `if`/`else` when the `if` operator is off.
    Identifier,
    /// A number literal, as written.
    Number,
    /// A string literal, unescaped.
    String,
    /// A bytes literal, unescaped.
    Bytes,
    /// An operator, including the word operators like `and` and `not`.
    Operator,
    /// One of `(`, `)`, `[`, `]`, `{` and `}`.
    Bracket,
    /// The end of the source.
    Eof,
}

impl Kind {
    /// The Go name of the kind, like `Identifier`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "Identifier",
            Self::Number => "Number",
            Self::String => "String",
            Self::Bytes => "Bytes",
            Self::Operator => "Operator",
            Self::Bracket => "Bracket",
            Self::Eof => "EOF",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A token of the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Where it is.
    pub location: Location,
    /// What it is.
    pub kind: Kind,
    /// Its text; for strings and bytes, the unescaped value. Bytes rather
    /// than a `String` because a Go string is bytes, and an escape can leave
    /// bytes that are not UTF-8 (see [`crate::ast::NodeKind::String`]).
    pub value: Vec<u8>,
}

impl Token {
    /// `Is`: the token is of `kind` and, when `values` is not empty, its
    /// value is one of them.
    pub fn is(&self, kind: Kind, values: &[&str]) -> bool {
        if kind != self.kind {
            return false;
        }
        values.is_empty() || values.iter().any(|v| v.as_bytes() == self.value)
    }

    /// The value as text: `None` when it is not UTF-8, which only a string
    /// or bytes token can be, and which no name or operator equals.
    pub fn value_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.value).ok()
    }

    /// The value as an owned string, for a value that came from the source
    /// and so is UTF-8 (anything but a string or bytes token).
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.value).into_owned()
    }
}

/// `String`: the kind, then the value quoted when it is not empty, like
/// `Identifier("foo")`.
impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.value.is_empty() {
            return f.write_str(self.kind.as_str());
        }
        write!(f, "{}({})", self.kind, gostr::quote(&self.value))
    }
}
