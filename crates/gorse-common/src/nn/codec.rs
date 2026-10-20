// Ports: github.com/matttproud/golang_protobuf_extensions/pbutil (ReadDelimited, WriteDelimited) @ v1.0.4
// Ports: google.golang.org/protobuf internal/impl/decode.go, codec_gen.go, encoding/protowire/wire.go (protocol.Tensor) @ v1.36.11
//
// Derived from Go code. pbutil: Copyright 2013 Matt T. Proud, Apache License
// 2.0. protobuf-go: Copyright 2018 The Go Authors, BSD-style license in
// crates/gorse-common/licenses/GO-LICENSE.

//! The length-delimited `protocol.Tensor` records of [`save`](super::save)
//! and [`load`](super::load), written and read as Go's `pbutil` and
//! `proto.Marshal` and `proto.Unmarshal` do, error texts included.
//!
//! The message is `protocol.Tensor` of `protocol/encoding.proto`:
//! `repeated string key = 1; repeated int32 shape = 2; repeated float data
//! = 3;`. Encoding it by hand keeps the error texts of Go's decoder, which
//! callers see; the tests check it against the generated
//! `gorse_protocol::Tensor`.

use std::io::{self, Read, Write};

use super::layers::Error;

/// A decoded `protocol.Tensor`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Message {
    pub(crate) key: Vec<String>,
    pub(crate) shape: Vec<i32>,
    pub(crate) data: Vec<f32>,
}

const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const BYTES: u64 = 2;
const START_GROUP: u64 = 3;
const END_GROUP: u64 = 4;
const FIXED32: u64 = 5;

/// `protowire.MaxValidNumber`.
const MAX_VALID_NUMBER: u64 = (1 << 29) - 1;

/// `protowire.DefaultRecursionLimit`.
const RECURSION_LIMIT: i32 = 10_000;

fn append_varint(b: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        b.push((v as u8) | 0x80);
        v >>= 7;
    }
    b.push(v as u8);
}

impl Message {
    /// `proto.Marshal`: the fields in order of number, packed repeated
    /// numbers, empty fields left out.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        for key in &self.key {
            b.push(0x0a);
            append_varint(&mut b, key.len() as u64);
            b.extend_from_slice(key.as_bytes());
        }
        if !self.shape.is_empty() {
            let mut packed = Vec::new();
            for &d in &self.shape {
                // An int32 is sign-extended to 64 bits.
                append_varint(&mut packed, i64::from(d).cast_unsigned());
            }
            b.push(0x12);
            append_varint(&mut b, packed.len() as u64);
            b.extend_from_slice(&packed);
        }
        if !self.data.is_empty() {
            b.push(0x1a);
            append_varint(&mut b, 4 * self.data.len() as u64);
            for &x in &self.data {
                b.extend_from_slice(&x.to_bits().to_le_bytes());
            }
        }
        b
    }

    /// `proto.Unmarshal` into a new `protocol.Tensor`.
    pub(crate) fn decode(mut b: &[u8]) -> Result<Message, Error> {
        let mut m = Message::default();
        while !b.is_empty() {
            let (tag, n) = consume_varint(b).ok_or(Error::InvalidWireFormat)?;
            b = &b[n..];
            let num = tag >> 3;
            if !(1..=MAX_VALID_NUMBER).contains(&num) {
                return Err(Error::InvalidWireFormat);
            }
            let typ = tag & 7;
            if typ == END_GROUP {
                // No group is open in a message at the top level.
                return Err(Error::InvalidWireFormat);
            }
            let n = match (num, typ) {
                (1, BYTES) => {
                    let (v, n) = consume_bytes(b).ok_or(Error::InvalidWireFormat)?;
                    let key = std::str::from_utf8(v).map_err(|_| Error::InvalidUtf8)?;
                    m.key.push(key.to_owned());
                    n
                }
                (2, BYTES) => {
                    let (mut v, n) = consume_bytes(b).ok_or(Error::InvalidWireFormat)?;
                    while !v.is_empty() {
                        let (x, k) = consume_varint(v).ok_or(Error::InvalidWireFormat)?;
                        m.shape.push(x as i32);
                        v = &v[k..];
                    }
                    n
                }
                (2, VARINT) => {
                    let (x, n) = consume_varint(b).ok_or(Error::InvalidWireFormat)?;
                    m.shape.push(x as i32);
                    n
                }
                (3, BYTES) => {
                    let (v, n) = consume_bytes(b).ok_or(Error::InvalidWireFormat)?;
                    let chunks = v.chunks_exact(4);
                    if !chunks.remainder().is_empty() {
                        return Err(Error::InvalidWireFormat);
                    }
                    m.data.extend(chunks.map(f32_of));
                    n
                }
                (3, FIXED32) => {
                    let v = b.get(..4).ok_or(Error::InvalidWireFormat)?;
                    m.data.push(f32_of(v));
                    4
                }
                // An unknown field, or a known one of another wire type.
                _ => consume_field_value(num, typ, b, RECURSION_LIMIT)
                    .ok_or(Error::InvalidWireFormat)?,
            };
            b = &b[n..];
        }
        Ok(m)
    }
}

fn f32_of(b: &[u8]) -> f32 {
    f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `protowire.ConsumeVarint`: at most ten bytes, the tenth at most 1.
fn consume_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &byte) in b.iter().enumerate().take(10) {
        if i == 9 {
            return (byte < 2).then(|| (v | (u64::from(byte) << 63), 10));
        }
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte < 0x80 {
            return Some((v, i + 1));
        }
    }
    None
}

/// `protowire.ConsumeBytes`: a length and that many bytes.
fn consume_bytes(b: &[u8]) -> Option<(&[u8], usize)> {
    let (len, n) = consume_varint(b)?;
    let len = usize::try_from(len).ok()?;
    let v = b.get(n..)?.get(..len)?;
    Some((v, n + len))
}

/// `protowire.ConsumeFieldValue`: the length of a field value to skip.
fn consume_field_value(num: u64, typ: u64, b: &[u8], depth: i32) -> Option<usize> {
    match typ {
        VARINT => consume_varint(b).map(|(_, n)| n),
        FIXED32 => (b.len() >= 4).then_some(4),
        FIXED64 => (b.len() >= 8).then_some(8),
        BYTES => consume_bytes(b).map(|(_, n)| n),
        START_GROUP => {
            if depth < 0 {
                return None;
            }
            let mut rest = b;
            loop {
                // `ConsumeTag`: a field number of at least 1 that fits an
                // int32.
                let (tag, n) = consume_varint(rest)?;
                let num2 = tag >> 3;
                if num2 < 1 || num2 > i32::MAX as u64 {
                    return None;
                }
                rest = &rest[n..];
                let typ2 = tag & 7;
                if typ2 == END_GROUP {
                    return (num2 == num).then_some(b.len() - rest.len());
                }
                let n = consume_field_value(num2, typ2, rest, depth - 1)?;
                rest = &rest[n..];
            }
        }
        _ => None,
    }
}

/// `pbutil.WriteDelimited`: the length of the message as a varint, then the
/// message.
pub(crate) fn write_delimited(w: &mut dyn Write, m: &Message) -> io::Result<()> {
    let body = m.encode();
    let mut header = Vec::with_capacity(5);
    append_varint(&mut header, body.len() as u64);
    w.write_all(&header)?;
    w.write_all(&body)
}

/// Why [`read_delimited`] read no message.
pub(crate) enum ReadError {
    /// The stream ended before the message began, or after only part of its
    /// length: Go's `io.EOF`, which ends `Load` without an error.
    Eof,
    /// Any other error.
    Error(Error),
}

impl From<Error> for ReadError {
    fn from(e: Error) -> ReadError {
        ReadError::Error(e)
    }
}

/// Reads one byte, retrying an interrupted read; `None` at the end of the
/// stream.
fn read_byte(r: &mut dyn Read) -> Result<Option<u8>, ReadError> {
    let mut byte = [0u8];
    loop {
        match r.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(byte[0])),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e).into()),
        }
    }
}

/// `pbutil.ReadDelimited`: a varint length of at most five bytes, read a
/// byte at a time so as not to read past the message, then the message.
///
/// The message is read as it arrives, not into a buffer of the stated
/// length first as Go allocates it, so a length far beyond the stream fails
/// with the same `unexpected EOF` without the allocation.
pub(crate) fn read_delimited(r: &mut dyn Read) -> Result<Message, ReadError> {
    let mut length = 0u64;
    let mut header_bytes = 0;
    loop {
        if header_bytes >= 5 {
            return Err(Error::InvalidVarint.into());
        }
        let Some(byte) = read_byte(r)? else {
            return Err(ReadError::Eof);
        };
        length |= u64::from(byte & 0x7f) << (7 * header_bytes);
        header_bytes += 1;
        if byte < 0x80 {
            break;
        }
    }
    let mut body = Vec::new();
    let read = r.take(length).read_to_end(&mut body).map_err(Error::Io)?;
    if (read as u64) < length {
        // `io.ReadFull`: io.EOF for nothing read, else io.ErrUnexpectedEOF.
        return Err(if read == 0 {
            ReadError::Eof
        } else {
            Error::UnexpectedEof.into()
        });
    }
    Ok(Message::decode(&body)?)
}

#[cfg(test)]
mod tests {
    use prost::Message as _;

    use super::*;

    /// splitmix64, for random messages.
    struct Seq(u64);

    impl Seq {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> usize {
            usize::try_from(self.next() % n).expect("small")
        }
    }

    fn random_message(s: &mut Seq) -> Message {
        let key = (0..s.below(4))
            .map(|_| {
                (0..s.below(6))
                    .map(|_| char::from(b'A' + s.below(26) as u8))
                    .collect()
            })
            .collect();
        let shape = (0..s.below(4))
            .map(|_| s.next() as i32 >> s.below(32))
            .collect();
        let data = (0..s.below(9))
            .map(|_| f32::from_bits(s.next() as u32))
            .collect();
        Message { key, shape, data }
    }

    fn to_prost(m: &Message) -> gorse_protocol::Tensor {
        gorse_protocol::Tensor {
            key: m.key.clone(),
            shape: m.shape.clone(),
            data: m.data.clone(),
        }
    }

    fn bits(m: &Message) -> (Vec<String>, Vec<i32>, Vec<u32>) {
        (
            m.key.clone(),
            m.shape.clone(),
            m.data.iter().map(|x| x.to_bits()).collect(),
        )
    }

    #[test]
    fn nn_codec_encodes_as_the_generated_message() {
        let mut s = Seq(1);
        for _ in 0..500 {
            let m = random_message(&mut s);
            assert_eq!(m.encode(), to_prost(&m).encode_to_vec(), "{m:?}");
            let back = Message::decode(&m.encode()).expect("decodes");
            assert_eq!(bits(&back), bits(&m));
        }
    }

    #[test]
    fn nn_codec_decodes_as_the_generated_message() {
        // Unpacked repeated numbers, and unknown fields of every wire type,
        // a group among them, which are skipped.
        let mut b = vec![0x0a, 0x01, b'k'];
        b.extend([
            0x10, 0x05, 0x10, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ]);
        b.extend([0x1d, 0x00, 0x00, 0x80, 0x3f]);
        b.extend([
            0x20, 0x07, 0x29, 1, 2, 3, 4, 5, 6, 7, 8, 0x32, 0x00, 0x3d, 1, 2, 3, 4,
        ]);
        b.extend([0x43, 0x08, 0x01, 0x44]);
        b.extend([0x12, 0x02, 0x03, 0x04, 0x1a, 0x04, 0x00, 0x00, 0x00, 0x40]);
        let ours = Message::decode(&b).expect("decodes");
        let theirs = gorse_protocol::Tensor::decode(b.as_slice()).expect("prost decodes");
        assert_eq!(ours.key, theirs.key);
        assert_eq!(ours.shape, theirs.shape);
        assert_eq!(ours.shape, [5, -2, 3, 4]);
        assert_eq!(ours.data, theirs.data);
        assert_eq!(ours.data, [1.0, 2.0]);
    }

    #[test]
    fn nn_codec_skips_known_fields_of_another_wire_type() {
        // protobuf-go's field decoders return errUnknown for a wire type
        // they do not take, and the field is skipped as unknown (prost
        // rejects it instead).
        let b = [0x08, 0x01, 0x15, 0, 0, 0, 0, 0x18, 0x02, 0x0a, 0x00];
        let m = Message::decode(&b).expect("decodes");
        assert_eq!(
            m,
            Message {
                key: vec![String::new()],
                shape: vec![],
                data: vec![]
            }
        );
    }

    #[test]
    fn nn_codec_errors_as_protobuf_go() {
        let invalid = |b: &[u8]| matches!(Message::decode(b), Err(Error::InvalidWireFormat));
        assert!(invalid(&[0x0a, 0xff]));
        assert!(invalid(&[0x0a, 0x02, b'a']));
        assert!(invalid(&[0x00, 0x00]), "field number 0");
        assert!(invalid(&[0x0c]), "an end group outside a group");
        assert!(invalid(&[0x0e]), "wire type 6");
        assert!(invalid(&[0x1a, 0x03, 0, 0, 0]), "packed floats of 3 bytes");
        assert!(invalid(&[0x12, 0x01, 0x80]), "a truncated packed varint");
        assert!(invalid(&[0x43, 0x4c]), "a group closed by another number");
        assert!(invalid(&[0x43]), "an unclosed group");
        assert!(
            invalid(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02]),
            "varint overflow"
        );
        assert!(
            invalid(&[0x80, 0x80, 0x80, 0x80, 0x10, 0x00]),
            "field number 2^29"
        );
        assert!(matches!(
            Message::decode(&[0x0a, 0x01, 0xff]),
            Err(Error::InvalidUtf8)
        ));
        let ok = |b: &[u8]| Message::decode(b).is_ok();
        assert!(
            ok(&[0xf8, 0xff, 0xff, 0xff, 0x0f, 0x00]),
            "field number 2^29 - 1"
        );
        assert!(
            ok(&[0xfb, 0xff, 0xff, 0xff, 0x0f, 0xfc, 0xff, 0xff, 0xff, 0x0f]),
            "an empty group 2^29 - 1"
        );
    }
}
