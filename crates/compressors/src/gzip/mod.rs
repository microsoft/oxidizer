// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Gzip (RFC 1952): a deflate payload with a member header and a CRC-32 plus length trailer.
//!
//! The header is a ten byte fixed prefix followed by optional fields -- an original file name, a
//! comment, an extra field, a header checksum -- so a member header is not a fixed size.
//!
//! This is the format behind HTTP `Content-Encoding: gzip` and the `.gz` file extension.
//! Concatenated members decompress as one logical stream by default, matching `gzip(1)`.
//!
//! # Examples
//!
//! ```
//! use compressors::{Resources, gzip};
//!
//! let compressed = gzip::compress(b"the quick brown fox", &Resources::default())?;
//! assert_eq!(compressed.range(0..2).to_vec(), vec![0x1f, 0x8b]);
//!
//! assert_eq!(
//!     gzip::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::flate::{DEFAULT_LIMITS, Wrapper};
use crate::macros::define_format;

/// Selects gzip as the format of a [`CompressorBuilder`][crate::CompressorBuilder] or [`DecompressorBuilder`][crate::DecompressorBuilder].
///
/// Gzip has no settings beyond the ones every format shares, so this type carries none. It exists
/// to name the format in the builder's type parameter, which is what gives that builder a `build`
/// method producing this module's [`Compressor`] and [`Decompressor`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Gzip;

impl Gzip {
    /// The settings a gzip builder starts with. Gzip has none of its own.
    pub(crate) const fn new() -> Self {
        Self
    }
}

define_format! {
    name = "gzip",
    format = Gzip,
    build_method = build_gzip,
    compressor_codec = FlateCompress,
    compressor_build = infallible,
    new_compressor = |level, _format, pool| FlateCompress::new(Wrapper::Gzip, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_build = infallible,
    default_limits = DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, _format, pool| {
        FlateDecompress::new(Wrapper::Gzip, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = true,
}
