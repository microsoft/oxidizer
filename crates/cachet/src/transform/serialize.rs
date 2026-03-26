// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization codecs for converting typed values to/from bytes via serde.

use crate::{Codec, Error};

/// A codec that serializes values to `Vec<u8>` using bincode.
///
/// Implements `Codec<T, Vec<u8>>` for any `T: Serialize + Send + Sync`.
///
/// For deserialization, use [`BincodeDecoder`].
///
/// # Examples
///
/// ```ignore
/// use cachet::{BincodeEncoder, BincodeDecoder};
///
/// let cache = Cache::builder::<String, MyValue>(clock)
///     .memory()
///     .serialize(BincodeEncoder, BincodeEncoder, BincodeDecoder)
///     .fallback(redis)
///     .build();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BincodeEncoder;

/// A codec that deserializes `Vec<u8>` to typed values using bincode.
///
/// Implements `Codec<Vec<u8>, T>` for any `T: DeserializeOwned + Send + Sync`.
#[derive(Debug, Clone, Copy)]
pub struct BincodeDecoder;

impl<T: serde::Serialize + Send + Sync> Codec<T, Vec<u8>> for BincodeEncoder {
    fn apply(&self, value: &T) -> Result<Vec<u8>, Error> {
        bincode::serialize(value).map_err(Error::from_source)
    }
}

impl<T: serde::de::DeserializeOwned + Send + Sync> Codec<Vec<u8>, T> for BincodeDecoder {
    fn apply(&self, bytes: &Vec<u8>) -> Result<T, Error> {
        bincode::deserialize(bytes).map_err(Error::from_source)
    }
}
