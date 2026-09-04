// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Streaming compression and decompression over [`bytesbuf`] byte sequences.
//!
//! Each supported format -- `deflate`, `zlib`, `gzip`, `brotli`, `zstd` -- lives in a module of its
//! own behind a cargo feature of its own. What those modules share is uniform: `compress`,
//! `decompress`, `Compressor` and `Decompressor` have the same shape in every one of them, so
//! moving a call site between formats is a change of import. Their builders are not uniform:
//! `brotli` and `zstd` add format-specific settings, and their compressor `build` returns a
//! [`Result`], so switching a builder call site can take more than an import change.
//!
//! **Engine** below means a third-party format implementation (`flate2`, the `brotli` crate,
//! `zstd-safe`) together with the working memory it allocates. A `Compressor` or a `Decompressor`
//! owns one, configured and positioned in a single stream, and returns it to [`Resources`] on drop.
//!
//! This crate is distinguished by:
//!
//! * **It reads and writes [`bytesbuf`] sequences directly.** Input is read from a [`BytesView`]'s
//!   segments where they already sit, and output is written into the uninitialized spare capacity
//!   of a [`BytesBuf`][bytesbuf::BytesBuf]. Nothing is flattened into an intermediate buffer on the
//!   way in, and nothing is copied out of one on the way back.
//! * **It recycles engine state.** [`Resources`] keeps the window and hash tables an engine
//!   allocates and hands them to the next compressor or decompressor that needs them. On a small
//!   message that setup costs about as much as the compression itself, so the saving is worth
//!   having.
//! * **One API spans every format, at any size.** The same push/pull contract drives every engine,
//!   so code is written once and works with whichever one it is given. Because an engine is a state
//!   machine rather than a one-shot transform, a stream of any length passes through it while the
//!   pending output it buffers stays bounded by the configured chunk size.
//!
//! Secondarily, this is also why the engines are not driven through `std::io`. That route works --
//! [`BytesView`] implements `BufRead` over its segments and `BytesBufWriter` implements `Write`
//! into segmented storage, so nothing has to be flattened to use it. What the direct adapters buy
//! is narrower: output goes straight into a [`BytesBuf`][bytesbuf::BytesBuf]'s uninitialized spare
//! capacity rather than through an intermediate buffer the adapter owns, engine state stays
//! reusable from one stream to the next, and flush and chunk boundaries remain under this crate's
//! control.
//!
//! # Whole buffers
//!
//! Each format module has its own `compress` and `decompress` for the common case. The crate-level
//! [`compress`] and [`decompress`] instead accept any engine implementing [`Compression`],
//! however it was constructed.
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
//! An engine is a state machine rather than a one-shot transform, so a stream of any length moves
//! through it while the output it has buffered but not yet handed back stays bounded by the
//! configured chunk size. Pending input and the engine's own window and tables are additional, and
//! their size depends on the format and its configuration. `CompressionStream`, behind the
//! `futures-stream` feature, is how to reach that -- it turns any stream of byte sequences into its
//! compressed or decompressed counterpart:
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
//! // Each chunk is inspected and dropped as it arrives, so the caller stays bounded too --
//! // collecting them all would put the whole encoded body back in memory.
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
//! When the format is only known at runtime -- from a `Content-Encoding` token, say -- the
//! [`format`](mod@crate::format) module resolves the token and carries the same shape every other
//! format module does: a `Compressor`, a `Decompressor`, and the whole-buffer conveniences. Use
//! [`CompressorBuilder::build_format`] instead when a level or chunk size has to be set on the
//! result.
//!
//! Note that the `deflate` feature and the HTTP `deflate` content coding are not the same thing.
//! `Format::Deflate` is raw DEFLATE (RFC 1951), which has no content-coding token, so
//! `Format::Deflate.content_encoding()` returns `None`. The HTTP `deflate` token denotes a
//! zlib-wrapped stream (RFC 1950), so `Format::from_content_encoding("deflate")` resolves to
//! `Format::Zlib` and needs the `zlib` feature, not the `deflate` one.
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
//! Building a compressor allocates and initializes a substantial amount of state -- on a small
//! message, as much work as the compression itself. [`Resources`] recycles it: hold one, hand it to
//! every compressor and decompressor, and each engine returns to it on drop. The saving is roughly
//! fixed per message, so it matters most for small bodies.
//!
//! Recycling is on by default, which is why every API that builds an engine asks for resources rather
//! than for a memory provider alone. Set the capacity to zero with
//! [`with_pool_capacity`][Resources::with_pool_capacity] when compression is rare enough that
//! retaining engine state costs more than rebuilding it.
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
//! Recycling applies only to the engines whose state is expensive enough to be worth retaining and
//! is skipped for the rest, so calling code never has to know which engines benefit.
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
//! [`max_output_len`][DecompressorLimits::max_output_len] to what you can afford. That
//! guardrail is for the common case, not a substitute for bounding how many bodies you decompress
//! at once. [`DecompressorLimits`] documents what each format bounds by default, and why a ratio
//! alone is not protection.
//!
//! Decompression can yield bytes before a checksum or trailer has rejected the stream, so treat
//! them as provisional until the decompressor reports that it is done.
//!
//! # Features
//!
//! Every format is a separate feature and none is on by default, so a build compiles only the
//! engines it names:
//!
//! * `gzip` -- the `gzip` module and `Format::Gzip`, via `flate2`. Accepted by essentially every
//!   HTTP client and server, so it is the safe default when the peer's capabilities are unknown.
//! * `deflate` -- the `deflate` module and `Format::Deflate`, via `flate2`. Raw DEFLATE, with no
//!   HTTP content-coding token of its own.
//! * `zlib` -- the `zlib` module and `Format::Zlib`, via `flate2`. This is what the HTTP `deflate`
//!   content coding actually denotes.
//! * `brotli` -- the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
//! * `zstd` -- the `zstd` module and `Format::Zstd`, via `zstd-safe`.
//! * `futures-stream` -- `CompressionStream`, presenting compression and decompression as a
//!   `futures_core::Stream` over any stream of byte sequences.
//!
//! The deflate-family features share one dependency, so enabling more than one of them costs no
//! more than enabling one. A build that needs only `brotli` or only `zstd` never compiles `flate2`
//! at all, and a build that names no format at all still gets [`Compression`], the builders and
//! [`Resources`], which is what a crate that only passes compressors and decompressors around
//! needs.
//!
//! # Further reading
//!
//! Two guides cover the decisions that span several APIs, which no single item's documentation can
//! carry:
//!
//! * [DESIGN.md] -- the user-visible policies: format selection, what is uniform across formats and
//!   what is not, how decompression is bounded, stream framing, and why the public surface is
//!   sealed.
//! * [IMPLEMENTATION.md] -- the mechanisms behind them: the pump state machine, the unsafe
//!   initialized-output contract every backend adapter must honour, engine pooling and why some
//!   engines are excluded, and the async driving rules.
//!
//! [DESIGN.md]: https://github.com/microsoft/oxidizer/blob/main/crates/compressors/docs/DESIGN.md
//! [IMPLEMENTATION.md]: https://github.com/microsoft/oxidizer/blob/main/crates/compressors/docs/IMPLEMENTATION.md

#[cfg(any(test, feature = "brotli"))]
pub mod brotli;
mod builder;
pub mod core;
#[cfg(any(test, feature = "deflate"))]
pub mod deflate;
#[cfg(any(
    test,
    feature = "brotli",
    feature = "deflate",
    feature = "gzip",
    feature = "zlib",
    feature = "zstd"
))]
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
#[cfg(any(
    test,
    feature = "brotli",
    feature = "deflate",
    feature = "gzip",
    feature = "zlib",
    feature = "zstd"
))]
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
#[cfg(any(
    test,
    feature = "brotli",
    feature = "deflate",
    feature = "gzip",
    feature = "zlib",
    feature = "zstd"
))]
pub use level::Level;
pub use limits::DecompressorLimits;
pub use resources::Resources;
#[cfg(any(test, feature = "futures-stream"))]
pub use stream::CompressionStream;
pub use trailing::TrailingData;

use crate::core::{Compress, Compression, Decompress, process};

/// Compresses one complete byte sequence that is already in memory.
///
/// Takes any compressor: a concrete one such as a `gzip::Compressor`, or a
/// boxed one whose format was chosen at runtime. The direction is part of the bound, so a
/// decompressor will not compile here.
///
/// Prefer driving the engine directly for data that arrives incrementally: this buffers the
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
/// with [`DecompressorLimits::max_output_len`][crate::DecompressorLimits::max_output_len]
/// when the input is untrusted. Each format's own `decompress` is the bounded convenience.
pub fn decompress(input: BytesView, decompressor: impl Compression<Mode = Decompress>) -> Result<BytesView> {
    process(decompressor, input)
}
