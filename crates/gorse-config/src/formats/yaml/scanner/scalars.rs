// Ports: scannerc.go (yaml_parser_scan_block_scalar, yaml_parser_scan_block_scalar_breaks, yaml_parser_scan_flow_scalar, yaml_parser_scan_plain_scalar) @ go.yaml.in/yaml/v3 v3.0.4

//! The scalars of a stream: block scalars (`|` and `>`), quoted scalars
//! (`'` and `"`) and plain scalars, with their line folding.

use super::{
    Mark, ScalarStyle, Scanner, SyntaxError, Token, TokenKind, is_blank, is_blankz, is_break,
    is_breakz, is_hex,
};

impl Scanner {
    /// `yaml_parser_scan_block_scalar`: a literal (`|`) or folded (`>`)
    /// scalar, with its indentation and chomping indicators.
    pub(super) fn scan_block_scalar(&mut self, literal: bool) -> Result<Token, SyntaxError> {
        let start_mark = self.mark;
        self.skip_char();
        self.ensure(1)?;
        let mut chomping = 0;
        let mut increment = 0;
        if self.at(0) == '+' || self.at(0) == '-' {
            chomping = if self.at(0) == '+' { 1 } else { -1 };
            self.skip_char();
            self.ensure(1)?;
            if self.at(0).is_ascii_digit() {
                if self.at(0) == '0' {
                    return Err(
                        self.scanner_error(start_mark, "found an indentation indicator equal to 0")
                    );
                }
                increment = self.at(0).to_digit(10).unwrap_or(0) as isize;
                self.skip_char();
            }
        } else if self.at(0).is_ascii_digit() {
            if self.at(0) == '0' {
                return Err(
                    self.scanner_error(start_mark, "found an indentation indicator equal to 0")
                );
            }
            increment = self.at(0).to_digit(10).unwrap_or(0) as isize;
            self.skip_char();
            self.ensure(1)?;
            if self.at(0) == '+' || self.at(0) == '-' {
                chomping = if self.at(0) == '+' { 1 } else { -1 };
                self.skip_char();
            }
        }
        self.ensure(1)?;
        while is_blank(self.at(0)) {
            self.skip_char();
            self.ensure(1)?;
        }
        if self.at(0) == '#' {
            self.scan_line_comment()?;
            while !is_breakz(self.at(0)) {
                self.skip_char();
                self.ensure(1)?;
            }
        }
        if !is_breakz(self.at(0)) {
            return Err(
                self.scanner_error(start_mark, "did not find expected comment or line break")
            );
        }
        if is_break(self.at(0)) {
            self.ensure(2)?;
            self.skip_break();
        }
        let mut end_mark = self.mark;
        let mut indent: isize = 0;
        if increment > 0 {
            indent = if self.indent >= 0 {
                self.indent + increment
            } else {
                increment
            };
        }
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        self.scan_block_scalar_breaks(
            &mut indent,
            &mut trailing_breaks,
            start_mark,
            &mut end_mark,
        )?;
        self.ensure(1)?;
        let mut leading_blank = false;
        while self.mark.column as isize == indent && !self.at_end() {
            let trailing_blank = is_blank(self.at(0));
            if !literal && !leading_blank && !trailing_blank && leading_break.starts_with('\n') {
                if trailing_breaks.is_empty() {
                    s.push(' ');
                }
            } else {
                s.push_str(&leading_break);
            }
            leading_break.clear();
            s.push_str(&trailing_breaks);
            trailing_breaks.clear();
            leading_blank = is_blank(self.at(0));
            while !is_breakz(self.at(0)) {
                self.take_char(&mut s);
                self.ensure(1)?;
            }
            self.ensure(2)?;
            self.take_break(&mut leading_break);
            self.scan_block_scalar_breaks(
                &mut indent,
                &mut trailing_breaks,
                start_mark,
                &mut end_mark,
            )?;
        }
        if chomping != -1 {
            s.push_str(&leading_break);
        }
        if chomping == 1 {
            s.push_str(&trailing_breaks);
        }
        let style = if literal {
            ScalarStyle::Literal
        } else {
            ScalarStyle::Folded
        };
        Ok(Token {
            kind: TokenKind::Scalar(s, style),
            start: start_mark,
            end: end_mark,
        })
    }

    /// `yaml_parser_scan_block_scalar_breaks`: the empty lines before a
    /// block scalar's next content line, and the indentation when it is
    /// not set.
    fn scan_block_scalar_breaks(
        &mut self,
        indent: &mut isize,
        breaks: &mut String,
        start_mark: Mark,
        end_mark: &mut Mark,
    ) -> Result<(), SyntaxError> {
        *end_mark = self.mark;
        let mut max_indent = 0;
        loop {
            self.ensure(1)?;
            while (*indent == 0 || (self.mark.column as isize) < *indent) && self.at(0) == ' ' {
                self.skip_char();
                self.ensure(1)?;
            }
            if self.mark.column > max_indent {
                max_indent = self.mark.column;
            }
            if (*indent == 0 || (self.mark.column as isize) < *indent) && self.at(0) == '\t' {
                return Err(self.scanner_error(
                    start_mark,
                    "found a tab character where an indentation space is expected",
                ));
            }
            if !is_break(self.at(0)) {
                break;
            }
            self.ensure(2)?;
            self.take_break(breaks);
            *end_mark = self.mark;
        }
        if *indent == 0 {
            *indent = max_indent as isize;
            if *indent < self.indent + 1 {
                *indent = self.indent + 1;
            }
            if *indent < 1 {
                *indent = 1;
            }
        }
        Ok(())
    }

    /// `yaml_parser_scan_flow_scalar`: a single- or double-quoted scalar, with
    /// its escapes and line folding.
    pub(super) fn scan_flow_scalar(&mut self, single: bool) -> Result<Token, SyntaxError> {
        let start_mark = self.mark;
        self.skip_char();
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        loop {
            self.ensure(4)?;
            if self.mark.column == 0
                && ((self.at(0) == '-' && self.at(1) == '-' && self.at(2) == '-')
                    || (self.at(0) == '.' && self.at(1) == '.' && self.at(2) == '.'))
                && is_blankz(self.at(3))
            {
                return Err(self.scanner_error(start_mark, "found unexpected document indicator"));
            }
            if self.at_end() {
                return Err(self.scanner_error(start_mark, "found unexpected end of stream"));
            }
            let mut leading_blanks = false;
            while !is_blankz(self.at(0)) {
                let c = self.at(0);
                if single && c == '\'' && self.at(1) == '\'' {
                    s.push('\'');
                    self.skip_char();
                    self.skip_char();
                } else if (single && c == '\'') || (!single && c == '"') {
                    break;
                } else if !single && c == '\\' && is_break(self.at(1)) {
                    self.ensure(3)?;
                    self.skip_char();
                    self.skip_break();
                    leading_blanks = true;
                    break;
                } else if !single && c == '\\' {
                    let code_length = match self.at(1) {
                        '0' => {
                            s.push('\0');
                            0
                        }
                        'a' => {
                            s.push('\x07');
                            0
                        }
                        'b' => {
                            s.push('\x08');
                            0
                        }
                        't' | '\t' => {
                            s.push('\x09');
                            0
                        }
                        'n' => {
                            s.push('\x0A');
                            0
                        }
                        'v' => {
                            s.push('\x0B');
                            0
                        }
                        'f' => {
                            s.push('\x0C');
                            0
                        }
                        'r' => {
                            s.push('\x0D');
                            0
                        }
                        'e' => {
                            s.push('\x1B');
                            0
                        }
                        ' ' => {
                            s.push('\x20');
                            0
                        }
                        '"' => {
                            s.push('"');
                            0
                        }
                        '\'' => {
                            s.push('\'');
                            0
                        }
                        '\\' => {
                            s.push('\\');
                            0
                        }
                        'N' => {
                            s.push('\u{85}');
                            0
                        }
                        '_' => {
                            s.push('\u{a0}');
                            0
                        }
                        'L' => {
                            s.push('\u{2028}');
                            0
                        }
                        'P' => {
                            s.push('\u{2029}');
                            0
                        }
                        'x' => 2,
                        'u' => 4,
                        'U' => 8,
                        _ => {
                            return Err(
                                self.scanner_error(start_mark, "found unknown escape character")
                            );
                        }
                    };
                    self.skip_char();
                    self.skip_char();
                    if code_length > 0 {
                        self.ensure(code_length)?;
                        let mut value: u32 = 0;
                        for k in 0..code_length {
                            if !is_hex(self.at(k)) {
                                return Err(self.scanner_error(
                                    start_mark,
                                    "did not find expected hexdecimal number",
                                ));
                            }
                            value = (value << 4) + self.at(k).to_digit(16).unwrap_or(0);
                        }
                        let Some(ch) = char::from_u32(value) else {
                            return Err(self.scanner_error(
                                start_mark,
                                "found invalid Unicode character escape code",
                            ));
                        };
                        s.push(ch);
                        for _ in 0..code_length {
                            self.skip_char();
                        }
                    }
                } else {
                    self.take_char(&mut s);
                }
                self.ensure(2)?;
            }
            self.ensure(1)?;
            if (single && self.at(0) == '\'') || (!single && self.at(0) == '"') {
                break;
            }
            while is_blank(self.at(0)) || is_break(self.at(0)) {
                if is_blank(self.at(0)) {
                    if !leading_blanks {
                        self.take_char(&mut whitespaces);
                    } else {
                        self.skip_char();
                    }
                } else {
                    self.ensure(2)?;
                    if !leading_blanks {
                        whitespaces.clear();
                        self.take_break(&mut leading_break);
                        leading_blanks = true;
                    } else {
                        self.take_break(&mut trailing_breaks);
                    }
                }
                self.ensure(1)?;
            }
            if leading_blanks {
                if leading_break.starts_with('\n') {
                    if trailing_breaks.is_empty() {
                        s.push(' ');
                    } else {
                        s.push_str(&trailing_breaks);
                    }
                } else {
                    s.push_str(&leading_break);
                    s.push_str(&trailing_breaks);
                }
                trailing_breaks.clear();
                leading_break.clear();
            } else {
                s.push_str(&whitespaces);
                whitespaces.clear();
            }
        }
        self.skip_char();
        let end_mark = self.mark;
        let style = if single {
            ScalarStyle::SingleQuoted
        } else {
            ScalarStyle::DoubleQuoted
        };
        Ok(Token {
            kind: TokenKind::Scalar(s, style),
            start: start_mark,
            end: end_mark,
        })
    }

    /// `yaml_parser_scan_plain_scalar`: a plain scalar, which may run over
    /// several lines in the block context.
    pub(super) fn scan_plain_scalar(&mut self) -> Result<Token, SyntaxError> {
        let mut s = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut whitespaces = String::new();
        let mut leading_blanks = false;
        let indent = self.indent + 1;
        let start_mark = self.mark;
        let mut end_mark = self.mark;
        loop {
            self.ensure(4)?;
            if self.mark.column == 0
                && ((self.at(0) == '-' && self.at(1) == '-' && self.at(2) == '-')
                    || (self.at(0) == '.' && self.at(1) == '.' && self.at(2) == '.'))
                && is_blankz(self.at(3))
            {
                break;
            }
            if self.at(0) == '#' {
                break;
            }
            while !is_blankz(self.at(0)) {
                let c = self.at(0);
                if (c == ':' && is_blankz(self.at(1)))
                    || (self.flow_level > 0 && matches!(c, ',' | '?' | '[' | ']' | '{' | '}'))
                {
                    break;
                }
                if leading_blanks || !whitespaces.is_empty() {
                    if leading_blanks {
                        if leading_break.starts_with('\n') {
                            if trailing_breaks.is_empty() {
                                s.push(' ');
                            } else {
                                s.push_str(&trailing_breaks);
                            }
                        } else {
                            s.push_str(&leading_break);
                            s.push_str(&trailing_breaks);
                        }
                        trailing_breaks.clear();
                        leading_break.clear();
                        leading_blanks = false;
                    } else {
                        s.push_str(&whitespaces);
                        whitespaces.clear();
                    }
                }
                self.take_char(&mut s);
                end_mark = self.mark;
                self.ensure(2)?;
            }
            if !(is_blank(self.at(0)) || is_break(self.at(0))) {
                break;
            }
            self.ensure(1)?;
            while is_blank(self.at(0)) || is_break(self.at(0)) {
                if is_blank(self.at(0)) {
                    if leading_blanks && (self.mark.column as isize) < indent && self.at(0) == '\t'
                    {
                        return Err(self.scanner_error(
                            start_mark,
                            "found a tab character that violates indentation",
                        ));
                    }
                    if !leading_blanks {
                        self.take_char(&mut whitespaces);
                    } else {
                        self.skip_char();
                    }
                } else {
                    self.ensure(2)?;
                    if !leading_blanks {
                        whitespaces.clear();
                        self.take_break(&mut leading_break);
                        leading_blanks = true;
                    } else {
                        self.take_break(&mut trailing_breaks);
                    }
                }
                self.ensure(1)?;
            }
            if self.flow_level == 0 && (self.mark.column as isize) < indent {
                break;
            }
        }
        if leading_blanks {
            self.simple_key_allowed = true;
        }
        Ok(Token {
            kind: TokenKind::Scalar(s, ScalarStyle::Plain),
            start: start_mark,
            end: end_mark,
        })
    }
}
