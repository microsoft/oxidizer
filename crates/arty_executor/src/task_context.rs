// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use seismograph::recorder::runtime::TaskId;

thread_local! {
    static CURRENT_TASK: Cell<Option<TaskId>> = const { Cell::new(None) };
}

/// Returns the observed task that currently runs on this thread.
#[must_use]
pub fn current_task() -> Option<TaskId> {
    CURRENT_TASK.with(Cell::get)
}

/// Enters an observed task scope on the current thread.
pub fn enter_task(task: TaskId) -> TaskContextGuard {
    TaskContextGuard {
        previous: CURRENT_TASK.with(|current| current.replace(Some(task))),
        _not_send: PhantomData,
    }
}

/// Scope that restores the previous observed task when dropped.
#[derive(Debug)]
#[must_use]
pub struct TaskContextGuard {
    previous: Option<TaskId>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for TaskContextGuard {
    fn drop(&mut self) {
        _ = CURRENT_TASK.try_with(|current| current.set(self.previous));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static_assertions::assert_not_impl_any!(TaskContextGuard: Send, Sync);

    #[test]
    fn nested_scopes_restore_task_context() {
        let parent = seismograph_runtime::allocate_task_id();
        let child = seismograph_runtime::allocate_task_id();
        assert_eq!(current_task(), None);
        {
            let _parent = enter_task(parent);
            {
                let _child = enter_task(child);
                assert_eq!(current_task(), Some(child));
            }
            assert_eq!(current_task(), Some(parent));
        }
        assert_eq!(current_task(), None);
    }
}
