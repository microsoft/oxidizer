// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

/// Test that our automatic Debug implementation produces identical output to standard #[derive(Debug)]
use ohno::{Error, OhnoCore};

#[test]
fn test_named_struct_debug_structure() {
    #[derive(Debug)]
    pub struct RefNamedStruct {
        pub inner: OhnoCore,
        pub code: i32,
        pub message: String,
    }

    #[derive(Error)]
    pub struct TestNamedStruct {
        #[error]
        pub inner: OhnoCore,
        pub code: i32,
        pub message: String,
    }

    let ref_struct = RefNamedStruct {
        inner: OhnoCore::builder().error("test_error").build(),
        code: 404,
        message: "Not found".to_string(),
    };
    let ref_debug = format!("{ref_struct:?}").replace("RefNamedStruct", "TestNamedStruct");

    let test_struct = TestNamedStruct {
        inner: ref_struct.inner,
        code: ref_struct.code,
        message: ref_struct.message,
    };

    let test_debug = format!("{test_struct:?}");
    assert_eq!(ref_debug, test_debug);
}

#[test]
fn test_tuple_struct_debug_structure() {
    #[derive(Debug)]
    pub struct RefTupleStruct(pub OhnoCore, pub String, pub i32);

    #[derive(Error)]
    pub struct TestTupleStruct(#[error] pub OhnoCore, pub String, pub i32);

    let ref_struct = RefTupleStruct(
        OhnoCore::builder().error("error_content").build(),
        "additional_info".to_string(),
        42,
    );

    let ref_debug = format!("{ref_struct:?}").replace("RefTupleStruct", "TestTupleStruct");

    let test_struct = TestTupleStruct(ref_struct.0, ref_struct.1, ref_struct.2);

    let test_debug = format!("{test_struct:?}");
    assert_eq!(ref_debug, test_debug);
}

#[test]
fn test_single_field_struct_debug_structure() {
    #[derive(Debug)]
    pub struct RefSingleField {
        pub value: OhnoCore,
    }

    #[derive(Error)]
    pub struct TestSingleField {
        #[error]
        pub value: OhnoCore,
    }

    let ref_struct = RefSingleField {
        value: OhnoCore::builder().error("single_value").build(),
    };

    let ref_debug = format!("{ref_struct:?}").replace("RefSingleField", "TestSingleField");

    let test_struct = TestSingleField { value: ref_struct.value };

    let test_debug = format!("{test_struct:?}");
    assert_eq!(ref_debug, test_debug);
}

#[test]
fn test_struct_with_enum_field_debug_structure() {
    #[derive(Debug)]
    #[expect(dead_code, reason = "Test")]
    enum TestStatus {
        Ok,
        NotFound,
        Error(String),
    }

    #[derive(Debug)]
    struct RefEnumFieldStruct {
        error: OhnoCore,
        status: TestStatus,
    }

    #[derive(Error)]
    struct TestEnumFieldStruct {
        #[error]
        error: OhnoCore,
        status: TestStatus,
    }

    let ref_struct = RefEnumFieldStruct {
        error: OhnoCore::builder().error("enum_error").build(),
        status: TestStatus::Error("fail".to_string()),
    };
    let ref_debug = format!("{ref_struct:?}").replace("RefEnumFieldStruct", "TestEnumFieldStruct");

    let test_struct = TestEnumFieldStruct {
        error: ref_struct.error,
        status: ref_struct.status,
    };
    let test_debug = format!("{test_struct:?}");
    assert_eq!(ref_debug, test_debug);
}
