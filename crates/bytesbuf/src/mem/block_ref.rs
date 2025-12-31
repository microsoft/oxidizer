// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// References a block of memory capacity rented from a memory provider.
///
/// While a memory provider only leases each block to one caller at a time, this caller may further
/// share and subdivide the block between multiple co-owners. These co-owners will coordinate the
/// read/write permissions over different slices of the block via their own logic, with the
/// `BlockRef` only used to represent the block as a whole, each co-owner having a cloned
/// `BlockRef` to the same block.
///
/// # Implementation design
///
/// Each memory provider implements its own accounting logic for tracking the memory blocks it
/// provides. This takes the form of a "manual" dynamic dispatch implementation via a function
/// table and data pointer passed to [`new()`][1].
///
/// You can think of `BlockRef` as an `Arc<RealBlock>`, except we are intentionally obscuring the
/// `RealBlock` from the API surface to allow all code upstream of `BlockRef` to be ignorant of the
/// real type of the block.
///
/// The assumption is that an efficient memory provider will allocate its data objects in a pool,
/// so if the `BlockRef` itself is held on the stack, there are no heap allocations necessary to
/// operate on memory blocks. This would be infeasible to achieve with trait objects, which are
/// unsized and have significant limitations on how they can be used. This is why we use the
/// manual dynamic dispatch mechanism instead of using Rust's trait system.
///
/// [1]: Self::new
#[derive(Debug)]
pub struct BlockRef {
    // Note that this entire object is simply a fat pointer - there is no real state.
    // At rent-time, some state is presented to the `SpanBuilder` that takes ownership
    // of the memory block but this is not preserved in the block reference itself.
    state: OpaqueStatePtr,
    vtable: &'static BlockRefVTableInner,
}

impl BlockRef {
    /// Creates a new block reference using the provided dynamic implementation
    /// state and matching function table.
    ///
    /// # Safety
    ///
    /// `state` must remain valid for reads and writes until `BlockRefDynamic::drop`
    /// is called via `vtable`.
    #[must_use]
    pub const unsafe fn new<T: BlockRefDynamic>(state: NonNull<T::State>, vtable: &'static BlockRefVTable<T>) -> Self {
        Self {
            state: state.cast(),
            vtable: &vtable.inner,
        }
    }

    /// Memory provider specific metadata describing the block.
    #[must_use]
    pub fn meta(&self) -> Option<&dyn Any> {
        self.vtable.meta.map(|f| {
            // SAFETY: We are required to pass the original `state` here. We do.
            let meta_ptr = unsafe { f(self.state) };

            // SAFETY: The implementation is required to return a pointer that is valid for
            // reads for the lifetime of the `BlockRef`, so all is well here
            // because the returned reference borrows the `BlockRef`.
            unsafe { meta_ptr.as_ref() }
        })
    }
}

impl Clone for BlockRef {
    fn clone(&self) -> Self {
        // SAFETY: We are required to pass the original `state` here. We do.
        let new_data = unsafe { (self.vtable.clone)(self.state) };

        Self {
            state: new_data,
            vtable: self.vtable,
        }
    }
}

impl Drop for BlockRef {
    fn drop(&mut self) {
        // SAFETY: We are required to pass the original `state` here. We do.
        unsafe { (self.vtable.drop)(self.state) }
    }
}

type OpaqueStatePtr = NonNull<()>;

// # Safety
//
// These functions must always be called with the original `OpaqueStatePtr` supplied by the memory
// provider when creating the BlockRef (with clones using the clone's `OpaqueStatePtr`, respectively).
type CloneFn = unsafe fn(state: OpaqueStatePtr) -> OpaqueStatePtr;
type DropFn = unsafe fn(state: OpaqueStatePtr);
type MetaFn = unsafe fn(state: OpaqueStatePtr) -> NonNull<dyn Any>;

// SAFETY: The safety requirements of the dynamic implementation traits require thread-safety.
// The type itself consists entirely of data fields treated as read-only, so the
// thread-safety guarantee only relies on the implementation behind the trait being thread-safe.
unsafe impl Send for BlockRef {}

// SAFETY: The safety requirements of the dynamic implementation traits require thread-safety.
// The type itself consists entirely of data fields treated as read-only, so the
// thread-safety guarantee only relies on the implementation behind the trait being thread-safe.
unsafe impl Sync for BlockRef {}

#[derive(Debug)]
struct BlockRefVTableInner {
    clone: CloneFn,
    drop: DropFn,
    meta: Option<MetaFn>,
}

/// Function table that implements [`BlockRef`] for a specific memory provider.
///
/// Wraps a specific memory provider's [`BlockRefDynamic`] or [`BlockRefDynamicWithMeta`]
/// implementation into a form required to construct a [`BlockRef`].
#[derive(Debug)]
pub struct BlockRefVTable<T> {
    inner: BlockRefVTableInner,
    _t: PhantomData<T>,
}

impl<T: BlockRefDynamicWithMeta> BlockRefVTable<T> {
    #[expect(missing_docs, reason = "TODO")]
    #[must_use]
    pub const fn from_trait_with_meta() -> Self {
        Self {
            inner: BlockRefVTableInner {
                clone: wrap_clone::<T>,
                drop: wrap_drop::<T>,
                meta: Some(wrap_meta::<T>),
            },
            _t: PhantomData,
        }
    }
}

impl<T: BlockRefDynamic> BlockRefVTable<T> {
    #[expect(missing_docs, reason = "TODO")]
    #[must_use]
    pub const fn from_trait() -> Self {
        Self {
            inner: BlockRefVTableInner {
                clone: wrap_clone::<T>,
                drop: wrap_drop::<T>,
                meta: None,
            },
            _t: PhantomData,
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
fn wrap_clone<T: BlockRefDynamic>(state_ptr: OpaqueStatePtr) -> OpaqueStatePtr {
    T::clone(state_ptr.cast()).cast()
}

#[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
fn wrap_drop<T: BlockRefDynamic>(state_ptr: OpaqueStatePtr) {
    T::drop(state_ptr.cast());
}

fn wrap_meta<T: BlockRefDynamicWithMeta>(state_ptr: OpaqueStatePtr) -> NonNull<dyn Any> {
    T::meta(state_ptr.cast())
}

/// Implements [`BlockRefVTable`] via a trait, without publishing block metadata.
///
/// This is the minimal that required to implement a [`BlockRef`] for a memory provider.
///
/// A typical high-efficiency implementation for a pooling memory provider will resemble something
/// like an `Arc<...>`, with cloning and dropping adjusting the reference count and potentially
/// returning the block to the pool.
///
/// # Safety
///
/// A [`BlockRef`] may move between threads and be accessed from any thread, while different
/// clones of a [`BlockRef`] may be accessed concurrently from different threads.
///
/// The implementation must accordingly be thread-safe to the degree required to
/// correctly operate under these conditions.
pub unsafe trait BlockRefDynamic {
    /// The inner state passed from the [`BlockRef`] to the implementation
    /// of this trait with each function call.
    type State;

    /// Will be called when a [`BlockRef`] is cloned, which means ownership of the block is
    /// to be shared with another co-owner.
    ///
    /// The owners themselves coordinate who owns which part of the block and the [`BlockRef`]
    /// always represents the block as a whole.
    ///
    /// # Returns
    ///
    /// Returns a pointer to use for the dynamic implementation state of the new clone.
    /// The same state may be reused between clones, so the returned pointer may just be
    /// a pointer to the first function parameter received here.
    ///
    /// The pointer must be valid for reads for the lifetime of the clone and there must never
    /// exist any exclusive references to it, as the caller will create shared references on
    /// demand.
    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State>;

    /// Will be called when a [`BlockRef`] is dropped.
    ///
    /// The caller will not access `state` after this call, so it is safe to deallocate the
    /// backing memory if the implementation itself no longer needs the state.
    fn drop(state_ptr: NonNull<Self::State>);
}

/// Implements [`BlockRefVTable`] via a trait.
///
/// This is an extension of [`BlockRefDynamic`] that adds the ability to
/// retrieve metadata about the memory block.
///
/// # Safety
///
/// A [`BlockRef`] may move between threads and be accessed from any thread, while different
/// clones of a [`BlockRef`] may be accessed concurrently from different threads.
///
/// The implementation must accordingly be thread-safe to the degree required to
/// correctly operate under these conditions.
pub unsafe trait BlockRefDynamicWithMeta: BlockRefDynamic {
    /// Will be called to retrieve the memory provider specific metadata of the memory block.
    ///
    /// Must return a pointer to an object whose lifetime it least as long as all clones of
    /// the [`BlockRef`] and which is valid for reads.
    fn meta(state_ptr: NonNull<Self::State>) -> NonNull<dyn Any>;
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};

    use super::*;

    struct TestBlock {
        ref_count: AtomicUsize,
        meta: Option<NonNull<dyn Any>>,
    }

    struct TestBlockMeta {
        label: String,
    }

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
        fn meta(state_ptr: NonNull<Self::State>) -> NonNull<dyn Any> {
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
    fn from_trait_with_meta_creates_vtable_with_meta_fn() {
        // Create a vtable using from_trait_with_meta at runtime to ensure we measure test coverage.
        // We leak the Box to get a 'static reference, which is okay as long as Miri is not looking.
        let vtable: &'static BlockRefVTable<TestBlock> = Box::leak(Box::new(BlockRefVTable::from_trait_with_meta()));

        // Verify that the vtable has a meta function pointer set
        assert!(vtable.inner.meta.is_some());

        // Create a test block with metadata
        let meta_ptr = NonNull::new(Box::into_raw(Box::new(TestBlockMeta {
            label: "Test Metadata".to_string(),
        })))
        .unwrap();

        let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
            ref_count: AtomicUsize::new(1),
            meta: Some(meta_ptr),
        })))
        .unwrap();

        // SAFETY: block_ptr must remain valid for reads and writes until drop()
        // is called via the dynamic fns. It is - we clean it up at the end.
        let block_ref = unsafe { BlockRef::new(block_ptr, vtable) };

        // Verify that meta() works correctly
        let meta = block_ref.meta().expect("Meta should be available");
        assert_eq!(meta.downcast_ref::<TestBlockMeta>().unwrap().label, "Test Metadata");

        drop(block_ref);

        // Clean up
        // SAFETY: Yep, that is our block.
        drop(unsafe { Box::from_raw(block_ptr.as_ptr()) });
        // SAFETY: Yep, that is our meta.
        drop(unsafe { Box::from_raw(meta_ptr.as_ptr()) });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn from_trait_creates_vtable_without_meta_fn() {
        // Create a vtable using from_trait (without meta) at runtime to ensure we measure test coverage.
        // We leak the Box to get a 'static reference, which is okay as long as Miri is not looking.
        let vtable: &'static BlockRefVTable<TestBlock> = Box::leak(Box::new(BlockRefVTable::from_trait()));

        // Verify that the vtable does NOT have a meta function pointer
        assert!(vtable.inner.meta.is_none());

        // Create a test block
        let block_ptr = NonNull::new(Box::into_raw(Box::new(TestBlock {
            ref_count: AtomicUsize::new(1),
            meta: None,
        })))
        .unwrap();

        // SAFETY: block_ptr must remain valid for reads and writes until drop()
        // is called via the dynamic fns. It is - we clean it up at the end.
        let block_ref = unsafe { BlockRef::new(block_ptr, vtable) };

        // Verify that meta() returns None
        assert!(block_ref.meta().is_none());

        drop(block_ref);

        // Clean up
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
}
