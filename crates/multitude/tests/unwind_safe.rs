// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static and behavioral unwind-safety contract tests.

#![allow(clippy::std_instead_of_core, reason = "test code")]

use core::alloc::Layout;
use core::cell::Cell;
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::ptr::NonNull;
use std::panic::catch_unwind;

use allocator_api2::alloc::{Allocator, Global};
use multitude::{Alloc, Arc, Arena, Box, Rc};

fn assert_unwind_safe<T: UnwindSafe>() {}
fn assert_ref_unwind_safe<T: RefUnwindSafe>() {}

trait AmbiguousIfUnwindSafe<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfUnwindSafe<()> for T {}
impl<T: ?Sized + UnwindSafe> AmbiguousIfUnwindSafe<u8> for T {}
fn assert_not_unwind_safe<T: ?Sized>() {
    let _ = <T as AmbiguousIfUnwindSafe<_>>::probe;
}

trait AmbiguousIfRefUnwindSafe<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfRefUnwindSafe<()> for T {}
impl<T: ?Sized + RefUnwindSafe> AmbiguousIfRefUnwindSafe<u8> for T {}
fn assert_not_ref_unwind_safe<T: ?Sized>() {
    let _ = <T as AmbiguousIfRefUnwindSafe<_>>::probe;
}

#[derive(Clone, Default)]
struct InteriorMutableAllocator(Cell<()>);

// SAFETY: allocation and deallocation forward unchanged to `Global`.
unsafe impl Allocator for InteriorMutableAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded under the caller's `Allocator` contract.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[test]
fn unwind_safe_bounds_follow_ownership() {
    assert_unwind_safe::<Arena>();
    assert_ref_unwind_safe::<Arena>();

    assert_unwind_safe::<Box<u64>>();
    assert_ref_unwind_safe::<Box<u64>>();
    assert_unwind_safe::<Arc<u64>>();
    assert_ref_unwind_safe::<Arc<u64>>();
    assert_unwind_safe::<Rc<u64>>();
    assert_ref_unwind_safe::<Rc<u64>>();
    assert_unwind_safe::<Alloc<'static, u64>>();
    assert_ref_unwind_safe::<Alloc<'static, u64>>();

    // Unique owners can carry interior-mutability types across a boundary,
    // but shared owners cannot because another owner could observe the state.
    assert_unwind_safe::<Box<Cell<u64>>>();
    assert_not_ref_unwind_safe::<Box<Cell<u64>>>();
    assert_unwind_safe::<Alloc<'static, Cell<u64>>>();
    assert_not_ref_unwind_safe::<Alloc<'static, Cell<u64>>>();
    assert_not_unwind_safe::<Arc<Cell<u64>>>();
    assert_not_ref_unwind_safe::<Arc<Cell<u64>>>();
    assert_not_unwind_safe::<Rc<Cell<u64>>>();
    assert_not_ref_unwind_safe::<Rc<Cell<u64>>>();

    // The allocator is shared by the arena and detached handles.
    assert_not_unwind_safe::<Arena<InteriorMutableAllocator>>();
    assert_not_ref_unwind_safe::<Arena<InteriorMutableAllocator>>();
    assert_not_unwind_safe::<Box<u64, InteriorMutableAllocator>>();
    assert_not_ref_unwind_safe::<Box<u64, InteriorMutableAllocator>>();
}

#[test]
fn arena_remains_usable_after_caught_allocation_panic() {
    let arena = Arena::new();
    let result = catch_unwind(|| {
        let _ = arena.alloc_box_with::<u64, _>(|| panic!("planned panic"));
    });

    assert!(result.is_err());
    assert_eq!(*arena.alloc(42), 42);
}
