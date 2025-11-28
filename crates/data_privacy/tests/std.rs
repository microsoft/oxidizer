// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_allocation)] // We're deliberately testing Box allocations

use data_privacy::{Classified, DataClass, RedactionEngine};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

// Expected data class for public/standard library types
const PUBLIC: DataClass = DataClass::new("public", "data");

/// Test helper macro for types with both Debug and Display
macro_rules! test_type_with_display {
    ($value:expr) => {{
        let engine = RedactionEngine::default();

        // Test Classified
        assert_eq!($value.data_class(), PUBLIC);

        // Test RedactedDebug matches Debug
        let debug_output = format!("{:?}", $value);
        let mut redacted_debug_output = String::new();
        engine.redacted_debug(&$value, &mut redacted_debug_output).unwrap();
        assert_eq!(
            debug_output, redacted_debug_output,
            "RedactedDebug should match Debug for {:?}",
            $value
        );

        // Test RedactedDisplay matches Display
        let display_output = format!("{}", $value);
        let mut redacted_display_output = String::new();
        engine.redacted_display(&$value, &mut redacted_display_output).unwrap();
        assert_eq!(
            display_output, redacted_display_output,
            "RedactedDisplay should match Display for {:?}",
            $value
        );
    }};
}

/// Test helper macro for types with only Debug (no Display)
macro_rules! test_type_debug_only {
    ($value:expr) => {{
        let engine = RedactionEngine::default();

        // Test Classified
        assert_eq!($value.data_class(), PUBLIC);

        // Test RedactedDebug matches Debug
        let debug_output = format!("{:?}", $value);
        let mut redacted_debug_output = String::new();
        engine.redacted_debug(&$value, &mut redacted_debug_output).unwrap();
        assert_eq!(
            debug_output, redacted_debug_output,
            "RedactedDebug should match Debug for {:?}",
            $value
        );
    }};
}

/// Test helper for unordered collections (HashSet, HashMap) where Debug output order is non-deterministic
macro_rules! test_unordered_collection {
    ($value:expr) => {{
        let engine = RedactionEngine::default();

        // Test Classified
        assert_eq!($value.data_class(), PUBLIC);

        // Test RedactedDebug succeeds (can't compare exact output due to non-deterministic ordering)
        let mut redacted_debug_output = String::new();
        engine.redacted_debug(&$value, &mut redacted_debug_output).unwrap();
        assert!(!redacted_debug_output.is_empty());
    }};
}

#[test]
fn test_all_std_types() {
    // Non-generic types with Display
    test_type_with_display!(String::from("hello"));
    test_type_with_display!("hello");
    test_type_with_display!(true);
    test_type_with_display!(false);
    test_type_with_display!('x');

    // Unit type (debug only)
    test_type_debug_only!(());

    // Integer types
    test_type_with_display!(42_i8);
    test_type_with_display!(42_i16);
    test_type_with_display!(42_i32);
    test_type_with_display!(42_i64);
    test_type_with_display!(42_i128);
    test_type_with_display!(42_isize);

    test_type_with_display!(42_u8);
    test_type_with_display!(42_u16);
    test_type_with_display!(42_u32);
    test_type_with_display!(42_u64);
    test_type_with_display!(42_u128);
    test_type_with_display!(42_usize);

    // Float types
    test_type_with_display!(1.0_f32);
    test_type_with_display!(1.0_f64);

    // Generic types (debug only)
    test_type_debug_only!(vec![1, 2, 3]);
    test_type_debug_only!(Some(42));
    test_type_debug_only!(None::<i32>);
    test_type_debug_only!(&[1, 2, 3][..]);
    test_type_debug_only!(&mut [1, 2, 3][..]);

    // Box (with Display)
    test_type_with_display!(Box::new(42));
    test_type_with_display!(Box::new(String::from("boxed")));

    // Box (debug only for types without Display)
    test_type_debug_only!(Box::new(vec![1, 2, 3]));

    // Result (debug only)
    test_type_debug_only!(Ok::<i32, String>(42));
    test_type_debug_only!(Err::<i32, String>(String::from("error")));

    // Rc (with Display)
    test_type_with_display!(Rc::new(42));
    test_type_with_display!(Rc::new(String::from("rc")));

    // Rc (debug only)
    test_type_debug_only!(Rc::new(vec![1, 2, 3]));

    // Arc (with Display)
    test_type_with_display!(Arc::new(42));
    test_type_with_display!(Arc::new(String::from("arc")));

    // Arc (debug only)
    test_type_debug_only!(Arc::new(vec![1, 2, 3]));

    // Cow (debug only)
    test_type_debug_only!(std::borrow::Cow::Borrowed("borrowed"));
    test_type_debug_only!(std::borrow::Cow::<str>::Owned(String::from("owned")));

    // Collections (debug only)
    test_type_debug_only!(VecDeque::from([1, 2, 3]));
    test_type_debug_only!(LinkedList::from([1, 2, 3]));
    test_type_debug_only!(BTreeSet::from([1, 2, 3]));

    // Unordered collections (order is non-deterministic)
    test_unordered_collection!(HashSet::from([1, 2, 3]));
    test_unordered_collection!(HashMap::from([("key", "value")]));

    // BTreeMap is ordered so we can test exact output
    test_type_debug_only!(BTreeMap::from([("key", "value")]));
}

#[test]
fn test_nested_generic_types() {
    // Test nested types to ensure generic impls work recursively
    let engine = RedactionEngine::default();

    // Vec of Vec
    let nested_vec = vec![vec![1, 2], vec![3, 4]];
    assert_eq!(nested_vec.data_class(), PUBLIC);
    let debug_output = format!("{nested_vec:?}",);
    let mut redacted_output = String::new();
    engine.redacted_debug(&nested_vec, &mut redacted_output).unwrap();
    assert_eq!(debug_output, redacted_output);

    // Option of Vec
    let opt_vec = Some(vec![1, 2, 3]);
    assert_eq!(opt_vec.data_class(), PUBLIC);

    // Box of Box
    let boxed_box = Box::new(Box::new(42));
    assert_eq!(boxed_box.data_class(), PUBLIC);
    let debug_output = format!("{boxed_box:?}",);
    let mut redacted_output = String::new();
    engine.redacted_debug(&boxed_box, &mut redacted_output).unwrap();
    assert_eq!(debug_output, redacted_output);
}

#[test]
fn test_edge_cases() {
    // Empty collections
    test_type_debug_only!(Vec::<i32>::new());
    test_type_debug_only!(VecDeque::<i32>::new());
    test_unordered_collection!(HashMap::<String, String>::new());
    test_type_debug_only!(BTreeMap::<String, String>::new());

    // Empty string
    test_type_with_display!(String::new());
    test_type_with_display!("");

    // Special float values
    test_type_with_display!(f64::NAN);
    test_type_with_display!(f64::INFINITY);
    test_type_with_display!(f64::NEG_INFINITY);

    // Min/max integer values
    test_type_with_display!(i32::MIN);
    test_type_with_display!(i32::MAX);
    test_type_with_display!(u64::MIN);
    test_type_with_display!(u64::MAX);
}
