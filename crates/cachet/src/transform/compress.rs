// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compression codecs for transforming bytes to/from compressed form.

use crate::{Codec, Encoder, Error};

/// A bidirectional codec that compresses and decompresses bytes using Zstandard.
///
/// Implements `Codec<Vec<u8>, Vec<u8>>` with `encode` for compression and
/// `decode` for decompression.
#[derive(Debug, Clone)]
pub struct ZstdCodec {
    level: i32,
}

impl ZstdCodec {
    /// Creates a new Zstd codec with the given compression level.
    ///
    /// Levels typically range from 1 (fastest) to 22 (best compression).
    /// Level 3 is a good default.
    pub fn new(level: i32) -> Self {
        Self { level }
    }
}

impl Encoder<Vec<u8>, Vec<u8>> for ZstdCodec {
    fn encode(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        zstd::encode_all(value.as_slice(), self.level).map_err(Error::from_source)
    }
}

impl Codec<Vec<u8>, Vec<u8>> for ZstdCodec {
    fn decode(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        zstd::decode_all(value.as_slice()).map_err(Error::from_source)
    }
}
