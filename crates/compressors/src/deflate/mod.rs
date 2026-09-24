// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Raw deflate (RFC 1951): the compressed payload with no header and no checksum.
//!
//! Use this only where the surrounding format supplies its own framing and integrity check, such as
//! inside a ZIP archive. Without a checksum, corruption is not reliably detected, so prefer `zlib`
//! or `gzip` for data in transit. PNG is not an example of this: its `IDAT` payloads concatenate
//! into a single zlib stream, so reach for `zlib` there.
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

/// Selects raw deflate as the format of a [`CompressorBuilder`][crate::CompressorBuilder] or [`DecompressorBuilder`][crate::DecompressorBuilder].
///
/// Raw deflate has no settings beyond the ones every format shares, so this type carries none. It
/// exists to name the format in the builder's type parameter, which is what gives that builder a
/// `build` method producing this module's [`Compressor`] and [`Decompressor`].
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
