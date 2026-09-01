// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::gzip;
//!
//! let memory = GlobalPool::new();
//! let compressed = gzip::compress(
//!     BytesView::copied_from_slice(b"hello", &memory),
//!     memory.clone(),
//! )?;
//!
//! assert_eq!(
//!     gzip::decompress(compressed, memory)?.to_vec(),
//!     b"hello".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Streaming
//!
//! [`gzip::Compressor`] and [`gzip::Decompressor`] are push/pull state machines rather than one-shot
//! transforms. Each `pull` returns at most one chunk, so processing a multi-gigabyte stream never
//! holds more than one pending input view plus one output chunk:
//!
//! ```
//! use bytesbuf::mem::GlobalPool;
//! use bytesbuf::{BytesBuf, BytesView};
//! use compressors::{Output, gzip};
//!
//! # let memory = GlobalPool::new();
//! # let source = vec![gzip::compress(
//! #     BytesView::copied_from_slice(b"streamed", &memory), memory.clone())?];
//! let mut decompressor = gzip::Decompressor::new(memory);
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
//! runtime -- from a `Content-Encoding` token, say -- [`format::Format`] resolves it and its builders
//! produce a boxed operation, which is itself a `Compression` and so fits anywhere a concrete one
//! does:
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::Level;
//! use compressors::format::Format;
//!
//! let format = Format::from_content_encoding("gzip").expect("this build supports gzip");
//!
//! let memory = GlobalPool::new();
//! let compressed = format.compress(
//!     BytesView::copied_from_slice(b"runtime selected", &memory),
//!     memory.clone(),
//! )?;
//!
//! assert_eq!(
//!     format.decompress(compressed, memory)?.to_vec(),
//!     b"runtime selected".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```
//!
//! # Reusing engine state
//!
//! Building a compressor allocates and initialises a substantial amount of state -- on a small
//! message, as much work as the compression itself. A service that compresses many messages should
//! hold one [`Pool`], clone it into each compressor, and let the engine return to the pool when the
//! compressor drops. The saving is roughly fixed per message, so it matters most for small bodies.
//!
//! ```
//! use bytesbuf::mem::GlobalPool;
//! use compressors::{Pool, gzip};
//!
//! let codecs = Pool::new();
//! let memory = GlobalPool::new();
//!
//! // Per request: cheap to build, recycles the engine on drop.
//! let compressor = gzip::Compressor::builder().pool(codecs.clone()).build(memory);
//! # let _ = compressor;
//! ```
//!
//! The pool is transparent -- it recycles what is worth recycling and builds the rest -- so calling
//! code never has to know which engines benefit. See [`Pool`] for what is pooled today.
//!
//! # Security
//!
//! Every one of these formats can expand its input by orders of magnitude, so a decompressor pointed at
//! untrusted data is a memory-exhaustion vector.
//!
//! The codecs themselves never accumulate: each `pull` hands back one bounded chunk, so nothing in
//! this crate grows with the length of the stream. The exposure belongs to whatever the caller does
//! with those chunks, which is why the limits matter most for the accumulating conveniences --
//! `compress`, `decompress`, and [`format::Format::compress`] / [`format::Format::decompress`].
//! Use each format's `decompress_with_limits` or [`format::Format::decompress_with_limits`] for
//! untrusted in-memory input.
//!
//! Each format declares its own default bounds, because a single portable ratio cannot serve both
//! families. Deflate cannot expand by more than about 1032x -- a structural property of the format --
//! so the deflate family defaults to 1100x and never rejects data it could legitimately have
//! produced. Brotli has no such ceiling: measured on ordinary repetitive input it reaches 9 000x
//! for a repeated short string, 21 000x for a repeated sentence and 80 660x for a megabyte of
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
mod compression;
#[cfg(feature = "deflate")]
pub mod deflate;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
mod engine;
mod error;
#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
mod flate;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
pub mod format;
#[cfg(feature = "gzip")]
pub mod gzip;
mod level;
mod limits;
mod output;
mod pool;
mod trailing;
#[cfg(feature = "zlib")]
pub mod zlib;
#[cfg(feature = "zstd")]
pub mod zstd;

#[cfg(feature = "futures-stream")]
mod stream;

pub use compression::{Compress, Compressing, Compression, Decompress, Decompressing};
pub use error::{Error, Result};
pub use level::Level;
pub use limits::DecompressionLimits;
pub use output::Output;
pub use pool::Pool;
#[cfg(feature = "futures-stream")]
pub use stream::CompressionStream;
pub use trailing::TrailingData;
