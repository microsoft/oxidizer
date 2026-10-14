// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};

static TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn enable_tracking() {
    TRACKING_ENABLED.store(true, Ordering::Relaxed);
}

/// Allocator used by generated entry points; tracking is enabled only in allocation workers.
#[doc(hidden)]
#[derive(Debug)]
pub struct Allocator<A: GlobalAlloc> {
    inner: A,
}

impl Allocator<System> {
    /// Uses the system allocator.
    #[must_use]
    pub const fn system() -> Self {
        Self::new(System)
    }
}

impl<A: GlobalAlloc> Allocator<A> {
    /// Wraps the allocator shared by tracked and untracked operations.
    #[must_use]
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }

    fn tracker(&self) -> alloc_tracker::Allocator<AllocatorRef<'_, A>> {
        // Borrow the same allocator in both paths; custom allocator state must not be duplicated.
        alloc_tracker::Allocator::new(AllocatorRef(&self.inner))
    }
}

// SAFETY: Both paths forward the caller's allocation contract to the same underlying allocator.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { self.tracker().alloc(layout) }
        } else {
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { self.inner.alloc(layout) }
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { self.tracker().alloc_zeroed(layout) }
        } else {
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { self.inner.alloc_zeroed(layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            // SAFETY: The caller supplies the live allocation, original layout, and valid new size.
            unsafe { self.tracker().realloc(ptr, layout, new_size) }
        } else {
            // SAFETY: The caller supplies the live allocation, original layout, and valid new size.
            unsafe { self.inner.realloc(ptr, layout, new_size) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING_ENABLED.load(Ordering::Relaxed) {
            // SAFETY: Both paths allocate through self.inner; the caller supplies its live allocation and layout.
            unsafe { self.tracker().dealloc(ptr, layout) }
        } else {
            // SAFETY: Both paths allocate through self.inner; the caller supplies its live allocation and layout.
            unsafe { self.inner.dealloc(ptr, layout) }
        }
    }
}

struct AllocatorRef<'a, A>(&'a A);

// SAFETY: This adapter forwards all operations unchanged to the borrowed GlobalAlloc implementation.
unsafe impl<A: GlobalAlloc> GlobalAlloc for AllocatorRef<'_, A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { self.0.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies the live allocation, original layout, and valid new size.
        unsafe { self.0.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live allocation from this allocator and its original layout.
        unsafe { self.0.dealloc(ptr, layout) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::ptr::NonNull;
    use std::sync::atomic::AtomicUsize;

    use alloc_tracker::Session;

    use super::*;

    #[derive(Default)]
    struct CountingAllocator {
        allocations: AtomicUsize,
        zeroed: AtomicUsize,
        reallocations: AtomicUsize,
        frees: AtomicUsize,
    }

    // SAFETY: All operations preserve the GlobalAlloc contract by forwarding unchanged to System.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            self.zeroed.fetch_add(1, Ordering::Relaxed);
            // SAFETY: The caller supplies a valid allocation layout.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            self.reallocations.fetch_add(1, Ordering::Relaxed);
            // SAFETY: The caller supplies a live System allocation and valid layouts.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            self.frees.fetch_add(1, Ordering::Relaxed);
            // SAFETY: The caller supplies a live System allocation and its original layout.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    struct DisableOnDrop;

    impl Drop for DisableOnDrop {
        fn drop(&mut self) {
            TRACKING_ENABLED.store(false, Ordering::Relaxed);
        }
    }

    #[test]
    fn only_enabled_operations_are_tracked_and_both_paths_share_allocator_state() {
        let _reset = DisableOnDrop;
        let allocator = Allocator::new(CountingAllocator::default());
        let session = Session::new().no_stdout().no_file();
        let layout = Layout::from_size_align(16, 8).unwrap();
        let resized = Layout::from_size_align(32, 8).unwrap();

        for (name, enabled) in [("disabled", false), ("enabled", true)] {
            TRACKING_ENABLED.store(enabled, Ordering::Relaxed);
            let span = session.operation(name).measure_process().iterations(1);
            // SAFETY: The layout has non-zero size and valid alignment.
            let ptr = NonNull::new(unsafe { allocator.alloc(layout) }).unwrap();
            // SAFETY: The allocation contains at least one writable byte.
            unsafe { ptr.as_ptr().write(42) };
            // SAFETY: The pointer is live, its original layout matches, and the new size is valid.
            let ptr = NonNull::new(unsafe { allocator.realloc(ptr.as_ptr(), layout, resized.size()) }).unwrap();
            // SAFETY: Successful reallocation preserves the initialized first byte.
            assert_eq!(unsafe { ptr.as_ptr().read() }, 42);
            // SAFETY: The resized layout matches this live allocation.
            unsafe { allocator.dealloc(ptr.as_ptr(), resized) };
            // SAFETY: The layout has non-zero size and valid alignment.
            let zeroed = NonNull::new(unsafe { allocator.alloc_zeroed(layout) }).unwrap();
            // SAFETY: The allocation contains layout.size() initialized bytes.
            let bytes = unsafe { std::slice::from_raw_parts(zeroed.as_ptr(), layout.size()) };
            assert!(bytes.iter().all(|byte| *byte == 0));
            // SAFETY: The layout matches this live allocation.
            unsafe { allocator.dealloc(zeroed.as_ptr(), layout) };
            drop(span);
        }

        let report = session.to_report();
        for (name, operation) in report.operations() {
            let (count, bytes) = if name == "enabled" { (3, 64) } else { (0, 0) };
            assert_eq!(operation.total_allocations_count(), count, "{name}");
            assert_eq!(operation.total_bytes_allocated(), bytes, "{name}");
        }
        assert_eq!(allocator.inner.allocations.load(Ordering::Relaxed), 2);
        assert_eq!(allocator.inner.zeroed.load(Ordering::Relaxed), 2);
        assert_eq!(allocator.inner.reallocations.load(Ordering::Relaxed), 2);
        assert_eq!(allocator.inner.frees.load(Ordering::Relaxed), 4);

        TRACKING_ENABLED.store(false, Ordering::Relaxed);
        // SAFETY: A valid layout is supplied; the allocation is freed with the same allocator and layout.
        let ptr = NonNull::new(unsafe { allocator.alloc(layout) }).unwrap();
        enable_tracking();
        // SAFETY: Enabling tracking does not change ownership of this live allocation.
        unsafe { allocator.dealloc(ptr.as_ptr(), layout) };
        assert_eq!(allocator.inner.frees.load(Ordering::Relaxed), 5);
    }
}
