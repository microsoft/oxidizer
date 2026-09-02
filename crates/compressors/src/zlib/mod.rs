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
//! use compressors::{Resources, zlib};
//!
//! let memory = GlobalPool::new();
//! let compressed = zlib::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     &Resources::default(),
//! )?;
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

/// Selects zlib as the format of a [`CompressorBuilder`][crate::CompressorBuilder] or [`DecompressorBuilder`][crate::DecompressorBuilder].
///
/// Zlib has no settings beyond the ones every format shares, so this type carries none. It exists
/// to name the format in the builder's type parameter, which is what gives that builder a `build`
/// method producing this module's [`Compressor`] and [`Decompressor`].
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
