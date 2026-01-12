// Copyright (c) Microsoft Corporation.

//! Tests for error chaining and finding sources.

use ohno::app::AppError;

#[test]
fn find_source() {
    #[ohno::error]
    struct OhnoParseError;

    let parse_err = "xyz".parse::<u32>().unwrap_err();
    let parse_err = OhnoParseError::caused_by(parse_err);
    let err = AppError::new(parse_err);

    // find_source
    let _parse_err = err.find_source::<OhnoParseError>().unwrap();
    let _parse_err = err.find_source::<std::num::ParseIntError>().unwrap();

    // source
    let source = err.source().unwrap();
    let _parse_err = source.downcast_ref::<OhnoParseError>().unwrap();
    let source = source.source().unwrap();
    let _parse_err = source.downcast_ref::<std::num::ParseIntError>().unwrap();
}

#[test]
fn no_source() {
    let err = AppError::new("simple error");
    assert!(err.source().is_none());
}
