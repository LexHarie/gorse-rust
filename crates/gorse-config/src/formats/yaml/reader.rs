// Ports: readerc.go (yaml_parser_determine_encoding, yaml_parser_update_raw_buffer, yaml_parser_update_buffer) @ go.yaml.in/yaml/v3 v3.0.4
//
// Derived from go.yaml.in/yaml/v3, a Go port of libyaml: Copyright (c)
// 2006-2011 Kirill Simonov, Copyright (c) 2011-2019 Canonical Ltd. MIT
// licensed; see crates/gorse-config/licenses/YAML-V3-LICENSE.

//! libyaml's reader: it detects the encoding from a byte order mark,
//! decodes UTF-8 or UTF-16 and rejects malformed sequences and the
//! characters YAML 1.1 does not allow.
//!
//! libyaml decodes lazily: each refill of its character buffer decodes the
//! next window of at most 512 raw bytes, so a malformed byte is only
//! reported once the scanner needs a character of its window. [`decode`]
//! decodes the whole input up front but records where that window starts
//! ([`Decoded::error_at`]); [`Chars`] hands the characters to the scanner
//! and raises the error when the scanner asks for a character at or past
//! that point.

use std::cell::Cell;
use std::rc::Rc;

/// libyaml's `input_raw_buffer_size`: the raw bytes one refill reads.
const RAW_BUFFER_SIZE: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

/// The input as characters, and the reader error libyaml raises, if any.
pub(crate) struct Decoded {
    /// The characters before the window holding the error (all of them when
    /// there is no error). NEL (U+0085) is replaced by `\n`; see
    /// [`decode`].
    pub(crate) chars: Vec<char>,
    /// The reader error's problem text and the number of characters libyaml
    /// has decoded before it reports it.
    pub(crate) error: Option<(&'static str, usize)>,
}

/// Decodes `input` as libyaml's reader does.
///
/// NEL (U+0085) is a line break for libyaml everywhere (`is_break`), and
/// `read_line` copies it as `\n`; the YAML 1.2 scanner the port uses treats
/// it as an ordinary character, so it is handed to the scanner as `\n`.
pub(crate) fn decode(input: &[u8]) -> Decoded {
    // yaml_parser_determine_encoding.
    let (encoding, mut pos) = if input.starts_with(b"\xff\xfe") {
        (Encoding::Utf16Le, 2)
    } else if input.starts_with(b"\xfe\xff") {
        (Encoding::Utf16Be, 2)
    } else if input.starts_with(b"\xef\xbb\xbf") {
        (Encoding::Utf8, 3)
    } else {
        (Encoding::Utf8, 0)
    };
    let mut chars = Vec::with_capacity(input.len());
    // The raw buffer holds at most RAW_BUFFER_SIZE bytes, counted from the
    // start of the input (the BOM included) for the first window and from
    // the first byte not yet decoded for the next ones.
    let mut window_end = RAW_BUFFER_SIZE.min(input.len());
    let mut window_start_chars = 0;
    loop {
        // The inner loop of yaml_parser_update_buffer over one window.
        while pos < window_end {
            match decode_one(input, pos, window_end, encoding) {
                Step::Char(value, width) => {
                    if !allowed(value) {
                        return failed(chars, "control characters are not allowed", window_start_chars);
                    }
                    chars.push(if value == 0x85 { '\n' } else { char_of(value) });
                    pos += width;
                }
                Step::Incomplete => break,
                Step::Error(problem) => return failed(chars, problem, window_start_chars),
            }
        }
        if window_end == input.len() {
            // The next read reports EOF; bytes left over are a truncated
            // sequence.
            if pos < input.len() {
                let problem = match encoding {
                    Encoding::Utf8 => "incomplete UTF-8 octet sequence",
                    Encoding::Utf16Le | Encoding::Utf16Be => {
                        if input.len() - pos < 2 {
                            "incomplete UTF-16 character"
                        } else {
                            "incomplete UTF-16 surrogate pair"
                        }
                    }
                };
                let at = chars.len();
                return failed(chars, problem, at);
            }
            return Decoded { chars, error: None };
        }
        window_start_chars = chars.len();
        window_end = (pos + RAW_BUFFER_SIZE).min(input.len());
    }
}

fn failed(mut chars: Vec<char>, problem: &'static str, at: usize) -> Decoded {
    chars.truncate(at);
    Decoded { chars, error: Some((problem, at)) }
}

enum Step {
    Char(u32, usize),
    Incomplete,
    Error(&'static str),
}

/// Decodes the character at `pos`, whose window ends at `end`.
fn decode_one(input: &[u8], pos: usize, end: usize, encoding: Encoding) -> Step {
    let raw_unread = end - pos;
    match encoding {
        Encoding::Utf8 => {
            let octet = input[pos];
            let width = if octet & 0x80 == 0x00 {
                1
            } else if octet & 0xE0 == 0xC0 {
                2
            } else if octet & 0xF0 == 0xE0 {
                3
            } else if octet & 0xF8 == 0xF0 {
                4
            } else {
                return Step::Error("invalid leading UTF-8 octet");
            };
            if width > raw_unread {
                // At EOF this is "incomplete UTF-8 octet sequence", which
                // decode reports once the window is the last one.
                return Step::Incomplete;
            }
            let mut value = u32::from(match width {
                1 => octet & 0x7F,
                2 => octet & 0x1F,
                3 => octet & 0x0F,
                _ => octet & 0x07,
            });
            for k in 1..width {
                let octet = input[pos + k];
                if octet & 0xC0 != 0x80 {
                    return Step::Error("invalid trailing UTF-8 octet");
                }
                value = (value << 6) + u32::from(octet & 0x3F);
            }
            let length_ok = match width {
                1 => true,
                2 => value >= 0x80,
                3 => value >= 0x800,
                _ => value >= 0x10000,
            };
            if !length_ok {
                return Step::Error("invalid length of a UTF-8 sequence");
            }
            if (0xD800..=0xDFFF).contains(&value) || value > 0x10FFFF {
                return Step::Error("invalid Unicode character");
            }
            Step::Char(value, width)
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let unit = |at: usize| {
                let (lo, hi) = if encoding == Encoding::Utf16Le {
                    (input[at], input[at + 1])
                } else {
                    (input[at + 1], input[at])
                };
                u32::from(lo) | (u32::from(hi) << 8)
            };
            if raw_unread < 2 {
                return Step::Incomplete;
            }
            let value = unit(pos);
            if value & 0xFC00 == 0xDC00 {
                return Step::Error("unexpected low surrogate area");
            }
            if value & 0xFC00 == 0xD800 {
                if raw_unread < 4 {
                    return Step::Incomplete;
                }
                let value2 = unit(pos + 2);
                if value2 & 0xFC00 != 0xDC00 {
                    return Step::Error("expected low surrogate area");
                }
                return Step::Char(0x10000 + ((value & 0x3FF) << 10) + (value2 & 0x3FF), 4);
            }
            Step::Char(value, 2)
        }
    }
}

/// The characters libyaml's reader accepts.
fn allowed(value: u32) -> bool {
    matches!(value, 0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0x85 | 0xA0..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn char_of(value: u32) -> char {
    // decode_one only returns scalar values (surrogates are rejected).
    char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The characters handed to the scanner. Asking for the character at
/// [`Decoded::error_at`] sets the shared error flag and ends the stream.
pub(crate) struct Chars {
    chars: Rc<[char]>,
    next: usize,
    error_at: Option<usize>,
    failed: Rc<Cell<bool>>,
}

impl Chars {
    /// The scanner's input over `chars`; `failed` is set when the scanner
    /// reaches `error_at`.
    pub(crate) fn new(chars: Rc<[char]>, error_at: Option<usize>, failed: Rc<Cell<bool>>) -> Self {
        Chars { chars, next: 0, error_at, failed }
    }
}

impl Iterator for Chars {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if Some(self.next) == self.error_at {
            self.failed.set(true);
            return None;
        }
        let c = self.chars.get(self.next).copied()?;
        self.next += 1;
        Some(c)
    }
}
