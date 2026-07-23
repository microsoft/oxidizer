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
    #![allow(clippy::items_after_statements, reason = "relocated tests put inner types near use")]
    #![allow(clippy::clone_on_ref_ptr, reason = "relocated tests use .clone() on Arc/Rc")]
    #![allow(dead_code, reason = "relocated helpers retain fields for layout")]
    #![allow(
        unfulfilled_lint_expectations,
        reason = "relocated #[expect] may be fulfilled at file or feature level"
    )]
    #![allow(
        clippy::undocumented_unsafe_blocks,
        reason = "relocated test bodies preserve original safety reasoning"
    )]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "relocated tests group related unsafe ops")]
    #![allow(clippy::cast_possible_truncation, reason = "relocated tests use bounded values")]
    #![allow(clippy::cast_sign_loss, reason = "relocated tests use non-negative values")]
    #![allow(clippy::empty_drop, reason = "relocated tests use empty Drop impls to mark dropability")]
    #![allow(clippy::assertions_on_result_states, reason = "relocated tests deliberately assert error returns")]
    #![allow(clippy::empty_line_after_doc_comments, reason = "relocated test doc-comments")]
    use multitude::Arena;

    #[expect(unused_imports, reason = "relocated tests may reference common helpers")]
    use crate::common;

    #[test]
    fn arena_box_str_serializes_to_string() {
        let arena: Arena = Arena::new();
        let s = arena.alloc_str_box("box-str");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"box-str\"");
    }
}
