// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.

use bytesbuf::BytesView;

use crate::{Codec, Encoder, Error};

/// An encoder that serializes values to [`BytesView`] using bincode (one-directional).
///
/// Implements `Encoder<T, BytesView>` for any `T: Serialize + Send + Sync`.
///
/// For bidirectional serialization/deserialization, use [`BincodeCodec`].
///
/// # Examples
///
/// ```ignore
/// use cachet::{BincodeEncoder, BincodeCodec};
///
/// let cache = Cache::builder::<String, MyValue>(clock)
///     .memory()
///     .serialize(BincodeEncoder, BincodeCodec)
///     .fallback(redis)
///     .build();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BincodeEncoder;

/// A bidirectional codec that serializes and deserializes values using bincode.
///
/// Implements `Codec<T, BytesView>` for any `T: Serialize + DeserializeOwned + Send + Sync`.
#[derive(Debug, Clone, Copy)]
pub struct BincodeCodec;

impl<T: serde::Serialize + Send + Sync> Encoder<T, BytesView> for BincodeEncoder {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        let bytes = bincode::serialize(value).map_err(Error::from_source)?;
        Ok(bytes.into())
    }
}

impl<T: serde::Serialize + Send + Sync> Encoder<T, BytesView> for BincodeCodec {
    fn encode(&self, value: &T) -> Result<BytesView, Error> {
        let bytes = bincode::serialize(value).map_err(Error::from_source)?;
        Ok(bytes.into())
    }
}

impl<T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync> Codec<T, BytesView> for BincodeCodec {
    fn decode(&self, bytes: &BytesView) -> Result<T, Error> {
        bincode::deserialize(bytes.first_slice()).map_err(Error::from_source)
    }
}
