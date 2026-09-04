// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fixed-size storage allocated explicitly through the system allocator.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// A fixed-size initialized slice whose backing bypasses the process global allocator.
#[doc(hidden)]
#[derive(Debug)]
pub struct SystemSlice<T> {
    pointer: NonNull<T>,
    len: usize,
}

impl<T> SystemSlice<T> {
    /// Allocates and initializes `len` elements through [`System`].
    pub fn from_fn(len: usize, mut initialize: impl FnMut(usize) -> T) -> Self {
        if len == 0 {
            return Self {
                pointer: NonNull::dangling(),
                len,
            };
        }
        let layout = Layout::array::<T>(len).unwrap_or_else(|_| handle_alloc_error(Layout::new::<T>()));
        let allocated = layout.size() != 0;
        let pointer = if allocated {
            // SAFETY: layout is non-zero and was constructed for len elements of T.
            NonNull::new(unsafe { System.alloc(layout) }.cast::<T>()).unwrap_or_else(|| handle_alloc_error(layout))
        } else {
            NonNull::dangling()
        };
        let mut guard = InitializationGuard {
            pointer,
            initialized: 0,
            layout,
            allocated,
        };
        for index in 0..len {
            // SAFETY: the allocation contains len properly aligned T slots and
            // index is less than len.
            let slot = unsafe { pointer.as_ptr().add(index) };
            // SAFETY: each slot is initialized exactly once.
            unsafe { slot.write(initialize(index)) };
            guard.initialized += 1;
        }
        std::mem::forget(guard);
        Self { pointer, len }
    }
}

impl<T> Deref for SystemSlice<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        // SAFETY: pointer references len initialized elements for this value's lifetime.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.len) }
    }
}

impl<T> DerefMut for SystemSlice<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: &mut self provides exclusive access to all initialized elements.
        unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.len) }
    }
}

impl<T> Drop for SystemSlice<T> {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        let layout = Layout::array::<T>(self.len).unwrap_or_else(|_| handle_alloc_error(Layout::new::<T>()));
        // SAFETY: the slice contains exactly len initialized elements owned by self.
        unsafe { std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.pointer.as_ptr(), self.len)) };
        if layout.size() != 0 {
            // SAFETY: pointer and layout are the pair returned by System::alloc in from_fn.
            unsafe { System.dealloc(self.pointer.as_ptr().cast(), layout) };
        }
    }
}

// SAFETY: ownership of the allocation moves with SystemSlice, and access follows T's Send bound.
unsafe impl<T: Send> Send for SystemSlice<T> {}
// SAFETY: shared access exposes only &[T], so sharing follows T's Sync bound.
unsafe impl<T: Sync> Sync for SystemSlice<T> {}

struct InitializationGuard<T> {
    pointer: NonNull<T>,
    initialized: usize,
    layout: Layout,
    allocated: bool,
}

impl<T> Drop for InitializationGuard<T> {
    fn drop(&mut self) {
        // SAFETY: initialized tracks the exact prefix written before unwinding.
        unsafe {
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.pointer.as_ptr(), self.initialized));
        }
        if self.allocated {
            // SAFETY: pointer and layout are the pair returned by System::alloc in from_fn.
            unsafe { System.dealloc(self.pointer.as_ptr().cast(), self.layout) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::SystemSlice;

    #[repr(align(128))]
    struct Aligned(u32);

    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn storage_preserves_length_alignment_and_drops_elements() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let storage = SystemSlice::from_fn(4, |index| {
            assert!(index < 4);
            DropCounter(Arc::clone(&dropped))
        });
        assert_eq!(storage.len(), 4);
        drop(storage);
        assert_eq!(dropped.load(Ordering::Relaxed), 4);

        let aligned = SystemSlice::from_fn(2, |index| Aligned(u32::try_from(index).unwrap()));
        assert_eq!(aligned.as_ptr().addr() % 128, 0);
        assert_eq!(aligned[1].0, 1);
    }

    #[test]
    fn initialization_panic_drops_the_initialized_prefix() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let result = std::panic::catch_unwind({
            let dropped = Arc::clone(&dropped);
            move || {
                let _storage = SystemSlice::from_fn(4, |index| {
                    assert_ne!(index, 2, "injected initializer failure");
                    DropCounter(Arc::clone(&dropped))
                });
            }
        });

        assert!(result.is_err());
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn zero_sized_elements_are_initialized_and_dropped() {
        struct ZeroSized;

        let storage = SystemSlice::from_fn(3, |_| ZeroSized);
        assert_eq!(storage.len(), 3);
    }

    #[test]
    fn empty_storage_and_mutable_access_are_supported() {
        let empty = SystemSlice::<u32>::from_fn(0, |_| unreachable!());
        assert_eq!(&*empty, &[]);
        drop(empty);

        let mut storage = SystemSlice::from_fn(2, |index| index);
        storage[1] = 7;
        assert_eq!(&*storage, &[0, 7]);
    }
}
