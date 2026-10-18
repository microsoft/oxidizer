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
#[derive(Clone)]
pub(crate) struct TaskRef {
    inner: Pin<PoolArc<dyn TypeErasedTask>>,
}

impl TaskRef {
    pub(crate) fn new<T: TypeErasedTask + 'static>(inner: Pin<PoolArc<T>>) -> Self {
        Self {
            inner: PoolArc::unsize_pin(inner, coerce!(dyn TypeErasedTask)),
        }
    }

    /// For test purposes, we can create a fake instance.
    ///
    /// # Safety
    ///
    /// The "value" of the fake task reference is undefined. It is only for use as a
    /// placeholder and actually dereferencing it or accessing its contents is invalid.
    #[cfg(test)]
    pub(crate) unsafe fn fake() -> Self {
        use crate::MockTypeErasedTask;

        let pool = plurality::Pool::new();
        Self::new(pool.alloc_arc_pin(MockTypeErasedTask::new()))
    }

    pub(crate) fn pointer(&self) -> TaskPointer {
        TaskPointer {
            inner: NonNull::from(&*self.inner),
        }
    }

    pub(crate) fn into_owned_pointer(self) -> TaskPointer {
        let Self { inner } = self;
        // SAFETY: Extracting the Arc handle does not move or expose mutable access to its pinned
        // pointee. Ownership immediately transfers into the raw strong pointer.
        let inner = unsafe { Pin::into_inner_unchecked(inner) };
        TaskPointer {
            inner: PoolArc::into_raw(inner),
        }
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
        self.inner.as_ref()
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

    pub(crate) unsafe fn release_strong(self) {
        // SAFETY: Forwarding the caller's ownership of exactly one raw strong reference.
        unsafe { PoolArc::<dyn TypeErasedTask>::decrement_strong_count(self.inner) };
    }

    pub(crate) unsafe fn from_owned(self) -> TaskRef {
        // SAFETY: Forwarding the caller's ownership of exactly one raw strong reference.
        let inner = unsafe { PoolArc::<dyn TypeErasedTask>::from_raw(self.inner) };
        // SAFETY: The task was pinned before its pointer was published and remains immovable.
        let inner = unsafe { Pin::new_unchecked(inner) };
        TaskRef { inner }
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
        unsafe { self.from_owned() }
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
        drop(unsafe { owned.from_owned() });
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
