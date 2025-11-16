// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::{Error, ErrorExt, ErrorTraceExt, OhnoCore, assert_error_message};

#[derive(Error)]
#[display("Failed to read config with path: {path}")]
struct ConfigError {
    path: String, // Changed to String to avoid Display issues
    inner_error: OhnoCore,
}

#[test]
fn test_display_error_override_with_empty_core() {
    let error = ConfigError {
        path: "/etc/config.toml".to_string(),
        inner_error: OhnoCore::default(),
    };

    assert_error_message!(error, "Failed to read config with path: /etc/config.toml");
    assert_eq!(error.message(), "Failed to read config with path: /etc/config.toml");
}

#[test]
fn test_display_error_override_with_field() {
    let error = ConfigError {
        path: "/etc/config.toml".to_string(),
        inner_error: OhnoCore::from("file not found"),
    };

    assert_error_message!(
        error,
        "Failed to read config with path: /etc/config.toml\ncaused by: file not found"
    );
    assert_eq!(
        error.message(),
        "Failed to read config with path: /etc/config.toml\ncaused by: file not found"
    );
}

#[test]
fn test_display_error_override_with_context() {
    let error = ConfigError {
        path: "/tmp/test.conf".to_string(),
        inner_error: OhnoCore::from("permission denied")
            .error_trace("filesystem access failed")
            .error_trace("security check failed"),
    };

    let display = format!("{error}");
    assert!(display.starts_with("Failed to read config with path: /tmp/test.conf\ncaused by: permission denied"));
    assert!(display.contains("filesystem access failed"));
    assert!(display.contains("security check failed"));
    assert_eq!(
        error.message(),
        "Failed to read config with path: /tmp/test.conf\ncaused by: permission denied"
    );
}

#[derive(Error)]
#[display("Static error message")]
struct StaticError {
    inner_error: OhnoCore,
}

#[test]
fn test_display_error_override_static_empty() {
    let error = StaticError {
        inner_error: OhnoCore::default(),
    };

    assert_error_message!(error, "Static error message");
    assert_eq!(error.message(), "Static error message");
}

#[test]
fn test_display_error_override_static() {
    let error = StaticError {
        inner_error: OhnoCore::from("underlying error"),
    };

    assert_error_message!(error, "Static error message\ncaused by: underlying error");
    assert_eq!(error.message(), "Static error message\ncaused by: underlying error");
}

#[derive(Error)]
#[display("Multiple fields: {name} - {code}")]
struct MultiFieldError {
    name: String,
    code: i32,
    inner_error: OhnoCore,
}

#[test]
fn test_display_error_override_multiple_fields() {
    let error = MultiFieldError {
        name: "test".to_string(),
        code: 404,
        inner_error: OhnoCore::from("not found"),
    };

    assert_error_message!(error, "Multiple fields: test - 404\ncaused by: not found");
    assert_eq!(error.message(), "Multiple fields: test - 404\ncaused by: not found");
}

#[test]
fn test_struct_display_with_subfield() {
    #[derive(Debug)]
    struct Data(u32, u32);

    #[ohno::error]
    #[display("Invalid data: {} - {}", data.0, data.1)]
    struct InvalidData {
        data: Data,
    }

    let error = InvalidData::new(Data(123, 456));
    assert_error_message!(error, "Invalid data: 123 - 456");
}

#[test]
fn test_tuple_display_with_subfield() {
    #[derive(Debug)]
    struct Data(u32, u32);

    #[ohno::error]
    #[display("Invalid data: {} - {}", 0.0, 0.1)]
    struct InvalidData(Data);

    let error = InvalidData::new(Data(789, 444));
    assert_error_message!(error, "Invalid data: 789 - 444");
}

#[test]
fn test_mixed_display_syntax() {
    #[derive(Debug)]
    #[expect(dead_code, reason = "Test")]
    struct Code(i32, String);

    #[ohno::error]
    #[display("Operation '{operation}' failed with code {}", code.0)]
    struct MixedDisplayError {
        operation: String,
        code: Code,
    }

    let error = MixedDisplayError::new("test_operation".to_string(), Code(500, "Internal Server Error".to_string()));
    assert_error_message!(error, "Operation 'test_operation' failed with code 500");
}

#[test]
fn test_named_subfields() {
    #[derive(Debug)]
    struct ErrorCode {
        code: i32,
        message: String,
    }

    #[ohno::error]
    #[display("Operation failed with code {} and message '{}'", error_code.code, error_code.message)]
    struct NamedSubfieldError {
        error_code: ErrorCode,
    }

    let error = NamedSubfieldError::new(ErrorCode {
        code: 404,
        message: "Not Found".to_string(),
    });
    assert_error_message!(error, "Operation failed with code 404 and message 'Not Found'");
}

#[test]
fn test_deep_subfields() {
    #[derive(Debug)]
    struct StructType {
        m: &'static str,
    }

    impl StructType {
        fn message(&self) -> &'static str {
            self.m
        }
    }

    #[derive(Debug)]
    struct TupleType(((StructType, &'static str), &'static str), &'static str);

    #[ohno::error]
    #[display("Error {}, {}:{} - {} => {}", t.0.0.0.message(), t.0.0.0.m, t.0.0.1, t.0.1, t.1)]
    struct TestError {
        t: TupleType,
    }

    let t = TupleType(((StructType { m: "Struct" }, "Level0"), "Level1"), "Level2");
    println!("Error {}, {}:{} - {} => {}", t.0.0.0.message(), t.0.0.0.m, t.0.0.1, t.0.1, t.1);

    let error = TestError::new(t);
    assert_error_message!(error, "Error Struct, Struct:Level0 - Level1 => Level2");
}
