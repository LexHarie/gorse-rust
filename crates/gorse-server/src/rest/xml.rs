// Ports: $GOROOT/src/encoding/xml/xml.go (Decoder.Token, rawToken, text, attrval, name, nsname, readName, space, getc, mustgetc, popElement, procInst, isInCharacterRange, the entity table, Skip) and read.go (Decode, unmarshal for a struct, a slice and a string, bool, float64 or time.Time, unmarshalTextInterface, unmarshalPath, copyValue) @ go1.27.0, as go-restful v3.13.0's ReadEntity runs them: a strict Decoder with no CharsetReader and no entity map.

//! Decodes an XML request body into a Go type as `encoding/xml` does. The
//! decoder reads the body lazily, one token at a time, so a body whose root
//! element closes is decoded even when more bytes follow it, as Go does. Names
//! are not namespace-translated: no field of a Go type takes a namespace, so
//! translation can only change a name's space, which the fields never compare.

use gorse_common::gostrconv::{parse_bool, parse_float, quote};
use gorse_common::gotime::Time;

use super::entity::{Assign, FieldKind, GoStruct, rfc3339};
use super::xmlname::{is_name_char, is_name_start};

/// A decoder result. `Err` means the decoder has recorded its error.
type Res<T> = Result<T, Stop>;

/// The marker of a failed read. The error itself is in [`Decoder::err`].
#[derive(Debug)]
struct Stop;

/// What the decoder stopped with: `io.EOF`, or an error's text.
enum Fault {
    /// `io.EOF`: the bytes ran out.
    Eof,
    /// An error's text.
    Text(String),
}

/// A name as `encoding/xml` splits it: a prefix (`space`) and a local part.
#[derive(Clone, Default)]
struct Name {
    /// The prefix, which is empty when there is none.
    space: String,
    /// The local part.
    local: String,
}

/// A token. Only the elements and the character data change what a Go type
/// takes; comments, processing instructions and directives are `Other`.
enum Token {
    /// A start element, with the name it opens.
    Start(Name),
    /// An end element, with the name it closes.
    End(Name),
    /// Character data, with its entities expanded.
    Text(Vec<u8>),
    /// A comment, a processing instruction or a directive.
    Other,
}

/// The decoder: `encoding/xml`'s strict `Decoder` over the bytes of a body.
struct Decoder<'a> {
    /// The bytes of the body.
    input: &'a [u8],
    /// The index of the next byte of `input` to read.
    pos: usize,
    /// The reader's error after `input`, when a decompressor failed. `None`
    /// means the body ends with `io.EOF`.
    failure: Option<&'a str>,
    /// The line the decoder is on, which the syntax errors name.
    line: usize,
    /// The byte that the last `ungetc` put back.
    next_byte: Option<u8>,
    /// The error the decoder stopped with. Every read fails once it is set.
    err: Option<Fault>,
    /// The bytes of the name, the text or the entity being read.
    buf: Vec<u8>,
    /// The open elements, innermost last.
    open: Vec<Name>,
    /// Whether the last start tag was empty (`<a/>`), so its end is due.
    need_close: bool,
    /// The name of the element that `need_close` ends.
    to_close: Name,
}

impl<'a> Decoder<'a> {
    /// A decoder at the start of `input`.
    fn new(input: &'a [u8], failure: Option<&'a str>) -> Self {
        Decoder {
            input,
            pos: 0,
            failure,
            line: 1,
            next_byte: None,
            err: None,
            buf: Vec::new(),
            open: Vec::new(),
            need_close: false,
            to_close: Name::default(),
        }
    }

    /// The text of the error the decoder stopped with.
    fn error_text(&self) -> String {
        match &self.err {
            Some(Fault::Text(text)) => text.clone(),
            Some(Fault::Eof) | None => "EOF".to_owned(),
        }
    }

    /// Records `text` as the decoder's error.
    fn fail(&mut self, text: String) -> Stop {
        self.err = Some(Fault::Text(text));
        Stop
    }

    /// Records a syntax error at the current line.
    fn syntax(&mut self, message: &str) -> Stop {
        let text = format!("XML syntax error on line {}: {message}", self.line);
        self.fail(text)
    }

    /// Records a syntax error unless an error is already recorded, as
    /// `if d.err == nil` does.
    fn syntax_unless_failed(&mut self, message: &str) -> Stop {
        if self.err.is_some() {
            Stop
        } else {
            self.syntax(message)
        }
    }

    /// Reads a byte. `None` once the decoder has an error, and at the end of the
    /// input, which records `io.EOF` or the reader's failure.
    fn getc(&mut self) -> Option<u8> {
        if self.err.is_some() {
            return None;
        }
        let b = match self.next_byte.take() {
            Some(b) => b,
            None => match self.input.get(self.pos) {
                Some(&b) => {
                    self.pos += 1;
                    b
                }
                None => {
                    self.err = Some(match self.failure {
                        Some(text) => Fault::Text(text.to_owned()),
                        None => Fault::Eof,
                    });
                    return None;
                }
            },
        };
        if b == b'\n' {
            self.line += 1;
        }
        Some(b)
    }

    /// Puts a byte back, for the next `getc` to read.
    fn ungetc(&mut self, b: u8) {
        if b == b'\n' {
            self.line -= 1;
        }
        self.next_byte = Some(b);
    }

    /// Reads a byte that must be there: at the end of the input it is the
    /// syntax error `unexpected EOF`.
    fn mustgetc(&mut self) -> Res<u8> {
        match self.getc() {
            Some(b) => Ok(b),
            None => {
                if matches!(self.err, Some(Fault::Eof)) {
                    return Err(self.syntax("unexpected EOF"));
                }
                Err(Stop)
            }
        }
    }

    /// Skips the white space, which is only space, tab and the line breaks.
    fn space(&mut self) {
        while let Some(b) = self.getc() {
            if !matches!(b, b' ' | b'\r' | b'\n' | b'\t') {
                self.ungetc(b);
                return;
            }
        }
    }

    /// Reads the bytes of a name into `buf`. `false` means there is no name,
    /// and that is not an error.
    fn read_name(&mut self) -> Res<bool> {
        let b = self.mustgetc()?;
        if b < 0x80 && !is_name_byte(b) {
            self.ungetc(b);
            return Ok(false);
        }
        self.buf.push(b);
        loop {
            let b = self.mustgetc()?;
            if b < 0x80 && !is_name_byte(b) {
                self.ungetc(b);
                return Ok(true);
            }
            self.buf.push(b);
        }
    }

    /// Reads a name. `None` means there is no name, and that is not an error.
    fn name(&mut self) -> Res<Option<String>> {
        self.buf.clear();
        if !self.read_name()? {
            return Ok(None);
        }
        if !is_name(&self.buf) {
            let text = format!("invalid XML name: {}", String::from_utf8_lossy(&self.buf));
            return Err(self.syntax(&text));
        }
        Ok(Some(String::from_utf8_lossy(&self.buf).into_owned()))
    }

    /// Reads a name that may have a prefix, as `nsname` does. A name with more
    /// than one colon is no name, and that is not an error.
    fn nsname(&mut self) -> Res<Option<Name>> {
        let Some(text) = self.name()? else {
            return Ok(None);
        };
        if text.matches(':').count() > 1 {
            return Ok(None);
        }
        let split = text
            .split_once(':')
            .filter(|(space, local)| !space.is_empty() && !local.is_empty())
            .map(|(space, local)| (space.to_owned(), local.to_owned()));
        Ok(Some(match split {
            Some((space, local)) => Name { space, local },
            None => Name {
                space: String::new(),
                local: text,
            },
        }))
    }

    /// Reads the token that starts with `<`, or the character data that starts
    /// with another byte, as `rawToken` does. It does not match end elements.
    fn raw_token(&mut self) -> Res<Token> {
        if self.err.is_some() {
            return Err(Stop);
        }
        if self.need_close {
            self.need_close = false;
            return Ok(Token::End(self.to_close.clone()));
        }
        let Some(b) = self.getc() else {
            return Err(Stop);
        };
        if b != b'<' {
            self.ungetc(b);
            return Ok(Token::Text(self.text(None, false)?));
        }
        match self.mustgetc()? {
            b'/' => self.end_tag(),
            b'?' => self.processing_instruction(),
            b'!' => self.bang(),
            b => {
                self.ungetc(b);
                self.start_tag()
            }
        }
    }

    /// Reads an end tag after `</`.
    fn end_tag(&mut self) -> Res<Token> {
        let Some(name) = self.nsname()? else {
            return Err(self.syntax_unless_failed("expected element name after </"));
        };
        self.space();
        if self.mustgetc()? != b'>' {
            let text = format!("invalid characters between </{} and >", name.local);
            return Err(self.syntax(&text));
        }
        Ok(Token::End(name))
    }

    /// Reads a start tag after `<`, with its attributes, which are checked and
    /// not kept. An empty element's end is due next.
    fn start_tag(&mut self) -> Res<Token> {
        let Some(name) = self.nsname()? else {
            return Err(self.syntax_unless_failed("expected element name after <"));
        };
        loop {
            self.space();
            let b = self.mustgetc()?;
            if b == b'/' {
                if self.mustgetc()? != b'>' {
                    return Err(self.syntax("expected /> in element"));
                }
                self.need_close = true;
                self.to_close = name.clone();
                break;
            }
            if b == b'>' {
                break;
            }
            self.ungetc(b);
            if self.nsname()?.is_none() {
                return Err(self.syntax_unless_failed("expected attribute name in element"));
            }
            self.space();
            if self.mustgetc()? != b'=' {
                return Err(self.syntax("attribute name without = in element"));
            }
            self.space();
            self.attrval()?;
        }
        Ok(Token::Start(name))
    }

    /// Reads an attribute value, which strict mode takes only quoted.
    fn attrval(&mut self) -> Res<Vec<u8>> {
        let b = self.mustgetc()?;
        if b == b'"' || b == b'\'' {
            return self.text(Some(b), false);
        }
        Err(self.syntax("unquoted or missing attribute value in element"))
    }

    /// Reads a processing instruction after `<?`. An XML declaration's version
    /// and encoding are checked.
    fn processing_instruction(&mut self) -> Res<Token> {
        let Some(target) = self.name()? else {
            return Err(self.syntax_unless_failed("expected target name after <?"));
        };
        self.space();
        let mut data = Vec::new();
        let mut b0 = 0u8;
        loop {
            let b = self.mustgetc()?;
            data.push(b);
            if b0 == b'?' && b == b'>' {
                break;
            }
            b0 = b;
        }
        data.truncate(data.len() - 2);
        if target == "xml" {
            let version = proc_inst(b"version", &data);
            if !version.is_empty() && version != b"1.0" {
                let text = format!(
                    "xml: unsupported version {}; only version 1.0 is supported",
                    quote(version)
                );
                return Err(self.fail(text));
            }
            let encoding = proc_inst(b"encoding", &data);
            if !encoding.is_empty() && !encoding.eq_ignore_ascii_case(b"utf-8") {
                let text = format!(
                    "xml: encoding {} declared but Decoder.CharsetReader is nil",
                    quote(encoding)
                );
                return Err(self.fail(text));
            }
        }
        Ok(Token::Other)
    }

    /// Reads what follows `<!`: a comment, a CDATA section or a directive.
    fn bang(&mut self) -> Res<Token> {
        match self.mustgetc()? {
            b'-' => self.comment(),
            b'[' => self.cdata(),
            _ => self.directive(),
        }
    }

    /// Reads a comment after `<!-`, which must not contain `--` before its `>`.
    fn comment(&mut self) -> Res<Token> {
        if self.mustgetc()? != b'-' {
            return Err(self.syntax("invalid sequence <!- not part of <!--"));
        }
        let (mut b0, mut b1) = (0u8, 0u8);
        loop {
            let b = self.mustgetc()?;
            if b0 == b'-' && b1 == b'-' {
                if b != b'>' {
                    return Err(self.syntax(r#"invalid sequence "--" not allowed in comments"#));
                }
                return Ok(Token::Other);
            }
            (b0, b1) = (b1, b);
        }
    }

    /// Reads a CDATA section after `<![`, which must spell `CDATA[`.
    fn cdata(&mut self) -> Res<Token> {
        for &expected in b"CDATA[" {
            if self.mustgetc()? != expected {
                return Err(self.syntax("invalid <![ sequence"));
            }
        }
        Ok(Token::Text(self.text(None, true)?))
    }

    /// Reads a directive, such as a `DOCTYPE`, after `<!` and its first byte.
    /// Its quoted text and its nested markup are skipped as Go skips them, and
    /// its bytes are not kept. Go never reads the first byte for quotes or
    /// brackets, and neither does this.
    fn directive(&mut self) -> Res<Token> {
        let mut inquote = 0u8;
        let mut depth = 0i64;
        loop {
            let mut b = self.mustgetc()?;
            if inquote == 0 && b == b'>' && depth == 0 {
                break;
            }
            loop {
                if b == inquote {
                    inquote = 0;
                } else if inquote == 0 && (b == b'\'' || b == b'"') {
                    inquote = b;
                } else if inquote == 0 && b == b'>' {
                    depth -= 1;
                } else if inquote == 0 && b == b'<' {
                    let mut mismatch = None;
                    for &expected in b"!--" {
                        let c = self.mustgetc()?;
                        if c != expected {
                            mismatch = Some(c);
                            break;
                        }
                    }
                    if let Some(c) = mismatch {
                        depth += 1;
                        b = c;
                        continue;
                    }
                    let (mut b0, mut b1) = (0u8, 0u8);
                    loop {
                        let c = self.mustgetc()?;
                        if b0 == b'-' && b1 == b'-' && c == b'>' {
                            break;
                        }
                        (b0, b1) = (b1, c);
                    }
                }
                break;
            }
        }
        Ok(Token::Other)
    }

    /// Reads character data up to the next markup, the quote that ends an
    /// attribute value (`quote`), or the end of a CDATA section (`cdata`). The
    /// entity references are expanded and the characters are checked.
    fn text(&mut self, quote: Option<u8>, cdata: bool) -> Res<Vec<u8>> {
        let (mut b0, mut b1) = (0u8, 0u8);
        let mut trunc = 0;
        self.buf.clear();
        loop {
            let Some(b) = self.getc() else {
                if cdata {
                    if matches!(self.err, Some(Fault::Eof)) {
                        return Err(self.syntax("unexpected EOF in CDATA section"));
                    }
                    return Err(Stop);
                }
                break;
            };
            if quote.is_none() && b0 == b']' && b1 == b']' && b == b'>' {
                if cdata {
                    trunc = 2;
                    break;
                }
                return Err(self.syntax("unescaped ]]> not in CDATA section"));
            }
            if b == b'<' && !cdata {
                if quote.is_some() {
                    return Err(self.syntax("unescaped < inside quoted string"));
                }
                self.ungetc(b'<');
                break;
            }
            if quote == Some(b) {
                break;
            }
            if b == b'&' && !cdata {
                let before = self.buf.len();
                self.buf.push(b'&');
                match self.reference(before)? {
                    Some(text) => {
                        self.buf.truncate(before);
                        self.buf.extend_from_slice(&text);
                        (b0, b1) = (0, 0);
                        continue;
                    }
                    None => {
                        let mut ent = self.buf[before..].to_vec();
                        if ent.last() != Some(&b';') {
                            ent.extend_from_slice(b" (no semicolon)");
                        }
                        let text =
                            format!("invalid character entity {}", String::from_utf8_lossy(&ent));
                        return Err(self.syntax(&text));
                    }
                }
            }
            if b == b'\r' {
                self.buf.push(b'\n');
            } else if !(b1 == b'\r' && b == b'\n') {
                self.buf.push(b);
            }
            (b0, b1) = (b1, b);
        }
        let data = self.buf[..self.buf.len() - trunc].to_vec();
        self.check_characters(&data)?;
        Ok(data)
    }

    /// Reads the reference after the `&` at `before` in `buf`: a character
    /// reference or a named entity. `None` is a reference that strict mode
    /// does not take, and the bytes read stay in `buf` for its error.
    fn reference(&mut self, before: usize) -> Res<Option<Vec<u8>>> {
        let mut b = self.mustgetc()?;
        if b == b'#' {
            self.buf.push(b);
            b = self.mustgetc()?;
            let mut base = 10;
            if b == b'x' {
                base = 16;
                self.buf.push(b);
                b = self.mustgetc()?;
            }
            let start = self.buf.len();
            while b.is_ascii_digit() || (base == 16 && b.is_ascii_hexdigit()) {
                self.buf.push(b);
                b = self.mustgetc()?;
            }
            if b != b';' {
                self.ungetc(b);
                return Ok(None);
            }
            let digits = self.buf[start..].to_vec();
            self.buf.push(b';');
            return Ok(char_reference(&digits, base).map(|c| c.to_string().into_bytes()));
        }
        self.ungetc(b);
        self.read_name()?;
        let b = self.mustgetc()?;
        if b != b';' {
            self.ungetc(b);
            return Ok(None);
        }
        let name = self.buf[before + 1..].to_vec();
        self.buf.push(b';');
        Ok(named_entity(&name).map(|c| c.to_string().into_bytes()))
    }

    /// Checks that `data` is UTF-8 whose characters XML allows, in order.
    fn check_characters(&mut self, data: &[u8]) -> Res<()> {
        let mut rest = data;
        while let Some(&first) = rest.first() {
            let size = match first {
                0x00..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 0,
            };
            let decoded = rest
                .get(..size)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .and_then(|text| text.chars().next());
            let Some(c) = decoded else {
                return Err(self.syntax("invalid UTF-8"));
            };
            if !in_character_range(c) {
                let text = format!("illegal character code U+{:04X}", u32::from(c));
                return Err(self.syntax(&text));
            }
            rest = &rest[c.len_utf8()..];
        }
        Ok(())
    }

    /// Matches an end element with the start element it closes, as strict
    /// `popElement` does.
    fn pop_element(&mut self, name: Name) -> Res<Token> {
        let Some(open) = self.open.pop() else {
            let text = format!("unexpected end element </{}>", name.local);
            return Err(self.syntax(&text));
        };
        if open.local != name.local {
            let text = format!("element <{}> closed by </{}>", open.local, name.local);
            return Err(self.syntax(&text));
        }
        if open.space != name.space {
            let space = if name.space.is_empty() {
                "\"\""
            } else {
                name.space.as_str()
            };
            let text = format!(
                "element <{}> in space {} closed by </{}> in space {space}",
                open.local, open.space, name.local
            );
            return Err(self.syntax(&text));
        }
        Ok(Token::End(name))
    }

    /// The next token, as `Token` returns it: a start element is pushed on the
    /// open elements, and an end element is matched with the one it closes.
    fn token(&mut self) -> Res<Token> {
        let token = match self.raw_token() {
            Ok(token) => token,
            Err(Stop) => {
                if matches!(self.err, Some(Fault::Eof)) && !self.open.is_empty() {
                    return Err(self.syntax("unexpected EOF"));
                }
                return Err(Stop);
            }
        };
        match token {
            Token::Start(name) => {
                self.open.push(name.clone());
                Ok(Token::Start(name))
            }
            Token::End(name) => self.pop_element(name),
            other => Ok(other),
        }
    }

    /// Reads past the element that has just started, as `Skip` does.
    fn skip(&mut self) -> Res<()> {
        let mut depth = 0u64;
        loop {
            match self.token()? {
                Token::Start(_) => depth += 1,
                Token::End(_) => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Token::Text(_) | Token::Other => {}
            }
        }
    }

    /// Reads the character data of the element that has just started, skipping
    /// its children, as `unmarshal` reads a string.
    fn char_data(&mut self) -> Res<Vec<u8>> {
        let mut data = Vec::new();
        loop {
            match self.token()? {
                Token::Text(text) => data.extend_from_slice(&text),
                Token::Start(_) => self.skip()?,
                Token::End(_) => return Ok(data),
                Token::Other => {}
            }
        }
    }

    /// Reads the element that has just started as a `time.Time`: the character
    /// data of its own text, parsed as `UnmarshalText` parses it.
    fn time_element(&mut self) -> Res<Time> {
        let mut text = Vec::new();
        let mut depth = 1u64;
        while depth > 0 {
            match self.token()? {
                Token::Text(data) => {
                    if depth == 1 {
                        text.extend_from_slice(&data);
                    }
                }
                Token::Start(_) => depth += 1,
                Token::End(_) => depth -= 1,
                Token::Other => {}
            }
        }
        rfc3339(&String::from_utf8_lossy(&text)).map_err(|message| self.fail(message))
    }

    /// The value of a bool field from its text, as `copyValue` gives it: empty
    /// text is `false`.
    fn bool_value(&mut self, data: &[u8]) -> Res<bool> {
        if data.is_empty() {
            return Ok(false);
        }
        let (value, error) = parse_bool(String::from_utf8_lossy(data).trim());
        match error {
            Some(error) => Err(self.fail(error.to_string())),
            None => Ok(value),
        }
    }

    /// The value of a float64 field from its text, as `copyValue` gives it:
    /// empty text is zero.
    fn float_value(&mut self, data: &[u8]) -> Res<f64> {
        if data.is_empty() {
            return Ok(0.0);
        }
        let (value, error) = parse_float(String::from_utf8_lossy(data).trim(), 64);
        match error {
            Some(error) => Err(self.fail(error.to_string())),
            None => Ok(value),
        }
    }

    /// Reads the fields of a struct from the children of the element that has
    /// just started, as `unmarshal` does: a child that names a field is decoded
    /// into it, and any other child is skipped. A repeated `[]string` field
    /// grows, and an `any` field is skipped.
    fn struct_fields<T: GoStruct>(&mut self) -> Res<T> {
        let mut value = T::default();
        let mut lists: Vec<(&'static str, Vec<String>)> = Vec::new();
        loop {
            match self.token()? {
                Token::Start(child) => {
                    let Some(&(field, kind)) =
                        T::XML_FIELDS.iter().find(|(name, _)| *name == child.local)
                    else {
                        self.skip()?;
                        continue;
                    };
                    match kind {
                        FieldKind::Labels => self.skip()?,
                        FieldKind::Strings => {
                            let text = String::from_utf8_lossy(&self.char_data()?).into_owned();
                            match lists.iter_mut().find(|(name, _)| *name == field) {
                                Some((_, list)) => list.push(text),
                                None => lists.push((field, vec![text])),
                            }
                        }
                        FieldKind::String | FieldKind::OptString => {
                            let text = String::from_utf8_lossy(&self.char_data()?).into_owned();
                            value.set(field, Assign::String(text));
                        }
                        FieldKind::Bool | FieldKind::OptBool => {
                            let data = self.char_data()?;
                            let flag = self.bool_value(&data)?;
                            value.set(field, Assign::Bool(flag));
                        }
                        FieldKind::Float64 => {
                            let data = self.char_data()?;
                            let number = self.float_value(&data)?;
                            value.set(field, Assign::Float64(number));
                        }
                        FieldKind::OptTime => {
                            let time = self.time_element()?;
                            value.set(field, Assign::Time(time));
                        }
                    }
                }
                Token::End(_) => break,
                Token::Text(_) | Token::Other => {}
            }
        }
        for (field, list) in lists {
            value.set(field, Assign::Strings(list));
        }
        Ok(value)
    }
}

/// Whether `bytes` is a name: a first character that may start a name, then
/// characters that may continue one. Invalid UTF-8 is no name.
fn is_name(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => is_name_start(first) && chars.all(is_name_char),
        None => false,
    }
}

/// Whether `b` is an ASCII byte that a name may hold (`readName`).
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'.' | b'-')
}

/// Whether XML allows the character `c` (`isInCharacterRange`).
fn in_character_range(c: char) -> bool {
    matches!(
        u32::from(c),
        0x09 | 0x0a | 0x0d | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff
    )
}

/// The character of a numeric reference's digits in `base`, as `text` takes
/// it: a value past the largest rune, or past 64 bits, is no reference, and a
/// surrogate is the replacement character.
fn char_reference(digits: &[u8], base: u32) -> Option<char> {
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &digit in digits {
        let digit = char::from(digit).to_digit(base)?;
        value = value
            .checked_mul(u64::from(base))?
            .checked_add(u64::from(digit))?;
    }
    if value > 0x10ffff {
        return None;
    }
    Some(char::from_u32(value as u32).unwrap_or('\u{fffd}'))
}

/// The character that one of the five predefined entities names, when `name`
/// is a name (`text`'s entity table).
fn named_entity(name: &[u8]) -> Option<char> {
    if !is_name(name) {
        return None;
    }
    match name {
        b"lt" => Some('<'),
        b"gt" => Some('>'),
        b"amp" => Some('&'),
        b"apos" => Some('\''),
        b"quot" => Some('"'),
        _ => None,
    }
}

/// The value of the pseudo-attribute `param` in an XML declaration's `s`, or
/// empty when it is not there (`procInst`).
fn proc_inst<'s>(param: &[u8], s: &'s [u8]) -> &'s [u8] {
    let mut key = param.to_vec();
    key.push(b'=');
    let mut i = 0;
    let mut sep = 0u8;
    while i < s.len() {
        let sub = &s[i..];
        let Some(k) = sub.windows(key.len()).position(|window| window == key) else {
            return &[];
        };
        if key.len() + k >= sub.len() {
            return &[];
        }
        i += key.len() + k + 1;
        let c = sub[key.len() + k];
        if c == b'\'' || c == b'"' {
            sep = c;
            break;
        }
    }
    if sep == 0 {
        return &[];
    }
    match s[i..].iter().position(|&c| c == sep) {
        Some(j) => &s[i..i + j],
        None => &[],
    }
}

/// Reads the root element of `body` into `value`: the first start element is
/// the root, and everything after its end is not read.
fn decode_root<T>(
    body: &[u8],
    failure: Option<&str>,
    value: impl FnOnce(&mut Decoder<'_>, Name) -> Res<T>,
) -> Result<T, String> {
    let mut decoder = Decoder::new(body, failure);
    let start = loop {
        match decoder.token() {
            Ok(Token::Start(name)) => break name,
            Ok(_) => {}
            Err(Stop) => return Err(decoder.error_text()),
        }
    };
    value(&mut decoder, start).map_err(|Stop| decoder.error_text())
}

/// A value that an XML body decodes into: a struct, or a list that holds the
/// one struct that the root element is.
pub(crate) trait XmlEntity: Sized {
    /// Decodes `body`, which the reader ends with `failure` when a decompressor
    /// failed, or with the end of the input otherwise.
    ///
    /// # Errors
    ///
    /// Fails with the text `encoding/xml` and go-restful give.
    fn decode_body(body: &[u8], failure: Option<&str>) -> Result<Self, String>;
}

impl<T: GoStruct> XmlEntity for T {
    fn decode_body(body: &[u8], failure: Option<&str>) -> Result<Self, String> {
        decode_root(body, failure, |decoder, _| decoder.struct_fields::<T>())
    }
}

impl<T: GoStruct> XmlEntity for Vec<T> {
    fn decode_body(body: &[u8], failure: Option<&str>) -> Result<Self, String> {
        decode_root(body, failure, |decoder, _| {
            Ok(vec![decoder.struct_fields::<T>()?])
        })
    }
}
