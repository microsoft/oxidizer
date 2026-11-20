// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Portable compression levels and conversions.

use compressors::Level;

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
    assert!(error.to_string().contains("0..=9"), "the message must name the range: {error}");
}

#[test]
fn compression_levels_order_by_effort() {
    assert!(Level::MIN < Level::FAST);
    assert!(Level::FAST < Level::DEFAULT);
    assert!(Level::DEFAULT < Level::HIGH);
}
