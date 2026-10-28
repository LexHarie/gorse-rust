// Ports: the regexp.Compile that github.com/expr-lang/expr uses for `matches` (regexp/regexp.go and regexp/syntax's Error) @ go1.27.0. The matcher is the regex crate; the error texts are Go's.

//! Compiling the patterns of `matches`. Go's RE2 syntax and the regex crate's
//! agree on the patterns Gorse's expressions use; a pattern one of them
//! rejects is rejected with the text Go gives.

use regex::bytes::Regex;
use regex_syntax::ast::{ErrorKind, parse::Parser};

/// Compiles a pattern, as Go's `regexp.Compile` does: its error text is
/// `error parsing regexp: CODE: `EXPR``.
pub fn compile(pattern: impl AsRef<[u8]>) -> Result<Regex, String> {
    let pattern = pattern.as_ref();
    let Ok(text) = std::str::from_utf8(pattern) else {
        return Err("error parsing regexp: invalid UTF-8: `".to_owned()
            + &String::from_utf8_lossy(pattern)
            + "`");
    };
    if let Err(err) = Parser::new().parse(text) {
        let span = err.span();
        let (start, end) = (span.start.offset, span.end.offset);
        let fragment = |a: usize, b: usize| text.get(a..b).unwrap_or(text).to_owned();
        let (code, expr) = match err.kind() {
            ErrorKind::GroupUnclosed => ("missing closing )", text.to_owned()),
            ErrorKind::GroupUnopened => ("unexpected )", text.to_owned()),
            ErrorKind::ClassUnclosed => (
                "missing closing ]",
                text.get(start..).unwrap_or(text).to_owned(),
            ),
            ErrorKind::ClassRangeInvalid | ErrorKind::ClassRangeLiteral => {
                ("invalid character class range", fragment(start, end))
            }
            ErrorKind::EscapeUnrecognized => ("invalid escape sequence", fragment(start, end)),
            ErrorKind::RepetitionMissing => (
                "missing argument to repetition operator",
                fragment(start, end),
            ),
            ErrorKind::RepetitionCountInvalid
            | ErrorKind::RepetitionCountUnclosed
            | ErrorKind::RepetitionCountDecimalEmpty => {
                ("invalid repeat count", fragment(start, end))
            }
            ErrorKind::NestLimitExceeded(_) => ("expression nests too deeply", text.to_owned()),
            _ => ("invalid or unsupported Perl syntax", fragment(start, end)),
        };
        return Err(format!("error parsing regexp: {code}: `{expr}`"));
    }
    Regex::new(text).map_err(|err| format!("error parsing regexp: {err}"))
}
