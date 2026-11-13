// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize};

use bytesbuf::mem::{BlockMeta, BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable};

struct TestBlock {
    ref_count: AtomicUsize,
    meta: Option<NonNull<dyn BlockMeta>>,
}

#[derive(Debug)]
struct TestBlockMeta {
    label: String,
}

impl BlockMeta for TestBlockMeta {}

// SAFETY: We must ensure thread-safety of the implementation. We do.
unsafe impl BlockRefDynamic for TestBlock {
    type State = Self;

    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: The state pointer is always valid for reads.
        let state = unsafe { state_ptr.as_ref() };

        state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        state_ptr
    }

    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: The state pointer is always valid for reads.
        let state = unsafe { state_ptr.as_ref() };

        state.ref_count.fetch_sub(1, atomic::Ordering::Release);

        // We do not actually deallocate anything - the test logic will do that because it
        // first needs to inspect the block structure to verify the reference count.
    }
}

// SAFETY: We must ensure thread-safety of the implementation. We do.
unsafe impl BlockRefDynamicWithMeta for TestBlock {
    fn meta(state_ptr: NonNull<Self::State>) -> NonNull<dyn BlockMeta> {
        // SAFETY: The state pointer is always valid for reads.
        let state = unsafe { state_ptr.as_ref() };

        state.meta.unwrap()
    }
}

const TEST_BLOCK_REF_FNS: BlockRefVTable<TestBlock> = BlockRefVTable::from_trait_with_meta();

const TEST_BLOCK_REF_FNS_WITHOUT_META: BlockRefVTable<TestBlock> = BlockRefVTable::from_trait();

#[test]
fn smoke_test() {
    let meta_ptr = NonNull::new(Box::into_raw(Box::new(TestBlockMeta {
        label: "Test Block".to_string(),
    })))
    .unwrap();

    let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
        ref_count: AtomicUsize::new(1),
        meta: Some(meta_ptr),
    })))
    .unwrap();

    // SAFETY: block_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. Yep, it does - the dynamic impl type takes ownership.
    let block_ref = unsafe { BlockRef::new(block_ptr, &TEST_BLOCK_REF_FNS) };

    let meta = block_ref.meta().unwrap();

    assert_eq!(meta.downcast_ref::<TestBlockMeta>().unwrap().label, "Test Block");

    let block_ref_clone = block_ref.clone();

    let meta = block_ref_clone.meta().unwrap();

    assert_eq!(meta.downcast_ref::<TestBlockMeta>().unwrap().label, "Test Block");

    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);

    assert_eq!(2, ref_count);

    drop(block_ref_clone);
    drop(block_ref);

    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);

    assert_eq!(0, ref_count);

    // All done, clean up please.
    // SAFETY: Yep, that is our block.
    drop(unsafe { Box::from_raw(block_ptr.as_ptr()) });
    // SAFETY: Yep, that is our meta.
    drop(unsafe { Box::from_raw(meta_ptr.as_ptr()) });
}

#[test]
fn without_meta_returns_none_meta() {
    let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
        ref_count: AtomicUsize::new(1),
        meta: None,
    })))
    .unwrap();

    // SAFETY: block_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. Yep, it does - the dynamic impl type takes ownership.
    let block_ref = unsafe { BlockRef::new(block_ptr, &TEST_BLOCK_REF_FNS_WITHOUT_META) };

    assert!(block_ref.meta().is_none());

    let block_ref_clone = block_ref.clone();

    assert!(block_ref_clone.meta().is_none());

    drop(block_ref_clone);
    drop(block_ref);

    // All done, clean up please.
    // SAFETY: Yep, that is our block.
    drop(unsafe { Box::from_raw(block_ptr.as_ptr()) });
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_trait_with_meta_vtable_handles_clone_correctly() {
    // Create vtable at runtime to ensure we measure test coverage.
    // We leak the Box to get a 'static reference, which is okay as long as Miri is not looking.
    let vtable: &'static BlockRefVTable<TestBlock> = Box::leak(Box::new(BlockRefVTable::from_trait_with_meta()));

    let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
        ref_count: AtomicUsize::new(1),
        meta: None,
    })))
    .unwrap();

    // SAFETY: block_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. It is - we clean it up at the end.
    let block_ref = unsafe { BlockRef::new(block_ptr, vtable) };

    // Clone the block ref
    let block_ref_clone = block_ref.clone();

    // Verify reference count increased
    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);
    assert_eq!(ref_count, 2);

    drop(block_ref_clone);
    drop(block_ref);

    // Verify reference count decreased to 0
    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);
    assert_eq!(ref_count, 0);

    // Clean up
    // SAFETY: Yep, that is our block.
    drop(unsafe { Box::from_raw(block_ptr.as_ptr()) });
}

#[test]
#[cfg_attr(miri, ignore)]
fn from_trait_vtable_handles_clone_correctly() {
    // Create vtable at runtime to ensure we measure test coverage.
    // We leak the Box to get a 'static reference, which is okay as long as Miri is not looking.
    let vtable: &'static BlockRefVTable<TestBlock> = Box::leak(Box::new(BlockRefVTable::from_trait()));

    let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
        ref_count: AtomicUsize::new(1),
        meta: None,
    })))
    .unwrap();

    // SAFETY: block_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. It is - we clean it up at the end.
    let block_ref = unsafe { BlockRef::new(block_ptr, vtable) };

    // Clone the block ref
    let block_ref_clone = block_ref.clone();

    // Verify reference count increased
    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);
    assert_eq!(ref_count, 2);

    drop(block_ref_clone);
    drop(block_ref);

    // Verify reference count decreased to 0
    // SAFETY: That is our block and it is perfectly valid for reads.
    let ref_count = unsafe { block_ptr.as_ref() }.ref_count.load(atomic::Ordering::Relaxed);
    assert_eq!(ref_count, 0);

    // Clean up
    // SAFETY: Yep, that is our block.
    drop(unsafe { Box::from_raw(block_ptr.as_ptr()) });
}
