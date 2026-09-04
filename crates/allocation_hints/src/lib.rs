// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![warn(missing_docs)]

//! Passive scoped allocation hints.
//!
//! This crate owns logical heap descriptors and the current thread's requested
//! descriptor. It does not register, call, or query an allocator backend.
//! Supporting global allocators may inspect [`active_hint`] and lazily realize
//! the requested heap. Other allocators simply ignore the hint.
//!
//! ```
//! use allocation_hints::heaps::{Heap, bump};
//! use allocation_hints::with_hint;
//!
//! let heap = Heap::bump(bump::Options::new());
//! let values = with_hint(&heap, || vec![1, 2, 3]);
//! assert_eq!(values.len(), 3);
//! ```
//!
//! [`heaps::thread_heap`] captures the current thread's logical identity. A
//! supporting allocator associates that identity with the heap it would
//! ordinarily use for the thread, allowing another thread to allocate toward
//! that owner:
//!
//! ```
//! use allocation_hints::heaps::thread_heap;
//! use allocation_hints::with_hint;
//!
//! let owner = thread_heap();
//! let value = std::thread::spawn(move || with_hint(&owner, || Box::new(42)))
//!     .join()
//!     .unwrap();
//! assert_eq!(*value, 42);
//! ```

use std::cell::Cell;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Heap descriptors.
pub mod heaps;

thread_local! {
    static THREAD_HINTS: Cell<ThreadHints> = const { Cell::new(ThreadHints::EMPTY) };
}

#[derive(Clone, Copy)]
struct ThreadHints {
    active_descriptor: *const heaps::Descriptor,
    thread_heap: Option<heaps::ThreadId>,
}

impl ThreadHints {
    const EMPTY: Self = Self {
        active_descriptor: std::ptr::null(),
        thread_heap: None,
    };
}

/// Allocation hints currently requested by this thread.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocatorHints {
    active: Option<heaps::ActiveHint>,
    thread_heap: Option<heaps::ThreadId>,
}

impl AllocatorHints {
    /// Returns the active scoped heap request.
    #[must_use]
    pub const fn active(self) -> Option<heaps::ActiveHint> {
        self.active
    }

    /// Returns the thread whose allocator-preferred heap was requested.
    #[must_use]
    pub const fn thread_heap(self) -> Option<heaps::ThreadId> {
        self.thread_heap
    }
}

/// Returns all allocation hints requested by the current thread.
///
/// Supporting allocators use this combined snapshot to avoid separate TLS
/// accesses for scoped and thread-heap hints.
#[must_use]
#[doc(hidden)]
#[inline]
pub fn allocator_hints() -> AllocatorHints {
    THREAD_HINTS.with(|hints| {
        let hints = hints.get();
        let active = if hints.active_descriptor.is_null() {
            None
        } else {
            // SAFETY: with_hint keeps the borrowed Heap alive while its
            // descriptor pointer is installed in this thread's TLS.
            let descriptor = unsafe { &*hints.active_descriptor };
            Some(heaps::ActiveHint::new(descriptor.id, descriptor.kind))
        };
        AllocatorHints {
            active,
            thread_heap: hints.thread_heap,
        }
    })
}

/// Returns the current thread's requested heap, if any.
///
/// Supporting allocators call this from their allocation path. The returned
/// value owns no resources and is valid independently of the scope guard.
#[must_use]
#[doc(hidden)]
#[inline]
pub fn active_hint() -> Option<heaps::ActiveHint> {
    allocator_hints().active()
}

/// Runs `operation` with `heap` requested for global allocations on this thread.
///
/// Supporting allocators may honor the request. Other allocators execute the
/// operation normally. Nested scopes restore the previous request on exit,
/// including during unwinding.
#[inline]
pub fn with_hint<R>(heap: &heaps::Heap, operation: impl FnOnce() -> R) -> R {
    let previous = THREAD_HINTS.with(|hints| {
        let previous = hints.get();
        hints.set(ThreadHints {
            active_descriptor: Arc::as_ptr(&heap.descriptor),
            ..previous
        });
        previous.active_descriptor
    });
    let _restore = Restore { previous };
    operation()
}

/// Wraps `future` so `heap` is requested whenever the future is polled.
///
/// Awaited child futures inherit the hint. Separately spawned tasks do not.
#[must_use = "futures do nothing unless polled or awaited"]
pub fn with_hint_async<F>(heap: &heaps::Heap, future: F) -> WithHint<F> {
    WithHint {
        heap: heap.clone(),
        future,
    }
}

pin_project_lite::pin_project! {
    /// A future that requests a prospective heap around every poll.
    #[must_use = "futures do nothing unless polled or awaited"]
    pub struct WithHint<F> {
        heap: heaps::Heap,
        #[pin]
        future: F,
    }
}

impl<F: fmt::Debug> fmt::Debug for WithHint<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithHint")
            .field("heap", &self.heap)
            .field("future", &self.future)
            .finish()
    }
}

impl<F: Future> Future for WithHint<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        with_hint(this.heap, || this.future.poll(cx))
    }
}

struct Restore {
    previous: *const heaps::Descriptor,
}

impl Drop for Restore {
    fn drop(&mut self) {
        THREAD_HINTS.with(|hints| {
            let current = hints.get();
            hints.set(ThreadHints {
                active_descriptor: self.previous,
                ..current
            });
        });
    }
}

pub(crate) fn request_thread_heap(thread_id: heaps::ThreadId) {
    THREAD_HINTS.with(|hints| {
        let current = hints.get();
        hints.set(ThreadHints {
            thread_heap: Some(thread_id),
            ..current
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heaps::{Heap, Kind, bump, thread_heap, thread_heap_request};

    #[test]
    fn nested_scopes_restore_the_previous_descriptor() {
        let outer = Heap::new();
        let inner = Heap::bump(bump::Options::new());

        with_hint(&outer, || {
            assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(outer.id()));
            with_hint(&inner, || {
                assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(inner.id()));
            });
            assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(outer.id()));
        });
        assert_eq!(active_hint(), None);
    }

    #[test]
    fn unused_prospective_heaps_require_no_backend() {
        let first = Heap::bump(bump::Options::new());
        let first_id = first.id();
        drop(first);
        let second = Heap::bump(bump::Options::new());
        assert_ne!(second.id(), first_id);
    }

    #[test]
    fn thread_heap_captures_the_originating_thread() {
        let first = thread_heap();
        let second = thread_heap();
        let Kind::Thread(first_thread) = first.kind() else {
            panic!("thread_heap must create a thread-target descriptor");
        };
        let Kind::Thread(second_thread) = second.kind() else {
            panic!("thread_heap must create a thread-target descriptor");
        };

        assert_eq!(first_thread, second_thread);
        assert_eq!(first.id(), second.id());
        assert_eq!(thread_heap_request(), Some(first_thread));
    }

    #[test]
    fn allocator_hints_return_scoped_and_thread_requests_together() {
        let thread = thread_heap();
        let Kind::Thread(thread_id) = thread.kind() else {
            panic!("thread_heap must create a thread-target descriptor");
        };
        let scoped = Heap::bump(bump::Options::new());

        with_hint(&scoped, || {
            let hints = allocator_hints();
            assert_eq!(
                (hints.active().map(heaps::ActiveHint::id), hints.thread_heap()),
                (Some(scoped.id()), Some(thread_id))
            );
        });
    }

    #[test]
    fn unsupported_allocators_execute_operations_normally() {
        let heap = Heap::bump(bump::Options::new());
        let values = with_hint(&heap, || vec![1, 2, 3]);
        assert_eq!(values, [1, 2, 3]);
    }
}
