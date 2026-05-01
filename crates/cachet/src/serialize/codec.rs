// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.
//!
//! These codecs serialize values into [`BytesView`] using pool-backed memory,
//! avoiding heap allocations. The serialized bytes can then flow through
//! compression, encryption, and network I/O without additional copies.

use std::borrow::Cow;
use std::io::Write;

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};

use crate::transform::DecodeOutcome;
use crate::{Codec, Encoder, Error};

use serde::{Serialize, de::DeserializeOwned};

const FORMAT_VERSION: u8 = 1;

/// An encoder that serializes values to [`BytesView`] using postcard (one-directional).
///
/// Implements `Encoder<T, BytesView>` for any `T: Serialize + Send + Sync`.
///
/// For bidirectional serialization/deserialization, use [`PostcardCodec`].
#[derive(Debug, Clone)]
pub struct PostcardEncoder;

/// A bidirectional codec that serializes and deserializes values using postcard.
///
/// Implements `Codec<T, BytesView>` for any `T: Serialize + DeserializeOwned + Send + Sync`.
///
/// Serialization writes directly into pool-backed memory via [`BytesBufWriter`](bytesbuf::BytesBufWriter),
/// producing a [`BytesView`] with no intermediate heap allocation.
#[derive(Debug, Clone)]
pub struct PostcardCodec;

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for PostcardEncoder {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for PostcardCodec {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

fn encode<T: Serialize + Send + Sync>(value: &T) -> Result<BytesView, Error> {
    // TODO make Cache thread aware so we can simply store the pool in the encoder
    // Until then we need this to avoid creating a new pool for every encode call, which would be
    // very expensive
    thread_local! {
        static POOL: GlobalPool = GlobalPool::new();
    }
    let pool = POOL.with(GlobalPool::clone);

    let mut writer = BytesBuf::new().into_writer(pool);
    writer.write_all(&[FORMAT_VERSION]).map_err(Error::from_source)?;
    postcard::to_io(value, &mut writer).map_err(Error::from_source)?;
    Ok(writer.into_inner().peek())
}

impl<T: Serialize + DeserializeOwned + Send + Sync> Codec<T, BytesView> for PostcardCodec {
    /// Decodes a stored value back to the original type.
    ///
    /// Returns `DecodeOutcome::Value(v)` on success, or
    /// `DecodeOutcome::SoftFailure(reason)` if the stored data is undecodable
    /// and should be treated as a cache miss.
    fn decode(&self, value: BytesView) -> Result<DecodeOutcome<T>, Error> {
        let bytes = to_contiguous(&value);
        let Some((version, payload)) = bytes.split_first() else {
            return Ok(DecodeOutcome::SoftFailure("empty payload"));
        };

        if *version != FORMAT_VERSION {
            return Ok(DecodeOutcome::SoftFailure("format version mismatch"));
        }

        match postcard::from_bytes(payload) {
            Ok(value) => Ok(DecodeOutcome::Value(value)),
            Err(_) => Ok(DecodeOutcome::SoftFailure("deserialization failed")),
        }
    }
}

/// Returns a contiguous byte slice from a [`BytesView`]. Zero-copy for single-span
/// views (the common case), collects into a Vec only for multi-span views.
fn to_contiguous(view: &BytesView) -> Cow<'_, [u8]> {
    let first = view.first_slice();
    if first.len() == view.len() {
        Cow::Borrowed(first)
    } else {
        let mut buf = Vec::with_capacity(view.len());
        for (slice, _) in view.slices() {
            buf.extend_from_slice(slice);
        }
        Cow::Owned(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type whose Serialize impl always fails, used to test the encode error path.
    struct FailSerialize;

    impl Serialize for FailSerialize {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional failure"))
        }
    }

    #[test]
    fn encode_serialization_failure_returns_err() {
        let result = encode(&FailSerialize);
        assert!(result.is_err(), "encode should propagate serialization errors");
    }

    #[test]
    fn decode_empty_payload_returns_soft_failure() {
        let codec = PostcardCodec;
        let empty = BytesView::from(Vec::<u8>::new());
        let result: Result<DecodeOutcome<String>, Error> = codec.decode(empty);
        assert!(matches!(result.unwrap(), DecodeOutcome::SoftFailure("empty payload")));
    }

    #[test]
    fn decode_wrong_format_version_returns_soft_failure() {
        let codec = PostcardCodec;
        let mut data = vec![0xFF];
        data.extend_from_slice(&postcard::to_allocvec(&"hello".to_string()).unwrap());
        let view = BytesView::from(data);
        let result: Result<DecodeOutcome<String>, Error> = codec.decode(view);
        assert!(matches!(result.unwrap(), DecodeOutcome::SoftFailure("format version mismatch")));
    }

    #[test]
    fn decode_corrupt_payload_returns_soft_failure() {
        let codec = PostcardCodec;
        let data = vec![FORMAT_VERSION, 0xFF, 0xFE, 0xFD];
        let view = BytesView::from(data);
        let result: Result<DecodeOutcome<String>, Error> = codec.decode(view);
        assert!(matches!(result.unwrap(), DecodeOutcome::SoftFailure("deserialization failed")));
    }

    #[test]
    fn encode_decode_roundtrip() {
        let codec = PostcardCodec;
        let original = "hello, world!".to_string();
        let encoded = codec.encode(&original).expect("encode should succeed");
        let DecodeOutcome::<String>::Value(decoded) = codec.decode(encoded).expect("decode should succeed") else {
            panic!("expected Value, got SoftFailure");
        };
        assert_eq!(decoded, original);
    }

    #[test]
    fn encoder_encode_produces_valid_output() {
        let value = 42u32;
        let encoded = PostcardEncoder.encode(&value).expect("encode should succeed");
        let bytes = to_contiguous(&encoded);
        assert_eq!(bytes[0], FORMAT_VERSION, "first byte should be format version");
        let decoded: u32 = postcard::from_bytes(&bytes[1..]).expect("postcard decode should succeed");
        assert_eq!(decoded, value);
    }

    #[test]
    fn decode_multi_span_view() {
        let codec = PostcardCodec;
        let original = "multi-span test".to_string();
        let encoded = codec.encode(&original).expect("encode should succeed");

        let bytes = to_contiguous(&encoded);
        let mid = bytes.len() / 2;
        let mut first_half = BytesView::from(bytes[..mid].to_vec());
        let second_half = BytesView::from(bytes[mid..].to_vec());
        first_half.append(second_half);

        assert_ne!(first_half.first_slice().len(), first_half.len(), "should be multi-span");

        let DecodeOutcome::<String>::Value(decoded) = codec.decode(first_half).expect("decode should succeed") else {
            panic!("expected Value, got SoftFailure");
        };
        assert_eq!(decoded, original);
    }
}
