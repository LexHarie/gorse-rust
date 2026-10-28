// Ports: scannerc.go (yaml_parser_scan_directive, yaml_parser_scan_directive_name, yaml_parser_scan_version_directive_value, yaml_parser_scan_version_directive_number, yaml_parser_scan_tag_directive_value, yaml_parser_scan_anchor, yaml_parser_scan_tag, yaml_parser_scan_tag_handle, yaml_parser_scan_tag_uri, yaml_parser_scan_uri_escapes) @ go.yaml.in/yaml/v3 v3.0.4

//! The directives, anchors, aliases and tags of a stream: libyaml's lexemes
//! other than scalars.

use super::{Mark, Scanner, SyntaxError, Token, TokenKind, is_alpha, is_blank, is_blankz, is_break, is_breakz, is_hex};

/// The longest number a version directive may hold (`max_number_length`).
const MAX_NUMBER_LENGTH: usize = 2;

impl Scanner {
    /// `yaml_parser_scan_directive`: a `%YAML` or `%TAG` directive line.
    pub(super) fn scan_directive(&mut self) -> Result<Token, SyntaxError> {
        let start_mark = self.mark;
        self.skip_char();
        let name = self.scan_directive_name(start_mark)?;
        let kind = if name == "YAML" {
            let (major, minor) = self.scan_version_directive_value(start_mark)?;
            TokenKind::VersionDirective(major, minor)
        } else if name == "TAG" {
            let (handle, prefix) = self.scan_tag_directive_value(start_mark)?;
            TokenKind::TagDirective(handle, prefix)
        } else {
            return Err(self.scanner_error(start_mark, "found unknown directive name"));
        };
        let end_mark = self.mark;
        self.ensure(1)?;
        while is_blank(self.at(0)) {
            self.skip_char();
            self.ensure(1)?;
        }
        if self.at(0) == '#' {
            while !is_breakz(self.at(0)) {
                self.skip_char();
                self.ensure(1)?;
            }
        }
        if !is_breakz(self.at(0)) {
            return Err(self.scanner_error(start_mark, "did not find expected comment or line break"));
        }
        if is_break(self.at(0)) {
            self.ensure(2)?;
            self.skip_break();
        }
        Ok(Token { kind, start: start_mark, end: end_mark })
    }

    /// `yaml_parser_scan_directive_name`.
    fn scan_directive_name(&mut self, start_mark: Mark) -> Result<String, SyntaxError> {
        self.ensure(1)?;
        let mut s = String::new();
        while is_alpha(self.at(0)) {
            self.take_char(&mut s);
            self.ensure(1)?;
        }
        if s.is_empty() {
            return Err(self.scanner_error(start_mark, "could not find expected directive name"));
        }
        if !is_blankz(self.at(0)) {
            return Err(self.scanner_error(start_mark, "found unexpected non-alphabetical character"));
        }
        Ok(s)
    }

    /// `yaml_parser_scan_version_directive_value`: the major and minor numbers.
    fn scan_version_directive_value(&mut self, start_mark: Mark) -> Result<(u32, u32), SyntaxError> {
        self.ensure(1)?;
        while is_blank(self.at(0)) {
            self.skip_char();
            self.ensure(1)?;
        }
        let major = self.scan_version_directive_number(start_mark)?;
        if self.at(0) != '.' {
            return Err(self.scanner_error(start_mark, "did not find expected digit or '.' character"));
        }
        self.skip_char();
        let minor = self.scan_version_directive_number(start_mark)?;
        Ok((major, minor))
    }

    /// `yaml_parser_scan_version_directive_number`: one or two digits.
    fn scan_version_directive_number(&mut self, start_mark: Mark) -> Result<u32, SyntaxError> {
        self.ensure(1)?;
        let mut value = 0;
        let mut length = 0;
        while self.at(0).is_ascii_digit() {
            length += 1;
            if length > MAX_NUMBER_LENGTH {
                return Err(self.scanner_error(start_mark, "found extremely long version number"));
            }
            value = value * 10 + self.at(0).to_digit(10).unwrap_or(0);
            self.skip_char();
            self.ensure(1)?;
        }
        if length == 0 {
            return Err(self.scanner_error(start_mark, "did not find expected version number"));
        }
        Ok(value)
    }

    /// `yaml_parser_scan_tag_directive_value`: the handle and the prefix.
    fn scan_tag_directive_value(&mut self, start_mark: Mark) -> Result<(String, String), SyntaxError> {
        self.ensure(1)?;
        while is_blank(self.at(0)) {
            self.skip_char();
            self.ensure(1)?;
        }
        let handle = self.scan_tag_handle(true, start_mark)?;
        self.ensure(1)?;
        if !is_blank(self.at(0)) {
            return Err(self.scanner_error(start_mark, "did not find expected whitespace"));
        }
        while is_blank(self.at(0)) {
            self.skip_char();
            self.ensure(1)?;
        }
        let prefix = self.scan_tag_uri("", start_mark)?;
        self.ensure(1)?;
        if !is_blankz(self.at(0)) {
            return Err(self.scanner_error(start_mark, "did not find expected whitespace or line break"));
        }
        Ok((handle, prefix))
    }

    /// `yaml_parser_scan_anchor`: an anchor (`&name`) or an alias (`*name`).
    pub(super) fn scan_anchor(&mut self, alias: bool) -> Result<Token, SyntaxError> {
        let start_mark = self.mark;
        self.skip_char();
        self.ensure(1)?;
        let mut s = String::new();
        while is_alpha(self.at(0)) {
            self.take_char(&mut s);
            self.ensure(1)?;
        }
        let end_mark = self.mark;
        let c = self.at(0);
        if s.is_empty() || !(is_blankz(c) || matches!(c, '?' | ':' | ',' | ']' | '}' | '%' | '@' | '`')) {
            return Err(self.scanner_error(start_mark, "did not find expected alphabetic or numeric character"));
        }
        let kind = if alias { TokenKind::Alias(s) } else { TokenKind::Anchor(s) };
        Ok(Token { kind, start: start_mark, end: end_mark })
    }

    /// `yaml_parser_scan_tag`: a tag, verbatim (`!<uri>`), shorthand
    /// (`!handle!suffix`) or non-specific (`!`).
    pub(super) fn scan_tag(&mut self) -> Result<Token, SyntaxError> {
        let start_mark = self.mark;
        let mut handle;
        let mut suffix;
        self.ensure(2)?;
        if self.at(1) == '<' {
            self.skip_char();
            self.skip_char();
            handle = String::new();
            suffix = self.scan_tag_uri("", start_mark)?;
            if self.at(0) != '>' {
                return Err(self.scanner_error(start_mark, "did not find the expected '>'"));
            }
            self.skip_char();
        } else {
            handle = self.scan_tag_handle(false, start_mark)?;
            if handle.len() > 1 && handle.starts_with('!') && handle.ends_with('!') {
                suffix = self.scan_tag_uri("", start_mark)?;
            } else {
                suffix = self.scan_tag_uri(&handle, start_mark)?;
                handle = "!".to_owned();
                if suffix.is_empty() {
                    // Go swaps the two: the handle is empty and the suffix is "!".
                    std::mem::swap(&mut handle, &mut suffix);
                }
            }
        }
        self.ensure(1)?;
        if !is_blankz(self.at(0)) {
            return Err(self.scanner_error(start_mark, "did not find expected whitespace or line break"));
        }
        let end_mark = self.mark;
        Ok(Token { kind: TokenKind::Tag(handle, suffix), start: start_mark, end: end_mark })
    }

    /// `yaml_parser_scan_tag_handle`: `!`, `!!`, `!name!`, or for a
    /// directive the handle with its closing `!`.
    fn scan_tag_handle(&mut self, directive: bool, start_mark: Mark) -> Result<String, SyntaxError> {
        self.ensure(1)?;
        if self.at(0) != '!' {
            return Err(self.scanner_error(start_mark, "did not find expected '!'"));
        }
        let mut s = String::new();
        self.take_char(&mut s);
        self.ensure(1)?;
        while is_alpha(self.at(0)) {
            self.take_char(&mut s);
            self.ensure(1)?;
        }
        if self.at(0) == '!' {
            self.take_char(&mut s);
        } else if directive && s != "!" {
            return Err(self.scanner_error(start_mark, "did not find expected '!'"));
        }
        Ok(s)
    }

    /// `yaml_parser_scan_tag_uri`: the URI characters, after `head` (the
    /// handle, minus its `!`, when there is one). The octets of a
    /// percent-escaped URI are not always UTF-8; they are replaced as
    /// `String::from_utf8_lossy` does.
    fn scan_tag_uri(&mut self, head: &str, start_mark: Mark) -> Result<String, SyntaxError> {
        let mut s: Vec<u8> = Vec::new();
        let mut has_tag = !head.is_empty();
        if head.len() > 1 {
            s.extend_from_slice(&head.as_bytes()[1..]);
        }
        self.ensure(1)?;
        while is_uri_char(self.at(0)) {
            if self.at(0) == '%' {
                self.scan_uri_escapes(start_mark, &mut s)?;
            } else {
                let c = self.advance_char();
                let mut utf8 = [0; 4];
                s.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
            }
            self.ensure(1)?;
            has_tag = true;
        }
        if !has_tag {
            return Err(self.scanner_error(start_mark, "did not find expected tag URI"));
        }
        Ok(String::from_utf8_lossy(&s).into_owned())
    }

    /// `yaml_parser_scan_uri_escapes`: the octets of one UTF-8 character,
    /// each written `%XX`.
    fn scan_uri_escapes(&mut self, start_mark: Mark, s: &mut Vec<u8>) -> Result<(), SyntaxError> {
        let mut w = 1024;
        while w > 0 {
            self.ensure(3)?;
            if !(self.at(0) == '%' && is_hex(self.at(1)) && is_hex(self.at(2))) {
                return Err(self.scanner_error(start_mark, "did not find URI escaped octet"));
            }
            let octet = (hex_value(self.at(1)) << 4) + hex_value(self.at(2));
            if w == 1024 {
                w = utf8_width(octet);
                if w == 0 {
                    return Err(self.scanner_error(start_mark, "found an incorrect leading UTF-8 octet"));
                }
            } else if octet & 0xC0 != 0x80 {
                return Err(self.scanner_error(start_mark, "found an incorrect trailing UTF-8 octet"));
            }
            s.push(octet);
            self.skip_char();
            self.skip_char();
            self.skip_char();
            w -= 1;
        }
        Ok(())
    }
}

/// The characters a URI may hold (`yaml_parser_scan_tag_uri`), `%` included.
fn is_uri_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '_' | '-' | ';' | '/' | '?' | ':' | '@' | '&' | '=' | '+' | '$' | ',' | '.' | '!' | '~' | '*' | '\'' | '(' | ')' | '[' | ']' | '%'
        )
}

/// The value of a hexadecimal digit (`as_hex`).
fn hex_value(c: char) -> u8 {
    c.to_digit(16).map_or(0, |d| d as u8)
}

/// The length of a UTF-8 sequence from its first octet, or 0 when the
/// octet cannot start one (`width`).
fn utf8_width(octet: u8) -> usize {
    if octet & 0x80 == 0x00 {
        1
    } else if octet & 0xE0 == 0xC0 {
        2
    } else if octet & 0xF0 == 0xE0 {
        3
    } else if octet & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}
