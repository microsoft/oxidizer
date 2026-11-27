// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zstd compression levels and window ranges.

#![cfg(feature = "zstd")]

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
