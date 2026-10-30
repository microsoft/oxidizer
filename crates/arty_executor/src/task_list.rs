// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::{TaskHandle, TaskPtr};

/// Intrusive owner-thread inventory of all pending tasks.
#[derive(Default)]
pub(crate) struct TaskList {
    head: Option<TaskPtr>,
    tail: Option<TaskPtr>,
    _owner_thread: PhantomData<Rc<()>>,
}

impl TaskList {
    #[cfg(feature = "inspection")]
    pub(crate) fn inspect_until(&self, mut visit: impl FnMut(TaskPtr) -> bool) {
        let mut next = self.head;
        while let Some(task) = next {
            // SAFETY: The list owns these live tasks. Inspection cannot mutate the
            // list, and a cycle cannot run while its owner borrows it for inspection.
            next = unsafe { task.as_task() }.links().next.get();
            if !visit(task) {
                break;
            }
        }
    }

    pub(crate) fn insert(&mut self, task_handle: TaskHandle) -> TaskPtr {
        let pointer = task_handle.as_ptr();
        // SAFETY: `task_handle` keeps the pinned allocation alive on its owner thread.
        let links = unsafe { pointer.as_task() }.get_ref().links();
        assert!(!links.linked.replace(true), "a task may be linked only once");
        links.previous.set(self.tail);
        links.next.set(None);

        if let Some(tail) = self.tail {
            // SAFETY: Every linked neighbor is owned by this list and accessed on the owner.
            unsafe { tail.as_task() }.get_ref().links().next.set(Some(pointer));
        } else {
            self.head = Some(pointer);
        }
        self.tail = Some(pointer);

        let owned = task_handle.into_raw();
        debug_assert_eq!(owned, pointer);
        pointer
    }

    /// # Safety
    ///
    /// The caller must keep `pointer` live on the task's owner thread. If the task is linked,
    /// it must belong to this list.
    pub(crate) unsafe fn remove(&mut self, pointer: TaskPtr) -> Option<TaskHandle> {
        // SAFETY: The caller guarantees that the pointer is live and accessed on its owner thread.
        let links = unsafe { pointer.as_task() }.get_ref().links();
        if !links.linked.replace(false) {
            return None;
        }

        let previous = links.previous.replace(None);
        let next = links.next.replace(None);
        if let Some(previous) = previous {
            // SAFETY: Linked neighbors remain owned by the list.
            unsafe { previous.as_task() }.get_ref().links().next.set(next);
        } else {
            self.head = next;
        }
        if let Some(next) = next {
            // SAFETY: Linked neighbors remain owned by the list.
            unsafe { next.as_task() }.get_ref().links().previous.set(previous);
        } else {
            self.tail = previous;
        }
        // SAFETY: Insertion transferred exactly one raw strong reference to the list, and this
        // successful unlink returns that ownership exactly once.
        Some(unsafe { pointer.into_handle() })
    }

    pub(crate) fn pop_front(&mut self) -> Option<TaskHandle> {
        // SAFETY: The head is a live strong reference owned by this list on its owner thread.
        self.head.and_then(|head| unsafe { self.remove(head) })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.head.is_none()
    }
}

impl std::fmt::Debug for TaskList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskList").finish_non_exhaustive()
    }
}

impl Drop for TaskList {
    fn drop(&mut self) {
        while self.pop_front().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use plurality::MultiPool;
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn task_list_is_thread_affine() {
        assert_not_impl_any!(TaskList: Send, Sync);
    }

    #[test]
    fn insertion_removal_and_drop_preserve_ownership() {
        let pool = MultiPool::new();
        let first = crate::allocate_test_task_in(&pool);
        let first_pointer = first.as_ptr();
        let second = crate::allocate_test_task_in(&pool);
        let second_pointer = second.as_ptr();
        let mut list = TaskList::default();

        assert_eq!(list.insert(first), first_pointer);
        assert_eq!(list.insert(second), second_pointer);
        assert_eq!(format!("{list:?}"), "TaskList { .. }");
        // SAFETY: `first_pointer` is live and belongs to `list`.
        drop(unsafe { list.remove(first_pointer) }.unwrap());
        assert_eq!(list.pop_front().unwrap().as_ptr(), second_pointer);
        assert!(list.is_empty());

        _ = list.insert(crate::allocate_test_task_in(&pool));
        drop(list);
        assert!(pool.is_empty());
    }
}
