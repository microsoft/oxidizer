// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compression codecs for transforming bytes to/from compressed form.

use crate::{Codec, Error};

/// A codec that compresses and decompresses bytes using Zstandard.
///
/// Implements `Codec<Vec<u8>, Vec<u8>>` for both compression (encode) and
/// decompression (decode). The direction is determined by `TransformAdapter`'s
/// type parameters.
#[derive(Debug, Clone)]
pub struct ZstdEncoder {
    level: i32,
}

impl ZstdEncoder {
    /// Creates a new Zstd encoder with the given compression level.
    ///
    /// Levels typically range from 1 (fastest) to 22 (best compression).
    /// Level 3 is a good default.
    pub fn new(level: i32) -> Self {
        Self { level }
    }
}

impl Codec<Vec<u8>, Vec<u8>> for ZstdEncoder {
    fn apply(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        zstd::encode_all(value.as_slice(), self.level).map_err(Error::from_source)
    }
}

/// A codec that decompresses Zstandard-compressed bytes.
#[derive(Debug, Clone, Copy)]
pub struct ZstdDecoder;

impl Codec<Vec<u8>, Vec<u8>> for ZstdDecoder {
    fn apply(&self, value: &Vec<u8>) -> Result<Vec<u8>, Error> {
        zstd::decode_all(value.as_slice()).map_err(Error::from_source)
    }
}
