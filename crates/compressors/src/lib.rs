// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Streaming compression and decompression over [`bytesbuf`] byte sequences.
//!
//! Five formats are available, each behind a cargo feature of its own: `deflate`, `zlib`,
//! `gzip`, `brotli` and `zstd`. Each lives in its own module and exposes the same seven items,
//! so moving between them is a change of import rather than a change of code.
//!
//! Compression engines normally speak `std::io::Read` and `std::io::Write`, which assume a single
//! contiguous `&[u8]`. A [`BytesView`][bytesbuf::BytesView] is a chain of segments with no
//! contiguous representation, so bridging the two through `std::io` would mean copying every byte
//! into a flat buffer first. This crate drives the engine from the view's segments directly, and
//! writes into the uninitialized spare capacity of a [`BytesBuf`][bytesbuf::BytesBuf], so no
//! intermediate copy is needed.
//!
//! # Whole buffers
//!
//! Each format module has its own `compress` and `decompress` for the common case. The crate-level
//! [`compress`] and [`decompress`] take an operation you already have instead, whatever built it.
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::{Resources, gzip};
//!
//! let memory = GlobalPool::new();
//! let compressed = gzip::compress(
//!     BytesView::copied_from_slice(b"hello", &memory),
//!     &Resources::default(),
//! )?;
//!
//! assert_eq!(
//!     gzip::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"hello".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Streaming
//!
//! [`gzip::Compressor`] and [`gzip::Decompressor`] are push/pull state machines rather than one-shot
//! transforms. They carry no operations of their own: everything is driven through
//! [`Compression`], so the same loop works for any format. Each `pull` returns at most one chunk,
//! so processing a multi-gigabyte stream never holds more than one pending input view plus one
//! output chunk:
//!
//! ```
//! use bytesbuf::mem::GlobalPool;
//! use bytesbuf::{BytesBuf, BytesView};
//! use compressors::core::Compression;
//! use compressors::{Output, Resources, gzip};
//!
//! # let memory = GlobalPool::new();
//! # let source = vec![gzip::compress(
//! #     BytesView::copied_from_slice(b"streamed", &memory), &Resources::default())?];
//! let mut decompressor = gzip::Decompressor::new(&Resources::default());
//! let mut chunks = source.into_iter();
//! let mut plain = BytesBuf::new();
//!
//! loop {
//!     match decompressor.pull()? {
//!         Output::Data(data) => plain.put_bytes(data),
//!         Output::Progress => {}
//!         Output::NeedInput => match chunks.next() {
//!             Some(chunk) => decompressor.push(chunk)?,
//!             None => decompressor.end_input(),
//!         },
//!         Output::Done => break,
//!     }
//! }
//!
//! assert_eq!(plain.consume_all().to_vec(), b"streamed".to_vec());
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Choosing a format
//!
//! The [`Compression`] trait describes the contract independently of the format and direction, so
//! code can be written once and used with any implementation. When the format is only known at
//! runtime -- from a `Content-Encoding` token, say -- [`Format`] resolves it, and
//! [`CompressorBuilder::build_format`] produces a boxed operation, which is itself a `Compression`
//! and so fits anywhere a concrete one does:
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::Resources;
//! use compressors::Format;
//!
//! let format = Format::from_content_encoding("gzip").expect("this build supports gzip");
//!
//! let memory = GlobalPool::new();
//! let compressed = format.compress(
//!     BytesView::copied_from_slice(b"runtime selected", &memory),
//!     &Resources::default(),
//! )?;
//!
//! assert_eq!(
//!     format.decompress(compressed, &Resources::default())?.to_vec(),
//!     b"runtime selected".to_vec()
//! );
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
//! use compressors::{Level, Resources, gzip};
//!
//! // Held once by the application, cloned into whatever needs it.
//! let resources = Resources::global();
//!
//! // Per request: cheap to build, recycles the engine on drop.
//! let compressor = gzip::Compressor::builder().level(Level::DEFAULT).build(resources);
//! # let _ = compressor;
//! ```
//!
//! Recycling is transparent -- it applies to the engines that are worth it and quietly skips the
//! rest -- so calling code never has to know which engines benefit.
//!
//! # Security
//!
//! Every one of these formats can expand its input by orders of magnitude, so a decompressor pointed at
//! untrusted data is a memory-exhaustion vector.
//!
//! The codecs themselves never accumulate: each `pull` hands back one bounded chunk, so nothing in
//! this crate grows with the length of the stream. The exposure belongs to whatever the caller does
//! with those chunks, which is why the limits matter most for the accumulating conveniences --
//! `compress`, `decompress`, and [`Format::compress`] / [`Format::decompress`].
//! Use each format's `decompress_with_limits` or [`Format::decompress_with_limits`] for
//! untrusted in-memory input.
//!
//! Each format declares its own default bounds, because a single portable ratio cannot serve both
//! families. Deflate cannot expand by more than about `1032x` -- a structural property of the format --
//! so the deflate family defaults to `1100x` and never rejects data it could legitimately have
//! produced. Brotli has no such ceiling: measured on ordinary repetitive input it reaches `9 000x`
//! for a repeated short string, `21 000x` for a repeated sentence and `80 660x` for a megabyte of
//! zeros. It therefore has no default ratio limit; callers handling untrusted Brotli input must set
//! an absolute output limit.
//!
//! [`DecompressionLimits`] carries *overrides*, not values: bounds you leave unset keep the
//! format's default, so [`DecompressionLimits::default()`] never silently imposes one format's
//! calibration on another.
//!
//! **A ratio limit is therefore a coarse backstop, not real protection.** For untrusted input, set
//! [`DecompressionLimits::with_max_output_len`] to whatever the caller can actually afford to
//! buffer, and [`DecompressionLimits::with_max_streams`] when concatenated streams are accepted.
//! Use [`DecompressionLimits::UNLIMITED`] only for sources you trust as much as your own process.
//!
//! Streaming decompression can yield bytes before a final checksum or trailer has been verified.
//! Treat those bytes as provisional until the operation reports [`Output::Done`].
//!
//! # Features
//!
//! Every format is a separate feature, so a build compiles only the engines it names:
//!
//! * `gzip` -- the `gzip` module and `Format::Gzip`, via `flate2`. The only feature on by
//!   default, being the encoding most often seen on the wire.
//! * `deflate` -- the `deflate` module and `Format::Deflate`, via `flate2`.
//! * `zlib` -- the `zlib` module and `Format::Zlib`, via `flate2`.
//! * `brotli` -- the `brotli` module and `Format::Brotli`, via the pure-Rust `brotli` crate.
//! * `zstd` -- the `zstd` module and `Format::Zstd`, via `zstd-safe`.
//! * `futures-stream` -- [`CompressionStream`], presenting compression and decompression as a
//!   `futures_core::Stream` over any stream of byte sequences.
//!
//! The deflate-family features share one dependency, so enabling all three costs no more than one.
//! A build that needs only `brotli` or only `zstd` never compiles `flate2` at all.

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
mod limits;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
mod macros;
mod output;
mod pool;
mod resources;
mod trailing;
#[cfg(feature = "zlib")]
pub mod zlib;
#[cfg(feature = "zstd")]
pub mod zstd;

#[cfg(feature = "futures-stream")]
mod stream;

pub use builder::{CompressorBuilder, DecompressorBuilder};
pub use error::{BuildError, Error, Result};
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
pub use format::Format;
pub use level::Level;
pub use limits::DecompressionLimits;
pub use output::Output;
pub use resources::Resources;
#[cfg(feature = "futures-stream")]
pub use stream::CompressionStream;
pub use trailing::TrailingData;

use bytesbuf::BytesView;

use crate::core::{Compress, Compression, Decompress, process};

/// Compresses one complete byte sequence that is already in memory.
///
/// Takes any compressor: a concrete one such as [`gzip::Compressor`][crate::gzip::Compressor], or a
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
/// use compressors::Format;
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
/// A format's default bounds are a coarse backstop. For untrusted input, build the decompressor
/// with [`DecompressionLimits::with_max_output_len`][crate::DecompressionLimits::with_max_output_len].
pub fn decompress(input: BytesView, decompressor: impl Compression<Mode = Decompress>) -> Result<BytesView> {
    process(decompressor, input)
}
