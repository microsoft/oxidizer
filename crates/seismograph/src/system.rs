// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fixed-size storage allocated explicitly through the system allocator.

use std::alloc::{GlobalAlloc, Layout, System, handle_alloc_error};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// A fixed-size initialized slice whose backing bypasses the process global allocator.
#[derive(Debug)]
pub(crate) struct SystemSlice<T> {
    pointer: NonNull<T>,
    len: usize,
}

impl<T> SystemSlice<T> {
    /// Allocates and initializes `len` elements through [`System`].
    pub(crate) fn from_fn(len: usize, mut initialize: impl FnMut(usize) -> T) -> Self {
        // #[gamma::skip(cond.always_false, reason = "the zero-length fallthrough has zero Layout, chooses the same dangling pointer, initializes no elements and returns the identical empty slice without allocating")]
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
            NonNull::new(unsafe { allocate_system(layout, false) }.cast::<T>()).unwrap_or_else(|| handle_alloc_error(layout))
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

impl SystemSlice<u8> {
    /// Allocates `len` zeroed bytes through [`System`], returning `None` on allocation failure.
    #[must_use]
    pub(crate) fn try_zeroed(len: usize) -> Option<Self> {
        if len == 0 {
            return Some(Self {
                pointer: NonNull::dangling(),
                len,
            });
        }
        let layout = Layout::array::<u8>(len).ok()?;
        // SAFETY: layout is nonzero and valid. The returned pointer is owned by
        // this value and released with the same allocator and layout.
        let pointer = NonNull::new(unsafe { allocate_system(layout, true) })?;
        Some(Self { pointer, len })
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
        // #[gamma::skip(cond.always_false, reason = "the empty fallthrough computes a valid zero Layout, drops an empty slice and skips deallocation, exactly matching the early return")]
        if self.len == 0 {
            return;
        }
        let layout = Layout::array::<T>(self.len).unwrap_or_else(|_| handle_alloc_error(Layout::new::<T>()));
        // SAFETY: the slice contains exactly len initialized elements owned by self.
        unsafe { std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.pointer.as_ptr(), self.len)) };
        if layout.size() != 0 {
            // SAFETY: pointer and layout are the pair returned by System::alloc in from_fn.
            unsafe { deallocate_system(self.pointer.as_ptr().cast(), layout) };
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
            unsafe { deallocate_system(self.pointer.as_ptr().cast(), self.layout) };
        }
    }
}

unsafe fn allocate_system(layout: Layout, zeroed: bool) -> *mut u8 {
    #[cfg(test)]
    if let Some(allocate) = TEST_SYSTEM_HOOKS.get().map(|hooks| hooks.allocate) {
        return allocate(layout, zeroed);
    }
    // #[gamma::skip(cond.always_true, reason = "The false caller is SystemSlice::from_fn, which overwrites every exposed element; its unwind guard reads only the initialized prefix, so initial zeroing cannot change any observed value")]
    if zeroed {
        // SAFETY: the caller supplies a nonzero valid allocation layout.
        unsafe { System.alloc_zeroed(layout) }
    } else {
        // SAFETY: the caller supplies a nonzero valid allocation layout.
        unsafe { System.alloc(layout) }
    }
}

unsafe fn deallocate_system(pointer: *mut u8, layout: Layout) {
    #[cfg(test)]
    if let Some(deallocate) = TEST_SYSTEM_HOOKS.get().map(|hooks| hooks.deallocate) {
        return deallocate(pointer, layout);
    }
    // SAFETY: the caller supplies its live allocation and original layout.
    unsafe { System.dealloc(pointer, layout) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestSystemHooks {
    allocate: fn(Layout, bool) -> *mut u8,
    deallocate: fn(*mut u8, Layout),
}

#[cfg(test)]
thread_local! {
    static TEST_SYSTEM_HOOKS: std::cell::Cell<Option<TestSystemHooks>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
#[path = "../tests/support/gamma_survivor_system.rs"]
mod gamma_survivor_system;

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{SystemSlice, TEST_SYSTEM_HOOKS, TestSystemHooks};

    #[derive(Default)]
    struct AllocationTrace {
        expected: Option<Layout>,
        fail: bool,
        requests: Vec<(Layout, bool)>,
        releases: Vec<Layout>,
        invalid_releases: Vec<(*mut u8, Layout)>,
        outstanding: Vec<(*mut u8, Layout)>,
    }

    thread_local! {
        static ALLOCATION_TRACE: RefCell<AllocationTrace> = RefCell::new(AllocationTrace::default());
    }

    struct AllocationScope(Option<TestSystemHooks>, AllocationTrace);

    impl AllocationScope {
        fn new(expected: Layout, fail: bool) -> Self {
            let previous_trace = ALLOCATION_TRACE.replace(AllocationTrace {
                expected: Some(expected),
                fail,
                ..AllocationTrace::default()
            });
            let previous = TEST_SYSTEM_HOOKS.replace(Some(TestSystemHooks {
                allocate: |layout, zeroed| {
                    ALLOCATION_TRACE.with_borrow_mut(|trace| {
                        assert_eq!(Some(layout), trace.expected);
                        assert_ne!(layout.size(), 0, "native allocation requires a nonzero layout");
                        trace.requests.push((layout, zeroed));
                        if trace.fail {
                            return std::ptr::null_mut();
                        }
                        let pointer = if zeroed {
                            // SAFETY: the checked layout is valid and nonzero.
                            unsafe { System.alloc_zeroed(layout) }
                        } else {
                            // SAFETY: the checked layout is valid and nonzero.
                            unsafe { System.alloc(layout) }
                        };
                        assert!(!pointer.is_null());
                        trace.outstanding.push((pointer, layout));
                        pointer
                    })
                },
                deallocate: |pointer, layout| {
                    ALLOCATION_TRACE.with_borrow_mut(|trace| {
                        let Some(index) = trace.outstanding.iter().position(|(allocated, _)| *allocated == pointer) else {
                            trace.invalid_releases.push((pointer, layout));
                            return;
                        };
                        if trace.outstanding[index].1 != layout {
                            trace.invalid_releases.push((pointer, layout));
                            return;
                        }
                        trace.outstanding.remove(index);
                        trace.releases.push(layout);
                        // SAFETY: the pointer and layout match the live allocation removed above.
                        unsafe { System.dealloc(pointer, layout) };
                    });
                },
            }));
            Self(previous, previous_trace)
        }
    }

    impl Drop for AllocationScope {
        fn drop(&mut self) {
            TEST_SYSTEM_HOOKS.set(self.0);
            let current = ALLOCATION_TRACE.replace(std::mem::take(&mut self.1));
            for (pointer, layout) in current.outstanding {
                // SAFETY: the scoped observer retained ownership of allocations not yet released.
                unsafe { System.dealloc(pointer, layout) };
            }
        }
    }

    #[test]
    fn november_system_storage_forwards_exact_layout_and_zeroing_and_returns_none_on_allocation_failure() {
        for fail in [false, true] {
            for length in [1, 7] {
                let layout = Layout::array::<u8>(length).unwrap();
                let _scope = AllocationScope::new(layout, fail);
                let storage = SystemSlice::try_zeroed(length);
                if fail {
                    assert!(storage.is_none());
                } else {
                    assert_eq!(&*storage.unwrap(), vec![0; length]);
                }
                ALLOCATION_TRACE.with_borrow(|trace| {
                    assert_eq!(trace.requests, [(layout, true)]);
                    assert_eq!(trace.releases.len(), usize::from(!fail));
                    assert!(trace.invalid_releases.is_empty());
                    assert!(trace.outstanding.is_empty());
                });
            }
        }
        let layout = Layout::array::<u32>(3).unwrap();
        let _scope = AllocationScope::new(layout, false);
        assert_eq!(&*SystemSlice::from_fn(3, |index| u32::try_from(index).unwrap()), [0, 1, 2]);
        ALLOCATION_TRACE.with_borrow(|trace| {
            assert_eq!(trace.requests, [(layout, false)]);
            assert_eq!(trace.releases, [layout]);
            assert!(trace.invalid_releases.is_empty());
            assert!(trace.outstanding.is_empty());
        });
    }

    #[test]
    fn november_system_storage_releases_exact_allocation_once_on_success_and_initializer_unwind_but_never_for_zst() {
        let layout = Layout::array::<u32>(3).unwrap();
        {
            let _scope = AllocationScope::new(layout, false);
            let error = std::panic::catch_unwind(|| {
                SystemSlice::from_fn(3, |index| {
                    assert_ne!(index, 1, "injected initializer failure");
                    u32::try_from(index).unwrap()
                })
            });
            error.unwrap_err();
            ALLOCATION_TRACE.with_borrow(|trace| {
                assert_eq!(trace.requests, [(layout, false)]);
                assert_eq!(trace.releases, [layout]);
                assert!(trace.invalid_releases.is_empty());
                assert!(trace.outstanding.is_empty());
            });
        }
        let _scope = AllocationScope::new(layout, false);
        drop(SystemSlice::from_fn(3, |_| ()));
        let error = std::panic::catch_unwind(|| {
            SystemSlice::from_fn(3, |index| {
                assert_ne!(index, 1, "injected ZST initializer failure");
            })
        });
        error.unwrap_err();
        drop(SystemSlice::<u32>::from_fn(0, |_| unreachable!()));
        ALLOCATION_TRACE.with_borrow(|trace| {
            assert!(trace.requests.is_empty());
            assert!(trace.releases.is_empty());
            assert!(trace.invalid_releases.is_empty());
            assert!(trace.outstanding.is_empty());
        });
    }

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
