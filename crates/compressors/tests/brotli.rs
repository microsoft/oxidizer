// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Brotli quality and window ranges.

#![cfg(feature = "brotli")]

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
