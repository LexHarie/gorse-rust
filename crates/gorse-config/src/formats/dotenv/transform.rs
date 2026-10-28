// Ports: golang.org/x/text v0.36.0 transform/transform.go (Reader), encoding/unicode/unicode.go (utf8bomDecoder, utf8Decoder, utf16Decoder), internal/utf8internal/utf8internal.go; $GOROOT/src/unicode/utf16/utf16.go (IsSurrogate, DecodeRune) @ go1.27.0 (the versions add21b5's go.mod selects)
//
// Derived from golang.org/x/text and Go: Copyright 2009 The Go Authors. All
// rights reserved. Use of this source code is governed by a BSD-style license
// that can be found in crates/gorse-config/licenses/GO-LICENSE (x/text ships
// the same text).

//! The decoding reader gotenv puts in front of the scanner when a file starts
//! with a byte order mark: `transform.NewReader(z, d)` with
//! `unicode.UTF8BOM.NewDecoder()` or
//! `unicode.UTF16(endianness, unicode.ExpectBOM).NewDecoder()`.
//!
//! The reader is ported with its 4096-byte buffers because the chunks it
//! returns decide where the scanner's buffer ends, and a `"\r"` that is the
//! last byte of the buffer ends a line on its own (see `scan.rs`).

use super::gostr::{ACCEPT_RANGES, FIRST, HICB, LOCB, RUNE_ERROR, RUNE_SELF, XX};
use super::scan::{GoError, GoReader};

/// `defaultBufSize`: the size of the reader's source and destination buffers.
const DEFAULT_BUF_SIZE: usize = 4096;

/// A `transform.Transformer`.
pub(crate) trait Transformer {
    /// `Transform(dst, src, atEOF)`: the bytes written to `dst`, the bytes
    /// read from `src`, and the error.
    fn transform(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        at_eof: bool,
    ) -> (usize, usize, Option<GoError>);

    /// `Reset()`.
    fn reset(&mut self);
}

/// `transform.Reader`: reads from `r` and transforms the bytes with `t`.
pub(crate) struct Reader<R, T> {
    r: R,
    t: T,
    err: Option<GoError>,
    /// `dst[dst0..dst1]` holds transformed bytes not yet copied out.
    dst: Vec<u8>,
    dst0: usize,
    dst1: usize,
    /// `src[src0..src1]` holds bytes read from `r` not yet transformed.
    src: Vec<u8>,
    src0: usize,
    src1: usize,
    transform_complete: bool,
}

impl<R: GoReader, T: Transformer> Reader<R, T> {
    /// `transform.NewReader(r, t)`, which resets `t`.
    pub(crate) fn new(r: R, mut t: T) -> Self {
        t.reset();
        Reader {
            r,
            t,
            err: None,
            dst: vec![0; DEFAULT_BUF_SIZE],
            dst0: 0,
            dst1: 0,
            src: vec![0; DEFAULT_BUF_SIZE],
            src0: 0,
            src1: 0,
            transform_complete: false,
        }
    }
}

impl<R: GoReader, T: Transformer> GoReader for Reader<R, T> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoError>) {
        loop {
            // Copy out any transformed bytes and return the final error if we
            // are done.
            if self.dst0 != self.dst1 {
                let n = copy(p, &self.dst[self.dst0..self.dst1]);
                self.dst0 += n;
                if self.dst0 == self.dst1 && self.transform_complete {
                    return (n, self.err);
                }
                return (n, None);
            } else if self.transform_complete {
                return (0, self.err);
            }

            // Try to transform some source bytes, or to flush the transformer
            // if we are out of source bytes, even if r.Read returned an error.
            if self.src0 != self.src1 || self.err.is_some() {
                self.dst0 = 0;
                let at_eof = self.err == Some(GoError::Eof);
                let (n_dst, n, err) =
                    self.t
                        .transform(&mut self.dst, &self.src[self.src0..self.src1], at_eof);
                self.dst1 = n_dst;
                self.src0 += n;

                match err {
                    None => {
                        if self.src0 != self.src1 {
                            self.err = Some(GoError::InconsistentByteCount);
                        }
                        // The Transform call was successful; we are complete
                        // if we cannot read more bytes into src.
                        self.transform_complete = self.err.is_some();
                        continue;
                    }
                    Some(GoError::ShortDst) if self.dst1 != 0 || n != 0 => {
                        // Make room in dst by copying out, and try again.
                        continue;
                    }
                    Some(GoError::ShortSrc)
                        if self.src1 - self.src0 != self.src.len() && self.err.is_none() =>
                    {
                        // Read more bytes into src below, and try again.
                    }
                    Some(e) => {
                        self.transform_complete = true;
                        // The reader error takes precedence over the
                        // transformer error unless it is nil or io.EOF.
                        if self.err.is_none() || self.err == Some(GoError::Eof) {
                            self.err = Some(e);
                        }
                        continue;
                    }
                }
            }

            // Move any untransformed source bytes to the start of the buffer
            // and read more bytes.
            if self.src0 != 0 {
                self.src.copy_within(self.src0..self.src1, 0);
                self.src1 -= self.src0;
                self.src0 = 0;
            }
            let (n, err) = self.r.read(&mut self.src[self.src1..]);
            self.err = err;
            self.src1 += n;
        }
    }
}

/// Go's `copy(dst, src)`.
fn copy(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

/// `unicode.UTF8BOM.NewDecoder()`: drops a leading UTF-8 byte order mark,
/// then decodes as `utf8Decoder`.
#[derive(Default)]
pub(crate) struct Utf8BomDecoder {
    checked: bool,
}

/// The UTF-8 byte order mark.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

impl Transformer for Utf8BomDecoder {
    fn transform(
        &mut self,
        dst: &mut [u8],
        mut src: &[u8],
        at_eof: bool,
    ) -> (usize, usize, Option<GoError>) {
        let mut n_src = 0;
        if !self.checked {
            if !at_eof && src.len() < UTF8_BOM.len() {
                if src.is_empty() {
                    return (0, 0, None);
                }
                return (0, 0, Some(GoError::ShortSrc));
            }
            if src.starts_with(UTF8_BOM) {
                n_src += UTF8_BOM.len();
                src = &src[UTF8_BOM.len()..];
            }
            self.checked = true;
        }
        let (n_dst, n, err) = utf8_decode(dst, src, at_eof);
        (n_dst, n_src + n, err)
    }

    fn reset(&mut self) {
        self.checked = false;
    }
}

/// `utf8Decoder.Transform`: copies valid UTF-8 and writes U+FFFD for each
/// maximal subpart of an ill-formed sequence (the W3C way, not Go's
/// one-byte-at-a-time way).
fn utf8_decode(dst: &mut [u8], src: &[u8], mut at_eof: bool) -> (usize, usize, Option<GoError>) {
    let mut n_dst = 0;
    let mut n_src = 0;
    let mut err = None;
    // The point from which to start copying in src.
    let mut p_src = 0;

    // The decoder can only make the input larger, not smaller.
    let mut n = src.len();
    if dst.len() < n {
        err = Some(GoError::ShortDst);
        n = dst.len();
        at_eof = false;
    }
    while n_src < n {
        let c = src[n_src];
        if c < RUNE_SELF {
            n_src += 1;
            continue;
        }
        let first = FIRST[usize::from(c)];
        let size = usize::from(first & 7);
        // The size of the ill-formed sequence to replace, if any.
        let invalid = if first == XX {
            // Invalid starter byte.
            Some(1)
        } else {
            let (lo, hi) = ACCEPT_RANGES[usize::from(first >> 4)];
            if n_src + size > n {
                if !at_eof {
                    // We may stop earlier than necessary here if the short
                    // sequence has invalid bytes.
                    if err.is_none() {
                        err = Some(GoError::ShortSrc);
                    }
                    break;
                }
                // Determine the maximal subpart of an ill-formed subsequence.
                if n_src + 1 >= n || src[n_src + 1] < lo || hi < src[n_src + 1] {
                    Some(1)
                } else if n_src + 2 >= n || src[n_src + 2] < LOCB || HICB < src[n_src + 2] {
                    Some(2)
                } else {
                    // As we are short, the maximum is 3.
                    Some(3)
                }
            } else if src[n_src + 1] < lo || hi < src[n_src + 1] {
                Some(1)
            } else if size == 2 {
                None
            } else if src[n_src + 2] < LOCB || HICB < src[n_src + 2] {
                Some(2)
            } else if size == 3 {
                None
            } else if src[n_src + 3] < LOCB || HICB < src[n_src + 3] {
                Some(3)
            } else {
                None
            }
        };
        let Some(size) = invalid else {
            n_src += size;
            continue;
        };

        // Copy the scanned input so far.
        n_dst += copy(&mut dst[n_dst..], &src[p_src..n_src]);

        // Append RuneError to the destination.
        const RUNE_ERROR_UTF8: &[u8] = "\u{fffd}".as_bytes();
        if n_dst + RUNE_ERROR_UTF8.len() > dst.len() {
            return (n_dst, n_src, Some(GoError::ShortDst));
        }
        n_dst += copy(&mut dst[n_dst..], RUNE_ERROR_UTF8);

        // Skip the maximal subpart of an ill-formed subsequence.
        n_src += size;
        p_src = n_src;

        // Recompute the maximum source length.
        let sz = dst.len() - n_dst;
        if sz < src.len() - n_src {
            err = Some(GoError::ShortDst);
            n = n_src + sz;
            at_eof = false;
        }
    }
    let copied = copy(&mut dst[n_dst..], &src[p_src..n_src]);
    (n_dst + copied, n_src, err)
}

/// `BOMPolicy` bit `acceptBOM`.
const ACCEPT_BOM: u8 = 0x02;
/// `BOMPolicy` bit `requireBOM`.
const REQUIRE_BOM: u8 = 0x04;
/// `IgnoreBOM`.
const IGNORE_BOM: u8 = 0;
/// `ExpectBOM` (`writeBOM | acceptBOM | requireBOM`).
const EXPECT_BOM: u8 = 0x01 | ACCEPT_BOM | REQUIRE_BOM;

/// A UTF-16 decoder's `config`.
#[derive(Clone, Copy)]
struct Config {
    little_endian: bool,
    bom_policy: u8,
}

/// `unicode.UTF16(e, unicode.ExpectBOM).NewDecoder()`.
pub(crate) struct Utf16Decoder {
    initial: Config,
    current: Config,
}

impl Utf16Decoder {
    /// The decoder for `ExpectBOM` with the given default endianness.
    pub(crate) fn expect_bom(little_endian: bool) -> Self {
        let config = Config {
            little_endian,
            bom_policy: EXPECT_BOM,
        };
        Utf16Decoder {
            initial: config,
            current: config,
        }
    }
}

/// `utf16.IsSurrogate`.
fn is_surrogate(r: u32) -> bool {
    (0xD800..0xE000).contains(&r)
}

/// x/text's `isHighSurrogate`, which tests the range of the second
/// (low, trail) surrogate.
fn is_high_surrogate(r: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&r)
}

/// `utf16.DecodeRune`: the code point of a surrogate pair, or U+FFFD when
/// `r1` and `r2` are not one.
fn decode_surrogates(r1: u32, r2: u32) -> u32 {
    if (0xD800..0xDC00).contains(&r1) && (0xDC00..0xE000).contains(&r2) {
        (((r1 - 0xD800) << 10) | (r2 - 0xDC00)) + 0x10000
    } else {
        RUNE_ERROR
    }
}

impl Transformer for Utf16Decoder {
    fn transform(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        at_eof: bool,
    ) -> (usize, usize, Option<GoError>) {
        if src.len() < 2 && at_eof && self.current.bom_policy & REQUIRE_BOM != 0 {
            return (0, 0, Some(GoError::MissingBom));
        }
        if src.is_empty() {
            return (0, 0, None);
        }
        let mut n_src = 0;
        if src.len() >= 2 && self.current.bom_policy & ACCEPT_BOM != 0 {
            match (src[0], src[1]) {
                (0xFE, 0xFF) => {
                    self.current.little_endian = false;
                    n_src = 2;
                }
                (0xFF, 0xFE) => {
                    self.current.little_endian = true;
                    n_src = 2;
                }
                _ => {
                    if self.current.bom_policy & REQUIRE_BOM != 0 {
                        return (0, 0, Some(GoError::MissingBom));
                    }
                }
            }
            self.current.bom_policy = IGNORE_BOM;
        }

        let unit = |hi: u8, lo: u8, little_endian: bool| -> u32 {
            let x = (u16::from(hi) << 8) | u16::from(lo);
            u32::from(if little_endian { x.swap_bytes() } else { x })
        };
        let mut n_dst = 0;
        let mut err = None;
        while n_src < src.len() {
            let (mut r, d_size, s_size);
            if n_src + 1 < src.len() {
                r = unit(src[n_src], src[n_src + 1], self.current.little_endian);
                let mut size = 2;
                if is_surrogate(r) {
                    if n_src + 3 < src.len() {
                        let x = unit(src[n_src + 2], src[n_src + 3], self.current.little_endian);
                        // Save for next iteration if it is not a high
                        // surrogate.
                        if is_high_surrogate(x) {
                            r = decode_surrogates(r, x);
                            size = 4;
                        }
                    } else if !at_eof {
                        err = Some(GoError::ShortSrc);
                        break;
                    }
                }
                s_size = size;
                // utf8.RuneLen is -1 for a surrogate.
                if is_surrogate(r) {
                    r = RUNE_ERROR;
                }
                d_size = char::from_u32(r).map_or(3, char::len_utf8);
            } else if at_eof {
                // Single trailing byte.
                r = RUNE_ERROR;
                d_size = 3;
                s_size = 1;
            } else {
                err = Some(GoError::ShortSrc);
                break;
            }
            if n_dst + d_size > dst.len() {
                err = Some(GoError::ShortDst);
                break;
            }
            let c = char::from_u32(r).unwrap_or(char::REPLACEMENT_CHARACTER);
            n_dst += c.encode_utf8(&mut dst[n_dst..]).len();
            n_src += s_size;
        }
        (n_dst, n_src, err)
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a transformer over `src` in one call at EOF.
    fn run(t: &mut dyn Transformer, src: &[u8]) -> (Vec<u8>, usize, Option<GoError>) {
        let mut dst = vec![0; 64];
        let (n_dst, n_src, err) = t.transform(&mut dst, src, true);
        dst.truncate(n_dst);
        (dst, n_src, err)
    }

    #[test]
    fn utf8_decoder_replaces_maximal_subparts() {
        // x/text replaces "\xE2\x82" (a truncated sequence) with one U+FFFD
        // where Go's range loop gives two.
        let mut d = Utf8BomDecoder::default();
        let (out, n, err) = run(&mut d, b"\xEF\xBB\xBFa\xE2\x82b\xF0\x90\x80\xFF");
        assert_eq!(out, "a\u{fffd}b\u{fffd}\u{fffd}".as_bytes());
        assert_eq!((n, err), (11, None));
    }

    #[test]
    fn utf8_decoder_waits_for_a_split_sequence() {
        let mut dst = [0u8; 16];
        let (n_dst, n_src, err) = utf8_decode(&mut dst, b"ab\xE2\x82", false);
        assert_eq!((n_dst, n_src, err), (2, 2, Some(GoError::ShortSrc)));
    }

    #[test]
    fn utf16_decoder_surrogates() {
        let mut d = Utf16Decoder::expect_bom(false);
        // A pair, a lone trail, two trails (one U+FFFD for both), a lone
        // lead at the end and a trailing odd byte.
        let src = b"\xFE\xFF\xD8\x00\xDC\x00\xDC\x00\x00a\xDC\x00\xDC\x01\xD8\x00\x00";
        let (out, n, err) = run(&mut d, src);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\u{10000}\u{fffd}a\u{fffd}\u{fffd}\u{fffd}"
        );
        assert_eq!((n, err), (src.len(), None));
    }

    #[test]
    fn utf16_decoder_requires_a_bom() {
        let mut d = Utf16Decoder::expect_bom(true);
        assert_eq!(run(&mut d, b"a\x00").2, Some(GoError::MissingBom));
        d.reset();
        assert_eq!(run(&mut d, b"a").2, Some(GoError::MissingBom));
    }
}
