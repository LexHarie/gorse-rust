// Ports: none. The gRPC codec of the generated services (O-93). The bytes are Go's protobuf bytes.

//! The gRPC codec of the generated services: prost's codec, with one difference.
//!
//! prost writes a proto3 float only when it is not `0.0`, so it leaves out a score of
//! `-0.0`. Go's protobuf writes a float whenever its bits are not zero, so the Go master
//! sends `-0.0`, the score of an item's own neighbour in `QueryVectors`, and the Rust
//! master must send the same bytes. [`GoWireCodec`] writes the `score` of a
//! `ScoredVector` by Go's rule. Every other message is written by prost.

use std::marker::PhantomData;

use prost::Message;
use prost::bytes::BufMut;
use prost::encoding::{WireType, encode_key, encode_varint};
use tonic::Status;
use tonic::codec::{Codec, EncodeBuf, Encoder};
use tonic_prost::ProstDecoder;

use crate::QueryVectorsResponse;

/// The codec the generated services use: prost's decoder, and an encoder that writes
/// Go's bytes for a `QueryVectorsResponse` (see the module documentation).
#[derive(Debug)]
pub struct GoWireCodec<E, D>(PhantomData<fn() -> (E, D)>);

impl<E, D> Default for GoWireCodec<E, D> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E, D> Codec for GoWireCodec<E, D>
where
    E: Message + Send + 'static,
    D: Message + Default + Send + 'static,
{
    type Encode = E;
    type Decode = D;
    type Encoder = GoWireEncoder<E>;
    type Decoder = ProstDecoder<D>;

    fn encoder(&mut self) -> Self::Encoder {
        GoWireEncoder(PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        ProstDecoder::default()
    }
}

/// The encoder of [`GoWireCodec`].
#[derive(Debug)]
pub struct GoWireEncoder<E>(PhantomData<fn() -> E>);

impl<E: Message + Send + 'static> Encoder for GoWireEncoder<E> {
    type Item = E;
    type Error = Status;

    fn encode(&mut self, item: E, buf: &mut EncodeBuf<'_>) -> Result<(), Status> {
        match (&item as &dyn std::any::Any).downcast_ref::<QueryVectorsResponse>() {
            Some(response) => encode_query_vectors(response, buf),
            None => item
                .encode(buf)
                .expect("Message only errors if not enough space"),
        }
        Ok(())
    }
}

/// Writes a `QueryVectorsResponse` as Go's protobuf writes it. Each `ScoredVector` is field 1
/// of the response, and inside it the vector is field 1 and the score is field 2. The score is
/// written when its bits are not zero, so `-0.0` is written and `+0.0` is left out.
fn encode_query_vectors(response: &QueryVectorsResponse, buf: &mut impl BufMut) {
    for scored in &response.vectors {
        let mut message = Vec::new();
        if let Some(vector) = &scored.vector {
            encode_key(1, WireType::LengthDelimited, &mut message);
            encode_varint(vector.encoded_len() as u64, &mut message);
            vector
                .encode(&mut message)
                .expect("a vector grows the buffer");
        }
        if scored.score.to_bits() != 0 {
            encode_key(2, WireType::ThirtyTwoBit, &mut message);
            message.put_f32_le(scored.score);
        }
        encode_key(1, WireType::LengthDelimited, buf);
        encode_varint(message.len() as u64, buf);
        buf.put_slice(&message);
    }
}

/// The bytes Go's protobuf writes for `response`, for the tests.
#[cfg(test)]
fn go_bytes(response: &QueryVectorsResponse) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_query_vectors(response, &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use prost::bytes::Bytes;

    use super::*;
    use crate::{ScoredVector, Vector};

    /// The response of Go's `protocol.QueryVectorsResponse` with one `ScoredVector` whose
    /// vector has id `1` and whose score is `score`.
    fn one_score(score: f32) -> QueryVectorsResponse {
        QueryVectorsResponse {
            vectors: vec![ScoredVector {
                vector: Some(Vector {
                    id: "1".to_owned(),
                    ..Default::default()
                }),
                score,
            }],
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Go's `proto.Marshal` of the response with score `-0.0` (bits `0x80000000`). Go
    /// writes the float because its bits are not zero. prost leaves it out.
    #[test]
    fn negative_zero_score_is_written_as_go_writes_it() {
        assert_eq!(hex(&go_bytes(&one_score(-0.0))), "0a0a0a030a01311500000080");
    }

    /// Go leaves out `+0.0`, whose bits are zero.
    #[test]
    fn positive_zero_score_is_left_out_as_go_leaves_it() {
        assert_eq!(hex(&go_bytes(&one_score(0.0))), "0a050a030a0131");
    }

    /// A non-zero score is written by Go as its fixed32 bits.
    #[test]
    fn non_zero_score_is_written_as_go_writes_it() {
        assert_eq!(hex(&go_bytes(&one_score(0.5))), "0a0a0a030a0131150000003f");
    }

    /// The `-0.0` that Go writes decodes to `-0.0` on the Rust side.
    #[test]
    fn negative_zero_score_decodes_to_negative_zero() {
        let bytes = Bytes::from(hex_to_bytes("0a0a0a030a01311500000080"));
        let decoded = QueryVectorsResponse::decode(bytes).expect("valid message");
        assert_eq!(decoded.vectors[0].score.to_bits(), 0x8000_0000);
    }

    fn hex_to_bytes(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
            .collect()
    }
}
