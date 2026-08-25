// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "test-util")]

use ohno::{Error, OhnoCore, assert_error_message};

#[test]
fn test_from_attribute_single_type() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct MyError {
        inner: OhnoCore,
        code: u32,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
    let my_err: MyError = io_err.into();

    // Verify the error field is set correctly
    assert_error_message!(my_err, "test error");

    // Verify other fields are defaulted
    assert_eq!(my_err.code, 0);
}

#[test]
fn test_from_attribute_multiple_types() {
    #[derive(Error, Default)]
    #[from(std::io::Error, std::fmt::Error)]
    struct MultiError {
        inner: OhnoCore,
        optional_field: Option<String>,
        count: usize,
    }

    // Test From<std::io::Error>
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "io error");
    let multi_err: MultiError = io_err.into();
    assert_error_message!(multi_err, "io error");
    assert_eq!(multi_err.optional_field, None);
    assert_eq!(multi_err.count, 0);

    // Test From<std::fmt::Error>
    let fmt_err = std::fmt::Error;
    let multi_err: MultiError = fmt_err.into();
    assert_error_message!(multi_err, "an error occurred when formatting an argument");
    assert_eq!(multi_err.optional_field, None);
    assert_eq!(multi_err.count, 0);
}

#[test]
fn test_from_attribute_complex_fields() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct ComplexError {
        inner: OhnoCore,
        data: Vec<u8>,
        flags: bool,
        info: Option<String>,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let complex_err: ComplexError = io_err.into();

    assert_error_message!(complex_err, "access denied");
    assert!(complex_err.data.is_empty());
    assert!(!complex_err.flags);
    assert!(complex_err.info.is_none());
}

#[test]
fn test_from_attribute_with_custom_error_field() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct CustomFieldError {
        #[error]
        error_core: OhnoCore,
        metadata: String,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
    let custom_err: CustomFieldError = io_err.into();

    assert_error_message!(custom_err, "timeout");
    assert!(custom_err.metadata.is_empty());
}
#[test]
fn test_from_attribute_generic_source_type() {
    #[derive(Debug)]
    struct PairError<A, B>(A, B);

    impl<A: std::fmt::Debug, B: std::fmt::Debug> std::fmt::Display for PairError<A, B> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "pair error {:?} and {:?}", self.0, self.1)
        }
    }

    impl<A: std::fmt::Debug, B: std::fmt::Debug> std::error::Error for PairError<A, B> {}

    // The comma inside `<...>` separates generic arguments, not `#[from(...)]` entries.
    #[derive(Error, Default)]
    #[from(PairError<u32, bool>)]
    struct GenericSourceError {
        inner: OhnoCore,
        note: String,
    }

    let pair_err = PairError(7u32, true);
    let generic_err: GenericSourceError = pair_err.into();

    assert_error_message!(generic_err, "pair error 7 and true");
    assert!(generic_err.note.is_empty());
}

#[test]
fn test_from_attribute_initializer_reaches_an_outer_item() {
    // The locals the derive binds initializers to must not shadow an item a later initializer
    // names. `__ohno_field_0` is the name the first local would take.
    #[expect(non_upper_case_globals, reason = "the name is what the test is about")]
    const __ohno_field_0: u32 = 99;

    #[derive(Error)]
    #[from(std::io::Error(first: 1, second: __ohno_field_0))]
    struct ShadowError {
        first: u32,
        second: u32,
        inner: OhnoCore,
    }

    let shadow_err: ShadowError = std::io::Error::other("shadow").into();

    assert_eq!(shadow_err.first, 1);
    assert_eq!(shadow_err.second, 99);
}
