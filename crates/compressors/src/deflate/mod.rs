// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Raw deflate (RFC 1951): the compressed payload with no header and no checksum.
//!
//! Use this format when another container supplies framing and an integrity check, such as a ZIP
//! archive. Raw deflate cannot reliably detect corruption. Use `zlib` or `gzip` for data in
//! transit. PNG `IDAT` payloads form one zlib stream, so use `zlib` for PNG data.
//!
//! # Examples
//!
//! ```
//! use compressors::{Resources, deflate};
//!
//! let compressed = deflate::compress(b"the quick brown fox", &Resources::default())?;
//!
//! assert_eq!(
//!     deflate::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::flate::{DEFAULT_LIMITS, Wrapper};
use crate::macros::define_format;

/// Selects raw deflate for a [`CompressorBuilder`][crate::CompressorBuilder] or
/// [`DecompressorBuilder`][crate::DecompressorBuilder].
///
/// Raw deflate has no format-specific settings. This marker adds a `build` method that creates this
/// module's [`Compressor`] or [`Decompressor`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Deflate;

impl Deflate {
    /// The settings a deflate builder starts with. Raw deflate has none of its own.
    pub(crate) const fn new() -> Self {
        Self
    }
}

define_format! {
    name = "deflate",
    format = Deflate,
    build_method = build_deflate,
    compressor_codec = FlateCompress,
    compressor_build = infallible,
    new_compressor = |level, _format, pool| FlateCompress::new(Wrapper::Raw, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_build = infallible,
    default_limits = DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, _format, pool| {
        FlateDecompress::new(Wrapper::Raw, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = false,
}
