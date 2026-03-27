// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.

use crate::{Codec, Encoder, Error};

/// An encoder that serializes values to `Vec<u8>` using bincode (one-directional).
///
/// Implements `Encoder<T, Vec<u8>>` for any `T: Serialize + Send + Sync`.
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
/// Implements `Codec<T, Vec<u8>>` for any `T: Serialize + DeserializeOwned + Send + Sync`.
#[derive(Debug, Clone, Copy)]
pub struct BincodeCodec;

impl<T: serde::Serialize + Send + Sync> Encoder<T, Vec<u8>> for BincodeEncoder {
    fn encode(&self, value: &T) -> Result<Vec<u8>, Error> {
        bincode::serialize(value).map_err(Error::from_source)
    }
}

impl<T: serde::Serialize + Send + Sync> Encoder<T, Vec<u8>> for BincodeCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, Error> {
        bincode::serialize(value).map_err(Error::from_source)
    }
}

impl<T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync> Codec<T, Vec<u8>> for BincodeCodec {
    fn decode(&self, bytes: &Vec<u8>) -> Result<T, Error> {
        bincode::deserialize(bytes).map_err(Error::from_source)
    }
}
