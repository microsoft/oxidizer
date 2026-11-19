// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The deflate family: raw deflate, zlib and gzip.
//!
//! All three wrap the same deflate payload, differing only in framing, so the `deflate`, `zlib` and
//! `gzip` modules share one engine implementation, parameterized by [`Wrapper`].

pub(crate) mod codec;

use flate2::{Compress, Compression, Decompress};

use crate::level::Level;
use crate::limits::FormatLimits;

/// The deflate family's default bounds.
///
/// Deflate cannot expand its input by more than about `1032x`. This limit is a structural property
/// of the format, not a tuning choice. The measured worst case for 1 MiB of
/// zeros is `1015x`, so this sits just above what the format can actually produce and never rejects
/// data that deflate can generate. Total output and stream count are open. Whole-buffer functions
/// add limits for accumulated output and concatenated stream count.
pub(crate) const DEFAULT_LIMITS: FormatLimits = FormatLimits::new(Some(1_100), None, None);

/// The deflate window size exponent: the maximum the backend accepts, for the best compression
/// ratio.
///
/// Only the gzip container needs it explicitly. The raw and zlib constructors use it by default.
#[cfg(any(test, feature = "gzip"))]
const WINDOW_BITS: u8 = 15;

/// The container framing wrapped around a deflate payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Wrapper {
    /// Raw deflate (RFC 1951): no header and no checksum.
    #[cfg(any(test, feature = "deflate"))]
    Raw,
    /// zlib (RFC 1950): a two byte header and an Adler-32 trailer.
    #[cfg(any(test, feature = "zlib"))]
    Zlib,
    /// gzip (RFC 1952): a ten byte header and a CRC-32 plus length trailer.
    #[cfg(any(test, feature = "gzip"))]
    Gzip,
}

/// Maps the portable scale onto flate's.
///
/// The identity: both run 0 to 9 and mean the same thing at each step. Named and tested anyway,
/// because "the scales happen to agree" is a mapping decision like any other, and this passthrough
/// is the only thing standing between [`Level`] and the engine.
fn compression_for(level: Level) -> Compression {
    Compression::new(u32::from(level.get()))
}

impl Wrapper {
    pub(crate) fn compressor(self, level: Level) -> Compress {
        let compression = compression_for(level);

        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Raw => Compress::new(compression, false),
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => Compress::new(compression, true),
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => Compress::new_gzip(compression, WINDOW_BITS),
        }
    }

    pub(crate) fn decompressor(self) -> Decompress {
        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Raw => Decompress::new(false),
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => Decompress::new(true),
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => Self::gzip_decompressor(WINDOW_BITS),
        }
    }

    #[cfg(any(test, feature = "gzip"))]
    fn gzip_decompressor(window_bits: u8) -> Decompress {
        #[cfg(test)]
        tests::LAST_GZIP_WINDOW_BITS.set(Some(window_bits));
        // #[gamma::skip(expr.decrement, tag = "equivalent", reason = "The zlib-rs gzip decoder allocates its history at MAX_WBITS independently of this argument; 14 and 15 both enable gzip and retain the full 32 KiB history")]
        Decompress::new_gzip(window_bits)
    }

    /// Whether a recycled decompressor keeps this container's framing after a reset.
    ///
    /// `Decompress::reset` takes a boolean selecting raw deflate or zlib, so it cannot express
    /// gzip, which the engine compresses as `window_bits + 16`. Recycling a gzip decompressor can
    /// silently drop it to raw deflate, so gzip decompressors are never pooled.
    pub(crate) fn reset_restores_framing(self) -> bool {
        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Raw => true,
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => true,
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => false,
        }
    }

    /// The boolean `Decompress::reset` needs to restore this container.
    ///
    /// Only `Raw` and `Zlib` ever reach this call: [`Self::reset_restores_framing`] keeps a gzip
    /// decompressor out of the pool, so `checkout` never asks it to reset.
    #[cfg(any(test, feature = "deflate", feature = "zlib"))]
    pub(crate) fn expects_zlib_header(self) -> bool {
        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Raw => false,
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => true,
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => false,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            #[cfg(any(test, feature = "deflate"))]
            Self::Raw => "deflate",
            #[cfg(any(test, feature = "zlib"))]
            Self::Zlib => "zlib",
            #[cfg(any(test, feature = "gzip"))]
            Self::Gzip => "gzip",
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    std::thread_local! {
        pub(super) static LAST_GZIP_WINDOW_BITS: std::cell::Cell<Option<u8>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn gamma_afternoon_gzip_decoder_requests_the_full_deflate_window() {
        LAST_GZIP_WINDOW_BITS.set(None);
        let _decoder = super::Wrapper::Gzip.decompressor();
        assert_eq!(LAST_GZIP_WINDOW_BITS.get(), Some(15));
    }

    #[test]
    fn nov6_gzip_decoder_retains_long_distance_history_across_small_output_chunks() {
        let mut state = 0x1234_5678_u32;
        let block: Vec<u8> = (0..20_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state.to_be_bytes()[0]
            })
            .collect();
        let payload = [block.as_slice(), block.as_slice()].concat();
        let mut encoded = Vec::with_capacity(payload.len() + 1_024);
        assert_eq!(
            Wrapper::Gzip
                .compressor(Level::HIGH)
                .compress_vec(&payload, &mut encoded, flate2::FlushCompress::Finish)
                .unwrap(),
            flate2::Status::StreamEnd
        );
        assert!(encoded.len() < 30_000, "the encoder must use history more than 16 KiB behind it");

        let mut decoder = Wrapper::Gzip.decompressor();
        let mut plain = Vec::new();
        let mut consumed = 0;
        loop {
            let mut chunk = [0; 1_024];
            let before_in = decoder.total_in();
            let before_out = decoder.total_out();
            let status = decoder
                .decompress(&encoded[consumed..], &mut chunk, flate2::FlushDecompress::None)
                .unwrap();
            let step_in = usize::try_from(decoder.total_in() - before_in).unwrap();
            let step_out = usize::try_from(decoder.total_out() - before_out).unwrap();
            consumed += step_in;
            plain.extend_from_slice(&chunk[..step_out]);
            if status == flate2::Status::StreamEnd {
                break;
            }
            assert!(step_in != 0 || step_out != 0, "the decoder must make progress");
        }
        assert_eq!(consumed, encoded.len());
        assert_eq!(plain, payload);
    }

    use super::*;

    #[test]
    fn every_wrapper_has_a_name() {
        assert_eq!(Wrapper::Raw.name(), "deflate");
        assert_eq!(Wrapper::Zlib.name(), "zlib");
        assert_eq!(Wrapper::Gzip.name(), "gzip");
    }

    #[test]
    fn only_gzip_loses_its_framing_on_reset() {
        assert!(Wrapper::Raw.reset_restores_framing());
        assert!(Wrapper::Zlib.reset_restores_framing());
        assert!(!Wrapper::Gzip.reset_restores_framing());
    }

    #[test]
    fn nov6_only_zlib_requests_a_header_when_resetting_a_decoder() {
        assert!(!Wrapper::Raw.expects_zlib_header());
        assert!(Wrapper::Zlib.expects_zlib_header());
        assert!(!Wrapper::Gzip.expects_zlib_header());
    }

    #[test]
    fn the_level_maps_onto_flates_scale_unchanged() {
        // Brotli and zstd pin the arithmetic of their mappings. This test pins the flate mapping.
        // The identity mapping is easy to
        // replace with a constant without anything noticing.
        assert_eq!(compression_for(Level::MIN).level(), 0, "the floor must line up");
        assert_eq!(compression_for(Level::HIGH).level(), 9, "the ceiling must line up");

        for raw in 0..=Level::MAX.get() {
            let level = Level::new(raw).unwrap();
            assert_eq!(compression_for(level).level(), u32::from(raw), "every step must map to itself");
        }
    }
}
