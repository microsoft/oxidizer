// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{TaskPointer, TaskRef};

/// Intrusive owner-thread inventory of all pending tasks.
#[derive(Default)]
pub(crate) struct TaskList {
    head: Option<TaskPointer>,
    tail: Option<TaskPointer>,
}

impl TaskList {
    pub(crate) fn insert(&mut self, task_ref: TaskRef) {
        let pointer = task_ref.pointer();
        // SAFETY: `task_ref` keeps the pinned allocation alive on its owner thread.
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

        let owned = task_ref.into_owned_pointer();
        debug_assert_eq!(owned, pointer);
    }

    pub(crate) fn remove(&mut self, pointer: TaskPointer) -> Option<TaskRef> {
        // SAFETY: Callers provide either a runnable strong reference or a pointer owned by this
        // list, so the allocation is live and accessed on its owner thread.
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
        Some(unsafe { pointer.into_task_ref() })
    }

    pub(crate) fn pop_front(&mut self) -> Option<TaskRef> {
        self.head.and_then(|head| self.remove(head))
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
    use std::future::pending;

    use events_once::LocalEventPool;
    use plurality::MultiPool;

    use super::*;
    use crate::Task;

    fn task(pool: &MultiPool) -> TaskRef {
        let (sender, _receiver) = LocalEventPool::<()>::new().rent();
        TaskRef::new(pool.alloc_arc_pin(Task::new(pending(), sender)))
    }

    #[test]
    fn insertion_removal_and_drain_preserve_ownership_order() {
        let pool = MultiPool::new();
        let first = task(&pool);
        let first_pointer = first.pointer();
        let second = task(&pool);
        let second_pointer = second.pointer();
        let mut list = TaskList::default();

        list.insert(first);
        list.insert(second);
        assert!(!list.is_empty());

        let removed = list.remove(first_pointer).unwrap();
        assert_eq!(removed.pointer(), first_pointer);
        drop(removed);

        let remaining = list.pop_front().unwrap();
        assert_eq!(remaining.pointer(), second_pointer);
        drop(remaining);
        assert!(list.is_empty());
        assert!(pool.is_empty());
    }
}
