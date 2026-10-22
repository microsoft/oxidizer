// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::OnceLock;

use plurality::{Arc as PoolArc, coerce};

use crate::TypeErasedTask;

/// Stores an immutable, uncounted handle template inside its pinned task.
///
/// Counted handles and owned raw pointers keep the task alive; the template is never dropped.
/// This keeps raw ownership private without a detached allocation or new plurality APIs.
pub(crate) type TaskHandleSlot = OnceLock<ManuallyDrop<TaskHandleTemplate>>;

pub(crate) struct TaskHandleTemplate {
    inner: Pin<PoolArc<dyn TypeErasedTask>>,
}

impl std::fmt::Debug for TaskHandleTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHandleTemplate").finish_non_exhaustive()
    }
}

/// A counted owner of a pinned, type-erased task allocation.
///
/// Clones retain the pinned pooled task allocation. Task methods remain owner-thread-only even
/// when a waker or remote notification carries a strong reference across threads.
/// Each handle directly owns a pinned pooled Arc; raw owners use the embedded handle template.
#[derive(Clone)]
pub(crate) struct TaskHandle {
    inner: Pin<PoolArc<dyn TypeErasedTask>>,
    _owner_thread: PhantomData<Rc<()>>,
}

impl TaskHandle {
    pub(crate) fn new<T: TypeErasedTask + 'static>(inner: Pin<PoolArc<T>>) -> Self {
        let handle = Self {
            inner: PoolArc::unsize_pin(inner, coerce!(dyn TypeErasedTask)),
            _owner_thread: PhantomData,
        };
        // SAFETY: This uncounted copy is immutable and never dropped. Only raw ownership
        // transitions may copy it into an owner, transferring an already counted reference.
        // The handle contains only a pinned Global-allocator pooled Arc, not per-handle resources.
        let template = ManuallyDrop::new(TaskHandleTemplate {
            // SAFETY: The immutable template is never dropped and represents no additional owner.
            inner: unsafe { ptr::read(&raw const handle.inner) },
        });
        let initialized = handle.inner.as_ref().get_ref().handle_slot().set(template);
        assert!(initialized.is_ok(), "task ownership must be initialized exactly once");
        handle
    }

    pub(crate) fn as_ptr(&self) -> TaskPtr {
        let template = self
            .inner
            .as_ref()
            .get_ref()
            .handle_slot()
            .get()
            .expect("task ownership is initialized before publishing task pointers");
        TaskPtr {
            template: NonNull::from(&**template),
        }
    }

    pub(crate) fn into_raw(self) -> TaskPtr {
        let pointer = self.as_ptr();
        std::mem::forget(self);
        pointer
    }

    /// # Safety
    ///
    /// The caller must access the task only on its creating thread and must not poll or abort
    /// it reentrantly while its payload is borrowed. This owner keeps the task alive for the borrow.
    #[must_use]
    pub(crate) unsafe fn as_task(&self) -> Pin<&dyn TypeErasedTask> {
        self.inner.as_ref()
    }
}

/// Non-owning identity for a task's immutable handle template.
///
/// Copying a pointer never acquires ownership. A counted strong reference must protect every
/// dereference; raw wakers and the intrusive list separately account for their owned references.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TaskPtr {
    template: NonNull<TaskHandleTemplate>,
}

impl TaskPtr {
    /// # Safety
    ///
    /// An existing strong reference must keep the initialized task alive throughout this call.
    pub(crate) unsafe fn increment_strong(self) {
        // SAFETY: The caller keeps the initialized task alive while acquiring the new reference.
        // Forgetting this owner transfers its counted reference to the caller.
        std::mem::forget(unsafe { self.retain_remote() });
    }

    /// # Safety
    ///
    /// The caller must transfer exactly one owned strong reference to a live, pinned task.
    pub(crate) unsafe fn into_handle(self) -> TaskHandle {
        // SAFETY: The transferred strong reference keeps the immutable template alive for this
        // read. The copy receives that reference, not an additional reference, and no borrow into
        // the allocation is created before the returned owner can reclaim it.
        TaskHandle {
            // SAFETY: The raw reference being transferred keeps the immutable template live.
            inner: unsafe { ptr::read(&raw const self.template.as_ref().inner) },
            _owner_thread: PhantomData,
        }
    }

    /// # Safety
    ///
    /// The caller must keep the initialized task alive for the borrow and access it only on
    /// its creating thread, without violating the task's payload and intrusive-link aliasing rules.
    #[must_use]
    pub(crate) unsafe fn as_task(&self) -> Pin<&dyn TypeErasedTask> {
        // SAFETY: The caller keeps the immutable template and task alive for the returned borrow
        // and guarantees owner-thread-only payload and link access.
        unsafe { self.template.as_ref() }.inner.as_ref()
    }

    /// # Safety
    ///
    /// An existing strong reference must keep the initialized task alive throughout this call.
    pub(crate) unsafe fn retain(self) -> TaskHandle {
        // SAFETY: The caller keeps the immutable template alive while cloning its handle.
        TaskHandle {
            inner: unsafe { self.template.as_ref() }.inner.clone(),
            _owner_thread: PhantomData,
        }
    }

    /// # Safety
    ///
    /// An existing strong reference must keep the initialized task alive throughout this call.
    pub(crate) unsafe fn retain_remote(self) -> RemoteTaskHandle {
        // SAFETY: The caller keeps the immutable template alive while cloning its handle.
        RemoteTaskHandle {
            inner: unsafe { self.template.as_ref() }.inner.clone(),
        }
    }

    /// # Safety
    ///
    /// The caller must transfer exactly one owned strong reference to a live, pinned task.
    pub(crate) unsafe fn into_remote(self) -> RemoteTaskHandle {
        RemoteTaskHandle {
            // SAFETY: The transferred reference keeps the immutable template live.
            inner: unsafe { ptr::read(&raw const self.template.as_ref().inner) },
        }
    }
}

#[derive(Clone)]
pub(crate) struct RemoteTaskHandle {
    inner: Pin<PoolArc<dyn TypeErasedTask>>,
}

impl RemoteTaskHandle {
    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> TaskPtr {
        let template = self
            .inner
            .as_ref()
            .get_ref()
            .handle_slot()
            .get()
            .expect("task ownership is initialized before publishing task pointers");
        TaskPtr {
            template: NonNull::from(&**template),
        }
    }

    /// # Safety
    ///
    /// The caller must be the task's owner thread and must not violate payload aliasing rules.
    pub(crate) unsafe fn as_task(&self) -> Pin<&dyn TypeErasedTask> {
        self.inner.as_ref()
    }

    /// # Safety
    ///
    /// The caller must be the task's owner thread.
    pub(crate) unsafe fn into_owner(self) -> TaskHandle {
        TaskHandle {
            inner: self.inner,
            _owner_thread: PhantomData,
        }
    }
}

// SAFETY: Remote threads may read the immutable template for reference-count operations while
// a counted owner keeps it live. Task payload and link access remains an unsafe owner-thread operation.
unsafe impl Send for TaskPtr {}
// SAFETY: Same invariant as `Send`.
unsafe impl Sync for TaskPtr {}

impl PartialEq for TaskPtr {
    fn eq(&self, other: &Self) -> bool {
        self.template == other.template
    }
}

impl Eq for TaskPtr {}

impl std::fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TaskHandle").field(&self.as_ptr()).finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;
    use std::thread;

    use plurality::MultiPool;
    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::allocate_test_task_in;

    #[test]
    fn owner_handle_is_thread_affine() {
        assert_not_impl_any!(TaskHandle: Send, Sync);
    }

    #[test]
    fn raw_ownership_round_trip_preserves_identity() {
        let pool = MultiPool::new();
        let task_handle = allocate_test_task_in(&pool);
        let pointer = task_handle.as_ptr();
        let owned = task_handle.into_raw();
        assert_eq!(owned, pointer);
        // SAFETY: `into_raw` transferred exactly one strong reference.
        drop(unsafe { owned.into_handle() });
        assert!(pool.is_empty());
    }

    #[test]
    fn embedded_handle_debug_stops_at_the_template_pointer() {
        let pool = MultiPool::new();
        let handle = allocate_test_task_in(&pool);

        assert_eq!(format!("{handle:?}"), format!("TaskHandle({:?})", handle.as_ptr()));

        drop(handle);
        assert!(pool.is_empty());
    }

    #[test]
    fn mixed_owners_release_the_allocation_exactly_once() {
        let pool = MultiPool::new();
        let original = allocate_test_task_in(&pool);
        let cloned = original.clone();
        let owned = cloned.into_raw();
        // SAFETY: `owned` still accounts for the reference transferred from `cloned`.
        let retained = unsafe { owned.retain() };

        drop(original);
        assert_eq!(pool.len(), 1);
        // SAFETY: This consumes the reference transferred from `cloned` exactly once.
        drop(unsafe { owned.into_handle() });
        assert_eq!(pool.len(), 1);
        assert_eq!(retained.as_ptr(), owned);
        drop(retained);
        assert!(pool.is_empty());
    }

    #[test]
    fn remote_template_reads_race_with_owner_retirement() {
        for _ in 0..4 {
            let pool = MultiPool::new();
            let original = allocate_test_task_in(&pool);
            let pointer = original.as_ptr();
            // SAFETY: `original` keeps the initialized template alive while retaining owners.
            let remote = [unsafe { pointer.retain_remote() }, unsafe { pointer.retain_remote() }];
            let barrier = Barrier::new(3);

            thread::scope(|scope| {
                for task in remote {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        let pointer = task.as_ptr();
                        let retained = task.clone();
                        drop(task);
                        assert_eq!(retained.as_ptr(), pointer);
                        drop(retained);
                    });
                }
                barrier.wait();
                // SAFETY: The original owner retires the non-Send payload on its creating thread.
                unsafe { original.as_task() }.abort();
                drop(original);
            });

            assert!(pool.is_empty());
        }
    }

    #[test]
    fn different_task_pointers_are_not_equal() {
        let pool = MultiPool::new();
        let task_handle1 = allocate_test_task_in(&pool);
        let task_handle2 = allocate_test_task_in(&pool);

        assert_ne!(task_handle1.as_ptr(), task_handle2.as_ptr());

        drop((task_handle1, task_handle2));
    }
}
