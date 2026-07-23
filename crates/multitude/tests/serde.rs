// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the optional Serde serialization support.

#![cfg(feature = "serde")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use multitude::Arena;

#[test]
fn arena_arc_str_serializes_to_string() {
    let arena = Arena::new();
    let s = arena.alloc_str_arc("shared");
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"shared\"");
}

#[test]
fn arena_string_serializes_to_string() {
    let arena = Arena::new();
    let mut s = arena.alloc_string();
    s.push_str("growable");
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"growable\"");
}

#[test]
fn arena_string_empty_serializes() {
    let arena = Arena::new();
    let s = arena.alloc_string();
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"\"");
}

#[test]
fn arena_vec_serializes_to_array() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.push(1);
    v.push(2);
    v.push(3);
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, "[1,2,3]");
}

#[test]
fn arena_vec_empty_serializes_to_array() {
    let arena = Arena::new();
    let v: multitude::vec::Vec<u32, _> = arena.alloc_vec();
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, "[]");
}

#[test]
fn arena_vec_of_strings_serializes() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<String>();
    v.push("a".to_string());
    v.push("b".to_string());
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, "[\"a\",\"b\"]");
}

#[test]
fn frozen_arena_slice_serializes_to_array() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec();
    values.extend([1_u32, 2, 3]);
    let values = values.into_boxed_slice();
    assert_eq!(serde_json::to_string(&values).unwrap(), "[1,2,3]");
}

mod from_coverage_extras_serde {
    #![allow(clippy::items_after_statements, reason = "test-local types are declared near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests exercise method-call clone syntax")]
    #![allow(dead_code, reason = "helper fields preserve test layouts")]
    #![allow(unfulfilled_lint_expectations, reason = "expectations depend on active features")]
    #![allow(clippy::undocumented_unsafe_blocks, reason = "unsafe test setup is documented at each call site")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "tests group related unsafe operations")]
    #![allow(clippy::cast_possible_truncation, reason = "test values fit the target type")]
    #![allow(clippy::cast_sign_loss, reason = "test values are non-negative")]
    #![allow(clippy::empty_drop, reason = "empty Drop impls mark drop-sensitive types")]
    #![allow(clippy::assertions_on_result_states, reason = "tests assert error returns directly")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "test documentation is adjacent to declarations")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "common helpers are feature-dependent")]
    use crate::common;

    #[test]
    fn arena_box_str_serializes_to_string() {
        let arena: Arena = Arena::new();
        let s = arena.alloc_str_box("box-str");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"box-str\"");
    }
}
