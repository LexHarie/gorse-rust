// Ports: encoding/json's Decoder at Go 1.27.0, its jsontext and internal/jsonwire
// packages, as go-restful v3.13.0's ReadEntity runs it.

//! The JSON scanner of Go's `encoding/json` decoder, for request bodies.
//!
//! [`scan`] reads the first JSON value of a body and stops after it, as the
//! decoder does. When the value is malformed it returns the text the decoder
//! gives: `unexpected EOF` when the body ends inside the value, an
//! `invalid character` text for a byte that cannot come next, and
//! `exceeded max depth` for nesting past 10000. The tree is built iteratively
//! and freed one level at a time, so a body nested to the limit cannot exhaust
//! the stack.

use std::ops::Range;

use super::printable::is_print;

/// Go's `maxNestingDepth`: how many arrays and objects a value may nest.
const MAX_DEPTH: usize = 10_000;

/// The message for a body that ends inside a value.
const UNEXPECTED_EOF: &str = "unexpected EOF";

/// The place in a number where a digit was expected.
const IN_NUMERIC_LITERAL: &str = "in numeric literal";

/// A JSON value, with the bytes it takes up in the body.
#[derive(Debug)]
pub(crate) struct Node {
    /// The value's bytes: from its first byte up to, not including, the byte
    /// after its last.
    pub(crate) span: Range<usize>,
    /// What the value is.
    pub(crate) kind: Kind,
}

/// The kinds of JSON value.
#[derive(Debug)]
pub(crate) enum Kind {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number. Its text is the value's bytes.
    Number,
    /// A string, with its escapes decoded.
    String(String),
    /// An array.
    Array(Vec<Node>),
    /// An object, with its members in the order the body gives them.
    Object(Vec<Member>),
}

/// A member of an object.
#[derive(Debug)]
pub(crate) struct Member {
    /// The member's name, with its escapes decoded.
    pub(crate) name: String,
    /// The member's value.
    pub(crate) value: Node,
}

impl Drop for Node {
    /// Frees the values nested in this one a level at a time, so that dropping
    /// a value nested to [`MAX_DEPTH`] does not recurse that deep.
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.take_children(&mut pending);
        while let Some(mut node) = pending.pop() {
            node.take_children(&mut pending);
        }
    }
}

impl Node {
    /// Moves the values nested directly in this one into `pending`.
    fn take_children(&mut self, pending: &mut Vec<Node>) {
        match &mut self.kind {
            Kind::Array(items) => pending.append(items),
            Kind::Object(members) => {
                pending.extend(members.drain(..).map(|member| member.value));
            }
            Kind::Null | Kind::Bool(_) | Kind::Number | Kind::String(_) => {}
        }
    }
}

/// Whether a body holds nothing but the white space JSON allows. Go's decoder
/// reports such a body as `EOF`, before it reads a value.
pub(crate) fn is_blank(body: &[u8]) -> bool {
    body.iter().all(is_space)
}

/// The white space JSON allows: space, tab, carriage return and line feed.
fn is_space(byte: &u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Reads the first JSON value of `body`. The bytes after it are not read, as
/// the decoder does not read them.
///
/// # Errors
///
/// Fails with the text Go's decoder gives for a body that does not begin with
/// a complete, valid value. A body of white space alone is not scanned: see
/// [`is_blank`].
pub(crate) fn scan(body: &[u8]) -> Result<Node, String> {
    let mut cursor = Cursor { body, pos: 0 };
    let mut frames: Vec<Frame> = Vec::new();
    let mut expect = Expect::Value;
    loop {
        cursor.skip_space();
        let start = cursor.pos;
        let byte = cursor.peek()?;
        let node = match (expect, byte) {
            (Expect::Value | Expect::ValueOrClose, b'{') => {
                cursor.pos += 1;
                open(
                    &mut frames,
                    Frame::Object {
                        start,
                        members: Vec::new(),
                        name: None,
                    },
                )?;
                expect = Expect::KeyOrClose;
                continue;
            }
            (Expect::Value | Expect::ValueOrClose, b'[') => {
                cursor.pos += 1;
                open(
                    &mut frames,
                    Frame::Array {
                        start,
                        items: Vec::new(),
                    },
                )?;
                expect = Expect::ValueOrClose;
                continue;
            }
            (Expect::ValueOrClose | Expect::After, b']') if is_array(&frames) => {
                cursor.pos += 1;
                close(&mut frames, cursor.pos)
            }
            (Expect::KeyOrClose | Expect::After, b'}') if is_object(&frames) => {
                cursor.pos += 1;
                close(&mut frames, cursor.pos)
            }
            (Expect::Value | Expect::ValueOrClose, b'"') => {
                let text = cursor.string()?;
                Node {
                    span: start..cursor.pos,
                    kind: Kind::String(text),
                }
            }
            (Expect::Value | Expect::ValueOrClose, b't' | b'f' | b'n') => {
                let (word, kind) = match byte {
                    b't' => ("true", Kind::Bool(true)),
                    b'f' => ("false", Kind::Bool(false)),
                    _ => ("null", Kind::Null),
                };
                cursor.literal(word)?;
                Node {
                    span: start..cursor.pos,
                    kind,
                }
            }
            (Expect::Value | Expect::ValueOrClose, b'-' | b'0'..=b'9') => {
                cursor.number()?;
                Node {
                    span: start..cursor.pos,
                    kind: Kind::Number,
                }
            }
            (Expect::KeyOrClose | Expect::Key, b'"') => {
                let name = cursor.string()?;
                if let Some(Frame::Object { name: pending, .. }) = frames.last_mut() {
                    *pending = Some(name);
                }
                expect = Expect::Colon;
                continue;
            }
            (Expect::Colon, b':') => {
                cursor.pos += 1;
                expect = Expect::Value;
                continue;
            }
            (Expect::After, b',') => {
                cursor.pos += 1;
                expect = if is_array(&frames) {
                    Expect::Value
                } else {
                    Expect::Key
                };
                continue;
            }
            (Expect::Value | Expect::ValueOrClose, _) => {
                return Err(cursor.invalid("looking for beginning of value"));
            }
            (Expect::KeyOrClose | Expect::Key, _) => {
                return Err(cursor.invalid("looking for beginning of object key string"));
            }
            (Expect::Colon, _) => return Err(cursor.invalid("after object key")),
            (Expect::After, _) => {
                let place = if is_array(&frames) {
                    "after array element"
                } else {
                    "after object key:value pair"
                };
                return Err(cursor.invalid(place));
            }
        };
        match attach(&mut frames, node) {
            Some(root) => return Ok(root),
            None => expect = Expect::After,
        }
    }
}

/// What the scanner reads next.
#[derive(Clone, Copy)]
enum Expect {
    /// A value.
    Value,
    /// A value, or the `]` that ends an empty array.
    ValueOrClose,
    /// A member name, or the `}` that ends an empty object.
    KeyOrClose,
    /// A member name, after a comma.
    Key,
    /// The `:` after a member name.
    Colon,
    /// The comma or closing bracket that follows a value.
    After,
}

/// An array or object that is still open.
enum Frame {
    /// An array, with the values read so far.
    Array { start: usize, items: Vec<Node> },
    /// An object, with its members read so far and the name of the member
    /// whose value is being read.
    Object {
        start: usize,
        members: Vec<Member>,
        name: Option<String>,
    },
}

/// Whether the innermost open container is an array.
fn is_array(frames: &[Frame]) -> bool {
    matches!(frames.last(), Some(Frame::Array { .. }))
}

/// Whether the innermost open container is an object.
fn is_object(frames: &[Frame]) -> bool {
    matches!(frames.last(), Some(Frame::Object { .. }))
}

/// Opens a container, unless that would nest past [`MAX_DEPTH`].
fn open(frames: &mut Vec<Frame>, frame: Frame) -> Result<(), String> {
    if frames.len() >= MAX_DEPTH {
        return Err("exceeded max depth".to_owned());
    }
    frames.push(frame);
    Ok(())
}

/// Closes the innermost container, which ends at `end`, and returns it as a
/// value. The callers check that a container is open.
fn close(frames: &mut Vec<Frame>, end: usize) -> Node {
    match frames.pop() {
        Some(Frame::Array { start, items }) => Node {
            span: start..end,
            kind: Kind::Array(items),
        },
        Some(Frame::Object { start, members, .. }) => Node {
            span: start..end,
            kind: Kind::Object(members),
        },
        None => Node {
            span: end..end,
            kind: Kind::Null,
        },
    }
}

/// Adds a complete value to the innermost open container. With no container
/// open, the value is the body's first value, and it is handed back.
fn attach(frames: &mut [Frame], node: Node) -> Option<Node> {
    match frames.last_mut() {
        None => Some(node),
        Some(Frame::Array { items, .. }) => {
            items.push(node);
            None
        }
        Some(Frame::Object { members, name, .. }) => {
            members.push(Member {
                name: name.take().unwrap_or_default(),
                value: node,
            });
            None
        }
    }
}

/// The read position in a body.
struct Cursor<'a> {
    /// The body.
    body: &'a [u8],
    /// The position of the next byte to read.
    pos: usize,
}

impl Cursor<'_> {
    /// Skips the white space at the read position.
    fn skip_space(&mut self) {
        while self.body.get(self.pos).is_some_and(is_space) {
            self.pos += 1;
        }
    }

    /// The byte at the read position. A body that ends where a byte is
    /// needed is `unexpected EOF`.
    fn peek(&self) -> Result<u8, String> {
        self.body
            .get(self.pos)
            .copied()
            .ok_or_else(|| UNEXPECTED_EOF.to_owned())
    }

    /// The message for the byte at the read position, which is not one the
    /// scanner can use here. `place` says what the scanner was reading.
    fn invalid(&self, place: &str) -> String {
        invalid_character(&self.body[self.pos..], place)
    }

    /// Reads the string whose opening quote is at the read position, and
    /// returns its text with the escapes decoded.
    fn string(&mut self) -> Result<String, String> {
        let body = self.body;
        let mut text = String::new();
        self.pos += 1;
        loop {
            let byte = self.peek()?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(text);
                }
                b'\\' => self.escape(&mut text)?,
                0x00..=0x1f => {
                    return Err(invalid_character(&body[self.pos..], "in string"));
                }
                0x20..=0x7f => {
                    text.push(char::from(byte));
                    self.pos += 1;
                }
                0x80..=0xff => self.non_ascii(&mut text)?,
            }
        }
    }

    /// Reads one character that is not ASCII. A byte that does not begin a
    /// valid character reads as U+FFFD, as the decoder reads it, unless the
    /// body ends inside the character.
    fn non_ascii(&mut self, text: &mut String) -> Result<(), String> {
        let rest = &self.body[self.pos..];
        if let Some(c) = decode_char(rest) {
            text.push(c);
            self.pos += c.len_utf8();
        } else if is_incomplete(rest) {
            return Err(UNEXPECTED_EOF.to_owned());
        } else {
            text.push(char::REPLACEMENT_CHARACTER);
            self.pos += 1;
        }
        Ok(())
    }

    /// Reads the escape sequence at the read position, which is a backslash.
    fn escape(&mut self, text: &mut String) -> Result<(), String> {
        let body = self.body;
        let rest = &body[self.pos..];
        let Some(&letter) = rest.get(1) else {
            return Err(UNEXPECTED_EOF.to_owned());
        };
        let simple = match letter {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(text),
            _ => return Err(invalid_escape(&rest[..2])),
        };
        text.push(simple);
        self.pos += 2;
        Ok(())
    }

    /// Reads the `\uXXXX` escape at the read position. A high surrogate that
    /// the next escape completes makes one character with it; any other
    /// surrogate is U+FFFD.
    fn unicode_escape(&mut self, text: &mut String) -> Result<(), String> {
        let rest = &self.body[self.pos..];
        if rest.len() < 6 {
            return Err(if has_utf16_prefix(rest) {
                UNEXPECTED_EOF.to_owned()
            } else {
                invalid_escape(rest)
            });
        }
        let unit = hex4(&rest[2..6]).ok_or_else(|| invalid_escape(&rest[..6]))?;
        self.pos += 6;
        if (0xD800..0xDC00).contains(&unit)
            && let Some(low) = self.low_surrogate()
        {
            let c = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            text.push(char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER));
            self.pos += 6;
            return Ok(());
        }
        text.push(char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER));
        Ok(())
    }

    /// The value of the `\uXXXX` escape at the read position, when that
    /// escape is a low surrogate.
    fn low_surrogate(&self) -> Option<u32> {
        let rest = self.body.get(self.pos..)?;
        if rest.len() < 6 || rest[0] != b'\\' || rest[1] != b'u' {
            return None;
        }
        let unit = hex4(&rest[2..6])?;
        (0xDC00..0xE000).contains(&unit).then_some(unit)
    }

    /// Reads the literal `word` (`true`, `false` or `null`) at the read
    /// position.
    fn literal(&mut self, word: &str) -> Result<(), String> {
        for (index, want) in word.bytes().enumerate() {
            match self.body.get(self.pos + index) {
                None => return Err(UNEXPECTED_EOF.to_owned()),
                Some(&got) if got == want => {}
                Some(_) => {
                    let place = format!(
                        "in literal {word} (expecting {})",
                        quote_rune(char::from(want))
                    );
                    return Err(invalid_character(&self.body[self.pos + index..], &place));
                }
            }
        }
        self.pos += word.len();
        Ok(())
    }

    /// Reads the number at the read position, which is a minus sign or a digit.
    fn number(&mut self) -> Result<(), String> {
        let mut at = self.pos + usize::from(self.body[self.pos] == b'-');
        if self.body.get(at) == Some(&b'0') {
            at += 1;
        } else {
            at = self.digits(at)?;
        }
        if self.body.get(at) == Some(&b'.') {
            at = self.digits(at + 1)?;
        }
        if matches!(self.body.get(at), Some(b'e' | b'E')) {
            at += 1;
            if matches!(self.body.get(at), Some(b'+' | b'-')) {
                at += 1;
            }
            at = self.digits(at)?;
        }
        self.pos = at;
        Ok(())
    }

    /// Reads one or more digits from `at`, and returns the position after
    /// them.
    fn digits(&self, at: usize) -> Result<usize, String> {
        match self.body.get(at) {
            None => Err(UNEXPECTED_EOF.to_owned()),
            Some(b'0'..=b'9') => {
                let end = self.body[at..]
                    .iter()
                    .position(|byte| !byte.is_ascii_digit())
                    .map_or(self.body.len(), |offset| at + offset);
                Ok(end)
            }
            Some(_) => Err(invalid_character(&self.body[at..], IN_NUMERIC_LITERAL)),
        }
    }
}

/// The message for the character that begins `bytes`, which the scanner
/// cannot use where it is. `place` says what the scanner was reading.
fn invalid_character(bytes: &[u8], place: &str) -> String {
    format!("invalid character {} {place}", quote_char(bytes))
}

/// The message for an escape sequence that is not one JSON allows.
fn invalid_escape(what: &[u8]) -> String {
    format!(
        "invalid escape sequence `{}` in string",
        String::from_utf8_lossy(what)
    )
}

/// The character that begins `bytes`, quoted as Go quotes it. A byte that
/// does not begin a valid character quotes as its hex escape.
fn quote_char(bytes: &[u8]) -> String {
    match decode_char(bytes) {
        Some(c) => quote_rune(c),
        None => format!("'\\x{:02x}'", bytes.first().copied().unwrap_or_default()),
    }
}

/// `strconv.QuoteRune`: the character in single quotes, with the escapes Go
/// uses for it.
fn quote_rune(c: char) -> String {
    let body = match c {
        '\u{7}' => "\\a".to_owned(),
        '\u{8}' => "\\b".to_owned(),
        '\u{c}' => "\\f".to_owned(),
        '\n' => "\\n".to_owned(),
        '\r' => "\\r".to_owned(),
        '\t' => "\\t".to_owned(),
        '\u{b}' => "\\v".to_owned(),
        '\'' => "\\'".to_owned(),
        '\\' => "\\\\".to_owned(),
        c if c < ' ' || c == '\u{7f}' => format!("\\x{:02x}", u32::from(c)),
        c if c.is_ascii() || is_print(c) => c.to_string(),
        c if u32::from(c) < 0x1_0000 => format!("\\u{:04x}", u32::from(c)),
        c => format!("\\U{:08x}", u32::from(c)),
    };
    format!("'{body}'")
}

/// The character that begins `bytes`, or `None` when the bytes do not begin
/// with a valid one.
fn decode_char(bytes: &[u8]) -> Option<char> {
    let window = &bytes[..bytes.len().min(4)];
    let valid = match std::str::from_utf8(window) {
        Ok(_) => window,
        Err(err) => &window[..err.valid_up_to()],
    };
    std::str::from_utf8(valid).ok()?.chars().next()
}

/// Whether `bytes` begin with a valid character that the body has not
/// finished, because it ends first. Such a character is `unexpected EOF`.
fn is_incomplete(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(4)];
    matches!(std::str::from_utf8(window), Err(err) if err.error_len().is_none())
}

/// Whether `bytes`, shorter than a `\u` escape, begin one: a backslash, a `u`
/// and hex digits.
fn has_utf16_prefix(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(index, byte)| match index {
        0 => *byte == b'\\',
        1 => *byte == b'u',
        _ => byte.is_ascii_hexdigit(),
    })
}

/// The value of four hex digits.
fn hex4(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0u32, |value, &digit| {
        Some(value * 16 + char::from(digit).to_digit(16)?)
    })
}
