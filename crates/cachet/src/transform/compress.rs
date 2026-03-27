// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compression codecs for transforming bytes to/from compressed form.

use std::cell::RefCell;
use std::io::Read;

use crate::{Codec, Encoder, Error};

/// A bidirectional codec that compresses and decompresses bytes using Zstandard.
///
/// Implements `Codec<Vec<u8>, Vec<u8>>` with `encode` for compression and
/// `decode` for decompression.
///
/// Decompression reuses a thread-local `DCtx` to avoid re-allocating expensive
/// internal state (memory tables, huffman trees) on every call.
#[derive(Debug, Clone)]
pub struct ZstdCodec {
    level: i32,
}

thread_local! {
    // The zstd DCtx holds expensive internal state. Reusing it across calls
    // avoids repeated allocation of that state.
    static DCTX: RefCell<zstd::zstd_safe::DCtx<'static>> =
        RefCell::new(zstd::zstd_safe::DCtx::create());
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
        DCTX.with_borrow_mut(|dctx| {
            dctx.reset(zstd::zstd_safe::ResetDirective::SessionOnly)
                .map_err(|code| Error::from_message(format!("failed to reset zstd decompression context: error code {code}")))?;
            let mut decoder = zstd::stream::read::Decoder::with_context(value.as_slice(), dctx);
            let mut output = Vec::new();
            decoder.read_to_end(&mut output).map_err(Error::from_source)?;
            decoder.finish();
            Ok(output)
        })
    }
}
