// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Raw deflate (RFC 1951): the compressed payload with no header and no checksum.
//!
//! Use this only where the surrounding format supplies its own framing and integrity check, such
//! as inside a ZIP archive or a PNG chunk. Without a checksum, corruption is not reliably detected,
//! so prefer [`zlib`][crate::zlib] or [`gzip`][crate::gzip] for data in transit.
//!
//! # Examples
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::deflate;
//!
//! let memory = GlobalPool::new();
//! let compressed = deflate::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     memory.clone(),
//! )?;
//!
//! assert_eq!(
//!     deflate::decompress(compressed, memory)?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

use crate::flate::Wrapper;
use crate::flate::codec::{FlateCompress, FlateDecompress};
use crate::format::macros::define_format;

define_format! {
    name = "deflate",
    compressor_codec = FlateCompress,
    compressor_options = (),
    new_compressor = |level, (), pool| FlateCompress::new(Wrapper::Raw, level, pool),
    decompressor_codec = FlateDecompress,
    decompressor_options = (),
    default_limits = crate::flate::DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, (), pool| {
        FlateDecompress::new(Wrapper::Raw, limits, multi_stream, trailing_data, pool)
    },
    multi_stream_default = false,
    multi_stream_doc = "Sets whether consecutive deflate streams decompress as one logical stream.\n\nDisabled by default: raw deflate carries no framing, so trailing bytes are usually not another stream.",
}
