// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::ptr::{self, NonNull};

use plurality::{Box as PoolBox, coerce};

use crate::TypeErasedTask;

/// Allows a task to be referenced by the executor and its resources released, without knowing
/// the exact type of the task, only that it implements [`TypeErasedTask`].
///
/// This acts like a super-powered pointer and enforces no ownership semantics - the caller is
/// responsible for not using it once the task has been dropped.
#[derive(Copy, Clone, Debug)]
pub(crate) struct TaskRef {
    inner: NonNull<dyn TypeErasedTask>,
}

impl TaskRef {
    pub(crate) fn new<T: TypeErasedTask + 'static>(inner: PoolBox<T>) -> Self {
        let inner: PoolBox<dyn TypeErasedTask> = PoolBox::unsize(inner, coerce!(dyn TypeErasedTask));
        Self {
            inner: PoolBox::into_raw(inner),
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

        let inner = NonNull::<MockTypeErasedTask>::dangling().as_ptr();
        let inner: *mut dyn TypeErasedTask = inner;

        // SAFETY: The typed dangling pointer is non-null, and the coercion above supplies valid
        // trait-object metadata. The resulting pointer remains a placeholder and is never
        // dereferenced or released.
        let inner = unsafe { NonNull::new_unchecked(inner) };
        Self { inner }
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
        // SAFETY: Forwarding safety guarantees from the caller.
        let task = unsafe { self.inner.as_ref() };

        // SAFETY: Our tasks are always pinned, they just lose this metadata in their pointer form.
        unsafe { Pin::new_unchecked(task) }
    }

    /// Drops the task and returns its slot to the pool.
    ///
    /// # Safety
    ///
    /// This may only be called on the same thread as the task was created on. That is, the
    /// `TaskRef` may be passed from thread to thread but has to end up back on its original
    /// thread to actually be used.
    ///
    /// The caller must ensure this is called exactly once for the referenced task and that no
    /// references to it remain.
    pub(crate) unsafe fn release(self) {
        // SAFETY: Forwarding the caller's guarantee that this is the original raw pointer and is
        // reconstructed exactly once after all references have expired.
        drop(unsafe { PoolBox::<dyn TypeErasedTask>::from_raw(self.inner) });
    }
}

// SAFETY: It is permissible to send `TaskRef` between threads for the purpose of passing the
// reference around. However, methods must only be called on the original thread the task
// was created on.
unsafe impl Send for TaskRef {}

impl PartialEq for TaskRef {
    #[cfg_attr(test, mutants::skip)] // Liable to cause test timeouts, as collection logic gets wonky.
    fn eq(&self, other: &Self) -> bool {
        ptr::addr_eq(self.inner.as_ptr(), other.inner.as_ptr())
    }
}

impl Eq for TaskRef {}

impl Hash for TaskRef {
    #[cfg_attr(test, mutants::skip)] // Liable to cause test timeouts, as collection logic gets wonky.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.as_ptr().cast::<()>().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use std::hash::DefaultHasher;

    use plurality::MultiPool;

    use super::*;
    use crate::MockTypeErasedTask;

    #[test]
    fn same_task_refs_eq() {
        let pool = MultiPool::new();
        let task_ref1 = TaskRef::new(pool.alloc_box(MockTypeErasedTask::new()));
        let task_ref2 = task_ref1;

        assert_eq!(task_ref1, task_ref2);

        // Their hashes must also be equal.
        let mut hasher1 = DefaultHasher::new();
        task_ref1.hash(&mut hasher1);
        let hash1 = hasher1.finish();

        let mut hasher2 = DefaultHasher::new();
        task_ref2.hash(&mut hasher2);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);

        // SAFETY: This is the only release of the pooled task and no references remain.
        unsafe { task_ref1.release() };
    }

    #[test]
    fn different_task_refs_not_eq() {
        let pool = MultiPool::new();
        let task_ref1 = TaskRef::new(pool.alloc_box(MockTypeErasedTask::new()));
        let task_ref2 = TaskRef::new(pool.alloc_box(MockTypeErasedTask::new()));

        assert_ne!(task_ref1, task_ref2);

        // SAFETY: The first pooled task is released exactly once and no references remain.
        unsafe { task_ref1.release() };
        // SAFETY: The second pooled task is released exactly once and no references remain.
        unsafe { task_ref2.release() };
    }
}
