// Ports: $GOROOT/src/bufio/scan.go (Scanner.Scan with advance inlined, setErr, Err), src/bytes/buffer.go (Buffer.Read), src/io/io.go (TeeReader), src/io/multi.go (MultiReader) @ go1.27.0 (the toolchain add21b5 builds with); github.com/subosito/gotenv v1.6.0 gotenv.go (splitLines and the reader set-up of strictParse)
//
// Derived from Go: Copyright 2009 The Go Authors. All rights reserved. Use of
// this source code is governed by a BSD-style license that can be found in
// crates/gorse-config/licenses/GO-LICENSE. Derived from gotenv: Copyright (c)
// 2013 Alif Rachmawadi. Use of this source code is governed by the MIT license
// that can be found in crates/gorse-config/licenses/GOTENV-LICENSE.

//! The line scanner gotenv reads a file with: a `bufio.Scanner` with
//! gotenv's `splitLines` over the reader chain `strictParse` builds.
//!
//! Where a line ends depends on what the scanner has buffered: `splitLines`
//! ends a line at a `"\r"` that is the last buffered byte without looking for
//! the `"\n"` after it, which then reads as an empty line. So the buffer
//! (4096 bytes, doubled up to `bufio.MaxScanTokenSize`) and every reader in
//! front of it (the three-byte BOM probe, `io.MultiReader`, x/text's
//! `transform.Reader`) are ported with their exact read sizes.

use super::transform;

/// The errors the ported readers and the scanner can report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoError {
    /// `io.EOF`.
    Eof,
    /// `bufio.ErrTooLong`.
    TooLong,
    /// `io.ErrNoProgress`.
    NoProgress,
    /// `transform.ErrShortSrc`.
    ShortSrc,
    /// `transform.ErrShortDst`.
    ShortDst,
    /// x/text's `errInconsistentByteCount`.
    InconsistentByteCount,
    /// `unicode.ErrMissingBOM`.
    MissingBom,
}

impl GoError {
    /// The error's `Error()` text.
    pub(crate) fn text(self) -> &'static str {
        match self {
            GoError::Eof => "EOF",
            GoError::TooLong => "bufio.Scanner: token too long",
            GoError::NoProgress => "multiple Read calls return no data or error",
            GoError::ShortSrc => "transform: short source buffer",
            GoError::ShortDst => "transform: short destination buffer",
            GoError::InconsistentByteCount => "transform: inconsistent byte count returned",
            GoError::MissingBom => "encoding: missing byte order mark",
        }
    }
}

/// An `io.Reader`: `Read(p)` returns the number of bytes read and the error.
pub(crate) trait GoReader {
    /// `Read(p)`.
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoError>);
}

/// The read side of a `bytes.Buffer`.
struct Buffer<'a> {
    buf: &'a [u8],
    off: usize,
}

impl GoReader for Buffer<'_> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoError>) {
        if self.off == self.buf.len() {
            if p.is_empty() {
                return (0, None);
            }
            return (0, Some(GoError::Eof));
        }
        let n = p.len().min(self.buf.len() - self.off);
        p[..n].copy_from_slice(&self.buf[self.off..self.off + n]);
        self.off += n;
        (n, None)
    }
}

/// `io.MultiReader`: the readers one after the other.
struct MultiReader<'a> {
    readers: Vec<Buffer<'a>>,
    next: usize,
}

impl GoReader for MultiReader<'_> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoError>) {
        while self.next < self.readers.len() {
            let (n, err) = self.readers[self.next].read(p);
            if err == Some(GoError::Eof) {
                self.next += 1;
            }
            if n > 0 || err != Some(GoError::Eof) {
                if err == Some(GoError::Eof) && self.next < self.readers.len() {
                    // Don't return EOF yet. More readers remain.
                    return (n, None);
                }
                return (n, err);
            }
        }
        (0, Some(GoError::Eof))
    }
}

/// The UTF-8 byte order mark.
const BOM_UTF8: &[u8] = b"\xEF\xBB\xBF";
/// The UTF-16LE byte order mark.
const BOM_UTF16_LE: &[u8] = b"\xFF\xFE";
/// The UTF-16BE byte order mark.
const BOM_UTF16_BE: &[u8] = b"\xFE\xFF";

/// The scanner `strictParse` reads `input` with (viper hands gotenv a
/// `bytes.Buffer` holding the file):
///
/// ```go
/// buf := new(bytes.Buffer)
/// tee := io.TeeReader(r, buf)
/// bomByteBuffer := make([]byte, 3)
/// _, err := tee.Read(bomByteBuffer)
/// z := io.MultiReader(buf, r)
/// ```
///
/// then a scanner over `z`, or over `transform.NewReader(z, decoder)` when
/// the probe starts with a UTF-8, UTF-16LE or UTF-16BE byte order mark.
/// `bytes.Buffer.Read` cannot fail, so the probe's error check is not needed.
pub(crate) fn new_scanner(input: &[u8]) -> Scanner<Box<dyn GoReader + '_>> {
    let mut r = Buffer { buf: input, off: 0 };
    // There can be a maximum of 3 BOM bytes. The tee copies what it reads
    // into buf; the probe keeps its zero bytes past a short input.
    let mut bom_byte_buffer = [0u8; 3];
    let (n, _) = r.read(&mut bom_byte_buffer);
    let buf = Buffer {
        buf: &input[..n],
        off: 0,
    };
    let z = MultiReader {
        readers: vec![buf, r],
        next: 0,
    };
    let reader: Box<dyn GoReader + '_> = if bom_byte_buffer.starts_with(BOM_UTF8) {
        Box::new(transform::Reader::new(
            z,
            transform::Utf8BomDecoder::default(),
        ))
    } else if bom_byte_buffer.starts_with(BOM_UTF16_LE) {
        Box::new(transform::Reader::new(
            z,
            transform::Utf16Decoder::expect_bom(true),
        ))
    } else if bom_byte_buffer.starts_with(BOM_UTF16_BE) {
        Box::new(transform::Reader::new(
            z,
            transform::Utf16Decoder::expect_bom(false),
        ))
    } else {
        Box::new(z)
    };
    Scanner::new(reader)
}

impl<R: GoReader + ?Sized> GoReader for Box<R> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoError>) {
        (**self).read(p)
    }
}

/// `bufio.MaxScanTokenSize`.
pub(crate) const MAX_SCAN_TOKEN_SIZE: usize = 64 * 1024;

/// `startBufSize`: the size of the first buffer.
const START_BUF_SIZE: usize = 4096;

/// `maxConsecutiveEmptyReads`.
const MAX_CONSECUTIVE_EMPTY_READS: usize = 100;

/// What `splitLines` returns: `(advance, token, err)`, where the only error
/// it returns is `bufio.ErrFinalToken`.
struct Split<'a> {
    advance: usize,
    token: Option<&'a [u8]>,
    final_token: bool,
}

/// gotenv's `splitLines`: lines end at `"\r"`, `"\n"` or `"\r\n"`. A `"\r"`
/// that is the last byte of `data` ends a line without waiting to see whether
/// a `"\n"` follows.
fn split_lines(data: &[u8], at_eof: bool) -> Split<'_> {
    if at_eof && data.is_empty() {
        return Split {
            advance: 0,
            token: None,
            final_token: true,
        };
    }

    let idx = data.iter().position(|&c| c == b'\r' || c == b'\n');
    let Some(idx) = idx else {
        if at_eof {
            return Split {
                advance: data.len(),
                token: Some(data),
                final_token: true,
            };
        }
        return Split {
            advance: 0,
            token: None,
            final_token: false,
        };
    };

    // consume CR or LF
    let mut eol = idx + 1;
    // detect CRLF
    if data.len() > eol && data[eol - 1] == b'\r' && data[eol] == b'\n' {
        eol += 1;
    }

    Split {
        advance: eol,
        token: Some(&data[..idx]),
        final_token: false,
    }
}

/// `bufio.Scanner` with gotenv's `splitLines`.
///
/// Go's guards against a misbehaving reader or split function
/// (`ErrBadReadCount`, `ErrNegativeAdvance`, `ErrAdvanceTooFar` and the panic
/// on too many empty tokens) are left out: the readers here never return more
/// than asked, and `splitLines` always advances when it returns a token.
pub(crate) struct Scanner<R> {
    r: R,
    max_token_size: usize,
    token: Option<Vec<u8>>,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    err: Option<GoError>,
    done: bool,
}

impl<R: GoReader> Scanner<R> {
    /// `bufio.NewScanner(r)` with `Split(splitLines)`.
    fn new(r: R) -> Self {
        Scanner {
            r,
            max_token_size: MAX_SCAN_TOKEN_SIZE,
            token: None,
            buf: Vec::new(),
            start: 0,
            end: 0,
            err: None,
            done: false,
        }
    }

    /// `Text()`: the most recent token.
    pub(crate) fn text(&self) -> &[u8] {
        self.token.as_deref().unwrap_or_default()
    }

    /// `Err()`: the first error other than `io.EOF`.
    pub(crate) fn err(&self) -> Option<GoError> {
        match self.err {
            Some(GoError::Eof) => None,
            err => err,
        }
    }

    /// `setErr`: keeps the first error, but replaces `io.EOF`.
    fn set_err(&mut self, err: GoError) {
        if self.err.is_none() || self.err == Some(GoError::Eof) {
            self.err = Some(err);
        }
    }

    /// `Scan()`: advances to the next token.
    pub(crate) fn scan(&mut self) -> bool {
        if self.done {
            return false;
        }
        // Loop until we have a token.
        loop {
            // See if we can get a token with what we already have. If we've
            // run out of data but have an error, give the split function a
            // chance to recover any remaining, possibly empty token.
            if self.end > self.start || self.err.is_some() {
                let split = split_lines(&self.buf[self.start..self.end], self.err.is_some());
                let token = split.token.map(<[u8]>::to_vec);
                if split.final_token {
                    self.token = token;
                    self.done = true;
                    // A nil token means there is no final token.
                    return self.token.is_some();
                }
                self.start += split.advance;
                self.token = token;
                if self.token.is_some() {
                    return true;
                }
            }
            // We cannot generate a token with what we are holding. If we've
            // already hit EOF or an I/O error, we are done.
            if self.err.is_some() {
                self.start = 0;
                self.end = 0;
                return false;
            }
            // Must read more data. First, shift data to beginning of buffer
            // if there's lots of empty space or space is needed.
            if self.start > 0 && (self.end == self.buf.len() || self.start > self.buf.len() / 2) {
                self.buf.copy_within(self.start..self.end, 0);
                self.end -= self.start;
                self.start = 0;
            }
            // Is the buffer full? If so, resize.
            if self.end == self.buf.len() {
                if self.buf.len() >= self.max_token_size {
                    self.set_err(GoError::TooLong);
                    return false;
                }
                let mut new_size = self.buf.len() * 2;
                if new_size == 0 {
                    new_size = START_BUF_SIZE;
                }
                new_size = new_size.min(self.max_token_size);
                let mut new_buf = vec![0; new_size];
                new_buf[..self.end - self.start].copy_from_slice(&self.buf[self.start..self.end]);
                self.buf = new_buf;
                self.end -= self.start;
                self.start = 0;
            }
            // Finally we can read some input.
            let mut empty_reads = 0;
            loop {
                let (n, err) = self.r.read(&mut self.buf[self.end..]);
                self.end += n;
                if let Some(err) = err {
                    self.set_err(err);
                    break;
                }
                if n > 0 {
                    break;
                }
                empty_reads += 1;
                if empty_reads > MAX_CONSECUTIVE_EMPTY_READS {
                    self.set_err(GoError::NoProgress);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect<R: GoReader>(mut s: Scanner<R>) -> (Vec<Vec<u8>>, Option<GoError>) {
        let mut out = Vec::new();
        while s.scan() {
            out.push(s.text().to_vec());
        }
        (out, s.err())
    }

    /// The lines `strictParse`'s scanner gives for `input`.
    fn lines(input: &[u8]) -> (Vec<Vec<u8>>, Option<GoError>) {
        collect(new_scanner(input))
    }

    /// gotenv's scanner_test.go `TestScanner`: a scanner with `splitLines`
    /// over a `strings.Reader`, which returns the whole input on the first
    /// read. gotenv's test expects a final empty line after a trailing
    /// terminator (first, third and fifth cases); Go 1.27's `Scan` returns
    /// false for the nil final token `splitLines` returns at EOF, and the
    /// test fails there when run with go1.27.0 (`porting/notes/WP-1-15-dotenv.md`,
    /// Q-125). The expectations here are Go 1.27's.
    #[test]
    fn gotenv_test_scanner() {
        /// A case: its name, the input and the lines.
        type ScannerCase<'a> = (&'a str, &'a [u8], &'a [&'a [u8]]);
        let cases: [ScannerCase<'_>; 7] = [
            (
                "regular LF split with trailing LF",
                b"aa\nbb\ncc\n",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "regular LF split with no trailing LF",
                b"aa\nbb\ncc",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "regular CR split with trailing CR",
                b"aa\rbb\rcc\r",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "regular CR split with no trailing CR",
                b"aa\rbb\rcc",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "regular CRLF split with trailing CRLF",
                b"aa\r\nbb\r\ncc\r\n",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "regular CRLF split with no trailing CRLF",
                b"aa\r\nbb\r\ncc",
                &[b"aa", b"bb", b"cc"],
            ),
            (
                "mix of possible line endings",
                b"aa\r\nbb\ncc\rdd",
                &[b"aa", b"bb", b"cc", b"dd"],
            ),
        ];
        for (name, input, want) in cases {
            let (got, err) = collect(Scanner::new(Buffer { buf: input, off: 0 }));
            assert_eq!(err, None, "{name}");
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn first_read_is_the_three_probe_bytes() {
        // The probe's three bytes come back alone, so a "\r" at index 2 ends
        // a line and its "\n" is an empty line.
        let (got, _) = lines(b"ab\r\ncd");
        assert_eq!(got, [&b"ab"[..], b"", b"cd"]);
        let (got, _) = lines(b"abc\r\nd");
        assert_eq!(got, [&b"abc"[..], b"d"]);
    }

    #[test]
    fn cr_at_the_end_of_the_first_buffer() {
        // 3 probe bytes, then 4093 more fill the 4096-byte buffer: a "\r" at
        // offset 4095 is the last buffered byte.
        let mut input = vec![b'x'; 4095];
        input.extend_from_slice(b"\r\ny");
        let (got, err) = lines(&input);
        assert_eq!(err, None);
        assert_eq!(got, [&input[..4095], b"", b"y"]);
        // One byte earlier, the "\r\n" is seen whole.
        let mut input = vec![b'x'; 4094];
        input.extend_from_slice(b"\r\ny");
        let (got, _) = lines(&input);
        assert_eq!(got, [&input[..4094], b"y"]);
    }

    #[test]
    fn token_too_long() {
        // The buffer stops growing at 64 KiB: a line and its terminator must
        // fit in it.
        let mut input = vec![b'x'; MAX_SCAN_TOKEN_SIZE - 1];
        input.push(b'\n');
        let (got, err) = lines(&input);
        assert_eq!((got.len(), err), (1, None));
        let mut input = vec![b'x'; MAX_SCAN_TOKEN_SIZE];
        input.push(b'\n');
        let (got, err) = lines(&input);
        assert_eq!((got.len(), err), (0, Some(GoError::TooLong)));
        // At EOF a line of exactly 64 KiB still fails: the buffer is full
        // before the reader reports EOF.
        let (_, err) = lines(&vec![b'x'; MAX_SCAN_TOKEN_SIZE]);
        assert_eq!(err, Some(GoError::TooLong));
        let (got, err) = lines(&vec![b'x'; MAX_SCAN_TOKEN_SIZE - 1]);
        assert_eq!((got.len(), err), (1, None));
    }
}
