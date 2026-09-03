// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(
    not(all(feature = "futures-stream", feature = "gzip")),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "the crate documentation illustrates itself with gzip and CompressionStream, so its links need those features"
    )
)]

//! Streaming compression and decompression over [`bytesbuf`] byte sequences.
//!
//! Five formats are available, each behind a cargo feature of its own: `deflate`, `zlib`,
//! `gzip`, `brotli` and `zstd`. Each lives in its own module and exposes the same handful of items,
//! so moving between them is a change of import rather than a change of code.
//!
//! Three things distinguish this crate:
//!
//! * **It speaks [`bytesbuf`] natively.** Input is read from a [`BytesView`]'s segments where they
//!   already sit, and output is written into the uninitialized spare capacity of a
//!   [`BytesBuf`][bytesbuf::BytesBuf]. Nothing is flattened into an intermediate buffer on the way
//!   in, and nothing is copied out of one on the way back.
//! * **It recycles engine state.** [`Resources`] keeps the window and hash tables an engine
//!   allocates and hands them to the next codec that needs them. On a small message that setup
//!   costs about as much as the compression itself, so the saving is worth having.
//! * **One API spans every format, at any size.** The same push/pull contract drives all five
//!   engines, so code is written once and works with whichever one it is given. Because a codec is
//!   a state machine rather than a one-shot transform, gigabytes pass through it with a working set
//!   of one pending input view and one output chunk.
//!
//! Secondarily, this is also why the engines are not driven through `std::io`. `std::io::Read` and
//! `std::io::Write` assume a single contiguous `&[u8]`, whereas a [`BytesView`] is a chain of
//! segments with no contiguous representation, so bridging the two that way would mean copying
//! every byte into a flat buffer first.
//!
//! # Whole buffers
//!
//! Each format module has its own `compress` and `decompress` for the common case. The crate-level
//! [`compress`] and [`decompress`] take an operation you already have instead, whatever built it.
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use bytesbuf::BytesView;
//! use compressors::{Resources, gzip};
//!
//! let resources = Resources::global();
//! let compressed = gzip::compress(
//!     BytesView::copied_from_slice(b"hello", resources.memory()),
//!     resources,
//! )?;
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
//! A codec is a state machine rather than a one-shot transform, so a stream of any length moves
//! through it with a bounded working set: one pending input view and one output chunk, however many
//! gigabytes pass through. [`CompressionStream`], behind the `futures-stream` feature, is how to
//! reach that -- it turns any stream of byte sequences into its compressed or decompressed
//! counterpart:
//!
//! ```
//! # #[cfg(all(feature = "futures-stream", feature = "gzip"))]
//! # {
//! use bytesbuf::BytesView;
//! use compressors::{CompressionStream, Resources, gzip};
//! use futures::{StreamExt, stream};
//!
//! # futures::executor::block_on(async {
//! let resources = Resources::global();
//! let body = stream::iter(vec![
//!     Ok::<_, std::io::Error>(BytesView::copied_from_slice(b"a body ", resources.memory())),
//!     Ok(BytesView::copied_from_slice(
//!         b"in pieces",
//!         resources.memory(),
//!     )),
//! ]);
//!
//! let chunks: Vec<_> = CompressionStream::compress(body, gzip::Compressor::new(resources))
//!     .collect()
//!     .await;
//!
//! let gzip = BytesView::from_views(chunks.into_iter().map(|chunk| chunk.unwrap()));
//! assert_eq!(gzip.range(0..2).to_vec(), vec![0x1f, 0x8b]);
//! # });
//! # }
//! ```
//!
//! # Choosing a format
//!
//! When the format is only known at runtime -- from a `Content-Encoding` token, say -- [`Format`]
//! resolves the token and compresses with whatever it names. Reach for
//! [`CompressorBuilder::build_format`] instead when the level or the chunk size matters: it returns
//! an operation that fits wherever a concrete one does.
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use bytesbuf::BytesView;
//! use compressors::{Format, Resources};
//!
//! let format = Format::from_content_encoding("gzip").expect("this build supports gzip");
//!
//! let resources = Resources::global();
//! let compressed = format.compress(
//!     BytesView::copied_from_slice(b"runtime selected", resources.memory()),
//!     resources,
//! )?;
//!
//! assert_eq!(
//!     format.decompress(compressed, resources)?.to_vec(),
//!     b"runtime selected".to_vec()
//! );
//! # }
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Reusing engine state
//!
//! Building a compressor allocates and initializes a substantial amount of state -- on a small
//! message, as much work as the compression itself. [`Resources`] recycles it: hold one, hand it to
//! every operation, and each engine returns to it when its codec drops. The saving is roughly fixed
//! per message, so it matters most for small bodies.
//!
//! Recycling is on by default, which is why every API that builds a codec asks for resources rather
//! than for a memory provider alone. Turn it off with
//! [`enable_pooling(0)`][Resources::enable_pooling] when there is genuinely nothing to reuse.
//!
//! ```
//! # #[cfg(feature = "gzip")]
//! # {
//! use compressors::{Level, Resources, gzip};
//!
//! // Held once by the application, cloned into whatever needs it.
//! let resources = Resources::global();
//!
//! // Per request: cheap to build, recycles the engine on drop.
//! let compressor = gzip::Compressor::builder()
//!     .level(Level::DEFAULT)
//!     .build(resources);
//! # let _ = compressor;
//! # }
//! ```
//!
//! Recycling is transparent -- it applies to the engines that are worth it and quietly skips the
//! rest -- so calling code never has to know which engines benefit.
//!
//! # Security
//!
//! Every one of these formats can expand its input by orders of magnitude, so a decompressor
//! pointed at untrusted data is a memory-exhaustion vector. A decompressor driven directly never
//! accumulates -- each chunk it hands back is bounded -- so the exposure is in what the caller
//! keeps, which makes it the conveniences that buffer a whole result that need bounding. Those add
//! a 64 MiB output cap and a 1024 concatenated-stream cap to whatever the caller did not set.
//!
//! When you buffer decompressed output yourself, set
//! [`with_max_output_len`][DecompressorLimits::with_max_output_len] to what you can afford. That
//! guardrail is for the common case, not a substitute for bounding how many bodies you decompress
//! at once. [`DecompressorLimits`] documents what each format bounds by default, and why a ratio
//! alone is not protection.
//!
//! Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
//! them as provisional until the operation reports that it is done.
//!
//! # Features
//!
//! Every format is a separate feature and none is on by default, so a build compiles only the
//! engines it names:
//!
//! * `gzip` -- the `gzip` module and `Format::Gzip`, via `flate2`. The encoding most often seen on
//!   the wire, and the one to reach for when in doubt.
//! * `deflate` -- the `deflate` module and `Format::Deflate`, via `flate2`.
//! * `zlib` -- the `zlib` module and `Format::Zlib`, via `flate2`.
//! * `brotli` -- the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
//! * `zstd` -- the `zstd` module and `Format::Zstd`, via `zstd-safe`.
//! * `futures-stream` -- [`CompressionStream`], presenting compression and decompression as a
//!   `futures_core::Stream` over any stream of byte sequences.
//!
//! The deflate-family features share one dependency, so enabling all three costs no more than one.
//! A build that needs only `brotli` or only `zstd` never compiles `flate2` at all, and a build that
//! names no format at all still gets [`Compression`], the builders and [`Resources`], which is what
//! a crate that only passes operations around needs.

#[cfg(feature = "brotli")]
pub mod brotli;
mod builder;
pub mod core;
#[cfg(feature = "deflate")]
pub mod deflate;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
mod engine;
mod error;
#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
mod flate;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
mod format;
#[cfg(feature = "gzip")]
pub mod gzip;
mod level;
pub(crate) mod limits;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
mod macros;
mod pool;
mod resources;
mod trailing;
#[cfg(feature = "zlib")]
pub mod zlib;
#[cfg(feature = "zstd")]
pub mod zstd;

#[cfg(feature = "futures-stream")]
mod stream;
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests;

pub use builder::{CompressorBuilder, DecompressorBuilder};
use bytesbuf::BytesView;
pub use error::{BuildError, Error, Result};
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
pub use format::Format;
pub use level::Level;
pub use limits::DecompressorLimits;
pub use resources::Resources;
#[cfg(feature = "futures-stream")]
pub use stream::CompressionStream;
pub use trailing::TrailingData;

use crate::core::{Compress, Compression, Decompress, process};

/// Compresses one complete byte sequence that is already in memory.
///
/// Takes any compressor: a concrete one such as [`gzip::Compressor`], or a
/// boxed one whose format was chosen at runtime. The direction is part of the bound, so a
/// decompressor will not compile here.
///
/// Prefer driving the operation directly for data that arrives incrementally: this buffers the
/// entire result before returning.
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
/// use compressors::{CompressorBuilder, Format, Resources, gzip};
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
/// Takes any decompressor, exactly as [`compress`] takes any compressor.
///
/// # Errors
///
/// Returns an error if the data is malformed, truncated, or exceeds the limits the decompressor
/// was built with.
///
/// # Security
///
/// This adds no bounds of its own: the decompressor arrives already configured, so whatever it was
/// built with is what applies. It does accumulate the whole result, so pass a decompressor built
/// with [`DecompressorLimits::with_max_output_len`][crate::DecompressorLimits::with_max_output_len]
/// when the input is untrusted. Each format's own `decompress` is the bounded convenience.
pub fn decompress(input: BytesView, decompressor: impl Compression<Mode = Decompress>) -> Result<BytesView> {
    process(decompressor, input)
}
