// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for automatic error conversion with ? operator.

use ohno::{app_err, assert_error_message, AppError};

#[test]
fn question_mark_on_io_error() {
    fn read_file() -> Result<String, AppError> {
        let err = Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        err?;
        Ok("abc".to_string())
    }

    let result = read_file();
    let err = result.unwrap_err();
    assert_error_message!(err, "file not found");
    err.find_source::<std::io::Error>().unwrap();
    let _ = err.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
}

#[test]
fn question_mark_on_parse_error() {
    fn parse_number() -> Result<i32, AppError> {
        Ok("not_a_number".parse()?)
    }

    let err = parse_number().unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    err.find_source::<std::num::ParseIntError>().unwrap();
    let _ = err.source().unwrap().downcast_ref::<std::num::ParseIntError>().unwrap();
}

#[test]
fn question_mark_on_constructed_error() {
    fn validate(value: i32) -> Result<i32, AppError> {
        if value < 0 {
            Err(app_err!("negative: {}", value))?;
        }
        Ok(value)
    }

    assert_eq!(validate(10).unwrap(), 10);

    let err = validate(-5).unwrap_err();
    assert_error_message!(err, "negative: -5");
}

#[test]
fn question_mark_in_validation_chain() {
    fn process(x: i32) -> Result<i32, AppError> {
        if x < 0 {
            Err(app_err!("value cannot be negative"))?;
        }
        if x > 100 {
            Err(app_err!("value too large"))?;
        }
        Ok(x * 2)
    }

    assert_eq!(process(50).unwrap(), 100);

    let err1 = process(-1).unwrap_err();
    assert_error_message!(err1, "value cannot be negative");

    let err2 = process(150).unwrap_err();
    assert_error_message!(err2, "value too large");
}
