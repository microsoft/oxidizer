// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "test-util")]

use ohno::{EnrichableExt, Error, ErrorExt, OhnoCore, assert_error_message};

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
fn test_display_error_override_with_enrichment() {
    let error = ConfigError {
        path: "/tmp/test.conf".to_string(),
        inner_error: OhnoCore::from("permission denied")
            .enrich("filesystem access failed")
            .enrich("security check failed"),
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
fn test_tuple_index_literal_root() {
    #[ohno::error]
    #[display("Invalid data: {} - {}", 0, 1.abs())]
    struct InvalidData(u32, i32);

    let error = InvalidData::new(789u32, -444i32);
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

#[test]
fn test_raw_identifier_field() {
    // A field name reaches the template as text, so a raw identifier arrives spelled `r#type`.
    // Rebuilding it with `Ident::new` rejects that spelling and panics, turning a template into a
    // macro crash, so the prefix has to survive both the placeholder and the positional argument.
    #[ohno::error]
    #[display("{r#type} at {}", r#type.len())]
    struct TestError {
        r#type: String,
    }

    let error = TestError::new("timeout".to_string());
    assert_error_message!(error, "timeout at 7");
}

#[test]
fn test_documented_fields_stay_referenceable() {
    // The injected core is marked with a reserved doc string, so an ordinary doc comment must not
    // be mistaken for it and hide the field it documents from the display template
    #[ohno::error]
    #[display("{path} failed with {code}")]
    struct TestError {
        /// Where the failure happened.
        path: String,
        /// The ohno generated core field is not this one.
        code: u32,
    }

    let error = TestError::new("/etc/hosts".to_string(), 13_u32);
    assert_error_message!(error, "/etc/hosts failed with 13");
}

/// A count whose `Mul` differs by receiver, so a rendered value says which one ran.
///
/// `&(self.count * 2)` multiplies the field and borrows the result. `&self.count * 2`, the
/// expansion before positional arguments were parenthesized, multiplies a reference instead.
#[derive(Debug, Clone, Copy)]
struct Count(u32);

impl std::ops::Mul<u32> for Count {
    type Output = u32;

    fn mul(self, rhs: u32) -> u32 {
        self.0 * rhs
    }
}

impl std::ops::Mul<u32> for &Count {
    type Output = u32;

    fn mul(self, _: u32) -> u32 {
        0
    }
}

#[test]
fn test_binary_argument_applies_to_the_field_value() {
    #[ohno::error]
    #[display("{}", count * 2)]
    struct TestError {
        count: Count,
    }

    let error = TestError::new(Count(21));

    // 0 is what the reference-side `Mul` returns, so it reports the operator having been applied
    // around the borrow rather than under it
    assert_error_message!(error, "42");
}

#[test]
fn test_cast_argument_applies_to_the_field_value() {
    // Casting the borrow instead of the value is not merely wrong, it does not compile: this is
    // the `casting &u32 as u64 is invalid` that reached users as an error in generated code
    #[ohno::error]
    #[display("{}", size as u64)]
    struct TestError {
        size: u32,
    }

    let error = TestError::new(7_u32);
    assert_error_message!(error, "7");
}

/// A positional argument may call a method of `self`, which is what the diagnostic for an
/// unsupported root promises.
#[derive(Error)]
#[display("{}", describe())]
struct MethodArgumentError {
    code: u32,
    inner: OhnoCore,
}

impl MethodArgumentError {
    fn describe(&self) -> String {
        format!("code {}", self.code)
    }
}

#[test]
fn test_display_argument_calls_a_method_of_self() {
    let error = MethodArgumentError {
        code: 7,
        inner: OhnoCore::default(),
    };

    assert_error_message!(error, "code 7");
}
