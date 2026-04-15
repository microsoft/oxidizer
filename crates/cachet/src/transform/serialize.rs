// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.
//!
//! These codecs serialize values into [`BytesView`] using pool-backed memory,
//! avoiding heap allocations. The serialized bytes can then flow through
//! compression, encryption, and network I/O without additional copies.

use bincode::{deserialize_from, serialize_into};
use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};

use crate::{Codec, Encoder, Error};

use serde::{Serialize, de::DeserializeOwned};

/// An encoder that serializes values to [`BytesView`] using bincode (one-directional).
///
/// Implements `Encoder<T, BytesView>` for any `T: Serialize + Send + Sync`.
///
/// For bidirectional serialization/deserialization, use [`BincodeCodec`].
#[derive(Debug, Clone, Copy)]
pub struct BincodeEncoder;

/// A bidirectional codec that serializes and deserializes values using bincode.
///
/// Implements `Codec<T, BytesView>` for any `T: Serialize + DeserializeOwned + Send + Sync`.
///
/// Serialization writes directly into pool-backed memory via [`BytesBufWriter`](bytesbuf::BytesBufWriter),
/// producing a [`BytesView`] with no intermediate heap allocation.
#[derive(Debug, Clone, Copy)]
pub struct BincodeCodec;

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for BincodeEncoder {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

impl<T: Serialize + Send + Sync> Encoder<T, BytesView> for BincodeCodec {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        encode(value)
    }
}

fn encode<T: Serialize + Send + Sync>(value: &T) -> Result<BytesView, Error> {
    let mut writer = BytesBuf::new().into_writer(GlobalPool::new());
    serialize_into(&mut writer, value).map_err(Error::from_source)?;
    Ok(writer.into_inner().peek())
}

impl<T: Serialize + DeserializeOwned + Send + Sync> Codec<T, BytesView> for BincodeCodec {
    fn decode(&self, mut value: BytesView) -> Result<T, Error> {
        // BytesView implements Read, allowing deserialization across all spans
        // without copying bytes into an intermediate buffer.
        deserialize_from(&mut value).map_err(Error::from_source)
    }
}
