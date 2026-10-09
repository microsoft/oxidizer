// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API contracts that do not need access to compressor internals.

#![allow(clippy::unwrap_used, reason = "test code")]

#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
use std::num::{NonZeroU32, NonZeroU64};

use bytesbuf::mem::GlobalPool;
#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
use compressors::DecompressorLimits;
use compressors::format::Format;
use compressors::{Level, Resources};

#[test]
fn compression_levels_accept_exactly_the_portable_range() {
    for level in 0..=Level::MAX.get() {
        assert_eq!(Level::new(level).map(Level::get), Some(level));
    }

    assert_eq!(Level::new(10), None);
    assert_eq!(Level::new(u8::MAX), None);
}

#[test]
fn compression_level_conversions_and_named_values_match() {
    assert_eq!(Level::MIN.get(), 0);
    assert_eq!(Level::FAST.get(), 1);
    assert_eq!(Level::DEFAULT.get(), 6);
    assert_eq!(Level::HIGH.get(), 9);
    assert_eq!(Level::MAX, Level::HIGH);
    assert_eq!(Level::default(), Level::DEFAULT);
    assert_eq!(Level::try_from(9).unwrap(), Level::HIGH);
    assert_eq!(u8::from(Level::HIGH), 9);

    let error = Level::try_from(10).unwrap_err();
    assert!(error.is_invalid_configuration(), "got {error}");
    assert!(error.to_string().contains("0..=9"), "the message should name the range: {error}");
}

#[test]
fn compression_levels_order_by_effort() {
    assert!(Level::MIN < Level::FAST);
    assert!(Level::FAST < Level::DEFAULT);
    assert!(Level::DEFAULT < Level::HIGH);
}

#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
#[test]
fn decompressor_limit_fallbacks_fill_only_unset_bounds() {
    let fallbacks = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(100).unwrap())
        .max_output_len(NonZeroU64::new(4096).unwrap())
        .max_streams(NonZeroU64::new(8).unwrap());
    let limits = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(7).unwrap())
        .unbounded_output_len()
        .with_fallbacks(fallbacks);
    let expected = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(7).unwrap())
        .unbounded_output_len()
        .max_streams(NonZeroU64::new(8).unwrap());

    assert_eq!(limits, expected);
}

#[cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
#[test]
fn decompressor_limit_fallbacks_fill_an_unset_output_bound() {
    let fallbacks = DecompressorLimits::new().max_output_len(NonZeroU64::new(4096).unwrap());
    let limits = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(7).unwrap())
        .with_fallbacks(fallbacks);
    let expected = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(7).unwrap())
        .max_output_len(NonZeroU64::new(4096).unwrap());

    assert_eq!(limits, expected);
}

#[test]
fn resources_expose_one_global_instance_and_usable_memory() {
    assert!(
        std::ptr::eq(Resources::global(), Resources::global()),
        "every caller must see the same global resources"
    );

    let resources = Resources::new(GlobalPool::new());
    assert!(
        resources.memory().reserve(16).remaining_capacity() >= 16,
        "the memory provider must be reachable"
    );
    assert!(format!("{resources:?}").contains("Resources"));
}

#[test]
fn every_enabled_runtime_format_round_trips() {
    let payload = b"runtime selected format ".repeat(200);

    for &format in Format::ALL {
        let compressed = compressors::format::compress(format, payload.as_slice(), Resources::global()).unwrap();
        let plain = compressors::format::decompress(format, compressed, Resources::global()).unwrap();

        assert_eq!(plain.to_vec(), payload, "{format:?} failed to round trip");
    }
}

#[test]
fn content_encoding_tokens_round_trip() {
    for &format in Format::ALL {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        assert_eq!(
            Format::from_content_encoding(token),
            Some(format),
            "{format:?} did not survive its own token"
        );
    }
}

#[cfg(all(feature = "deflate", feature = "zlib"))]
#[test]
fn http_deflate_token_means_zlib() {
    assert_eq!(Format::from_content_encoding("deflate"), Some(Format::Zlib));
    assert_eq!(Format::Deflate.content_encoding(), None);
}

#[cfg(feature = "gzip")]
#[test]
fn content_encoding_parsing_is_case_insensitive_and_trims() {
    assert_eq!(Format::from_content_encoding("GZIP"), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("  gzip  "), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("x-gzip"), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("identity"), None);
    assert_eq!(Format::from_content_encoding(""), None);
}

#[cfg(feature = "brotli")]
#[test]
fn brotli_uses_the_br_token() {
    assert_eq!(Format::from_content_encoding("br"), Some(Format::Brotli));
    assert_eq!(Format::Brotli.content_encoding(), Some("br"));
}

#[test]
fn unknown_content_encoding_tokens_are_rejected() {
    assert_eq!(Format::from_content_encoding("compress"), None);
    assert_eq!(Format::from_content_encoding("identity"), None);
}

#[test]
fn format_all_lists_exactly_the_enabled_formats() {
    let expected = usize::from(cfg!(feature = "deflate"))
        + usize::from(cfg!(feature = "zlib"))
        + usize::from(cfg!(feature = "gzip"))
        + usize::from(cfg!(feature = "brotli"))
        + usize::from(cfg!(feature = "zstd"));

    assert_eq!(Format::ALL.len(), expected);
}

#[cfg(feature = "brotli")]
mod brotli {
    use compressors::brotli::{Quality, WindowSize};

    #[test]
    fn every_native_quality_is_representable() {
        for quality in Quality::MIN.get()..=Quality::MAX.get() {
            assert_eq!(Quality::new(quality).map(Quality::get), Some(quality));
        }

        assert_eq!(Quality::new(12), None);
        assert_eq!(Quality::try_from(8).unwrap(), Quality::new(8).unwrap());
        assert_eq!(u8::from(Quality::MAX), 11);

        let error = Quality::try_from(12).unwrap_err();
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn every_valid_window_exponent_is_representable() {
        for exponent in WindowSize::MIN.get()..=WindowSize::MAX.get() {
            assert_eq!(WindowSize::new(exponent).map(WindowSize::get), Some(exponent));
        }

        assert_eq!(WindowSize::new(WindowSize::MIN.get() - 1), None);
        assert_eq!(WindowSize::new(WindowSize::MAX.get() + 1), None);
        assert_eq!(WindowSize::default(), WindowSize::DEFAULT);
        assert_eq!(WindowSize::try_from(20).unwrap(), WindowSize::new(20).unwrap());
        assert_eq!(u8::from(WindowSize::DEFAULT), 22);

        let error = WindowSize::try_from(WindowSize::MAX.get() + 1).unwrap_err();
        assert!(error.is_invalid_configuration(), "got {error}");
    }
}

#[cfg(feature = "zstd")]
mod zstd {
    use compressors::zstd::{CompressionLevel, WindowLog};

    #[test]
    fn native_compression_levels_are_validated() {
        assert!(CompressionLevel::min().get() < 0);
        assert_eq!(CompressionLevel::new(CompressionLevel::min().get()), Some(CompressionLevel::min()));
        assert_eq!(CompressionLevel::new(CompressionLevel::max().get()), Some(CompressionLevel::max()));
        assert_eq!(CompressionLevel::default(), CompressionLevel::DEFAULT);
        assert_eq!(
            CompressionLevel::try_from(CompressionLevel::DEFAULT.get()).unwrap(),
            CompressionLevel::DEFAULT
        );
        assert_eq!(i32::from(CompressionLevel::DEFAULT), CompressionLevel::DEFAULT.get());

        let error = CompressionLevel::try_from(CompressionLevel::max().get().saturating_add(1)).unwrap_err();
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn window_logs_are_validated() {
        assert_eq!(WindowLog::new(WindowLog::MIN.get()), Some(WindowLog::MIN));
        assert_eq!(WindowLog::new(WindowLog::MAX.get()), Some(WindowLog::MAX));
        assert_eq!(WindowLog::new(WindowLog::MIN.get() - 1), None);
        assert_eq!(WindowLog::new(WindowLog::MAX.get() + 1), None);
        assert_eq!(WindowLog::default(), WindowLog::DEFAULT);
        assert_eq!(WindowLog::try_from(WindowLog::DEFAULT.get()).unwrap(), WindowLog::DEFAULT);

        let error = WindowLog::try_from(WindowLog::MIN.get() - 1).unwrap_err();
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn maximum_window_log_matches_the_target_pointer_width() {
        let expected = if usize::BITS == 32 { 30 } else { 31 };
        assert_eq!(WindowLog::MAX.get(), expected);
    }
}
