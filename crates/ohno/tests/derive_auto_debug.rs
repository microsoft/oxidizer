// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::{Error, OhnoCore};

// Test automatic Debug implementation
#[derive(Error)]
struct SimpleError {
    #[error]
    inner_error: OhnoCore,
}

#[derive(Error)]
#[display("Multiple fields: {name} - {code}")]
struct MultiFieldError {
    name: String,
    code: i32,
    #[error]
    inner_error: OhnoCore,
}

// Test tuple struct with automatic Debug
#[derive(Error)]
struct TupleError(String, #[error] OhnoCore);

// Test unit struct conversion with automatic Debug
#[derive(Error)]
struct UnitError(#[error] OhnoCore);

// Test that no_debug attribute disables automatic Debug
#[derive(Error, Debug)]
#[no_debug]
struct NoDebugError {
    #[error]
    inner_error: OhnoCore,
}

// Test that no_debug works with other attributes
#[derive(Error, Debug, Default)]
#[no_debug]
#[display("Custom message: {code}")]
#[from(std::io::Error)]
struct ComplexNoDebugError {
    code: i32,
    #[error]
    inner_error: OhnoCore,
}

#[test]
fn test_simple_error_debug() {
    let error = SimpleError {
        inner_error: OhnoCore::from("test error"),
    };

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("SimpleError"));
    assert!(debug_str.contains("inner_error"));
}

#[test]
fn test_multi_field_error_debug() {
    let error = MultiFieldError {
        name: "test".to_string(),
        code: 404,
        inner_error: OhnoCore::from("not found"),
    };

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("MultiFieldError"));
    assert!(debug_str.contains("name"));
    assert!(debug_str.contains("test"));
    assert!(debug_str.contains("code"));
    assert!(debug_str.contains("404"));
    assert!(debug_str.contains("inner_error"));
}

#[test]
fn test_tuple_error_debug() {
    let error = TupleError("test".to_string(), OhnoCore::from("error"));

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("TupleError"));
    assert!(debug_str.contains("test"));
}

#[test]
fn test_unit_error_debug() {
    let error = UnitError(OhnoCore::from("unit error"));

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("UnitError"));
}

#[test]
fn test_no_debug_attribute_works() {
    let error = NoDebugError {
        inner_error: OhnoCore::from("no debug test"),
    };

    // Should work with manual Debug derive
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("NoDebugError"));
}

#[test]
fn test_no_debug_with_complex_attributes() {
    let error = ComplexNoDebugError {
        code: 500,
        inner_error: OhnoCore::from("server error"),
    };

    // Should work with manual Debug derive
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("ComplexNoDebugError"));

    // Should still have the display functionality
    let display_str = format!("{error}");
    assert!(display_str.contains("Custom message: 500"));

    // Should still have the From implementation
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
    let _converted: ComplexNoDebugError = io_error.into();
}

#[test]
fn test_debug_with_context() {
    use ohno::ErrorTraceExt;

    let error = SimpleError {
        inner_error: OhnoCore::from("base error")
            .error_span("first context")
            .error_span("second context"),
    };

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("SimpleError"));
    // The inner OhnoCore should contain the context
    assert!(debug_str.contains("context"));
}
