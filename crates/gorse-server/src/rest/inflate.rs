// Ports: compress/flate, compress/gzip and compress/zlib at Go 1.27.0, as
// go-restful v3.13.0's ReadEntity runs them on a `Content-Encoding` of `gzip`
// or `deflate`. The inflater reads the input a byte at a time, as Go's does, so
// the offset in its corrupt-input error is the same.

//! Request bodies that are compressed. [`gzip`] and [`zlib`] inflate a body
//! as go-restful's `ReadEntity` does, and they keep what came out before the
//! first error. The entity reader then sees exactly the bytes Go's reader
//! would have delivered before it failed.

/// A body after inflation: the bytes that came out, and the error the reader
/// met after them. `failure` is `None` when the stream ended cleanly.
#[derive(Debug, Default)]
pub(crate) struct Inflated {
    /// The decompressed bytes that came out before any error.
    pub(crate) output: Vec<u8>,
    /// The text of the error the reader returned after `output`, when there was
    /// one, as Go's error reports it.
    pub(crate) failure: Option<String>,
}

/// The longest window: `maxMatchOffset` in Go's `compress/flate`.
const MAX_MATCH_OFFSET: usize = 1 << 15;

/// The order in which a dynamic block's code-length code lengths are sent.
const CODE_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// How inflation stopped: the errors Go's decompressor returns.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The input ended inside a value (Go's `io.ErrUnexpectedEOF`).
    UnexpectedEof,
    /// The stream is corrupt, at the byte offset Go reports.
    Corrupt(usize),
    /// A state Go's decoder cannot reach.
    Internal(&'static str),
}

impl Failure {
    /// The text Go gives for the error.
    fn text(&self) -> String {
        match self {
            Failure::UnexpectedEof => "unexpected EOF".to_owned(),
            Failure::Corrupt(offset) => format!("flate: corrupt input before offset {offset}"),
            Failure::Internal(what) => format!("flate: internal error: {what}"),
        }
    }
}

/// A canonical Huffman code, as Go's `huffmanDecoder` decodes one.
#[derive(Debug)]
struct Huffman {
    /// The shortest code length, which the decoder reads at once.
    min: u32,
    /// How many codes there are of each length.
    count: [u32; 16],
    /// The symbols, sorted by code length and then by symbol.
    symbol: Vec<usize>,
}

impl Huffman {
    /// Builds the code for `lengths`, one length per symbol. It returns `None`
    /// where Go's `init` does: the code is over-subscribed, or incomplete
    /// (except a single code of length 1).
    fn new(lengths: &[u8]) -> Option<Huffman> {
        let mut count = [0u32; 16];
        let mut min = 0;
        let mut max = 0;
        for &length in lengths {
            if length == 0 {
                continue;
            }
            let length = usize::from(length);
            if min == 0 || length < min {
                min = length;
            }
            max = max.max(length);
            count[length] += 1;
        }
        let min = u32::try_from(min).ok()?;
        if max == 0 {
            return Some(Huffman {
                min: 0,
                count,
                symbol: Vec::new(),
            });
        }
        // Scaled by 2^15: the code is complete when nothing is left over.
        let mut left: i64 = 1;
        for &codes in &count[1..] {
            left = left * 2 - i64::from(codes);
            if left < 0 {
                return None;
            }
        }
        if left != 0 && !(max == 1 && count[1] == 1) {
            return None;
        }
        let mut offset = [0usize; 16];
        for length in 1..15 {
            offset[length + 1] = offset[length] + count[length] as usize;
        }
        let mut symbol = vec![0; offset[15] + count[15] as usize];
        for (value, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let length = usize::from(length);
                symbol[offset[length]] = value;
                offset[length] += 1;
            }
        }
        Some(Huffman { min, count, symbol })
    }

    /// The symbol that starts `bits`, read least significant bit first, and
    /// the length of its code. The bits above the ones read are zero. `None`
    /// when no code matches.
    fn lookup(&self, bits: u32) -> Option<(usize, u32)> {
        let mut code: u32 = 0;
        let mut first: u32 = 0;
        let mut index: usize = 0;
        for length in 1..16u32 {
            code |= (bits >> (length - 1)) & 1;
            let count = self.count[length as usize];
            if code < first + count {
                let offset = (code - first) as usize;
                return Some((self.symbol[index + offset], length));
            }
            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// The bit reader and window of one deflate stream.
struct Inflater<'a> {
    /// The compressed input.
    input: &'a [u8],
    /// The bytes read so far, from the start of the body.
    pos: usize,
    /// Where the deflate stream starts in the body. Go counts its offsets from
    /// here, so a corrupt-input error reports `pos - origin`.
    origin: usize,
    /// The bits read and not yet used, least significant first.
    bits: u32,
    /// How many bits are in `bits`.
    nb: u32,
    /// Everything decompressed so far, across the members of a gzip stream.
    output: Vec<u8>,
    /// Where the current stream's output starts. A stream's history starts
    /// there, since Go resets its window for each member.
    start: usize,
}

impl<'a> Inflater<'a> {
    fn new(input: &'a [u8], pos: usize, output: Vec<u8>) -> Self {
        Inflater {
            input,
            pos,
            origin: pos,
            bits: 0,
            nb: 0,
            start: output.len(),
            output,
        }
    }

    /// The offset Go reports for a corrupt stream: the bytes of the deflate data
    /// read so far.
    fn offset(&self) -> usize {
        self.pos - self.origin
    }

    /// Reads one byte into the bit buffer: Go's `moreBits`.
    fn read_byte(&mut self) -> Result<(), Failure> {
        let byte = *self.input.get(self.pos).ok_or(Failure::UnexpectedEof)?;
        self.pos += 1;
        self.bits |= u32::from(byte) << self.nb;
        self.nb += 8;
        Ok(())
    }

    /// Reads bytes until `count` bits are buffered.
    fn need(&mut self, count: u32) -> Result<(), Failure> {
        while self.nb < count {
            self.read_byte()?;
        }
        Ok(())
    }

    /// Uses the next `count` buffered bits.
    fn consume(&mut self, count: u32) {
        self.bits >>= count;
        self.nb -= count;
    }

    /// Reads the bits of an integer of `count` bits and uses them.
    fn take(&mut self, count: u32) -> Result<u32, Failure> {
        self.need(count)?;
        let value = self.bits & ((1 << count) - 1);
        self.consume(count);
        Ok(value)
    }

    /// Reads one symbol with `code`: Go's `huffSym`. The loop reads as Go's
    /// does, so the byte count at an error is the same.
    fn symbol(&mut self, code: &Huffman) -> Result<usize, Failure> {
        let mut want = code.min;
        loop {
            self.need(want)?;
            match code.lookup(self.bits) {
                None => return Err(Failure::Corrupt(self.offset())),
                Some((symbol, length)) if length <= self.nb => {
                    self.consume(length);
                    return Ok(symbol);
                }
                Some((_, length)) => want = length,
            }
        }
    }

    /// Inflates the blocks of one stream, up to its final block.
    fn blocks(&mut self) -> Result<(), Failure> {
        loop {
            self.need(3)?;
            let last = self.bits & 1 == 1;
            let kind = (self.bits >> 1) & 3;
            self.consume(3);
            match kind {
                0 => self.stored()?,
                1 => self.codes(&fixed_literal(), None)?,
                2 => {
                    let (literal, distance) = self.dynamic()?;
                    self.codes(&literal, Some(&distance))?;
                }
                _ => return Err(Failure::Corrupt(self.offset())),
            }
            if last {
                return Ok(());
            }
        }
    }

    /// A stored block: its length, its complement, and its bytes.
    fn stored(&mut self) -> Result<(), Failure> {
        self.bits = 0;
        self.nb = 0;
        let head = self.input.get(self.pos..).unwrap_or_default();
        let read = head.len().min(4);
        let head = &head[..read];
        self.pos += read;
        if read < 4 {
            return Err(Failure::UnexpectedEof);
        }
        let length = usize::from(u16::from_le_bytes([head[0], head[1]]));
        let complement = u16::from_le_bytes([head[2], head[3]]);
        if complement != !(length as u16) {
            return Err(Failure::Corrupt(self.offset()));
        }
        if length == 0 {
            return Ok(());
        }
        let data = self.input.get(self.pos..).unwrap_or_default();
        let copied = data.len().min(length);
        self.output.extend_from_slice(&data[..copied]);
        self.pos += copied;
        if copied < length {
            return Err(Failure::UnexpectedEof);
        }
        Ok(())
    }

    /// The header of a dynamic block: its two codes.
    fn dynamic(&mut self) -> Result<(Huffman, Huffman), Failure> {
        self.need(14)?;
        let literal_count = (self.bits & 0x1f) as usize + 257;
        if literal_count > 286 {
            return Err(Failure::Corrupt(self.offset()));
        }
        self.consume(5);
        let distance_count = (self.bits & 0x1f) as usize + 1;
        if distance_count > 30 {
            return Err(Failure::Corrupt(self.offset()));
        }
        self.consume(5);
        let code_length_count = (self.bits & 0xf) as usize + 4;
        self.consume(4);

        let mut code_bits = [0u8; 19];
        for &index in CODE_ORDER.iter().take(code_length_count) {
            code_bits[index] = self.take(3)? as u8;
        }
        let code_lengths = Huffman::new(&code_bits).ok_or(Failure::Corrupt(self.offset()))?;

        let total = literal_count + distance_count;
        let mut lengths = vec![0u8; total];
        let mut index = 0;
        while index < total {
            let symbol = self.symbol(&code_lengths)?;
            if symbol < 16 {
                lengths[index] = symbol as u8;
                index += 1;
                continue;
            }
            let (mut repeat, extra_bits, value) = match symbol {
                16 => {
                    if index == 0 {
                        return Err(Failure::Corrupt(self.offset()));
                    }
                    (3, 2, lengths[index - 1])
                }
                17 => (3, 3, 0),
                18 => (11, 7, 0),
                _ => return Err(Failure::Internal("unexpected length code")),
            };
            repeat += self.take(extra_bits)? as usize;
            if index + repeat > total {
                return Err(Failure::Corrupt(self.offset()));
            }
            for _ in 0..repeat {
                lengths[index] = value;
                index += 1;
            }
        }

        let mut literal =
            Huffman::new(&lengths[..literal_count]).ok_or(Failure::Corrupt(self.offset()))?;
        let distance =
            Huffman::new(&lengths[literal_count..]).ok_or(Failure::Corrupt(self.offset()))?;
        // Every block ends with the end-of-block code, so the reader may read
        // at least that many bits at a time without passing the stream's end.
        let end_of_block = u32::from(lengths[256]);
        if literal.min < end_of_block {
            literal.min = end_of_block;
        }
        Ok((literal, distance))
    }

    /// The symbols of a compressed block, up to its end-of-block code.
    fn codes(&mut self, literal: &Huffman, distance: Option<&Huffman>) -> Result<(), Failure> {
        loop {
            let value = self.symbol(literal)?;
            if value < 256 {
                self.output.push(value as u8);
                continue;
            }
            if value == 256 {
                return Ok(());
            }
            let (mut length, extra_bits) = match value {
                257..=264 => (value - 254, 0),
                265..=268 => (value * 2 - (265 * 2 - 11), 1),
                269..=272 => (value * 4 - (269 * 4 - 19), 2),
                273..=276 => (value * 8 - (273 * 8 - 35), 3),
                277..=280 => (value * 16 - (277 * 16 - 67), 4),
                281..=284 => (value * 32 - (281 * 32 - 131), 5),
                285 => (258, 0),
                _ => return Err(Failure::Corrupt(self.offset())),
            };
            length += self.take(extra_bits)? as usize;

            let code = match distance {
                None => {
                    // A fixed block's distance code is five bits, sent most
                    // significant bit first.
                    self.need(5)?;
                    let code = (self.bits & 0x1f).reverse_bits() >> 27;
                    self.consume(5);
                    code as usize
                }
                Some(code) => self.symbol(code)?,
            };
            let distance = match code {
                0..=3 => code + 1,
                4..=29 => {
                    let extra_bits = ((code - 2) >> 1) as u32;
                    let extra = (code & 1) << extra_bits | self.take(extra_bits)? as usize;
                    (1 << (extra_bits + 1)) + 1 + extra
                }
                _ => return Err(Failure::Corrupt(self.offset())),
            };
            let history = (self.output.len() - self.start).min(MAX_MATCH_OFFSET);
            if distance > history {
                return Err(Failure::Corrupt(self.offset()));
            }
            for _ in 0..length {
                let byte = self.output[self.output.len() - distance];
                self.output.push(byte);
            }
        }
    }
}

/// The code of a fixed block's literals and lengths (RFC 1951, section 3.2.6).
fn fixed_literal() -> Huffman {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    Huffman::new(&lengths).expect("the fixed code is complete")
}

/// Inflates the blocks of one deflate stream that starts at `pos`, into
/// `output`. The result is the inflater, so that its position is the byte
/// after the stream, and the error when the stream did not end cleanly.
fn inflate(input: &[u8], pos: usize, output: Vec<u8>) -> (Inflater<'_>, Option<Failure>) {
    let mut inflater = Inflater::new(input, pos, output);
    let failure = inflater.blocks().err();
    (inflater, failure)
}

/// The CRC-32 of `data`, updated from `crc` (IEEE polynomial).
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The Adler-32 checksum of `data`.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// The `gzip` body, as go-restful inflates it: each member's header, its
/// deflate stream, and its trailer. A body that is not gzip fails at its
/// header, with Go's text for it.
pub(crate) fn gzip(body: &[u8]) -> Inflated {
    let mut output = Vec::new();
    let mut pos = 0;
    loop {
        match gzip_header(body, pos) {
            Ok(next) => pos = next,
            // The end of the body, between members, is a clean end.
            Err(None) => {
                return Inflated {
                    output,
                    failure: None,
                };
            }
            Err(Some(text)) => {
                return Inflated {
                    output,
                    failure: Some(text),
                };
            }
        }
        let start = output.len();
        let (inflater, failure) = inflate(body, pos, output);
        let end = inflater.pos;
        output = inflater.output;
        if let Some(failure) = failure {
            return Inflated {
                output,
                failure: Some(failure.text()),
            };
        }
        let member = &output[start..];
        let Some(trailer) = body.get(end..end + 8) else {
            return Inflated {
                output,
                failure: Some("unexpected EOF".to_owned()),
            };
        };
        let digest = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
        let size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
        if digest != crc32_update(0, member) || size != member.len() as u32 {
            return Inflated {
                output,
                failure: Some("gzip: invalid checksum".to_owned()),
            };
        }
        pos = end + 8;
    }
}

/// Reads a gzip member's header at `pos` and returns the position after it. The
/// error is `None` where the body ends before a header, which is a clean end,
/// and the text of the error otherwise.
fn gzip_header(body: &[u8], pos: usize) -> Result<usize, Option<String>> {
    let rest = body.get(pos..).unwrap_or_default();
    if rest.is_empty() {
        return Err(None);
    }
    let unexpected = || Some("unexpected EOF".to_owned());
    let Some(fixed) = rest.get(..10) else {
        return Err(unexpected());
    };
    if fixed[0] != 0x1f || fixed[1] != 0x8b || fixed[2] != 8 {
        return Err(Some("gzip: invalid header".to_owned()));
    }
    let flags = fixed[3];
    let mut at = pos + 10;
    let mut digest = crc32_update(0, fixed);
    if flags & 4 != 0 {
        let Some(length) = body.get(at..at + 2) else {
            return Err(unexpected());
        };
        let count = usize::from(u16::from_le_bytes([length[0], length[1]]));
        if body.len() < at + 2 + count {
            return Err(unexpected());
        }
        digest = crc32_update(digest, &body[at..at + 2 + count]);
        at += 2 + count;
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            // A name or a comment: Latin-1 text up to a NUL, at most 512 bytes.
            let Some(end) = body[at..].iter().take(512).position(|&byte| byte == 0) else {
                return Err(if body.len() - at >= 512 {
                    Some("gzip: invalid header".to_owned())
                } else {
                    unexpected()
                });
            };
            digest = crc32_update(digest, &body[at..at + end + 1]);
            at += end + 1;
        }
    }
    if flags & 2 != 0 {
        let Some(stored) = body.get(at..at + 2) else {
            return Err(unexpected());
        };
        if u16::from_le_bytes([stored[0], stored[1]]) != digest as u16 {
            return Err(Some("gzip: invalid header".to_owned()));
        }
        at += 2;
    }
    Ok(at)
}

/// The `deflate` body, as go-restful inflates it: a zlib stream. Its header is
/// read before the body, so a bad header is the error of the whole request.
pub(crate) fn zlib(body: &[u8]) -> Result<Inflated, String> {
    let Some(header) = body.get(..2) else {
        return Err("unexpected EOF".to_owned());
    };
    let (method, info) = (header[0] & 0x0f, header[0] >> 4);
    if method != 8 || info > 7 || u16::from_be_bytes([header[0], header[1]]) % 31 != 0 {
        return Err("zlib: invalid header".to_owned());
    }
    let mut pos = 2;
    if header[1] & 0x20 != 0 {
        let Some(dictionary) = body.get(2..6) else {
            return Err("unexpected EOF".to_owned());
        };
        // The check value of the empty dictionary, which go-restful gives.
        if u32::from_be_bytes([dictionary[0], dictionary[1], dictionary[2], dictionary[3]]) != 1 {
            return Err("zlib: invalid dictionary".to_owned());
        }
        pos = 6;
    }
    let (inflater, failure) = inflate(body, pos, Vec::new());
    let output = inflater.output;
    if let Some(failure) = failure {
        return Ok(Inflated {
            output,
            failure: Some(failure.text()),
        });
    }
    let Some(check) = body.get(inflater.pos..inflater.pos + 4) else {
        return Ok(Inflated {
            output,
            failure: Some("unexpected EOF".to_owned()),
        });
    };
    if u32::from_be_bytes([check[0], check[1], check[2], check[3]]) != adler32(&output) {
        return Ok(Inflated {
            output,
            failure: Some("zlib: invalid checksum".to_owned()),
        });
    }
    Ok(Inflated {
        output,
        failure: None,
    })
}
