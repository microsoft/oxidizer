// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Gzip (RFC 1952): a deflate payload with a ten byte header and a CRC-32 plus length trailer.
//!
//! This is the format behind HTTP `Content-Encoding: gzip` and the `.gz` file extension.
//! Concatenated members decompress as one logical stream by default, matching `gzip(1)`.
//!
//! # Examples
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::gzip;
//!
//! let memory = GlobalPool::new();
//! let compressed = gzip::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     memory.clone(),
//! )?;
//! assert_eq!(compressed.range(0..2).to_vec(), vec![0x1f, 0x8b]);
//!
//! assert_eq!(
//!     gzip::decompress(compressed, memory)?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::Wrapper;
use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::format::macros::define_format;

define_format! {
    name = "gzip",
    compressor_codec = FlateCompress,
    compressor_options = (),
    new_compressor = |level, (), pool| FlateCompress::new(Wrapper::Gzip, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_options = (),
    default_limits = crate::flate::DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, (), pool| {
        FlateDecompress::new(Wrapper::Gzip, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = true,
    multi_stream_doc = "Sets whether concatenated gzip members decompress as one logical stream.\n\nEnabled by default, matching `gzip(1)`.",
}
