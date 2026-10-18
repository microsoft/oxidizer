// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::ptr::{self, NonNull};

use plurality::{Arc as PoolArc, coerce};

use crate::TypeErasedTask;

/// Allows a task to be referenced by the executor and its resources released, without knowing
/// the exact type of the task, only that it implements [`TypeErasedTask`].
///
/// Clones retain the pinned pooled task allocation. Task methods remain owner-thread-only even
/// when a waker or remote notification carries a strong reference across threads.
/// Each handle owns a raw Arc strong reference, preserving the allocation pointer's provenance
/// for access to the pool's adjacent reference count.
pub(crate) struct TaskRef {
    pointer: TaskPointer,
}

impl TaskRef {
    pub(crate) fn new<T: TypeErasedTask + 'static>(inner: Pin<PoolArc<T>>) -> Self {
        let inner = PoolArc::unsize_pin(inner, coerce!(dyn TypeErasedTask));
        // SAFETY: Only owner-thread pinned borrows are exposed by TaskRef. Moving the Arc
        // handle into raw ownership cannot move its pinned task.
        let inner = unsafe { Pin::into_inner_unchecked(inner) };
        Self {
            pointer: TaskPointer {
                inner: PoolArc::into_raw(inner),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn mock() -> Self {
        use crate::MockTypeErasedTask;

        let pool = plurality::Pool::new();
        Self::new(pool.alloc_arc_pin(MockTypeErasedTask::new()))
    }

    pub(crate) fn pointer(&self) -> TaskPointer {
        self.pointer
    }

    pub(crate) fn into_owned_pointer(self) -> TaskPointer {
        let pointer = self.pointer;
        std::mem::forget(self);
        pointer
    }

    /// # Safety
    ///
    /// The caller is responsible for ensuring that Rust aliasing rules are not violated.
    ///
    /// This may only be called on the same thread as the task was created on. That is, the
    /// `TaskRef` may be passed from thread to thread but has to end up back on its original
    /// thread to actually be used.
    ///
    /// The caller must guarantee that the referenced task is still alive.
    #[must_use]
    pub(crate) unsafe fn as_task(&self) -> Pin<&dyn TypeErasedTask> {
        // SAFETY: Forwarding the caller's owner-thread guarantee; this owner keeps the task live.
        unsafe { self.pointer.as_task() }
    }
}

impl Clone for TaskRef {
    fn clone(&self) -> Self {
        // SAFETY: This owner keeps the allocation alive while acquiring another strong reference.
        unsafe { self.pointer.retain() }
    }
}

impl Drop for TaskRef {
    fn drop(&mut self) {
        // SAFETY: Every TaskRef owns one raw Arc strong reference. All pointers originate from
        // into_raw, retaining the slot's provenance; reconstructed handles are only dropped.
        drop(unsafe { PoolArc::<dyn TypeErasedTask>::from_raw(self.pointer.inner) });
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskPointer {
    inner: NonNull<dyn TypeErasedTask>,
}

impl TaskPointer {
    pub(crate) unsafe fn increment_strong(self) {
        // SAFETY: Forwarding the caller's live-allocation guarantee.
        unsafe { PoolArc::<dyn TypeErasedTask>::increment_strong_count(self.inner) };
    }

    /// # Safety
    ///
    /// The caller must transfer exactly one owned strong reference to a live, pinned task.
    pub(crate) unsafe fn into_task_ref(self) -> TaskRef {
        TaskRef { pointer: self }
    }

    #[must_use]
    pub(crate) unsafe fn as_task(&self) -> Pin<&dyn TypeErasedTask> {
        // SAFETY: Forwarding the caller's owner-thread and live-allocation guarantees.
        let task = unsafe { self.inner.as_ref() };
        // SAFETY: Published task allocations are pinned for their entire lifetime.
        unsafe { Pin::new_unchecked(task) }
    }

    pub(crate) unsafe fn retain(self) -> TaskRef {
        // SAFETY: Forwarding the caller's live-allocation guarantee.
        unsafe { self.increment_strong() };
        // SAFETY: The increment above created exactly one raw strong reference.
        unsafe { self.into_task_ref() }
    }
}

// SAFETY: The pointer is dereferenced only through an unsafe owner-thread operation. Remote
// threads use it solely as stable identity while a strong pooled owner keeps the allocation live.
unsafe impl Send for TaskPointer {}
// SAFETY: Same invariant as `Send`.
unsafe impl Sync for TaskPointer {}

impl PartialEq for TaskPointer {
    fn eq(&self, other: &Self) -> bool {
        ptr::addr_eq(self.inner.as_ptr(), other.inner.as_ptr())
    }
}

impl Eq for TaskPointer {}

impl std::fmt::Debug for TaskRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TaskRef").field(&self.pointer()).finish()
    }
}

#[cfg(test)]
mod tests {
    use plurality::MultiPool;

    use super::*;
    use crate::MockTypeErasedTask;

    #[test]
    fn raw_ownership_round_trip_preserves_identity() {
        let pool = MultiPool::new();
        let task_ref = TaskRef::new(pool.alloc_arc_pin(MockTypeErasedTask::new()));
        let pointer = task_ref.pointer();
        let owned = task_ref.into_owned_pointer();
        assert_eq!(owned, pointer);
        // SAFETY: `into_owned_pointer` transferred exactly one strong reference.
        drop(unsafe { owned.into_task_ref() });
        assert!(pool.is_empty());
    }

    #[test]
    fn different_task_pointers_are_not_equal() {
        let pool = MultiPool::new();
        let task_ref1 = TaskRef::new(pool.alloc_arc_pin(MockTypeErasedTask::new()));
        let task_ref2 = TaskRef::new(pool.alloc_arc_pin(MockTypeErasedTask::new()));

        assert_ne!(task_ref1.pointer(), task_ref2.pointer());

        drop((task_ref1, task_ref2));
    }
}
