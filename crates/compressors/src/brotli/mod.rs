// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Brotli (RFC 7932): a general-purpose compressor with a static dictionary tuned for web content.
//!
//! Compresses text noticeably better than [`gzip`][crate::gzip] at comparable speed, which is why
//! it is the usual choice for HTTP `Content-Encoding: br`. Requires the `brotli` cargo feature.
//!
//! Brotli streams carry no magic bytes, so the format has to be known from context, such as a
//! `Content-Encoding` header.
//!
//! # Examples
//!
//! ```
//! use bytesbuf::BytesView;
//! use bytesbuf::mem::GlobalPool;
//! use compressors::{Resources, brotli};
//!
//! let memory = GlobalPool::new();
//! let compressed = brotli::compress(
//!     BytesView::copied_from_slice(b"the quick brown fox", &memory),
//!     &Resources::default(),
//! )?;
//!
//! assert_eq!(
//!     brotli::decompress(compressed, &Resources::default())?.to_vec(),
//!     b"the quick brown fox".to_vec()
//! );
//! # Ok::<(), compressors::Error>(())
//! ```

mod codec;

use crate::brotli::codec::{BrotliCompress, BrotliDecompress};
use crate::limits::FormatLimits;

/// Brotli's default bounds.
///
/// Brotli has no structural expansion ceiling, so a ratio bound cannot distinguish a bomb from
/// legitimate highly-compressible data. Callers handling untrusted input should set an absolute
/// output limit based on how much data they can afford to buffer.
const DEFAULT_LIMITS: FormatLimits = FormatLimits::new(None, None);
use crate::macros::define_format;

/// Selects brotli as the format of a [`CompressorBuilder`] or [`DecompressorBuilder`], and carries
/// the settings only brotli has.
///
/// Naming the format in the builder's type parameter is what gives that builder a `build` method
/// producing this module's [`Compressor`] and [`Decompressor`], along with the setters below.
#[derive(Debug, Clone)]
pub struct Brotli {
    quality: Option<Quality>,
    mode: Mode,
    window_size: WindowSize,
}

impl Brotli {
    /// The settings a brotli builder starts with: brotli's own defaults, and the portable
    /// [`Level`][crate::Level] left in charge of the quality.
    pub(crate) const fn new() -> Self {
        Self {
            quality: None,
            mode: Mode::Generic,
            window_size: WindowSize::DEFAULT,
        }
    }
}

define_format! {
    name = "brotli",
    format = Brotli,
    build_method = build_brotli,
    compressor_codec = BrotliCompress,
    compressor_build = fallible,
    new_compressor = |level, format, _pool| BrotliCompress::new(level, format),
    decompressor_codec = BrotliDecompress,
    decompressor_build = infallible,
    default_limits = DEFAULT_LIMITS,
    new_decompressor = |limits, multi_stream, trailing_data, _format, _pool| {
        BrotliDecompress::new(limits, multi_stream, trailing_data)
    },
    multi_stream_default = false,
}

/// The kind of data brotli should tune its model for.
///
/// Brotli ships a static dictionary of common web text, and its entropy model can be biased
/// towards a particular kind of input. Choosing correctly is worth a few percent on the ratio;
/// choosing wrongly costs about as much, so leave it at [`Mode::Generic`] unless you know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
pub enum Mode {
    /// No assumption about the input. The default.
    #[default]
    Generic,
    /// UTF-8 text.
    Text,
    /// A WOFF 2.0 font.
    Font,
}

/// A compression quality on brotli's native `0..=11` scale.
///
/// Quality zero is brotli's fastest mode; it still compresses. The portable [`Level`] scale maps
/// onto this range, while this type makes every native quality reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quality(u8);

impl Quality {
    /// Brotli's fastest quality.
    pub const MIN: Self = Self(0);

    /// Brotli's native default.
    pub const DEFAULT: Self = Self(11);

    /// Brotli's strongest quality.
    pub const MAX: Self = Self(11);

    /// Creates a native brotli quality.
    #[must_use]
    pub const fn new(quality: u8) -> Option<Self> {
        if quality <= Self::MAX.0 { Some(Self(quality)) } else { None }
    }

    /// Returns the native brotli quality.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Quality {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for Quality {
    type Error = crate::Error;

    fn try_from(quality: u8) -> core::result::Result<Self, Self::Error> {
        Self::new(quality).ok_or_else(|| {
            crate::Error::invalid_configuration(format!(
                "brotli quality {quality} is out of range; expected {}..={}",
                Self::MIN.get(),
                Self::MAX.get()
            ))
        })
    }
}

impl From<Quality> for u8 {
    fn from(quality: Quality) -> Self {
        quality.get()
    }
}

/// The base-2 logarithm of brotli's sliding window, in bytes.
///
/// A larger window lets the compressor find matches further back, which is what helps on large inputs.
///
/// It is tempting to read this as a memory dial and shrink it to economize. Measurement says
/// otherwise, and in more than one direction. Compressor memory and throughput do not fall off
/// smoothly as the window shrinks: below a threshold the compressor allocates *more* and runs
/// *slower*, so a small window can cost on every axis at once. The ratio is not monotonic either,
/// because a window comparable to the payload can beat a much larger one. Decompressor memory tracks
/// the data actually decompressed rather than the window the compressor declared, so a small window is not
/// a reliable way to spare the reader.
///
/// The practical advice is to leave this alone unless a measurement on real payloads says
/// otherwise.
///
/// This is a newtype rather than a bare `u8` for the same reason [`Level`] is: an out-of-range
/// value is a configuration mistake to report, not a panic to suffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowSize(u8);

impl WindowSize {
    /// The smallest window brotli accepts, 1 KiB.
    pub const MIN: Self = Self(10);

    /// Brotli's default window, 4 MiB.
    pub const DEFAULT: Self = Self(22);

    /// The largest window brotli accepts without the large-window extension, 16 MiB.
    pub const MAX: Self = Self(24);

    /// Creates a window size from its base-2 exponent, or returns `None` outside `10..=24`.
    #[must_use]
    pub const fn new(exponent: u8) -> Option<Self> {
        if exponent < Self::MIN.0 || exponent > Self::MAX.0 {
            return None;
        }

        Some(Self(exponent))
    }

    /// Returns the base-2 exponent.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for WindowSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for WindowSize {
    type Error = crate::Error;

    fn try_from(exponent: u8) -> core::result::Result<Self, Self::Error> {
        Self::new(exponent).ok_or_else(|| {
            crate::Error::invalid_configuration(format!(
                "brotli window size 2^{exponent} is out of range; expected the exponent in {}..={}",
                Self::MIN.get(),
                Self::MAX.get()
            ))
        })
    }
}

impl From<WindowSize> for u8 {
    fn from(window_size: WindowSize) -> Self {
        window_size.get()
    }
}

/// Settings that only brotli has.
///
/// The portable settings -- [`level`][CompressorBuilder::level] and
/// [`output_chunk_size`][CompressorBuilder::output_chunk_size] -- are shared with every other format
/// and are also reachable from a [`CompressorBuilder<()>`][crate::CompressorBuilder] that has not
/// chosen a format yet. These are not: a builder that might produce any format cannot honour a
/// setting only brotli has, so reach for them through this concrete builder and box the result if
/// you need a [`Compression`][crate::core::Compression] trait object.
///
/// # Examples
///
/// ```
/// use compressors::brotli::{self, Mode, Quality, WindowSize};
/// use compressors::core::{Compress, Compression};
/// use compressors::Resources;
///
/// let compressor: Box<dyn Compression<Mode = Compress>> = Box::new(
///     brotli::Compressor::builder()
///         .quality(Quality::new(8).expect("8 is in range"))
///         .mode(Mode::Text)
///         .window_size(WindowSize::new(20).expect("20 is in range"))
///         .build(&Resources::default())?,
/// );
/// # let _ = compressor;
/// # Ok::<(), compressors::BuildError>(())
/// ```
impl CompressorBuilder {
    /// Sets brotli's native quality, overriding any portable [`Level`][crate::Level].
    #[must_use]
    pub const fn quality(mut self, quality: Quality) -> Self {
        self.format.quality = Some(quality);
        self
    }

    /// Tunes the entropy model for a particular kind of input.
    #[must_use]
    pub const fn mode(mut self, mode: Mode) -> Self {
        self.format.mode = mode;
        self
    }

    /// Sets the sliding window size.
    ///
    /// A larger declared window can increase decompressor memory, so raising it is a cost paid by
    /// the reader as well as the writer.
    #[must_use]
    pub const fn window_size(mut self, window_size: WindowSize) -> Self {
        self.format.window_size = window_size;
        self
    }
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn every_native_quality_is_representable() {
        for quality in Quality::MIN.get()..=Quality::MAX.get() {
            assert_eq!(Quality::new(quality).map(Quality::get), Some(quality));
        }

        assert_eq!(Quality::new(12), None);
        assert_eq!(Quality::default(), Quality::DEFAULT);
        assert_eq!(Quality::try_from(8).expect("in range"), Quality::new(8).expect("in range"));
        assert_eq!(u8::from(Quality::MAX), 11);

        let error = Quality::try_from(12).expect_err("out of range");
        assert!(error.is_invalid_configuration(), "got {error}");
    }
}

#[cfg(test)]
mod window_size_tests {
    use super::*;

    #[test]
    fn every_valid_exponent_is_representable() {
        for exponent in WindowSize::MIN.get()..=WindowSize::MAX.get() {
            assert_eq!(WindowSize::new(exponent).map(WindowSize::get), Some(exponent));
        }

        assert_eq!(WindowSize::new(WindowSize::MIN.get() - 1), None);
        assert_eq!(WindowSize::new(WindowSize::MAX.get() + 1), None);
        assert_eq!(WindowSize::default(), WindowSize::DEFAULT);
        assert_eq!(WindowSize::try_from(20).expect("in range"), WindowSize::new(20).expect("in range"));
        assert_eq!(u8::from(WindowSize::DEFAULT), 22);

        let error = WindowSize::try_from(WindowSize::MAX.get() + 1).expect_err("out of range");
        assert!(error.is_invalid_configuration(), "got {error}");
    }
}
