// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout};

use seismograph::recorder::event::EventClass;

use crate::backend::{Backend, Snmalloc};

/// A stateless allocator handle with a backend selected at compile time.
///
/// Use [`rallocator!`](crate::rallocator) to install the global allocator.
/// The macro selects the backend. The default is snmalloc.
///
/// Handles with the same backend share allocator state. Return each allocation
/// to the backend that created it.
///
/// For platform and deployment requirements, see [`rallocator!`](crate::rallocator).
#[derive(Debug)]
pub struct Rallocator<B: Backend = Snmalloc> {
    backend: B,
}

impl<B: Backend> Rallocator<B> {
    /// Internal constructor for [`rallocator!`](crate::rallocator).
    ///
    /// # Safety
    ///
    /// Every allocation must return to the same backend and linked allocator
    /// copy that created it. That copy must remain loaded until all its
    /// allocations are retired. Mixing backends or independently linked copies
    /// can cause invalid memory access.
    ///
    /// Direct construction requires an explicit unsafe call:
    ///
    /// ```compile_fail,E0133
    /// use rallocator::{Rallocator, backend};
    ///
    /// let allocator = Rallocator::new(backend::Snmalloc);
    /// ```
    ///
    /// Safe default construction is unavailable:
    ///
    /// ```compile_fail,E0599
    /// use rallocator::{Rallocator, backend};
    ///
    /// let allocator = Rallocator::<backend::Snmalloc>::default();
    /// ```
    #[doc(hidden)]
    pub const unsafe fn new(backend: B) -> Self {
        Self { backend }
    }

    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        if seismograph::recorder::is_enabled(EventClass::Allocation) {
            // SAFETY: GlobalAlloc supplied the valid nonzero layout.
            return unsafe { self.allocate_recorded::<ZEROED>(layout) };
        }
        // SAFETY: The layout meets the backend's allocation contract.
        unsafe { self.backend.allocate::<ZEROED>(layout) }
    }

    // Keep recorder and publication state out of the diagnostics-off fast paths.
    #[inline(never)]
    unsafe fn allocate_recorded<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid nonzero allocation layout.
        let pointer = unsafe { self.backend.allocate::<ZEROED>(layout) };
        if !pointer.is_null() {
            crate::recording::emit(pointer, layout.size(), layout.align(), false);
            self.backend.publish_state();
        }
        pointer
    }

    #[inline(never)]
    unsafe fn deallocate_recorded(&self, pointer: *mut u8, layout: Layout) {
        crate::recording::emit(pointer, layout.size(), layout.align(), true);
        // SAFETY: Recording finishes before the backend can make this live block reusable.
        unsafe { self.backend.deallocate(pointer, layout) };
        self.backend.publish_state();
    }

    #[inline(never)]
    unsafe fn reallocate_recorded(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let record = |replacement| crate::recording::resize(pointer, replacement, layout.size(), new_size, layout.align());
        // SAFETY: The caller forwards the live block and complete resize contract.
        unsafe { self.reallocate_recorded_with(pointer, layout, new_size, record) }
    }

    unsafe fn reallocate_recorded_with(&self, pointer: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
        // SAFETY: The caller supplies a live block and a valid resize request.
        let replacement = unsafe { self.backend.prepare_reallocation(pointer, layout, new_size) };
        if replacement.is_null() {
            return replacement;
        }
        if replacement != pointer || new_size != layout.size() {
            record(replacement);
        }
        if replacement != pointer {
            // SAFETY: Preparation copied the payload and kept the old block live
            // until its successful resize event was recorded.
            unsafe { self.backend.deallocate(pointer, layout) };
        }
        self.backend.publish_state();
        replacement
    }
}

// SAFETY: The sealed backend implementations return disjoint, aligned blocks
// and preserve GlobalAlloc's failure, resizing, and cross-thread free contracts.
unsafe impl<B: Backend> GlobalAlloc for Rallocator<B> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc supplies a valid, nonzero allocation layout.
        unsafe { self.allocate::<false>(layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc supplies a valid, nonzero allocation layout.
        unsafe { self.allocate::<true>(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if seismograph::recorder::is_enabled(EventClass::Allocation) {
            // SAFETY: GlobalAlloc transfers a live block for its exact layout.
            return unsafe { self.deallocate_recorded(ptr, layout) };
        }
        // SAFETY: GlobalAlloc transfers a live allocation from this backend
        // with its exact layout, and it is retired once.
        unsafe { self.backend.deallocate(ptr, layout) };
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if seismograph::recorder::is_enabled(EventClass::Allocation) {
            // SAFETY: GlobalAlloc supplies the same live-block resize contract.
            return unsafe { self.reallocate_recorded(ptr, layout, new_size) };
        }
        // SAFETY: GlobalAlloc supplies a live allocation from this backend,
        // its exact layout, and a valid nonzero resize request.
        unsafe { self.backend.reallocate(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::System;
    use std::cell::Cell;

    use super::*;
    use crate::backend::sealed::Operations;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Phase {
        Initial,
        Prepared,
        Recorded,
        Released,
        Published,
    }

    struct Probe<'a>(&'a Cell<Phase>);

    impl Backend for Probe<'_> {}

    impl Operations for Probe<'_> {
        unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
            if ZEROED {
                // SAFETY: The caller supplies a valid allocation layout.
                unsafe { System.alloc_zeroed(layout) }
            } else {
                // SAFETY: The caller supplies a valid allocation layout.
                unsafe { System.alloc(layout) }
            }
        }

        unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
            assert_eq!(self.0.get(), Phase::Recorded);
            self.0.set(Phase::Released);
            // SAFETY: The probe exclusively owns this block from System.
            unsafe { System.dealloc(ptr, layout) };
        }

        unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: This probe uses System for the complete live-block contract.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn prepare_reallocation(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let new_layout = Layout::from_size_align(new_size, layout.align()).unwrap();
            // SAFETY: The checked nonzero layout creates separate live storage.
            let replacement = unsafe { System.alloc(new_layout) };
            assert!(!replacement.is_null());
            // SAFETY: Both owned blocks remain disjoint and cover the copy bound.
            unsafe { std::ptr::copy_nonoverlapping(ptr, replacement, layout.size().min(new_size)) };
            self.0.set(Phase::Prepared);
            replacement
        }

        fn publish_state(&self) {
            assert_eq!(self.0.get(), Phase::Released);
            self.0.set(Phase::Published);
        }
    }

    #[test]
    fn resize_record_is_emitted_before_old_storage_becomes_reusable() {
        let phase = Cell::new(Phase::Initial);
        // SAFETY: This fixture owns both blocks and keeps its System-backed probe alive.
        let allocator = unsafe { Rallocator::new(Probe(&phase)) };
        let layout = Layout::from_size_align(64, 16).unwrap();
        // SAFETY: The probe receives the valid nonzero layout.
        let old = unsafe { allocator.backend.allocate::<false>(layout) };
        assert!(!old.is_null());
        // SAFETY: The old allocation exclusively owns the full requested range.
        unsafe { old.write_bytes(0x5a, layout.size()) };
        let record = |new: *mut u8| {
            assert_eq!(phase.get(), Phase::Prepared);
            // SAFETY: Preparation must keep the original live until recording finishes.
            assert_eq!(unsafe { old.read() }, 0x5a);
            // SAFETY: Preparation copied the initialized prefix into the new block.
            assert_eq!(unsafe { new.read() }, 0x5a);
            phase.set(Phase::Recorded);
        };

        // SAFETY: The fixture transfers its live old block for this valid resize.
        let new = unsafe { allocator.reallocate_recorded_with(old, layout, 128, record) };

        assert_ne!(old, new);
        assert_eq!(phase.get(), Phase::Published);
        // SAFETY: The successful probe resize transfers this System-backed replacement.
        unsafe { System.dealloc(new, Layout::from_size_align(128, 16).unwrap()) };
    }
}
