// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The macro that generates each format module's public surface.
//!
//! Every format exposes the same four types and three functions, differing only in which codec they
//! drive and in their documentation. Generating them keeps the four modules honest -- a change to
//! the contract cannot drift between formats -- without collapsing them into one type that would
//! lose the compile-time distinction between, say, a gzip and a brotli compressor.
//!
//! # Format-specific settings
//!
//! Formats are not actually identical: brotli has native quality, window and content-mode settings,
//! while zstd has native levels and a decompressor window limit. The macro handles that with a
//! `compressor_options` / `decompressor_options` type, defaulted and threaded through to the codec.
//! A format with no extra settings passes `()`; a format that has some declares its own options
//! struct and writes the setters by hand in its own module.
//!
//! Only the portable settings appear on the runtime [`Format`][crate::Format] builders, because a
//! builder that might produce any format cannot honour a setting that only one of them has. Code
//! that needs both a runtime format and a format-specific setting branches on the format, uses the
//! concrete builder, and boxes the result -- which works because a boxed [`Compression`][crate::Compression]
//! is itself a `Compression`.

/// Generates `Compressor`, `CompressorBuilder`, `Decompressor`, `DecompressorBuilder`, `compress`,
/// `decompress`, and `decompress_with_limits` for one format.
macro_rules! define_format {
    (
        name = $name:literal,
        compressor_codec = $compressor_codec:ty,
        compressor_options = $compressor_options:ty,
        new_compressor = $new_compressor:expr,
        decompressor_codec = $decompressor_codec:ty,
        decompressor_options = $decompressor_options:ty,
        default_limits = $default_limits:expr,
        new_decompressor = $new_decompressor:expr,
        multi_stream_default = $multi_stream_default:expr,
        multi_stream_doc = $multi_stream_doc:literal,
    ) => {
        use std::num::NonZeroUsize;

        use bytesbuf::BytesView;
        use bytesbuf::mem::MemoryShared;
        use $crate::TrailingData;
        // Anonymous because the import exists only to bring the trait's provided methods into scope.
        use $crate::compression::Compression as _;
        use $crate::engine::{DEFAULT_CHUNK_SIZE, Pump};
        use $crate::error::Result;
        use $crate::level::Level;
        use $crate::limits::DecompressionLimits;
        use $crate::output::Output;

        #[doc = concat!("Compresses a stream of byte sequences into ", $name, ".")]
        ///
        /// A push/pull state machine: supply input with [`Compressor::push`], take output with
        /// [`Compressor::pull`], and call [`Compressor::end_input`] when there is no more input. Each pull
        /// returns at most one bounded chunk, so a stream of any length can be compressed with a
        /// bounded working set.
        #[derive(Debug)]
        pub struct Compressor {
            pump: Pump,
            codec: $compressor_codec,
        }

        impl Compressor {
            /// Creates a compressor at [`Level::DEFAULT`].
            #[must_use]
            pub fn new(memory: impl MemoryShared) -> Self {
                Self::builder().build(memory)
            }

            /// Starts configuring a compressor.
            #[must_use]
            pub fn builder() -> CompressorBuilder {
                CompressorBuilder::default()
            }

            /// Supplies more uncompressed input.
            ///
            /// # Errors
            ///
            /// Returns an [`Error::is_invalid_state`][crate::Error::is_invalid_state] error if
            /// input is still pending from a previous push, or if [`Compressor::end_input`] has already
            /// been called. Drain pending input with [`Compressor::pull`] until it reports
            /// [`Output::NeedInput`] first.
            pub fn push(&mut self, input: BytesView) -> Result<()> {
                self.pump.push(input)
            }

            /// Requests a resumable flush of all input supplied so far.
            ///
            /// Drain [`Compressor::pull`] until it reports [`Output::NeedInput`] before pushing more
            /// input. Flushing can reduce the compression ratio.
            ///
            /// # Errors
            ///
            /// Returns an invalid-state error after end of input or a previous operation failure.
            pub fn flush(&mut self) -> Result<()> {
                self.pump.flush()
            }

            /// Signals that no further input will be supplied.
            ///
            /// Calling this more than once has no additional effect. Continue pulling until
            /// [`Output::Done`] to finish writing the compressed stream.
            pub fn end_input(&mut self) {
                self.pump.end_input();
            }

            /// Produces the next chunk of compressed output.
            ///
            /// # Errors
            ///
            /// Returns an error if the underlying compression engine fails.
            pub fn pull(&mut self) -> Result<Output> {
                self.pump.pull(&mut self.codec)
            }

            /// The number of uncompressed bytes consumed so far.
            #[must_use]
            pub fn total_in(&self) -> u64 {
                self.pump.total_in()
            }

            /// The number of compressed bytes produced so far.
            #[must_use]
            pub fn total_out(&self) -> u64 {
                self.pump.total_out()
            }
        }

        /// Configures an [`Compressor`].
        #[derive(Debug, Clone)]
        pub struct CompressorBuilder {
            level: Level,
            chunk_size: NonZeroUsize,
            pool: Option<$crate::Pool>,
            /// Settings that only this format has. `()` for formats with none.
            ///
            /// The generated builder never reads this beyond handing it to the codec; the format's
            /// own module adds the setters that populate it.
            options: $compressor_options,
        }

        impl CompressorBuilder {
            #[doc = concat!("Sets the compression level, mapped onto ", $name, "'s native range.")]
            #[must_use]
            pub const fn level(mut self, level: Level) -> Self {
                self.level = level;
                self
            }

            /// Sets how much output a single [`Compressor::pull`] produces before returning.
            ///
            /// This bounds the compressor's working set. Larger chunks reduce per-call overhead;
            /// smaller chunks reduce peak memory and latency.
            #[must_use]
            pub const fn output_chunk_size(mut self, bytes: NonZeroUsize) -> Self {
                self.chunk_size = bytes;
                self
            }

            /// Recycles engine state through a shared [`Pool`][crate::Pool].
            ///
            /// Building a compressor is not free, so a service that compresses many messages should
            /// hand every compressor the same pool. The engine is returned when the compressor is
            /// dropped. Without a pool each compressor builds its own engine, which is the default.
            #[must_use]
            pub fn pool(mut self, pool: $crate::Pool) -> Self {
                self.pool = Some(pool);
                self
            }

            /// Builds the compressor, drawing its output buffers from `memory`.
            #[must_use]
            pub fn build(self, memory: impl MemoryShared) -> Compressor {
                Compressor {
                    pump: Pump::new(memory, self.chunk_size),
                    codec: $new_compressor(self.level, self.options, self.pool),
                }
            }
        }

        impl Default for CompressorBuilder {
            fn default() -> Self {
                Self {
                    level: Level::DEFAULT,
                    chunk_size: NonZeroUsize::new(DEFAULT_CHUNK_SIZE).unwrap_or(NonZeroUsize::MIN),
                    pool: None,
                    options: <$compressor_options>::default(),
                }
            }
        }

        #[doc = concat!("Decompresses a ", $name, " stream into a stream of byte sequences.")]
        ///
        /// # Security
        ///
        /// Compressed data can expand by orders of magnitude, so a decompressor pointed at untrusted
        /// input is a memory-exhaustion vector. This format's own default bounds apply unless
        /// [`DecompressorBuilder::limits`] overrides them.
        ///
        /// Output is provisional until [`Output::Done`], because a checksum or trailer can reject
        /// the stream after earlier chunks have been returned.
        #[derive(Debug)]
        pub struct Decompressor {
            pump: Pump,
            codec: $decompressor_codec,
        }

        impl Decompressor {
            /// Creates a decompressor with default options.
            #[must_use]
            pub fn new(memory: impl MemoryShared) -> Self {
                Self::builder().build(memory)
            }

            /// Starts configuring a decompressor.
            #[must_use]
            pub fn builder() -> DecompressorBuilder {
                DecompressorBuilder::default()
            }

            /// Supplies more compressed input.
            ///
            /// # Errors
            ///
            /// Returns an [`Error::is_invalid_state`][crate::Error::is_invalid_state] error if
            /// input is still pending from a previous push, or if [`Decompressor::end_input`] has already
            /// been called.
            pub fn push(&mut self, input: BytesView) -> Result<()> {
                self.pump.push(input)
            }

            /// Signals that no further input will be supplied.
            ///
            /// If the input ended part-way through a stream, the next [`Decompressor::pull`] reports
            /// [`Error::is_unexpected_end_of_stream`][crate::Error::is_unexpected_end_of_stream].
            pub fn end_input(&mut self) {
                self.pump.end_input();
            }

            /// Produces the next chunk of decompressed output.
            ///
            /// # Errors
            ///
            /// Returns [`Error::is_corrupt_data`][crate::Error::is_corrupt_data] if the input is
            /// malformed, [`Error::is_limit_exceeded`][crate::Error::is_limit_exceeded] if the
            /// configured limits would be exceeded, or
            /// [`Error::is_unexpected_end_of_stream`][crate::Error::is_unexpected_end_of_stream]
            /// if the input ended early.
            pub fn pull(&mut self) -> Result<Output> {
                self.pump.pull(&mut self.codec)
            }

            /// The number of compressed bytes consumed so far.
            #[must_use]
            pub fn total_in(&self) -> u64 {
                self.pump.total_in()
            }

            /// The number of decompressed bytes produced so far.
            #[must_use]
            pub fn total_out(&self) -> u64 {
                self.pump.total_out()
            }

            /// Takes bytes already buffered after a completed single stream.
            ///
            /// # Errors
            ///
            /// Returns an invalid-state error until the decompressor reports [`Output::Done`].
            pub fn take_remainder(&mut self) -> Result<BytesView> {
                self.pump.take_remainder()
            }
        }

        /// Configures a [`Decompressor`].
        #[derive(Debug, Clone)]
        pub struct DecompressorBuilder {
            limits: DecompressionLimits,
            chunk_size: NonZeroUsize,
            multi_stream: bool,
            trailing_data: TrailingData,
            pool: Option<$crate::Pool>,
            /// Settings that only this format has. `()` for formats with none.
            options: $decompressor_options,
        }

        impl DecompressorBuilder {
            #[doc = concat!("Overrides the bounds on how much data decompression may produce.")]
            ///
            /// Bounds left unset on the passed value keep this format's own defaults.
            #[must_use]
            pub const fn limits(mut self, limits: DecompressionLimits) -> Self {
                self.limits = limits;
                self
            }

            /// Sets how much output a single [`Decompressor::pull`] produces before returning.
            #[must_use]
            pub const fn output_chunk_size(mut self, bytes: NonZeroUsize) -> Self {
                self.chunk_size = bytes;
                self
            }

            #[doc = $multi_stream_doc]
            ///
            /// When enabled, any bytes following a complete stream must themselves form another
            /// valid stream; trailing padding is reported as corrupt data. Disable this to stop
            /// after the first stream and preserve already-buffered trailing bytes for
            /// [`Decompressor::take_remainder`].
            #[must_use]
            pub const fn multi_stream(mut self, enabled: bool) -> Self {
                self.multi_stream = enabled;
                self
            }

            /// Sets how a single-stream decompressor handles bytes after the compressed stream.
            ///
            /// In multi-stream mode, subsequent bytes are interpreted as another compressed
            /// stream regardless of this setting.
            #[must_use]
            pub const fn trailing_data(mut self, trailing_data: TrailingData) -> Self {
                self.trailing_data = trailing_data;
                self
            }

            /// Recycles engine state through a shared [`Pool`][crate::Pool].
            ///
            /// The engine is returned when the decompressor is dropped. Without a pool each decompressor
            /// builds its own engine, which is the default. See [`Pool`][crate::Pool] for which
            /// engines are actually recycled.
            #[must_use]
            pub fn pool(mut self, pool: $crate::Pool) -> Self {
                self.pool = Some(pool);
                self
            }

            /// Builds the decompressor, drawing its output buffers from `memory`.
            #[must_use]
            pub fn build(self, memory: impl MemoryShared) -> Decompressor {
                Decompressor {
                    pump: Pump::new(memory, self.chunk_size),
                    codec: $new_decompressor(
                        self.limits.resolve($default_limits),
                        self.multi_stream,
                        self.trailing_data,
                        self.options,
                        self.pool,
                    ),
                }
            }
        }

        impl Default for DecompressorBuilder {
            fn default() -> Self {
                Self {
                    limits: DecompressionLimits::new(),
                    chunk_size: NonZeroUsize::new(DEFAULT_CHUNK_SIZE).unwrap_or(NonZeroUsize::MIN),
                    multi_stream: $multi_stream_default,
                    trailing_data: TrailingData::Preserve,
                    pool: None,
                    options: <$decompressor_options>::default(),
                }
            }
        }

        #[doc = concat!("Compresses a complete byte sequence into ", $name, ".")]
        ///
        /// Uses [`Level::DEFAULT`]. Prefer [`Compressor`] for data that arrives incrementally; this
        /// convenience buffers the entire result before returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the underlying compression engine fails.
        pub fn compress(input: BytesView, memory: impl MemoryShared) -> Result<BytesView> {
            Compressor::new(memory).compress(input)
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream that is already in memory.")]
        ///
        /// Applies this format's default bounds. Prefer [`Decompressor`] for data that arrives
        /// incrementally; this convenience buffers the entire result before returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the data is malformed, truncated, or exceeds the default limits.
        pub fn decompress(input: BytesView, memory: impl MemoryShared) -> Result<BytesView> {
            Decompressor::new(memory).decompress(input)
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream with explicit limits.")]
        ///
        /// This is the convenient path for untrusted in-memory input.
        ///
        /// # Errors
        ///
        /// Returns an error if the data is malformed, truncated, or exceeds `limits`.
        pub fn decompress_with_limits(input: BytesView, memory: impl MemoryShared, limits: DecompressionLimits) -> Result<BytesView> {
            Decompressor::builder().limits(limits).build(memory).decompress(input)
        }
    };
}

pub(crate) use define_format;
