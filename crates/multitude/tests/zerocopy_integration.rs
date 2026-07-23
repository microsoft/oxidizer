// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the zerocopy integration module.

#![cfg(feature = "zerocopy")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use std::thread;

use multitude::Arena;
use zerocopy::FromZeros;

#[derive(Debug, PartialEq, FromZeros)]
struct Scalars {
    a: u32,
    b: u64,
    c: i16,
}

#[test]
fn alloc_box_scalar_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_box::<u64>();
    assert_eq!(*v, 0);
}

#[test]
fn alloc_box_struct_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_box::<Scalars>();
    assert_eq!(*v, Scalars { a: 0, b: 0, c: 0 });
}

#[test]
fn try_alloc_box_scalar_ok() {
    let arena = Arena::new();
    let v = arena.zerocopy().try_alloc_box::<u32>().unwrap();
    assert_eq!(*v, 0);
}

#[test]
fn alloc_arc_scalar_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_arc::<u64>();
    assert_eq!(*v, 0);
}

#[test]
fn alloc_arc_cross_thread() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_arc::<u64>();
    let v2 = v.clone();
    let h = thread::spawn(move || *v2);
    assert_eq!(*v, 0);
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn try_alloc_arc_scalar_ok() {
    let arena = Arena::new();
    let v = arena.zerocopy().try_alloc_arc::<u32>().unwrap();
    assert_eq!(*v, 0);
}

#[test]
fn alloc_slice_box_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_slice_box::<u32>(8);
    assert_eq!(v.len(), 8);
    assert!(v.iter().all(|&x| x == 0));
}

#[test]
fn alloc_slice_box_empty() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_slice_box::<u64>(0);
    assert_eq!(v.len(), 0);
}

#[test]
fn try_alloc_slice_box_ok() {
    let arena = Arena::new();
    let v = arena.zerocopy().try_alloc_slice_box::<u8>(16).unwrap();
    assert_eq!(v.len(), 16);
    assert!(v.iter().all(|&x| x == 0));
}

#[test]
fn alloc_slice_arc_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_slice_arc::<u32>(10);
    assert_eq!(v.len(), 10);
    assert!(v.iter().all(|&x| x == 0));
}

#[test]
fn alloc_slice_arc_empty() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_slice_arc::<u64>(0);
    assert_eq!(v.len(), 0);
}

#[test]
fn alloc_slice_arc_cross_thread() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc_slice_arc::<u64>(4);
    let v2 = v.clone();
    let h = thread::spawn(move || v2.iter().sum::<u64>());
    assert!(v.iter().all(|&x| x == 0));
    assert_eq!(h.join().unwrap(), 0);
}

#[test]
fn try_alloc_slice_arc_ok() {
    let arena = Arena::new();
    let v = arena.zerocopy().try_alloc_slice_arc::<u8>(64).unwrap();
    assert_eq!(v.len(), 64);
    assert!(v.iter().all(|&x| x == 0));
}

// Windows cannot materialize a 64 KiB-aligned value in its default stack;
// non-slice variants check the same alignment guard there.
#[cfg(not(utc_backend))]
#[derive(FromZeros)]
#[repr(C, align(65536))]
struct OverAligned {
    _data: u8,
}

#[test]
#[cfg(not(utc_backend))]
fn try_alloc_box_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc_box::<OverAligned>();
    assert!(result.is_err());
}

#[test]
#[cfg(not(utc_backend))]
fn try_alloc_arc_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc_arc::<OverAligned>();
    assert!(result.is_err());
}

#[test]
#[cfg(not(utc_backend))]
fn try_alloc_slice_box_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc_slice_box::<OverAligned>(4);
    assert!(result.is_err());
}

#[test]
#[cfg(not(utc_backend))]
fn try_alloc_slice_arc_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc_slice_arc::<OverAligned>(4);
    assert!(result.is_err());
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_box_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc_box::<OverAligned>();
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_arc_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc_arc::<OverAligned>();
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_slice_box_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc_slice_box::<OverAligned>(4);
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_slice_arc_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc_slice_arc::<OverAligned>(4);
}

#[test]
fn zerocopy_view_debug() {
    let arena = Arena::new();
    let view = arena.zerocopy();
    let dbg = format!("{view:?}");
    assert!(dbg.contains("ZerocopyView"));
}

#[test]
fn alloc_ref_scalar_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc::<u64>();
    assert_eq!(*v, 0);
}

#[test]
fn alloc_ref_struct_is_zeroed() {
    let arena = Arena::new();
    let v = arena.zerocopy().alloc::<Scalars>();
    assert_eq!(*v, Scalars { a: 0, b: 0, c: 0 });
}

#[test]
fn try_alloc_ref_scalar_ok() {
    let arena = Arena::new();
    let v = arena.zerocopy().try_alloc::<u32>().unwrap();
    assert_eq!(*v, 0);
}

#[test]
#[cfg(not(utc_backend))]
fn try_alloc_ref_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc::<OverAligned>();
    assert!(result.is_err());
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_ref_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc::<OverAligned>();
}

#[test]
fn alloc_ref_is_mutable() {
    let arena = Arena::new();
    let mut v = arena.zerocopy().alloc::<u32>();
    assert_eq!(*v, 0);
    *v = 42;
    assert_eq!(*v, 42);
}

#[test]
fn alloc_slice_ref_is_zeroed() {
    let arena = Arena::new();
    let s = arena.zerocopy().alloc_slice::<u32>(5);
    assert_eq!(&*s, &[0, 0, 0, 0, 0]);
}

#[test]
fn alloc_slice_ref_empty() {
    let arena = Arena::new();
    let s = arena.zerocopy().alloc_slice::<u64>(0);
    assert!(s.is_empty());
}

#[test]
fn try_alloc_slice_ref_ok() {
    let arena = Arena::new();
    let s = arena.zerocopy().try_alloc_slice::<u16>(3).unwrap();
    assert_eq!(&*s, &[0, 0, 0]);
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
fn try_alloc_slice_ref_over_aligned_returns_err() {
    let arena = Arena::new();
    let result = arena.zerocopy().try_alloc_slice::<OverAligned>(4);
    assert!(result.is_err());
}

#[test]
#[cfg(all(not(target_os = "windows"), not(utc_backend)))]
#[should_panic = "arena allocation failed"]
fn alloc_slice_ref_panics_on_over_aligned() {
    let arena = Arena::new();
    let _ = arena.zerocopy().alloc_slice::<OverAligned>(4);
}

#[test]
fn alloc_slice_ref_is_mutable() {
    let arena = Arena::new();
    let mut s = arena.zerocopy().alloc_slice::<u32>(3);
    s[1] = 99;
    assert_eq!(&*s, &[0, 99, 0]);
}

mod from_coverage_extras_zerocopy {
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
    use crate::common::{self, FailingAllocator, SendFailingAllocator};

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::builder_in(FailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc::<u32>();
    }

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_slice_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::builder_in(FailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc_slice::<u32>(4);
    }

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::builder_in(FailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc_box::<u32>();
    }

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::builder_in(SendFailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc_arc::<u32>();
    }

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_slice_arc_panics_on_failing_allocator() {
        let arena: Arena<SendFailingAllocator> = Arena::builder_in(SendFailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc_slice_arc::<u32>(4);
    }

    #[test]
    #[should_panic(expected = "zerocopy: arena allocation failed")]
    fn zerocopy_view_alloc_slice_box_panics_on_failing_allocator() {
        let arena: Arena<FailingAllocator> = Arena::builder_in(FailingAllocator::new(0)).build();
        let _ = arena.zerocopy().alloc_slice_box::<u32>(4);
    }
}
