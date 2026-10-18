// Ports: github.com/expr-lang/expr/file/{location,source,error}.go @ v1.17.8

//! The source of an expression, spans in it, and errors that point into it.

use std::fmt;

/// A span of a [`Source`], counted in characters (Go: runes) from the start:
/// `from` is the first character, `to` the one after the last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Location {
    /// The first character.
    pub from: usize,
    /// The character after the last.
    pub to: usize,
}

/// The text of an expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Source {
    raw: String,
}

impl Source {
    /// Go: `NewSource`.
    pub fn new(contents: impl Into<String>) -> Self {
        Self {
            raw: contents.into(),
        }
    }

    /// The text (Go: `String`).
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Line `line` of the source, counting from 1, without its newline; `None`
    /// past the last line and for an empty source. A line at or below 1 is the
    /// first.
    pub fn snippet(&self, line: usize) -> Option<&str> {
        if self.raw.is_empty() {
            return None;
        }
        let mut start = 0;
        for _ in 1..line {
            let pos = self.raw[start..].find('\n')?;
            start += pos + 1;
        }
        let end = self.raw[start..]
            .find('\n')
            .map_or(self.raw.len(), |pos| start + pos);
        Some(&self.raw[start..end])
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// An error at a [`Location`] of a source. Once bound to the source
/// ([`Error::bind`]) it carries the line, the column and a snippet showing
/// where it is, which [`Display`](fmt::Display) appends to the message.
///
/// Go's `Prev` (the wrapped error) is left out: nothing in the parser sets it
/// but an unreachable branch; WP-3-6 adds it when the VM wraps errors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Error {
    /// Where the error is.
    pub location: Location,
    /// The line of `location.from`, from 1. Set by [`Error::bind`].
    pub line: usize,
    /// The column of `location.from` in its line, from 0. Set by
    /// [`Error::bind`].
    pub column: usize,
    /// What went wrong.
    pub message: String,
    /// The line of the error and a `^` under its column, each after
    /// `"\n | "`. Set by [`Error::bind`]; empty when the line is.
    pub snippet: String,
}

impl Error {
    /// An unbound error.
    pub fn new(location: Location, message: impl Into<String>) -> Self {
        Self {
            location,
            message: message.into(),
            ..Self::default()
        }
    }

    /// Computes the line, column and snippet of the error in `source`.
    ///
    /// Like Go, the snippet is left as it was when the error's line is
    /// empty, and tabs in the line are shown as spaces.
    #[must_use]
    pub fn bind(mut self, source: &Source) -> Self {
        let src = source.as_str();
        let mut line_start = 0;
        self.line = 1;
        self.column = 0;
        for (rune_count, (i, r)) in src.char_indices().enumerate() {
            if rune_count == self.location.from {
                break;
            }
            if r == '\n' {
                line_start = i + 1;
                self.line += 1;
                self.column = 0;
            } else {
                self.column += 1;
            }
        }
        let line_end = src[line_start..]
            .find('\n')
            .map_or(src.len(), |pos| line_start + pos);
        if line_start == line_end {
            return self;
        }
        const PREFIX: &str = "\n | ";
        let line = &src[line_start..line_end];
        let mut snippet = String::with_capacity(2 * PREFIX.len() + line.len() + self.column + 1);
        snippet.push_str(PREFIX);
        snippet.push_str(&line.replace('\t', " "));
        snippet.push_str(PREFIX);
        snippet.extend(std::iter::repeat_n('.', self.column));
        snippet.push('^');
        self.snippet = snippet;
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.snippet.is_empty() {
            return f.write_str(&self.message);
        }
        // The column is shown from 1.
        write!(
            f,
            "{} ({}:{}){}",
            self.message,
            self.line,
            self.column + 1,
            self.snippet
        )
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    // file/source_test.go
    #[test]
    fn string_source_snippet_multi_line() {
        let source = Source::new("hello\nworld\nmy\nbub\n");
        assert_eq!(source.snippet(1), Some("hello"));
        assert_eq!(source.snippet(2), Some("world"));
        assert_eq!(source.snippet(3), Some("my"));
        assert_eq!(source.snippet(4), Some("bub"));
        assert_eq!(source.snippet(5), Some(""));
    }

    #[test]
    fn string_source_snippet_single_line() {
        let source = Source::new("hello, world");
        assert_eq!(source.snippet(1), Some("hello, world"));
        assert_eq!(source.snippet(2), None);
    }
}
