// Ports: common/expression/expression.go @ add21b5

//! Feedback type expressions: a feedback type, optionally compared with the
//! feedback value, such as `star`, `read>=3` or `rating<2.5`. They select
//! positive, negative and read feedback in the config
//! (`recommend.data_source.*_feedback_types`), REST paths and data store
//! queries.
//!
//! The text form is `<feedback_type>[<operator><value>]`, matched with the Go
//! pattern itself. The Go behaviour is kept where it surprises
//! (`porting/notes/WP-1-7.md`): `=` parses but means "any value", a value
//! without an operator is read and ignored, [`FeedbackTypeExpression::from_string`]
//! sets fields before it fails and keeps the old value when the text has
//! none, and the text form of a value of 1e6 or more is exponent notation that
//! does not parse back.

use std::fmt;
use std::sync::LazyLock;

use gorse_protocol as pb;
use regex::Regex;

/// `expressionPattern` in Go, unchanged.
static EXPRESSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<feedback_type>[a-zA-Z][a-zA-Z0-9_]*)(?P<expr_type><=|>=|<|>|=)?(?P<value>[0-9]*\.?[0-9]*)$",
    )
    .expect("the expression pattern compiles")
});

/// An error from parsing or decoding a feedback type expression. The
/// messages are Go's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The text does not match `<feedback_type>[<operator><value>]`.
    #[error("invalid expression format, expected format: <feedback_type>[<operator><value>]")]
    InvalidFormat,
    /// The value is not a number `strconv.ParseFloat` accepts.
    #[error("invalid value: {0}")]
    InvalidValue(#[source] ParseFloatError),
    /// The JSON value given to `unmarshal_json` is not a string; holds Go's
    /// name for its kind (`number`, `bool`, `array` or `object`).
    #[error("json: cannot unmarshal {0} into Go value of type string")]
    NotAString(&'static str),
    /// The input of `unmarshal_json` is not JSON. The text is serde_json's,
    /// not Go's; Go's decoders never pass such input to `UnmarshalJSON`.
    #[error("{0}")]
    Syntax(String),
    /// `unmarshal_json` failed with the wrapped error.
    #[error("unmarshal FeedbackTypeExpression: {0}")]
    Unmarshal(#[source] Box<Error>),
}

/// Go's `*strconv.NumError` from `strconv.ParseFloat`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("strconv.ParseFloat: parsing \"{num}\": {kind}")]
pub struct ParseFloatError {
    /// The input. The pattern admits only ASCII digits and `.`, which Go's
    /// `strconv.Quote` leaves as they are, so it is printed between plain
    /// quotes.
    pub num: String,
    /// Why parsing failed.
    pub kind: ParseFloatErrorKind,
}

/// `strconv.ErrSyntax` or `strconv.ErrRange`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseFloatErrorKind {
    /// Not a number, such as `.`.
    #[error("invalid syntax")]
    Syntax,
    /// More than half a ULP above the largest `f64`; Go returns `+Inf` with
    /// it.
    #[error("value out of range")]
    Range,
}

/// The comparison of an expression (Go `ExprType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExprType {
    /// No comparison: every value matches.
    #[default]
    None,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
}

impl fmt::Display for ExprType {
    /// The operator, or nothing for [`ExprType::None`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ExprType::None => "",
            ExprType::Less => "<",
            ExprType::LessOrEqual => "<=",
            ExprType::Greater => ">",
            ExprType::GreaterOrEqual => ">=",
        })
    }
}

/// A feedback type, optionally compared with the feedback value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FeedbackTypeExpression {
    /// The feedback type to match.
    pub feedback_type: String,
    /// How the feedback value is compared with [`Self::value`].
    pub expr_type: ExprType,
    /// The value compared against; unused for [`ExprType::None`].
    pub value: f64,
}

impl fmt::Display for FeedbackTypeExpression {
    /// Go `String`: the feedback type, then the operator and the value as Go's
    /// `%v` prints it. [`ExprType::None`] prints the feedback type alone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.expr_type == ExprType::None {
            f.write_str(&self.feedback_type)
        } else {
            write!(
                f,
                "{}{}{}",
                self.feedback_type,
                self.expr_type,
                format_float_v(self.value)
            )
        }
    }
}

impl std::str::FromStr for FeedbackTypeExpression {
    type Err = Error;

    /// Parses into a zero value, as Go callers do with
    /// `var e FeedbackTypeExpression; e.FromString(s)`.
    fn from_str(s: &str) -> Result<Self, Error> {
        let mut e = Self::default();
        e.from_string(s)?;
        Ok(e)
    }
}

impl FeedbackTypeExpression {
    /// Go `FromString`: parses `data` into `self`.
    ///
    /// Like Go, the feedback type and operator are set before the value is
    /// parsed, so a bad value leaves them changed and sets the value to what
    /// `strconv.ParseFloat` returned (0, or `+Inf` when out of range); text
    /// without a value keeps the previous value; and `=` is read as
    /// [`ExprType::None`].
    pub fn from_string(&mut self, data: &str) -> Result<(), Error> {
        let caps = EXPRESSION_PATTERN
            .captures(data)
            .ok_or(Error::InvalidFormat)?;
        let group = |name| caps.name(name).map_or("", |m| m.as_str());
        group("feedback_type").clone_into(&mut self.feedback_type);
        self.expr_type = match group("expr_type") {
            "<" => ExprType::Less,
            "<=" => ExprType::LessOrEqual,
            ">" => ExprType::Greater,
            ">=" => ExprType::GreaterOrEqual,
            _ => ExprType::None,
        };
        let value = group("value");
        if !value.is_empty() {
            let (parsed, err) = parse_float(value);
            self.value = parsed;
            if let Some(err) = err {
                return Err(Error::InvalidValue(err));
            }
        }
        Ok(())
    }

    /// Go `MarshalJSON`: the text form as a JSON string, escaped like Go's
    /// `json.Marshal`, so `<`, `>` and `&` come out as `\u003c`, `\u003e`
    /// and `\u0026`.
    pub fn marshal_json(&self) -> String {
        go_json_quote(&self.to_string())
    }

    /// Go `UnmarshalJSON`: decodes a JSON string into a Go string, then
    /// [`Self::from_string`]. JSON `null` decodes to the empty string, which
    /// then fails to parse; other non-strings fail with Go's type error.
    pub fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), Error> {
        self.unmarshal_json_value(data)
            .map_err(|err| Error::Unmarshal(Box::new(err)))
    }

    fn unmarshal_json_value(&mut self, data: &[u8]) -> Result<(), Error> {
        // `json.Unmarshal` checks the whole input before decoding it. serde_json
        // accepts the same documents here: it skips strings without decoding
        // their escapes or checking their UTF-8, as Go's scanner does.
        serde_json::from_slice::<serde::de::IgnoredAny>(data)
            .map_err(|err| Error::Syntax(err.to_string()))?;
        let is_space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
        let start = data.iter().position(|b| !is_space(b)).unwrap_or(0);
        let end = data.iter().rposition(|b| !is_space(b)).map_or(0, |i| i + 1);
        let token = &data[start..end];
        let text = match token.first() {
            Some(b'"') => go_json_unquote(token),
            // `null` leaves the Go string empty.
            Some(b'n') => String::new(),
            Some(b't' | b'f') => return Err(Error::NotAString("bool")),
            Some(b'[') => return Err(Error::NotAString("array")),
            Some(b'{') => return Err(Error::NotAString("object")),
            _ => return Err(Error::NotAString("number")),
        };
        self.from_string(&text)
    }

    /// Go `ToPB`.
    pub fn to_pb(&self) -> pb::FeedbackTypeExpression {
        let expression_type = match self.expr_type {
            ExprType::None => pb::ExpressionType::None,
            ExprType::Less => pb::ExpressionType::Less,
            ExprType::LessOrEqual => pb::ExpressionType::LessOrEqual,
            ExprType::Greater => pb::ExpressionType::Greater,
            ExprType::GreaterOrEqual => pb::ExpressionType::GreaterOrEqual,
        };
        pb::FeedbackTypeExpression {
            feedback_type: self.feedback_type.clone(),
            expression_type: expression_type.into(),
            value: self.value,
        }
    }

    /// Go `FromPB`. An expression type outside the enum leaves
    /// [`Self::expr_type`] as it was, as Go's `switch` does.
    pub fn from_pb(&mut self, pb: &pb::FeedbackTypeExpression) {
        pb.feedback_type.clone_into(&mut self.feedback_type);
        if let Ok(expression_type) = pb::ExpressionType::try_from(pb.expression_type) {
            self.expr_type = match expression_type {
                pb::ExpressionType::None => ExprType::None,
                pb::ExpressionType::Less => ExprType::Less,
                pb::ExpressionType::LessOrEqual => ExprType::LessOrEqual,
                pb::ExpressionType::Greater => ExprType::Greater,
                pb::ExpressionType::GreaterOrEqual => ExprType::GreaterOrEqual,
            };
        }
        self.value = pb.value;
    }

    /// Go `Match`: whether feedback of `feedback_type` with `value` matches.
    pub fn matches(&self, feedback_type: &str, value: f64) -> bool {
        if self.feedback_type != feedback_type {
            return false;
        }
        match self.expr_type {
            ExprType::None => true,
            ExprType::Less => value < self.value,
            ExprType::LessOrEqual => value <= self.value,
            ExprType::Greater => value > self.value,
            ExprType::GreaterOrEqual => value >= self.value,
        }
    }
}

/// Go `MatchFeedbackTypeExpressions`: whether any expression matches.
pub fn match_feedback_type_expressions(
    exprs: &[FeedbackTypeExpression],
    feedback_type: &str,
    value: f64,
) -> bool {
    exprs.iter().any(|e| e.matches(feedback_type, value))
}

/// Go `MustParseFeedbackTypeExpression`: parses `s`.
///
/// # Panics
///
/// If `s` does not parse, with the error text as the message, like Go's
/// `lo.Must0`.
pub fn must_parse_feedback_type_expression(s: &str) -> FeedbackTypeExpression {
    match s.parse() {
        Ok(e) => e,
        Err(err) => panic!("{err}"),
    }
}

impl serde::Serialize for FeedbackTypeExpression {
    /// The text form as a string, as Go's `MarshalJSON` writes it for every
    /// slice element. Go's encoder also escapes `<`, `>` and `&`; a writer
    /// that must match Go byte for byte applies that to every string (see
    /// [`FeedbackTypeExpression::marshal_json`]).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for FeedbackTypeExpression {
    /// Go `UnmarshalJSON` into a zero value: a string is parsed, `null` is
    /// parsed as the empty string, and other values fail with Go's message.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ExpressionVisitor)
    }
}

struct ExpressionVisitor;

impl ExpressionVisitor {
    fn parse<E: serde::de::Error>(text: &str) -> Result<FeedbackTypeExpression, E> {
        text.parse()
            .map_err(|err| E::custom(Error::Unmarshal(Box::new(err))))
    }

    fn not_a_string<E: serde::de::Error>(kind: &'static str) -> E {
        E::custom(Error::Unmarshal(Box::new(Error::NotAString(kind))))
    }
}

impl<'de> serde::de::Visitor<'de> for ExpressionVisitor {
    type Value = FeedbackTypeExpression;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a feedback type expression string")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Self::parse(v)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Self::parse("")
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Self::parse("")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Err(Self::not_a_string("bool"))
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Err(Self::not_a_string("number"))
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Err(Self::not_a_string("number"))
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Err(Self::not_a_string("number"))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
        Err(Self::not_a_string("array"))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
        Err(Self::not_a_string("object"))
    }
}

/// Go `strconv.ParseFloat(s, 64)` for the strings the pattern admits: ASCII
/// digits with at most one `.`, not empty. Returns the value Go returns
/// together with its error, if any.
fn parse_float(s: &str) -> (f64, Option<ParseFloatError>) {
    let error = |kind| ParseFloatError {
        num: s.to_owned(),
        kind,
    };
    match s.parse::<f64>() {
        // Rust rounds to infinity where Go reports ErrRange with +Inf.
        Ok(v) if v.is_infinite() => (v, Some(error(ParseFloatErrorKind::Range))),
        Ok(v) => (v, None),
        Err(_) => (0.0, Some(error(ParseFloatErrorKind::Syntax))),
    }
}

/// Go's `%v` of a `float64`: `strconv.FormatFloat(v, 'g', -1, 64)`, the
/// shortest digits that read back to `v`, in exponent form when the decimal
/// exponent is below -4 or at least 6 (`1e+06`, `1e-05`), plus `+Inf`, `-Inf`
/// and `NaN`.
fn format_float_v(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let sign = if v.is_sign_negative() { "-" } else { "" };
    if v == 0.0 {
        return format!("{sign}0");
    }
    // Rust's `{:e}` writes the shortest round-trip digits too, as
    // `d[.ddd]e<exp>`; only the layout differs from Go's.
    let sci = format!("{:e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` writes an exponent");
    let exp: i32 = exp.parse().expect("`{:e}` writes a decimal exponent");
    let digits: Vec<u8> = mantissa.bytes().filter(|&b| b != b'.').collect();
    let nd = i32::try_from(digits.len()).expect("at most 17 digits");
    let mut out = String::from(sign);
    // Go: `exp < -4 || exp >= eprec`, with `eprec` 6 for shortest output.
    if !(-4..6).contains(&exp) {
        out.push(char::from(digits[0]));
        if digits.len() > 1 {
            out.push('.');
            out.extend(digits[1..].iter().map(|&b| char::from(b)));
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let exp = exp.unsigned_abs();
        if exp < 10 {
            out.push('0');
        }
        out.push_str(&exp.to_string());
    } else {
        // Digit `i` of the number, counting from the first significant one
        // at position `dp - 1` before the point.
        let dp = exp + 1;
        let digit = |i: i32| {
            usize::try_from(i)
                .ok()
                .and_then(|i| digits.get(i))
                .map_or('0', |&b| char::from(b))
        };
        if dp > 0 {
            out.extend((0..dp).map(digit));
        } else {
            out.push('0');
        }
        let decimals = (nd - dp).max(0);
        if decimals > 0 {
            out.push('.');
            out.extend((dp..dp + decimals).map(digit));
        }
    }
    out
}

/// Go's `json.Marshal` of a string: escapes `"`, `\`, control characters,
/// `<`, `>`, `&`, U+2028 and U+2029, with lower-case hex.
fn go_json_quote(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                let code = u32::from(c);
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    out.push(char::from(HEX[((code >> shift) & 0xf) as usize]));
                }
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Go's `unquoteBytes` for a JSON string token already checked to be valid:
/// decodes the escapes, turns a `\u` surrogate that is not part of a pair and
/// each byte of invalid UTF-8 into U+FFFD.
fn go_json_unquote(token: &[u8]) -> String {
    let s = &token[1..token.len() - 1];
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    // The four hex digits of a `\uXXXX` escape starting at `at`.
    let u4 = |at: usize| -> Option<u32> {
        if s.get(at..at + 2) != Some(b"\\u".as_slice()) {
            return None;
        }
        let hex = std::str::from_utf8(s.get(at + 2..at + 6)?).ok()?;
        u32::from_str_radix(hex, 16).ok()
    };
    while i < s.len() {
        match s[i] {
            b'\\' => {
                let escaped = s[i + 1];
                if escaped == b'u' {
                    let first = u4(i).expect("a checked \\u escape");
                    i += 6;
                    let c = if (0xd800..0xe000).contains(&first) {
                        match u4(i) {
                            Some(second)
                                if (0xd800..0xdc00).contains(&first)
                                    && (0xdc00..0xe000).contains(&second) =>
                            {
                                i += 6;
                                char::from_u32(
                                    0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00),
                                )
                            }
                            _ => None,
                        }
                    } else {
                        char::from_u32(first)
                    };
                    out.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
                } else {
                    out.push(match escaped {
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        other => char::from(other),
                    });
                    i += 2;
                }
            }
            b if b.is_ascii() => {
                out.push(char::from(b));
                i += 1;
            }
            b => {
                // Go's `utf8.DecodeRune`: a whole valid sequence, or U+FFFD
                // for one byte.
                let width = match b {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                match s.get(i..i + width).map(std::str::from_utf8) {
                    Some(Ok(valid)) if width > 1 => {
                        out.push_str(valid);
                        i += width;
                    }
                    _ => {
                        out.push(char::REPLACEMENT_CHARACTER);
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    // Ports: common/expression/expression_test.go @ add21b5
    use super::*;

    #[test]
    fn test_feedback_type_expression_unmarshal_json() {
        let mut f = FeedbackTypeExpression::default();
        let err = f.unmarshal_json(br#""test""#);
        assert!(err.is_ok());
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::None, f.expr_type);

        let err = f.unmarshal_json(br#""1a""#);
        assert!(err.is_err());

        let err = f.unmarshal_json(br#""test<16""#);
        assert!(err.is_ok());
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::Less, f.expr_type);
        assert_eq!(16.0, f.value);

        let err = f.unmarshal_json(br#""test<=16""#);
        assert!(err.is_ok());
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::LessOrEqual, f.expr_type);
        assert_eq!(16.0, f.value);

        let err = f.unmarshal_json(br#""test>16""#);
        assert!(err.is_ok());
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::Greater, f.expr_type);
        assert_eq!(16.0, f.value);

        let err = f.unmarshal_json(br#""test>=16""#);
        assert!(err.is_ok());
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::GreaterOrEqual, f.expr_type);
        assert_eq!(16.0, f.value);
    }

    #[test]
    fn test_feedback_type_expression_marshal_json() {
        let mut f = FeedbackTypeExpression {
            feedback_type: "test".to_owned(),
            value: 16.0,
            ..Default::default()
        };
        let buf = f.marshal_json();
        assert_eq!(r#""test""#, buf);

        f.expr_type = ExprType::Less;
        let buf = f.marshal_json();
        assert_eq!(r#""test\u003c16""#, buf);

        f.expr_type = ExprType::LessOrEqual;
        let buf = f.marshal_json();
        assert_eq!(r#""test\u003c=16""#, buf);

        f.expr_type = ExprType::Greater;
        let buf = f.marshal_json();
        assert_eq!(r#""test\u003e16""#, buf);

        f.expr_type = ExprType::GreaterOrEqual;
        let buf = f.marshal_json();
        assert_eq!(r#""test\u003e=16""#, buf);
    }

    #[test]
    fn test_feedback_type_expression_to_pb() {
        let mut f = FeedbackTypeExpression {
            feedback_type: "test".to_owned(),
            value: 6.0,
            ..Default::default()
        };
        let pb = f.to_pb();
        assert_eq!("test", pb.feedback_type);
        assert_eq!(pb::ExpressionType::None, pb.expression_type());
        assert_eq!(6.0, pb.value);

        f.expr_type = ExprType::Less;
        let pb = f.to_pb();
        assert_eq!(pb::ExpressionType::Less, pb.expression_type());

        f.expr_type = ExprType::LessOrEqual;
        let pb = f.to_pb();
        assert_eq!(pb::ExpressionType::LessOrEqual, pb.expression_type());

        f.expr_type = ExprType::Greater;
        let pb = f.to_pb();
        assert_eq!(pb::ExpressionType::Greater, pb.expression_type());

        f.expr_type = ExprType::GreaterOrEqual;
        let pb = f.to_pb();
        assert_eq!(pb::ExpressionType::GreaterOrEqual, pb.expression_type());
    }

    #[test]
    fn test_feedback_type_expression_from_pb() {
        let mut pb = pb::FeedbackTypeExpression {
            feedback_type: "test".to_owned(),
            expression_type: pb::ExpressionType::Less.into(),
            value: 6.0,
        };
        let mut f = FeedbackTypeExpression::default();
        f.from_pb(&pb);
        assert_eq!("test", f.feedback_type);
        assert_eq!(ExprType::Less, f.expr_type);
        assert_eq!(6.0, f.value);

        pb.set_expression_type(pb::ExpressionType::LessOrEqual);
        f.from_pb(&pb);
        assert_eq!(ExprType::LessOrEqual, f.expr_type);

        pb.set_expression_type(pb::ExpressionType::Greater);
        f.from_pb(&pb);
        assert_eq!(ExprType::Greater, f.expr_type);

        pb.set_expression_type(pb::ExpressionType::GreaterOrEqual);
        f.from_pb(&pb);
        assert_eq!(ExprType::GreaterOrEqual, f.expr_type);

        pb.set_expression_type(pb::ExpressionType::None);
        f.from_pb(&pb);
        assert_eq!(ExprType::None, f.expr_type);
    }

    #[test]
    fn test_feedback_type_expression_match() {
        let mut f = FeedbackTypeExpression {
            feedback_type: "test".to_owned(),
            value: 6.0,
            ..Default::default()
        };
        assert!(f.matches("test", 0.0));
        assert!(!f.matches("a", 1.0));

        f.expr_type = ExprType::Less;
        assert!(f.matches("test", 5.0));
        assert!(!f.matches("test", 6.0));

        f.expr_type = ExprType::LessOrEqual;
        assert!(f.matches("test", 6.0));
        assert!(!f.matches("test", 7.0));

        f.expr_type = ExprType::Greater;
        assert!(f.matches("test", 7.0));
        assert!(!f.matches("test", 6.0));

        f.expr_type = ExprType::GreaterOrEqual;
        assert!(f.matches("test", 6.0));
        assert!(!f.matches("test", 5.0));
    }

    #[test]
    fn test_match_feedback_type_expressions() {
        let expressions = [
            FeedbackTypeExpression {
                feedback_type: "a".to_owned(),
                ..Default::default()
            },
            FeedbackTypeExpression {
                feedback_type: "b".to_owned(),
                ..Default::default()
            },
            FeedbackTypeExpression {
                feedback_type: "c".to_owned(),
                ..Default::default()
            },
        ];
        assert!(match_feedback_type_expressions(&expressions, "a", 0.0));
        assert!(!match_feedback_type_expressions(&expressions, "d", 0.0));
    }

    #[test]
    #[should_panic]
    fn test_must_parse_feedback_type_expression() {
        must_parse_feedback_type_expression("test+");
    }
}
