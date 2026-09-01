// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The deflate family: raw deflate, zlib and gzip.
//!
//! All three wrap the same deflate payload, differing only in framing, so the `deflate`, `zlib` and
//! `gzip` modules share one codec implementation, parameterised by [`Wrapper`].

pub(crate) mod codec;

use flate2::{Compress, Compression, Decompress};

use crate::level::Level;
use crate::limits::FormatLimits;

/// The deflate family's default bounds.
///
/// Deflate cannot expand its input by more than about 1032x -- a structural property of the format,
/// not a tuning choice -- so a single stream is inherently bounded. Measured worst case for 1 MiB of
/// zeros is 1015x, so this sits just above what the format can actually produce and never rejects
/// data deflate could legitimately have generated. No cap on total size, so large streams decompress.
pub(crate) const DEFAULT_LIMITS: FormatLimits = FormatLimits::new(Some(1_100), None);

/// The deflate window size exponent. 15 is the maximum, giving the best compression ratio.
///
/// Only the gzip container needs it explicitly; the raw and zlib constructors default to it.
#[cfg(feature = "gzip")]
const WINDOW_BITS: u8 = 15;

/// The container framing wrapped around a deflate payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Wrapper {
    /// Raw deflate (RFC 1951): no header and no checksum.
    #[cfg(feature = "deflate")]
    Raw,
    /// zlib (RFC 1950): a two byte header and an Adler-32 trailer.
    #[cfg(feature = "zlib")]
    Zlib,
    /// gzip (RFC 1952): a ten byte header and a CRC-32 plus length trailer.
    #[cfg(feature = "gzip")]
    Gzip,
}

impl Wrapper {
    pub(crate) fn compressor(self, level: Level) -> Compress {
        let compression = Compression::new(u32::from(level.get()));

        match self {
            #[cfg(feature = "deflate")]
            Self::Raw => Compress::new(compression, false),
            #[cfg(feature = "zlib")]
            Self::Zlib => Compress::new(compression, true),
            #[cfg(feature = "gzip")]
            Self::Gzip => Compress::new_gzip(compression, WINDOW_BITS),
        }
    }

    pub(crate) fn decompressor(self) -> Decompress {
        match self {
            #[cfg(feature = "deflate")]
            Self::Raw => Decompress::new(false),
            #[cfg(feature = "zlib")]
            Self::Zlib => Decompress::new(true),
            #[cfg(feature = "gzip")]
            Self::Gzip => Decompress::new_gzip(WINDOW_BITS),
        }
    }

    /// Whether a recycled decompressor keeps this container's framing after a reset.
    ///
    /// `Decompress::reset` takes a boolean selecting raw deflate or zlib, so it cannot express
    /// gzip, which the engine compresses as `window_bits + 16`. Recycling a gzip decompressor would
    /// silently drop it to raw deflate, so gzip decompressors are never pooled.
    pub(crate) fn reset_restores_framing(self) -> bool {
        match self {
            #[cfg(feature = "deflate")]
            Self::Raw => true,
            #[cfg(feature = "zlib")]
            Self::Zlib => true,
            #[cfg(feature = "gzip")]
            Self::Gzip => false,
        }
    }

    /// The boolean `Decompress::reset` needs to restore this container.
    #[cfg(any(feature = "deflate", feature = "zlib"))]
    pub(crate) fn expects_zlib_header(self) -> bool {
        match self {
            #[cfg(feature = "deflate")]
            Self::Raw => false,
            #[cfg(feature = "zlib")]
            Self::Zlib => true,
            #[cfg(feature = "gzip")]
            Self::Gzip => false,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "deflate")]
            Self::Raw => "deflate",
            #[cfg(feature = "zlib")]
            Self::Zlib => "zlib",
            #[cfg(feature = "gzip")]
            Self::Gzip => "gzip",
        }
    }
}

#[cfg(all(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
mod tests {
    use super::*;

    #[test]
    fn every_wrapper_has_a_name() {
        assert_eq!(Wrapper::Raw.name(), "deflate");
        assert_eq!(Wrapper::Zlib.name(), "zlib");
        assert_eq!(Wrapper::Gzip.name(), "gzip");
    }
}
