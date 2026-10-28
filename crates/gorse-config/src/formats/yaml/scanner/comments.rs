// Ports: scannerc.go (yaml_parser_scan_line_comment, yaml_parser_scan_comments) @ go.yaml.in/yaml/v3 v3.0.4

//! Comments. yaml.v3 keeps them for the nodes it builds; the decoder does not
//! read them, but their marks decide where block-end tokens go (see
//! [`Scanner::unroll_indent`]) and the reader errors inside them are reported,
//! so they are scanned in full. Only the marks are kept.

use super::{Comment, Mark, Scanner, SyntaxError, TokenKind, is_blank, is_break, is_breakz};

impl Scanner {
    /// `yaml_parser_scan_line_comment`: a comment after a token on its line.
    /// Nothing is kept for a comment on a line of its own, which is the case
    /// `newlines` > 0 leaves to the foot-comment scan. Lookahead failures
    /// latch the reader's error and end the scan.
    pub(super) fn scan_line_comment(&mut self) -> Result<(), SyntaxError> {
        if self.newlines > 0 {
            return Ok(());
        }
        let mut start_mark = Mark::default();
        let mut has_text = false;
        let mut peek = 0;
        while peek < 512 {
            if !self.ensure_soft(peek + 1) {
                break;
            }
            if is_blank(self.at(peek)) {
                peek += 1;
                continue;
            }
            if self.at(peek) == '#' {
                let seen = self.mark.index + peek;
                loop {
                    self.ensure(1)?;
                    if is_breakz(self.at(0)) {
                        if self.mark.index >= seen {
                            break;
                        }
                        self.ensure(2)?;
                        self.skip_break();
                    } else if self.mark.index >= seen {
                        if !has_text {
                            start_mark = self.mark;
                        }
                        self.advance_char();
                        has_text = true;
                    } else {
                        self.skip_char();
                    }
                }
            }
            break;
        }
        if has_text {
            self.comments.push(Comment {
                scan_mark: Mark::default(),
                start_mark,
                end_mark: Mark::default(),
            });
        }
        Ok(())
    }

    /// `yaml_parser_scan_comments`: the comments before the next token, which
    /// are recorded as foot comments of the previous token or head comments of
    /// the next one. The marks are all that is kept.
    pub(super) fn scan_comments(&mut self, mut scan_mark: Mark) -> Result<(), SyntaxError> {
        let last = self.tokens.len() - 1;
        let token_index = if matches!(self.tokens[last].kind, TokenKind::FlowEntry) && last > 0 {
            last - 1
        } else {
            last
        };
        let token_is_value = matches!(self.tokens[token_index].kind, TokenKind::Value);
        let mut next_indent = self.indent.max(0);
        let mut recent_empty = false;
        let mut first_empty = self.newlines <= 1;
        let mut line = self.mark.line;
        let mut column = self.mark.column;
        let mut has_text = false;
        let mut foot_line: isize = -1;
        if scan_mark.line > 0 {
            foot_line = self.mark.line as isize - self.newlines as isize + 1;
            if self.newlines == 0 && self.mark.column > 1 {
                foot_line += 1;
            }
        }
        let mut start_mark = Mark::default();
        let mut peek = 0;
        while peek < 512 {
            if !self.ensure_soft(peek + 1) {
                break;
            }
            column += 1;
            if is_blank(self.at(peek)) {
                peek += 1;
                continue;
            }
            let c = self.at(peek);
            let close_flow = self.flow_level > 0 && (c == ']' || c == '}');
            if close_flow || is_breakz(c) {
                if close_flow || !recent_empty {
                    let foot_here = start_mark.line as isize == foot_line && !token_is_value;
                    let indented = start_mark.column as isize - 1 < next_indent;
                    if (close_flow || (first_empty && (foot_here || indented))) && has_text {
                        let end_mark = Mark {
                            index: self.mark.index + peek,
                            line,
                            column,
                        };
                        self.comments.push(Comment {
                            scan_mark,
                            start_mark,
                            end_mark,
                        });
                        scan_mark = Mark {
                            index: self.mark.index + peek,
                            line,
                            column,
                        };
                        has_text = false;
                    }
                }
                if !is_break(self.at(peek)) {
                    break;
                }
                first_empty = false;
                recent_empty = true;
                column = 0;
                line += 1;
                peek += 1;
                continue;
            }
            if has_text
                && (close_flow
                    || (column as isize - 1 < next_indent && column != start_mark.column))
            {
                let end_mark = Mark {
                    index: self.mark.index + peek,
                    line,
                    column,
                };
                self.comments.push(Comment {
                    scan_mark,
                    start_mark,
                    end_mark,
                });
                scan_mark = Mark {
                    index: self.mark.index + peek,
                    line,
                    column,
                };
                has_text = false;
            }
            if self.at(peek) != '#' {
                break;
            }
            if !has_text {
                start_mark = Mark {
                    index: self.mark.index + peek,
                    line,
                    column,
                };
            }
            recent_empty = false;
            let seen = self.mark.index + peek;
            loop {
                self.ensure(1)?;
                if is_breakz(self.at(0)) {
                    if self.mark.index >= seen {
                        break;
                    }
                    self.ensure(2)?;
                    self.skip_break();
                } else if self.mark.index >= seen {
                    self.advance_char();
                    has_text = true;
                } else {
                    self.skip_char();
                }
            }
            // Go sets peek to 0 here and the loop's post statement adds one.
            peek = 1;
            column = 0;
            line = self.mark.line;
            next_indent = self.indent.max(0);
        }
        if has_text {
            let end_mark = Mark {
                index: (self.mark.index + peek).saturating_sub(1),
                line,
                column,
            };
            self.comments.push(Comment {
                scan_mark,
                start_mark,
                end_mark,
            });
        }
        Ok(())
    }
}
