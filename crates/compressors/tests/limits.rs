// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallback bounds for decompression.

#![cfg(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]

use std::num::{NonZeroU32, NonZeroU64};

use compressors::DecompressorLimits;

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
