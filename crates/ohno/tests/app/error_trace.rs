// Copyright (c) Microsoft Corporation.

//! Tests for adding context to errors.

use ohno::app::{OhWell, Result};
use ohno::welp;
use ohno::{enrich_err, EnrichableExt};

#[test]
fn error_trace_ext_simple() {
    let err = welp!("connection failed").enrich("database operation");
    let msg = err.to_string();
    assert!(msg.starts_with("connection failed"));
    assert!(msg.contains("database operation"));
}

#[test]
fn error_trace_ext_with() {
    let user_id = 123;
    let err = welp!("not found").enrich_with(|| format!("failed to load user {user_id}"));
    let msg = err.to_string();
    assert!(msg.starts_with("not found"));
    assert!(msg.contains("failed to load user 123"));
}

#[test]
fn error_trace_ext_mutltiple_layers() {
    let base = welp!("timeout");
    let ctx1 = base.enrich("http request");
    let ctx2 = ctx1.enrich_with(|| "api call");
    let msg = ctx2.to_string();
    assert!(msg.starts_with("timeout"));
    assert!(msg.contains("http request"));
    assert!(msg.contains("api call"));
}

#[test]
fn error_trace_on_result() {
    fn fail() -> Result<i32> {
        Err(welp!("operation failed"))
    }

    let err = fail().ohwell("additional context").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("operation failed"));
    assert!(msg.contains("additional context"));
}

#[test]
fn error_trace_macro_with_simple_message() {
    #[enrich_err("failed to process request")]
    fn fail() -> Result<i32> {
        Err(welp!("invalid input"))
    }

    let err = fail().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid input"));
    assert!(msg.contains("failed to process request"));
}

#[test]
fn error_trace_macro_with_format_args() {
    #[enrich_err("failed to parse value: {}", value)]
    fn parse_value(value: &str) -> Result<i32> {
        value
            .parse::<i32>()
            .map_err(|e| welp!("parse error: {}", e))
    }

    assert_eq!(parse_value("42").unwrap(), 42);

    let err = parse_value("abc").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("parse error"));
    assert!(msg.contains("failed to parse value: abc"));
}

#[test]
fn error_trace_macro_with_multiple_params() {
    #[enrich_err("operation {} failed for user {}", op_name, user_id)]
    fn perform_operation(op_name: &str, user_id: i32, should_fail: bool) -> Result<()> {
        if should_fail {
            return Err(welp!("internal error"));
        }
        Ok(())
    }

    assert!(perform_operation("save", 123, false).is_ok());

    let err = perform_operation("delete", 456, true).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("internal error"));
    assert!(msg.contains("operation delete failed for user 456"));
}

#[test]
fn error_trace_macro_default_message() {
    #[enrich_err]
    fn some_operation(should_fail: bool) -> Result<i32> {
        if should_fail {
            return Err(welp!("something went wrong"));
        }
        Ok(100)
    }

    assert_eq!(some_operation(false).unwrap(), 100);

    let err = some_operation(true).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("something went wrong"));
    assert!(msg.contains("error in function some_operation"));
}
