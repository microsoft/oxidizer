// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zlib (RFC 1950): a deflate payload with a two byte header and an Adler-32 trailer.
//!
//! This is the format behind HTTP `Content-Encoding: deflate`, which despite its name carries a
//! zlib stream rather than raw deflate.
//!
//! # Examples
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::zlib;
//!
//! let memory = GlobalPool::new();
//! let compressed = zlib::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     memory.clone(),
//! )?;
//!
//! assert_eq!(
//!     zlib::decompress(compressed, memory)?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::Wrapper;
use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::format::macros::define_format;

define_format! {
    name = "zlib",
    compressor_codec = FlateCompress,
    compressor_options = (),
    new_compressor = |level, (), pool| FlateCompress::new(Wrapper::Zlib, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_options = (),
    default_limits = crate::flate::DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, (), pool| {
        FlateDecompress::new(Wrapper::Zlib, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = false,
    multi_stream_doc = "Sets whether concatenated zlib streams decompress as one logical stream.\n\nDisabled by default: unlike gzip, concatenating zlib streams is not an established convention.",
}
