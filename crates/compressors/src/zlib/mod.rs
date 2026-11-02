// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zlib (RFC 1950): a deflate payload with a two byte header and an Adler-32 trailer.
//!
//! HTTP `Content-Encoding: deflate` uses this format, not raw deflate.
//!
//! # Examples
//!
//! ```
//! use compressors::{Resources, zlib};
//!
//! let compressed = zlib::compress(b"the quick brown fox", &Resources::default())?;
//!
//! assert_eq!(
//!     zlib::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::Wrapper;
use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::macros::define_format;

/// Selects zlib for a [`CompressorBuilder`][crate::CompressorBuilder] or
/// [`DecompressorBuilder`][crate::DecompressorBuilder].
///
/// Zlib has no format-specific settings. This marker adds a `build` method that creates this
/// module's [`Compressor`] or [`Decompressor`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Zlib;

impl Zlib {
    /// The settings a zlib builder starts with. Zlib has none of its own.
    pub(crate) const fn new() -> Self {
        Self
    }
}

define_format! {
    name = "zlib",
    format = Zlib,
    build_method = build_zlib,
    compressor_codec = FlateCompress,
    compressor_build = infallible,
    new_compressor = |level, _format, pool| FlateCompress::new(Wrapper::Zlib, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_build = infallible,
    default_limits = crate::flate::DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, _format, pool| {
        FlateDecompress::new(Wrapper::Zlib, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = false,
}
