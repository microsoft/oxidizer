// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The macros that generate each format module's public surface.
//!
//! Every format exposes the same types and functions, differing only in which codec they drive and
//! in their documentation. Generating them keeps the modules honest -- a change to the contract
//! cannot drift between formats -- without collapsing them into one type that would lose the
//! compile-time distinction between, say, a gzip and a brotli compressor.
//!
//! # What a format module exposes
//!
//! A `Compressor` is reached through its builder and driven through
//! [`Compression`][crate::core::Compression]; it has no inherent
//! operations of its own. That is what lets code be written once against the trait and used with
//! any format, including a boxed one whose format was chosen at runtime.
//!
//! # Format-specific settings
//!
//! The builders themselves are shared: [`CompressorBuilder`][crate::CompressorBuilder] and
//! [`DecompressorBuilder`][crate::DecompressorBuilder] carry every setting that means the same
//! thing in every format. What differs is the type parameter, which names the format and carries
//! whatever settings only that format has -- brotli's quality, window and content mode, zstd's
//! native levels and decompressor window limit. A format with no extra settings uses a unit struct.
//!
//! Each format's module owns that type, the setters that populate it, and the `build` method that
//! consumes it, so nothing outside the module has to know which formats a build enabled.
//!
//! # Fallible builds
//!
//! Most engines take their configuration without validating it, so their builders cannot fail.
//! Brotli and zstd validate as they apply it, so theirs return a [`BuildError`][crate::BuildError].
//! Each format declares which of the two it is and gets the matching signatures.

/// Generates one format's compressor builds and its whole-buffer `compress`.
///
/// The leading token selects whether the engine can reject its configuration.
macro_rules! define_compressor_build {
    (infallible, $name:literal, $format:ty, $build_method:ident, $new_compressor:expr) => {
        impl Compressor {
            #[doc = concat!("Creates a ", $name, " compressor at [`Level::DEFAULT`][crate::Level::DEFAULT].")]
            #[must_use]
            pub fn new(resources: &$crate::Resources) -> Self {
                Self::builder().build(resources)
            }
        }

        impl $crate::CompressorBuilder<$format> {
            /// Builds the compressor, drawing its memory and engine state from `resources`.
            #[must_use]
            pub fn build(self, resources: &$crate::Resources) -> Compressor {
                Compressor {
                    pump: Pump::new(resources.memory().clone(), self.chunk_size),
                    codec: $new_compressor(self.level, &self.format, resources.pool().clone()),
                }
            }
        }

        impl $crate::CompressorBuilder<()> {
            #[doc = concat!("Builds a ", $name, " compressor from the format-independent settings.")]
            ///
            /// Everything this builder carries means the same thing in every format, so committing
            /// to one here rather than up front costs nothing.
            #[must_use]
            pub fn $build_method(self, resources: &$crate::Resources) -> Compressor {
                self.specialize(<$format>::new()).build(resources)
            }
        }

        #[doc = concat!("Compresses a complete byte sequence into ", $name, ".")]
        ///
        /// Uses [`Level::DEFAULT`][crate::Level::DEFAULT], and recycles engine state through
        /// `pool`; pass [`Pool::disabled`][crate::Pool::disabled] to recycle nothing. Prefer
        /// [`Compressor`] for data that arrives incrementally; this convenience buffers the entire
        /// result before returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the underlying compression engine fails.
        pub fn compress(input: BytesView, resources: &$crate::Resources) -> Result<BytesView> {
            Compressor::new(resources).compress(input)
        }
    };
    (fallible, $name:literal, $format:ty, $build_method:ident, $new_compressor:expr) => {
        impl Compressor {
            #[doc = concat!("Creates a ", $name, " compressor at [`Level::DEFAULT`][crate::Level::DEFAULT].")]
            ///
            /// # Panics
            ///
            /// Never in practice: the default settings are inside the ranges the engine accepts, so
            /// it has nothing to reject. Build through [`Compressor::builder`] to handle a
            /// rejection of settings that are not the defaults.
            #[must_use]
            pub fn new(resources: &$crate::Resources) -> Self {
                Self::builder()
                    .build(resources)
                    .expect("the default settings are inside the engine's own ranges, so it cannot reject them")
            }
        }

        impl $crate::CompressorBuilder<$format> {
            /// Builds the compressor, drawing its memory and engine state from `resources`.
            ///
            /// # Errors
            ///
            /// Returns a [`BuildError`][crate::BuildError] if the engine rejects the configuration.
            pub fn build(self, resources: &$crate::Resources) -> ::core::result::Result<Compressor, $crate::BuildError> {
                Ok(Compressor {
                    pump: Pump::new(resources.memory().clone(), self.chunk_size),
                    codec: $new_compressor(self.level, &self.format, resources.pool().clone())?,
                })
            }
        }

        impl $crate::CompressorBuilder<()> {
            #[doc = concat!("Builds a ", $name, " compressor from the format-independent settings.")]
            ///
            /// Everything this builder carries means the same thing in every format, so committing
            /// to one here rather than up front costs nothing.
            ///
            /// # Errors
            ///
            /// Returns a [`BuildError`][crate::BuildError] if the engine rejects the configuration.
            pub fn $build_method(self, resources: &$crate::Resources) -> ::core::result::Result<Compressor, $crate::BuildError> {
                self.specialize(<$format>::new()).build(resources)
            }
        }

        #[doc = concat!("Compresses a complete byte sequence into ", $name, ".")]
        ///
        /// Uses [`Level::DEFAULT`][crate::Level::DEFAULT], and recycles engine state through
        /// `pool`; pass [`Pool::disabled`][crate::Pool::disabled] to recycle nothing. Prefer
        /// [`Compressor`] for data that arrives incrementally; this convenience buffers the entire
        /// result before returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the underlying compression engine fails.
        pub fn compress(input: BytesView, resources: &$crate::Resources) -> Result<BytesView> {
            Compressor::new(resources).compress(input)
        }
    };
}

/// Generates one format's decompressor builds, its `decompress` and its `decompress_with_limits`.
macro_rules! define_decompressor_build {
    (
        infallible,
        $name:literal,
        $format:ty,
        $build_method:ident,
        $new_decompressor:expr,
        $default_limits:expr,
        $multi_stream_default:expr
    ) => {
        impl Decompressor {
            #[doc = concat!("Creates a ", $name, " decompressor with this format's default bounds.")]
            #[must_use]
            pub fn new(resources: &$crate::Resources) -> Self {
                Self::builder().build(resources)
            }
        }

        impl $crate::DecompressorBuilder<$format> {
            /// Builds the decompressor, drawing its memory and engine state from `resources`.
            #[must_use]
            pub fn build(self, resources: &$crate::Resources) -> Decompressor {
                Decompressor {
                    pump: Pump::new(resources.memory().clone(), self.chunk_size),
                    codec: $new_decompressor(
                        self.limits.resolve($default_limits),
                        self.multi_stream.unwrap_or($multi_stream_default),
                        self.trailing_data,
                        &self.format,
                        resources.pool().clone(),
                    ),
                }
            }
        }

        impl $crate::DecompressorBuilder<()> {
            #[doc = concat!("Builds a ", $name, " decompressor from the format-independent settings.")]
            ///
            /// Everything this builder carries means the same thing in every format, so committing
            /// to one here rather than up front costs nothing. Bounds left unset, and a
            /// multi-stream policy left unset, keep this format's own defaults.
            #[must_use]
            pub fn $build_method(self, resources: &$crate::Resources) -> Decompressor {
                self.specialize(<$format>::new()).build(resources)
            }
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream that is already in memory.")]
        ///
        /// Applies this format's default bounds, and recycles engine state through `pool`; pass
        /// [`Pool::disabled`][crate::Pool::disabled] to recycle nothing. Prefer [`Decompressor`]
        /// for data that arrives incrementally; this convenience buffers the entire result before
        /// returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the data is malformed, truncated, or exceeds the default limits.
        pub fn decompress(input: BytesView, resources: &$crate::Resources) -> Result<BytesView> {
            Decompressor::new(resources).decompress(input)
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream with explicit limits.")]
        ///
        /// This is the convenient path for untrusted in-memory input.
        ///
        /// # Errors
        ///
        /// Returns an error if the data is malformed, truncated, or exceeds `limits`.
        pub fn decompress_with_limits(input: BytesView, resources: &$crate::Resources, limits: DecompressionLimits) -> Result<BytesView> {
            Decompressor::builder().limits(limits).build(resources).decompress(input)
        }
    };
    (
        fallible,
        $name:literal,
        $format:ty,
        $build_method:ident,
        $new_decompressor:expr,
        $default_limits:expr,
        $multi_stream_default:expr
    ) => {
        impl Decompressor {
            #[doc = concat!("Creates a ", $name, " decompressor with this format's default bounds.")]
            ///
            /// # Panics
            ///
            /// Never in practice: the default settings are inside the ranges the engine accepts, so
            /// it has nothing to reject. Build through [`Decompressor::builder`] to handle a
            /// rejection of settings that are not the defaults.
            #[must_use]
            pub fn new(resources: &$crate::Resources) -> Self {
                Self::builder()
                    .build(resources)
                    .expect("the default settings are inside the engine's own ranges, so it cannot reject them")
            }
        }

        impl $crate::DecompressorBuilder<$format> {
            /// Builds the decompressor, drawing its memory and engine state from `resources`.
            ///
            /// # Errors
            ///
            /// Returns a [`BuildError`][crate::BuildError] if the engine rejects the configuration.
            pub fn build(self, resources: &$crate::Resources) -> ::core::result::Result<Decompressor, $crate::BuildError> {
                Ok(Decompressor {
                    pump: Pump::new(resources.memory().clone(), self.chunk_size),
                    codec: $new_decompressor(
                        self.limits.resolve($default_limits),
                        self.multi_stream.unwrap_or($multi_stream_default),
                        self.trailing_data,
                        &self.format,
                        resources.pool().clone(),
                    )?,
                })
            }
        }

        impl $crate::DecompressorBuilder<()> {
            #[doc = concat!("Builds a ", $name, " decompressor from the format-independent settings.")]
            ///
            /// Everything this builder carries means the same thing in every format, so committing
            /// to one here rather than up front costs nothing. Bounds left unset, and a
            /// multi-stream policy left unset, keep this format's own defaults.
            ///
            /// # Errors
            ///
            /// Returns a [`BuildError`][crate::BuildError] if the engine rejects the configuration.
            pub fn $build_method(self, resources: &$crate::Resources) -> ::core::result::Result<Decompressor, $crate::BuildError> {
                self.specialize(<$format>::new()).build(resources)
            }
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream that is already in memory.")]
        ///
        /// Applies this format's default bounds, and recycles engine state through `pool`; pass
        /// [`Pool::disabled`][crate::Pool::disabled] to recycle nothing. Prefer [`Decompressor`]
        /// for data that arrives incrementally; this convenience buffers the entire result before
        /// returning.
        ///
        /// # Errors
        ///
        /// Returns an error if the decompressor cannot be built, or if the data is malformed,
        /// truncated, or exceeds the default limits.
        pub fn decompress(input: BytesView, resources: &$crate::Resources) -> Result<BytesView> {
            Decompressor::new(resources).decompress(input)
        }

        #[doc = concat!("Decompresses a complete ", $name, " stream with explicit limits.")]
        ///
        /// This is the convenient path for untrusted in-memory input.
        ///
        /// # Errors
        ///
        /// Returns an error if the decompressor cannot be built, or if the data is malformed,
        /// truncated, or exceeds `limits`.
        pub fn decompress_with_limits(input: BytesView, resources: &$crate::Resources, limits: DecompressionLimits) -> Result<BytesView> {
            Decompressor::builder().limits(limits).build(resources)?.decompress(input)
        }
    };
}

/// Generates `Compressor`, `Decompressor`, their builder aliases, `compress`, `decompress`, and
/// `decompress_with_limits` for one format.
macro_rules! define_format {
    (
        name = $name:literal,
        format = $format:ty,
        build_method = $build_method:ident,
        compressor_codec = $compressor_codec:ty,
        compressor_build = $compressor_build:tt,
        new_compressor = $new_compressor:expr,
        decompressor_codec = $decompressor_codec:ty,
        decompressor_build = $decompressor_build:tt,
        default_limits = $default_limits:expr,
        new_decompressor = $new_decompressor:expr,
        multi_stream_default = $multi_stream_default:expr,
    ) => {
        use bytesbuf::BytesView;
        // Anonymous because the import exists only to bring the trait's methods into scope.
        use $crate::core::Compression as _;
        use $crate::engine::Pump;
        use $crate::error::Result;
        use $crate::limits::DecompressionLimits;
        use $crate::output::Output;

        #[doc = concat!("Configures a ", $name, " compressor.")]
        ///
        /// An alias for the shared [`CompressorBuilder`][crate::CompressorBuilder] once it has
        /// committed to this format, which is what gives it this format's own settings and a
        /// `build` method returning this module's [`Compressor`].
        pub type CompressorBuilder = $crate::CompressorBuilder<$format>;

        #[doc = concat!("Configures a ", $name, " decompressor.")]
        ///
        /// An alias for the shared [`DecompressorBuilder`][crate::DecompressorBuilder] once it has
        /// committed to this format, which is what gives it this format's own settings and a
        /// `build` method returning this module's [`Decompressor`].
        pub type DecompressorBuilder = $crate::DecompressorBuilder<$format>;

        impl Default for CompressorBuilder {
            fn default() -> Self {
                $crate::CompressorBuilder::with_format(<$format>::new())
            }
        }

        impl Default for DecompressorBuilder {
            fn default() -> Self {
                $crate::DecompressorBuilder::with_format(<$format>::new())
            }
        }

        #[doc = concat!("Compresses a stream of byte sequences into ", $name, ".")]
        ///
        /// A push/pull state machine, driven through [`Compression`][crate::core::Compression]:
        /// supply input with [`push`][crate::core::Compression::push], take output with
        /// [`pull`][crate::core::Compression::pull], and call
        /// [`end_input`][crate::core::Compression::end_input] when there is no more input. Each pull
        /// returns at most one bounded chunk, so a stream of any length can be compressed with a
        /// bounded working set.
        ///
        /// The operations live on the trait rather than here, so code written against it works with
        /// every format, and with a boxed compressor whose format was picked at runtime.
        #[derive(Debug)]
        pub struct Compressor {
            pump: Pump,
            codec: $compressor_codec,
        }

        impl Compressor {
            /// Starts configuring a compressor.
            #[must_use]
            pub fn builder() -> CompressorBuilder {
                CompressorBuilder::default()
            }
        }

        impl $crate::core::sealed::Compression for Compressor {}

        impl $crate::core::Compression for Compressor {
            type Mode = $crate::core::Compress;

            fn push(&mut self, input: BytesView) -> Result<()> {
                self.pump.push(input)
            }

            fn end_input(&mut self) {
                self.pump.end_input();
            }

            fn pull(&mut self) -> Result<Output> {
                self.pump.pull(&mut self.codec)
            }

            fn total_in(&self) -> u64 {
                self.pump.total_in()
            }

            fn total_out(&self) -> u64 {
                self.pump.total_out()
            }

            fn flush(&mut self) -> Result<()> {
                self.pump.flush()
            }
        }

        $crate::format::macros::define_compressor_build! {
            $compressor_build, $name, $format, $build_method, $new_compressor
        }

        #[doc = concat!("Decompresses a ", $name, " stream into a stream of byte sequences.")]
        ///
        /// Driven through [`Compression`][crate::core::Compression], like every other format's
        /// decompressor.
        ///
        /// # Security
        ///
        /// Compressed data can expand by orders of magnitude, so a decompressor pointed at untrusted
        /// input is a memory-exhaustion vector. This format's own default bounds apply unless
        /// [`DecompressorBuilder::limits`][crate::DecompressorBuilder::limits] overrides them.
        ///
        /// Output is provisional until [`Output::Done`], because a checksum or trailer can reject
        /// the stream after earlier chunks have been returned.
        #[derive(Debug)]
        pub struct Decompressor {
            pump: Pump,
            codec: $decompressor_codec,
        }

        impl Decompressor {
            /// Starts configuring a decompressor.
            #[must_use]
            pub fn builder() -> DecompressorBuilder {
                DecompressorBuilder::default()
            }
        }

        impl $crate::core::sealed::Compression for Decompressor {}

        impl $crate::core::Compression for Decompressor {
            type Mode = $crate::core::Decompress;

            fn push(&mut self, input: BytesView) -> Result<()> {
                self.pump.push(input)
            }

            fn end_input(&mut self) {
                self.pump.end_input();
            }

            fn pull(&mut self) -> Result<Output> {
                self.pump.pull(&mut self.codec)
            }

            fn total_in(&self) -> u64 {
                self.pump.total_in()
            }

            fn total_out(&self) -> u64 {
                self.pump.total_out()
            }
        }

        $crate::format::macros::define_decompressor_build! {
            $decompressor_build,
            $name,
            $format,
            $build_method,
            $new_decompressor,
            $default_limits,
            $multi_stream_default
        }
    };
}

pub(crate) use {define_compressor_build, define_decompressor_build, define_format};
