// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Streams compression and decompression over [`bytesbuf`] byte sequences.
//!
//! Each supported format has a module and a Cargo feature with the same name. The formats are
//! `deflate`, `zlib`, `gzip`, `brotli`, and `zstd`. Each module provides `compress`, `decompress`,
//! `Compressor`, and `Decompressor`. The `brotli` and `zstd` builders also provide format-specific
//! settings. The zstd compressor and decompressor `build` methods return a [`Result`].
//!
//! An engine is a third-party format implementation and its working memory. The implementations are
//! `flate2`, the `brotli` crate, and `zstd-safe`. A `Compressor` or `Decompressor` owns one engine
//! for one operation. When dropped, it returns reusable engine state to [`Resources`] if that
//! engine supports recycling.
//!
//! This crate provides these capabilities:
//!
//! * It reads existing [`BytesView`] segments directly.
//! * It writes output into the uninitialized spare capacity of a
//!   [`BytesBuf`][bytesbuf::BytesBuf].
//! * It recycles engine state through [`Resources`].
//! * It uses one push/pull contract for every format and stream length.
//!
//! The engines do not use `std::io` adapters. Direct adapters write into the destination
//! [`BytesBuf`][bytesbuf::BytesBuf] without an intermediate output buffer. They also preserve
//! reusable engine state and keep flush and chunk boundaries under this crate's control.
//!
//! # Whole buffers
//!
//! Each format module provides `compress` and `decompress`. The crate-level [`compress`] and
//! [`decompress`] functions accept any engine that implements [`Compression`].
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use compressors::{Resources, gzip};
//!
//! let resources = Resources::global();
//! let compressed = gzip::compress(b"hello", resources)?;
//!
//! assert_eq!(
//!     gzip::decompress(compressed, resources)?.to_vec(),
//!     b"hello".to_vec()
//! );
//! # }
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Streaming
//!
//! An engine is a state machine. Its pending output stays bounded by the configured chunk size.
//! Pending input, the engine window, and engine tables use additional memory. Their size depends on
//! the format and its configuration. With the `futures-stream` feature, `CompressionStream`
//! converts a stream of byte sequences:
//!
//! ```
//! # #[cfg(all(feature = "futures-stream", feature = "gzip"))]
//! # {
//! use std::io::Error as IoError;
//!
//! use bytesbuf::BytesView;
//! use compressors::{CompressionStream, Resources, gzip};
//! use futures::{TryStreamExt, stream};
//!
//! # futures::executor::block_on(async {
//! let resources = Resources::global();
//! let body = stream::iter(vec![
//!     Ok::<_, IoError>(BytesView::copied_from_slice(b"a body ", resources.memory())),
//!     Ok(BytesView::copied_from_slice(
//!         b"in pieces",
//!         resources.memory(),
//!     )),
//! ]);
//!
//! let mut compressed = CompressionStream::compress(body, gzip::Compressor::new(resources));
//!
//! // Inspect and drop each chunk to keep retained memory bounded.
//! let mut magic = Vec::new();
//! while let Some(chunk) = compressed.try_next().await? {
//!     if magic.is_empty() && chunk.len() >= 2 {
//!         magic = chunk.range(0..2).to_vec();
//!     }
//! }
//!
//! assert_eq!(magic, vec![0x1f, 0x8b]);
//! # Ok::<(), compressors::Error>(())
//! # })
//! # .expect("the in-memory source stream cannot fail");
//! # }
//! ```
//!
//! # Choosing a format
//!
//! If the format is known only at run time, use the [`format` module][crate::format].
//! It resolves a `Content-Encoding` token and provides `Compressor`, `Decompressor`, and
//! whole-buffer functions. Use [`CompressorBuilder::build_format`] to set a level or chunk size.
//!
//! The `deflate` feature and the HTTP `deflate` content coding are different. `Format::Deflate` is
//! raw DEFLATE (RFC 1951), which has no content-coding token. Therefore,
//! `Format::Deflate.content_encoding()` returns `None`. The HTTP `deflate` token denotes a
//! zlib-wrapped stream (RFC 1950). `Format::from_content_encoding("deflate")` therefore resolves
//! to `Format::Zlib` and needs the `zlib` feature, not the `deflate` feature.
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use compressors::Resources;
//! use compressors::format::{self, Format};
//!
//! let format = Format::from_content_encoding("gzip").expect("this build supports gzip");
//!
//! let resources = Resources::global();
//! let compressed = format::compress(format, b"runtime selected", resources)?;
//!
//! assert_eq!(
//!     format::decompress(format, compressed, resources)?.to_vec(),
//!     b"runtime selected".to_vec()
//! );
//! # }
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Reusing engine state
//!
//! Building a compressor allocates and initializes engine state. For a small message, this setup
//! costs about as much as compression. [`Resources`] recycles this state. Keep one value and pass it
//! to each compressor and decompressor.
//!
//! Recycling is enabled by default. Each API that builds an engine therefore takes `Resources`.
//! Set the pool capacity to zero with [`with_pool_capacity`][Resources::with_pool_capacity] to
//! disable recycling.
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use compressors::{Level, Resources, gzip};
//!
//! // Keep one value for the application.
//! let resources = Resources::global();
//!
//! // Build one compressor for each request. Dropping it recycles its engine.
//! let compressor = gzip::Compressor::builder()
//!     .level(Level::DEFAULT)
//!     .build(resources);
//! # let _ = compressor;
//! # }
//! ```
//!
//! The pool retains only engine state for which reuse justifies its memory cost. This choice is not
//! part of the public contract.
//!
//! # Security
//!
//! Each format can expand its input by orders of magnitude. Untrusted compressed data can exhaust
//! memory. `CompressionStream` does not accumulate output, but its caller can still retain chunks.
//! Whole-buffer decompression adds fallback limits for bounds that the caller did not set. The
//! fallbacks are 64 MiB of output and 1024 concatenated streams.
//!
//! If you buffer decompressed output, set
//! [`max_output_len`][DecompressorLimits::max_output_len] to an affordable value. Also limit the
//! number of operations that decompress data at the same time. [`DecompressorLimits`] lists the
//! defaults for each format.
//!
//! Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
//! them as provisional until the decompressor reports that it is done. A checksum match is not an
//! authenticity claim. These formats are not authenticated. Decompression does not make content
//! trusted.
//!
//! See [`documentation`] for the full threat model.
//!
//! # Features
//!
//! Each format is a separate feature. No format is enabled by default:
//!
//! * `gzip`: The `gzip` module and `Format::Gzip`, through `flate2`. Most HTTP clients and servers
//!   accept this format.
//! * `deflate`: The `deflate` module and `Format::Deflate`, through `flate2`. Raw DEFLATE has no
//!   HTTP content-coding token of its own.
//! * `zlib`: The `zlib` module and `Format::Zlib`, through `flate2`. The HTTP `deflate` content
//!   coding denotes this format.
//! * `brotli`: The `brotli` module and `Format::Brotli`, through the pure-Rust `brotli` crate.
//! * `zstd`: The `zstd` module and `Format::Zstd`, through `zstd-safe`.
//! * `futures-stream`: `CompressionStream` presents compression and decompression as a
//!   `futures_core::Stream` over any stream of byte sequences.
//!
//! The deflate-family features share one dependency. A build that needs only `brotli` or `zstd`
//! does not compile `flate2`. A build with no format still provides [`Compression`], the builders,
//! and [`Resources`].
//!
//! # Further reading
//!
//! See [`documentation`] for the longer form guides.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/compressors/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/compressors/favicon.ico")]

#[cfg(any(doc, test))]
pub mod documentation;

#[cfg(any(test, feature = "brotli"))]
pub mod brotli;
mod builder;
pub mod core;
#[cfg(any(test, feature = "deflate"))]
pub mod deflate;
#[cfg(any(test, any_format))]
mod engine;
mod error;
#[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
mod flate;
pub mod format;
#[cfg(any(test, feature = "gzip"))]
pub mod gzip;
mod input;
mod level;
pub(crate) mod limits;
#[cfg(any(test, any_format))]
mod macros;
mod pool;
mod resources;
mod trailing;
#[cfg(any(test, feature = "zlib"))]
pub mod zlib;
#[cfg(any(test, feature = "zstd"))]
pub mod zstd;

#[cfg(any(test, feature = "futures-stream"))]
mod stream;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod testing;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;

pub use builder::{CompressorBuilder, DecompressorBuilder};
use bytesbuf::BytesView;
pub use error::{BuildError, Error, Result};
pub use input::InputData;
pub use level::Level;
pub use limits::DecompressorLimits;
pub use resources::Resources;
#[cfg(any(test, feature = "futures-stream"))]
pub use stream::CompressionStream;
pub use trailing::TrailingData;

use crate::core::{Compress, Compression, Decompress, process};

/// Compresses one complete byte sequence that is already in memory.
///
/// Takes any compressor that implements [`Compression`] with [`Compress`] mode. This includes a
/// concrete compressor and the `format::Compressor` from [`CompressorBuilder::build_format`].
///
/// This function buffers the complete result. Use `CompressionStream` for incremental input.
///
/// # Security
///
/// Unlike [`decompress`], this function applies no output ceiling. Compression does not amplify
/// retained data by orders of magnitude. A fixed cap can also reject valid large input.
///
/// # Errors
///
/// Returns an error if the underlying compression engine fails.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use bytesbuf::BytesView;
/// use compressors::format::Format;
/// use compressors::{CompressorBuilder, Resources, gzip};
///
/// let resources = Resources::global();
/// let input = BytesView::copied_from_slice(b"either way", resources.memory());
///
/// // A compressor built by hand, or one for a format chosen at runtime.
/// let by_hand = compressors::compress(input.clone(), gzip::Compressor::new(resources))?;
/// let at_runtime = compressors::compress(
///     input,
///     CompressorBuilder::new().build_format(Format::Gzip, resources)?,
/// )?;
///
/// assert_eq!(by_hand.to_vec(), at_runtime.to_vec());
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
pub fn compress(input: BytesView, compressor: impl Compression<Mode = Compress>) -> Result<BytesView> {
    process(compressor, input)
}

/// Decompresses one complete stream that is already in memory.
///
/// Takes any decompressor that implements [`Compression`] with [`Decompress`] mode.
///
/// # Errors
///
/// Returns an error if the data is malformed, truncated, or exceeds the limits the decompressor
/// was built with.
///
/// # Security
///
/// Buffering decompressed output from untrusted input can exhaust memory. For unset bounds, this
/// function adds a 64 MiB output limit and a 1024 concatenated-stream limit. The output limit
/// bounds expansion. The stream limit bounds repeated engine setup for many small members.
///
/// These limits are independent fallbacks. The decompressor settings from
/// [`limits`][DecompressorBuilder::limits] take precedence:
///
/// | That bound was built with | Applied here |
/// |---|---|
/// | nothing, or [`DecompressorLimits::new`] | 64 MiB output, 1024 streams |
/// | [`max_output_len`][DecompressorLimits::max_output_len] or [`max_streams`][DecompressorLimits::max_streams] | the caller's value |
/// | [`unbounded_output_len`][DecompressorLimits::unbounded_output_len] or [`unbounded_streams`][DecompressorLimits::unbounded_streams] | none for that bound |
/// | [`DecompressorLimits::UNLIMITED`] | none for either bound |
pub fn decompress(input: BytesView, decompressor: impl Compression<Mode = Decompress>) -> Result<BytesView> {
    process(decompressor, input)
}
