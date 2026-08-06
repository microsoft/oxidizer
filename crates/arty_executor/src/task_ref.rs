// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::ptr::{self};

use infinity_pool::{RawBlindPooled, define_pooled_dyn_cast};

use crate::TypeErasedTask;

// Enables casting of RawBlindPooled<???> to RawBlindPooled<dyn TypeErasedTask>.
define_pooled_dyn_cast!(TypeErasedTask);

/// Allows a task to be referenced by the executor and its resources released, without knowing
/// the exact type of the task, only that it implements [`TypeErasedTask`].
///
/// This acts like a super-powered pointer and enforces no ownership semantics - the caller is
/// responsible for not using it once the task has been dropped.
#[derive(Copy, Clone, Debug)]
pub(crate) struct TaskRef {
    // We only use the pointer inside for implementing Eq/Hash, ignoring the pool ticket details.
    inner: RawBlindPooled<dyn TypeErasedTask>,
}

impl TaskRef {
    pub(crate) fn new(inner: RawBlindPooled<dyn TypeErasedTask>) -> Self {
        Self { inner }
    }

    /// For test purposes, we can create a fake instance.
    ///
    /// # Safety
    ///
    /// The "value" of the fake task reference is undefined. It is only for use as a
    /// placeholder and actually dereferencing it or accessing its contents is invalid.
    #[cfg(test)]
    pub(crate) unsafe fn fake() -> Self {
        use infinity_pool::RawBlindPool;

        use crate::MockTypeErasedTask;

        let mut fake_pool = RawBlindPool::new();
        let pool_ticket = fake_pool.insert(MockTypeErasedTask::new());

        // SAFETY: The object is still alive and no references to it exist, so it is valid to
        // create a shared reference to it for the purpose of casting.
        let pool_ticket = unsafe { pool_ticket.cast_type_erased_task() };

        Self {
            inner: pool_ticket.into_shared(),
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
        // SAFETY: Forwarding safety guarantees from the caller.
        let task = unsafe { self.inner.ptr().as_ref() };

        // SAFETY: Our tasks are always pinned, they just lose this metadata in their pointer form.
        unsafe { Pin::new_unchecked(task) }
    }

    /// Obtains a pool ticket that can be used to release the resources associated with this task.
    ///
    /// # Safety
    ///
    /// This may only be called on the same thread as the task was created on. That is, the
    /// `TaskRef` may be passed from thread to thread but has to end up back on its original
    /// thread to actually be used.
    #[must_use]
    pub(crate) unsafe fn into_pool_ticket(self) -> RawBlindPooled<dyn TypeErasedTask> {
        self.inner
    }
}

// SAFETY: It is permissible to send `TaskRef` between threads for the purpose of passing the
// reference around. However, methods must only be called on the original thread the task
// was created on.
unsafe impl Send for TaskRef {}

impl PartialEq for TaskRef {
    #[cfg_attr(test, mutants::skip)] // Liable to cause test timeouts, as collection logic gets wonky.
    fn eq(&self, other: &Self) -> bool {
        ptr::addr_eq(self.inner.ptr().as_ptr(), other.inner.ptr().as_ptr())
    }
}

impl Eq for TaskRef {}

impl Hash for TaskRef {
    #[cfg_attr(test, mutants::skip)] // Liable to cause test timeouts, as collection logic gets wonky.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.ptr().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use std::hash::DefaultHasher;

    use infinity_pool::RawBlindPool;

    use super::*;
    use crate::MockTypeErasedTask;

    #[test]
    fn same_task_refs_eq() {
        let mut pool = RawBlindPool::new();
        let ticket1 = pool.insert(MockTypeErasedTask::new());

        // SAFETY: We just inserted the item, so it is guaranteed to be alive and valid for
        // shared referencing, as no other references exist.
        let ticket1 = unsafe { ticket1.cast_type_erased_task() };

        let task_ref1 = TaskRef::new(ticket1.into_shared());
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
    }

    #[test]
    fn different_task_refs_not_eq() {
        let mut pool = RawBlindPool::new();
        let ticket1 = pool.insert(MockTypeErasedTask::new());
        let ticket2 = pool.insert(MockTypeErasedTask::new());

        // SAFETY: We just inserted the item, so it is guaranteed to be alive and valid for
        // shared referencing, as no other references exist.
        let ticket1 = unsafe { ticket1.cast_type_erased_task() };

        // SAFETY: We just inserted the item, so it is guaranteed to be alive and valid for
        // shared referencing, as no other references exist.
        let ticket2 = unsafe { ticket2.cast_type_erased_task() };

        let task_ref1 = TaskRef::new(ticket1.into_shared());
        let task_ref2 = TaskRef::new(ticket2.into_shared());

        assert_ne!(task_ref1, task_ref2);
    }
}
