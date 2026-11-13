// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::Layout;

use crate::backend::sealed::Operations;
use crate::backend::snmalloc::{classes, observation, thread};

/// Selects the Rust snmalloc backend for [`Rallocator`](crate::Rallocator).
///
/// This is the default backend. Install it with
/// `rallocator!(backend = snmalloc)` or `rallocator!()`.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct Snmalloc;

impl crate::backend::Backend for Snmalloc {}

impl Operations for Snmalloc {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        if ZEROED {
            thread::allocate_zeroed(layout)
        } else {
            thread::allocate(layout)
        }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: The caller transfers a live allocation from this backend.
        unsafe { thread::deallocate(ptr) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies the live allocation and valid new size.
        unsafe { reallocate_core(ptr, layout, new_size) }
    }

    #[inline]
    unsafe fn prepare_reallocation(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some(request) = classes::Request::with_size(layout, new_size) else {
            return std::ptr::null_mut();
        };
        if classes::Request::new(layout) == Some(request) {
            return ptr;
        }
        let replacement = allocate_acquired_with(request, thread::allocate_request);
        if !replacement.is_null() {
            // SAFETY: Both allocations remain live and disjoint for the copy bound.
            unsafe { std::ptr::copy_nonoverlapping(ptr, replacement, layout.size().min(new_size)) };
        }
        replacement
    }

    fn publish_state(&self) {
        observation::publish();
    }
}

fn allocate_acquired_with<T>(request: T, allocate: impl FnOnce(T) -> *mut u8) -> *mut u8 {
    crate::sync_fence::fence(std::sync::atomic::Ordering::Acquire);
    allocate(request)
}

#[inline]
pub(super) unsafe fn reallocate_core(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Some(request) = classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    // override/rust.cc retains a pointer exactly when the aligned old and
    // new layouts select the same size class, not for arbitrary shrinking.
    if classes::Request::new(layout) == Some(request) {
        return ptr;
    }
    // SAFETY: The caller guarantees ptr is live for layout; the copy bound
    // fits both the old allocation and requested replacement.
    unsafe { thread::reallocate(ptr, request, layout.size().min(new_size)) }
}

#[cfg(test)]
mod tests {
    use std::alloc::GlobalAlloc;

    use super::*;
    use crate::Rallocator;

    fn prime_destination(layout: Layout) -> (Vec<*mut u8>, Vec<*mut u8>, usize) {
        thread::flush();
        let request = classes::Request::new(layout).unwrap();
        let index = request.tag() - classes::SMALL_TAG;
        let class = classes::CLASSES[index];
        let mut held = Vec::new();
        loop {
            let pointer = thread::allocate(layout);
            assert!(!pointer.is_null());
            // SAFETY: The internal allocator grants the complete request class.
            unsafe { pointer.write_bytes(19, class.size) };
            held.push(pointer);
            assert!(held.len() <= 4096);
            let mut available = false;
            thread::observe_current(|owner| {
                // SAFETY: This synchronous callback runs on the core's sole owner thread.
                let state = unsafe { &*owner.core_ptr() }.observe(&mut 0);
                available = state.classes[index].fast_nonempty || state.classes[index].available_slabs != 0;
            });
            if !available {
                break;
            }
        }
        let base = held[0].addr() & !(class.slab - 1);
        let mut reusable = Vec::new();
        held.retain(|&pointer| {
            if pointer.addr() & !(class.slab - 1) == base && reusable.len() < usize::from(class.waking) {
                reusable.push(pointer);
                // SAFETY: Each initialized destination is retired only once.
                unsafe { thread::deallocate(pointer) };
                false
            } else {
                true
            }
        });
        assert_eq!(reusable.len(), usize::from(class.waking));
        (reusable, held, class.size)
    }

    #[test]
    fn replacement_acquires_before_the_allocator_can_observe_the_request() {
        let trace = crate::sync_fence::trace::Trace::start();
        let mut byte = 17u8;
        let pointer = &raw mut byte;
        assert_eq!(
            allocate_acquired_with((71, 83), |request| {
                assert_eq!(request, (71, 83));
                assert_eq!(trace.events(), [std::sync::atomic::Ordering::Acquire]);
                pointer
            }),
            pointer
        );
        assert_eq!(trace.events(), [std::sync::atomic::Ordering::Acquire]);
    }

    #[test]
    fn unzeroed_entrypoints_preserve_known_initialized_reusable_tails() {
        thread::with_fresh_owner(|| {
            seismograph::recorder::without_recording(|| {
                // SAFETY: Every fixture block stays in this linked snmalloc instance.
                let allocator = unsafe { Rallocator::new(Snmalloc) };
                let layout = Layout::from_size_align(33, 16).unwrap();
                for public_entrypoint in [false, true] {
                    let (reusable, held, _) = prime_destination(layout);
                    let pointer = if public_entrypoint {
                        // SAFETY: The fixture layout is valid and nonzero.
                        unsafe { allocator.alloc(layout) }
                    } else {
                        thread::allocate(layout)
                    };
                    assert!(!pointer.is_null());
                    assert!(reusable.contains(&pointer));
                    let link_bytes = size_of::<usize>();
                    // SAFETY: The fixture initializes the whole class to 19. Free-list
                    // operations overwrite only the first link word.
                    let tail = unsafe { std::slice::from_raw_parts(pointer.wrapping_add(link_bytes), layout.size() - link_bytes) };
                    assert_eq!(tail, &[19; 25]);
                    for peer in held {
                        // SAFETY: Each held peer is a distinct live fixture allocation.
                        unsafe { thread::deallocate(peer) };
                    }
                    // SAFETY: The fixture still owns this live allocation.
                    unsafe { allocator.dealloc(pointer, layout) };
                }
            });
        });
    }

    #[test]
    fn zeroed_entrypoint_clears_known_initialized_reused_bytes() {
        thread::with_fresh_owner(|| {
            seismograph::recorder::without_recording(|| {
                // SAFETY: Every fixture block stays in this linked snmalloc instance.
                let allocator = unsafe { Rallocator::new(Snmalloc) };
                let layout = Layout::from_size_align(33, 16).unwrap();
                let (reusable, held, _) = prime_destination(layout);
                // SAFETY: The fixture layout is valid and nonzero.
                let zeroed = unsafe { allocator.alloc_zeroed(layout) };
                assert!(!zeroed.is_null());
                assert!(reusable.contains(&zeroed));
                // SAFETY: The live zeroed allocation initializes all requested bytes.
                assert_eq!(unsafe { std::slice::from_raw_parts(zeroed, layout.size()) }, [0; 33]);
                for peer in held {
                    // SAFETY: Each held peer is a distinct live fixture allocation.
                    unsafe { thread::deallocate(peer) };
                }
                // SAFETY: The fixture still owns the zeroed allocation.
                unsafe { allocator.dealloc(zeroed, layout) };
            });
        });
    }

    #[test]
    fn prepared_resize_preserves_the_original_and_only_copies_the_requested_prefix() {
        thread::with_fresh_owner(|| {
            seismograph::recorder::without_recording(|| {
                for (old_size, new_size, retained) in [
                    (33, 47, true),
                    (33, 48, true),
                    (33, 80, false),
                    (80, 33, false),
                    (65536, 65537, false),
                    (8192, 8192, true),
                ] {
                    let layout = Layout::from_size_align(old_size, 16).unwrap();
                    let original = thread::allocate(layout);
                    assert!(!original.is_null());
                    // SAFETY: The fixture owns all old_size writable requested bytes.
                    unsafe { original.write_bytes(97, old_size) };
                    let primed =
                        (!retained && new_size <= 65_536).then(|| prime_destination(Layout::from_size_align(new_size, 16).unwrap()));
                    // SAFETY: The original remains live and the resize is valid and nonzero.
                    let replacement = unsafe { Snmalloc.prepare_reallocation(original, layout, new_size) };
                    assert!(!replacement.is_null());
                    assert_eq!(replacement == original, retained);
                    // SAFETY: Preparation retains the initialized original until the caller retires it.
                    let original_bytes = unsafe { std::slice::from_raw_parts(original, old_size) };
                    assert!(original_bytes.iter().all(|&byte| byte == 97));
                    // SAFETY: A successful resize copies the initialized old/new intersection.
                    let prefix = unsafe { std::slice::from_raw_parts(replacement, old_size.min(new_size)) };
                    assert!(prefix.iter().all(|&byte| byte == 97));
                    if let Some((reusable, held, class_size)) = primed {
                        assert!(reusable.contains(&replacement));
                        // SAFETY: The fixture initialized the complete destination class.
                        let tail = unsafe { std::slice::from_raw_parts(replacement.wrapping_add(new_size), class_size - new_size) };
                        assert!(tail.iter().all(|&byte| byte == 19));
                        for peer in held {
                            // SAFETY: Each peer remains a distinct live fixture allocation.
                            unsafe { thread::deallocate(peer) };
                        }
                    }
                    if !retained {
                        // SAFETY: Preparation leaves this distinct original live for the caller.
                        unsafe { thread::deallocate(original) };
                    }
                    // SAFETY: The fixture owns the retained or replacement block.
                    unsafe { thread::deallocate(replacement) };
                }
            });
        });
    }
}
